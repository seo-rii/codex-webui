use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicU64, Ordering},
        mpsc as std_mpsc,
    },
    thread,
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    runtime::{Builder as TokioRuntimeBuilder, Handle as TokioRuntimeHandle},
    sync::{Mutex, broadcast, oneshot},
    task::JoinHandle,
    time::timeout,
};
use tracing::{info, warn};

const APP_SERVER_THREAD_STACK_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppServerProfile {
    pub id: String,
    pub codex_home: PathBuf,
}

#[derive(Clone, Debug)]
pub struct AppServerClientConfig {
    pub codex_bin: String,
    pub client_name: String,
    pub client_title: String,
    pub client_version: String,
    pub stderr_log_path: Option<PathBuf>,
    pub extra_env: HashMap<String, String>,
    pub controller_threads: usize,
    pub request_timeout: Duration,
    pub handoff_dir: Option<PathBuf>,
}

impl Default for AppServerClientConfig {
    fn default() -> Self {
        Self {
            codex_bin: "codex".to_string(),
            client_name: "codex_webui".to_string(),
            client_title: "Codex Web UI".to_string(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
            stderr_log_path: None,
            extra_env: HashMap::new(),
            controller_threads: default_controller_thread_count(),
            request_timeout: default_request_timeout(),
            handoff_dir: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AppServerNotification {
    pub method: String,
    pub params: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AppServerRequest {
    pub id: Value,
    pub method: String,
    pub params: Value,
}

#[derive(Clone)]
pub struct AppServerClient {
    inner: Arc<AppServerClientInner>,
}

#[derive(Clone)]
pub struct AppServerManager {
    config: AppServerClientConfig,
    clients: Arc<Mutex<HashMap<String, AppServerClient>>>,
    controller: Arc<AppServerControllerRuntime>,
}

struct AppServerClientInner {
    profile: AppServerProfile,
    config: AppServerClientConfig,
    controller: Arc<AppServerControllerRuntime>,
    start_lock: Mutex<()>,
    process: Mutex<Option<ProcessState>>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>,
    next_request_id: AtomicU64,
    notifications_tx: broadcast::Sender<AppServerNotification>,
    requests_tx: broadcast::Sender<AppServerRequest>,
}

struct ProcessState {
    stdin: Arc<Mutex<ChildStdin>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    join_handle: JoinHandle<()>,
}

#[derive(Clone, Debug)]
struct HandoffPaths {
    socket_path: PathBuf,
    meta_path: PathBuf,
    log_path: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct HandoffMeta {
    pid: u32,
    profile_id: String,
    socket_path: String,
    codex_bin: String,
    codex_home: String,
    started_at_ms: u128,
}

struct AppServerControllerRuntime {
    handle: TokioRuntimeHandle,
    shutdown_tx: StdMutex<Option<oneshot::Sender<()>>>,
}

#[derive(Debug, PartialEq)]
enum IncomingMessage {
    Response {
        id: u64,
        payload: Result<Value, String>,
    },
    Notification(AppServerNotification),
    Request(AppServerRequest),
}

impl AppServerClient {
    pub fn new(profile: AppServerProfile, config: AppServerClientConfig) -> Self {
        let controller = AppServerControllerRuntime::new(config.controller_threads);
        Self::with_controller(profile, config, controller)
    }

    fn with_controller(
        profile: AppServerProfile,
        config: AppServerClientConfig,
        controller: Arc<AppServerControllerRuntime>,
    ) -> Self {
        let (notifications_tx, _) = broadcast::channel(8192);
        let (requests_tx, _) = broadcast::channel(1024);

        Self {
            inner: Arc::new(AppServerClientInner {
                profile,
                config,
                controller,
                start_lock: Mutex::new(()),
                process: Mutex::new(None),
                pending: Mutex::new(HashMap::new()),
                next_request_id: AtomicU64::new(1),
                notifications_tx,
                requests_tx,
            }),
        }
    }

    pub fn subscribe_notifications(&self) -> broadcast::Receiver<AppServerNotification> {
        self.inner.notifications_tx.subscribe()
    }

    pub fn subscribe_requests(&self) -> broadcast::Receiver<AppServerRequest> {
        self.inner.requests_tx.subscribe()
    }

    pub async fn request(&self, method: impl Into<String>, params: Value) -> Result<Value> {
        let client = self.clone();
        let method = method.into();
        self.inner
            .controller
            .handle
            .spawn(async move { client.request_on_controller(method, params).await })
            .await
            .context("codex app-server controller task failed")?
    }

    pub async fn respond(&self, id: Value, result: Value) -> Result<()> {
        let client = self.clone();
        self.inner
            .controller
            .handle
            .spawn(async move { client.respond_on_controller(id, result).await })
            .await
            .context("codex app-server controller task failed")?
    }

    pub async fn reject(&self, id: Value, message: impl Into<String>) -> Result<()> {
        let client = self.clone();
        let message = message.into();
        self.inner
            .controller
            .handle
            .spawn(async move { client.reject_on_controller(id, message).await })
            .await
            .context("codex app-server controller task failed")?
    }

    pub async fn close(&self) -> Result<()> {
        let client = self.clone();
        self.inner
            .controller
            .handle
            .spawn(async move { client.close_on_controller().await })
            .await
            .context("codex app-server controller task failed")?
    }

    async fn request_on_controller(&self, method: String, params: Value) -> Result<Value> {
        self.ensure_started().await?;
        self.request_started(method, params).await
    }

    async fn respond_on_controller(&self, id: Value, result: Value) -> Result<()> {
        self.ensure_started().await?;
        self.write_message(&json!({
            "id": id,
            "result": result
        }))
        .await
    }

    async fn reject_on_controller(&self, id: Value, message: String) -> Result<()> {
        self.ensure_started().await?;
        self.write_message(&json!({
            "id": id,
            "error": {
                "code": -32000,
                "message": message
            }
        }))
        .await
    }

    async fn close_on_controller(&self) -> Result<()> {
        let process = self.inner.process.lock().await.take();
        if let Some(mut process) = process {
            if let Some(shutdown_tx) = process.shutdown_tx.take() {
                let _ = shutdown_tx.send(());
            }
            let _ = process.join_handle.await;
        }
        Ok(())
    }

    async fn ensure_started(&self) -> Result<()> {
        if self.inner.process.lock().await.is_some() {
            return Ok(());
        }

        let _guard = self.inner.start_lock.lock().await;
        if self.inner.process.lock().await.is_some() {
            return Ok(());
        }

        let stdin = self.spawn_process().await?;
        {
            let mut process = self.inner.process.lock().await;
            *process = Some(stdin);
        }

        if let Err(error) = async {
            self.request_started(
                "initialize".to_string(),
                json!({
                "clientInfo": {
                        "name": self.inner.config.client_name.clone(),
                        "title": self.inner.config.client_title.clone(),
                        "version": self.inner.config.client_version.clone()
                    },
                    "capabilities": {
                        "experimentalApi": true
                    }
                }),
            )
            .await?;
            self.write_message(&json!({
                "method": "initialized",
                "params": {}
            }))
            .await
        }
        .await
        {
            let _ = self.close_on_controller().await;
            return Err(error);
        }

        Ok(())
    }

    async fn request_started(&self, method: String, params: Value) -> Result<Value> {
        let id = self.inner.next_request_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.inner.pending.lock().await.insert(id, tx);
        let method_name = method.clone();

        if let Err(error) = self
            .write_message(&json!({
                "id": id,
                "method": method,
                "params": params
            }))
            .await
        {
            self.inner.pending.lock().await.remove(&id);
            return Err(error);
        }

        match timeout(self.inner.config.request_timeout, rx).await {
            Err(_) => {
                self.inner.pending.lock().await.remove(&id);
                Err(anyhow!(
                    "codex app-server request timed out after {}s: {}",
                    self.inner.config.request_timeout.as_secs(),
                    method_name
                ))
            }
            Ok(result) => match result {
                Ok(Ok(value)) => Ok(value),
                Ok(Err(message)) => Err(anyhow!(message)),
                Err(_) => Err(anyhow!(
                    "codex app-server request channel closed before a response arrived"
                )),
            },
        }
    }

    async fn write_message(&self, payload: &Value) -> Result<()> {
        let stdin = {
            let process = self.inner.process.lock().await;
            process
                .as_ref()
                .map(|state| state.stdin.clone())
                .ok_or_else(|| anyhow!("codex app-server is not running"))?
        };

        let mut writer = stdin.lock().await;
        let encoded =
            serde_json::to_string(payload).context("failed to encode app-server message")?;
        writer
            .write_all(encoded.as_bytes())
            .await
            .context("failed to write app-server message")?;
        writer
            .write_all(b"\n")
            .await
            .context("failed to terminate app-server message")?;
        writer
            .flush()
            .await
            .context("failed to flush app-server stdin")
    }

    async fn spawn_process(&self) -> Result<ProcessState> {
        let mut command = Command::new(&self.inner.config.codex_bin);
        if let Some(handoff_paths) = self.ensure_handoff_server_running().await? {
            command
                .arg("app-server")
                .arg("proxy")
                .arg("--sock")
                .arg(&handoff_paths.socket_path);
        } else {
            command.arg("app-server").arg("--listen").arg("stdio://");
        }

        command
            .env("CODEX_HOME", &self.inner.profile.codex_home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        for (key, value) in &self.inner.config.extra_env {
            command.env(key, value);
        }

        let mut child = command
            .spawn()
            .with_context(|| format!("failed to spawn {}", self.inner.config.codex_bin))?;
        let stdin =
            Arc::new(Mutex::new(child.stdin.take().ok_or_else(|| {
                anyhow!("failed to capture codex app-server stdin")
            })?));
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("failed to capture codex app-server stdout"))?;
        let stderr = child.stderr.take();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let inner = self.inner.clone();

        let join_handle = tokio::spawn(async move {
            supervise_process(inner, child, stdout, stderr, shutdown_rx).await;
        });

        Ok(ProcessState {
            stdin,
            shutdown_tx: Some(shutdown_tx),
            join_handle,
        })
    }

    async fn ensure_handoff_server_running(&self) -> Result<Option<HandoffPaths>> {
        let Some(paths) = handoff_paths(&self.inner.config, &self.inner.profile) else {
            return Ok(None);
        };

        #[cfg(not(unix))]
        {
            let _ = paths;
            return Ok(None);
        }

        #[cfg(unix)]
        {
            if handoff_socket_is_live(&paths.socket_path).await {
                return Ok(Some(paths));
            }

            if let Some(parent) = paths.socket_path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }
            if let Some(parent) = paths.meta_path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }
            let _ = tokio::fs::remove_file(&paths.socket_path).await;

            let log = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&paths.log_path)
                .with_context(|| format!("failed to open {}", paths.log_path.display()))?;
            let log_for_stderr = log
                .try_clone()
                .with_context(|| format!("failed to clone {}", paths.log_path.display()))?;

            let mut command = Command::new(&self.inner.config.codex_bin);
            command
                .arg("app-server")
                .arg("--listen")
                .arg(format!("unix://{}", paths.socket_path.display()))
                .env("CODEX_HOME", &self.inner.profile.codex_home)
                .stdin(Stdio::null())
                .stdout(Stdio::from(log))
                .stderr(Stdio::from(log_for_stderr));

            for (key, value) in &self.inner.config.extra_env {
                command.env(key, value);
            }

            #[cfg(unix)]
            {
                command.process_group(0);
            }

            let child = command.spawn().with_context(|| {
                format!(
                    "failed to spawn persistent {} app-server",
                    self.inner.config.codex_bin
                )
            })?;
            let pid = child.id().unwrap_or_default();
            drop(child);

            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            while tokio::time::Instant::now() < deadline {
                if handoff_socket_is_live(&paths.socket_path).await {
                    let meta = HandoffMeta {
                        pid,
                        profile_id: self.inner.profile.id.clone(),
                        socket_path: paths.socket_path.display().to_string(),
                        codex_bin: self.inner.config.codex_bin.clone(),
                        codex_home: self.inner.profile.codex_home.display().to_string(),
                        started_at_ms: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis(),
                    };
                    let encoded = serde_json::to_vec_pretty(&meta)
                        .context("failed to encode app-server handoff metadata")?;
                    tokio::fs::write(&paths.meta_path, encoded)
                        .await
                        .with_context(|| {
                            format!("failed to write {}", paths.meta_path.display())
                        })?;
                    info!(
                        profile_id = %self.inner.profile.id,
                        pid,
                        socket = %paths.socket_path.display(),
                        "started persistent codex app-server for restart handoff"
                    );
                    return Ok(Some(paths));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }

            Err(anyhow!(
                "persistent codex app-server did not open {}",
                paths.socket_path.display()
            ))
        }
    }
}

impl AppServerManager {
    pub fn new(config: AppServerClientConfig) -> Self {
        let controller = AppServerControllerRuntime::new(config.controller_threads);
        Self {
            config,
            clients: Arc::new(Mutex::new(HashMap::new())),
            controller,
        }
    }

    pub async fn get_or_create(&self, profile: AppServerProfile) -> AppServerClient {
        let mut clients = self.clients.lock().await;
        if let Some(existing) = clients.get(&profile.id) {
            return existing.clone();
        }

        let client = AppServerClient::with_controller(
            profile.clone(),
            self.config.clone(),
            Arc::clone(&self.controller),
        );
        clients.insert(profile.id, client.clone());
        client
    }

    pub async fn close_profile(&self, profile_id: &str) -> Result<()> {
        let client = self.clients.lock().await.remove(profile_id);
        if let Some(client) = client {
            let profile = client.inner.profile.clone();
            client.close().await?;
            stop_handoff_server(&self.config, &profile).await?;
        }
        Ok(())
    }

    pub async fn client_count(&self) -> usize {
        self.clients.lock().await.len()
    }

    pub async fn close_all(&self) -> Result<()> {
        let clients = {
            let mut clients = self.clients.lock().await;
            clients
                .drain()
                .map(|(_, client)| client)
                .collect::<Vec<_>>()
        };

        for client in clients {
            let profile = client.inner.profile.clone();
            client.close().await?;
            stop_handoff_server(&self.config, &profile).await?;
        }
        Ok(())
    }

    pub async fn detach_all(&self) -> Result<()> {
        let clients = {
            let mut clients = self.clients.lock().await;
            clients
                .drain()
                .map(|(_, client)| client)
                .collect::<Vec<_>>()
        };

        for client in clients {
            client.close().await?;
        }
        Ok(())
    }
}

fn handoff_paths(
    config: &AppServerClientConfig,
    profile: &AppServerProfile,
) -> Option<HandoffPaths> {
    let handoff_dir = config.handoff_dir.as_ref()?;
    #[cfg(not(unix))]
    {
        let _ = profile;
        let _ = handoff_dir;
        return None;
    }
    #[cfg(unix)]
    {
        let mut hasher = Sha256::new();
        hasher.update(profile.id.as_bytes());
        hasher.update(b"\0");
        hasher.update(profile.codex_home.display().to_string().as_bytes());
        hasher.update(b"\0");
        hasher.update(handoff_dir.display().to_string().as_bytes());
        let digest = hasher.finalize();
        let suffix = digest
            .iter()
            .take(8)
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let safe_profile = profile
            .id
            .chars()
            .map(|ch| match ch {
                'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '_' | '-' => ch,
                _ => '-',
            })
            .collect::<String>()
            .trim_matches('-')
            .to_string();
        let safe_profile = if safe_profile.is_empty() {
            "default".to_string()
        } else {
            safe_profile
        };
        let socket_path = env::temp_dir()
            .join("codex-webui-app-server")
            .join(format!("{suffix}.sock"));
        Some(HandoffPaths {
            socket_path,
            meta_path: handoff_dir.join(format!("{safe_profile}-{suffix}.json")),
            log_path: handoff_dir.join(format!("{safe_profile}-{suffix}.log")),
        })
    }
}

#[cfg(unix)]
async fn handoff_socket_is_live(socket_path: &Path) -> bool {
    use tokio::net::UnixStream;

    timeout(Duration::from_millis(250), UnixStream::connect(socket_path))
        .await
        .is_ok_and(|result| result.is_ok())
}

#[cfg(unix)]
async fn stop_handoff_server(
    config: &AppServerClientConfig,
    profile: &AppServerProfile,
) -> Result<()> {
    let Some(paths) = handoff_paths(config, profile) else {
        return Ok(());
    };
    let Some(meta) = tokio::fs::read(&paths.meta_path)
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice::<HandoffMeta>(&bytes).ok())
    else {
        let _ = tokio::fs::remove_file(&paths.socket_path).await;
        return Ok(());
    };

    if !handoff_socket_is_live(&paths.socket_path).await {
        let _ = tokio::fs::remove_file(&paths.socket_path).await;
        let _ = tokio::fs::remove_file(&paths.meta_path).await;
        return Ok(());
    }

    if meta.pid > 0 {
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(meta.pid.to_string())
            .status()
            .await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        if handoff_socket_is_live(&paths.socket_path).await {
            let _ = Command::new("kill")
                .arg("-KILL")
                .arg(meta.pid.to_string())
                .status()
                .await;
        }
    }

    let _ = tokio::fs::remove_file(&paths.socket_path).await;
    let _ = tokio::fs::remove_file(&paths.meta_path).await;
    Ok(())
}

#[cfg(not(unix))]
async fn stop_handoff_server(
    _config: &AppServerClientConfig,
    _profile: &AppServerProfile,
) -> Result<()> {
    Ok(())
}

impl AppServerControllerRuntime {
    fn new(worker_threads: usize) -> Arc<Self> {
        let worker_threads = worker_threads.max(1).min(16);
        let blocking_threads = worker_threads.saturating_mul(8).max(8);
        let (handle_tx, handle_rx) = std_mpsc::sync_channel(1);
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        thread::Builder::new()
            .name("codex-webui-codex-controller".to_string())
            .stack_size(APP_SERVER_THREAD_STACK_BYTES)
            .spawn(move || {
                let runtime = TokioRuntimeBuilder::new_multi_thread()
                    .enable_all()
                    .worker_threads(worker_threads)
                    .max_blocking_threads(blocking_threads)
                    .thread_name("codex-webui-codex-io")
                    .thread_stack_size(APP_SERVER_THREAD_STACK_BYTES)
                    .build()
                    .expect("failed to build codex app-server controller runtime");
                let handle = runtime.handle().clone();
                let _ = handle_tx.send(handle);
                runtime.block_on(async {
                    let _ = shutdown_rx.await;
                });
            })
            .expect("failed to start codex app-server controller thread");

        let handle = handle_rx
            .recv()
            .expect("codex app-server controller did not start");
        Arc::new(Self {
            handle,
            shutdown_tx: StdMutex::new(Some(shutdown_tx)),
        })
    }
}

impl Drop for AppServerControllerRuntime {
    fn drop(&mut self) {
        if let Ok(mut shutdown_tx) = self.shutdown_tx.lock() {
            if let Some(shutdown_tx) = shutdown_tx.take() {
                let _ = shutdown_tx.send(());
            }
        }
    }
}

fn default_controller_thread_count() -> usize {
    std::env::var("CODEX_WEBUI_CONTROLLER_THREADS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(2)
                .min(4)
                .max(2)
        })
        .clamp(1, 16)
}

fn default_request_timeout() -> Duration {
    let seconds = std::env::var("CODEX_WEBUI_APP_SERVER_TIMEOUT_SECONDS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(30)
        .clamp(5, 300);
    Duration::from_secs(seconds)
}

async fn supervise_process(
    inner: Arc<AppServerClientInner>,
    mut child: Child,
    stdout: ChildStdout,
    stderr: Option<tokio::process::ChildStderr>,
    mut shutdown_rx: oneshot::Receiver<()>,
) {
    let stdout_task = tokio::spawn(read_stdout(inner.clone(), stdout));
    let stderr_task = tokio::spawn(read_stderr(stderr, inner.config.stderr_log_path.clone()));

    let exit_result = tokio::select! {
        _ = &mut shutdown_rx => {
            let _ = child.kill().await;
            child.wait().await
        }
        result = child.wait() => result,
    };

    let _ = stdout_task.await;
    let _ = stderr_task.await;

    let reason = match exit_result {
        Ok(status) => format!("codex app-server exited ({status})"),
        Err(error) => format!("failed to wait for codex app-server exit: {error}"),
    };

    warn!(
        profile_id = %inner.profile.id,
        codex_home = %inner.profile.codex_home.display(),
        "{reason}"
    );
    fail_pending_requests(&inner, &reason).await;

    let mut process = inner.process.lock().await;
    *process = None;
}

async fn read_stdout(inner: Arc<AppServerClientInner>, stdout: ChildStdout) {
    let mut reader = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = reader.next_line().await {
        match classify_incoming_message(&line) {
            Ok(Some(IncomingMessage::Response { id, payload })) => {
                if let Some(sender) = inner.pending.lock().await.remove(&id) {
                    let _ = sender.send(payload);
                }
            }
            Ok(Some(IncomingMessage::Notification(notification))) => {
                let _ = inner.notifications_tx.send(notification);
            }
            Ok(Some(IncomingMessage::Request(request))) => {
                let _ = inner.requests_tx.send(request);
            }
            Ok(None) => {}
            Err(error) => {
                warn!(
                    profile_id = %inner.profile.id,
                    "failed to parse codex app-server stdout line: {error:#}; line={line}"
                );
            }
        }
    }
}

async fn read_stderr(
    stderr: Option<tokio::process::ChildStderr>,
    stderr_log_path: Option<PathBuf>,
) {
    let Some(stderr) = stderr else {
        return;
    };

    let mut reader = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = reader.next_line().await {
        let message = line.trim();
        if message.is_empty() {
            continue;
        }

        if let Some(path) = &stderr_log_path {
            append_text_log_line(path, message);
        }
        info!("[codex-app-server] {message}");
    }
}

async fn fail_pending_requests(inner: &Arc<AppServerClientInner>, reason: &str) {
    let pending = {
        let mut pending = inner.pending.lock().await;
        pending
            .drain()
            .map(|(_, sender)| sender)
            .collect::<Vec<_>>()
    };

    for sender in pending {
        let _ = sender.send(Err(reason.to_string()));
    }
}

fn append_text_log_line(path: &Path, message: &str) {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return;
    }

    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let line = format!("{trimmed}\n");
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = std::io::Write::write_all(&mut file, line.as_bytes());
    }
}

fn classify_incoming_message(line: &str) -> Result<Option<IncomingMessage>> {
    let payload: Value = serde_json::from_str(line).context("invalid json-rpc payload")?;
    let Some(object) = payload.as_object() else {
        return Ok(None);
    };

    if let Some(method) = object.get("method").and_then(Value::as_str) {
        let params = object.get("params").cloned().unwrap_or_else(|| json!({}));
        if let Some(id) = object.get("id").cloned() {
            return Ok(Some(IncomingMessage::Request(AppServerRequest {
                id,
                method: method.to_string(),
                params,
            })));
        }

        return Ok(Some(IncomingMessage::Notification(AppServerNotification {
            method: method.to_string(),
            params,
        })));
    }

    let Some(id) = object.get("id").and_then(Value::as_u64) else {
        return Ok(None);
    };

    if let Some(result) = object.get("result") {
        return Ok(Some(IncomingMessage::Response {
            id,
            payload: Ok(result.clone()),
        }));
    }

    if let Some(error) = object.get("error") {
        let has_structured_error = error.get("codexErrorInfo").is_some()
            || error.get("additionalDetails").is_some()
            || error.get("data").is_some_and(|data| {
                data.get("codexErrorInfo").is_some()
                    || data.get("additionalDetails").is_some()
                    || data.get("errorInfo").is_some()
            });
        let message = if has_structured_error {
            error.to_string()
        } else {
            error
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| "codex app-server returned an unknown error".to_string())
        };
        return Ok(Some(IncomingMessage::Response {
            id,
            payload: Err(message),
        }));
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::{
        AppServerClientConfig, AppServerNotification, AppServerProfile, AppServerRequest,
        IncomingMessage, classify_incoming_message, handoff_paths,
    };
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn classifies_notifications() {
        let payload =
            classify_incoming_message(r#"{"method":"session/updated","params":{"id":"abc"}}"#)
                .expect("line should parse");

        assert_eq!(
            payload,
            Some(IncomingMessage::Notification(AppServerNotification {
                method: "session/updated".to_string(),
                params: json!({ "id": "abc" }),
            }))
        );
    }

    #[test]
    fn classifies_server_requests() {
        let payload = classify_incoming_message(
            r#"{"id":"srv-1","method":"input/request","params":{"question":"Continue?"}}"#,
        )
        .expect("line should parse");

        assert_eq!(
            payload,
            Some(IncomingMessage::Request(AppServerRequest {
                id: json!("srv-1"),
                method: "input/request".to_string(),
                params: json!({ "question": "Continue?" }),
            }))
        );
    }

    #[test]
    fn classifies_responses() {
        let payload = classify_incoming_message(r#"{"id":7,"result":{"ok":true}}"#)
            .expect("line should parse");

        assert_eq!(
            payload,
            Some(IncomingMessage::Response {
                id: 7,
                payload: Ok(json!({ "ok": true })),
            })
        );
    }

    #[test]
    fn classifies_response_errors() {
        let payload =
            classify_incoming_message(r#"{"id":7,"error":{"code":-32000,"message":"boom"}}"#)
                .expect("line should parse");

        assert_eq!(
            payload,
            Some(IncomingMessage::Response {
                id: 7,
                payload: Err("boom".to_string()),
            })
        );
    }

    #[test]
    fn preserves_structured_response_errors() {
        let payload = classify_incoming_message(
            r#"{"id":7,"error":{"code":-32000,"message":"You've hit your usage limit.","codexErrorInfo":"usageLimitExceeded","additionalDetails":"{\"resetsAt\":1766664000}"}}"#,
        )
        .expect("line should parse");

        assert_eq!(
            payload,
            Some(IncomingMessage::Response {
                id: 7,
                payload: Err(json!({
                    "code": -32000,
                    "message": "You've hit your usage limit.",
                    "codexErrorInfo": "usageLimitExceeded",
                    "additionalDetails": "{\"resetsAt\":1766664000}"
                })
                .to_string()),
            })
        );
    }

    #[test]
    fn handoff_paths_are_stable_per_profile() {
        let config = AppServerClientConfig {
            handoff_dir: Some(PathBuf::from("/tmp/codex-webui-handoff-test")),
            ..AppServerClientConfig::default()
        };
        let profile = AppServerProfile {
            id: "default".to_string(),
            codex_home: PathBuf::from("/tmp/codex-home"),
        };

        let first = handoff_paths(&config, &profile);
        let second = handoff_paths(&config, &profile);

        #[cfg(unix)]
        {
            let first = first.expect("handoff should be available on unix");
            let second = second.expect("handoff should be available on unix");
            assert_eq!(first.socket_path, second.socket_path);
            assert!(first.socket_path.to_string_lossy().ends_with(".sock"));
            assert!(first.meta_path.to_string_lossy().contains("default"));
        }

        #[cfg(not(unix))]
        {
            assert!(first.is_none());
            assert!(second.is_none());
        }
    }
}
