use std::{
    collections::{HashMap, VecDeque},
    env, fs,
    net::{SocketAddr, TcpListener},
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{
        Request, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{
        HeaderMap, Method, StatusCode,
        header::{self, HeaderValue},
    },
    response::{IntoResponse, Redirect, Response},
    routing::{any, get},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use backend::codex_app_server::{
    AppServerClient, AppServerClientConfig, AppServerManager, AppServerNotification,
    AppServerProfile,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt, TryStreamExt};
use hmac::{Hmac, Mac};
use reqwest::multipart::{Form, Part};
use scrypt::{Params as ScryptParams, scrypt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use time::Duration as CookieDuration;
use tokio::{
    fs as tokio_fs,
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
    sync::{Mutex, broadcast, mpsc},
};
use tokio_util::io::StreamReader;
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

const AUTH_COOKIE: &str = "codex_webui_auth";
const PROFILE_COOKIE: &str = "codex_webui_profile";
const PROFILE_HEADER: &str = "x-codex-webui-profile-id";
const LOGIN_WINDOW_MS: u128 = 10 * 60 * 1000;
const LOGIN_MAX_ATTEMPTS: usize = 8;
const INTERNAL_HEADER: &str = "x-codex-webui-internal-token";
const CACHE_TTL: Duration = Duration::from_secs(15 * 60);
const GLOBAL_RELAY_KEY: &str = "__global__";
const CODEX_NPM_PACKAGE: &str = "@openai/codex";
const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const CODEX_USAGE_USER_AGENT: &str = "codex_cli_rs/0.120.0 (Codex Web UI)";
const NPM_VIEW_TIMEOUT: Duration = Duration::from_millis(2500);
const NPM_INSTALL_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const QUOTA_CACHE_TTL: Duration = Duration::from_secs(60);
const QUOTA_REQUEST_TIMEOUT: Duration = Duration::from_secs(12);
const TERMINAL_BUFFER_LIMIT: usize = 500_000;
const TERMINAL_RELAY_PREFIX: &str = "__terminal__:";
const STATIC_BASE_PLACEHOLDER: &str = "/__CODEX_WEBUI_BASE__";
const RUNTIME_ERROR_LOG_NAME: &str = "runtime-errors.jsonl";
const INTERNAL_NODE_LOG_NAME: &str = "internal-node.log";
const INTERNAL_NODE_STDERR_TAIL_LIMIT: usize = 120;

tokio::task_local! {
    static ACTIVE_PROFILE_ID: String;
}

fn session_relay_key(profile_id: &str, session_id: &str) -> String {
    format!("profile::{profile_id}::session::{session_id}")
}

fn global_relay_key(profile_id: &str) -> String {
    format!("profile::{profile_id}::{GLOBAL_RELAY_KEY}")
}

fn request_cache_key(profile_id: &str, request_id: &str) -> String {
    format!("profile::{profile_id}::request::{request_id}")
}

fn runtime_logs_dir(config: &Config) -> PathBuf {
    config.data_dir.join("logs")
}

fn runtime_error_log_path(config: &Config) -> PathBuf {
    runtime_logs_dir(config).join(RUNTIME_ERROR_LOG_NAME)
}

fn internal_node_log_path(config: &Config) -> PathBuf {
    runtime_logs_dir(config).join(INTERNAL_NODE_LOG_NAME)
}

fn append_text_log_line(path: &Path, message: &str) {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return;
    }

    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let line = format!("{} {trimmed}\n", now_millis());
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = std::io::Write::write_all(&mut file, line.as_bytes());
    }
}

fn append_runtime_error_log(config: &Config, source: &str, message: &str, details: Value) {
    let path = runtime_error_log_path(config);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let entry = json!({
        "atMs": now_millis(),
        "pid": std::process::id(),
        "source": source,
        "message": message,
        "details": details
    });

    if let Ok(line) = serde_json::to_string(&entry) {
        if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = std::io::Write::write_all(&mut file, line.as_bytes());
            let _ = std::io::Write::write_all(&mut file, b"\n");
        }
    }
}

async fn snapshot_stderr_tail(stderr_tail: &Arc<Mutex<VecDeque<String>>>) -> Vec<String> {
    stderr_tail.lock().await.iter().cloned().collect()
}

fn install_panic_logger(config: Arc<Config>) {
    std::panic::set_hook(Box::new(move |panic_info| {
        let location = panic_info
            .location()
            .map(|location| format!("{}:{}", location.file(), location.line()));
        let payload = panic_info
            .payload()
            .downcast_ref::<&str>()
            .map(|value| (*value).to_string())
            .or_else(|| panic_info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "panic without string payload".to_string());
        append_runtime_error_log(
            &config,
            "rust-gateway",
            "panic",
            json!({
                "payload": payload,
                "location": location,
                "backtrace": std::backtrace::Backtrace::force_capture().to_string()
            }),
        );
    }));
}

#[derive(Clone, Debug)]
struct Config {
    project_root: PathBuf,
    allowed_roots: Vec<PathBuf>,
    default_profile_id: String,
    profiles: HashMap<String, RuntimeProfile>,
    data_dir: PathBuf,
    base_path: String,
    static_dir: PathBuf,
    public_host: String,
    public_port: u16,
    internal_port: u16,
    internal_proxy_token: String,
    internal_base_url: String,
    node_entry: PathBuf,
    node_binary: String,
    codex_bin: String,
    password: Option<String>,
    password_hash: Option<String>,
    viewer_password: Option<String>,
    viewer_password_hash: Option<String>,
    hcaptcha_site_key: Option<String>,
    hcaptcha_secret_key: Option<String>,
    session_secret: Option<String>,
    cookie_same_site: SameSiteMode,
    cookie_secure_mode: CookieSecureMode,
    cors_allowed_origins: Vec<String>,
}

impl Config {
    fn hcaptcha_site_key(&self) -> Option<&str> {
        self.hcaptcha_site_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    fn hcaptcha_secret_key(&self) -> Option<&str> {
        self.hcaptcha_secret_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    fn hcaptcha_enabled(&self) -> bool {
        self.hcaptcha_site_key().is_some() && self.hcaptcha_secret_key().is_some()
    }
}

#[derive(Clone, Debug)]
struct RuntimeProfile {
    codex_home: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SameSiteMode {
    Strict,
    Lax,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CookieSecureMode {
    Auto,
    Always,
    Never,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum UserRole {
    Admin,
    Viewer,
}

#[derive(Clone, Debug)]
struct AuthContext {
    role: UserRole,
    profile_id: String,
}

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    app_servers: AppServerManager,
    http: reqwest::Client,
    login_attempts: Arc<Mutex<HashMap<String, Vec<u128>>>>,
    response_cache: Arc<Mutex<HashMap<String, CachedResponse>>>,
    static_asset_cache: Arc<Mutex<HashMap<String, CachedStaticAsset>>>,
    inflight_requests: Arc<Mutex<HashMap<String, Vec<mpsc::UnboundedSender<ServerEnvelope>>>>>,
    quota_cache: Arc<Mutex<HashMap<String, CachedQuota>>>,
    relays: Arc<Mutex<HashMap<String, broadcast::Sender<Value>>>>,
    terminals: Arc<Mutex<HashMap<String, Arc<TerminalSession>>>>,
}

#[derive(Clone)]
struct CachedResponse {
    created_at: Instant,
    message: ServerEnvelope,
}

#[derive(Clone)]
struct CachedQuota {
    created_at: Instant,
    payload: Value,
}

#[derive(Clone)]
struct CachedStaticAsset {
    bytes: Bytes,
    content_type: &'static str,
    cache_control: &'static str,
}

#[derive(Debug, Deserialize)]
struct RuntimeProfileShape {
    id: Option<String>,
    #[serde(alias = "codex_home", alias = "codexHome")]
    codex_home: Option<String>,
}

struct TerminalSession {
    summary: Mutex<TerminalSummaryState>,
    buffer: Mutex<String>,
    stdin: Mutex<Option<tokio::process::ChildStdin>>,
    relay: broadcast::Sender<Value>,
    pid: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
struct TerminalSummaryState {
    id: String,
    title: String,
    cwd: String,
    #[serde(rename = "createdAt")]
    created_at: u64,
    #[serde(rename = "lastActivityAt")]
    last_activity_at: u64,
    status: String,
    #[serde(rename = "exitCode")]
    exit_code: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct AuthFile {
    tokens: Option<AuthTokens>,
}

#[derive(Debug, Deserialize)]
struct AuthTokens {
    access_token: Option<String>,
    account_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UsageResponseShape {
    email: Option<String>,
    #[serde(alias = "planType")]
    plan_type: Option<String>,
    #[serde(alias = "rateLimit")]
    rate_limit: Option<UsageRateLimitShape>,
}

#[derive(Debug, Deserialize)]
struct UsageRateLimitShape {
    #[serde(alias = "primaryWindow")]
    primary_window: Option<UsageWindowShape>,
    #[serde(alias = "secondaryWindow")]
    secondary_window: Option<UsageWindowShape>,
}

#[derive(Debug, Deserialize)]
struct UsageWindowShape {
    #[serde(alias = "usedPercent")]
    used_percent: Option<f64>,
    #[serde(alias = "resetAfterSeconds")]
    reset_after_seconds: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum ClientEnvelope {
    Request {
        id: String,
        method: String,
        params: Value,
    },
    Ping {
        nonce: Option<String>,
    },
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum ServerEnvelope {
    Ready {
        #[serde(rename = "connectionId")]
        connection_id: String,
    },
    Response {
        id: String,
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    Event {
        #[serde(rename = "sessionId")]
        session_id: String,
        event: Value,
    },
    TerminalEvent {
        #[serde(rename = "terminalId")]
        terminal_id: String,
        event: Value,
    },
    GlobalEvent {
        event: Value,
    },
    Pong {
        nonce: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
struct LoginPayload {
    password: Option<String>,
    #[serde(alias = "hcaptchaToken", alias = "hcaptcha_token")]
    hcaptcha_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UploadFilePayload {
    name: String,
    mime_type: Option<String>,
    data_base64: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AuditLogEntry {
    id: String,
    at: u64,
    role: String,
    method: String,
    target: Option<String>,
    ok: bool,
    error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct DirectoryEntryPayload {
    name: String,
    path: String,
    #[serde(rename = "isDirectory")]
    is_directory: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct DirectoryPayload {
    #[serde(rename = "allowedRoots")]
    allowed_roots: Vec<DirectoryEntryPayload>,
    #[serde(rename = "currentPath")]
    current_path: Option<String>,
    #[serde(rename = "parentPath")]
    parent_path: Option<String>,
    entries: Vec<DirectoryEntryPayload>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct EditableFilePayload {
    path: String,
    #[serde(rename = "displayName")]
    display_name: String,
    content: String,
    language: String,
    writable: bool,
}

#[derive(Clone, Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

type ApiResult<T> = std::result::Result<T, ApiError>;

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

fn api_error(status: StatusCode, message: impl Into<String>) -> ApiError {
    ApiError {
        status,
        message: message.into(),
    }
}

impl TerminalSession {
    async fn summary(&self) -> TerminalSummaryState {
        self.summary.lock().await.clone()
    }

    async fn snapshot(&self) -> (TerminalSummaryState, String) {
        let summary = self.summary().await;
        let buffer = self.buffer.lock().await.clone();
        (summary, buffer)
    }

    async fn write_input(&self, data: &str) -> Result<()> {
        let mut stdin = self.stdin.lock().await;
        let writer = stdin
            .as_mut()
            .ok_or_else(|| anyhow!("Terminal input is no longer available."))?;
        writer
            .write_all(data.as_bytes())
            .await
            .context("failed to write terminal input")?;
        writer
            .flush()
            .await
            .context("failed to flush terminal input")?;
        self.summary.lock().await.last_activity_at = now_unix_ms();
        Ok(())
    }

    async fn append_output(&self, text: &str) {
        {
            let mut buffer = self.buffer.lock().await;
            buffer.push_str(text);
            trim_terminal_buffer(&mut buffer);
        }
        self.summary.lock().await.last_activity_at = now_unix_ms();
        let _ = self.relay.send(json!({
            "kind": "notification",
            "method": "terminal/output",
            "params": {
                "text": text
            }
        }));
    }

    async fn mark_exited(&self, exit_code: Option<i32>) {
        {
            let mut summary = self.summary.lock().await;
            summary.status = "exited".to_string();
            summary.exit_code = exit_code;
            summary.last_activity_at = now_unix_ms();
        }
        self.stdin.lock().await.take();
        let _ = self.relay.send(json!({
            "kind": "notification",
            "method": "terminal/exit",
            "params": {
                "exitCode": exit_code
            }
        }));
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = Arc::new(Config::from_env()?);
    install_panic_logger(config.clone());

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let result = async {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("failed to build reqwest client")?;

        let (mut child, internal_node_stderr_tail) = spawn_internal_node(&config).await?;
        if let Err(error) = wait_for_internal_node(&config, &http, &internal_node_stderr_tail).await {
            let _ = child.kill().await;
            return Err(error);
        }

        let state = AppState {
            config: config.clone(),
            app_servers: AppServerManager::new(AppServerClientConfig {
                codex_bin: config.codex_bin.clone(),
                stderr_log_path: Some(runtime_logs_dir(&config).join("codex-app-server.log")),
                ..AppServerClientConfig::default()
            }),
            http,
            login_attempts: Arc::new(Mutex::new(HashMap::new())),
            response_cache: Arc::new(Mutex::new(HashMap::new())),
            static_asset_cache: Arc::new(Mutex::new(HashMap::new())),
            inflight_requests: Arc::new(Mutex::new(HashMap::new())),
            quota_cache: Arc::new(Mutex::new(HashMap::new())),
            relays: Arc::new(Mutex::new(HashMap::new())),
            terminals: Arc::new(Mutex::new(HashMap::new())),
        };

        let router = Router::new()
            .route(&with_base(&config.base_path, "/ws"), get(handle_ws))
            .route("/", any(handle_http))
            .route("/{*path}", any(handle_http))
            .with_state(state.clone());

        let address: SocketAddr = format!("{}:{}", config.public_host, config.public_port)
            .parse()
            .context("invalid public listen address")?;
        info!("Rust gateway listening on http://{address}");

        let listener = tokio::net::TcpListener::bind(address)
            .await
            .context("failed to bind public listener")?;

        tokio::select! {
            server_result = axum::serve(listener, router) => {
                let _ = child.kill().await;
                let _ = state.app_servers.close_all().await;
                server_result.context("axum server terminated unexpectedly")
            }
            child_result = child.wait() => {
                let details = match child_result {
                    Ok(status) => json!({
                        "status": status.to_string(),
                        "internalBaseUrl": config.internal_base_url,
                        "internalNodeLogPath": internal_node_log_path(&config).display().to_string(),
                        "stderrTail": snapshot_stderr_tail(&internal_node_stderr_tail).await
                    }),
                    Err(error) => json!({
                        "error": error.to_string(),
                        "internalBaseUrl": config.internal_base_url,
                        "internalNodeLogPath": internal_node_log_path(&config).display().to_string(),
                        "stderrTail": snapshot_stderr_tail(&internal_node_stderr_tail).await
                    })
                };
                append_runtime_error_log(&config, "rust-gateway", "internal Node backend exited unexpectedly", details);
                let _ = state.app_servers.close_all().await;
                Err(anyhow!("internal Node backend exited unexpectedly"))
            }
        }
    }
    .await;

    if let Err(error) = &result {
        append_runtime_error_log(
            &config,
            "rust-gateway",
            "gateway fatal error",
            json!({
                "error": format!("{error:#}"),
                "internalNodeLogPath": internal_node_log_path(&config).display().to_string()
            }),
        );
    }

    result
}

impl Config {
    fn from_env() -> Result<Self> {
        let cwd = env::current_dir().context("failed to read current directory")?;
        load_dotenv(&cwd);
        let project_root = resolve_project_root(&cwd);
        let allowed_roots = parse_allowed_roots(&project_root);
        let base_path = normalize_base_path(env::var("CODEX_WEBUI_BASE_PATH").ok());
        let static_dir = project_root.join("build/static");
        let public_host = env::var("HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
        let public_port = parse_port(env::var("PORT").ok(), 4173)?;
        let internal_port = parse_port(
            env::var("CODEX_WEBUI_INTERNAL_PORT").ok(),
            choose_free_port()?,
        )?;
        let internal_proxy_token = Uuid::new_v4().to_string();
        let node_entry = project_root.join("build/internal/index.js");
        if !node_entry.exists() {
            return Err(anyhow!(
                "missing internal API build at {}. Run `pnpm build` in codex-webui first.",
                node_entry.display()
            ));
        }
        if !static_dir.exists() {
            return Err(anyhow!(
                "missing static frontend build at {}. Run `pnpm build` in codex-webui first.",
                static_dir.display()
            ));
        }

        let codex_home = resolve_codex_home()?;
        let (default_profile_id, profiles) = parse_runtime_profiles(&codex_home)?;

        Ok(Self {
            project_root,
            allowed_roots,
            default_profile_id,
            profiles,
            data_dir: env::var("CODEX_WEBUI_DATA_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| cwd.join(".data")),
            base_path,
            static_dir,
            public_host,
            public_port,
            internal_port,
            internal_proxy_token,
            internal_base_url: format!("http://127.0.0.1:{internal_port}"),
            node_entry,
            node_binary: env::var("NODE_BINARY").unwrap_or_else(|_| "node".to_string()),
            codex_bin: env::var("CODEX_WEBUI_CODEX_BIN").unwrap_or_else(|_| "codex".to_string()),
            password: env::var("CODEX_WEBUI_PASSWORD").ok(),
            password_hash: env::var("CODEX_WEBUI_PASSWORD_HASH").ok(),
            viewer_password: env::var("CODEX_WEBUI_VIEWER_PASSWORD").ok(),
            viewer_password_hash: env::var("CODEX_WEBUI_VIEWER_PASSWORD_HASH").ok(),
            hcaptcha_site_key: env::var("CODEX_WEBUI_HCAPTCHA_SITE_KEY").ok(),
            hcaptcha_secret_key: env::var("CODEX_WEBUI_HCAPTCHA_SECRET_KEY").ok(),
            session_secret: env::var("CODEX_WEBUI_SESSION_SECRET").ok(),
            cookie_same_site: parse_same_site(
                env::var("CODEX_WEBUI_COOKIE_SAMESITE").ok().as_deref(),
            ),
            cookie_secure_mode: parse_secure_mode(
                env::var("CODEX_WEBUI_COOKIE_SECURE").ok().as_deref(),
            ),
            cors_allowed_origins: parse_cors_origins(
                env::var("CODEX_WEBUI_CORS_ALLOWED_ORIGINS").ok(),
            )?,
        })
    }
}

fn sanitize_profile_id(input: &str) -> String {
    let sanitized = input
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|character| match character {
            'a'..='z' | '0'..='9' | '.' | '_' | '-' => character,
            _ => '-',
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if sanitized.is_empty() {
        "default".to_string()
    } else {
        sanitized
    }
}

fn parse_runtime_profiles(
    default_codex_home: &PathBuf,
) -> Result<(String, HashMap<String, RuntimeProfile>)> {
    let default_profile_id = sanitize_profile_id(
        &env::var("CODEX_WEBUI_DEFAULT_PROFILE_ID").unwrap_or_else(|_| "default".to_string()),
    );
    let raw_profiles = env::var("CODEX_WEBUI_PROFILES_JSON").ok();

    let Some(raw_profiles) = raw_profiles.filter(|value| !value.trim().is_empty()) else {
        let mut profiles = HashMap::new();
        profiles.insert(
            default_profile_id.clone(),
            RuntimeProfile {
                codex_home: default_codex_home.clone(),
            },
        );
        return Ok((default_profile_id, profiles));
    };

    let parsed: Vec<RuntimeProfileShape> =
        serde_json::from_str(&raw_profiles).context("invalid CODEX_WEBUI_PROFILES_JSON")?;
    let mut profiles = HashMap::new();

    for entry in parsed {
        let id = sanitize_profile_id(entry.id.as_deref().unwrap_or("default"));
        profiles
            .entry(id.clone())
            .or_insert_with(|| RuntimeProfile {
                codex_home: entry
                    .codex_home
                    .as_deref()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| default_codex_home.clone()),
            });
    }

    if profiles.is_empty() {
        profiles.insert(
            default_profile_id.clone(),
            RuntimeProfile {
                codex_home: default_codex_home.clone(),
            },
        );
    }

    let resolved_default_profile_id = if profiles.contains_key(&default_profile_id) {
        default_profile_id
    } else {
        profiles
            .keys()
            .next()
            .cloned()
            .unwrap_or_else(|| "default".to_string())
    };

    Ok((resolved_default_profile_id, profiles))
}

fn parse_allowed_roots(project_root: &Path) -> Vec<PathBuf> {
    let mut roots = env::var_os("CODEX_WEBUI_ALLOWED_ROOTS")
        .map(|value| {
            env::split_paths(&value)
                .map(|entry| {
                    normalize_path(if entry.is_absolute() {
                        entry
                    } else {
                        project_root.join(entry)
                    })
                })
                .filter(|entry| !entry.as_os_str().is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| {
            let fallback = project_root
                .parent()
                .filter(|parent| *parent != project_root)
                .unwrap_or(project_root);
            vec![fallback.to_path_buf()]
        });

    roots.dedup();
    roots
}

fn normalize_path(path: PathBuf) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(value) => normalized.push(value.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(value) => normalized.push(value),
        }
    }
    normalized
}

fn resolve_input_path(project_root: &Path, input: &str) -> PathBuf {
    let candidate = PathBuf::from(input);
    normalize_path(if candidate.is_absolute() {
        candidate
    } else {
        project_root.join(candidate)
    })
}

async fn real_path_safe(target: &Path) -> PathBuf {
    tokio_fs::canonicalize(target)
        .await
        .unwrap_or_else(|_| target.to_path_buf())
}

async fn resolved_allowed_roots(config: &Config) -> Vec<PathBuf> {
    let mut roots = Vec::with_capacity(config.allowed_roots.len());
    for root in &config.allowed_roots {
        roots.push(real_path_safe(root).await);
    }
    roots
}

fn path_is_within(root: &Path, candidate: &Path) -> bool {
    candidate == root || candidate.starts_with(root)
}

fn directory_entry_payload(path: &Path) -> DirectoryEntryPayload {
    DirectoryEntryPayload {
        name: path
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| path.display().to_string()),
        path: path.display().to_string(),
        is_directory: true,
    }
}

fn infer_editor_language(file_path: &Path) -> String {
    match file_path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .as_deref()
    {
        Some("ts" | "tsx") => "typescript",
        Some("js" | "mjs" | "cjs" | "jsx") => "javascript",
        Some("json") => "json",
        Some("toml") => "ini",
        Some("md") => "markdown",
        Some("yml" | "yaml") => "yaml",
        Some("svelte") => "html",
        Some("rs") => "rust",
        Some("py") => "python",
        Some("css") => "css",
        Some("sh") => "shell",
        _ => "plaintext",
    }
    .to_string()
}

fn query_param_value(query: Option<&str>, key: &str) -> Option<String> {
    query?.split('&').find_map(|entry| {
        let (raw_key, raw_value) = entry.split_once('=').unwrap_or((entry, ""));
        if raw_key != key {
            return None;
        }
        let decoded = raw_value.replace('+', "%20");
        urlencoding::decode(&decoded)
            .ok()
            .map(|value| value.into_owned())
    })
}

async fn list_directories_payload(
    state: &AppState,
    current_path: Option<&str>,
) -> ApiResult<Value> {
    let resolved_roots = resolved_allowed_roots(&state.config).await;
    let root_entries = resolved_roots
        .iter()
        .map(|root| directory_entry_payload(root))
        .collect::<Vec<_>>();

    let Some(current_path) = current_path.filter(|value| !value.trim().is_empty()) else {
        return Ok(serde_json::to_value(DirectoryPayload {
            allowed_roots: root_entries.clone(),
            current_path: None,
            parent_path: None,
            entries: root_entries,
        })
        .expect("directory payload should serialize"));
    };

    let candidate = resolve_input_path(&state.config.project_root, current_path);
    let resolved = real_path_safe(&candidate).await;
    if !resolved_roots
        .iter()
        .any(|root| path_is_within(root, &resolved))
    {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "The selected path is outside the allowed roots.",
        ));
    }

    let metadata = tokio_fs::metadata(&resolved).await.map_err(|_| {
        api_error(
            StatusCode::BAD_REQUEST,
            "The selected path is not a directory.",
        )
    })?;
    if !metadata.is_dir() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "The selected path is not a directory.",
        ));
    }

    let mut reader = tokio_fs::read_dir(&resolved).await.map_err(|_| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to read directory.",
        )
    })?;
    let mut entries = Vec::new();
    while let Some(entry) = reader.next_entry().await.map_err(|_| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to read directory.",
        )
    })? {
        let file_type = entry.file_type().await.map_err(|_| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to inspect directory entry.",
            )
        })?;
        if file_type.is_dir() {
            entries.push(directory_entry_payload(&entry.path()));
        }
    }
    entries.sort_by(|left, right| left.name.cmp(&right.name));

    let parent_path = if resolved_roots.iter().any(|root| root == &resolved) {
        None
    } else {
        resolved.parent().map(|parent| parent.display().to_string())
    };

    Ok(serde_json::to_value(DirectoryPayload {
        allowed_roots: root_entries,
        current_path: Some(resolved.display().to_string()),
        parent_path,
        entries,
    })
    .expect("directory payload should serialize"))
}

async fn resolve_editable_file_path(
    state: &AppState,
    profile_id: &str,
    file_path: &str,
) -> ApiResult<PathBuf> {
    if file_path.trim().is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "filePath is required."));
    }

    let candidate = resolve_input_path(&state.config.project_root, file_path);
    let existing = tokio_fs::canonicalize(&candidate).await.ok();
    let path_to_check = existing.unwrap_or_else(|| candidate.clone());

    let mut roots = resolved_allowed_roots(&state.config).await;
    let profile_root =
        real_path_safe(&resolve_runtime_profile(&state.config, profile_id).codex_home).await;
    roots.push(profile_root);

    if !roots
        .iter()
        .any(|root| path_is_within(root, &path_to_check))
    {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "This file is outside editable roots.",
        ));
    }

    Ok(candidate)
}

async fn read_editable_file_payload(
    state: &AppState,
    profile_id: &str,
    file_path: &str,
) -> ApiResult<Value> {
    let resolved_path = resolve_editable_file_path(state, profile_id, file_path).await?;
    let content = match tokio_fs::read_to_string(&resolved_path).await {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(_) => {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to read the selected file.",
            ));
        }
    };

    Ok(serde_json::to_value(EditableFilePayload {
        path: resolved_path.display().to_string(),
        display_name: resolved_path
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| resolved_path.display().to_string()),
        content,
        language: infer_editor_language(&resolved_path),
        writable: true,
    })
    .expect("editable file payload should serialize"))
}

async fn write_editable_file_payload(
    state: &AppState,
    profile_id: &str,
    file_path: &str,
    content: &str,
) -> ApiResult<Value> {
    let resolved_path = resolve_editable_file_path(state, profile_id, file_path).await?;
    if let Some(parent) = resolved_path.parent() {
        tokio_fs::create_dir_all(parent).await.map_err(|_| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to create parent directories for the file.",
            )
        })?;
    }
    tokio_fs::write(&resolved_path, content)
        .await
        .map_err(|_| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to write the selected file.",
            )
        })?;
    read_editable_file_payload(state, profile_id, &resolved_path.display().to_string()).await
}

async fn handle_http(State(state): State<AppState>, jar: CookieJar, request: Request) -> Response {
    let method = request.method().clone();
    let uri = request.uri().clone();
    let headers = request.headers().clone();
    let path = uri.path().to_string();

    match normalize_request_path(&state.config.base_path, &path) {
        NormalizedPath::Redirect(target) => return Redirect::temporary(&target).into_response(),
        NormalizedPath::OutsideBase => return (StatusCode::NOT_FOUND, "Not found").into_response(),
        NormalizedPath::Route(route_path) => {
            let origin = extract_origin(&headers);
            let cors_origin = allowed_cors_origin(&state.config, &origin);
            let requested_headers = headers
                .get("access-control-request-headers")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);

            if route_path.starts_with("/api/")
                && method == Method::OPTIONS
                && headers.contains_key("access-control-request-method")
            {
                if let Some(origin_value) = cors_origin {
                    let mut response = Response::new(Body::empty());
                    *response.status_mut() = StatusCode::NO_CONTENT;
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                    return response;
                }
                return (StatusCode::FORBIDDEN, "CORS origin is not allowed.").into_response();
            }

            if route_path.starts_with("/api/auth/") {
                return handle_auth_http(state, jar, method, route_path, headers, request)
                    .await
                    .into_response();
            }

            if route_path == "/api/account" || route_path.starts_with("/api/account/") {
                let Some(auth) = auth_context(&state.config, &jar) else {
                    let mut response =
                        json_error(StatusCode::UNAUTHORIZED, "Authentication required.");
                    if let Some(origin_value) = cors_origin {
                        apply_cors_headers(
                            response.headers_mut(),
                            &origin_value,
                            requested_headers.as_deref(),
                        );
                    }
                    return response;
                };

                let mut response =
                    handle_account_api_http(state, method, route_path, request, auth).await;
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            if route_path == "/api/directories" || route_path == "/api/editor" {
                let Some(auth) = auth_context(&state.config, &jar) else {
                    let mut response =
                        json_error(StatusCode::UNAUTHORIZED, "Authentication required.");
                    if let Some(origin_value) = cors_origin {
                        apply_cors_headers(
                            response.headers_mut(),
                            &origin_value,
                            requested_headers.as_deref(),
                        );
                    }
                    return response;
                };

                let mut response = match route_path.as_str() {
                    "/api/directories" => handle_directories_api_http(state, request).await,
                    "/api/editor" => handle_editor_api_http(state, request, auth).await,
                    _ => unreachable!(),
                };
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            if route_path.starts_with("/api/") {
                if auth_context(&state.config, &jar).is_none() {
                    let mut response =
                        json_error(StatusCode::UNAUTHORIZED, "Authentication required.");
                    if let Some(origin_value) = cors_origin {
                        apply_cors_headers(
                            response.headers_mut(),
                            &origin_value,
                            requested_headers.as_deref(),
                        );
                    }
                    return response;
                }

                let mut response =
                    proxy_to_internal(state, method, &route_path, uri.query(), headers, request)
                        .await;
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            return serve_static_asset(state, &route_path).await;
        }
    }
}

async fn handle_account_api_http(
    state: AppState,
    method: Method,
    route_path: String,
    request: Request,
    auth: AuthContext,
) -> Response {
    let result = match (method, route_path.as_str()) {
        (Method::GET, "/api/account") => get_account_state(&state, &auth.profile_id).await,
        (Method::POST, "/api/account/login") => {
            let body = to_bytes(request.into_body(), usize::MAX)
                .await
                .context("failed to read account login request body");
            match body {
                Ok(body) => {
                    let payload: Value =
                        serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
                    start_account_login(&state, &auth.profile_id, &payload).await
                }
                Err(error) => Err(error),
            }
        }
        (Method::POST, "/api/account/login/cancel") => {
            let body = to_bytes(request.into_body(), usize::MAX)
                .await
                .context("failed to read account login cancel request body");
            match body {
                Ok(body) => {
                    let payload: Value =
                        serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
                    cancel_account_login(&state, &auth.profile_id, &payload).await
                }
                Err(error) => Err(error),
            }
        }
        (Method::POST, "/api/account/logout") => logout_account(&state, &auth.profile_id).await,
        _ => return json_error(StatusCode::NOT_FOUND, "Not found."),
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => {
            let message = error.to_string();
            let status = if message.contains("required")
                || message.contains("Invalid account login type")
                || message.contains("API key is required")
            {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::BAD_GATEWAY
            };
            json_error(status, &message)
        }
    }
}

async fn handle_directories_api_http(state: AppState, request: Request) -> Response {
    if request.method() != Method::GET {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }

    let current_path = query_param_value(request.uri().query(), "path");
    match list_directories_payload(&state, current_path.as_deref()).await {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

async fn handle_editor_api_http(state: AppState, request: Request, auth: AuthContext) -> Response {
    let method = request.method().clone();
    let result = match method {
        Method::GET => {
            let file_path =
                query_param_value(request.uri().query(), "filePath").unwrap_or_default();
            read_editable_file_payload(&state, &auth.profile_id, &file_path).await
        }
        Method::PUT => {
            if auth.role != UserRole::Admin {
                return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
            }

            let body = to_bytes(request.into_body(), usize::MAX)
                .await
                .context("failed to read editor request body");
            match body {
                Ok(body) => {
                    let payload: Value =
                        serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
                    let file_path = payload
                        .get("filePath")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let content = payload
                        .get("content")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    write_editable_file_payload(&state, &auth.profile_id, file_path, content).await
                }
                Err(_) => Err(api_error(
                    StatusCode::BAD_REQUEST,
                    "Failed to read editor request body.",
                )),
            }
        }
        _ => return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed."),
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

async fn handle_ws(
    State(state): State<AppState>,
    jar: CookieJar,
    ws: WebSocketUpgrade,
) -> Response {
    let Some(auth) = auth_context(&state.config, &jar) else {
        return (StatusCode::UNAUTHORIZED, "Authentication required.").into_response();
    };

    ws.on_upgrade(move |socket| websocket_session(socket, state, auth))
        .into_response()
}

async fn handle_auth_http(
    state: AppState,
    jar: CookieJar,
    method: Method,
    route_path: String,
    headers: HeaderMap,
    request: Request,
) -> Response {
    let origin = extract_origin(&headers);
    let cors_origin = allowed_cors_origin(&state.config, &origin);
    let requested_headers = headers
        .get("access-control-request-headers")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    if method == Method::OPTIONS {
        if let Some(origin_value) = cors_origin {
            let mut response = Response::new(Body::empty());
            *response.status_mut() = StatusCode::NO_CONTENT;
            apply_cors_headers(
                response.headers_mut(),
                &origin_value,
                requested_headers.as_deref(),
            );
            return response;
        }
        return (StatusCode::FORBIDDEN, "CORS origin is not allowed.").into_response();
    }

    let result = match (method, route_path.as_str()) {
        (Method::POST, "/api/auth/login") => auth_login(state.clone(), jar, headers, request).await,
        (Method::POST, "/api/auth/logout") => Ok(auth_logout(jar)),
        (Method::POST, "/api/auth/profile") => {
            let Some(auth) = auth_context(&state.config, &jar) else {
                return json_error(StatusCode::UNAUTHORIZED, "Authentication required.");
            };
            select_profile(state.config.clone(), jar, headers, request, auth).await
        }
        (Method::GET, "/api/auth/session") => {
            let auth = auth_context(&state.config, &jar);
            let active_profile_id = auth
                .as_ref()
                .map(|context| context.profile_id.as_str())
                .unwrap_or(&state.config.default_profile_id);
            Ok((
                jar,
                Json(json!({
                    "authenticated": auth.is_some(),
                    "activeProfileId": active_profile_id,
                    "role": auth.map(|context| match context.role {
                        UserRole::Admin => "admin",
                        UserRole::Viewer => "viewer",
                    }),
                    "hcaptcha": {
                        "enabled": state.config.hcaptcha_enabled(),
                        "siteKey": state.config.hcaptcha_site_key(),
                    }
                })),
            )
                .into_response())
        }
        _ => Ok((StatusCode::NOT_FOUND, "Not found").into_response()),
    };

    let mut response = match result {
        Ok(response) => response,
        Err(error_message) => json_error(StatusCode::UNAUTHORIZED, &error_message),
    };

    if let Some(origin_value) = cors_origin {
        apply_cors_headers(
            response.headers_mut(),
            &origin_value,
            requested_headers.as_deref(),
        );
    }

    response
}

async fn auth_login(
    state: AppState,
    jar: CookieJar,
    headers: HeaderMap,
    request: Request,
) -> std::result::Result<Response, String> {
    let secure_request = request_is_secure(&headers);
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .map_err(|_| "Invalid request body.".to_string())?;
    let payload: LoginPayload = serde_json::from_slice(&body).unwrap_or(LoginPayload {
        password: None,
        hcaptcha_token: None,
    });
    let password = payload.password.unwrap_or_default();
    let identifier = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown")
        .to_string();

    if !check_rate_limit(&state, &identifier).await {
        return Ok(json_error(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many login attempts. Try again later.",
        ));
    }

    if state.config.hcaptcha_enabled() {
        let Some(hcaptcha_secret_key) = state.config.hcaptcha_secret_key() else {
            return Ok(json_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "hCaptcha is not fully configured.",
            ));
        };
        let Some(hcaptcha_token) = payload
            .hcaptcha_token
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(json_error(
                StatusCode::BAD_REQUEST,
                "Complete the hCaptcha challenge before signing in.",
            ));
        };

        let mut verification_payload = vec![
            ("secret", hcaptcha_secret_key.to_string()),
            ("response", hcaptcha_token.to_string()),
        ];
        if identifier != "unknown" {
            verification_payload.push(("remoteip", identifier.clone()));
        }

        let verification_response = state
            .http
            .post("https://api.hcaptcha.com/siteverify")
            .form(&verification_payload)
            .send()
            .await
            .map_err(|error| {
                tracing::warn!("failed to verify hcaptcha: {error}");
                "Failed to verify hCaptcha."
            })?;

        if !verification_response.status().is_success() {
            tracing::warn!(
                status = %verification_response.status(),
                "hcaptcha verification request returned a non-success status"
            );
            return Ok(json_error(
                StatusCode::BAD_GATEWAY,
                "Failed to verify hCaptcha.",
            ));
        }

        let verification_result: Value = verification_response.json().await.map_err(|error| {
            tracing::warn!("failed to parse hcaptcha verification response: {error}");
            "Failed to verify hCaptcha."
        })?;
        let verification_ok = verification_result
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        if !verification_ok {
            record_login_failure(&state, &identifier).await;
            let _ = append_audit_log(
                &state.config,
                AuditLogEntry {
                    id: Uuid::new_v4().to_string(),
                    at: now_unix_ms(),
                    role: "anonymous".to_string(),
                    method: "auth/login".to_string(),
                    target: None,
                    ok: false,
                    error: Some("Failed hCaptcha verification.".to_string()),
                },
            )
            .await;
            return Ok(json_error(
                StatusCode::UNAUTHORIZED,
                "Complete the hCaptcha challenge before signing in.",
            ));
        }
    }

    let Some(role) =
        authenticate_role(&state.config, &password).map_err(|error| error.to_string())?
    else {
        record_login_failure(&state, &identifier).await;
        let _ = append_audit_log(
            &state.config,
            AuditLogEntry {
                id: Uuid::new_v4().to_string(),
                at: now_unix_ms(),
                role: "anonymous".to_string(),
                method: "auth/login".to_string(),
                target: None,
                ok: false,
                error: Some("Invalid password.".to_string()),
            },
        )
        .await;
        return Ok(json_error(StatusCode::UNAUTHORIZED, "Invalid password."));
    };

    clear_login_failures(&state, &identifier).await;
    let next_jar = issue_auth_cookie(&state.config, jar, secure_request, role)
        .map_err(|error| error.to_string())?;
    let _ = append_audit_log(
        &state.config,
        AuditLogEntry {
            id: Uuid::new_v4().to_string(),
            at: now_unix_ms(),
            role: match role {
                UserRole::Admin => "admin".to_string(),
                UserRole::Viewer => "viewer".to_string(),
            },
            method: "auth/login".to_string(),
            target: None,
            ok: true,
            error: None,
        },
    )
    .await;
    Ok((
        next_jar,
        Json(json!({
            "ok": true,
            "role": match role {
                UserRole::Admin => "admin",
                UserRole::Viewer => "viewer",
            }
        })),
    )
        .into_response())
}

fn auth_logout(jar: CookieJar) -> Response {
    let mut cookie = Cookie::new(AUTH_COOKIE, "");
    cookie.set_path("/");
    cookie.set_max_age(CookieDuration::seconds(0));
    let mut profile_cookie = Cookie::new(PROFILE_COOKIE, "");
    profile_cookie.set_path("/");
    profile_cookie.set_max_age(CookieDuration::seconds(0));
    (
        jar.remove(cookie).remove(profile_cookie),
        Json(json!({ "ok": true })),
    )
        .into_response()
}

async fn proxy_to_internal(
    state: AppState,
    method: Method,
    route_path: &str,
    query: Option<&str>,
    headers: HeaderMap,
    request: Request,
) -> Response {
    let mut target = format!("{}{}", state.config.internal_base_url, route_path);
    if let Some(query) = query.filter(|value| !value.is_empty()) {
        target.push('?');
        target.push_str(query);
    }

    let body = match to_bytes(request.into_body(), usize::MAX).await {
        Ok(bytes) => bytes,
        Err(_) => return (StatusCode::BAD_REQUEST, "Failed to read request body.").into_response(),
    };

    match forward_request(&state, method, &target, headers, body.to_vec(), None, None).await {
        Ok(response) => response,
        Err(error) => {
            error!("proxy error: {error:#}");
            json_error(StatusCode::BAD_GATEWAY, "Failed to proxy frontend request.")
        }
    }
}

async fn serve_static_asset(state: AppState, route_path: &str) -> Response {
    let Some(relative_path) = sanitize_static_relative_path(route_path) else {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    };

    let cache_key = relative_path.to_string_lossy().into_owned();
    if let Some(cached) = state
        .static_asset_cache
        .lock()
        .await
        .get(&cache_key)
        .cloned()
    {
        return static_asset_response(cached);
    }

    let asset_path = state.config.static_dir.join(&relative_path);
    if let Some(asset) = load_static_asset(&state.config, &asset_path, route_path).await {
        state
            .static_asset_cache
            .lock()
            .await
            .insert(cache_key, asset.clone());
        return static_asset_response(asset);
    }

    if looks_like_static_asset(route_path) {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }

    let fallback_name = if route_path == "/" {
        "index.html"
    } else {
        "200.html"
    };
    let fallback_key = format!("__fallback__::{fallback_name}");
    if let Some(cached) = state
        .static_asset_cache
        .lock()
        .await
        .get(&fallback_key)
        .cloned()
    {
        return static_asset_response(cached);
    }

    let fallback_path = state.config.static_dir.join(fallback_name);
    if let Some(asset) = load_static_asset(&state.config, &fallback_path, route_path).await {
        state
            .static_asset_cache
            .lock()
            .await
            .insert(fallback_key, asset.clone());
        return static_asset_response(asset);
    }

    (StatusCode::NOT_FOUND, "Not found").into_response()
}

fn sanitize_static_relative_path(route_path: &str) -> Option<PathBuf> {
    let raw = route_path.trim_start_matches('/');
    if raw.is_empty() {
        return Some(PathBuf::from("index.html"));
    }

    let mut sanitized = PathBuf::new();
    for component in Path::new(raw).components() {
        match component {
            Component::Normal(value) => sanitized.push(value),
            Component::CurDir => {}
            Component::ParentDir | Component::Prefix(_) | Component::RootDir => return None,
        }
    }

    if sanitized.as_os_str().is_empty() {
        Some(PathBuf::from("index.html"))
    } else {
        Some(sanitized)
    }
}

fn looks_like_static_asset(route_path: &str) -> bool {
    Path::new(route_path)
        .file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.contains('.'))
}

async fn load_static_asset(
    config: &Config,
    asset_path: &Path,
    route_path: &str,
) -> Option<CachedStaticAsset> {
    let metadata = tokio_fs::metadata(asset_path).await.ok()?;
    if !metadata.is_file() {
        return None;
    }

    let content_type = static_content_type(asset_path);
    let cache_control = static_cache_control(route_path, asset_path);

    if static_asset_is_text(asset_path) {
        let text = tokio_fs::read_to_string(asset_path).await.ok()?;
        let replaced = text.replace(STATIC_BASE_PLACEHOLDER, &config.base_path);
        return Some(CachedStaticAsset {
            bytes: Bytes::from(replaced),
            content_type,
            cache_control,
        });
    }

    let bytes = tokio_fs::read(asset_path).await.ok()?;
    Some(CachedStaticAsset {
        bytes: Bytes::from(bytes),
        content_type,
        cache_control,
    })
}

fn static_asset_response(asset: CachedStaticAsset) -> Response {
    let mut response = Response::new(Body::from(asset.bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(asset.content_type),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(asset.cache_control),
    );
    response
}

fn static_asset_is_text(asset_path: &Path) -> bool {
    matches!(
        asset_path.extension().and_then(|value| value.to_str()),
        Some("html" | "js" | "mjs" | "css" | "json" | "map" | "svg" | "txt" | "webmanifest")
    )
}

fn static_content_type(asset_path: &Path) -> &'static str {
    match asset_path.extension().and_then(|value| value.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map" | "webmanifest") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("wasm") => "application/wasm",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn static_cache_control(route_path: &str, asset_path: &Path) -> &'static str {
    if route_path == "/"
        || matches!(
            asset_path.extension().and_then(|value| value.to_str()),
            Some("html")
        )
    {
        "no-cache"
    } else if route_path.starts_with("/_app/immutable/") {
        "public, max-age=31536000, immutable"
    } else {
        "public, max-age=3600"
    }
}

async fn websocket_session(socket: WebSocket, state: AppState, auth: AuthContext) {
    let (mut sender, mut receiver) = socket.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<ServerEnvelope>();
    let connection_id = Uuid::new_v4().to_string();
    let subscriptions: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let writer = tokio::spawn(async move {
        while let Some(message) = out_rx.recv().await {
            let text = match serde_json::to_string(&message) {
                Ok(text) => text,
                Err(error) => {
                    error!("failed to serialize websocket message: {error:#}");
                    continue;
                }
            };

            if sender.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    let _ = out_tx.send(ServerEnvelope::Ready {
        connection_id: connection_id.clone(),
    });

    while let Some(Ok(message)) = receiver.next().await {
        match message {
            Message::Text(text) => {
                let payload = match serde_json::from_str::<ClientEnvelope>(&text) {
                    Ok(payload) => payload,
                    Err(error) => {
                        let _ = out_tx.send(ServerEnvelope::Response {
                            id: Uuid::new_v4().to_string(),
                            ok: false,
                            result: None,
                            error: Some(format!("Invalid websocket payload: {error}")),
                        });
                        continue;
                    }
                };

                let state = state.clone();
                let out_tx = out_tx.clone();
                let subscriptions = Arc::clone(&subscriptions);
                let auth = auth.clone();
                tokio::spawn(async move {
                    ACTIVE_PROFILE_ID
                        .scope(auth.profile_id.clone(), async move {
                            if let Err(error) =
                                handle_ws_message(&state, &out_tx, &subscriptions, &auth, payload)
                                    .await
                            {
                                error!("websocket request failed: {error:#}");
                            }
                        })
                        .await;
                });
            }
            Message::Ping(payload) => {
                let _ = out_tx.send(ServerEnvelope::Pong {
                    nonce: Some(URL_SAFE_NO_PAD.encode(payload)),
                });
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    let mut handles = subscriptions.lock().await;
    for (_, handle) in handles.drain() {
        handle.abort();
    }
    writer.abort();
}

async fn handle_ws_message(
    state: &AppState,
    out_tx: &mpsc::UnboundedSender<ServerEnvelope>,
    subscriptions: &Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    auth: &AuthContext,
    payload: ClientEnvelope,
) -> Result<()> {
    match payload {
        ClientEnvelope::Ping { nonce } => {
            let _ = out_tx.send(ServerEnvelope::Pong { nonce });
        }
        ClientEnvelope::Request { id, method, params } => {
            let request_key = request_cache_key(&auth.profile_id, &id);

            if let Some(cached) = cached_response(state, &request_key).await {
                let _ = out_tx.send(cached);
                return Ok(());
            }

            if !register_inflight_request(state, &request_key, out_tx).await {
                return Ok(());
            }

            let audit_target = summarize_audit_target(&params);
            let message = match execute_ws_method(
                state,
                out_tx,
                subscriptions,
                auth,
                &method,
                params,
            )
            .await
            {
                Ok(result) => ServerEnvelope::Response {
                    id: id.clone(),
                    ok: true,
                    result: Some(result),
                    error: None,
                },
                Err(error) => ServerEnvelope::Response {
                    id: id.clone(),
                    ok: false,
                    result: None,
                    error: Some(error.to_string()),
                },
            };
            if should_audit_ws_method(&method) {
                let log_config = state.config.clone();
                let role = auth.role;
                let method_name = method.clone();
                let target = audit_target;
                let error = match &message {
                    ServerEnvelope::Response { error, .. } => error.clone(),
                    _ => None,
                };
                let ok = matches!(&message, ServerEnvelope::Response { ok: true, .. });
                tokio::spawn(async move {
                    let _ = append_audit_log(
                        &log_config,
                        AuditLogEntry {
                            id: Uuid::new_v4().to_string(),
                            at: now_unix_ms(),
                            role: match role {
                                UserRole::Admin => "admin".to_string(),
                                UserRole::Viewer => "viewer".to_string(),
                            },
                            method: method_name,
                            target,
                            ok,
                            error,
                        },
                    )
                    .await;
                });
            }

            cache_response(state, &request_key, message.clone()).await;
            resolve_inflight_request(state, &request_key, message).await;
        }
    }

    Ok(())
}

fn is_ws_method_allowed(role: UserRole, method: &str) -> bool {
    if role == UserRole::Admin {
        return true;
    }

    matches!(
        method,
        "config/get"
            | "runtime/status"
            | "runtime/checkUpdate"
            | "runtime/quota"
            | "catalog/get"
            | "directories/browse"
            | "editor/file/get"
            | "sessions/list"
            | "sessions/search"
            | "session/get"
            | "session/draft/get"
            | "session/queue/get"
            | "session/olderTurns/get"
            | "session/turn/get"
            | "session/itemDetail/get"
            | "notifications/list"
            | "account/get"
            | "arena/list"
            | "git/repositories/list"
            | "git/status"
            | "git/github/pulls"
            | "git/github/pull"
            | "git/commit/diff"
            | "git/file/get"
            | "git/file/resolve"
            | "git/worktrees/list"
            | "terminal/list"
            | "terminal/read"
            | "session/subscribe"
            | "session/unsubscribe"
            | "events/subscribe"
            | "events/unsubscribe"
            | "terminal/subscribe"
            | "terminal/unsubscribe"
            | "audit/list"
    )
}

fn should_audit_ws_method(method: &str) -> bool {
    !matches!(
        method,
        "config/get"
            | "runtime/status"
            | "runtime/checkUpdate"
            | "runtime/quota"
            | "catalog/get"
            | "directories/browse"
            | "editor/file/get"
            | "sessions/list"
            | "sessions/search"
            | "session/get"
            | "session/draft/get"
            | "session/queue/get"
            | "session/olderTurns/get"
            | "session/turn/get"
            | "session/itemDetail/get"
            | "notifications/list"
            | "account/get"
            | "arena/list"
            | "git/repositories/list"
            | "git/status"
            | "git/github/pulls"
            | "git/github/pull"
            | "git/commit/diff"
            | "git/file/get"
            | "git/file/resolve"
            | "git/worktrees/list"
            | "terminal/list"
            | "terminal/read"
            | "session/subscribe"
            | "session/unsubscribe"
            | "events/subscribe"
            | "events/unsubscribe"
            | "terminal/subscribe"
            | "terminal/unsubscribe"
    )
}

fn summarize_audit_target(params: &Value) -> Option<String> {
    for key in [
        "sessionId",
        "threadId",
        "terminalId",
        "queueId",
        "turnId",
        "presetId",
        "filterId",
        "repoPath",
        "filePath",
    ] {
        if let Some(value) = params
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            return Some(value.trim().to_string());
        }
    }
    None
}

async fn append_audit_log(config: &Config, entry: AuditLogEntry) -> Result<()> {
    tokio_fs::create_dir_all(&config.data_dir)
        .await
        .context("failed to create data directory")?;
    let path = config.data_dir.join("audit-log.jsonl");
    let mut file = tokio_fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .context("failed to open audit log")?;
    let line = serde_json::to_string(&entry).context("failed to serialize audit log entry")?;
    file.write_all(line.as_bytes())
        .await
        .context("failed to write audit log entry")?;
    file.write_all(b"\n")
        .await
        .context("failed to finalize audit log entry")?;
    Ok(())
}

async fn list_audit_log(config: &Config, limit: usize) -> Result<Value> {
    let path = config.data_dir.join("audit-log.jsonl");
    let raw = match tokio_fs::read_to_string(path).await {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error).context("failed to read audit log"),
    };

    let mut entries = raw
        .lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<AuditLogEntry>(line).ok())
        .take(limit.max(1))
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| right.at.cmp(&left.at));

    Ok(json!({ "entries": entries }))
}

async fn execute_ws_method(
    state: &AppState,
    out_tx: &mpsc::UnboundedSender<ServerEnvelope>,
    subscriptions: &Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    auth: &AuthContext,
    method: &str,
    params: Value,
) -> Result<Value> {
    if !is_ws_method_allowed(auth.role, method) {
        return Err(anyhow!(
            "{{\"code\":\"FORBIDDEN_ROLE\",\"message\":\"This action requires an admin role.\"}}"
        ));
    }

    fn append_session_filter_query(path: &mut String, params: &Value) {
        let Some(filter) = params.get("filter") else {
            return;
        };
        if filter
            .get("pinnedOnly")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            path.push_str("&filterPinned=true");
        }
        if filter
            .get("runningOnly")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            path.push_str("&filterRunning=true");
        }
        if filter
            .get("queuedOnly")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            path.push_str("&filterQueued=true");
        }
        let highlight = filter
            .get("highlight")
            .and_then(Value::as_str)
            .filter(|value| *value == "attention" || *value == "completed");
        if let Some(highlight) = highlight {
            path.push_str("&filterHighlight=");
            path.push_str(highlight);
        }
        if let Some(tags) = filter.get("tags").and_then(Value::as_array) {
            for tag in tags
                .iter()
                .filter_map(Value::as_str)
                .filter(|value| !value.trim().is_empty())
            {
                path.push_str("&filterTag=");
                path.push_str(&urlencoding::encode(tag));
            }
        }
    }

    match method {
        "config/get" => internal_json_request(state, Method::GET, "/api/config", None).await,
        "config/update" => {
            let payload = json!({
                "autostart": params.get("autostart").cloned().unwrap_or_else(|| json!({})),
                "systemShutdown": params.get("systemShutdown").cloned().unwrap_or_else(|| json!({})),
                "theme": params.get("theme").cloned().unwrap_or(Value::Null)
            });
            internal_json_request(state, Method::PATCH, "/api/config", Some(payload)).await
        }
        "notifications/list" => {
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(80);
            internal_json_request(
                state,
                Method::GET,
                &format!("/api/notifications?limit={limit}"),
                None,
            )
            .await
        }
        "audit/list" => {
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(120) as usize;
            list_audit_log(&state.config, limit).await
        }
        "notifications/markRead" => {
            let payload = json!({
                "ids": params.get("ids").cloned().unwrap_or(Value::Null)
            });
            internal_json_request(state, Method::PATCH, "/api/notifications", Some(payload)).await
        }
        "notifications/clear" => {
            internal_json_request(state, Method::DELETE, "/api/notifications", None).await
        }
        "notifications/settings/update" => {
            let payload = json!({
                "enabledEventTypes": params.get("enabledEventTypes").cloned().unwrap_or(Value::Null),
                "slackWebhookUrl": params.get("slackWebhookUrl").cloned().unwrap_or(Value::Null),
                "webhookUrl": params.get("webhookUrl").cloned().unwrap_or(Value::Null)
            });
            internal_json_request(
                state,
                Method::PATCH,
                "/api/notifications/settings",
                Some(payload),
            )
            .await
        }
        "automations/save" => {
            let payload = json!({
                "automation": params.get("automation").cloned().unwrap_or(Value::Null)
            });
            internal_json_request(state, Method::POST, "/api/automations", Some(payload)).await
        }
        "automations/delete" => {
            let automation_id = require_string(&params, "automationId")?;
            internal_json_request(
                state,
                Method::DELETE,
                &format!("/api/automations?automationId={automation_id}"),
                None,
            )
            .await
        }
        "automations/run" => {
            let automation_id = require_string(&params, "automationId")?;
            let payload = json!({
                "trigger": params.get("trigger").cloned().unwrap_or_else(|| json!("manual"))
            });
            internal_json_request(
                state,
                Method::POST,
                &format!("/api/automations/{automation_id}/run"),
                Some(payload),
            )
            .await
        }
        "runtime/status" => codex_runtime_status(state, false).await,
        "runtime/checkUpdate" => codex_runtime_status(state, true).await,
        "runtime/quota" => {
            codex_quota_status(
                state,
                params
                    .get("refresh")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                &auth.profile_id,
            )
            .await
        }
        "catalog/get" => internal_json_request(state, Method::GET, "/api/catalog", None).await,
        "editor/file/get" => {
            let file_path = require_string(&params, "filePath")?;
            read_editable_file_payload(state, &auth.profile_id, &file_path)
                .await
                .map_err(anyhow::Error::from)
        }
        "editor/file/save" => {
            let file_path = require_string(&params, "filePath")?;
            let content = params
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            write_editable_file_payload(state, &auth.profile_id, &file_path, &content)
                .await
                .map_err(anyhow::Error::from)
        }
        "runtime/install" => install_or_update_codex(state, true).await,
        "runtime/update" => install_or_update_codex(state, false).await,
        "sessions/list" => {
            let archived = params
                .get("archived")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let cursor = params
                .get("cursor")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(urlencoding::encode);
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(20);
            let mut path = format!("/api/sessions?archived={archived}&limit={limit}");
            if let Some(cursor) = cursor {
                path.push_str(&format!("&cursor={cursor}"));
            }
            append_session_filter_query(&mut path, &params);
            internal_json_request(state, Method::GET, &path, None).await
        }
        "sessions/search" => {
            let archived = params
                .get("archived")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let query_raw = require_string(&params, "query")?;
            let query = urlencoding::encode(&query_raw);
            let scope = if params.get("scope").and_then(Value::as_str) == Some("full") {
                "full"
            } else {
                "summary"
            };
            let cursor = params
                .get("cursor")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(urlencoding::encode);
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(20);
            let mut path = format!(
                "/api/sessions?query={query}&scope={scope}&archived={archived}&limit={limit}"
            );
            if let Some(cursor) = cursor {
                path.push_str(&format!("&cursor={cursor}"));
            }
            append_session_filter_query(&mut path, &params);
            internal_json_request(state, Method::GET, &path, None).await
        }
        "session/create" => {
            let payload = json!({
                "preferences": params.get("preferences").cloned().unwrap_or_else(|| json!({})),
                "name": params.get("name").cloned().unwrap_or(Value::Null),
            });
            internal_json_request(state, Method::POST, "/api/sessions", Some(payload)).await
        }
        "session/organization/update" => {
            let session_id = require_string(&params, "sessionId")?;
            let payload = json!({
                "pinned": params.get("pinned").cloned().unwrap_or(Value::Null),
                "tags": params.get("tags").cloned().unwrap_or(Value::Null)
            });
            internal_json_request(
                state,
                Method::PATCH,
                &format!("/api/sessions/{session_id}/organization"),
                Some(payload),
            )
            .await
        }
        "sessionFilters/save" => {
            let payload = json!({
                "filter": params.get("filter").cloned().unwrap_or(Value::Null)
            });
            internal_json_request(state, Method::POST, "/api/session-filters", Some(payload)).await
        }
        "sessionFilters/delete" => {
            let filter_id = require_string(&params, "filterId")?;
            let filter_id = urlencoding::encode(&filter_id);
            internal_json_request(
                state,
                Method::DELETE,
                &format!("/api/session-filters?filterId={filter_id}"),
                None,
            )
            .await
        }
        "promptPresets/save" => {
            let payload = json!({
                "preset": params.get("preset").cloned().unwrap_or(Value::Null)
            });
            internal_json_request(state, Method::POST, "/api/prompt-presets", Some(payload)).await
        }
        "promptPresets/delete" => {
            let preset_id = require_string(&params, "presetId")?;
            let preset_id = urlencoding::encode(&preset_id);
            internal_json_request(
                state,
                Method::DELETE,
                &format!("/api/prompt-presets?presetId={preset_id}"),
                None,
            )
            .await
        }
        "session/get" => {
            let session_id = require_string(&params, "sessionId")?;
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(20);
            internal_json_request(
                state,
                Method::GET,
                &format!("/api/sessions/{session_id}?limit={limit}"),
                None,
            )
            .await
        }
        "session/fork" => {
            let session_id = require_string(&params, "sessionId")?;
            let payload = json!({
                "mode": params.get("mode").cloned().unwrap_or_else(|| Value::String("fork".to_string())),
                "turnId": params.get("turnId").cloned().unwrap_or(Value::Null),
                "messageText": params.get("messageText").cloned().unwrap_or(Value::Null)
            });
            internal_json_request(
                state,
                Method::POST,
                &format!("/api/sessions/{session_id}/fork"),
                Some(payload),
            )
            .await
        }
        "session/search" => {
            let session_id = require_string(&params, "sessionId")?;
            let query_raw = require_string(&params, "query")?;
            let query = urlencoding::encode(&query_raw);
            let cursor = params
                .get("cursor")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(urlencoding::encode);
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(20);
            let mut path = format!("/api/sessions/{session_id}/search?query={query}&limit={limit}");
            if let Some(cursor) = cursor {
                path.push_str(&format!("&cursor={cursor}"));
            }
            internal_json_request(state, Method::GET, &path, None).await
        }
        "session/olderTurns/get" => {
            let session_id = require_string(&params, "sessionId")?;
            let before_turn_id = require_string(&params, "beforeTurnId")?;
            let before_turn_id = urlencoding::encode(&before_turn_id);
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(20);
            internal_json_request(
                state,
                Method::GET,
                &format!(
                    "/api/sessions/{session_id}/turns?beforeTurnId={before_turn_id}&limit={limit}"
                ),
                None,
            )
            .await
        }
        "session/turn/get" => {
            let session_id = require_string(&params, "sessionId")?;
            let turn_id = require_string(&params, "turnId")?;
            internal_json_request(
                state,
                Method::GET,
                &format!("/api/sessions/{session_id}/turns/{turn_id}"),
                None,
            )
            .await
        }
        "session/itemDetail/get" => {
            let session_id = require_string(&params, "sessionId")?;
            let turn_id = require_string(&params, "turnId")?;
            let item_id = require_string(&params, "itemId")?;
            internal_json_request(
                state,
                Method::GET,
                &format!("/api/sessions/{session_id}/turns/{turn_id}/items/{item_id}"),
                None,
            )
            .await
        }
        "session/draft/get" => {
            let session_id = require_string(&params, "sessionId")?;
            internal_json_request(
                state,
                Method::GET,
                &format!("/api/sessions/{session_id}/draft"),
                None,
            )
            .await
        }
        "session/draft/save" => {
            let session_id = require_string(&params, "sessionId")?;
            let payload = json!({
                "draft": params.get("draft").cloned().unwrap_or_else(|| Value::String(String::new())),
                "intent": params.get("intent").cloned().unwrap_or_else(|| Value::String("message".to_string()))
            });
            internal_json_request(
                state,
                Method::PATCH,
                &format!("/api/sessions/{session_id}/draft"),
                Some(payload),
            )
            .await
        }
        "session/draft/clear" => {
            let session_id = require_string(&params, "sessionId")?;
            internal_json_request(
                state,
                Method::DELETE,
                &format!("/api/sessions/{session_id}/draft"),
                None,
            )
            .await
        }
        "session/queue/get" => {
            let session_id = require_string(&params, "sessionId")?;
            internal_json_request(
                state,
                Method::GET,
                &format!("/api/sessions/{session_id}/queue"),
                None,
            )
            .await
        }
        "session/queue/enqueue" => {
            let session_id = require_string(&params, "sessionId")?;
            let payload = json!({
                "prompt": params.get("prompt").cloned().unwrap_or_else(|| Value::String(String::new())),
                "attachmentIds": params.get("attachmentIds").cloned().unwrap_or_else(|| json!([]))
            });
            internal_json_request(
                state,
                Method::POST,
                &format!("/api/sessions/{session_id}/queue"),
                Some(payload),
            )
            .await
        }
        "session/queue/resume" => {
            let session_id = require_string(&params, "sessionId")?;
            internal_json_request(
                state,
                Method::POST,
                &format!("/api/sessions/{session_id}/queue/resume"),
                Some(json!({})),
            )
            .await
        }
        "session/queue/remove" => {
            let session_id = require_string(&params, "sessionId")?;
            let queue_id = require_string(&params, "queueId")?;
            internal_json_request(
                state,
                Method::DELETE,
                &format!("/api/sessions/{session_id}/queue/{queue_id}"),
                None,
            )
            .await
        }
        "session/queue/update" => {
            let session_id = require_string(&params, "sessionId")?;
            let queue_id = require_string(&params, "queueId")?;
            let payload = json!({
                "prompt": params.get("prompt").cloned().unwrap_or_else(|| Value::String(String::new())),
                "attachmentIds": params.get("attachmentIds").cloned().unwrap_or_else(|| json!([]))
            });
            internal_json_request(
                state,
                Method::PATCH,
                &format!("/api/sessions/{session_id}/queue/{queue_id}"),
                Some(payload),
            )
            .await
        }
        "session/queue/reorder" => {
            let session_id = require_string(&params, "sessionId")?;
            let payload = json!({
                "queueIds": params.get("queueIds").cloned().unwrap_or_else(|| json!([]))
            });
            internal_json_request(
                state,
                Method::POST,
                &format!("/api/sessions/{session_id}/queue/reorder"),
                Some(payload),
            )
            .await
        }
        "session/queue/dispatch" => {
            let session_id = require_string(&params, "sessionId")?;
            let queue_id = require_string(&params, "queueId")?;
            let payload = json!({
                "mode": require_string(&params, "mode")?
            });
            internal_json_request(
                state,
                Method::POST,
                &format!("/api/sessions/{session_id}/queue/{queue_id}"),
                Some(payload),
            )
            .await
        }
        "session/savePreferences" => {
            let session_id = require_string(&params, "sessionId")?;
            let payload = json!({
                "preferences": params.get("preferences").cloned().unwrap_or_else(|| json!({}))
            });
            internal_json_request(
                state,
                Method::PATCH,
                &format!("/api/sessions/{session_id}"),
                Some(payload),
            )
            .await
        }
        "session/rename" => {
            let session_id = require_string(&params, "sessionId")?;
            let payload = json!({ "name": require_string(&params, "name")? });
            internal_json_request(
                state,
                Method::POST,
                &format!("/api/sessions/{session_id}/name"),
                Some(payload),
            )
            .await
        }
        "session/archive" => {
            let session_id = require_string(&params, "sessionId")?;
            internal_json_request(
                state,
                Method::POST,
                &format!("/api/sessions/{session_id}/archive"),
                Some(json!({})),
            )
            .await
        }
        "session/unarchive" => {
            let session_id = require_string(&params, "sessionId")?;
            internal_json_request(
                state,
                Method::POST,
                &format!("/api/sessions/{session_id}/unarchive"),
                Some(json!({})),
            )
            .await
        }
        "turn/send" => {
            let session_id = require_string(&params, "sessionId")?;
            let payload = json!({
                "prompt": params.get("prompt").cloned().unwrap_or_else(|| Value::String(String::new())),
                "attachmentIds": params.get("attachmentIds").cloned().unwrap_or_else(|| json!([])),
                "preferences": params.get("preferences").cloned().unwrap_or_else(|| json!({}))
            });
            internal_json_request(
                state,
                Method::POST,
                &format!("/api/sessions/{session_id}/messages"),
                Some(payload),
            )
            .await
        }
        "turn/steer" => {
            let session_id = require_string(&params, "sessionId")?;
            let payload = json!({
                "prompt": require_string(&params, "prompt")?,
                "attachmentIds": params.get("attachmentIds").cloned().unwrap_or_else(|| json!([]))
            });
            internal_json_request(
                state,
                Method::POST,
                &format!("/api/sessions/{session_id}/steer"),
                Some(payload),
            )
            .await
        }
        "turn/abort" => {
            let session_id = require_string(&params, "sessionId")?;
            internal_json_request(
                state,
                Method::POST,
                &format!("/api/sessions/{session_id}/abort"),
                Some(json!({})),
            )
            .await
        }
        "approval/resolve" => {
            let session_id = require_string(&params, "sessionId")?;
            let payload = json!({
                "requestId": require_string(&params, "requestId")?,
                "result": params.get("result").cloned().unwrap_or(Value::Null)
            });
            internal_json_request(
                state,
                Method::POST,
                &format!("/api/sessions/{session_id}/approval"),
                Some(payload),
            )
            .await
        }
        "directories/browse" => {
            let current_path = params.get("currentPath").and_then(Value::as_str);
            list_directories_payload(state, current_path)
                .await
                .map_err(anyhow::Error::from)
        }
        "attachments/upload" => {
            let session_id = require_string(&params, "sessionId")?;
            let files = params
                .get("files")
                .cloned()
                .ok_or_else(|| anyhow!("files is required"))?;
            let files: Vec<UploadFilePayload> = serde_json::from_value(files)?;
            upload_attachments(state, &session_id, files).await
        }
        "attachments/delete" => {
            let session_id = require_string(&params, "sessionId")?;
            let attachment_id = require_string(&params, "attachmentId")?;
            internal_json_request(
                state,
                Method::DELETE,
                &format!("/api/sessions/{session_id}/attachments/{attachment_id}"),
                None,
            )
            .await
        }
        "account/get" => get_account_state(state, &auth.profile_id).await,
        "account/login/start" => start_account_login(state, &auth.profile_id, &params).await,
        "account/login/cancel" => cancel_account_login(state, &auth.profile_id, &params).await,
        "account/logout" => logout_account(state, &auth.profile_id).await,
        "arena/list" => internal_json_request(state, Method::GET, "/api/arena", None).await,
        "arena/start" => {
            let payload = json!({
                "prompt": require_string(&params, "prompt")?,
                "contestants": params.get("contestants").cloned().unwrap_or_else(|| json!([])),
                "preferences": params.get("preferences").cloned().unwrap_or_else(|| json!({}))
            });
            internal_json_request(state, Method::POST, "/api/arena", Some(payload)).await
        }
        "git/repositories/list" => {
            internal_json_request(state, Method::GET, "/api/git/repositories", None).await
        }
        "git/status" => {
            let repo_path_raw = require_string(&params, "repoPath")?;
            let repo_path = urlencoding::encode(&repo_path_raw);
            internal_json_request(
                state,
                Method::GET,
                &format!("/api/git/status?repoPath={repo_path}"),
                None,
            )
            .await
        }
        "git/github/pulls" => {
            let repo_path_raw = require_string(&params, "repoPath")?;
            let repo_path = urlencoding::encode(&repo_path_raw);
            let pr_state = params
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or("open");
            let pr_state = urlencoding::encode(pr_state);
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(20);
            internal_json_request(
                state,
                Method::GET,
                &format!(
                    "/api/git/github/pulls?repoPath={repo_path}&state={pr_state}&limit={limit}"
                ),
                None,
            )
            .await
        }
        "git/github/pull" => {
            let repo_path_raw = require_string(&params, "repoPath")?;
            let repo_path = urlencoding::encode(&repo_path_raw);
            let number = params
                .get("number")
                .and_then(Value::as_u64)
                .ok_or_else(|| anyhow!("number is required"))?;
            internal_json_request(
                state,
                Method::GET,
                &format!("/api/git/github/pulls/{number}?repoPath={repo_path}"),
                None,
            )
            .await
        }
        "git/github/pull/checkout" => {
            let payload = json!({
                "repoPath": require_string(&params, "repoPath")?
            });
            let number = params
                .get("number")
                .and_then(Value::as_u64)
                .ok_or_else(|| anyhow!("number is required"))?;
            internal_json_request(
                state,
                Method::POST,
                &format!("/api/git/github/pulls/{number}/checkout"),
                Some(payload),
            )
            .await
        }
        "git/worktrees/list" => {
            let repo_path_raw = require_string(&params, "repoPath")?;
            let repo_path = urlencoding::encode(&repo_path_raw);
            internal_json_request(
                state,
                Method::GET,
                &format!("/api/git/worktrees?repoPath={repo_path}"),
                None,
            )
            .await
        }
        "git/worktrees/create" => {
            let payload = json!({
                "repoPath": require_string(&params, "repoPath")?,
                "worktreePath": require_string(&params, "worktreePath")?,
                "branchName": params.get("branchName").cloned().unwrap_or(Value::Null),
                "createBranch": params.get("createBranch").cloned().unwrap_or_else(|| Value::Bool(false)),
                "detach": params.get("detach").cloned().unwrap_or_else(|| Value::Bool(false))
            });
            internal_json_request(state, Method::POST, "/api/git/worktrees", Some(payload)).await
        }
        "git/worktrees/remove" => {
            let payload = json!({
                "repoPath": require_string(&params, "repoPath")?,
                "worktreePath": require_string(&params, "worktreePath")?,
                "force": params.get("force").cloned().unwrap_or_else(|| Value::Bool(false))
            });
            internal_json_request(state, Method::DELETE, "/api/git/worktrees", Some(payload)).await
        }
        "git/file/get" => {
            let repo_path_raw = require_string(&params, "repoPath")?;
            let file_path_raw = require_string(&params, "filePath")?;
            let repo_path = urlencoding::encode(&repo_path_raw);
            let file_path = urlencoding::encode(&file_path_raw);
            internal_json_request(
                state,
                Method::GET,
                &format!("/api/git/file?repoPath={repo_path}&filePath={file_path}"),
                None,
            )
            .await
        }
        "git/file/resolve" => {
            let file_path_raw = require_string(&params, "filePath")?;
            let file_path = urlencoding::encode(&file_path_raw);
            internal_json_request(
                state,
                Method::GET,
                &format!("/api/git/file/resolve?filePath={file_path}"),
                None,
            )
            .await
        }
        "git/file/save" => {
            let payload = json!({
                "repoPath": require_string(&params, "repoPath")?,
                "filePath": require_string(&params, "filePath")?,
                "content": params.get("content").cloned().unwrap_or_else(|| Value::String(String::new()))
            });
            internal_json_request(state, Method::PUT, "/api/git/file", Some(payload)).await
        }
        "git/stage" => {
            let payload = json!({
                "repoPath": require_string(&params, "repoPath")?,
                "filePath": params.get("filePath").cloned().unwrap_or(Value::Null)
            });
            internal_json_request(state, Method::POST, "/api/git/stage", Some(payload)).await
        }
        "git/unstage" => {
            let payload = json!({
                "repoPath": require_string(&params, "repoPath")?,
                "filePath": params.get("filePath").cloned().unwrap_or(Value::Null)
            });
            internal_json_request(state, Method::POST, "/api/git/unstage", Some(payload)).await
        }
        "git/fetch" => {
            let payload = json!({
                "repoPath": require_string(&params, "repoPath")?
            });
            internal_json_request(state, Method::POST, "/api/git/fetch", Some(payload)).await
        }
        "git/pull" => {
            let payload = json!({
                "repoPath": require_string(&params, "repoPath")?
            });
            internal_json_request(state, Method::POST, "/api/git/pull", Some(payload)).await
        }
        "git/commit" => {
            let payload = json!({
                "repoPath": require_string(&params, "repoPath")?,
                "message": require_string(&params, "message")?
            });
            internal_json_request(state, Method::POST, "/api/git/commit", Some(payload)).await
        }
        "git/commit/diff" => {
            let repo_path_raw = require_string(&params, "repoPath")?;
            let commit_hash_raw = require_string(&params, "commitHash")?;
            let repo_path = urlencoding::encode(&repo_path_raw);
            let commit_hash = urlencoding::encode(&commit_hash_raw);
            internal_json_request(
                state,
                Method::GET,
                &format!("/api/git/commit/diff?repoPath={repo_path}&commitHash={commit_hash}"),
                None,
            )
            .await
        }
        "git/checkout" => {
            let payload = json!({
                "repoPath": require_string(&params, "repoPath")?,
                "branchName": require_string(&params, "branchName")?,
                "create": params.get("create").cloned().unwrap_or_else(|| Value::Bool(false))
            });
            internal_json_request(state, Method::POST, "/api/git/checkout", Some(payload)).await
        }
        "terminal/list" => list_terminals(state).await,
        "terminal/create" => {
            let cwd = params
                .get("cwd")
                .and_then(Value::as_str)
                .map(str::to_string);
            let title = params
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_string);
            create_terminal(state.clone(), cwd, title).await
        }
        "terminal/read" => {
            let terminal_id = require_string(&params, "terminalId")?;
            read_terminal(state, &terminal_id).await
        }
        "terminal/context/attach" => {
            let session_id = require_string(&params, "sessionId")?;
            let terminal_id = require_string(&params, "terminalId")?;
            let max_bytes = params
                .get("maxBytes")
                .and_then(Value::as_u64)
                .map(|value| value.clamp(2_048, 128_000) as usize)
                .unwrap_or(24_000);
            let session = get_terminal_session(state, &terminal_id).await?;
            let (summary, snapshot) = session.snapshot().await;

            let mut cleaned = String::with_capacity(snapshot.len());
            let mut chars = snapshot.chars().peekable();
            while let Some(ch) = chars.next() {
                if ch == '\u{1b}' {
                    if matches!(
                        chars.peek(),
                        Some('[' | ']' | '(' | ')' | '#' | 'P' | '_' | '^')
                    ) {
                        while let Some(next) = chars.next() {
                            if ('@'..='~').contains(&next) {
                                break;
                            }
                        }
                    }
                    continue;
                }

                if ch != '\r' {
                    cleaned.push(ch);
                }
            }

            let trimmed = cleaned.trim();
            if trimmed.is_empty() {
                anyhow::bail!("terminal has no output to attach yet.");
            }

            let excerpt = if trimmed.len() > max_bytes {
                let start = trimmed
                    .char_indices()
                    .nth(trimmed.chars().count().saturating_sub(max_bytes))
                    .map(|(index, _)| index)
                    .unwrap_or(0);
                trimmed[start..].to_string()
            } else {
                trimmed.to_string()
            };

            let terminal_slug = {
                let value = sanitize_profile_id(&summary.title);
                if value.is_empty() {
                    sanitize_profile_id(&terminal_id)
                } else {
                    value
                }
            };
            let captured_at = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let content = format!(
                "# Terminal context\n\nTerminal: {}\nWorking directory: {}\nStatus: {}{}\nCaptured at: {}\n\n```text\n{}\n```\n",
                summary.title,
                summary.cwd,
                summary.status,
                summary
                    .exit_code
                    .map(|exit_code| format!(" (exit {})", exit_code))
                    .unwrap_or_default(),
                captured_at,
                excerpt
            );
            let upload = UploadFilePayload {
                name: format!("terminal-{}-{}.md", terminal_slug, captured_at),
                mime_type: Some("text/markdown".to_string()),
                data_base64: base64::engine::general_purpose::STANDARD.encode(content.as_bytes()),
            };
            let uploaded = upload_attachments(state, &session_id, vec![upload]).await?;
            Ok(json!({
                "terminal": summary,
                "attachments": uploaded.get("attachments").cloned().unwrap_or_else(|| json!([])),
                "excerpt": excerpt
            }))
        }
        "terminal/input" => {
            let terminal_id = require_string(&params, "terminalId")?;
            let data = require_string(&params, "data")?;
            write_terminal_input(state, &terminal_id, &data).await
        }
        "terminal/close" => {
            let terminal_id = require_string(&params, "terminalId")?;
            close_terminal(state.clone(), &terminal_id).await
        }
        "session/subscribe" => {
            let session_id = require_string(&params, "sessionId")?;
            subscribe_session(
                state.clone(),
                out_tx.clone(),
                subscriptions.clone(),
                auth.profile_id.clone(),
                session_id.clone(),
            )
            .await?;
            Ok(json!({ "subscribed": true, "sessionId": session_id }))
        }
        "session/unsubscribe" => {
            let session_id = require_string(&params, "sessionId")?;
            let mut current = subscriptions.lock().await;
            if let Some(handle) = current.remove(&session_relay_key(&auth.profile_id, &session_id))
            {
                handle.abort();
            }
            Ok(json!({ "subscribed": false, "sessionId": session_id }))
        }
        "terminal/subscribe" => {
            let terminal_id = require_string(&params, "terminalId")?;
            subscribe_terminal(
                state.clone(),
                out_tx.clone(),
                subscriptions.clone(),
                terminal_id.clone(),
            )
            .await?;
            Ok(json!({ "subscribed": true, "terminalId": terminal_id }))
        }
        "terminal/unsubscribe" => {
            let terminal_id = require_string(&params, "terminalId")?;
            let mut current = subscriptions.lock().await;
            if let Some(handle) = current.remove(&format!("{TERMINAL_RELAY_PREFIX}{terminal_id}")) {
                handle.abort();
            }
            Ok(json!({ "subscribed": false, "terminalId": terminal_id }))
        }
        "events/subscribe" => {
            subscribe_global(
                state.clone(),
                out_tx.clone(),
                subscriptions.clone(),
                auth.profile_id.clone(),
            )
            .await?;
            Ok(json!({ "subscribed": true, "scope": "global" }))
        }
        "events/unsubscribe" => {
            let mut current = subscriptions.lock().await;
            if let Some(handle) = current.remove(&global_relay_key(&auth.profile_id)) {
                handle.abort();
            }
            Ok(json!({ "subscribed": false, "scope": "global" }))
        }
        _ => Err(anyhow!("Unknown websocket method: {method}")),
    }
}

async fn subscribe_session(
    state: AppState,
    out_tx: mpsc::UnboundedSender<ServerEnvelope>,
    subscriptions: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    profile_id: String,
    session_id: String,
) -> Result<()> {
    let relay = ensure_stream_relay(&state, &profile_id, &session_id).await?;
    let mut receiver = relay.subscribe();
    let session_key = session_id.clone();
    let handle = tokio::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    let _ = out_tx.send(ServerEnvelope::Event {
                        session_id: session_key.clone(),
                        event,
                    });
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!("websocket lagged on session {session_key}: skipped {skipped} messages");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let mut current = subscriptions.lock().await;
    if let Some(existing) = current.insert(session_relay_key(&profile_id, &session_id), handle) {
        existing.abort();
    }
    Ok(())
}

async fn subscribe_terminal(
    state: AppState,
    out_tx: mpsc::UnboundedSender<ServerEnvelope>,
    subscriptions: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    terminal_id: String,
) -> Result<()> {
    let terminal = get_terminal_session(&state, &terminal_id).await?;
    let mut receiver = terminal.relay.subscribe();
    let relay_key = format!("{TERMINAL_RELAY_PREFIX}{terminal_id}");
    let terminal_key = terminal_id.clone();
    let handle = tokio::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    let _ = out_tx.send(ServerEnvelope::TerminalEvent {
                        terminal_id: terminal_key.clone(),
                        event,
                    });
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(
                        "websocket lagged on terminal {terminal_key}: skipped {skipped} messages"
                    );
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let mut current = subscriptions.lock().await;
    if let Some(existing) = current.insert(relay_key, handle) {
        existing.abort();
    }
    Ok(())
}

async fn subscribe_global(
    state: AppState,
    out_tx: mpsc::UnboundedSender<ServerEnvelope>,
    subscriptions: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    profile_id: String,
) -> Result<()> {
    let relay = ensure_global_relay(&state, &profile_id).await?;
    let mut receiver = relay.subscribe();
    let handle = tokio::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    let _ = out_tx.send(ServerEnvelope::GlobalEvent { event });
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!("websocket lagged on global relay: skipped {skipped} messages");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let mut current = subscriptions.lock().await;
    if let Some(existing) = current.insert(global_relay_key(&profile_id), handle) {
        existing.abort();
    }
    Ok(())
}

async fn ensure_stream_relay(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> Result<broadcast::Sender<Value>> {
    let relay_key = session_relay_key(profile_id, session_id);
    let mut relays = state.relays.lock().await;
    if let Some(existing) = relays.get(&relay_key) {
        return Ok(existing.clone());
    }

    let (sender, _) = broadcast::channel(256);
    relays.insert(relay_key, sender.clone());

    let state = state.clone();
    let session_id = session_id.to_string();
    let profile_id = profile_id.to_string();
    let relay_sender = sender.clone();
    tokio::spawn(async move {
        loop {
            if let Err(error) = stream_session_events(
                state.clone(),
                relay_sender.clone(),
                profile_id.clone(),
                session_id.clone(),
            )
            .await
            {
                warn!("session stream relay failed for {session_id}: {error:#}");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    });

    Ok(sender)
}

async fn ensure_global_relay(
    state: &AppState,
    profile_id: &str,
) -> Result<broadcast::Sender<Value>> {
    let relay_key = global_relay_key(profile_id);
    let mut relays = state.relays.lock().await;
    if let Some(existing) = relays.get(&relay_key) {
        return Ok(existing.clone());
    }

    let (sender, _) = broadcast::channel(256);
    relays.insert(relay_key, sender.clone());

    let state = state.clone();
    let profile_id = profile_id.to_string();
    let relay_sender = sender.clone();
    tokio::spawn(bridge_app_server_global_notifications(
        state.clone(),
        relay_sender.clone(),
        profile_id.clone(),
    ));
    tokio::spawn(async move {
        loop {
            if let Err(error) =
                stream_global_events(state.clone(), relay_sender.clone(), profile_id.clone()).await
            {
                warn!("global stream relay failed: {error:#}");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    });

    Ok(sender)
}

async fn bridge_app_server_global_notifications(
    state: AppState,
    sender: broadcast::Sender<Value>,
    profile_id: String,
) {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, &profile_id)
        .0
        .to_string();
    let client = match app_server_client(&state, &profile_id).await {
        Ok(client) => client,
        Err(error) => {
            warn!("failed to create app-server bridge for {profile_id}: {error:#}");
            return;
        }
    };
    let mut notifications = client.subscribe_notifications();

    loop {
        match notifications.recv().await {
            Ok(notification) => {
                if matches!(
                    notification.method.as_str(),
                    "account/updated" | "account/rateLimits/updated"
                ) {
                    state.quota_cache.lock().await.remove(&resolved_profile_id);
                }

                if let Some(event) = map_app_server_global_notification(&notification) {
                    let _ = sender.send(event);
                }
            }
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                warn!(
                    "global app-server relay lagged for {profile_id}: skipped {skipped} messages"
                );
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

async fn stream_session_events(
    state: AppState,
    sender: broadcast::Sender<Value>,
    profile_id: String,
    session_id: String,
) -> Result<()> {
    let target = internal_url(&state.config, &format!("/api/sessions/{session_id}/stream"));
    let response = state
        .http
        .get(target)
        .header(INTERNAL_HEADER, &state.config.internal_proxy_token)
        .header(PROFILE_HEADER, &profile_id)
        .send()
        .await
        .context("failed to connect to internal SSE stream")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("internal SSE request failed with {status}: {body}"));
    }

    let stream = response
        .bytes_stream()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::Other, error));
    let reader = StreamReader::new(stream);
    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = lines.next_line().await.context("failed to read SSE line")? {
        if let Some(data) = line.strip_prefix("data: ") {
            let payload: Value = serde_json::from_str(data).context("invalid SSE json payload")?;
            let _ = sender.send(payload);
        }
    }

    Err(anyhow!("internal SSE stream ended"))
}

async fn stream_global_events(
    state: AppState,
    sender: broadcast::Sender<Value>,
    profile_id: String,
) -> Result<()> {
    let target = internal_url(&state.config, "/api/events/stream");
    let response = state
        .http
        .get(target)
        .header(INTERNAL_HEADER, &state.config.internal_proxy_token)
        .header(PROFILE_HEADER, &profile_id)
        .send()
        .await
        .context("failed to connect to internal global SSE stream")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!(
            "internal global SSE request failed with {status}: {body}"
        ));
    }

    let stream = response
        .bytes_stream()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::Other, error));
    let reader = StreamReader::new(stream);
    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = lines
        .next_line()
        .await
        .context("failed to read global SSE line")?
    {
        if let Some(data) = line.strip_prefix("data: ") {
            let payload: Value =
                serde_json::from_str(data).context("invalid global SSE json payload")?;
            let _ = sender.send(payload);
        }
    }

    Err(anyhow!("internal global SSE stream ended"))
}

async fn list_terminal_summaries(state: &AppState) -> Vec<TerminalSummaryState> {
    let terminals = {
        let current = state.terminals.lock().await;
        current.values().cloned().collect::<Vec<_>>()
    };

    let mut summaries = Vec::with_capacity(terminals.len());
    for terminal in terminals {
        summaries.push(terminal.summary().await);
    }
    summaries.sort_by(|left, right| right.last_activity_at.cmp(&left.last_activity_at));
    summaries
}

async fn emit_global_notification(state: &AppState, event: Value) {
    let relay = {
        let relays = state.relays.lock().await;
        relays.get(GLOBAL_RELAY_KEY).cloned()
    };

    if let Some(relay) = relay {
        let _ = relay.send(event);
    }
}

async fn emit_terminals_updated(state: &AppState) {
    emit_global_notification(
        state,
        json!({
            "kind": "notification",
            "method": "codex-webui/terminalsUpdated",
            "params": {
                "terminals": list_terminal_summaries(state).await
            }
        }),
    )
    .await;
}

async fn get_terminal_session(state: &AppState, terminal_id: &str) -> Result<Arc<TerminalSession>> {
    state
        .terminals
        .lock()
        .await
        .get(terminal_id)
        .cloned()
        .ok_or_else(|| anyhow!("Terminal not found."))
}

async fn validate_terminal_cwd(state: &AppState, requested_cwd: Option<String>) -> Result<String> {
    let candidate = requested_cwd
        .map(PathBuf::from)
        .unwrap_or_else(|| state.config.project_root.clone());
    let resolved = fs::canonicalize(&candidate).with_context(|| {
        format!(
            "terminal working directory is invalid: {}",
            candidate.display()
        )
    })?;
    let metadata = fs::metadata(&resolved)
        .with_context(|| format!("failed to inspect {}", resolved.display()))?;
    if !metadata.is_dir() {
        anyhow::bail!("terminal working directory must be a directory.");
    }

    let allowed_roots = resolved_allowed_roots(&state.config).await;
    let allowed = allowed_roots
        .iter()
        .any(|root| path_is_within(root, &resolved));

    if !allowed {
        anyhow::bail!("terminal working directory must stay within allowed roots.");
    }

    Ok(resolved.display().to_string())
}

async fn spawn_terminal_process(cwd: &str) -> Result<Child> {
    if cfg!(windows) {
        Command::new("powershell.exe")
            .current_dir(cwd)
            .arg("-NoLogo")
            .arg("-NoExit")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("failed to start terminal process")
    } else {
        Command::new("script")
            .current_dir(cwd)
            .args([
                "-q",
                "-f",
                "-c",
                "env TERM=xterm-256color bash --noprofile --norc -i",
                "/dev/null",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("failed to start terminal process")
    }
}

async fn create_terminal(
    state: AppState,
    cwd: Option<String>,
    title: Option<String>,
) -> Result<Value> {
    let cwd = validate_terminal_cwd(&state, cwd).await?;
    let mut child = spawn_terminal_process(&cwd).await?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("terminal stdout unavailable"))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("terminal stdin unavailable"))?;
    let terminal_id = Uuid::new_v4().to_string();
    let created_at = now_unix_ms();
    let (relay, _) = broadcast::channel(256);
    let session = Arc::new(TerminalSession {
        summary: Mutex::new(TerminalSummaryState {
            id: terminal_id.clone(),
            title: title
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| {
                    PathBuf::from(&cwd)
                        .file_name()
                        .and_then(|value| value.to_str())
                        .filter(|value| !value.is_empty())
                        .map(|value| format!("{value} shell"))
                        .unwrap_or_else(|| "Terminal".to_string())
                }),
            cwd: cwd.clone(),
            created_at,
            last_activity_at: created_at,
            status: "running".to_string(),
            exit_code: None,
        }),
        buffer: Mutex::new(String::new()),
        stdin: Mutex::new(Some(stdin)),
        relay,
        pid: child.id(),
    });

    state
        .terminals
        .lock()
        .await
        .insert(terminal_id.clone(), session.clone());

    let output_session = session.clone();
    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout);
        let mut buffer = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) => break,
                Ok(read) => {
                    let text = String::from_utf8_lossy(&buffer[..read]).to_string();
                    output_session.append_output(&text).await;
                }
                Err(error) => {
                    warn!("terminal output stream failed: {error:#}");
                    break;
                }
            }
        }
    });

    let exit_session = session.clone();
    let exit_state = state.clone();
    tokio::spawn(async move {
        let exit_code = child.wait().await.ok().and_then(|status| status.code());
        exit_session.mark_exited(exit_code).await;
        emit_terminals_updated(&exit_state).await;
    });

    emit_terminals_updated(&state).await;

    let (summary, snapshot) = session.snapshot().await;
    Ok(json!({
        "terminal": summary,
        "snapshot": snapshot
    }))
}

async fn list_terminals(state: &AppState) -> Result<Value> {
    Ok(json!({
        "terminals": list_terminal_summaries(state).await
    }))
}

async fn read_terminal(state: &AppState, terminal_id: &str) -> Result<Value> {
    let session = get_terminal_session(state, terminal_id).await?;
    let (summary, snapshot) = session.snapshot().await;
    Ok(json!({
        "terminal": summary,
        "snapshot": snapshot
    }))
}

async fn write_terminal_input(state: &AppState, terminal_id: &str, data: &str) -> Result<Value> {
    let session = get_terminal_session(state, terminal_id).await?;
    session.write_input(data).await?;
    Ok(json!({ "ok": true }))
}

async fn close_terminal(state: AppState, terminal_id: &str) -> Result<Value> {
    let session = state
        .terminals
        .lock()
        .await
        .remove(terminal_id)
        .ok_or_else(|| anyhow!("Terminal not found."))?;

    let _ = session.write_input("exit\r").await;
    if let Some(pid) = session.pid {
        let _ = terminate_process(pid).await;
    }

    emit_terminals_updated(&state).await;
    Ok(json!({ "ok": true }))
}

async fn codex_runtime_status(state: &AppState, check_latest: bool) -> Result<Value> {
    let configured_bin = state.config.codex_bin.clone();
    let resolved_bin_path = resolve_binary_path(&configured_bin).await;
    let npm_available = command_available(npm_command()).await;
    let install_command = format!("npm install -g {CODEX_NPM_PACKAGE}@latest");
    let update_command = install_command.clone();
    let mut issues = Vec::new();

    let version = match read_codex_version(state).await {
        Ok(version) => Some(version),
        Err(error) => {
            issues.push(error.to_string());
            None
        }
    };

    let mut latest_version: Option<String> = None;
    let mut update_available: Option<bool> = None;
    let mut last_checked_at: Option<String> = None;

    if check_latest {
        last_checked_at = Some(now_rfc3339());
        if npm_available {
            match fetch_latest_published_version().await {
                Ok(value) => {
                    latest_version = value;
                    update_available = latest_version
                        .as_deref()
                        .and_then(extract_semver)
                        .zip(version.as_deref().and_then(extract_semver))
                        .map(|(latest, current)| compare_versions(&latest, &current) > 0);
                }
                Err(error) => issues.push(error.to_string()),
            }
        } else {
            issues.push("npm was not found in PATH.".to_string());
        }
    }

    Ok(json!({
        "installed": version.is_some(),
        "configuredBin": configured_bin,
        "resolvedBinPath": resolved_bin_path,
        "npmAvailable": npm_available,
        "version": version,
        "latestVersion": latest_version,
        "updateAvailable": update_available,
        "installCommand": install_command,
        "updateCommand": update_command,
        "lastCheckedAt": last_checked_at,
        "issues": issues,
    }))
}

async fn install_or_update_codex(state: &AppState, install_if_missing: bool) -> Result<Value> {
    let before = codex_runtime_status(state, false).await?;
    let installed = before
        .get("installed")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    if !install_if_missing && !installed {
        anyhow::bail!("Codex is not installed yet. Install it first.");
    }

    if !command_available(npm_command()).await {
        anyhow::bail!("npm was not found in PATH.");
    }

    let package_spec = format!("{CODEX_NPM_PACKAGE}@latest");
    let output = run_command_with_timeout(
        npm_command(),
        vec!["install".to_string(), "-g".to_string(), package_spec],
        NPM_INSTALL_TIMEOUT,
    )
    .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let message = if !stderr.is_empty() { stderr } else { stdout };
        anyhow::bail!(if message.is_empty() {
            "npm install -g failed.".to_string()
        } else {
            message
        });
    }

    let runtime = codex_runtime_status(state, true).await?;
    Ok(json!({
        "ok": true,
        "message": if install_if_missing && !installed {
            "Codex installed successfully."
        } else {
            "Codex updated successfully."
        },
        "runtime": runtime,
    }))
}

async fn codex_quota_status(state: &AppState, refresh: bool, profile_id: &str) -> Result<Value> {
    if !refresh {
        let cache = state.quota_cache.lock().await;
        if let Some(cached) = cache.get(profile_id) {
            if cached.created_at.elapsed() < QUOTA_CACHE_TTL {
                return Ok(cached.payload.clone());
            }
        }
    }

    let payload = match fetch_codex_quota(state, profile_id).await {
        Ok(payload) => payload,
        Err(error) => json!({
            "available": false,
            "source": Value::Null,
            "fetchedAt": now_unix_ms(),
            "account": Value::Null,
            "plan": Value::Null,
            "fiveHour": Value::Null,
            "weekly": Value::Null,
            "error": error.to_string(),
        }),
    };

    let mut cache = state.quota_cache.lock().await;
    cache.insert(
        profile_id.to_string(),
        CachedQuota {
            created_at: Instant::now(),
            payload: payload.clone(),
        },
    );

    Ok(payload)
}

async fn get_account_state(state: &AppState, profile_id: &str) -> Result<Value> {
    let client = app_server_client(state, profile_id).await?;
    match client
        .request("account/read", json!({ "refreshToken": false }))
        .await
    {
        Ok(response) => Ok(json!({
            "account": response.get("account").cloned().unwrap_or_else(|| json!({})),
            "requiresOpenaiAuth": response
                .get("requiresOpenaiAuth")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })),
        Err(error) if is_invalid_refresh_token_error_message(&error.to_string()) => Ok(json!({
            "account": {},
            "requiresOpenaiAuth": true,
        })),
        Err(error) => Err(error),
    }
}

async fn start_account_login(state: &AppState, profile_id: &str, params: &Value) -> Result<Value> {
    let login_type = require_string(params, "type")?;
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    match login_type.as_str() {
        "chatgpt" | "chatgptDeviceCode" => {}
        "apiKey" => {
            let api_key = params
                .get("apiKey")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| anyhow!("API key is required."))?;
            let client = app_server_client(state, profile_id).await?;
            state.quota_cache.lock().await.remove(&resolved_profile_id);
            return client
                .request(
                    "account/login/start",
                    json!({ "type": login_type, "apiKey": api_key }),
                )
                .await;
        }
        _ => anyhow::bail!("Invalid account login type."),
    }

    let client = app_server_client(state, profile_id).await?;
    state.quota_cache.lock().await.remove(&resolved_profile_id);
    client
        .request("account/login/start", json!({ "type": login_type }))
        .await
}

async fn cancel_account_login(state: &AppState, profile_id: &str, params: &Value) -> Result<Value> {
    let client = app_server_client(state, profile_id).await?;
    client
        .request(
            "account/login/cancel",
            json!({
                "loginId": require_string(params, "loginId")?
            }),
        )
        .await
}

async fn logout_account(state: &AppState, profile_id: &str) -> Result<Value> {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    let client = app_server_client(state, profile_id).await?;
    state.quota_cache.lock().await.remove(&resolved_profile_id);
    client.request("account/logout", json!({})).await
}

async fn fetch_codex_quota(state: &AppState, profile_id: &str) -> Result<Value> {
    let profile = resolve_runtime_profile(&state.config, profile_id);
    let auth = read_codex_auth(&profile.codex_home)?;
    let access_token = auth
        .tokens
        .as_ref()
        .and_then(|tokens| tokens.access_token.as_deref())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("No access token found in CODEX_HOME auth.json."))?;
    let account_id = auth
        .tokens
        .as_ref()
        .and_then(|tokens| tokens.account_id.as_deref())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("No account id found in CODEX_HOME auth.json."))?;

    let response = state
        .http
        .get(CODEX_USAGE_URL)
        .timeout(QUOTA_REQUEST_TIMEOUT)
        .header("authorization", format!("Bearer {access_token}"))
        .header("chatgpt-account-id", account_id)
        .header("user-agent", CODEX_USAGE_USER_AGENT)
        .send()
        .await
        .context("failed to fetch Codex quota")?;

    if response.status() == StatusCode::UNAUTHORIZED {
        anyhow::bail!("Codex quota token expired. Re-authenticate Codex and refresh quota.");
    }

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!(if body.trim().is_empty() {
            format!("Codex quota request failed with {status}.")
        } else {
            format!("Codex quota request failed with {status}: {}", body.trim())
        });
    }

    let payload: UsageResponseShape = response
        .json()
        .await
        .context("invalid Codex quota response")?;
    let five_hour = normalize_quota_window(
        payload
            .rate_limit
            .as_ref()
            .and_then(|rate_limit| rate_limit.primary_window.as_ref()),
    );
    let weekly = normalize_quota_window(
        payload
            .rate_limit
            .as_ref()
            .and_then(|rate_limit| rate_limit.secondary_window.as_ref()),
    );

    Ok(json!({
        "available": five_hour.is_some() || weekly.is_some(),
        "source": "backend-api",
        "fetchedAt": now_unix_ms(),
        "account": payload.email,
        "plan": payload.plan_type,
        "fiveHour": five_hour,
        "weekly": weekly,
        "error": Value::Null,
    }))
}

fn read_codex_auth(codex_home: &PathBuf) -> Result<AuthFile> {
    let auth_path = codex_home.join("auth.json");
    let raw = fs::read_to_string(&auth_path)
        .with_context(|| format!("missing Codex auth file at {}.", auth_path.display()))?;
    serde_json::from_str(&raw).context("invalid Codex auth.json")
}

fn normalize_quota_window(window: Option<&UsageWindowShape>) -> Option<Value> {
    let window = window?;
    let used_percent = (window.used_percent.unwrap_or(0.0))
        .clamp(0.0, 100.0)
        .round() as u64;
    let reset_after_seconds = window
        .reset_after_seconds
        .filter(|value| *value > 0)
        .map(|value| value as u64);
    let reset_at = reset_after_seconds
        .map(|seconds| now_unix_ms().saturating_add(seconds.saturating_mul(1000)));

    Some(json!({
        "usedPercent": used_percent,
        "remainingPercent": 100_u64.saturating_sub(used_percent),
        "resetAfterSeconds": reset_after_seconds,
        "resetAt": reset_at,
    }))
}

fn is_invalid_refresh_token_error_message(message: &str) -> bool {
    let lowered = message.to_ascii_lowercase();
    lowered.contains("tokenrefreshfailed") || lowered.contains("invalid refresh token")
}

fn map_app_server_global_notification(notification: &AppServerNotification) -> Option<Value> {
    match notification.method.as_str() {
        "account/updated" => Some(json!({
            "kind": "notification",
            "method": "codex-webui/accountUpdated",
            "params": notification.params
        })),
        "account/login/completed" => Some(json!({
            "kind": "notification",
            "method": "codex-webui/accountLoginCompleted",
            "params": {
                "loginId": notification
                    .params
                    .get("loginId")
                    .cloned()
                    .unwrap_or(Value::Null),
                "success": notification
                    .params
                    .get("success")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                "error": notification
                    .params
                    .get("error")
                    .cloned()
                    .unwrap_or(Value::Null)
            }
        })),
        "account/rateLimits/updated" => Some(json!({
            "kind": "notification",
            "method": "codex-webui/accountRateLimitsUpdated",
            "params": notification.params
        })),
        _ => None,
    }
}

async fn app_server_client(state: &AppState, profile_id: &str) -> Result<AppServerClient> {
    let (resolved_profile_id, profile) = resolve_runtime_profile_entry(&state.config, profile_id);
    Ok(state
        .app_servers
        .get_or_create(AppServerProfile {
            id: resolved_profile_id.to_string(),
            codex_home: profile.codex_home.clone(),
        })
        .await)
}

fn resolve_runtime_profile_entry<'a>(
    config: &'a Config,
    profile_id: &'a str,
) -> (&'a str, &'a RuntimeProfile) {
    if let Some(profile) = config.profiles.get(profile_id) {
        return (profile_id, profile);
    }

    if let Some(profile) = config.profiles.get(&config.default_profile_id) {
        return (config.default_profile_id.as_str(), profile);
    }

    config
        .profiles
        .iter()
        .next()
        .map(|(resolved_profile_id, profile)| (resolved_profile_id.as_str(), profile))
        .expect("at least one runtime profile must exist")
}

fn resolve_runtime_profile<'a>(config: &'a Config, profile_id: &'a str) -> &'a RuntimeProfile {
    resolve_runtime_profile_entry(config, profile_id).1
}

async fn read_codex_version(state: &AppState) -> Result<String> {
    let output = run_command_with_timeout(
        &state.config.codex_bin,
        vec!["--version".to_string()],
        Duration::from_secs(5),
    )
    .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let message = if !stderr.is_empty() { stderr } else { stdout };
        anyhow::bail!(if message.is_empty() {
            "Codex binary did not report a version.".to_string()
        } else {
            message
        });
    }

    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if version.is_empty() {
        anyhow::bail!("Codex version output was empty.");
    }
    Ok(version)
}

async fn fetch_latest_published_version() -> Result<Option<String>> {
    let output = run_command_with_timeout(
        npm_command(),
        vec![
            "view".to_string(),
            CODEX_NPM_PACKAGE.to_string(),
            "version".to_string(),
            "--json".to_string(),
        ],
        NPM_VIEW_TIMEOUT,
    )
    .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let message = if !stderr.is_empty() { stderr } else { stdout };
        anyhow::bail!(if message.is_empty() {
            "Failed to query npm for the latest Codex version.".to_string()
        } else {
            message
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        return Ok(None);
    }

    if let Ok(value) = serde_json::from_str::<String>(&stdout) {
        return Ok(Some(value));
    }

    Ok(Some(stdout))
}

async fn run_command_with_timeout(
    command: &str,
    args: Vec<String>,
    timeout: Duration,
) -> Result<std::process::Output> {
    let child = Command::new(command)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start `{command}`"))?;

    tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| anyhow!("`{command}` timed out"))?
        .with_context(|| format!("failed to wait for `{command}`"))
}

async fn command_available(name: &str) -> bool {
    run_command_with_timeout(
        which_command(),
        vec![name.to_string()],
        Duration::from_secs(2),
    )
    .await
    .map(|output| output.status.success())
    .unwrap_or(false)
}

async fn resolve_binary_path(command: &str) -> Option<String> {
    let candidate = PathBuf::from(command);
    if candidate.exists() {
        return Some(candidate.display().to_string());
    }

    let output = run_command_with_timeout(
        which_command(),
        vec![command.to_string()],
        Duration::from_secs(2),
    )
    .await
    .ok()?;
    if !output.status.success() {
        return None;
    }

    let path = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()?
        .trim()
        .to_string();
    if path.is_empty() { None } else { Some(path) }
}

fn which_command() -> &'static str {
    if cfg!(windows) { "where" } else { "which" }
}

fn npm_command() -> &'static str {
    if cfg!(windows) { "npm.cmd" } else { "npm" }
}

fn extract_semver(value: &str) -> Option<(u64, u64, u64)> {
    let mut parts = value
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .find(|part| part.split('.').count() >= 3)?
        .split('.');

    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    Some((major, minor, patch))
}

fn compare_versions(left: &(u64, u64, u64), right: &(u64, u64, u64)) -> i8 {
    match left.cmp(right) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

fn trim_terminal_buffer(buffer: &mut String) {
    if buffer.len() <= TERMINAL_BUFFER_LIMIT {
        return;
    }

    let target = buffer.len().saturating_sub(TERMINAL_BUFFER_LIMIT);
    let trim_index = buffer
        .char_indices()
        .find(|(index, _)| *index >= target)
        .map(|(index, _)| index)
        .unwrap_or(0);
    buffer.replace_range(..trim_index, "");
}

async fn terminate_process(pid: u32) -> Result<()> {
    if cfg!(windows) {
        let output = run_command_with_timeout(
            "taskkill",
            vec![
                "/PID".to_string(),
                pid.to_string(),
                "/T".to_string(),
                "/F".to_string(),
            ],
            Duration::from_secs(4),
        )
        .await?;
        if !output.status.success() {
            anyhow::bail!("failed to stop terminal process.");
        }
        return Ok(());
    }

    let output = run_command_with_timeout(
        "kill",
        vec!["-TERM".to_string(), pid.to_string()],
        Duration::from_secs(4),
    )
    .await?;
    if !output.status.success() {
        anyhow::bail!("failed to stop terminal process.");
    }
    Ok(())
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn now_rfc3339() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| String::new())
}

async fn upload_attachments(
    state: &AppState,
    session_id: &str,
    files: Vec<UploadFilePayload>,
) -> Result<Value> {
    let mut form = Form::new();
    for file in files {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(file.data_base64)
            .context("invalid base64 attachment payload")?;
        let part = Part::bytes(bytes)
            .file_name(file.name.clone())
            .mime_str(
                file.mime_type
                    .as_deref()
                    .unwrap_or("application/octet-stream"),
            )
            .context("invalid attachment mime type")?;
        form = form.part("files", part);
    }

    let target = internal_url(
        &state.config,
        &format!("/api/sessions/{session_id}/attachments"),
    );
    let response = state
        .http
        .post(target)
        .header(INTERNAL_HEADER, &state.config.internal_proxy_token)
        .header(
            PROFILE_HEADER,
            ACTIVE_PROFILE_ID
                .try_with(|profile_id| profile_id.clone())
                .unwrap_or_else(|_| state.config.default_profile_id.clone()),
        )
        .multipart(form)
        .send()
        .await
        .context("failed to upload attachments to internal backend")?;

    parse_json_response(response).await
}

async fn internal_json_request(
    state: &AppState,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value> {
    let target = internal_url(&state.config, path);
    let mut request = state
        .http
        .request(
            reqwest::Method::from_bytes(method.as_str().as_bytes())?,
            target,
        )
        .header(INTERNAL_HEADER, &state.config.internal_proxy_token);

    if let Ok(profile_id) = ACTIVE_PROFILE_ID.try_with(|profile_id| profile_id.clone()) {
        request = request.header(PROFILE_HEADER, profile_id);
    }

    if let Some(body) = body {
        request = request.json(&body);
    }

    let response = request
        .send()
        .await
        .with_context(|| format!("failed internal request for {path}"))?;
    parse_json_response(response).await
}

async fn parse_json_response(response: reqwest::Response) -> Result<Value> {
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!(text));
    }
    serde_json::from_str(&text).context("invalid json from internal backend")
}

async fn forward_request(
    state: &AppState,
    method: Method,
    target: &str,
    headers: HeaderMap,
    body: Vec<u8>,
    json_body: Option<Value>,
    form: Option<Form>,
) -> Result<Response> {
    let mut request = state
        .http
        .request(
            reqwest::Method::from_bytes(method.as_str().as_bytes())?,
            target.to_string(),
        )
        .header(INTERNAL_HEADER, &state.config.internal_proxy_token);

    if let Ok(profile_id) = ACTIVE_PROFILE_ID.try_with(|profile_id| profile_id.clone()) {
        request = request.header(PROFILE_HEADER, profile_id);
    }

    for (name, value) in headers.iter() {
        if name == header::HOST
            || name == header::CONTENT_LENGTH
            || name.as_str() == INTERNAL_HEADER
            || name.as_str() == PROFILE_HEADER
        {
            continue;
        }
        request = request.header(name, value);
    }

    if let Some(json_body) = json_body {
        request = request.json(&json_body);
    } else if let Some(form) = form {
        request = request.multipart(form);
    } else if !body.is_empty() {
        request = request.body(body);
    }

    let upstream = request.send().await.context("failed to forward request")?;
    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let upstream_headers = upstream.headers().clone();
    let bytes = upstream
        .bytes()
        .await
        .context("failed to read upstream response")?;

    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = status;
    for (name, value) in upstream_headers.iter() {
        if name == header::TRANSFER_ENCODING || name == header::CONNECTION {
            continue;
        }
        response.headers_mut().insert(name, value.clone());
    }
    Ok(response)
}

async fn cached_response(state: &AppState, request_id: &str) -> Option<ServerEnvelope> {
    let mut cache = state.response_cache.lock().await;
    cache.retain(|_, entry| entry.created_at.elapsed() < CACHE_TTL);
    cache.get(request_id).map(|entry| entry.message.clone())
}

async fn cache_response(state: &AppState, request_id: &str, message: ServerEnvelope) {
    let mut cache = state.response_cache.lock().await;
    cache.retain(|_, entry| entry.created_at.elapsed() < CACHE_TTL);
    cache.insert(
        request_id.to_string(),
        CachedResponse {
            created_at: Instant::now(),
            message,
        },
    );
}

async fn register_inflight_request(
    state: &AppState,
    request_id: &str,
    out_tx: &mpsc::UnboundedSender<ServerEnvelope>,
) -> bool {
    let mut inflight = state.inflight_requests.lock().await;
    inflight.retain(|_, waiters| !waiters.is_empty());

    if let Some(waiters) = inflight.get_mut(request_id) {
        waiters.push(out_tx.clone());
        return false;
    }

    inflight.insert(request_id.to_string(), vec![out_tx.clone()]);
    true
}

async fn resolve_inflight_request(state: &AppState, request_id: &str, message: ServerEnvelope) {
    let waiters = {
        let mut inflight = state.inflight_requests.lock().await;
        inflight.remove(request_id).unwrap_or_default()
    };

    for waiter in waiters {
        let _ = waiter.send(message.clone());
    }
}

async fn check_rate_limit(state: &AppState, identifier: &str) -> bool {
    let now = now_millis();
    let mut attempts = state.login_attempts.lock().await;
    let history = attempts.entry(identifier.to_string()).or_default();
    history.retain(|entry| now.saturating_sub(*entry) < LOGIN_WINDOW_MS);
    history.len() < LOGIN_MAX_ATTEMPTS
}

async fn record_login_failure(state: &AppState, identifier: &str) {
    let now = now_millis();
    let mut attempts = state.login_attempts.lock().await;
    let history = attempts.entry(identifier.to_string()).or_default();
    history.retain(|entry| now.saturating_sub(*entry) < LOGIN_WINDOW_MS);
    history.push(now);
}

async fn clear_login_failures(state: &AppState, identifier: &str) {
    state.login_attempts.lock().await.remove(identifier);
}

fn verify_password_pair(
    plain: Option<&String>,
    hashed: Option<&String>,
    input: &str,
    required_error: &str,
) -> Result<bool> {
    if let Some(password) = plain {
        return Ok(password.as_bytes().ct_eq(input.as_bytes()).into());
    }

    let Some(password_hash) = hashed else {
        return Err(anyhow!(required_error.to_string()));
    };

    let mut parts = password_hash.split('$');
    let Some(kind) = parts.next() else {
        return Ok(false);
    };
    let Some(saved_salt) = parts.next() else {
        return Ok(false);
    };
    let Some(saved_key) = parts.next() else {
        return Ok(false);
    };

    if kind != "scrypt" {
        return Err(anyhow!("Unsupported password hash format."));
    }

    let salt = URL_SAFE_NO_PAD
        .decode(saved_salt)
        .context("invalid password hash salt")?;
    let expected = URL_SAFE_NO_PAD
        .decode(saved_key)
        .context("invalid password hash key")?;
    let params = ScryptParams::new(14, 8, 1, expected.len())?;
    let mut derived = vec![0_u8; expected.len()];
    scrypt(input.as_bytes(), &salt, &params, &mut derived)
        .context("failed to derive password hash")?;
    Ok(derived.ct_eq(&expected).into())
}

fn authenticate_role(config: &Config, input: &str) -> Result<Option<UserRole>> {
    if verify_password_pair(
        config.password.as_ref(),
        config.password_hash.as_ref(),
        input,
        "Set CODEX_WEBUI_PASSWORD_HASH or CODEX_WEBUI_PASSWORD before using the Rust gateway.",
    )? {
        return Ok(Some(UserRole::Admin));
    }

    if config.viewer_password.is_none() && config.viewer_password_hash.is_none() {
        return Ok(None);
    }

    if verify_password_pair(
        config.viewer_password.as_ref(),
        config.viewer_password_hash.as_ref(),
        input,
        "Failed to verify viewer password.",
    )? {
        return Ok(Some(UserRole::Viewer));
    }

    Ok(None)
}

fn issue_auth_cookie(
    config: &Config,
    jar: CookieJar,
    secure_request: bool,
    role: UserRole,
) -> Result<CookieJar> {
    let secure = resolve_cookie_secure(config, secure_request)?;
    let cookie_value = make_auth_token(config, role)?;
    let mut cookie = Cookie::new(AUTH_COOKIE, cookie_value);
    cookie.set_path("/");
    cookie.set_http_only(true);
    cookie.set_same_site(match config.cookie_same_site {
        SameSiteMode::Strict => SameSite::Strict,
        SameSiteMode::Lax => SameSite::Lax,
        SameSiteMode::None => SameSite::None,
    });
    cookie.set_secure(secure);
    cookie.set_max_age(CookieDuration::days(7));
    Ok(jar.add(cookie))
}

fn issue_profile_cookie(
    config: &Config,
    jar: CookieJar,
    secure_request: bool,
    profile_id: &str,
) -> Result<CookieJar> {
    let secure = resolve_cookie_secure(config, secure_request)?;
    let mut cookie = Cookie::new(PROFILE_COOKIE, profile_id.to_string());
    cookie.set_path("/");
    cookie.set_http_only(true);
    cookie.set_same_site(match config.cookie_same_site {
        SameSiteMode::Strict => SameSite::Strict,
        SameSiteMode::Lax => SameSite::Lax,
        SameSiteMode::None => SameSite::None,
    });
    cookie.set_secure(secure);
    cookie.set_max_age(CookieDuration::days(30));
    Ok(jar.add(cookie))
}

fn resolve_cookie_secure(config: &Config, secure_request: bool) -> Result<bool> {
    if config.cookie_same_site == SameSiteMode::None
        && config.cookie_secure_mode == CookieSecureMode::Never
    {
        return Err(anyhow!(
            "CODEX_WEBUI_COOKIE_SAMESITE=none cannot be combined with CODEX_WEBUI_COOKIE_SECURE=never."
        ));
    }

    match config.cookie_secure_mode {
        CookieSecureMode::Always => Ok(true),
        CookieSecureMode::Never => Ok(false),
        CookieSecureMode::Auto => {
            if config.cookie_same_site == SameSiteMode::None && !secure_request {
                Err(anyhow!(
                    "CODEX_WEBUI_COOKIE_SAMESITE=none requires HTTPS or CODEX_WEBUI_COOKIE_SECURE=always."
                ))
            } else {
                Ok(secure_request)
            }
        }
    }
}

fn make_auth_token(config: &Config, role: UserRole) -> Result<String> {
    let now = now_millis();
    let expires = now + 7 * 24 * 60 * 60 * 1000;
    let nonce = Uuid::new_v4().simple().to_string();
    let payload = format!(
        "{now}.{expires}.{}.{}",
        match role {
            UserRole::Admin => "admin",
            UserRole::Viewer => "viewer",
        },
        nonce
    );
    let signature = sign(config, &payload)?;
    Ok(format!("{payload}.{signature}"))
}

async fn select_profile(
    config: Arc<Config>,
    jar: CookieJar,
    headers: HeaderMap,
    request: Request,
    auth: AuthContext,
) -> std::result::Result<Response, String> {
    let secure_request = request_is_secure(&headers);
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .map_err(|_| "Invalid request body.".to_string())?;
    let payload: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
    let requested_profile_id = payload
        .get("profileId")
        .and_then(Value::as_str)
        .map(sanitize_profile_id)
        .unwrap_or_else(|| config.default_profile_id.clone());

    if !config.profiles.contains_key(&requested_profile_id) {
        return Ok(json_error(StatusCode::BAD_REQUEST, "Unknown profile."));
    }

    let next_jar = issue_profile_cookie(&config, jar, secure_request, &requested_profile_id)
        .map_err(|error| error.to_string())?;
    let _ = append_audit_log(
        &config,
        AuditLogEntry {
            id: Uuid::new_v4().to_string(),
            at: now_unix_ms(),
            role: match auth.role {
                UserRole::Admin => "admin".to_string(),
                UserRole::Viewer => "viewer".to_string(),
            },
            method: "auth/profile".to_string(),
            target: Some(requested_profile_id.clone()),
            ok: true,
            error: None,
        },
    )
    .await;

    Ok((
        next_jar,
        Json(json!({
            "ok": true,
            "activeProfileId": requested_profile_id,
        })),
    )
        .into_response())
}

fn auth_context(config: &Config, jar: &CookieJar) -> Option<AuthContext> {
    let Some(cookie) = jar.get(AUTH_COOKIE) else {
        return None;
    };
    let token = cookie.value();
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 4 && parts.len() != 5 {
        return None;
    }
    let payload = parts[..parts.len() - 1].join(".");
    let Ok(expected) = sign(config, &payload) else {
        return None;
    };
    if expected
        .as_bytes()
        .ct_eq(parts[parts.len() - 1].as_bytes())
        .unwrap_u8()
        != 1
    {
        return None;
    }
    let expires = parts[1].parse::<u128>().ok()?;
    if now_millis() >= expires {
        return None;
    }

    let role = if parts.len() == 5 {
        match parts[2] {
            "viewer" => UserRole::Viewer,
            _ => UserRole::Admin,
        }
    } else {
        UserRole::Admin
    };

    let profile_id = jar
        .get(PROFILE_COOKIE)
        .map(|cookie| sanitize_profile_id(cookie.value()))
        .filter(|value| config.profiles.contains_key(value))
        .unwrap_or_else(|| config.default_profile_id.clone());

    Some(AuthContext { role, profile_id })
}

fn sign(config: &Config, payload: &str) -> Result<String> {
    let secret = config
        .session_secret
        .clone()
        .or_else(|| config.password_hash.clone())
        .or_else(|| config.password.clone())
        .unwrap_or_default();
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).context("failed to initialize HMAC")?;
    mac.update(payload.as_bytes());
    Ok(URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
}

fn request_is_secure(headers: &HeaderMap) -> bool {
    if let Some(forwarded) = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
    {
        return forwarded.eq_ignore_ascii_case("https");
    }
    false
}

fn extract_origin(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .and_then(normalize_origin)
}

fn allowed_cors_origin(config: &Config, origin: &Option<String>) -> Option<String> {
    let origin = origin.as_ref()?;
    if config
        .cors_allowed_origins
        .iter()
        .any(|allowed| allowed == origin)
    {
        Some(origin.clone())
    } else {
        None
    }
}

fn apply_cors_headers(headers: &mut HeaderMap, origin: &str, request_headers: Option<&str>) {
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_str(origin).unwrap_or_else(|_| HeaderValue::from_static("null")),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
        HeaderValue::from_static("true"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET,HEAD,POST,PATCH,PUT,DELETE,OPTIONS"),
    );
    headers.insert(
        header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static("600"),
    );
    append_vary(headers, "Origin");
    if let Some(request_headers) = request_headers {
        if let Ok(value) = HeaderValue::from_str(request_headers) {
            headers.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, value);
        }
        append_vary(headers, "Access-Control-Request-Headers");
    }
}

fn append_vary(headers: &mut HeaderMap, value: &str) {
    let existing = headers
        .get(header::VARY)
        .and_then(|current| current.to_str().ok())
        .unwrap_or_default();
    let mut values: Vec<&str> = existing
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .collect();
    if !values.iter().any(|entry| *entry == value) {
        values.push(value);
    }
    if let Ok(header_value) = HeaderValue::from_str(&values.join(", ")) {
        headers.insert(header::VARY, header_value);
    }
}

fn json_error(status: StatusCode, message: &str) -> Response {
    let mut response = Json(json!({ "message": message })).into_response();
    *response.status_mut() = status;
    response
}

fn normalize_origin(value: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(value).ok()?;
    let host = parsed.host_str()?;
    let mut origin = format!("{}://{}", parsed.scheme(), host);
    if let Some(port) = parsed.port() {
        origin.push(':');
        origin.push_str(&port.to_string());
    }
    Some(origin)
}

fn normalize_base_path(value: Option<String>) -> String {
    let Some(value) = value.map(|value| value.trim().to_string()) else {
        return String::new();
    };
    if value.is_empty() || value == "/" {
        return String::new();
    }
    let trimmed = value.trim_end_matches('/');
    if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    }
}

fn normalize_request_path(base_path: &str, path: &str) -> NormalizedPath {
    if base_path.is_empty() {
        return NormalizedPath::Route(path.to_string());
    }

    if path == "/" {
        return NormalizedPath::Redirect(format!("{base_path}/"));
    }

    if path == base_path {
        return NormalizedPath::Redirect(format!("{base_path}/"));
    }

    if let Some(stripped) = path.strip_prefix(base_path) {
        if stripped.is_empty() {
            return NormalizedPath::Route("/".to_string());
        }
        if stripped.starts_with('/') {
            return NormalizedPath::Route(stripped.to_string());
        }
    }

    NormalizedPath::OutsideBase
}

fn with_base(base_path: &str, route_path: &str) -> String {
    if base_path.is_empty() {
        return route_path.to_string();
    }
    if route_path == "/" {
        return format!("{base_path}/");
    }
    format!("{base_path}{route_path}")
}

fn parse_port(value: Option<String>, fallback: u16) -> Result<u16> {
    match value {
        Some(value) => value
            .parse::<u16>()
            .with_context(|| format!("invalid port: {value}")),
        None => Ok(fallback),
    }
}

fn parse_same_site(value: Option<&str>) -> SameSiteMode {
    match value.unwrap_or("strict").to_ascii_lowercase().as_str() {
        "lax" => SameSiteMode::Lax,
        "none" => SameSiteMode::None,
        _ => SameSiteMode::Strict,
    }
}

fn parse_secure_mode(value: Option<&str>) -> CookieSecureMode {
    match value.unwrap_or("auto").to_ascii_lowercase().as_str() {
        "always" => CookieSecureMode::Always,
        "never" => CookieSecureMode::Never,
        _ => CookieSecureMode::Auto,
    }
}

fn parse_cors_origins(value: Option<String>) -> Result<Vec<String>> {
    let Some(raw) = value else {
        return Ok(Vec::new());
    };
    raw.split([',', '\n'])
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| normalize_origin(entry).ok_or_else(|| anyhow!("Invalid CORS origin: {entry}")))
        .collect()
}

fn choose_free_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").context("failed to allocate internal port")?;
    Ok(listener.local_addr()?.port())
}

fn resolve_codex_home() -> Result<PathBuf> {
    if let Some(value) = env::var_os("CODEX_WEBUI_CODEX_HOME").or_else(|| env::var_os("CODEX_HOME"))
    {
        return Ok(PathBuf::from(value));
    }

    if let Some(home) = env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")) {
        return Ok(PathBuf::from(home).join(".codex"));
    }

    Err(anyhow!(
        "Could not determine CODEX_HOME. Set CODEX_WEBUI_CODEX_HOME or CODEX_HOME."
    ))
}

fn load_dotenv(cwd: &PathBuf) {
    let project_root = resolve_project_root(cwd);
    let path = project_root.join(".env");
    if path.exists() {
        let _ = dotenvy::from_path(path);
    }
}

fn resolve_project_root(cwd: &PathBuf) -> PathBuf {
    if cwd.join("build/internal/index.js").exists() || cwd.join("svelte.config.js").exists() {
        return cwd.clone();
    }

    if cwd
        .file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value == "backend")
    {
        if let Some(parent) = cwd.parent() {
            let parent = parent.to_path_buf();
            if parent.join("build/internal/index.js").exists()
                || parent.join("svelte.config.js").exists()
            {
                return parent;
            }
        }
    }

    cwd.clone()
}

fn internal_url(config: &Config, path: &str) -> String {
    format!("{}{}", config.internal_base_url, path)
}

async fn spawn_internal_node(config: &Config) -> Result<(Child, Arc<Mutex<VecDeque<String>>>)> {
    let mut command = Command::new(&config.node_binary);
    command
        .arg(&config.node_entry)
        .current_dir(&config.project_root)
        .envs(env::vars())
        .env("HOST", "127.0.0.1")
        .env("PORT", config.internal_port.to_string())
        .env(
            "CODEX_WEBUI_INTERNAL_PROXY_TOKEN",
            &config.internal_proxy_token,
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .context("failed to spawn internal Node backend")?;
    let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
    if let Some(stderr) = child.stderr.take() {
        let stderr_tail = stderr_tail.clone();
        let stderr_log_path = internal_node_log_path(config);
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let message = line.trim();
                if !message.is_empty() {
                    append_text_log_line(&stderr_log_path, message);
                    {
                        let mut tail = stderr_tail.lock().await;
                        tail.push_back(message.to_string());
                        while tail.len() > INTERNAL_NODE_STDERR_TAIL_LIMIT {
                            tail.pop_front();
                        }
                    }
                    info!("[internal-node] {message}");
                }
            }
        });
    }
    Ok((child, stderr_tail))
}

async fn wait_for_internal_node(
    config: &Config,
    http: &reqwest::Client,
    stderr_tail: &Arc<Mutex<VecDeque<String>>>,
) -> Result<()> {
    let target = format!("{}/health", config.internal_base_url);

    for _ in 0..100 {
        if let Ok(response) = http
            .get(&target)
            .header(INTERNAL_HEADER, &config.internal_proxy_token)
            .send()
            .await
        {
            if response.status().is_success() || response.status().is_redirection() {
                info!(
                    "Internal Node backend is ready at {}",
                    config.internal_base_url
                );
                return Ok(());
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    append_runtime_error_log(
        config,
        "rust-gateway",
        "timed out waiting for internal Node backend",
        json!({
            "internalBaseUrl": config.internal_base_url,
            "internalNodeLogPath": internal_node_log_path(config).display().to_string(),
            "stderrTail": snapshot_stderr_tail(stderr_tail).await
        }),
    );
    Err(anyhow!("timed out waiting for internal Node backend"))
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn require_string(params: &Value, key: &str) -> Result<String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("{key} is required"))
}

enum NormalizedPath {
    Redirect(String),
    OutsideBase,
    Route(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::{collections::HashMap, sync::Arc};

    fn unique_test_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("codex-webui-{label}-{}", Uuid::new_v4()))
    }

    fn test_state(
        project_root: PathBuf,
        allowed_roots: Vec<PathBuf>,
        codex_home: PathBuf,
    ) -> AppState {
        let profile_id = "default".to_string();
        let mut profiles = HashMap::new();
        profiles.insert(profile_id.clone(), RuntimeProfile { codex_home });

        AppState {
            config: Arc::new(Config {
                project_root: project_root.clone(),
                allowed_roots,
                default_profile_id: profile_id,
                profiles,
                data_dir: project_root.join(".data"),
                base_path: String::new(),
                static_dir: project_root.join("static"),
                public_host: "127.0.0.1".to_string(),
                public_port: 4173,
                internal_port: 4174,
                internal_proxy_token: "test-token".to_string(),
                internal_base_url: "http://127.0.0.1:4174".to_string(),
                node_entry: project_root.join("node-entry.js"),
                node_binary: "node".to_string(),
                codex_bin: "codex".to_string(),
                password: None,
                password_hash: None,
                viewer_password: None,
                viewer_password_hash: None,
                hcaptcha_site_key: None,
                hcaptcha_secret_key: None,
                session_secret: None,
                cookie_same_site: SameSiteMode::Strict,
                cookie_secure_mode: CookieSecureMode::Auto,
                cors_allowed_origins: Vec::new(),
            }),
            app_servers: AppServerManager::new(AppServerClientConfig::default()),
            http: reqwest::Client::new(),
            login_attempts: Arc::new(Mutex::new(HashMap::new())),
            response_cache: Arc::new(Mutex::new(HashMap::new())),
            static_asset_cache: Arc::new(Mutex::new(HashMap::new())),
            inflight_requests: Arc::new(Mutex::new(HashMap::new())),
            quota_cache: Arc::new(Mutex::new(HashMap::new())),
            relays: Arc::new(Mutex::new(HashMap::new())),
            terminals: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[test]
    fn detects_invalid_refresh_token_errors() {
        assert!(is_invalid_refresh_token_error_message(
            "Auth(TokenRefreshFailed(\"Server returned error response: invalid_grant: Invalid refresh token\"))"
        ));
        assert!(!is_invalid_refresh_token_error_message(
            "some other runtime failure"
        ));
    }

    #[test]
    fn maps_account_login_completed_notifications() {
        let mapped = map_app_server_global_notification(&AppServerNotification {
            method: "account/login/completed".to_string(),
            params: json!({
                "loginId": "login-1",
                "success": true
            }),
        })
        .expect("notification should map");

        assert_eq!(
            mapped,
            json!({
                "kind": "notification",
                "method": "codex-webui/accountLoginCompleted",
                "params": {
                    "loginId": "login-1",
                    "success": true,
                    "error": Value::Null
                }
            })
        );
    }

    #[tokio::test]
    async fn lists_only_directories_within_allowed_root() {
        let sandbox = unique_test_dir("directories");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        fs::create_dir_all(workspace.join("alpha")).unwrap();
        fs::create_dir_all(workspace.join("beta")).unwrap();
        fs::create_dir_all(&codex_home).unwrap();
        fs::write(workspace.join("notes.txt"), "ignore").unwrap();

        let state = test_state(workspace.clone(), vec![workspace.clone()], codex_home);
        let payload: DirectoryPayload = serde_json::from_value(
            list_directories_payload(&state, Some(workspace.to_str().unwrap()))
                .await
                .expect("directory payload should load"),
        )
        .expect("payload should deserialize");

        assert_eq!(payload.allowed_roots.len(), 1);
        assert_eq!(payload.current_path, Some(workspace.display().to_string()));
        assert_eq!(payload.parent_path, None);
        assert_eq!(
            payload
                .entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "beta"]
        );

        let _ = fs::remove_dir_all(sandbox);
    }

    #[tokio::test]
    async fn writes_and_reads_editable_files_inside_profile_home() {
        let sandbox = unique_test_dir("editor");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&codex_home).unwrap();

        let state = test_state(workspace.clone(), vec![workspace], codex_home.clone());
        let config_path = codex_home.join("config.toml");
        let saved: EditableFilePayload = serde_json::from_value(
            write_editable_file_payload(
                &state,
                "default",
                config_path.to_str().unwrap(),
                "model = 'gpt-5.4'\n",
            )
            .await
            .expect("save should succeed"),
        )
        .expect("payload should deserialize");
        let loaded: EditableFilePayload = serde_json::from_value(
            read_editable_file_payload(&state, "default", config_path.to_str().unwrap())
                .await
                .expect("read should succeed"),
        )
        .expect("payload should deserialize");

        assert_eq!(saved.path, config_path.display().to_string());
        assert_eq!(saved.language, "ini");
        assert_eq!(loaded.content, "model = 'gpt-5.4'\n");
        assert!(loaded.writable);

        let _ = fs::remove_dir_all(sandbox);
    }

    #[tokio::test]
    async fn rejects_editable_files_outside_allowed_roots() {
        let sandbox = unique_test_dir("editor-outside");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        let outside = sandbox.join("outside").join("secret.txt");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&codex_home).unwrap();

        let state = test_state(workspace.clone(), vec![workspace], codex_home);
        let error = resolve_editable_file_path(&state, "default", outside.to_str().unwrap())
            .await
            .expect_err("outside paths must be rejected");

        assert_eq!(error.status, StatusCode::FORBIDDEN);
        assert_eq!(error.message, "This file is outside editable roots.");

        let _ = fs::remove_dir_all(sandbox);
    }
}
