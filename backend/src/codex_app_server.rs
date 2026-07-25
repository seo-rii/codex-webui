use std::{
    collections::{HashMap, HashSet},
    env,
    error::Error as StdError,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command as StdCommand, Stdio},
    sync::{
        Arc, Mutex as StdMutex, OnceLock, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc as std_mpsc,
    },
    thread,
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    runtime::{Builder as TokioRuntimeBuilder, Handle as TokioRuntimeHandle},
    sync::{Mutex, OwnedSemaphorePermit, RwLock, Semaphore, broadcast, mpsc, oneshot},
    task::JoinHandle,
    time::{Instant as TokioInstant, timeout, timeout_at},
};
use tracing::{info, warn};

#[cfg(unix)]
use tokio::net::UnixStream;
#[cfg(unix)]
use tokio_tungstenite::{
    WebSocketStream, client_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};

const APP_SERVER_THREAD_STACK_BYTES: usize = 4 * 1024 * 1024;
const APP_SERVER_REQUEST_TIMEOUT_DEFAULT_SECONDS: u64 = 600;
const APP_SERVER_REQUEST_TIMEOUT_MIN_SECONDS: u64 = 5;
const APP_SERVER_REQUEST_TIMEOUT_MAX_SECONDS: u64 = 7_200;
const APP_SERVER_DEFAULT_CPU_SUB: usize = 2;
const APP_SERVER_DEFAULT_CPU_DIVISOR: usize = 1;
const APP_SERVER_DEFAULT_MEMORY_BYTES_PER_PROCESS: u64 = 2 * 1024 * 1024 * 1024;
const APP_SERVER_GATEWAY_MEMORY_RESERVE_BYTES: u64 = 1024 * 1024 * 1024;
const APP_SERVER_DEFAULT_MAX_PROCESSES_CAP: usize = 4;
const APP_SERVER_MAX_PROCESSES_HARD_CAP: usize = 512;
const APP_SERVER_DEFAULT_IDLE_TIMEOUT_SECONDS: u64 = 300;
const APP_SERVER_READER_CLEANUP_TIMEOUT: Duration = Duration::from_secs(1);

const APP_SERVER_WEBSOCKET_QUEUE_CAPACITY: usize = 256;
#[cfg(unix)]
const APP_SERVER_HANDOFF_MAX_WEBSOCKET_MESSAGE_BYTES: usize = 128 << 20;

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
    pub idle_client_timeout: Duration,
    pub handoff_dir: Option<PathBuf>,
    pub drop_inherited_capabilities: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AppServerHandoffStatus {
    pub client_count: usize,
    pub active_process_count: usize,
    pub stdio_process_count: usize,
    pub handoff_proxy_process_count: usize,
    pub blocking_process_count: usize,
    pub closed_idle_process_count: usize,
}

#[derive(Clone, Debug)]
pub struct AppServerProcessSnapshot {
    pub client_key: String,
    pub profile_id: String,
    pub codex_home: PathBuf,
    pub pid: u32,
    pub kind: String,
    pub handoff_proxy: bool,
    pub socket_path: Option<PathBuf>,
    pub log_path: Option<PathBuf>,
    pub started_at_ms: Option<u128>,
    pub codex_bin: String,
    pub pending_request_count: usize,
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
            idle_client_timeout: default_idle_client_timeout(),
            handoff_dir: None,
            drop_inherited_capabilities: default_drop_inherited_capabilities(),
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
    client_key: String,
    profile: AppServerProfile,
    config: AppServerClientConfig,
    controller: Arc<AppServerControllerRuntime>,
    clients_registry: Option<Weak<Mutex<HashMap<String, AppServerClient>>>>,
    lifecycle: RwLock<()>,
    start_lock: Mutex<()>,
    process: Mutex<Option<ProcessState>>,
    pending: Mutex<HashMap<u64, PendingRequest>>,
    active_turn_ids: Mutex<HashSet<String>>,
    pending_server_request_ids: Mutex<HashSet<String>>,
    next_request_id: AtomicU64,
    next_process_generation: AtomicU64,
    last_activity_at_ms: AtomicU64,
    notifications_tx: broadcast::Sender<AppServerNotification>,
    requests_tx: broadcast::Sender<AppServerRequest>,
}

struct PendingRequest {
    generation: u64,
    sender: oneshot::Sender<Result<Value, String>>,
}

struct ProcessState {
    generation: u64,
    writer: Arc<Mutex<AppServerWriter>>,
    pid: Option<u32>,
    process_identity: Option<ManagedProcessIdentity>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    join_handle: JoinHandle<()>,
    supervisor_start_tx: Option<oneshot::Sender<()>>,
    handoff_proxy: bool,
    _process_slot: OwnedSemaphorePermit,
}

enum AppServerWriter {
    Stdio(ChildStdin),
    #[cfg(unix)]
    WebSocket(mpsc::Sender<String>),
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
    #[serde(default)]
    enabled_features: Vec<String>,
    #[serde(default)]
    client_key: String,
    profile_id: String,
    socket_path: String,
    codex_bin: String,
    codex_home: String,
    started_at_ms: u128,
}

#[derive(Debug)]
struct LiveHandoffDaemonUnavailable {
    socket_path: PathBuf,
}

impl std::fmt::Display for LiveHandoffDaemonUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "persistent Codex app-server at {} is still alive but its WebSocket is unavailable",
            self.socket_path.display()
        )
    }
}

impl StdError for LiveHandoffDaemonUnavailable {}

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

fn codex_app_server_command_spec(
    codex_bin: &str,
    args: Vec<OsString>,
    drop_inherited_capabilities: bool,
    setpriv_available: bool,
) -> (OsString, Vec<OsString>) {
    #[cfg(target_os = "linux")]
    {
        if drop_inherited_capabilities && setpriv_available {
            let mut wrapped_args = vec![
                OsString::from("--inh-caps=-all"),
                OsString::from("--ambient-caps=-all"),
                OsString::from("--"),
                OsString::from(codex_bin),
            ];
            wrapped_args.extend(args);
            return (OsString::from("setpriv"), wrapped_args);
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = drop_inherited_capabilities;
        let _ = setpriv_available;
    }

    (OsString::from(codex_bin), args)
}

fn codex_capability_wrapper_available() -> bool {
    #[cfg(target_os = "linux")]
    {
        static SETPRIV_CAP_WRAPPER_AVAILABLE: OnceLock<bool> = OnceLock::new();
        *SETPRIV_CAP_WRAPPER_AVAILABLE.get_or_init(|| {
            StdCommand::new("setpriv")
                .arg("--inh-caps=-all")
                .arg("--ambient-caps=-all")
                .arg("--")
                .arg("true")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        })
    }

    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

#[cfg(target_os = "linux")]
static SETPRIV_CAP_WRAPPER_UNAVAILABLE_WARNED: AtomicBool = AtomicBool::new(false);

fn codex_command_from_app_server_args(
    config: &AppServerClientConfig,
    args: Vec<OsString>,
) -> Command {
    let setpriv_available = codex_capability_wrapper_available();
    #[cfg(target_os = "linux")]
    if config.drop_inherited_capabilities
        && !setpriv_available
        && !SETPRIV_CAP_WRAPPER_UNAVAILABLE_WARNED.swap(true, Ordering::SeqCst)
    {
        warn!(
            "setpriv capability wrapper is unavailable; starting codex app-server without dropping inherited capabilities"
        );
    }

    let (program, args) = codex_app_server_command_spec(
        &config.codex_bin,
        args,
        config.drop_inherited_capabilities,
        setpriv_available,
    );
    let mut command = Command::new(program);
    command.args(args);
    command
}

impl AppServerClient {
    pub fn new(profile: AppServerProfile, config: AppServerClientConfig) -> Self {
        let controller =
            AppServerControllerRuntime::new(config.controller_threads, config.max_processes);
        let client_key = profile.id.clone();
        Self::with_controller(client_key, profile, config, controller, None)
    }

    fn with_controller(
        client_key: String,
        profile: AppServerProfile,
        config: AppServerClientConfig,
        controller: Arc<AppServerControllerRuntime>,
        clients_registry: Option<Weak<Mutex<HashMap<String, AppServerClient>>>>,
    ) -> Self {
        // Native payloads can contain complete turns and images. Keeping
        // thousands of them retains gigabytes when one relay falls behind;
        // lagged consumers already receive an explicit resync signal.
        let (notifications_tx, _) = broadcast::channel(512);
        let (requests_tx, _) = broadcast::channel(128);

        Self {
            inner: Arc::new(AppServerClientInner {
                client_key,
                profile,
                config,
                controller,
                clients_registry,
                lifecycle: RwLock::new(()),
                start_lock: Mutex::new(()),
                process: Mutex::new(None),
                pending: Mutex::new(HashMap::new()),
                active_turn_ids: Mutex::new(HashSet::new()),
                pending_server_request_ids: Mutex::new(HashSet::new()),
                next_request_id: AtomicU64::new(1),
                next_process_generation: AtomicU64::new(1),
                last_activity_at_ms: AtomicU64::new(unix_time_ms()),
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

    pub fn request_timeout(&self) -> Duration {
        self.inner.config.request_timeout
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

    pub async fn has_active_turn_id(&self, turn_id: &str) -> bool {
        self.inner.active_turn_ids.lock().await.contains(turn_id)
    }

    async fn request_on_controller(&self, method: String, params: Value) -> Result<Value> {
        let _lifecycle = self.inner.lifecycle.read().await;
        self.touch_activity();
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
        let _lifecycle = self.inner.lifecycle.read().await;
        self.touch_activity();
        let deadline = TokioInstant::now() + request_timeout;
        // Starting Codex mutates shared process state. Keep that work alive when a
        // short caller deadline expires so a catalog or account probe cannot leave
        // a half-initialized process behind or repeatedly kill healthy startups.
        let startup_client = self.clone();
        let startup_task = self
            .inner
            .controller
            .handle
            .spawn(async move { startup_client.ensure_started().await });
        match timeout_at(deadline, startup_task).await {
            Ok(result) => result.context("codex app-server startup task failed")??,
            Err(_) => {
                self.inner
                    .pending
                    .lock()
                    .await
                    .retain(|_, pending| !pending.sender.is_closed());
                return Err(AppServerRequestTimeoutError {
                    method,
                    request_timeout,
                    recovered: false,
                }
                .into());
            }
        }
        let remaining = deadline.saturating_duration_since(TokioInstant::now());
        self.request_started_with_timeout(method, params, remaining, recover_handoff_on_timeout)
            .await
    }

    async fn respond_on_controller(&self, id: Value, result: Value) -> Result<()> {
        let _lifecycle = self.inner.lifecycle.read().await;
        self.touch_activity();
        let request_id_key = server_request_id_key(&id);
        if !self
            .inner
            .process
            .lock()
            .await
            .as_ref()
            .is_some_and(process_state_is_usable)
        {
            anyhow::bail!("codex app-server request expired after its process exited");
        }
        self.write_message(&json!({
            "id": id,
            "result": result
        }))
        .await?;
        self.inner
            .pending_server_request_ids
            .lock()
            .await
            .remove(&request_id_key);
        Ok(())
    }

    async fn reject_on_controller(&self, id: Value, message: String) -> Result<()> {
        let _lifecycle = self.inner.lifecycle.read().await;
        self.touch_activity();
        let request_id_key = server_request_id_key(&id);
        if !self
            .inner
            .process
            .lock()
            .await
            .as_ref()
            .is_some_and(process_state_is_usable)
        {
            anyhow::bail!("codex app-server request expired after its process exited");
        }
        self.write_message(&json!({
            "id": id,
            "error": {
                "code": -32000,
                "message": message
            }
        }))
        .await?;
        self.inner
            .pending_server_request_ids
            .lock()
            .await
            .remove(&request_id_key);
        Ok(())
    }

    async fn close_on_controller(&self) -> Result<()> {
        let process = self.inner.process.lock().await.take();
        if let Some(mut process) = process {
            fail_pending_requests(
                &self.inner,
                process.generation,
                "codex app-server client closed",
            )
            .await;
            if !process.handoff_proxy {
                terminate_managed_process_group(process.pid, process.process_identity).await;
            }
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

    fn touch_activity(&self) {
        self.inner
            .last_activity_at_ms
            .store(unix_time_ms(), Ordering::Relaxed);
    }

    async fn is_idle_for(&self, idle_for: Duration) -> bool {
        if self.has_live_work().await {
            return false;
        }
        unix_time_ms().saturating_sub(self.inner.last_activity_at_ms.load(Ordering::Relaxed))
            >= idle_for.as_millis().min(u128::from(u64::MAX)) as u64
    }

    async fn has_live_work(&self) -> bool {
        !self.inner.pending.lock().await.is_empty()
            || !self.inner.active_turn_ids.lock().await.is_empty()
            || !self
                .inner
                .pending_server_request_ids
                .lock()
                .await
                .is_empty()
    }

    async fn ensure_started(&self) -> Result<()> {
        let _guard = self.inner.start_lock.lock().await;
        let existing_process = {
            let mut process = self.inner.process.lock().await;
            match process.as_ref() {
                Some(process_state) if process_state_is_usable(process_state) => return Ok(()),
                Some(_) => process.take(),
                None => None,
            }
        };
        if let Some(process) = existing_process {
            warn!(
                profile_id = %self.inner.profile.id,
                "clearing stale codex app-server process state before restart"
            );
            self.terminate_unresponsive_process_state(
                process,
                "codex app-server process state was stale",
            )
            .await;
        }

        let process_state = self.spawn_process().await?;
        let used_handoff_connection = process_state.handoff_proxy;
        let mut process_state = process_state;
        let supervisor_start_tx = process_state.supervisor_start_tx.take();
        {
            let mut process = self.inner.process.lock().await;
            *process = Some(process_state);
        }
        if let Some(supervisor_start_tx) = supervisor_start_tx {
            let _ = supervisor_start_tx.send(());
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
            .await
        }
        .await
        {
            let _ = self.close_on_controller().await;
            if used_handoff_connection {
                warn!(
                    profile_id = %self.inner.profile.id,
                    error = %error,
                    "persistent codex app-server connection failed during initialization; preserving the daemon for a later reconnect"
                );
                return Err(error);
            }
            return Err(error);
        }

        Ok(())
    }

    async fn request_started(&self, method: String, params: Value) -> Result<Value> {
        self.request_started_with_timeout(method, params, self.inner.config.request_timeout, false)
            .await
    }

    async fn request_started_with_timeout(
        &self,
        method: String,
        params: Value,
        request_timeout: Duration,
        recover_handoff_on_timeout: bool,
    ) -> Result<Value> {
        let deadline = TokioInstant::now() + request_timeout;
        let id = self.inner.next_request_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        let (generation, writer) = {
            let process = self.inner.process.lock().await;
            let process = process
                .as_ref()
                .ok_or_else(|| anyhow!("codex app-server is not running"))?;
            (process.generation, process.writer.clone())
        };
        let method_name = method.clone();
        let encoded = serde_json::to_string(&json!({
            "id": id,
            "method": method,
            "params": params
        }))
        .context("failed to encode app-server message")?;
        self.inner.pending.lock().await.insert(
            id,
            PendingRequest {
                generation,
                sender: tx,
            },
        );

        let write_result = timeout_at(deadline, async {
            write_app_server_message(&writer, encoded).await
        })
        .await;
        match write_result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                self.inner.pending.lock().await.remove(&id);
                return Err(error);
            }
            Err(_) => {
                self.inner.pending.lock().await.remove(&id);
                let recovered = if recover_handoff_on_timeout {
                    self.recover_process_after_request_timeout(
                        Some(generation),
                        &format!("timed out writing codex app-server request: {method_name}"),
                    )
                    .await
                } else {
                    false
                };
                return Err(AppServerRequestTimeoutError {
                    method: method_name,
                    request_timeout,
                    recovered,
                }
                .into());
            }
        }

        match timeout_at(deadline, rx).await {
            Err(_) => {
                self.inner.pending.lock().await.remove(&id);
                let recovered = if recover_handoff_on_timeout {
                    self.recover_process_after_request_timeout(
                        Some(generation),
                        &format!("codex app-server request timed out: {method_name}"),
                    )
                    .await
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
                Ok(Ok(value)) => {
                    record_response_activity(&self.inner, &method_name, &value).await;
                    Ok(value)
                }
                Ok(Err(message)) => Err(anyhow!(message)),
                Err(_) => Err(anyhow!(
                    "codex app-server request channel closed before a response arrived"
                )),
            },
        }
    }

    async fn recover_process_after_request_timeout(
        &self,
        expected_generation: Option<u64>,
        reason: &str,
    ) -> bool {
        let active_turn_count = self.inner.active_turn_ids.lock().await.len();
        let pending_request_count = self.inner.pending.lock().await.len();
        let pending_server_request_count = self.inner.pending_server_request_ids.lock().await.len();
        if active_turn_count > 0 || pending_request_count > 0 || pending_server_request_count > 0 {
            warn!(
                profile_id = %self.inner.profile.id,
                client_key = %self.inner.client_key,
                active_turn_count,
                pending_request_count,
                pending_server_request_count,
                "{reason}; preserving app-server because it still owns live work"
            );
            return false;
        }
        let process = {
            let mut process = self.inner.process.lock().await;
            if process.as_ref().is_some_and(|process| {
                expected_generation.is_none_or(|generation| process.generation == generation)
            }) {
                process.take()
            } else {
                None
            }
        };
        let Some(process) = process else {
            return false;
        };
        let generation = process.generation;
        let was_handoff_proxy = process.handoff_proxy;

        warn!(
            profile_id = %self.inner.profile.id,
            client_key = %self.inner.client_key,
            handoff_proxy = was_handoff_proxy,
            "{reason}; discarding poisoned app-server process"
        );
        fail_pending_requests(&self.inner, generation, reason).await;
        self.terminate_unresponsive_process_state(process, reason)
            .await;
        if was_handoff_proxy {
            let _ = stop_handoff_server(
                &self.inner.config,
                &self.inner.client_key,
                &self.inner.profile,
            )
            .await;
        }
        true
    }

    async fn terminate_unresponsive_process_state(&self, mut process: ProcessState, reason: &str) {
        warn!(
            profile_id = %self.inner.profile.id,
            client_key = %self.inner.client_key,
            pid = ?process.pid,
            "{reason}; terminating codex app-server process group"
        );
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
                    client_key = %self.inner.client_key,
                    "timed out while waiting for unresponsive codex app-server supervisor to exit"
                );
                join_handle.abort();
            }
        }
    }

    async fn write_message(&self, payload: &Value) -> Result<()> {
        let writer = {
            let process = self.inner.process.lock().await;
            process
                .as_ref()
                .map(|state| state.writer.clone())
                .ok_or_else(|| anyhow!("codex app-server is not running"))?
        };

        let encoded =
            serde_json::to_string(payload).context("failed to encode app-server message")?;
        write_app_server_message(&writer, encoded).await
    }

    async fn spawn_process(&self) -> Result<ProcessState> {
        if self.inner.controller.process_slots.available_permits() == 0
            && let Some(clients) = self.inner.clients_registry.as_ref().and_then(Weak::upgrade)
        {
            let _ = evict_idle_clients_from_registry(
                &clients,
                self.inner.config.idle_client_timeout,
                1,
                Some(self.inner.client_key.as_str()),
            )
            .await;
        }
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
        let handoff_paths = match self.ensure_handoff_server_running().await {
            Ok(paths) => paths,
            Err(error)
                if error
                    .downcast_ref::<LiveHandoffDaemonUnavailable>()
                    .is_some() =>
            {
                return Err(error);
            }
            Err(error) => {
                warn!(
                    profile_id = %self.inner.profile.id,
                    client_key = %self.inner.client_key,
                    error = %error,
                    "failed to prepare persistent Codex app-server; falling back to stdio"
                );
                let _ = stop_handoff_server(
                    &self.inner.config,
                    &self.inner.client_key,
                    &self.inner.profile,
                )
                .await;
                None
            }
        };
        #[cfg(unix)]
        if let Some(paths) = handoff_paths {
            match self
                .connect_handoff_process(paths.clone(), process_slot)
                .await
            {
                Ok(process) => return Ok(process),
                Err((error, process_slot)) => {
                    warn!(
                        profile_id = %self.inner.profile.id,
                        socket = %paths.socket_path.display(),
                        error = %error,
                        "failed to connect to a live persistent Codex app-server; preserving it for a later reconnect"
                    );
                    drop(process_slot);
                    return Err(error);
                }
            }
        }
        #[cfg(not(unix))]
        let _ = handoff_paths;

        self.spawn_stdio_process(process_slot).await
    }

    async fn spawn_stdio_process(
        &self,
        process_slot: OwnedSemaphorePermit,
    ) -> Result<ProcessState> {
        let mut command = codex_command_from_app_server_args(
            &self.inner.config,
            vec![
                OsString::from("app-server"),
                OsString::from("--enable"),
                OsString::from("goals"),
                OsString::from("--listen"),
                OsString::from("stdio://"),
            ],
        );

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
        let writer = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("failed to capture codex app-server stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("failed to capture codex app-server stdout"))?;
        let stderr = child.stderr.take();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (supervisor_start_tx, supervisor_start_rx) = oneshot::channel();
        let inner = self.inner.clone();
        let generation = self
            .inner
            .next_process_generation
            .fetch_add(1, Ordering::SeqCst);

        let join_handle = tokio::spawn(async move {
            supervise_process(
                inner,
                generation,
                child,
                stdout,
                stderr,
                shutdown_rx,
                supervisor_start_rx,
            )
            .await;
        });

        Ok(ProcessState {
            generation,
            writer: Arc::new(Mutex::new(AppServerWriter::Stdio(writer))),
            pid,
            process_identity: read_managed_process_identity(pid),
            shutdown_tx: Some(shutdown_tx),
            join_handle,
            supervisor_start_tx: Some(supervisor_start_tx),
            handoff_proxy: false,
            _process_slot: process_slot,
        })
    }

    #[cfg(unix)]
    async fn connect_handoff_process(
        &self,
        paths: HandoffPaths,
        process_slot: OwnedSemaphorePermit,
    ) -> std::result::Result<ProcessState, (anyhow::Error, OwnedSemaphorePermit)> {
        let connect_result = async {
            let meta = tokio::fs::read(&paths.meta_path)
                .await
                .with_context(|| format!("failed to read {}", paths.meta_path.display()))
                .and_then(|bytes| {
                    serde_json::from_slice::<HandoffMeta>(&bytes)
                        .context("failed to decode Codex app-server handoff metadata")
                })?;
            if !handoff_meta_matches(
                &self.inner.config,
                &paths,
                &self.inner.client_key,
                &self.inner.profile,
                &meta,
            ) {
                anyhow::bail!("Codex app-server handoff metadata is stale or does not match");
            }

            let websocket = connect_handoff_websocket(&paths.socket_path).await?;
            let (outbound_tx, outbound_rx) = mpsc::channel(APP_SERVER_WEBSOCKET_QUEUE_CAPACITY);
            let (shutdown_tx, shutdown_rx) = oneshot::channel();
            let (supervisor_start_tx, supervisor_start_rx) = oneshot::channel();
            let generation = self
                .inner
                .next_process_generation
                .fetch_add(1, Ordering::SeqCst);
            let inner = self.inner.clone();
            let supervisor_paths = paths.clone();
            let join_handle = tokio::spawn(async move {
                supervise_handoff_connection(
                    inner,
                    generation,
                    supervisor_paths,
                    websocket,
                    outbound_rx,
                    shutdown_rx,
                    supervisor_start_rx,
                )
                .await;
            });

            Ok::<_, anyhow::Error>((
                generation,
                outbound_tx,
                shutdown_tx,
                supervisor_start_tx,
                join_handle,
                meta,
            ))
        }
        .await;

        match connect_result {
            Ok((generation, outbound_tx, shutdown_tx, supervisor_start_tx, join_handle, meta)) => {
                Ok(ProcessState {
                    generation,
                    writer: Arc::new(Mutex::new(AppServerWriter::WebSocket(outbound_tx))),
                    pid: Some(meta.pid),
                    process_identity: meta.process_identity,
                    shutdown_tx: Some(shutdown_tx),
                    join_handle,
                    supervisor_start_tx: Some(supervisor_start_tx),
                    handoff_proxy: true,
                    _process_slot: process_slot,
                })
            }
            Err(error) => Err((error, process_slot)),
        }
    }

    async fn ensure_handoff_server_running(&self) -> Result<Option<HandoffPaths>> {
        let Some(paths) = handoff_paths(
            &self.inner.config,
            &self.inner.client_key,
            &self.inner.profile,
        ) else {
            return Ok(None);
        };

        #[cfg(not(unix))]
        {
            let _ = paths;
            return Ok(None);
        }

        #[cfg(unix)]
        {
            let existing_meta = tokio::fs::read(&paths.meta_path)
                .await
                .ok()
                .and_then(|bytes| serde_json::from_slice::<HandoffMeta>(&bytes).ok());
            if existing_meta.as_ref().is_some_and(|meta| {
                handoff_meta_matches(
                    &self.inner.config,
                    &paths,
                    &self.inner.client_key,
                    &self.inner.profile,
                    meta,
                )
            }) {
                if existing_meta.as_ref().is_some_and(|meta| {
                    !meta
                        .enabled_features
                        .iter()
                        .any(|feature| feature == "goals")
                }) {
                    warn!(
                        profile_id = %self.inner.profile.id,
                        socket = %paths.socket_path.display(),
                        "reusing an older persistent Codex app-server without recorded goal support so active work is not interrupted"
                    );
                }
                for _ in 0..3 {
                    if handoff_socket_is_live(&paths.socket_path).await {
                        return Ok(Some(paths));
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                return Err(LiveHandoffDaemonUnavailable {
                    socket_path: paths.socket_path.clone(),
                }
                .into());
            }

            if handoff_socket_is_live(&paths.socket_path).await {
                warn!(
                    profile_id = %self.inner.profile.id,
                    socket = %paths.socket_path.display(),
                    "refusing to replace a live Codex app-server socket whose metadata cannot be verified"
                );
                return Err(LiveHandoffDaemonUnavailable {
                    socket_path: paths.socket_path.clone(),
                }
                .into());
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

            rotate_text_log_if_needed(&paths.log_path, 8 * 1024 * 1024, 3);
            let log = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&paths.log_path)
                .with_context(|| format!("failed to open {}", paths.log_path.display()))?;
            let log_for_stderr = log
                .try_clone()
                .with_context(|| format!("failed to clone {}", paths.log_path.display()))?;

            let mut command = codex_command_from_app_server_args(
                &self.inner.config,
                vec![
                    OsString::from("app-server"),
                    OsString::from("--enable"),
                    OsString::from("goals"),
                    OsString::from("--listen"),
                    OsString::from(format!("unix://{}", paths.socket_path.display())),
                ],
            );
            command
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

            let mut child = command.spawn().with_context(|| {
                format!(
                    "failed to spawn persistent {} app-server",
                    self.inner.config.codex_bin
                )
            })?;
            let pid = child.id().unwrap_or_default();
            let process_identity = read_managed_process_identity(Some(pid));
            let readiness = async {
                let deadline = tokio::time::Instant::now()
                    + self
                        .inner
                        .config
                        .startup_timeout
                        .min(Duration::from_secs(5));
                while tokio::time::Instant::now() < deadline {
                    if handoff_socket_is_live(&paths.socket_path).await {
                        let meta = HandoffMeta {
                            pid,
                            process_identity,
                            enabled_features: vec!["goals".to_string()],
                            client_key: self.inner.client_key.clone(),
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
                        return Ok(());
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(anyhow!(
                    "persistent codex app-server did not open {}",
                    paths.socket_path.display()
                ))
            }
            .await;

            match readiness {
                Ok(()) => {
                    info!(
                        profile_id = %self.inner.profile.id,
                        pid,
                        socket = %paths.socket_path.display(),
                        "started persistent codex app-server for restart handoff"
                    );
                    drop(child);
                    Ok(Some(paths))
                }
                Err(error) => {
                    terminate_managed_process_group(Some(pid), process_identity).await;
                    let _ = timeout(Duration::from_secs(2), child.wait()).await;
                    let _ = tokio::fs::remove_file(&paths.socket_path).await;
                    let _ = tokio::fs::remove_file(&paths.meta_path).await;
                    Err(error)
                }
            }
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
        self.get_or_create_with_key(profile.id.clone(), profile)
            .await
    }

    pub async fn get_or_create_with_key(
        &self,
        client_key: String,
        profile: AppServerProfile,
    ) -> AppServerClient {
        if self.controller.process_slots.available_permits() == 0 {
            let _ = self
                .evict_idle_clients_except(
                    self.config.idle_client_timeout,
                    1,
                    Some(client_key.as_str()),
                )
                .await;
        }

        loop {
            let mut clients = self.clients.lock().await;
            if let Some(existing) = clients.get(&client_key) {
                if existing.inner.profile == profile {
                    existing.touch_activity();
                    return existing.clone();
                }
                let stale_client = clients
                    .remove(&client_key)
                    .expect("existing app-server client should be removable");
                drop(clients);
                warn!(
                    client_key,
                    old_codex_home = %stale_client.inner.profile.codex_home.display(),
                    new_codex_home = %profile.codex_home.display(),
                    "replacing codex app-server client after runtime profile changed"
                );
                let _ = stale_client.close().await;
                continue;
            }

            let client = AppServerClient::with_controller(
                client_key.clone(),
                profile.clone(),
                self.config.clone(),
                Arc::clone(&self.controller),
                Some(Arc::downgrade(&self.clients)),
            );
            clients.insert(client_key.clone(), client.clone());
            return client;
        }
    }

    pub async fn evict_idle_clients(
        &self,
        idle_for: Duration,
        max_to_evict: usize,
    ) -> Result<Vec<String>> {
        self.evict_idle_clients_except(idle_for, max_to_evict, None)
            .await
    }

    pub fn spawn_idle_cleanup_loop(&self, interval: Duration) -> JoinHandle<()> {
        let manager = self.clone();
        tokio::spawn(async move {
            let mut timer = tokio::time::interval(interval);
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            timer.tick().await;
            loop {
                timer.tick().await;
                match manager
                    .evict_idle_clients(manager.config.idle_client_timeout, usize::MAX)
                    .await
                {
                    Ok(evicted) if !evicted.is_empty() => {
                        info!(
                            count = evicted.len(),
                            "evicted idle codex app-server clients"
                        );
                    }
                    Ok(_) => {}
                    Err(error) => {
                        warn!("failed to evict idle codex app-server clients: {error:#}");
                    }
                }
            }
        })
    }

    async fn evict_idle_clients_except(
        &self,
        idle_for: Duration,
        max_to_evict: usize,
        excluded_client_key: Option<&str>,
    ) -> Result<Vec<String>> {
        evict_idle_clients_from_registry(&self.clients, idle_for, max_to_evict, excluded_client_key)
            .await
    }

    pub async fn close_profile(&self, profile_id: &str) -> Result<()> {
        let clients = {
            let mut clients = self.clients.lock().await;
            let keys = clients
                .iter()
                .filter(|(_, client)| client.inner.profile.id == profile_id)
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|key| clients.remove(&key).map(|client| (key, client)))
                .collect::<Vec<_>>()
        };
        for (client_key, client) in clients {
            let profile = client.inner.profile.clone();
            client.close().await?;
            stop_handoff_server(&self.config, &client_key, &profile).await?;
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
            if client
                .inner
                .process
                .lock()
                .await
                .as_ref()
                .is_some_and(process_state_is_usable)
            {
                active += 1;
            }
        }
        active
    }

    pub async fn profile_has_active_process(&self, profile_id: &str) -> bool {
        let clients = {
            let clients = self.clients.lock().await;
            clients
                .values()
                .filter(|client| client.inner.profile.id == profile_id)
                .cloned()
                .collect::<Vec<_>>()
        };
        for client in clients {
            if client
                .inner
                .process
                .lock()
                .await
                .as_ref()
                .is_some_and(process_state_is_usable)
            {
                return true;
            }
        }
        false
    }

    pub async fn client_key_has_active_process(&self, profile_id: &str, client_key: &str) -> bool {
        let client = {
            let clients = self.clients.lock().await;
            clients.get(client_key).cloned()
        };
        let Some(client) = client.filter(|client| client.inner.profile.id == profile_id) else {
            return false;
        };
        client
            .inner
            .process
            .lock()
            .await
            .as_ref()
            .is_some_and(process_state_is_usable)
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
            if !process_state_is_usable(process) {
                continue;
            }
            status.active_process_count += 1;
            if process.handoff_proxy {
                status.handoff_proxy_process_count += 1;
            } else {
                status.stdio_process_count += 1;
            }
        }

        status
    }

    pub async fn prepare_restart_handoff(&self, handoff_enabled: bool) -> AppServerHandoffStatus {
        let clients = {
            let clients = self.clients.lock().await;
            clients.values().cloned().collect::<Vec<_>>()
        };
        let mut status = AppServerHandoffStatus {
            client_count: clients.len(),
            ..AppServerHandoffStatus::default()
        };
        let mut closable_clients = Vec::new();

        for client in &clients {
            let process_kind = {
                let process = client.inner.process.lock().await;
                process.as_ref().and_then(|process| {
                    process_state_is_usable(process).then_some(process.handoff_proxy)
                })
            };
            let Some(handoff_process) = process_kind else {
                continue;
            };
            status.active_process_count += 1;
            if handoff_process {
                status.handoff_proxy_process_count += 1;
            } else {
                status.stdio_process_count += 1;
            }

            if handoff_enabled && handoff_process {
                continue;
            }
            if client.has_live_work().await {
                status.blocking_process_count += 1;
                continue;
            }
            closable_clients.push(client.clone());
        }

        if status.blocking_process_count > 0 {
            return status;
        }

        for client in closable_clients {
            let client_for_close = client.clone();
            let close_result = client
                .inner
                .controller
                .handle
                .spawn(async move {
                    let _lifecycle = client_for_close.inner.lifecycle.write().await;
                    if client_for_close.has_live_work().await {
                        return Ok(false);
                    }
                    client_for_close.close_on_controller().await?;
                    Ok::<_, anyhow::Error>(true)
                })
                .await;
            match close_result {
                Ok(Ok(true)) => status.closed_idle_process_count += 1,
                Ok(Ok(false)) => status.blocking_process_count += 1,
                Ok(Err(error)) => {
                    warn!(
                        profile_id = %client.inner.profile.id,
                        client_key = %client.inner.client_key,
                        error = %error,
                        "failed to close idle Codex app-server before restart"
                    );
                    status.blocking_process_count += 1;
                }
                Err(error) => {
                    warn!(
                        profile_id = %client.inner.profile.id,
                        client_key = %client.inner.client_key,
                        error = %error,
                        "Codex app-server controller failed while preparing restart"
                    );
                    status.blocking_process_count += 1;
                }
            }
        }

        status
    }

    pub async fn process_snapshots_for_profiles(
        &self,
        profiles: Vec<AppServerProfile>,
    ) -> Vec<AppServerProcessSnapshot> {
        let clients = {
            let clients = self.clients.lock().await;
            clients
                .iter()
                .map(|(key, client)| (key.clone(), client.clone()))
                .collect::<Vec<_>>()
        };
        let mut snapshots = Vec::new();

        for (client_key, client) in &clients {
            let process = client.inner.process.lock().await;
            let Some(process) = process.as_ref() else {
                continue;
            };
            if !process_state_is_usable(process) {
                continue;
            }
            let Some(pid) = process.pid else {
                continue;
            };
            snapshots.push(AppServerProcessSnapshot {
                client_key: client_key.clone(),
                profile_id: client.inner.profile.id.clone(),
                codex_home: client.inner.profile.codex_home.clone(),
                pid,
                kind: if process.handoff_proxy {
                    "handoffProxy".to_string()
                } else {
                    "stdio".to_string()
                },
                handoff_proxy: process.handoff_proxy,
                socket_path: None,
                log_path: None,
                started_at_ms: None,
                codex_bin: client.inner.config.codex_bin.clone(),
                pending_request_count: client.inner.pending.lock().await.len(),
            });
        }

        for (client_key, client) in &clients {
            if let Some(snapshot) = self
                .handoff_process_snapshot_for_client_key(&client.inner.profile, client_key)
                .await
            {
                let duplicate = snapshots.iter().any(|existing| {
                    existing.client_key == snapshot.client_key
                        && existing.profile_id == snapshot.profile_id
                        && existing.pid == snapshot.pid
                        && existing.kind == snapshot.kind
                });
                if !duplicate {
                    snapshots.push(snapshot);
                }
            }
        }

        for profile in profiles {
            if let Some(snapshot) = self
                .handoff_process_snapshot_for_client_key(&profile, &profile.id)
                .await
            {
                let duplicate = snapshots.iter().any(|existing| {
                    existing.client_key == snapshot.client_key
                        && existing.profile_id == snapshot.profile_id
                        && existing.pid == snapshot.pid
                        && existing.kind == snapshot.kind
                });
                if !duplicate {
                    snapshots.push(snapshot);
                }
            }
        }

        snapshots.sort_by(|left, right| {
            left.profile_id
                .cmp(&right.profile_id)
                .then_with(|| left.client_key.cmp(&right.client_key))
                .then_with(|| left.kind.cmp(&right.kind))
                .then_with(|| left.pid.cmp(&right.pid))
        });
        snapshots
    }

    pub async fn force_kill_process(
        &self,
        profile: AppServerProfile,
        pid: u32,
    ) -> Result<AppServerProcessSnapshot> {
        if pid == 0 {
            anyhow::bail!("Invalid Codex process id.");
        }

        let clients = {
            let clients = self.clients.lock().await;
            clients
                .iter()
                .filter(|(_, client)| client.inner.profile.id == profile.id)
                .map(|(key, client)| (key.clone(), client.clone()))
                .collect::<Vec<_>>()
        };
        for (client_key, client) in &clients {
            let process = {
                let mut process = client.inner.process.lock().await;
                if process.as_ref().and_then(|process| process.pid) == Some(pid) {
                    process.take()
                } else {
                    None
                }
            };
            if let Some(process) = process {
                let snapshot = AppServerProcessSnapshot {
                    client_key: client_key.clone(),
                    profile_id: client.inner.profile.id.clone(),
                    codex_home: client.inner.profile.codex_home.clone(),
                    pid,
                    kind: if process.handoff_proxy {
                        "handoffProxy".to_string()
                    } else {
                        "stdio".to_string()
                    },
                    handoff_proxy: process.handoff_proxy,
                    socket_path: None,
                    log_path: None,
                    started_at_ms: None,
                    codex_bin: client.inner.config.codex_bin.clone(),
                    pending_request_count: client.inner.pending.lock().await.len(),
                };
                let reason = "codex app-server process force killed from settings";
                fail_pending_requests(&client.inner, process.generation, reason).await;
                client
                    .terminate_unresponsive_process_state(process, reason)
                    .await;
                return Ok(snapshot);
            }
        }

        for (client_key, client) in &clients {
            if let Some(snapshot) = self
                .handoff_process_snapshot_for_client_key(&client.inner.profile, client_key)
                .await
                && snapshot.pid == pid
            {
                stop_handoff_server(&self.config, client_key, &client.inner.profile).await?;
                return Ok(snapshot);
            }
        }

        if let Some(snapshot) = self
            .handoff_process_snapshot_for_client_key(&profile, &profile.id)
            .await
            && snapshot.pid == pid
        {
            stop_handoff_server(&self.config, &profile.id, &profile).await?;
            return Ok(snapshot);
        }

        anyhow::bail!("Codex process was not found or is no longer managed by this WebUI.")
    }

    async fn handoff_process_snapshot_for_client_key(
        &self,
        profile: &AppServerProfile,
        client_key: &str,
    ) -> Option<AppServerProcessSnapshot> {
        #[cfg(not(unix))]
        {
            let _ = (profile, client_key);
            None
        }
        #[cfg(unix)]
        {
            let paths = handoff_paths(&self.config, client_key, profile)?;
            let meta = tokio::fs::read(&paths.meta_path)
                .await
                .ok()
                .and_then(|bytes| serde_json::from_slice::<HandoffMeta>(&bytes).ok())?;
            if !handoff_meta_matches(&self.config, &paths, client_key, profile, &meta)
                || !handoff_socket_is_live(&paths.socket_path).await
            {
                return None;
            }
            Some(AppServerProcessSnapshot {
                client_key: client_key.to_string(),
                profile_id: profile.id.clone(),
                codex_home: profile.codex_home.clone(),
                pid: meta.pid,
                kind: "handoffDaemon".to_string(),
                handoff_proxy: false,
                socket_path: Some(paths.socket_path),
                log_path: Some(paths.log_path),
                started_at_ms: Some(meta.started_at_ms),
                codex_bin: meta.codex_bin,
                pending_request_count: 0,
            })
        }
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
            let client_key = client.inner.client_key.clone();
            client.close().await?;
            stop_handoff_server(&self.config, &client_key, &profile).await?;
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

async fn evict_idle_clients_from_registry(
    clients: &Arc<Mutex<HashMap<String, AppServerClient>>>,
    idle_for: Duration,
    max_to_evict: usize,
    excluded_client_key: Option<&str>,
) -> Result<Vec<String>> {
    if max_to_evict == 0 {
        return Ok(Vec::new());
    }
    let candidate_clients = {
        let clients = clients.lock().await;
        clients
            .iter()
            .filter(|(client_key, _)| excluded_client_key != Some(client_key.as_str()))
            .map(|(client_key, client)| (client_key.clone(), client.clone()))
            .collect::<Vec<_>>()
    };
    let mut candidates = Vec::with_capacity(candidate_clients.len());
    for (client_key, client) in candidate_clients {
        let has_process = client
            .inner
            .process
            .lock()
            .await
            .as_ref()
            .is_some_and(process_state_is_usable);
        candidates.push((
            client_key,
            client.clone(),
            has_process,
            client.inner.last_activity_at_ms.load(Ordering::Relaxed),
        ));
    }
    candidates.sort_by(|left, right| right.2.cmp(&left.2).then_with(|| left.3.cmp(&right.3)));

    let mut evicted = Vec::new();
    for (client_key, client, has_process, _) in candidates {
        if evicted.len() >= max_to_evict || !has_process {
            continue;
        }
        // Never wait on another client's active RPC while the caller may itself
        // hold a lifecycle read lock and be trying to free a process slot.
        let Ok(_lifecycle) = client.inner.lifecycle.try_write() else {
            continue;
        };
        if !client.is_idle_for(idle_for).await {
            continue;
        }
        let is_current = {
            let clients = clients.lock().await;
            clients
                .get(&client_key)
                .is_some_and(|current| Arc::ptr_eq(&current.inner, &client.inner))
        };
        if !is_current {
            continue;
        }
        // Keep the client registered. A request that was queued before cleanup
        // resumes after this write lock and starts a fresh tracked process.
        client.close().await?;
        evicted.push(client_key);
    }
    Ok(evicted)
}

fn handoff_paths(
    config: &AppServerClientConfig,
    client_key: &str,
    profile: &AppServerProfile,
) -> Option<HandoffPaths> {
    let handoff_dir = config.handoff_dir.as_ref()?;
    #[cfg(not(unix))]
    {
        let _ = client_key;
        let _ = profile;
        let _ = handoff_dir;
        return None;
    }
    #[cfg(unix)]
    {
        let mut hasher = Sha256::new();
        hasher.update(client_key.as_bytes());
        hasher.update(b"\0");
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
        let safe_profile = client_key
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
    let Ok(mut websocket) = connect_handoff_websocket(socket_path).await else {
        return false;
    };
    let _ = websocket.close(None).await;
    true
}

#[cfg(unix)]
async fn connect_handoff_websocket(socket_path: &Path) -> Result<WebSocketStream<UnixStream>> {
    let stream = timeout(Duration::from_secs(1), UnixStream::connect(socket_path))
        .await
        .context("timed out connecting to Codex app-server handoff socket")?
        .with_context(|| {
            format!(
                "failed to connect to Codex app-server handoff socket {}",
                socket_path.display()
            )
        })?;
    let websocket_config = WebSocketConfig::default()
        .max_frame_size(Some(APP_SERVER_HANDOFF_MAX_WEBSOCKET_MESSAGE_BYTES))
        .max_message_size(Some(APP_SERVER_HANDOFF_MAX_WEBSOCKET_MESSAGE_BYTES));
    let (websocket, _) = timeout(
        Duration::from_secs(1),
        client_async_with_config("ws://localhost/rpc", stream, Some(websocket_config)),
    )
    .await
    .context("timed out upgrading Codex app-server handoff WebSocket")?
    .with_context(|| {
        format!(
            "failed to upgrade Codex app-server handoff WebSocket {}",
            socket_path.display()
        )
    })?;
    Ok(websocket)
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
    #[cfg(target_os = "linux")]
    let process_group = format!("-{pid}");
    let pid_target = pid.to_string();
    #[cfg(target_os = "linux")]
    let _ = signal_managed_process_target("-TERM", &process_group).await;
    let _ = signal_managed_process_target("-TERM", &pid_target).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    if !managed_process_can_signal_group(pid, identity) {
        return;
    }
    #[cfg(target_os = "linux")]
    if signal_managed_process_target("-0", &process_group).await {
        let _ = signal_managed_process_target("-KILL", &process_group).await;
    }
    if signal_managed_process_target("-0", &pid_target).await {
        let _ = signal_managed_process_target("-KILL", &pid_target).await;
    }
}

#[cfg(unix)]
async fn signal_managed_process_target(signal: &str, target: &str) -> bool {
    Command::new("kill")
        .arg(signal)
        .arg("--")
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|status| status.success())
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

#[cfg(all(unix, not(target_os = "linux")))]
fn managed_process_identity_matches(_pid: u32, _expected: ManagedProcessIdentity) -> bool {
    false
}

#[cfg(unix)]
async fn stop_handoff_server(
    config: &AppServerClientConfig,
    client_key: &str,
    profile: &AppServerProfile,
) -> Result<()> {
    let Some(paths) = handoff_paths(config, client_key, profile) else {
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

    if !handoff_meta_matches(config, &paths, client_key, profile, &meta) {
        if handoff_socket_is_live(&paths.socket_path).await {
            warn!(
                pid = meta.pid,
                socket = %paths.socket_path.display(),
                "refusing to terminate codex app-server from stale or unverified handoff metadata"
            );
        } else {
            let _ = tokio::fs::remove_file(&paths.socket_path).await;
            let _ = tokio::fs::remove_file(&paths.meta_path).await;
        }
        return Ok(());
    }

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

    let _ = tokio::fs::remove_file(&paths.socket_path).await;
    let _ = tokio::fs::remove_file(&paths.meta_path).await;
    Ok(())
}

#[cfg(unix)]
fn handoff_meta_matches(
    config: &AppServerClientConfig,
    paths: &HandoffPaths,
    client_key: &str,
    profile: &AppServerProfile,
    meta: &HandoffMeta,
) -> bool {
    meta.pid > 0
        && meta.client_key == client_key
        && meta.profile_id == profile.id
        && meta.codex_home == profile.codex_home.display().to_string()
        && meta.socket_path == paths.socket_path.display().to_string()
        && meta.codex_bin == config.codex_bin
        && handoff_process_identity_matches(meta.pid, meta.process_identity)
}

#[cfg(target_os = "linux")]
fn handoff_process_identity_matches(pid: u32, identity: Option<ManagedProcessIdentity>) -> bool {
    identity.is_some_and(|identity| managed_process_identity_matches(pid, identity))
}

#[cfg(all(unix, not(target_os = "linux")))]
fn handoff_process_identity_matches(pid: u32, _identity: Option<ManagedProcessIdentity>) -> bool {
    StdCommand::new("kill")
        .arg("-0")
        .arg("--")
        .arg(pid.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(not(unix))]
async fn stop_handoff_server(
    _config: &AppServerClientConfig,
    _client_key: &str,
    _profile: &AppServerProfile,
) -> Result<()> {
    Ok(())
}

impl AppServerControllerRuntime {
    fn new(worker_threads: usize, max_processes: usize) -> Arc<Self> {
        let worker_threads = worker_threads.clamp(1, 4);
        let blocking_threads = worker_threads.saturating_mul(4).clamp(4, 32);
        let max_processes = max_processes.clamp(1, APP_SERVER_MAX_PROCESSES_HARD_CAP);
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
        if let Ok(mut shutdown_tx) = self.shutdown_tx.lock()
            && let Some(shutdown_tx) = shutdown_tx.take()
        {
            let _ = shutdown_tx.send(());
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
                .clamp(1, 2)
        })
        .clamp(1, 4)
}

fn default_max_process_count() -> usize {
    std::env::var("CODEX_WEBUI_MAX_APP_SERVERS")
        .ok()
        .and_then(|value| parse_max_process_count_override(value.trim()))
        .unwrap_or_else(|| {
            auto_max_process_count(
                std::thread::available_parallelism()
                    .map(usize::from)
                    .unwrap_or(1),
                detected_memory_limit_bytes(),
            )
        })
        .clamp(1, APP_SERVER_MAX_PROCESSES_HARD_CAP)
}

fn parse_max_process_count_override(value: &str) -> Option<usize> {
    if value.eq_ignore_ascii_case("unlimited") {
        return Some(APP_SERVER_MAX_PROCESSES_HARD_CAP);
    }
    if value.eq_ignore_ascii_case("auto") || value.trim() == "0" {
        return None;
    }
    value
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|value| *value > 0)
}

fn auto_max_process_count(available_cpus: usize, memory_limit_bytes: Option<u64>) -> usize {
    let cpu_limit = available_cpus
        .max(1)
        .div_ceil(APP_SERVER_DEFAULT_CPU_DIVISOR)
        .checked_sub(APP_SERVER_DEFAULT_CPU_SUB)
        .unwrap_or(1)
        .max(1);
    let memory_limit = memory_limit_bytes
        .map(|bytes| {
            (bytes.saturating_sub(APP_SERVER_GATEWAY_MEMORY_RESERVE_BYTES)
                / APP_SERVER_DEFAULT_MEMORY_BYTES_PER_PROCESS)
                .max(1) as usize
        })
        .unwrap_or(APP_SERVER_DEFAULT_MAX_PROCESSES_CAP);
    cpu_limit
        .min(memory_limit)
        .clamp(1, APP_SERVER_DEFAULT_MAX_PROCESSES_CAP)
}

fn detected_memory_limit_bytes() -> Option<u64> {
    cgroup_memory_limit_bytes().or_else(proc_mem_total_bytes)
}

fn cgroup_memory_limit_bytes() -> Option<u64> {
    let mut paths = vec![
        PathBuf::from("/sys/fs/cgroup/memory.max"),
        PathBuf::from("/sys/fs/cgroup/memory/memory.limit_in_bytes"),
    ];
    if let Ok(cgroup) = fs::read_to_string("/proc/self/cgroup") {
        for line in cgroup.lines() {
            let mut fields = line.splitn(3, ':');
            let hierarchy = fields.next().unwrap_or_default();
            let controllers = fields.next().unwrap_or_default();
            let relative = fields.next().unwrap_or_default().trim_start_matches('/');
            if hierarchy == "0" && controllers.is_empty() {
                paths.push(
                    Path::new("/sys/fs/cgroup")
                        .join(relative)
                        .join("memory.max"),
                );
            } else if controllers
                .split(',')
                .any(|controller| controller == "memory")
            {
                paths.push(
                    Path::new("/sys/fs/cgroup/memory")
                        .join(relative)
                        .join("memory.limit_in_bytes"),
                );
            }
        }
    }

    let mut detected = None;
    for path in paths {
        let Ok(value) = fs::read_to_string(&path) else {
            continue;
        };
        let trimmed = value.trim();
        if trimmed.eq_ignore_ascii_case("max") || trimmed.is_empty() {
            continue;
        }
        let Ok(bytes) = trimmed.parse::<u64>() else {
            continue;
        };
        if bytes > 0 && bytes < i64::MAX as u64 {
            detected = Some(detected.map_or(bytes, |current: u64| current.min(bytes)));
        }
    }
    detected
}

fn proc_mem_total_bytes() -> Option<u64> {
    let meminfo = fs::read_to_string("/proc/meminfo").ok()?;
    for line in meminfo.lines() {
        let Some(rest) = line.strip_prefix("MemTotal:") else {
            continue;
        };
        let kb = rest
            .split_whitespace()
            .next()
            .and_then(|value| value.parse::<u64>().ok())?;
        return kb.checked_mul(1024);
    }
    None
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

fn default_idle_client_timeout() -> Duration {
    let seconds = std::env::var("CODEX_WEBUI_APP_SERVER_IDLE_SECONDS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(APP_SERVER_DEFAULT_IDLE_TIMEOUT_SECONDS)
        .clamp(1, 86_400);
    Duration::from_secs(seconds)
}

fn default_drop_inherited_capabilities() -> bool {
    std::env::var("CODEX_WEBUI_DROP_CODEX_CAPS")
        .ok()
        .as_deref()
        .and_then(parse_env_bool)
        .unwrap_or(cfg!(target_os = "linux"))
}

fn parse_env_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

async fn write_app_server_message(
    writer: &Arc<Mutex<AppServerWriter>>,
    encoded: String,
) -> Result<()> {
    let mut writer = writer.lock().await;
    match &mut *writer {
        AppServerWriter::Stdio(stdin) => {
            stdin
                .write_all(encoded.as_bytes())
                .await
                .context("failed to write app-server message")?;
            stdin
                .write_all(b"\n")
                .await
                .context("failed to terminate app-server message")?;
            stdin
                .flush()
                .await
                .context("failed to flush app-server stdin")
        }
        #[cfg(unix)]
        AppServerWriter::WebSocket(outbound_tx) => outbound_tx
            .send(encoded)
            .await
            .map_err(|_| anyhow!("codex app-server WebSocket writer is closed")),
    }
}

async fn supervise_process(
    inner: Arc<AppServerClientInner>,
    generation: u64,
    mut child: Child,
    stdout: ChildStdout,
    stderr: Option<tokio::process::ChildStderr>,
    mut shutdown_rx: oneshot::Receiver<()>,
    supervisor_start_rx: oneshot::Receiver<()>,
) {
    if supervisor_start_rx.await.is_err() {
        let _ = child.kill().await;
        let _ = child.wait().await;
        return;
    }
    let mut stdout_task = tokio::spawn(read_stdout(inner.clone(), stdout));
    let stderr_task = tokio::spawn(read_stderr(stderr, inner.config.stderr_log_path.clone()));
    let pid = child.id();
    let process_identity = read_managed_process_identity(pid);
    let mut stdout_finished = false;

    let (exit_result, reader_failure) = tokio::select! {
        _ = &mut shutdown_rx => {
            let _ = child.kill().await;
            (child.wait().await, None)
        }
        result = child.wait() => (result, None),
        result = &mut stdout_task => {
            stdout_finished = true;
            let reason = match result {
                Ok(Ok(())) => "codex app-server stdout closed unexpectedly".to_string(),
                Ok(Err(error)) => format!("codex app-server stdout reader failed: {error:#}"),
                Err(error) => format!("codex app-server stdout reader task failed: {error}"),
            };
            terminate_managed_process_group(pid, process_identity).await;
            let _ = child.kill().await;
            (child.wait().await, Some(reason))
        }
    };

    let (reason, exit_code) = match (reader_failure, exit_result) {
        (Some(reason), Ok(status)) => (
            format!("{reason}; process exited ({status})"),
            status.code().map(Value::from).unwrap_or(Value::Null),
        ),
        (Some(reason), Err(error)) => (
            format!("{reason}; failed to wait for process exit: {error}"),
            Value::Null,
        ),
        (None, Ok(status)) => (
            format!("codex app-server exited ({status})"),
            status.code().map(Value::from).unwrap_or(Value::Null),
        ),
        (None, Err(error)) => (
            format!("failed to wait for codex app-server exit: {error}"),
            Value::Null,
        ),
    };

    finalize_process_exit(&inner, generation, reason, exit_code).await;

    if !stdout_finished {
        finish_reader_task(stdout_task, "stdout", &inner.profile.id).await;
    }
    finish_reader_task(stderr_task, "stderr", &inner.profile.id).await;
}

#[cfg(unix)]
async fn supervise_handoff_connection(
    inner: Arc<AppServerClientInner>,
    generation: u64,
    paths: HandoffPaths,
    websocket: WebSocketStream<UnixStream>,
    mut outbound_rx: mpsc::Receiver<String>,
    mut shutdown_rx: oneshot::Receiver<()>,
    supervisor_start_rx: oneshot::Receiver<()>,
) {
    let (mut websocket_writer, mut websocket_reader) = websocket.split();
    if supervisor_start_rx.await.is_err() {
        let _ = websocket_writer.close().await;
        return;
    }

    let reason = loop {
        tokio::select! {
            _ = &mut shutdown_rx => {
                let _ = websocket_writer.close().await;
                return;
            }
            outbound = outbound_rx.recv() => {
                let Some(encoded) = outbound else {
                    let _ = websocket_writer.close().await;
                    break "codex app-server handoff writer closed".to_string();
                };
                if let Err(error) = websocket_writer.send(Message::Text(encoded.into())).await {
                    break format!("failed to write Codex app-server handoff WebSocket: {error}");
                }
            }
            incoming = websocket_reader.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        handle_incoming_message(&inner, text.as_str()).await;
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        if let Err(error) = websocket_writer.send(Message::Pong(payload)).await {
                            break format!("failed to answer Codex app-server handoff ping: {error}");
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(frame))) => {
                        break frame
                            .map(|frame| format!("Codex app-server handoff WebSocket closed: {}", frame.reason))
                            .unwrap_or_else(|| "Codex app-server handoff WebSocket closed".to_string());
                    }
                    Some(Ok(Message::Binary(_))) | Some(Ok(Message::Frame(_))) => {}
                    Some(Err(error)) => {
                        break format!("Codex app-server handoff WebSocket failed: {error}");
                    }
                    None => break "Codex app-server handoff WebSocket reached EOF".to_string(),
                }
            }
        }
    };

    if handoff_daemon_is_live(&inner, &paths).await {
        finalize_handoff_connection_loss(&inner, generation, reason).await;
    } else {
        finalize_process_exit(&inner, generation, reason, Value::Null).await;
    }
}

#[cfg(unix)]
async fn handoff_daemon_is_live(inner: &AppServerClientInner, paths: &HandoffPaths) -> bool {
    let Some(meta) = tokio::fs::read(&paths.meta_path)
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice::<HandoffMeta>(&bytes).ok())
    else {
        return false;
    };
    if !handoff_meta_matches(
        &inner.config,
        paths,
        &inner.client_key,
        &inner.profile,
        &meta,
    ) {
        return false;
    }
    for _ in 0..3 {
        if handoff_socket_is_live(&paths.socket_path).await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

async fn finalize_handoff_connection_loss(
    inner: &Arc<AppServerClientInner>,
    generation: u64,
    reason: String,
) {
    let cleared_current_process = {
        let mut process = inner.process.lock().await;
        if process
            .as_ref()
            .is_some_and(|process| process.generation == generation)
        {
            *process = None;
            true
        } else {
            false
        }
    };

    fail_pending_requests(inner, generation, &reason).await;
    if cleared_current_process {
        inner.pending_server_request_ids.lock().await.clear();
        warn!(
            profile_id = %inner.profile.id,
            codex_home = %inner.profile.codex_home.display(),
            generation,
            "{reason}; persistent Codex app-server is still alive and will be reconnected"
        );
        let _ = inner.notifications_tx.send(AppServerNotification {
            method: "codex-webui/app-server/disconnected".to_string(),
            params: json!({ "reason": reason }),
        });
    }
}

async fn finalize_process_exit(
    inner: &Arc<AppServerClientInner>,
    generation: u64,
    reason: String,
    exit_code: Value,
) {
    let cleared_current_process = {
        let mut process = inner.process.lock().await;
        if process
            .as_ref()
            .is_some_and(|process| process.generation == generation)
        {
            *process = None;
            true
        } else {
            false
        }
    };

    fail_pending_requests(inner, generation, &reason).await;
    if cleared_current_process {
        inner.active_turn_ids.lock().await.clear();
        inner.pending_server_request_ids.lock().await.clear();
        warn!(
            profile_id = %inner.profile.id,
            codex_home = %inner.profile.codex_home.display(),
            generation,
            "{reason}"
        );
        let _ = inner.notifications_tx.send(AppServerNotification {
            method: "codex-webui/app-server/exited".to_string(),
            params: json!({
                "reason": reason,
                "exitCode": exit_code,
            }),
        });
    }
}

async fn finish_reader_task<T>(mut task: JoinHandle<T>, stream: &str, profile_id: &str) {
    if timeout(APP_SERVER_READER_CLEANUP_TIMEOUT, &mut task)
        .await
        .is_err()
    {
        warn!(
            profile_id,
            stream, "timed out draining exited codex app-server stream; aborting reader"
        );
        task.abort();
    }
}

async fn read_stdout(inner: Arc<AppServerClientInner>, stdout: ChildStdout) -> Result<()> {
    let mut reader = BufReader::new(stdout).lines();
    loop {
        let line = reader
            .next_line()
            .await
            .context("failed to read codex app-server stdout")?
            .ok_or_else(|| anyhow!("codex app-server stdout reached EOF"))?;
        handle_incoming_message(&inner, &line).await;
    }
}

async fn handle_incoming_message(inner: &Arc<AppServerClientInner>, line: &str) {
    inner
        .last_activity_at_ms
        .store(unix_time_ms(), Ordering::Relaxed);
    match classify_incoming_message(line) {
        Ok(Some(IncomingMessage::Response { id, payload })) => {
            if let Some(pending) = inner.pending.lock().await.remove(&id) {
                let _ = pending.sender.send(payload);
            }
        }
        Ok(Some(IncomingMessage::Notification(notification))) => {
            record_notification_activity(&inner, &notification).await;
            let _ = inner.notifications_tx.send(notification);
        }
        Ok(Some(IncomingMessage::Request(request))) => {
            inner
                .pending_server_request_ids
                .lock()
                .await
                .insert(server_request_id_key(&request.id));
            inner
                .last_activity_at_ms
                .store(unix_time_ms(), Ordering::Relaxed);
            let _ = inner.requests_tx.send(request);
        }
        Ok(None) => {}
        Err(error) => {
            warn!(
                profile_id = %inner.profile.id,
                "failed to parse Codex app-server message: {error:#}; line={line}"
            );
        }
    }
}

async fn record_notification_activity(
    inner: &Arc<AppServerClientInner>,
    notification: &AppServerNotification,
) {
    let turn_id = activity_turn_id(&notification.params);
    match notification.method.as_str() {
        "turn/started" => {
            if let Some(turn_id) = turn_id {
                inner
                    .active_turn_ids
                    .lock()
                    .await
                    .insert(turn_id.to_string());
            }
        }
        "turn/completed" => {
            if let Some(turn_id) = turn_id {
                inner.active_turn_ids.lock().await.remove(turn_id);
            }
        }
        _ => {}
    }
}

async fn record_response_activity(
    inner: &Arc<AppServerClientInner>,
    method: &str,
    response: &Value,
) {
    if !matches!(
        method,
        "turn/start" | "thread/compact/start" | "review/start"
    ) {
        return;
    }
    if let Some(turn_id) = activity_turn_id(response) {
        inner
            .active_turn_ids
            .lock()
            .await
            .insert(turn_id.to_string());
    }
}

fn activity_turn_id(payload: &Value) -> Option<&str> {
    payload
        .get("turn")
        .and_then(Value::as_object)
        .and_then(|turn| turn.get("id"))
        .and_then(Value::as_str)
        .or_else(|| payload.get("turnId").and_then(Value::as_str))
}

fn server_request_id_key(id: &Value) -> String {
    serde_json::to_string(id).unwrap_or_else(|_| id.to_string())
}

fn unix_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
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

async fn fail_pending_requests(inner: &Arc<AppServerClientInner>, generation: u64, reason: &str) {
    let pending = {
        let mut pending = inner.pending.lock().await;
        let request_ids = pending
            .iter()
            .filter_map(|(id, pending)| (pending.generation == generation).then_some(*id))
            .collect::<Vec<_>>();
        request_ids
            .into_iter()
            .filter_map(|id| pending.remove(&id).map(|pending| pending.sender))
            .collect::<Vec<_>>()
    };

    for sender in pending {
        let _ = sender.send(Err(reason.to_string()));
    }
}

fn process_state_is_alive(process: &ProcessState) -> bool {
    if process.join_handle.is_finished() {
        return false;
    }
    let Some(pid) = process.pid else {
        return true;
    };
    #[cfg(target_os = "linux")]
    {
        process
            .process_identity
            .is_some_and(|identity| managed_process_identity_matches(pid, identity))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        true
    }
}

fn process_state_is_usable(process: &ProcessState) -> bool {
    process_state_is_alive(process)
}

fn append_text_log_line(path: &Path, message: &str) {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return;
    }

    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    static LOG_WRITE_LOCK: OnceLock<StdMutex<()>> = OnceLock::new();
    let _guard = LOG_WRITE_LOCK
        .get_or_init(|| StdMutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    rotate_text_log_if_needed(path, 8 * 1024 * 1024, 3);
    let line = format!("{trimmed}\n");
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = std::io::Write::write_all(&mut file, line.as_bytes());
    }
}

fn rotate_text_log_if_needed(path: &Path, max_bytes: u64, backup_count: usize) {
    if backup_count == 0
        || fs::metadata(path)
            .map(|metadata| metadata.len() < max_bytes)
            .unwrap_or(true)
    {
        return;
    }

    let oldest = PathBuf::from(format!("{}.{}", path.display(), backup_count));
    let _ = fs::remove_file(oldest);
    for index in (1..backup_count).rev() {
        let from = PathBuf::from(format!("{}.{}", path.display(), index));
        let to = PathBuf::from(format!("{}.{}", path.display(), index + 1));
        if from.exists() {
            let _ = fs::rename(from, to);
        }
    }
    let first = PathBuf::from(format!("{}.1", path.display()));
    let _ = fs::rename(path, first);
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
        AppServerProfile, AppServerRequest, AppServerWriter, IncomingMessage, ProcessState,
        app_server_request_timed_out, app_server_timeout_recovered, classify_incoming_message,
        handoff_paths, rotate_text_log_if_needed, write_bytes_atomically,
    };
    #[cfg(unix)]
    use futures_util::{SinkExt, StreamExt};
    use serde_json::json;
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::time::Duration;
    use tokio::time::timeout;

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unix_handoff_transport_uses_websocket_text_frames() {
        use tokio::net::UnixListener;
        use tokio_tungstenite::{accept_async, tungstenite::Message};

        let directory = std::env::temp_dir().join(format!(
            "codex-webui-handoff-websocket-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let socket_path = directory.join("app-server.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut websocket = accept_async(stream).await.unwrap();
            let Some(Ok(Message::Text(payload))) = websocket.next().await else {
                panic!("expected a WebSocket text frame");
            };
            let request: serde_json::Value = serde_json::from_str(payload.as_str()).unwrap();
            websocket
                .send(Message::Text(
                    serde_json::to_string(&json!({
                        "id": request["id"],
                        "result": request["params"]
                    }))
                    .unwrap()
                    .into(),
                ))
                .await
                .unwrap();
        });

        let mut websocket = super::connect_handoff_websocket(&socket_path)
            .await
            .expect("Unix handoff WebSocket should connect");
        websocket
            .send(Message::Text(
                serde_json::to_string(&json!({
                    "id": 7,
                    "method": "echo",
                    "params": { "ok": true }
                }))
                .unwrap()
                .into(),
            ))
            .await
            .unwrap();
        let Some(Ok(Message::Text(response))) = websocket.next().await else {
            panic!("expected a WebSocket response");
        };
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(response.as_str()).unwrap(),
            json!({ "id": 7, "result": { "ok": true } })
        );

        server.await.unwrap();
        let _ = std::fs::remove_dir_all(directory);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unix_handoff_transport_accepts_large_single_frame_responses() {
        use tokio::net::UnixListener;
        use tokio_tungstenite::{accept_async, tungstenite::Message};

        let directory = std::env::temp_dir().join(format!(
            "codex-webui-handoff-large-frame-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let socket_path = directory.join("app-server.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let payload_bytes = (16 << 20) + 1;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut websocket = accept_async(stream).await.unwrap();
            websocket
                .send(Message::Text("x".repeat(payload_bytes).into()))
                .await
                .unwrap();
        });

        let mut websocket = super::connect_handoff_websocket(&socket_path)
            .await
            .expect("Unix handoff WebSocket should connect");
        let Some(Ok(Message::Text(response))) = websocket.next().await else {
            panic!("expected a large WebSocket response");
        };
        assert_eq!(response.len(), payload_bytes);

        server.await.unwrap();
        let _ = std::fs::remove_dir_all(directory);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn gateway_clients_reconnect_to_the_same_handoff_daemon() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        use tokio::net::UnixListener;
        use tokio::sync::oneshot;
        use tokio_tungstenite::{accept_async, tungstenite::Message};

        let directory = std::env::temp_dir().join(format!(
            "codex-webui-handoff-reconnect-{}",
            uuid::Uuid::new_v4()
        ));
        let handoff_dir = directory.join("handoff");
        let codex_home = directory.join("codex-home");
        std::fs::create_dir_all(&handoff_dir).unwrap();
        std::fs::create_dir_all(&codex_home).unwrap();
        let config = AppServerClientConfig {
            codex_bin: "codex-test".to_string(),
            handoff_dir: Some(handoff_dir),
            startup_timeout: Duration::from_secs(2),
            request_timeout: Duration::from_secs(2),
            ..AppServerClientConfig::default()
        };
        let profile = AppServerProfile {
            id: "default".to_string(),
            codex_home,
        };
        let paths = handoff_paths(&config, &profile.id, &profile).unwrap();
        if let Some(parent) = paths.socket_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let listener = UnixListener::bind(&paths.socket_path).unwrap();
        let pid = std::process::id();
        let meta = super::HandoffMeta {
            pid,
            process_identity: super::read_managed_process_identity(Some(pid)),
            enabled_features: vec!["goals".to_string()],
            client_key: profile.id.clone(),
            profile_id: profile.id.clone(),
            socket_path: paths.socket_path.display().to_string(),
            codex_bin: config.codex_bin.clone(),
            codex_home: profile.codex_home.display().to_string(),
            started_at_ms: 1,
        };
        write_bytes_atomically(&paths.meta_path, &serde_json::to_vec(&meta).unwrap())
            .await
            .unwrap();

        let accepted_connections = Arc::new(AtomicUsize::new(0));
        let accepted_for_server = Arc::clone(&accepted_connections);
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accepted = listener.accept() => {
                        let (stream, _) = accepted.unwrap();
                        accepted_for_server.fetch_add(1, Ordering::SeqCst);
                        tokio::spawn(async move {
                            let Ok(mut websocket) = accept_async(stream).await else {
                                return;
                            };
                            while let Some(message) = websocket.next().await {
                                match message {
                                    Ok(Message::Text(payload)) => {
                                        let request: serde_json::Value =
                                            serde_json::from_str(payload.as_str()).unwrap();
                                        let Some(id) = request.get("id") else {
                                            continue;
                                        };
                                        let result = if request.get("method") == Some(&json!("initialize")) {
                                            json!({ "serverInfo": { "name": "persistent-test" } })
                                        } else {
                                            request.get("params").cloned().unwrap_or_else(|| json!({}))
                                        };
                                        websocket
                                            .send(Message::Text(
                                                serde_json::to_string(&json!({
                                                    "id": id,
                                                    "result": result
                                                }))
                                                .unwrap()
                                                .into(),
                                            ))
                                            .await
                                            .unwrap();
                                    }
                                    Ok(Message::Ping(payload)) => {
                                        websocket.send(Message::Pong(payload)).await.unwrap();
                                    }
                                    Ok(Message::Close(_)) | Err(_) => break,
                                    _ => {}
                                }
                            }
                        });
                    }
                }
            }
        });

        let first_manager = AppServerManager::new(config.clone());
        let first_client = first_manager.get_or_create(profile.clone()).await;
        assert_eq!(
            first_client
                .request("echo", json!({ "gateway": 1 }))
                .await
                .unwrap(),
            json!({ "gateway": 1 })
        );
        first_manager.detach_all().await.unwrap();

        let second_manager = AppServerManager::new(config);
        let second_client = second_manager.get_or_create(profile).await;
        assert_eq!(
            second_client
                .request("echo", json!({ "gateway": 2 }))
                .await
                .unwrap(),
            json!({ "gateway": 2 })
        );
        second_manager.detach_all().await.unwrap();

        assert!(
            accepted_connections.load(Ordering::SeqCst) >= 4,
            "each gateway should probe and then attach to the persistent daemon"
        );
        let _ = shutdown_tx.send(());
        server.await.unwrap();
        let _ = std::fs::remove_dir_all(directory);
    }

    #[cfg(unix)]
    async fn install_synthetic_process(client: &AppServerClient, handoff: bool) {
        let process_slot = client
            .inner
            .controller
            .process_slots
            .clone()
            .acquire_owned()
            .await
            .unwrap();
        let (outbound_tx, _outbound_rx) = tokio::sync::mpsc::channel(1);
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let join_handle = tokio::spawn(async move {
            let _ = shutdown_rx.await;
        });
        *client.inner.process.lock().await = Some(ProcessState {
            generation: 1,
            writer: std::sync::Arc::new(tokio::sync::Mutex::new(AppServerWriter::WebSocket(
                outbound_tx,
            ))),
            pid: None,
            process_identity: None,
            shutdown_tx: Some(shutdown_tx),
            join_handle,
            supervisor_start_tx: None,
            handoff_proxy: handoff,
            _process_slot: process_slot,
        });
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn restart_preparation_closes_only_idle_legacy_processes() {
        let manager = AppServerManager::new(AppServerClientConfig {
            max_processes: 3,
            ..AppServerClientConfig::default()
        });
        let idle = manager
            .get_or_create(AppServerProfile {
                id: "idle".to_string(),
                codex_home: PathBuf::from("/tmp/codex-webui-idle"),
            })
            .await;
        install_synthetic_process(&idle, false).await;

        let status = manager.prepare_restart_handoff(true).await;
        assert_eq!(status.closed_idle_process_count, 1);
        assert_eq!(status.blocking_process_count, 0);
        assert!(idle.inner.process.lock().await.is_none());

        let active_legacy = manager
            .get_or_create(AppServerProfile {
                id: "active-legacy".to_string(),
                codex_home: PathBuf::from("/tmp/codex-webui-active-legacy"),
            })
            .await;
        install_synthetic_process(&active_legacy, false).await;
        active_legacy
            .inner
            .active_turn_ids
            .lock()
            .await
            .insert("turn-1".to_string());

        let status = manager.prepare_restart_handoff(true).await;
        assert_eq!(status.blocking_process_count, 1);
        assert_eq!(status.closed_idle_process_count, 0);
        assert!(active_legacy.inner.process.lock().await.is_some());

        active_legacy.inner.active_turn_ids.lock().await.clear();
        manager.close_all().await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn restart_preparation_preserves_active_handoff_processes() {
        let manager = AppServerManager::new(AppServerClientConfig::default());
        let client = manager
            .get_or_create(AppServerProfile {
                id: "handoff".to_string(),
                codex_home: PathBuf::from("/tmp/codex-webui-handoff"),
            })
            .await;
        install_synthetic_process(&client, true).await;
        client
            .inner
            .active_turn_ids
            .lock()
            .await
            .insert("turn-1".to_string());

        let status = manager.prepare_restart_handoff(true).await;
        assert_eq!(status.handoff_proxy_process_count, 1);
        assert_eq!(status.blocking_process_count, 0);
        assert!(client.inner.process.lock().await.is_some());

        manager.detach_all().await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handoff_connection_loss_preserves_active_turns_for_reconnect() {
        let manager = AppServerManager::new(AppServerClientConfig::default());
        let client = manager
            .get_or_create(AppServerProfile {
                id: "handoff-loss".to_string(),
                codex_home: PathBuf::from("/tmp/codex-webui-handoff-loss"),
            })
            .await;
        install_synthetic_process(&client, true).await;
        client
            .inner
            .active_turn_ids
            .lock()
            .await
            .insert("turn-1".to_string());
        let mut notifications = client.subscribe_notifications();

        super::finalize_handoff_connection_loss(
            &client.inner,
            1,
            "handoff transport interrupted".to_string(),
        )
        .await;

        assert!(client.inner.process.lock().await.is_none());
        assert!(client.inner.active_turn_ids.lock().await.contains("turn-1"));
        let notification = timeout(Duration::from_secs(1), notifications.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(notification.method, "codex-webui/app-server/disconnected");

        manager.detach_all().await.unwrap();
    }

    #[test]
    fn text_log_rotation_keeps_a_bounded_backup_chain() {
        let directory =
            std::env::temp_dir().join(format!("codex-webui-log-rotation-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("app-server.log");
        std::fs::write(&path, vec![b'x'; 32]).unwrap();

        rotate_text_log_if_needed(&path, 16, 2);
        assert!(!path.exists());
        assert_eq!(
            std::fs::read(path.with_extension("log.1")).unwrap().len(),
            32
        );

        std::fs::write(&path, vec![b'y'; 32]).unwrap();
        rotate_text_log_if_needed(&path, 16, 2);
        assert_eq!(
            std::fs::read(path.with_extension("log.1")).unwrap()[0],
            b'y'
        );
        assert_eq!(
            std::fs::read(path.with_extension("log.2")).unwrap()[0],
            b'x'
        );

        let _ = std::fs::remove_dir_all(directory);
    }

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

    #[cfg(unix)]
    #[tokio::test]
    async fn managed_process_signal_treats_missing_target_as_inactive() {
        assert!(!super::signal_managed_process_target("-0", "999999999").await);
        assert!(!super::signal_managed_process_target("-0", "-999999999").await);
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

    #[cfg(target_os = "linux")]
    #[test]
    fn codex_app_server_command_wraps_with_setpriv_when_cap_drop_is_enabled() {
        let (program, args) = super::codex_app_server_command_spec(
            "/usr/bin/codex",
            vec![OsString::from("app-server"), OsString::from("--listen")],
            true,
            true,
        );
        let args = args
            .into_iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();

        assert_eq!(program.to_string_lossy(), "setpriv");
        assert_eq!(
            args,
            vec![
                "--inh-caps=-all",
                "--ambient-caps=-all",
                "--",
                "/usr/bin/codex",
                "app-server",
                "--listen"
            ]
        );
    }

    #[test]
    fn codex_app_server_command_keeps_direct_command_when_cap_drop_is_disabled_or_unavailable() {
        let (program, args) = super::codex_app_server_command_spec(
            "/usr/bin/codex",
            vec![OsString::from("app-server")],
            false,
            true,
        );
        assert_eq!(program.to_string_lossy(), "/usr/bin/codex");
        assert_eq!(args, vec![OsString::from("app-server")]);

        let (program, args) = super::codex_app_server_command_spec(
            "/usr/bin/codex",
            vec![OsString::from("app-server")],
            true,
            false,
        );
        assert_eq!(program.to_string_lossy(), "/usr/bin/codex");
        assert_eq!(args, vec![OsString::from("app-server")]);
    }

    #[test]
    fn default_app_server_process_count_scales_with_cpu_and_memory() {
        assert_eq!(
            super::auto_max_process_count(1, Some(8 * 1024 * 1024 * 1024)),
            1
        );
        assert_eq!(
            super::auto_max_process_count(4, Some(4 * 1024 * 1024 * 1024)),
            1
        );
        assert_eq!(
            super::auto_max_process_count(16, Some(8 * 1024 * 1024 * 1024)),
            3
        );
        assert_eq!(
            super::auto_max_process_count(16, Some(1024 * 1024 * 1024)),
            1
        );
        assert_eq!(super::auto_max_process_count(16, None), 4);
    }

    #[test]
    fn app_server_process_count_override_distinguishes_auto_and_unlimited() {
        assert_eq!(super::parse_max_process_count_override("0"), None);
        assert_eq!(super::parse_max_process_count_override("auto"), None);
        assert_eq!(
            super::parse_max_process_count_override("unlimited"),
            Some(super::APP_SERVER_MAX_PROCESSES_HARD_CAP)
        );
        assert_eq!(super::parse_max_process_count_override("64"), Some(64));
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

        let first = handoff_paths(&config, &profile.id, &profile);
        let second = handoff_paths(&config, &profile.id, &profile);

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

args = sys.argv[1:]
if "app-server" in args and "--listen" in args and args[args.index("--listen") + 1] == "stdio://":
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
            log("config-write")
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
        assert!(
            !log.contains("config-write"),
            "startup feature flags are supplied on the command line and must not reload user config"
        );

        client.close().await.expect("client should close");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn manager_evicts_idle_client_before_process_cap_is_exhausted() {
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

args = sys.argv[1:]
if "app-server" in args and "--listen" in args and args[args.index("--listen") + 1] == "stdio://":
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
            startup_timeout: std::time::Duration::from_secs(1),
            request_timeout: std::time::Duration::from_secs(1),
            idle_client_timeout: std::time::Duration::ZERO,
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
        assert_eq!(
            second
                .request("echo", json!({ "profile": "other" }))
                .await
                .expect("second profile should evict the idle first client"),
            json!({ "profile": "other" })
        );
        assert_eq!(manager.active_process_count().await, 1);
        assert_eq!(manager.client_count().await, 2);
        second.close().await.expect("second client should close");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn manager_periodically_evicts_idle_clients_with_free_process_slots() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "codex-webui-periodic-idle-{}",
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
import time

for raw_line in sys.stdin:
    payload = json.loads(raw_line)
    method = payload.get("method")
    request_id = payload.get("id")
    if method == "initialized":
        continue
    if method == "slow":
        time.sleep(0.2)
    print(json.dumps({"id": request_id, "result": payload.get("params") or {}}), flush=True)
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
            max_processes: 8,
            startup_timeout: Duration::from_secs(1),
            request_timeout: Duration::from_secs(1),
            idle_client_timeout: Duration::from_millis(1),
            ..AppServerClientConfig::default()
        });
        let client = manager
            .get_or_create(AppServerProfile {
                id: "default".to_string(),
                codex_home: dir.join("codex-home"),
            })
            .await;
        let in_flight_client = client.clone();
        let in_flight = tokio::spawn(async move {
            in_flight_client
                .request("slow", json!({ "inFlight": true }))
                .await
        });
        timeout(Duration::from_secs(1), async {
            while manager.active_process_count().await == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("slow request should start an app-server process");

        let cleanup = manager.spawn_idle_cleanup_loop(Duration::from_millis(10));
        assert_eq!(
            in_flight
                .await
                .expect("request task should not panic")
                .expect("idle cleanup must not terminate an in-flight request"),
            json!({ "inFlight": true })
        );
        timeout(Duration::from_secs(1), async {
            while manager.active_process_count().await != 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("idle client should be evicted without exhausting process slots");
        cleanup.abort();
        assert_eq!(manager.client_count().await, 1);
        assert_eq!(
            client
                .request("echo", json!({ "restarted": true }))
                .await
                .expect("an evicted client should restart as the same tracked client"),
            json!({ "restarted": true })
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stdout_reader_failure_stops_and_restarts_the_managed_process() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "codex-webui-stdout-reader-{}",
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
import time

for raw_line in sys.stdin:
    payload = json.loads(raw_line)
    method = payload.get("method")
    request_id = payload.get("id")
    if method == "initialized":
        continue
    if method == "poison":
        sys.stdout.buffer.write(b"\xff\n")
        sys.stdout.buffer.flush()
        time.sleep(60)
        continue
    print(json.dumps({"id": request_id, "result": payload.get("params") or {}}), flush=True)
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
                startup_timeout: Duration::from_secs(1),
                request_timeout: Duration::from_secs(3),
                ..AppServerClientConfig::default()
            },
        );

        let error = timeout(Duration::from_secs(2), client.request("poison", json!({})))
            .await
            .expect("stdout failure should fail the pending request promptly")
            .expect_err("invalid stdout must not leave the request pending");
        assert!(
            error.to_string().contains("stdout reader failed"),
            "unexpected error: {error:#}"
        );
        assert_eq!(
            client
                .request("echo", json!({ "restarted": true }))
                .await
                .expect("the next request should start a fresh process"),
            json!({ "restarted": true })
        );

        client.close().await.expect("client should close");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn request_timeout_does_not_cancel_shared_startup() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "codex-webui-single-deadline-{}",
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
import time

def respond(payload, result=None):
    print(json.dumps({"id": payload.get("id"), "result": result or {}}), flush=True)

args = sys.argv[1:]
if "app-server" in args and "--listen" in args and args[args.index("--listen") + 1] == "stdio://":
    initialized = False
    for raw_line in sys.stdin:
        payload = json.loads(raw_line)
        method = payload.get("method")
        if method == "initialize":
            time.sleep(0.7)
            respond(payload, {"serverInfo": {"name": "fake"}})
        elif method == "initialized":
            initialized = True
        elif method == "config/batchWrite":
            respond(payload, {})
        elif method == "echo":
            if initialized:
                respond(payload, payload.get("params") or {})
            else:
                print(json.dumps({"id": payload.get("id"), "error": {"message": "not initialized"}}), flush=True)
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
                startup_timeout: std::time::Duration::from_secs(5),
                request_timeout: std::time::Duration::from_secs(5),
                ..AppServerClientConfig::default()
            },
        );

        let started_at = std::time::Instant::now();
        let error = client
            .request_with_timeout(
                "echo",
                json!({ "request": "single-deadline" }),
                std::time::Duration::from_millis(200),
                false,
            )
            .await
            .expect_err("short request should time out while startup continues");
        assert!(app_server_request_timed_out(&error));
        assert!(!app_server_timeout_recovered(&error));
        assert!(
            started_at.elapsed() < std::time::Duration::from_millis(600),
            "short request did not honor its deadline"
        );
        assert_eq!(
            client
                .request("echo", json!({ "request": "after-startup" }))
                .await
                .expect("shared startup should finish after the short caller leaves"),
            json!({ "request": "after-startup" })
        );

        client.close().await.expect("client should close");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exited_app_server_is_not_counted_active_and_restarts_on_next_request() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "codex-webui-exited-process-{}",
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

def respond(payload, result=None):
    print(json.dumps({"id": payload.get("id"), "result": result or {}}), flush=True)

args = sys.argv[1:]
if "app-server" in args and "--listen" in args and args[args.index("--listen") + 1] == "stdio://":
    for raw_line in sys.stdin:
        payload = json.loads(raw_line)
        method = payload.get("method")
        if method == "initialize":
            respond(payload, {"serverInfo": {"name": "fake"}})
        elif method == "initialized":
            pass
        elif method == "config/batchWrite":
            respond(payload, {})
        elif method == "echo":
            respond(payload, payload.get("params") or {})
            sys.exit(0)
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

        let manager = AppServerManager::new(AppServerClientConfig {
            codex_bin: script_path.display().to_string(),
            startup_timeout: std::time::Duration::from_secs(2),
            request_timeout: std::time::Duration::from_secs(2),
            ..AppServerClientConfig::default()
        });
        let client = manager
            .get_or_create(AppServerProfile {
                id: "default".to_string(),
                codex_home: dir.join("codex-home"),
            })
            .await;

        assert_eq!(
            client
                .request("echo", json!({ "request": 1 }))
                .await
                .expect("first request should complete before app-server exits"),
            json!({ "request": 1 })
        );
        for _ in 0..20 {
            if manager.active_process_count().await == 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert_eq!(
            manager.active_process_count().await,
            0,
            "exited app-server must not remain counted as active"
        );

        assert_eq!(
            client
                .request("echo", json!({ "request": 2 }))
                .await
                .expect("next request should restart app-server"),
            json!({ "request": 2 })
        );

        client.close().await.expect("client should close");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stdio_timeout_discards_poisoned_process_and_restarts() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "codex-webui-stdio-timeout-no-handoff-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let script_path = dir.join("fake-codex.py");
        let timeout_marker = dir.join("timed-out-once");
        std::fs::create_dir_all(&dir).expect("test dir should be created");
        std::fs::write(
            &script_path,
            r#"#!/usr/bin/env python3
import json
import os
import sys
import time

def respond(payload, result=None):
    print(json.dumps({"id": payload.get("id"), "result": result or {}}), flush=True)

args = sys.argv[1:]
timeout_marker = os.environ.get("FAKE_CODEX_TIMEOUT_MARKER")
if "app-server" in args and "--listen" in args and args[args.index("--listen") + 1] == "stdio://":
    for raw_line in sys.stdin:
        payload = json.loads(raw_line)
        method = payload.get("method")
        if method == "initialize":
            respond(payload, {"serverInfo": {"name": "fake"}})
        elif method == "initialized":
            pass
        elif method == "config/batchWrite":
            respond(payload, {})
        elif method == "echo":
            if timeout_marker and not os.path.exists(timeout_marker):
                open(timeout_marker, "w", encoding="utf-8").close()
                time.sleep(60)
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
                handoff_dir: None,
                startup_timeout: std::time::Duration::from_secs(2),
                request_timeout: std::time::Duration::from_secs(2),
                extra_env: HashMap::from([(
                    "FAKE_CODEX_TIMEOUT_MARKER".to_string(),
                    timeout_marker.display().to_string(),
                )]),
                ..AppServerClientConfig::default()
            },
        );

        let error = client
            .request_with_timeout(
                "echo",
                json!({ "via": "stdio" }),
                std::time::Duration::from_millis(250),
                true,
            )
            .await
            .expect_err("stdio app-server request should time out");
        assert!(app_server_request_timed_out(&error));
        assert!(
            app_server_timeout_recovered(&error),
            "stdio timeout should report that the poisoned process was discarded"
        );
        assert_eq!(
            client
                .request("echo", json!({ "via": "restarted-stdio" }))
                .await
                .expect("next request should start a fresh stdio app-server"),
            json!({ "via": "restarted-stdio" })
        );

        client.close().await.expect("client should close");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn incompatible_handoff_websocket_falls_back_to_stdio() {
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
proxy_pid_path = os.environ.get("FAKE_CODEX_PROXY_PID")
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
                startup_timeout: std::time::Duration::from_secs(1),
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
            .expect("client should fall back to stdio after an incompatible handoff endpoint");

        assert_eq!(response, json!({ "ok": true }));
        let log = std::fs::read_to_string(&log_path).expect("start log should exist");
        assert!(log.contains("handoff-server"));
        assert!(!log.contains("proxy"));
        assert!(log.contains("stdio"));

        client.close().await.expect("client should close");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "legacy raw-proxy fixture; direct WebSocket handoff is covered above"]
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
        let proxy_pid_path = dir.join("proxy.pid");
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
proxy_pid_path = os.environ.get("FAKE_CODEX_PROXY_PID")
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
    if proxy_pid_path:
        with open(proxy_pid_path, "w", encoding="utf-8") as handle:
            handle.write(str(os.getpid()))
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
                extra_env: HashMap::from([
                    (
                        "FAKE_CODEX_START_LOG".to_string(),
                        log_path.display().to_string(),
                    ),
                    (
                        "FAKE_CODEX_PROXY_PID".to_string(),
                        proxy_pid_path.display().to_string(),
                    ),
                ]),
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
        let mut proxy_pid_contents = None;
        for _ in 0..20 {
            if let Ok(contents) = std::fs::read_to_string(&proxy_pid_path) {
                proxy_pid_contents = Some(contents);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        let proxy_pid = proxy_pid_contents
            .expect("proxy pid should be recorded")
            .trim()
            .parse::<u32>()
            .expect("proxy pid should be numeric");
        let proxy_proc_path = PathBuf::from(format!("/proc/{proxy_pid}"));
        for _ in 0..20 {
            if !proxy_proc_path.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert!(
            !proxy_proc_path.exists(),
            "timed-out handoff proxy process should be terminated and reaped"
        );

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
    #[ignore = "legacy raw-proxy fixture; direct WebSocket handoff is covered above"]
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
