use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{Mutex, broadcast, oneshot},
    task::JoinHandle,
};
use tracing::{info, warn};

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
}

struct AppServerClientInner {
    profile: AppServerProfile,
    config: AppServerClientConfig,
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
        let (notifications_tx, _) = broadcast::channel(128);
        let (requests_tx, _) = broadcast::channel(128);

        Self {
            inner: Arc::new(AppServerClientInner {
                profile,
                config,
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
        self.ensure_started().await?;
        self.request_started(method.into(), params).await
    }

    pub async fn respond(&self, id: Value, result: Value) -> Result<()> {
        self.ensure_started().await?;
        self.write_message(&json!({
            "id": id,
            "result": result
        }))
        .await
    }

    pub async fn reject(&self, id: Value, message: impl Into<String>) -> Result<()> {
        self.ensure_started().await?;
        self.write_message(&json!({
            "id": id,
            "error": {
                "code": -32000,
                "message": message.into()
            }
        }))
        .await
    }

    pub async fn close(&self) -> Result<()> {
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
            let _ = self.close().await;
            return Err(error);
        }

        Ok(())
    }

    async fn request_started(&self, method: String, params: Value) -> Result<Value> {
        let id = self.inner.next_request_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.inner.pending.lock().await.insert(id, tx);

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

        match rx.await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(message)) => Err(anyhow!(message)),
            Err(_) => Err(anyhow!(
                "codex app-server request channel closed before a response arrived"
            )),
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
        command
            .arg("app-server")
            .arg("--listen")
            .arg("stdio://")
            .env("CODEX_HOME", &self.inner.profile.codex_home)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

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
}

impl AppServerManager {
    pub fn new(config: AppServerClientConfig) -> Self {
        Self {
            config,
            clients: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn get_or_create(&self, profile: AppServerProfile) -> AppServerClient {
        let mut clients = self.clients.lock().await;
        if let Some(existing) = clients.get(&profile.id) {
            return existing.clone();
        }

        let client = AppServerClient::new(profile.clone(), self.config.clone());
        clients.insert(profile.id, client.clone());
        client
    }

    pub async fn close_profile(&self, profile_id: &str) -> Result<()> {
        let client = self.clients.lock().await.remove(profile_id);
        if let Some(client) = client {
            client.close().await?;
        }
        Ok(())
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
            client.close().await?;
        }
        Ok(())
    }
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
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| "codex app-server returned an unknown error".to_string());
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
        AppServerNotification, AppServerRequest, IncomingMessage, classify_incoming_message,
    };
    use serde_json::json;

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
}
