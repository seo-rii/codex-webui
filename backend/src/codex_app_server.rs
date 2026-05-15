use std::{
    collections::HashMap,
    env,
    error::Error as StdError,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
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
    sync::{Mutex, OwnedSemaphorePermit, Semaphore, broadcast, oneshot},
    task::JoinHandle,
    time::timeout,
};
use tracing::{info, warn};

const APP_SERVER_THREAD_STACK_BYTES: usize = 4 * 1024 * 1024;
const APP_SERVER_REQUEST_TIMEOUT_DEFAULT_SECONDS: u64 = 600;
const APP_SERVER_REQUEST_TIMEOUT_MIN_SECONDS: u64 = 5;
const APP_SERVER_REQUEST_TIMEOUT_MAX_SECONDS: u64 = 7_200;

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
    pub max_processes: usize,
    pub request_timeout: Duration,
    pub startup_timeout: Duration,
    pub handoff_dir: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AppServerHandoffStatus {
    pub client_count: usize,
    pub active_process_count: usize,
    pub stdio_process_count: usize,
    pub handoff_proxy_process_count: usize,
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
            max_processes: default_max_process_count(),
            request_timeout: default_request_timeout(),
            startup_timeout: default_startup_timeout(),
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
    handoff_disabled_after_failure: AtomicBool,
    notifications_tx: broadcast::Sender<AppServerNotification>,
    requests_tx: broadcast::Sender<AppServerRequest>,
}

struct ProcessState {
    stdin: Arc<Mutex<ChildStdin>>,
    pid: Option<u32>,
    process_identity: Option<ManagedProcessIdentity>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    join_handle: JoinHandle<()>,
    handoff_proxy: bool,
    _process_slot: OwnedSemaphorePermit,
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
    #[serde(default)]
    process_identity: Option<ManagedProcessIdentity>,
    profile_id: String,
    socket_path: String,
    codex_bin: String,
    codex_home: String,
    started_at_ms: u128,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
struct ManagedProcessIdentity {
    pid: u32,
    process_group_id: u32,
    start_time_ticks: u64,
}

struct AppServerControllerRuntime {
    handle: TokioRuntimeHandle,
    shutdown_tx: StdMutex<Option<oneshot::Sender<()>>>,
    process_slots: Arc<Semaphore>,
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

#[derive(Debug)]
struct AppServerRequestTimeoutError {
    method: String,
    request_timeout: Duration,
    recovered: bool,
}

impl std::fmt::Display for AppServerRequestTimeoutError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "codex app-server request timed out after {}ms: {}",
            self.request_timeout.as_millis(),
            self.method
        )
    }
}

impl StdError for AppServerRequestTimeoutError {}

pub fn app_server_request_timed_out(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<AppServerRequestTimeoutError>()
        .is_some()
}

pub fn app_server_timeout_recovered(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<AppServerRequestTimeoutError>()
        .is_some_and(|error| error.recovered)
}

pub fn app_server_request_interrupted(error: &anyhow::Error) -> bool {
    let message = error.to_string();
    message.contains("codex app-server exited")
        || message.contains("codex app-server request channel closed")
}

impl AppServerClient {
    pub fn new(profile: AppServerProfile, config: AppServerClientConfig) -> Self {
        let controller =
            AppServerControllerRuntime::new(config.controller_threads, config.max_processes);
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
                handoff_disabled_after_failure: AtomicBool::new(false),
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

    pub async fn request_with_timeout(
        &self,
        method: impl Into<String>,
        params: Value,
        request_timeout: Duration,
        recover_handoff_on_timeout: bool,
    ) -> Result<Value> {
        let client = self.clone();
        let method = method.into();
        self.inner
            .controller
            .handle
            .spawn(async move {
                client
                    .request_on_controller_with_timeout(
                        method,
                        params,
                        request_timeout,
                        recover_handoff_on_timeout,
                    )
                    .await
            })
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

    async fn request_on_controller_with_timeout(
        &self,
        method: String,
        params: Value,
        request_timeout: Duration,
        recover_handoff_on_timeout: bool,
    ) -> Result<Value> {
        match timeout(request_timeout, self.ensure_started()).await {
            Ok(result) => result?,
            Err(_) => {
                let recovered = if recover_handoff_on_timeout {
                    self.recover_handoff_after_request_timeout().await
                } else {
                    false
                };
                return Err(AppServerRequestTimeoutError {
                    method,
                    request_timeout,
                    recovered,
                }
                .into());
            }
        }
        self.request_started_with_timeout(
            method,
            params,
            request_timeout,
            recover_handoff_on_timeout,
        )
        .await
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
            terminate_managed_process_group(process.pid, process.process_identity).await;
            if let Some(shutdown_tx) = process.shutdown_tx.take() {
                let _ = shutdown_tx.send(());
            }
            let mut join_handle = process.join_handle;
            match timeout(Duration::from_secs(2), &mut join_handle).await {
                Ok(_) => {}
                Err(_) => {
                    warn!(
                        profile_id = %self.inner.profile.id,
                        "timed out while waiting for codex app-server process supervisor to exit"
                    );
                    join_handle.abort();
                }
            }
        }
        Ok(())
    }

    async fn ensure_started(&self) -> Result<()> {
        let _guard = self.inner.start_lock.lock().await;
        if self.inner.process.lock().await.is_some() {
            return Ok(());
        }

        let mut last_start_error: Option<anyhow::Error> = None;
        for _ in 0..2 {
            let process_state = self.spawn_process().await?;
            let used_handoff_proxy = process_state.handoff_proxy;
            {
                let mut process = self.inner.process.lock().await;
                *process = Some(process_state);
            }

            if let Err(error) = async {
                self.request_started_with_timeout(
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
                    self.inner.config.startup_timeout,
                    false,
                )
                .await?;
                self.write_message(&json!({
                    "method": "initialized",
                    "params": {}
                }))
                .await?;
                self.enable_required_codex_features().await
            }
            .await
            {
                let _ = self.close_on_controller().await;
                if used_handoff_proxy {
                    warn!(
                        profile_id = %self.inner.profile.id,
                        error = %error,
                        "codex app-server handoff proxy failed during initialization; falling back to stdio"
                    );
                    self.inner
                        .handoff_disabled_after_failure
                        .store(true, Ordering::SeqCst);
                    let _ = stop_handoff_server(&self.inner.config, &self.inner.profile).await;
                    last_start_error = Some(error);
                    continue;
                }
                return Err(error);
            }

            return Ok(());
        }

        Err(last_start_error.unwrap_or_else(|| anyhow!("failed to start codex app-server")))
    }

    async fn request_started(&self, method: String, params: Value) -> Result<Value> {
        self.request_started_with_timeout(method, params, self.inner.config.request_timeout, false)
            .await
    }

    async fn enable_required_codex_features(&self) -> Result<()> {
        if let Err(error) = self
            .request_started_with_timeout(
                "config/batchWrite".to_string(),
                json!({
                    "edits": [
                        {
                            "keyPath": "features.goals",
                            "value": true,
                            "mergeStrategy": "replace"
                        }
                    ],
                    "filePath": null,
                    "expectedVersion": null,
                    "reloadUserConfig": true
                }),
                self.inner.config.startup_timeout,
                false,
            )
            .await
        {
            warn!(
                profile_id = %self.inner.profile.id,
                error = %error,
                "failed to enable required Codex config features; goal mode may be unavailable"
            );
        }
        Ok(())
    }

    async fn request_started_with_timeout(
        &self,
        method: String,
        params: Value,
        request_timeout: Duration,
        recover_handoff_on_timeout: bool,
    ) -> Result<Value> {
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

        match timeout(request_timeout, rx).await {
            Err(_) => {
                self.inner.pending.lock().await.remove(&id);
                let recovered = if recover_handoff_on_timeout {
                    self.recover_handoff_after_request_timeout().await
                } else {
                    false
                };
                Err(AppServerRequestTimeoutError {
                    method: method_name,
                    request_timeout,
                    recovered,
                }
                .into())
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

    async fn recover_handoff_after_request_timeout(&self) -> bool {
        let should_recover_handoff = {
            let process = self.inner.process.lock().await;
            let process_uses_handoff = process
                .as_ref()
                .map(|process| process.handoff_proxy)
                .unwrap_or(false);
            process_uses_handoff || handoff_paths(&self.inner.config, &self.inner.profile).is_some()
        };
        if !should_recover_handoff {
            return false;
        }

        warn!(
            profile_id = %self.inner.profile.id,
            "codex app-server handoff proxy timed out; restarting through stdio"
        );
        self.detach_unresponsive_process("codex app-server handoff proxy timed out")
            .await;
        self.inner
            .handoff_disabled_after_failure
            .store(true, Ordering::SeqCst);
        let _ = stop_handoff_server(&self.inner.config, &self.inner.profile).await;
        true
    }

    async fn detach_unresponsive_process(&self, reason: &str) {
        fail_pending_requests(&self.inner, reason).await;
        let process = self.inner.process.lock().await.take();
        if let Some(mut process) = process {
            if let Some(shutdown_tx) = process.shutdown_tx.take() {
                let _ = shutdown_tx.send(());
            }
            let mut join_handle = process.join_handle;
            tokio::spawn(async move {
                tokio::select! {
                    _ = &mut join_handle => {}
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {
                        join_handle.abort();
                    }
                }
            });
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
        let process_slot = match timeout(
            self.inner.config.startup_timeout,
            self.inner.controller.process_slots.clone().acquire_owned(),
        )
        .await
        {
            Ok(Ok(slot)) => slot,
            Ok(Err(_)) => anyhow::bail!("codex app-server process limiter is closed"),
            Err(_) => anyhow::bail!(
                "codex app-server process limit reached (max {}). Close an inactive session or set CODEX_WEBUI_MAX_APP_SERVERS to a larger value.",
                self.inner.config.max_processes.max(1)
            ),
        };
        let mut command = Command::new(&self.inner.config.codex_bin);
        let handoff_paths = self.ensure_handoff_server_running().await?;
        let handoff_proxy = handoff_paths.is_some();
        if let Some(handoff_paths) = handoff_paths {
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

        #[cfg(unix)]
        {
            command.process_group(0);
        }

        for (key, value) in &self.inner.config.extra_env {
            command.env(key, value);
        }

        let mut child = command
            .spawn()
            .with_context(|| format!("failed to spawn {}", self.inner.config.codex_bin))?;
        let pid = child.id();
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
            pid,
            process_identity: read_managed_process_identity(pid),
            shutdown_tx: Some(shutdown_tx),
            join_handle,
            handoff_proxy,
            _process_slot: process_slot,
        })
    }

    async fn ensure_handoff_server_running(&self) -> Result<Option<HandoffPaths>> {
        if self
            .inner
            .handoff_disabled_after_failure
            .load(Ordering::SeqCst)
        {
            return Ok(None);
        }
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
            let process_identity = read_managed_process_identity(Some(pid));
            drop(child);

            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            while tokio::time::Instant::now() < deadline {
                if handoff_socket_is_live(&paths.socket_path).await {
                    let meta = HandoffMeta {
                        pid,
                        process_identity,
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
                    write_bytes_atomically(&paths.meta_path, &encoded)
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
        let controller =
            AppServerControllerRuntime::new(config.controller_threads, config.max_processes);
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

    pub async fn active_process_count(&self) -> usize {
        let clients = {
            let clients = self.clients.lock().await;
            clients.values().cloned().collect::<Vec<_>>()
        };
        let mut active = 0;
        for client in clients {
            if client.inner.process.lock().await.is_some() {
                active += 1;
            }
        }
        active
    }

    pub async fn handoff_status(&self) -> AppServerHandoffStatus {
        let clients = {
            let clients = self.clients.lock().await;
            clients.values().cloned().collect::<Vec<_>>()
        };
        let mut status = AppServerHandoffStatus {
            client_count: clients.len(),
            ..AppServerHandoffStatus::default()
        };

        for client in clients {
            let process = client.inner.process.lock().await;
            let Some(process) = process.as_ref() else {
                continue;
            };
            status.active_process_count += 1;
            if process.handoff_proxy {
                status.handoff_proxy_process_count += 1;
            } else {
                status.stdio_process_count += 1;
            }
        }

        status
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
async fn terminate_managed_process_group(
    pid: Option<u32>,
    identity: Option<ManagedProcessIdentity>,
) {
    let Some(pid) = pid.filter(|pid| *pid > 0) else {
        return;
    };
    if !managed_process_can_signal_group(pid, identity) {
        return;
    }
    let process_group = format!("-{pid}");
    let _ = Command::new("kill")
        .arg("-TERM")
        .arg("--")
        .arg(&process_group)
        .status()
        .await;
    let _ = Command::new("kill")
        .arg("-TERM")
        .arg("--")
        .arg(pid.to_string())
        .status()
        .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let _ = Command::new("kill")
        .arg("-KILL")
        .arg("--")
        .arg(&process_group)
        .status()
        .await;
    let _ = Command::new("kill")
        .arg("-KILL")
        .arg("--")
        .arg(pid.to_string())
        .status()
        .await;
}

#[cfg(not(unix))]
async fn terminate_managed_process_group(
    _pid: Option<u32>,
    _identity: Option<ManagedProcessIdentity>,
) {
}

#[cfg(unix)]
fn managed_process_can_signal_group(pid: u32, identity: Option<ManagedProcessIdentity>) -> bool {
    #[cfg(target_os = "linux")]
    {
        let Some(expected) = identity else {
            warn!("refusing to terminate codex app-server pid {pid}: process identity unavailable");
            return false;
        };
        if expected.pid != pid {
            warn!(
                "refusing to terminate codex app-server pid {pid}: identity pid {} does not match",
                expected.pid
            );
            return false;
        }
        if !managed_process_identity_matches(pid, expected) {
            warn!("refusing to terminate codex app-server pid {pid}: process identity changed");
            return false;
        }
        if expected.process_group_id != pid {
            warn!(
                "refusing to terminate codex app-server pid {pid}: process group {} is not child-owned",
                expected.process_group_id
            );
            return false;
        }
        true
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (pid, identity);
        true
    }
}

fn read_managed_process_identity(pid: Option<u32>) -> Option<ManagedProcessIdentity> {
    let pid = pid?;
    read_managed_process_identity_for_pid(pid)
}

#[cfg(target_os = "linux")]
fn read_managed_process_identity_for_pid(pid: u32) -> Option<ManagedProcessIdentity> {
    let proc_stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_managed_process_identity(pid, &proc_stat)
}

#[cfg(not(target_os = "linux"))]
fn read_managed_process_identity_for_pid(_pid: u32) -> Option<ManagedProcessIdentity> {
    None
}

#[cfg(target_os = "linux")]
fn parse_managed_process_identity(pid: u32, proc_stat: &str) -> Option<ManagedProcessIdentity> {
    let fields = proc_stat
        .rsplit_once(") ")?
        .1
        .split_whitespace()
        .collect::<Vec<_>>();
    let process_group_id = fields.get(2)?.parse::<u32>().ok()?;
    let start_time_ticks = fields.get(19)?.parse::<u64>().ok()?;
    Some(ManagedProcessIdentity {
        pid,
        process_group_id,
        start_time_ticks,
    })
}

#[cfg(target_os = "linux")]
fn managed_process_identity_matches(pid: u32, expected: ManagedProcessIdentity) -> bool {
    read_managed_process_identity_for_pid(pid).is_some_and(|current| current == expected)
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
        terminate_managed_process_group(Some(meta.pid), meta.process_identity).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        if handoff_socket_is_live(&paths.socket_path).await {
            warn!(
                pid = meta.pid,
                socket = %paths.socket_path.display(),
                "codex app-server handoff socket is still live after guarded termination"
            );
            return Ok(());
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
    fn new(worker_threads: usize, max_processes: usize) -> Arc<Self> {
        let worker_threads = worker_threads.clamp(1, 4);
        let blocking_threads = worker_threads.saturating_mul(4).max(4).min(32);
        let max_processes = max_processes.clamp(1, 16);
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
            process_slots: Arc::new(Semaphore::new(max_processes)),
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
                .unwrap_or(1)
                .min(2)
                .max(1)
        })
        .clamp(1, 4)
}

fn default_max_process_count() -> usize {
    std::env::var("CODEX_WEBUI_MAX_APP_SERVERS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(1)
        .clamp(1, 16)
}

fn default_request_timeout() -> Duration {
    let env_value = std::env::var("CODEX_WEBUI_APP_SERVER_TIMEOUT_SECONDS").ok();
    Duration::from_secs(app_server_request_timeout_seconds_from_env_value(
        env_value.as_deref(),
    ))
}

fn app_server_request_timeout_seconds_from_env_value(value: Option<&str>) -> u64 {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(APP_SERVER_REQUEST_TIMEOUT_DEFAULT_SECONDS)
        .clamp(
            APP_SERVER_REQUEST_TIMEOUT_MIN_SECONDS,
            APP_SERVER_REQUEST_TIMEOUT_MAX_SECONDS,
        )
}

fn default_startup_timeout() -> Duration {
    let seconds = std::env::var("CODEX_WEBUI_APP_SERVER_STARTUP_TIMEOUT_SECONDS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(5)
        .clamp(1, 60);
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

    let (reason, exit_code) = match exit_result {
        Ok(status) => (
            format!("codex app-server exited ({status})"),
            status.code().map(Value::from).unwrap_or(Value::Null),
        ),
        Err(error) => (
            format!("failed to wait for codex app-server exit: {error}"),
            Value::Null,
        ),
    };

    warn!(
        profile_id = %inner.profile.id,
        codex_home = %inner.profile.codex_home.display(),
        "{reason}"
    );
    let _ = inner.notifications_tx.send(AppServerNotification {
        method: "codex-webui/app-server/exited".to_string(),
        params: json!({
            "reason": reason,
            "exitCode": exit_code,
        }),
    });
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

async fn write_bytes_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("target path has no parent: {}", path.display()))?;
    tokio::fs::create_dir_all(parent)
        .await
        .with_context(|| format!("failed to create {}", parent.display()))?;

    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("state");
    let temp_path = parent.join(format!(
        ".{file_name}.tmp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));

    let result = async {
        let mut file = tokio::fs::File::create(&temp_path)
            .await
            .with_context(|| format!("failed to create {}", temp_path.display()))?;
        file.write_all(bytes)
            .await
            .with_context(|| format!("failed to write {}", temp_path.display()))?;
        file.sync_all()
            .await
            .with_context(|| format!("failed to sync {}", temp_path.display()))?;
        drop(file);
        tokio::fs::rename(&temp_path, path)
            .await
            .with_context(|| format!("failed to rename {}", temp_path.display()))?;
        if let Ok(parent_dir) = fs::File::open(parent) {
            let _ = parent_dir.sync_all();
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;

    if result.is_err() {
        let _ = tokio::fs::remove_file(&temp_path).await;
    }
    result
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
        AppServerClient, AppServerClientConfig, AppServerManager, AppServerNotification,
        AppServerProfile, AppServerRequest, IncomingMessage, app_server_request_timed_out,
        app_server_timeout_recovered, classify_incoming_message, handoff_paths,
        write_bytes_atomically,
    };
    use serde_json::json;
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[cfg(target_os = "linux")]
    #[test]
    fn managed_process_group_signaling_requires_current_child_owned_identity() {
        let parsed = super::parse_managed_process_identity(
            123,
            "123 (codex app-server) S 1 123 123 0 -1 4194304 1 2 3 4 5 6 7 8 20 0 1 0 987654321 0",
        )
        .expect("proc stat identity should parse");
        assert_eq!(parsed.pid, 123);
        assert_eq!(parsed.process_group_id, 123);
        assert_eq!(parsed.start_time_ticks, 987654321);

        let current_pid = std::process::id();
        let current = super::read_managed_process_identity_for_pid(current_pid)
            .expect("current process identity should be readable on linux");
        assert!(!super::managed_process_can_signal_group(current_pid, None));
        assert_eq!(
            super::managed_process_can_signal_group(current_pid, Some(current)),
            current.process_group_id == current_pid
        );

        let changed_identity = super::ManagedProcessIdentity {
            start_time_ticks: current.start_time_ticks.saturating_add(1),
            ..current
        };
        assert!(!super::managed_process_can_signal_group(
            current_pid,
            Some(changed_identity)
        ));
    }

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
    fn app_server_request_timeout_defaults_for_long_sessions() {
        assert_eq!(
            super::app_server_request_timeout_seconds_from_env_value(None),
            600
        );
        assert_eq!(
            super::app_server_request_timeout_seconds_from_env_value(Some("1")),
            5
        );
        assert_eq!(
            super::app_server_request_timeout_seconds_from_env_value(Some("999999")),
            7_200
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

    #[tokio::test]
    async fn handoff_metadata_write_is_atomic() {
        let dir = std::env::temp_dir().join(format!(
            "codex-webui-handoff-atomic-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("default.json");

        write_bytes_atomically(&path, br#"{"pid":1}"#)
            .await
            .expect("metadata should write atomically");
        assert_eq!(
            std::fs::read_to_string(&path).expect("metadata should be readable"),
            r#"{"pid":1}"#
        );

        let leftovers = std::fs::read_dir(&dir)
            .expect("metadata directory should exist")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with(".default.json.tmp-"))
            .collect::<Vec<_>>();
        assert!(leftovers.is_empty());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_first_requests_wait_for_initialization() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "codex-webui-concurrent-start-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let script_path = dir.join("fake-codex.py");
        let log_path = dir.join("starts.log");
        std::fs::create_dir_all(&dir).expect("test dir should be created");
        std::fs::write(
            &script_path,
            r#"#!/usr/bin/env python3
import json
import os
import sys
import time

log_path = os.environ.get("FAKE_CODEX_START_LOG")
initialized = False
def log(message):
    if log_path:
        os.makedirs(os.path.dirname(log_path), exist_ok=True)
        with open(log_path, "a", encoding="utf-8") as handle:
            handle.write(message + "\n")

def respond(payload, result=None):
    print(json.dumps({"id": payload.get("id"), "result": result or {}}), flush=True)

if sys.argv[1:] == ["app-server", "--listen", "stdio://"]:
    for raw_line in sys.stdin:
        payload = json.loads(raw_line)
        method = payload.get("method")
        if method == "initialize":
            log("initialize-start")
            time.sleep(0.4)
            respond(payload, {"serverInfo": {"name": "fake"}})
            log("initialize-end")
        elif method == "initialized":
            initialized = True
            log("initialized")
        elif method == "config/batchWrite":
            respond(payload, {})
        elif method == "experimentalFeature/enablement/set":
            if not initialized:
                print(json.dumps({"id": payload.get("id"), "error": {"message": "feature before initialized"}}), flush=True)
            else:
                respond(payload, {})
        elif method == "echo":
            if not initialized:
                print(json.dumps({"id": payload.get("id"), "error": {"message": "echo before initialized"}}), flush=True)
            else:
                respond(payload, payload.get("params") or {})
        else:
            print(json.dumps({"id": payload.get("id"), "error": {"message": "unknown method"}}), flush=True)
"#,
        )
        .expect("fake codex should be written");
        let mut permissions = std::fs::metadata(&script_path)
            .expect("fake codex metadata should be readable")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script_path, permissions)
            .expect("fake codex should be executable");

        let client = AppServerClient::new(
            AppServerProfile {
                id: "default".to_string(),
                codex_home: dir.join("codex-home"),
            },
            AppServerClientConfig {
                codex_bin: script_path.display().to_string(),
                startup_timeout: std::time::Duration::from_secs(2),
                request_timeout: std::time::Duration::from_secs(2),
                extra_env: HashMap::from([(
                    "FAKE_CODEX_START_LOG".to_string(),
                    log_path.display().to_string(),
                )]),
                ..AppServerClientConfig::default()
            },
        );

        let (first, second) = tokio::join!(
            client.request("echo", json!({ "request": 1 })),
            client.request("echo", json!({ "request": 2 }))
        );
        assert_eq!(
            first.expect("first request should complete"),
            json!({ "request": 1 })
        );
        assert_eq!(
            second.expect("second request should complete"),
            json!({ "request": 2 })
        );

        let log = std::fs::read_to_string(&log_path).expect("start log should exist");
        assert!(log.contains("initialize-start"));
        assert!(log.contains("initialized"));

        client.close().await.expect("client should close");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn manager_limits_active_app_server_processes() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "codex-webui-process-limit-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let script_path = dir.join("fake-codex.py");
        std::fs::create_dir_all(&dir).expect("test dir should be created");
        std::fs::write(
            &script_path,
            r#"#!/usr/bin/env python3
import json
import sys

if sys.argv[1:] == ["app-server", "--listen", "stdio://"]:
    for raw_line in sys.stdin:
        payload = json.loads(raw_line)
        method = payload.get("method")
        request_id = payload.get("id")
        if method == "initialized":
            continue
        if method in ("initialize", "config/batchWrite"):
            print(json.dumps({"id": request_id, "result": {"ok": True}}), flush=True)
        elif method == "echo":
            print(json.dumps({"id": request_id, "result": payload.get("params") or {}}), flush=True)
        else:
            print(json.dumps({"id": request_id, "error": {"message": "unknown method"}}), flush=True)
"#,
        )
        .expect("fake codex should be written");
        let mut permissions = std::fs::metadata(&script_path)
            .expect("fake codex metadata should be readable")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script_path, permissions)
            .expect("fake codex should be executable");

        let manager = AppServerManager::new(AppServerClientConfig {
            codex_bin: script_path.display().to_string(),
            max_processes: 1,
            startup_timeout: std::time::Duration::from_millis(150),
            request_timeout: std::time::Duration::from_secs(1),
            ..AppServerClientConfig::default()
        });
        let first = manager
            .get_or_create(AppServerProfile {
                id: "default".to_string(),
                codex_home: dir.join("codex-home-1"),
            })
            .await;
        let second = manager
            .get_or_create(AppServerProfile {
                id: "other".to_string(),
                codex_home: dir.join("codex-home-2"),
            })
            .await;

        first
            .request("echo", json!({ "profile": "default" }))
            .await
            .expect("first profile should acquire the only process slot");
        let error = second
            .request("echo", json!({ "profile": "other" }))
            .await
            .expect_err("second profile should wait for the process cap");
        assert!(error.to_string().contains("process limit reached"));

        first.close().await.expect("first client should close");
        assert_eq!(
            second
                .request("echo", json!({ "profile": "other" }))
                .await
                .expect("second profile should start after the first closes"),
            json!({ "profile": "other" })
        );
        second.close().await.expect("second client should close");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn falls_back_to_stdio_when_handoff_proxy_does_not_initialize() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "codex-webui-handoff-fallback-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let script_path = dir.join("fake-codex.py");
        let log_path = dir.join("starts.log");
        let handoff_dir = dir.join("handoff");
        std::fs::create_dir_all(&dir).expect("test dir should be created");
        std::fs::write(
            &script_path,
            r#"#!/usr/bin/env python3
import json
import os
import signal
import socket
import sys
import time

log_path = os.environ.get("FAKE_CODEX_START_LOG")
def log(message):
    if log_path:
        os.makedirs(os.path.dirname(log_path), exist_ok=True)
        with open(log_path, "a", encoding="utf-8") as handle:
            handle.write(message + "\n")

args = sys.argv[1:]
if "--listen" in args:
    listen = args[args.index("--listen") + 1]
    if listen.startswith("unix://"):
        path = listen[len("unix://"):]
        try:
            os.unlink(path)
        except FileNotFoundError:
            pass
        os.makedirs(os.path.dirname(path), exist_ok=True)
        server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        server.bind(path)
        server.listen(1)
        log("handoff-server")
        signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
        while True:
            connection, _ = server.accept()
            log("handoff-accepted")
            try:
                while connection.recv(4096):
                    pass
            except Exception:
                pass
    if listen == "stdio://":
        log("stdio")
        for raw_line in sys.stdin:
            payload = json.loads(raw_line)
            method = payload.get("method")
            if method == "initialize":
                print(json.dumps({"id": payload.get("id"), "result": {"serverInfo": {"name": "fake"}}}), flush=True)
            elif method == "initialized":
                pass
            elif method == "echo":
                print(json.dumps({"id": payload.get("id"), "result": payload.get("params") or {}}), flush=True)
            else:
                print(json.dumps({"id": payload.get("id"), "error": {"message": "unknown method"}}), flush=True)
        sys.exit(0)

if "proxy" in args:
    log("proxy")
    for _ in sys.stdin:
        time.sleep(60)
"#,
        )
        .expect("fake codex should be written");
        let mut permissions = std::fs::metadata(&script_path)
            .expect("fake codex metadata should be readable")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script_path, permissions)
            .expect("fake codex should be executable");

        let client = AppServerClient::new(
            AppServerProfile {
                id: "default".to_string(),
                codex_home: dir.join("codex-home"),
            },
            AppServerClientConfig {
                codex_bin: script_path.display().to_string(),
                handoff_dir: Some(handoff_dir),
                startup_timeout: std::time::Duration::from_millis(100),
                request_timeout: std::time::Duration::from_secs(2),
                extra_env: HashMap::from([(
                    "FAKE_CODEX_START_LOG".to_string(),
                    log_path.display().to_string(),
                )]),
                ..AppServerClientConfig::default()
            },
        );

        let response = client
            .request("echo", json!({ "ok": true }))
            .await
            .expect("client should fall back to stdio after a broken handoff proxy");

        assert_eq!(response, json!({ "ok": true }));
        let log = std::fs::read_to_string(&log_path).expect("start log should exist");
        assert!(log.contains("handoff-server"));
        assert!(log.contains("proxy"));
        assert!(log.contains("stdio"));

        client.close().await.expect("client should close");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn request_timeout_recovers_stale_handoff_proxy() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "codex-webui-handoff-timeout-recovery-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let script_path = dir.join("fake-codex.py");
        let log_path = dir.join("starts.log");
        let handoff_dir = dir.join("handoff");
        std::fs::create_dir_all(&dir).expect("test dir should be created");
        std::fs::write(
            &script_path,
            r#"#!/usr/bin/env python3
import json
import os
import signal
import socket
import sys
import time

log_path = os.environ.get("FAKE_CODEX_START_LOG")
def log(message):
    if log_path:
        os.makedirs(os.path.dirname(log_path), exist_ok=True)
        with open(log_path, "a", encoding="utf-8") as handle:
            handle.write(message + "\n")

def respond(payload, result=None):
    print(json.dumps({"id": payload.get("id"), "result": result or {}}), flush=True)

args = sys.argv[1:]
if "--listen" in args:
    listen = args[args.index("--listen") + 1]
    if listen.startswith("unix://"):
        path = listen[len("unix://"):]
        try:
            os.unlink(path)
        except FileNotFoundError:
            pass
        os.makedirs(os.path.dirname(path), exist_ok=True)
        server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        server.bind(path)
        server.listen(1)
        log("handoff-server")
        signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
        while True:
            connection, _ = server.accept()
            log("handoff-accepted")
            try:
                while connection.recv(4096):
                    pass
            except Exception:
                pass
    if listen == "stdio://":
        log("stdio")
        for raw_line in sys.stdin:
            payload = json.loads(raw_line)
            method = payload.get("method")
            if method == "initialize":
                respond(payload, {"serverInfo": {"name": "fake"}})
            elif method == "initialized":
                pass
            elif method == "config/batchWrite":
                respond(payload, {})
            elif method == "experimentalFeature/enablement/set":
                respond(payload, {})
            elif method == "echo":
                respond(payload, payload.get("params") or {})
            else:
                print(json.dumps({"id": payload.get("id"), "error": {"message": "unknown method"}}), flush=True)
        sys.exit(0)

if "proxy" in args:
    log("proxy")
    for raw_line in sys.stdin:
        payload = json.loads(raw_line)
        method = payload.get("method")
        if method == "initialize":
            respond(payload, {"serverInfo": {"name": "fake-proxy"}})
        elif method == "initialized":
            pass
        elif method == "config/batchWrite":
            respond(payload, {})
        elif method == "experimentalFeature/enablement/set":
            respond(payload, {})
        elif method == "echo":
            time.sleep(60)
"#,
        )
        .expect("fake codex should be written");
        let mut permissions = std::fs::metadata(&script_path)
            .expect("fake codex metadata should be readable")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script_path, permissions)
            .expect("fake codex should be executable");

        let client = AppServerClient::new(
            AppServerProfile {
                id: "default".to_string(),
                codex_home: dir.join("codex-home"),
            },
            AppServerClientConfig {
                codex_bin: script_path.display().to_string(),
                handoff_dir: Some(handoff_dir),
                startup_timeout: std::time::Duration::from_secs(1),
                request_timeout: std::time::Duration::from_secs(2),
                extra_env: HashMap::from([(
                    "FAKE_CODEX_START_LOG".to_string(),
                    log_path.display().to_string(),
                )]),
                ..AppServerClientConfig::default()
            },
        );

        let timed_out = client
            .request_with_timeout(
                "echo",
                json!({ "via": "proxy" }),
                std::time::Duration::from_millis(500),
                true,
            )
            .await
            .expect_err("stale proxy request should time out");
        assert!(app_server_request_timed_out(&timed_out));
        assert!(app_server_timeout_recovered(&timed_out));

        let response = client
            .request("echo", json!({ "via": "stdio" }))
            .await
            .expect("client should restart through stdio after handoff timeout");
        assert_eq!(response, json!({ "via": "stdio" }));

        let log = std::fs::read_to_string(&log_path).expect("start log should exist");
        assert!(log.contains("handoff-server"));
        assert!(log.contains("proxy"));
        assert!(log.contains("stdio"));

        client.close().await.expect("client should close");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn request_timeout_recovers_handoff_proxy_that_hangs_during_initialization() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "codex-webui-handoff-startup-timeout-recovery-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let script_path = dir.join("fake-codex.py");
        let log_path = dir.join("starts.log");
        let handoff_dir = dir.join("handoff");
        std::fs::create_dir_all(&dir).expect("test dir should be created");
        std::fs::write(
            &script_path,
            r#"#!/usr/bin/env python3
import json
import os
import signal
import socket
import sys
import time

log_path = os.environ.get("FAKE_CODEX_START_LOG")
def log(message):
    if log_path:
        os.makedirs(os.path.dirname(log_path), exist_ok=True)
        with open(log_path, "a", encoding="utf-8") as handle:
            handle.write(message + "\n")

def respond(payload, result=None):
    print(json.dumps({"id": payload.get("id"), "result": result or {}}), flush=True)

args = sys.argv[1:]
if "--listen" in args:
    listen = args[args.index("--listen") + 1]
    if listen.startswith("unix://"):
        path = listen[len("unix://"):]
        try:
            os.unlink(path)
        except FileNotFoundError:
            pass
        os.makedirs(os.path.dirname(path), exist_ok=True)
        server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        server.bind(path)
        server.listen(1)
        log("handoff-server")
        signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
        while True:
            connection, _ = server.accept()
            log("handoff-accepted")
            try:
                while connection.recv(4096):
                    pass
            except Exception:
                pass
    if listen == "stdio://":
        log("stdio")
        for raw_line in sys.stdin:
            payload = json.loads(raw_line)
            method = payload.get("method")
            if method == "initialize":
                respond(payload, {"serverInfo": {"name": "fake"}})
            elif method == "initialized":
                pass
            elif method == "config/batchWrite":
                respond(payload, {})
            elif method == "experimentalFeature/enablement/set":
                respond(payload, {})
            elif method == "echo":
                respond(payload, payload.get("params") or {})
            else:
                print(json.dumps({"id": payload.get("id"), "error": {"message": "unknown method"}}), flush=True)
        sys.exit(0)

if "proxy" in args:
    log("proxy")
    for _ in sys.stdin:
        time.sleep(60)
"#,
        )
        .expect("fake codex should be written");
        let mut permissions = std::fs::metadata(&script_path)
            .expect("fake codex metadata should be readable")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script_path, permissions)
            .expect("fake codex should be executable");

        let client = AppServerClient::new(
            AppServerProfile {
                id: "default".to_string(),
                codex_home: dir.join("codex-home"),
            },
            AppServerClientConfig {
                codex_bin: script_path.display().to_string(),
                handoff_dir: Some(handoff_dir),
                startup_timeout: std::time::Duration::from_secs(10),
                request_timeout: std::time::Duration::from_secs(2),
                extra_env: HashMap::from([(
                    "FAKE_CODEX_START_LOG".to_string(),
                    log_path.display().to_string(),
                )]),
                ..AppServerClientConfig::default()
            },
        );

        let timed_out = client
            .request_with_timeout(
                "echo",
                json!({ "via": "proxy-startup" }),
                std::time::Duration::from_millis(500),
                true,
            )
            .await
            .expect_err("startup hang should time out through the request deadline");
        assert!(app_server_request_timed_out(&timed_out));
        assert!(app_server_timeout_recovered(&timed_out));

        let response = client
            .request("echo", json!({ "via": "stdio" }))
            .await
            .expect("client should restart through stdio after startup timeout");
        assert_eq!(response, json!({ "via": "stdio" }));

        let log = std::fs::read_to_string(&log_path).expect("start log should exist");
        assert!(log.contains("handoff-server"));
        assert!(log.contains("proxy"));
        assert!(log.contains("stdio"));

        client.close().await.expect("client should close");
        let _ = std::fs::remove_dir_all(dir);
    }
}
