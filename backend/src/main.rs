use std::{
    collections::{HashMap, HashSet, VecDeque},
    convert::Infallible,
    env, fs,
    future::Future,
    net::SocketAddr,
    path::{Component, Path, PathBuf},
    pin::Pin,
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{
        FromRequest, Multipart, Request, State,
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
use futures_util::{FutureExt, SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use scrypt::{Params as ScryptParams, scrypt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use time::Duration as CookieDuration;
use tokio::{
    fs as tokio_fs,
    io::{AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
    sync::{Mutex, broadcast, mpsc},
};
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

const AUTH_COOKIE: &str = "codex_webui_auth";
const PROFILE_COOKIE: &str = "codex_webui_profile";
const LOGIN_WINDOW_MS: u128 = 10 * 60 * 1000;
const LOGIN_MAX_ATTEMPTS: usize = 8;
const CACHE_TTL: Duration = Duration::from_secs(15 * 60);
const GLOBAL_RELAY_KEY: &str = "__global__";
const CODEX_NPM_PACKAGE: &str = "@openai/codex";
const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const CODEX_USAGE_USER_AGENT: &str = "codex_cli_rs/0.120.0 (Codex Web UI)";
const NPM_VIEW_TIMEOUT: Duration = Duration::from_millis(2500);
const NPM_INSTALL_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const QUOTA_CACHE_TTL: Duration = Duration::from_secs(60);
const CATALOG_CACHE_TTL: Duration = Duration::from_secs(10);
const GIT_REPOSITORY_CACHE_TTL: Duration = Duration::from_secs(5);
const QUOTA_REQUEST_TIMEOUT: Duration = Duration::from_secs(12);
const DEFAULT_NOTIFICATION_LIMIT: usize = 80;
const DEFAULT_AUTOMATION_RUN_HISTORY_LIMIT: usize = 40;
const TERMINAL_BUFFER_LIMIT: usize = 500_000;
const TERMINAL_RELAY_PREFIX: &str = "__terminal__:";
const STATIC_BASE_PLACEHOLDER: &str = "/__CODEX_WEBUI_BASE__";
const RUNTIME_ERROR_LOG_NAME: &str = "runtime-errors.jsonl";
const AUTOSTART_LABEL: &str = "dev.seorii.codex-webui";
const WINDOWS_STARTUP_SCRIPT: &str = "codex-webui.vbs";
const MACOS_LAUNCH_AGENT: &str = "dev.seorii.codex-webui.plist";
const LINUX_SYSTEMD_SERVICE: &str = "codex-webui-autostart.service";
const LINUX_DESKTOP_ENTRY: &str = "codex-webui.desktop";
const ATTACHMENT_PREAMBLE_START: &str = "[[codex-webui-attachments]]";
const ATTACHMENT_PREAMBLE_END: &str = "[[/codex-webui-attachments]]";

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

fn runtime_session_key(profile_id: &str, session_id: &str) -> String {
    format!("profile::{profile_id}::session-runtime::{session_id}")
}

fn runtime_logs_dir(config: &Config) -> PathBuf {
    config.data_dir.join("logs")
}

fn runtime_error_log_path(config: &Config) -> PathBuf {
    runtime_logs_dir(config).join(RUNTIME_ERROR_LOG_NAME)
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
    codex_bin: String,
    max_upload_bytes: u64,
    git_discovery_depth: u64,
    system_shutdown_enabled: bool,
    system_shutdown_delay_seconds: u64,
    system_shutdown_command_override: Option<String>,
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
    label: String,
    codex_home: PathBuf,
    data_dir: PathBuf,
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
    catalog_cache: Arc<Mutex<HashMap<String, CachedCatalog>>>,
    git_repository_cache: Arc<Mutex<Option<CachedGitRepositories>>>,
    pinned_git_repositories: Arc<Mutex<HashMap<String, Value>>>,
    inflight_requests: Arc<Mutex<HashMap<String, Vec<mpsc::UnboundedSender<ServerEnvelope>>>>>,
    quota_cache: Arc<Mutex<HashMap<String, CachedQuota>>>,
    relays: Arc<Mutex<HashMap<String, broadcast::Sender<Value>>>>,
    terminals: Arc<Mutex<HashMap<String, Arc<TerminalSession>>>>,
    ui_state_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    automation_timers: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    queue_dispatching: Arc<Mutex<HashSet<String>>>,
    active_turns: Arc<Mutex<HashMap<String, String>>>,
    pending_server_requests:
        Arc<Mutex<HashMap<String, HashMap<String, PendingServerRequestEntry>>>>,
    shutdown_timers: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
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

#[derive(Clone)]
struct CachedCatalog {
    created_at: Instant,
    payload: Value,
}

#[derive(Clone)]
struct CachedGitRepositories {
    created_at: Instant,
    repositories: Vec<Value>,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
struct PendingServerRequestEntry {
    raw_id: Value,
    method: String,
    params: Value,
    created_at: String,
}

#[derive(Debug, Deserialize)]
struct RuntimeProfileShape {
    id: Option<String>,
    label: Option<String>,
    #[serde(alias = "codex_home", alias = "codexHome")]
    codex_home: Option<String>,
    #[serde(alias = "data_dir", alias = "dataDir")]
    data_dir: Option<String>,
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

#[derive(Clone, Debug)]
struct AttachmentUploadPayload {
    name: String,
    mime_type: Option<String>,
    bytes: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredAttachmentRecord {
    id: String,
    #[serde(alias = "originalName")]
    original_name: String,
    path: Option<String>,
    #[serde(alias = "mimeType")]
    mime_type: Option<String>,
    size: Option<u64>,
    kind: Option<String>,
    #[serde(alias = "createdAt")]
    created_at: Option<String>,
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ArenaContestantRecord {
    id: String,
    session_id: String,
    model: String,
    label: String,
    status: String,
    response: Option<String>,
    created_at: u64,
    updated_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ArenaRunRecord {
    id: String,
    prompt: String,
    cwd: String,
    status: String,
    created_at: u64,
    updated_at: u64,
    contestants: Vec<ArenaContestantRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
struct ArenaStoreState {
    runs: Vec<ArenaRunRecord>,
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
            catalog_cache: Arc::new(Mutex::new(HashMap::new())),
            git_repository_cache: Arc::new(Mutex::new(None)),
            pinned_git_repositories: Arc::new(Mutex::new(HashMap::new())),
            inflight_requests: Arc::new(Mutex::new(HashMap::new())),
            quota_cache: Arc::new(Mutex::new(HashMap::new())),
            relays: Arc::new(Mutex::new(HashMap::new())),
            terminals: Arc::new(Mutex::new(HashMap::new())),
            ui_state_locks: Arc::new(Mutex::new(HashMap::new())),
            automation_timers: Arc::new(Mutex::new(HashMap::new())),
            queue_dispatching: Arc::new(Mutex::new(HashSet::new())),
            active_turns: Arc::new(Mutex::new(HashMap::new())),
            pending_server_requests: Arc::new(Mutex::new(HashMap::new())),
            shutdown_timers: Arc::new(Mutex::new(HashMap::new())),
        };

        tokio::spawn(restore_automation_schedules(state.clone()));
        for profile_id in state.config.profiles.keys().cloned().collect::<Vec<_>>() {
            tokio::spawn(restore_runtime_profile_state(state.clone(), profile_id));
        }

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

        let server_result = axum::serve(listener, router)
            .await
            .context("axum server terminated unexpectedly");
        let _ = state.app_servers.close_all().await;
        server_result
    }
    .await;

    if let Err(error) = &result {
        append_runtime_error_log(
            &config,
            "rust-gateway",
            "gateway fatal error",
            json!({ "error": format!("{error:#}") }),
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
        let data_dir = env::var("CODEX_WEBUI_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| cwd.join(".data"));
        let public_host = env::var("HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
        let public_port = parse_port(env::var("PORT").ok(), 4173)?;
        let max_upload_bytes = env::var("CODEX_WEBUI_MAX_UPLOAD_MB")
            .ok()
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| *value > 0.0)
            .map(|value| (value * 1024.0 * 1024.0).round() as u64)
            .unwrap_or(20 * 1024 * 1024);
        let git_discovery_depth = env::var("CODEX_WEBUI_GIT_DISCOVERY_DEPTH")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(1);
        let system_shutdown_delay_seconds = env::var("CODEX_WEBUI_SHUTDOWN_DELAY_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(30);
        if !static_dir.exists() {
            return Err(anyhow!(
                "missing static frontend build at {}. Run `pnpm build` in codex-webui first.",
                static_dir.display()
            ));
        }

        let codex_home = resolve_codex_home()?;
        let (default_profile_id, profiles) = parse_runtime_profiles(&codex_home, &data_dir)?;

        Ok(Self {
            project_root,
            allowed_roots,
            default_profile_id,
            profiles,
            data_dir,
            base_path,
            static_dir,
            public_host,
            public_port,
            codex_bin: env::var("CODEX_WEBUI_CODEX_BIN").unwrap_or_else(|_| "codex".to_string()),
            max_upload_bytes,
            git_discovery_depth,
            system_shutdown_enabled: env::var("CODEX_WEBUI_ENABLE_SYSTEM_SHUTDOWN")
                .is_ok_and(|value| value == "true"),
            system_shutdown_delay_seconds,
            system_shutdown_command_override: env::var("CODEX_WEBUI_SHUTDOWN_COMMAND")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
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
    root_data_dir: &PathBuf,
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
                label: "Default".to_string(),
                codex_home: default_codex_home.clone(),
                data_dir: root_data_dir.join("profiles").join(&default_profile_id),
            },
        );
        return Ok((default_profile_id, profiles));
    };

    let parsed: Vec<RuntimeProfileShape> =
        serde_json::from_str(&raw_profiles).context("invalid CODEX_WEBUI_PROFILES_JSON")?;
    let mut profiles = HashMap::new();

    for entry in parsed {
        let id = sanitize_profile_id(entry.id.as_deref().unwrap_or("default"));
        let label = entry
            .label
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                if id == default_profile_id {
                    "Default".to_string()
                } else {
                    id.clone()
                }
            });
        profiles
            .entry(id.clone())
            .or_insert_with(|| RuntimeProfile {
                label,
                codex_home: entry
                    .codex_home
                    .as_deref()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| default_codex_home.clone()),
                data_dir: entry
                    .data_dir
                    .as_deref()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| root_data_dir.join("profiles").join(&id)),
            });
    }

    if profiles.is_empty() {
        profiles.insert(
            default_profile_id.clone(),
            RuntimeProfile {
                label: "Default".to_string(),
                codex_home: default_codex_home.clone(),
                data_dir: root_data_dir.join("profiles").join(&default_profile_id),
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

fn query_param_values(query: Option<&str>, key: &str) -> Vec<String> {
    query
        .unwrap_or_default()
        .split('&')
        .filter_map(|entry| {
            let (raw_key, raw_value) = entry.split_once('=').unwrap_or((entry, ""));
            if raw_key != key {
                return None;
            }
            let decoded = raw_value.replace('+', "%20");
            urlencoding::decode(&decoded)
                .ok()
                .map(|value| value.into_owned())
        })
        .collect()
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

fn default_notification_settings_value() -> Value {
    json!({
        "enabledEventTypes": [
            "sessionCompleted",
            "sessionAttention",
            "queueDispatchFailed",
            "shutdownScheduled"
        ],
        "slackWebhookUrl": Value::Null,
        "webhookUrl": Value::Null
    })
}

fn default_ui_state_value() -> Value {
    json!({
        "global": {
            "shutdownAfterQueueCompletes": false,
            "scheduledShutdown": Value::Null
        },
        "notifications": {
            "items": [],
            "settings": default_notification_settings_value()
        },
        "sessionMetaByThreadId": {},
        "savedSessionFilters": [],
        "promptPresets": [],
        "automations": [],
        "automationRuns": [],
        "preferencesByThreadId": {},
        "draftsByThreadId": {},
        "queuesByThreadId": {},
        "highlightsByThreadId": {}
    })
}

fn ensure_ui_state_sections(ui_state: &mut Value) {
    if !ui_state.is_object() {
        *ui_state = default_ui_state_value();
        return;
    }

    let Some(root) = ui_state.as_object_mut() else {
        *ui_state = default_ui_state_value();
        return;
    };

    if !root.get("global").is_some_and(Value::is_object) {
        root.insert(
            "global".to_string(),
            json!({
                "shutdownAfterQueueCompletes": false,
                "scheduledShutdown": Value::Null
            }),
        );
    }

    if !root.get("notifications").is_some_and(Value::is_object) {
        root.insert(
            "notifications".to_string(),
            json!({
                "items": [],
                "settings": default_notification_settings_value()
            }),
        );
    }

    if let Some(notifications) = root.get_mut("notifications").and_then(Value::as_object_mut) {
        if !notifications.get("items").is_some_and(Value::is_array) {
            notifications.insert("items".to_string(), json!([]));
        }
        let normalized_settings =
            normalize_notification_settings_value(notifications.get("settings"));
        notifications.insert("settings".to_string(), normalized_settings);
    }

    for (key, default_value) in [
        ("sessionMetaByThreadId", json!({})),
        ("savedSessionFilters", json!([])),
        ("promptPresets", json!([])),
        ("automations", json!([])),
        ("automationRuns", json!([])),
        ("preferencesByThreadId", json!({})),
        ("draftsByThreadId", json!({})),
        ("queuesByThreadId", json!({})),
        ("highlightsByThreadId", json!({})),
    ] {
        let is_valid = if default_value.is_array() {
            root.get(key).is_some_and(Value::is_array)
        } else {
            root.get(key).is_some_and(Value::is_object)
        };
        if !is_valid {
            root.insert(key.to_string(), default_value);
        }
    }
}

fn is_valid_notification_event_type(value: &str) -> bool {
    matches!(
        value,
        "sessionCompleted" | "sessionAttention" | "queueDispatchFailed" | "shutdownScheduled"
    )
}

fn normalize_notification_settings_value(value: Option<&Value>) -> Value {
    let enabled_event_types = value
        .and_then(Value::as_object)
        .and_then(|settings| settings.get("enabledEventTypes"))
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(Value::as_str)
                .filter(|entry| is_valid_notification_event_type(entry))
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| {
            vec![
                "sessionCompleted".to_string(),
                "sessionAttention".to_string(),
                "queueDispatchFailed".to_string(),
                "shutdownScheduled".to_string(),
            ]
        });

    let normalize_url = |candidate: Option<&Value>| {
        candidate
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(|entry| Value::String(entry.to_string()))
            .unwrap_or(Value::Null)
    };

    json!({
        "enabledEventTypes": enabled_event_types,
        "slackWebhookUrl": normalize_url(value.and_then(|settings| settings.get("slackWebhookUrl"))),
        "webhookUrl": normalize_url(value.and_then(|settings| settings.get("webhookUrl")))
    })
}

fn profile_ui_state_path(config: &Config, profile_id: &str) -> PathBuf {
    resolve_runtime_profile(config, profile_id)
        .data_dir
        .join("ui-state.json")
}

async fn ui_state_lock(state: &AppState, profile_id: &str) -> Arc<Mutex<()>> {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    let mut locks = state.ui_state_locks.lock().await;
    locks
        .entry(resolved_profile_id)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

async fn read_profile_ui_state(config: &Config, profile_id: &str) -> Result<Value> {
    let profile = resolve_runtime_profile(config, profile_id);
    tokio_fs::create_dir_all(&profile.data_dir)
        .await
        .context("failed to create profile data directory")?;

    let path = profile_ui_state_path(config, profile_id);
    let raw = match tokio_fs::read_to_string(&path).await {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(default_ui_state_value());
        }
        Err(error) => return Err(error).context("failed to read ui-state file"),
    };

    match serde_json::from_str::<Value>(&raw) {
        Ok(mut parsed) => {
            ensure_ui_state_sections(&mut parsed);
            Ok(parsed)
        }
        Err(_) => {
            let backup_path = path.with_extension(format!("json.corrupt-{}", now_millis()));
            let _ = tokio_fs::rename(&path, &backup_path).await;
            let fallback = default_ui_state_value();
            tokio_fs::write(
                &path,
                serde_json::to_vec_pretty(&fallback).expect("default ui-state should serialize"),
            )
            .await
            .context("failed to recreate ui-state file after corruption")?;
            Ok(fallback)
        }
    }
}

async fn write_profile_ui_state(config: &Config, profile_id: &str, ui_state: &Value) -> Result<()> {
    let path = profile_ui_state_path(config, profile_id);
    if let Some(parent) = path.parent() {
        tokio_fs::create_dir_all(parent)
            .await
            .context("failed to create profile data directory")?;
    }
    let bytes = serde_json::to_vec_pretty(ui_state).context("failed to serialize ui-state")?;
    tokio_fs::write(&path, bytes)
        .await
        .context("failed to write ui-state file")?;
    Ok(())
}

fn theme_settings_path(config: &Config, profile_id: &str) -> PathBuf {
    resolve_runtime_profile(config, profile_id)
        .data_dir
        .join("theme-settings.json")
}

async fn read_stored_theme_settings(config: &Config, profile_id: &str) -> Result<Option<Value>> {
    let path = theme_settings_path(config, profile_id);
    let raw = match tokio_fs::read_to_string(&path).await {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("failed to read theme settings"),
    };

    match serde_json::from_str::<Value>(&raw) {
        Ok(value) => Ok(Some(value)),
        Err(_) => {
            let backup_path = path.with_extension(format!("json.corrupt-{}", now_millis()));
            let _ = tokio_fs::rename(&path, &backup_path).await;
            Ok(None)
        }
    }
}

async fn write_stored_theme_settings(
    config: &Config,
    profile_id: &str,
    theme: &Value,
) -> Result<Value> {
    let path = theme_settings_path(config, profile_id);
    if let Some(parent) = path.parent() {
        tokio_fs::create_dir_all(parent)
            .await
            .context("failed to create theme settings directory")?;
    }
    let payload = theme.clone();
    let bytes = serde_json::to_vec_pretty(&payload).context("failed to encode theme settings")?;
    tokio_fs::write(&path, bytes)
        .await
        .context("failed to write theme settings")?;
    Ok(payload)
}

fn home_dir_path() -> Option<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("USERPROFILE").map(PathBuf::from))
}

fn config_home_path() -> Option<PathBuf> {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir_path().map(|home| home.join(".config")))
}

fn windows_startup_path() -> Option<PathBuf> {
    env::var_os("APPDATA").map(PathBuf::from).map(|value| {
        value
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs")
            .join("Startup")
            .join(WINDOWS_STARTUP_SCRIPT)
    })
}

fn macos_launch_agent_path() -> Option<PathBuf> {
    home_dir_path().map(|home| {
        home.join("Library")
            .join("LaunchAgents")
            .join(MACOS_LAUNCH_AGENT)
    })
}

fn linux_systemd_user_path(config_home: &Path) -> PathBuf {
    config_home
        .join("systemd")
        .join("user")
        .join(LINUX_SYSTEMD_SERVICE)
}

fn linux_desktop_entry_path(config_home: &Path) -> PathBuf {
    config_home.join("autostart").join(LINUX_DESKTOP_ENTRY)
}

async fn path_exists_async(path: Option<&Path>) -> bool {
    match path {
        Some(path) => tokio_fs::metadata(path).await.is_ok(),
        None => false,
    }
}

fn current_launch_command(config: &Config) -> Result<(PathBuf, PathBuf)> {
    let executable = env::current_exe().context("failed to resolve the current executable")?;
    if !executable.exists() {
        anyhow::bail!(
            "Could not resolve the codex-webui executable at {}.",
            executable.display()
        );
    }
    Ok((executable, config.project_root.clone()))
}

fn escape_windows_vbs_string(value: &str) -> String {
    value.replace('"', "\"\"")
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn escape_systemd_value(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn escape_desktop_value(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

async fn can_use_linux_systemd_user() -> bool {
    run_command_with_timeout(
        "systemctl",
        vec!["--user".to_string(), "show-environment".to_string()],
        Duration::from_secs(3),
    )
    .await
    .map(|output| output.status.success())
    .unwrap_or(false)
}

async fn preferred_linux_autostart_provider(config_home: &Path) -> &'static str {
    if path_exists_async(Some(linux_systemd_user_path(config_home).as_path())).await {
        return "linux-systemd-user";
    }
    if path_exists_async(Some(linux_desktop_entry_path(config_home).as_path())).await {
        return "linux-xdg-autostart";
    }
    if can_use_linux_systemd_user().await {
        "linux-systemd-user"
    } else {
        "linux-xdg-autostart"
    }
}

async fn get_autostart_state(config: &Config) -> Result<Value> {
    if current_launch_command(config).is_err() {
        return Ok(json!({
            "available": false,
            "enabled": false,
            "provider": Value::Null,
            "location": Value::Null
        }));
    }

    if cfg!(windows) {
        let location = windows_startup_path();
        return Ok(json!({
            "available": location.is_some(),
            "enabled": path_exists_async(location.as_deref()).await,
            "provider": location.as_ref().map(|_| "windows-startup"),
            "location": location.map(|value| value.display().to_string())
        }));
    }

    if cfg!(target_os = "macos") {
        let location = macos_launch_agent_path();
        return Ok(json!({
            "available": location.is_some(),
            "enabled": path_exists_async(location.as_deref()).await,
            "provider": location.as_ref().map(|_| "macos-launch-agent"),
            "location": location.map(|value| value.display().to_string())
        }));
    }

    if cfg!(target_os = "linux") {
        let Some(config_home) = config_home_path() else {
            return Ok(json!({
                "available": false,
                "enabled": false,
                "provider": Value::Null,
                "location": Value::Null
            }));
        };
        let provider = preferred_linux_autostart_provider(&config_home).await;
        let location = if provider == "linux-systemd-user" {
            linux_systemd_user_path(&config_home)
        } else {
            linux_desktop_entry_path(&config_home)
        };
        return Ok(json!({
            "available": true,
            "enabled": path_exists_async(Some(location.as_path())).await,
            "provider": provider,
            "location": location.display().to_string()
        }));
    }

    Ok(json!({
        "available": false,
        "enabled": false,
        "provider": Value::Null,
        "location": Value::Null
    }))
}

async fn write_windows_startup_script(config: &Config) -> Result<()> {
    let target_path =
        windows_startup_path().ok_or_else(|| anyhow!("Windows startup folder is unavailable."))?;
    let (executable, working_directory) = current_launch_command(config)?;
    if let Some(parent) = target_path.parent() {
        tokio_fs::create_dir_all(parent)
            .await
            .context("failed to create the Windows startup directory")?;
    }
    tokio_fs::write(
        &target_path,
        [
            "Set WshShell = CreateObject(\"WScript.Shell\")".to_string(),
            format!(
                "WshShell.CurrentDirectory = \"{}\"",
                escape_windows_vbs_string(&working_directory.display().to_string())
            ),
            format!(
                "WshShell.Run \"\"\"\" & \"{}\" & \"\"\"\", 0, False",
                escape_windows_vbs_string(&executable.display().to_string())
            ),
        ]
        .join("\r\n"),
    )
    .await
    .context("failed to write the Windows startup script")?;
    Ok(())
}

async fn write_macos_launch_agent(config: &Config) -> Result<()> {
    let target_path = macos_launch_agent_path()
        .ok_or_else(|| anyhow!("LaunchAgents directory is unavailable."))?;
    let (executable, working_directory) = current_launch_command(config)?;
    if let Some(parent) = target_path.parent() {
        tokio_fs::create_dir_all(parent)
            .await
            .context("failed to create the LaunchAgents directory")?;
    }
    let log_path = config.data_dir.join("autostart-launch.log");
    if let Some(parent) = log_path.parent() {
        tokio_fs::create_dir_all(parent)
            .await
            .context("failed to create the autostart log directory")?;
    }
    tokio_fs::write(
        &target_path,
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n  <dict>\n    <key>Label</key>\n    <string>{}</string>\n    <key>ProgramArguments</key>\n    <array>\n      <string>{}</string>\n    </array>\n    <key>WorkingDirectory</key>\n    <string>{}</string>\n    <key>RunAtLoad</key>\n    <true/>\n    <key>KeepAlive</key>\n    <false/>\n    <key>StandardOutPath</key>\n    <string>{}</string>\n    <key>StandardErrorPath</key>\n    <string>{}</string>\n  </dict>\n</plist>\n",
            AUTOSTART_LABEL,
            escape_xml(&executable.display().to_string()),
            escape_xml(&working_directory.display().to_string()),
            escape_xml(&log_path.display().to_string()),
            escape_xml(&log_path.display().to_string())
        ),
    )
    .await
    .context("failed to write the launch agent")?;

    if let Ok(uid) = env::var("UID").or_else(|_| env::var("EUID")) {
        let domain = format!("gui/{uid}");
        let _ = run_command_with_timeout(
            "launchctl",
            vec![
                "bootout".to_string(),
                domain.clone(),
                target_path.display().to_string(),
            ],
            Duration::from_secs(4),
        )
        .await;
        let _ = run_command_with_timeout(
            "launchctl",
            vec![
                "bootstrap".to_string(),
                domain,
                target_path.display().to_string(),
            ],
            Duration::from_secs(4),
        )
        .await;
    }

    Ok(())
}

async fn write_linux_systemd_user_service(config: &Config) -> Result<()> {
    let config_home =
        config_home_path().ok_or_else(|| anyhow!("XDG config home is unavailable."))?;
    let target_path = linux_systemd_user_path(&config_home);
    let (executable, working_directory) = current_launch_command(config)?;
    if let Some(parent) = target_path.parent() {
        tokio_fs::create_dir_all(parent)
            .await
            .context("failed to create the systemd user directory")?;
    }
    tokio_fs::write(
        &target_path,
        format!(
            "[Unit]\nDescription=Codex Web UI autostart\n\n[Service]\nType=simple\nWorkingDirectory={}\nExecStart={}\nRestart=on-failure\nRestartSec=5\n\n[Install]\nWantedBy=default.target\n",
            escape_systemd_value(&working_directory.display().to_string()),
            escape_systemd_value(&executable.display().to_string())
        ),
    )
    .await
    .context("failed to write the systemd user service")?;

    let daemon_reload = run_command_with_timeout(
        "systemctl",
        vec!["--user".to_string(), "daemon-reload".to_string()],
        Duration::from_secs(5),
    )
    .await?;
    if !daemon_reload.status.success() {
        anyhow::bail!("Failed to reload the user systemd daemon.");
    }

    let enable = run_command_with_timeout(
        "systemctl",
        vec![
            "--user".to_string(),
            "enable".to_string(),
            LINUX_SYSTEMD_SERVICE.to_string(),
        ],
        Duration::from_secs(5),
    )
    .await?;
    if !enable.status.success() {
        anyhow::bail!("Failed to enable the user systemd service.");
    }

    Ok(())
}

async fn write_linux_desktop_entry(config: &Config) -> Result<()> {
    let config_home =
        config_home_path().ok_or_else(|| anyhow!("XDG config home is unavailable."))?;
    let target_path = linux_desktop_entry_path(&config_home);
    let (executable, working_directory) = current_launch_command(config)?;
    if let Some(parent) = target_path.parent() {
        tokio_fs::create_dir_all(parent)
            .await
            .context("failed to create the desktop autostart directory")?;
    }
    tokio_fs::write(
        &target_path,
        format!(
            "[Desktop Entry]\nType=Application\nVersion=1.0\nName=Codex Web UI\nComment=Start Codex Web UI automatically when you sign in\nExec={}\nPath={}\nTerminal=false\nX-GNOME-Autostart-enabled=true\nHidden=false\n",
            escape_desktop_value(&executable.display().to_string()),
            escape_desktop_value(&working_directory.display().to_string())
        ),
    )
    .await
    .context("failed to write the desktop autostart entry")?;
    Ok(())
}

async fn disable_windows_startup() {
    if let Some(path) = windows_startup_path() {
        let _ = tokio_fs::remove_file(path).await;
    }
}

async fn disable_macos_launch_agent() {
    if let Some(path) = macos_launch_agent_path() {
        if let Ok(uid) = env::var("UID").or_else(|_| env::var("EUID")) {
            let _ = run_command_with_timeout(
                "launchctl",
                vec![
                    "bootout".to_string(),
                    format!("gui/{uid}"),
                    path.display().to_string(),
                ],
                Duration::from_secs(4),
            )
            .await;
        }
        let _ = tokio_fs::remove_file(path).await;
    }
}

async fn disable_linux_autostart() {
    if let Some(config_home) = config_home_path() {
        let systemd_path = linux_systemd_user_path(&config_home);
        if path_exists_async(Some(systemd_path.as_path())).await {
            let _ = run_command_with_timeout(
                "systemctl",
                vec![
                    "--user".to_string(),
                    "disable".to_string(),
                    LINUX_SYSTEMD_SERVICE.to_string(),
                ],
                Duration::from_secs(5),
            )
            .await;
            let _ = tokio_fs::remove_file(&systemd_path).await;
            let _ = run_command_with_timeout(
                "systemctl",
                vec!["--user".to_string(), "daemon-reload".to_string()],
                Duration::from_secs(5),
            )
            .await;
        }
        let _ = tokio_fs::remove_file(linux_desktop_entry_path(&config_home)).await;
    }
}

async fn save_autostart_enabled(config: &Config, enabled: bool) -> Result<Value> {
    if !enabled {
        if cfg!(windows) {
            disable_windows_startup().await;
        } else if cfg!(target_os = "macos") {
            disable_macos_launch_agent().await;
        } else if cfg!(target_os = "linux") {
            disable_linux_autostart().await;
        }
        return get_autostart_state(config).await;
    }

    if cfg!(windows) {
        write_windows_startup_script(config).await?;
        return get_autostart_state(config).await;
    }

    if cfg!(target_os = "macos") {
        write_macos_launch_agent(config).await?;
        return get_autostart_state(config).await;
    }

    if cfg!(target_os = "linux") {
        if can_use_linux_systemd_user().await {
            match write_linux_systemd_user_service(config).await {
                Ok(()) => return get_autostart_state(config).await,
                Err(error) => {
                    warn!("failed to configure systemd user autostart: {error:#}");
                    if let Some(config_home) = config_home_path() {
                        let _ = tokio_fs::remove_file(linux_systemd_user_path(&config_home)).await;
                    }
                }
            }
        }

        write_linux_desktop_entry(config).await?;
        return get_autostart_state(config).await;
    }

    anyhow::bail!("Automatic startup is not supported on this operating system.");
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
struct SystemShutdownPlan {
    command: String,
    args: Vec<String>,
    availability_check: Option<(String, Vec<String>)>,
}

async fn is_root_user() -> bool {
    run_command_with_timeout("id", vec!["-u".to_string()], Duration::from_secs(2))
        .await
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim() == "0")
        .unwrap_or(false)
}

async fn resolve_command_path(command: &str) -> Option<String> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return None;
    }

    let candidate = PathBuf::from(trimmed);
    if candidate.is_absolute() || trimmed.contains('/') || trimmed.contains('\\') {
        if candidate.exists() {
            return Some(candidate.display().to_string());
        }
    }

    resolve_binary_path(trimmed).await
}

async fn resolve_system_shutdown_plan(config: &Config) -> Option<SystemShutdownPlan> {
    if !config.system_shutdown_enabled {
        return None;
    }

    if cfg!(windows) {
        let command = config
            .system_shutdown_command_override
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("shutdown")
            .to_string();
        return Some(SystemShutdownPlan {
            command,
            args: if config.system_shutdown_command_override.is_some() {
                Vec::new()
            } else {
                vec!["/s".to_string(), "/t".to_string(), "0".to_string()]
            },
            availability_check: None,
        });
    }

    let override_command = config
        .system_shutdown_command_override
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .to_string();
    let direct_command = if !override_command.is_empty() {
        resolve_command_path(&override_command).await
    } else if let Some(command) = resolve_command_path("shutdown").await {
        Some(command)
    } else if let Some(command) = resolve_command_path("/usr/sbin/shutdown").await {
        Some(command)
    } else if let Some(command) = resolve_command_path("/sbin/shutdown").await {
        Some(command)
    } else {
        resolve_command_path("systemctl").await
    }?;

    let direct_args = if !override_command.is_empty() {
        Vec::new()
    } else if Path::new(&direct_command)
        .file_name()
        .and_then(|value| value.to_str())
        == Some("systemctl")
    {
        vec!["poweroff".to_string()]
    } else {
        vec!["-h".to_string(), "now".to_string()]
    };

    if is_root_user().await {
        return Some(SystemShutdownPlan {
            command: direct_command,
            args: direct_args,
            availability_check: None,
        });
    }

    let sudo_command = resolve_command_path("sudo").await?;
    let mut sudo_args = vec!["-n".to_string(), direct_command.clone()];
    sudo_args.extend(direct_args.clone());
    let mut check_args = vec!["-n".to_string(), "-l".to_string(), direct_command];
    check_args.extend(direct_args);
    Some(SystemShutdownPlan {
        command: sudo_command.clone(),
        args: sudo_args,
        availability_check: Some((sudo_command, check_args)),
    })
}

async fn system_shutdown_capability(config: &Config) -> (bool, Option<SystemShutdownPlan>) {
    let Some(plan) = resolve_system_shutdown_plan(config).await else {
        return (false, None);
    };

    let Some((check_command, check_args)) = plan.availability_check.clone() else {
        return (true, Some(plan));
    };

    let available =
        run_command_with_timeout(&check_command, check_args, Duration::from_millis(1500))
            .await
            .map(|output| output.status.success())
            .unwrap_or(false);
    if available {
        (true, Some(plan))
    } else {
        (false, None)
    }
}

async fn with_ui_state_read<R, F>(state: &AppState, profile_id: &str, reader: F) -> ApiResult<R>
where
    F: FnOnce(&Value) -> ApiResult<R>,
{
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    let lock = ui_state_lock(state, &resolved_profile_id).await;
    let _guard = lock.lock().await;
    let ui_state = read_profile_ui_state(&state.config, &resolved_profile_id)
        .await
        .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    reader(&ui_state)
}

async fn with_ui_state_write<R, F>(state: &AppState, profile_id: &str, writer: F) -> ApiResult<R>
where
    F: FnOnce(&mut Value) -> ApiResult<R>,
{
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    let lock = ui_state_lock(state, &resolved_profile_id).await;
    let _guard = lock.lock().await;
    let mut ui_state = read_profile_ui_state(&state.config, &resolved_profile_id)
        .await
        .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    let result = writer(&mut ui_state)?;
    write_profile_ui_state(&state.config, &resolved_profile_id, &ui_state)
        .await
        .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    Ok(result)
}

fn ui_state_notification_items(ui_state: &Value) -> Vec<Value> {
    ui_state
        .get("notifications")
        .and_then(|value| value.get("items"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn unread_notification_count(items: &[Value]) -> usize {
    items
        .iter()
        .filter(|entry| entry.get("readAt").is_none_or(Value::is_null))
        .count()
}

fn notifications_payload_from_items(mut items: Vec<Value>, limit: usize) -> Value {
    items.sort_by(|left, right| {
        let left_created = left.get("createdAt").and_then(Value::as_i64).unwrap_or(0);
        let right_created = right.get("createdAt").and_then(Value::as_i64).unwrap_or(0);
        right_created.cmp(&left_created)
    });
    let unread_count = unread_notification_count(&items);
    let limited = items.into_iter().take(limit.max(1)).collect::<Vec<_>>();
    json!({
        "notifications": limited,
        "unreadCount": unread_count
    })
}

fn known_tags_from_ui_state(ui_state: &Value) -> Vec<String> {
    let mut tags = ui_state
        .get("sessionMetaByThreadId")
        .and_then(Value::as_object)
        .map(|entries| {
            entries
                .values()
                .filter_map(Value::as_object)
                .filter_map(|entry| entry.get("tags"))
                .filter_map(Value::as_array)
                .flat_map(|tags| tags.iter().filter_map(Value::as_str))
                .filter(|tag| !tag.trim().is_empty())
                .map(|tag| tag.trim().to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    tags.sort();
    tags.dedup();
    tags
}

async fn get_notifications_payload(
    state: &AppState,
    profile_id: &str,
    limit: usize,
) -> ApiResult<Value> {
    with_ui_state_read(state, profile_id, |ui_state| {
        Ok(notifications_payload_from_items(
            ui_state_notification_items(ui_state),
            limit,
        ))
    })
    .await
}

async fn mark_notifications_read_payload(
    state: &AppState,
    profile_id: &str,
    ids: Option<Vec<String>>,
) -> ApiResult<Value> {
    let target_ids = ids.map(|items| {
        items
            .into_iter()
            .filter_map(|item| {
                let trimmed = item.trim().to_string();
                (!trimmed.is_empty()).then_some(trimmed)
            })
            .collect::<Vec<_>>()
    });

    let (payload, changed) = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(items) = ui_state
            .get_mut("notifications")
            .and_then(Value::as_object_mut)
            .and_then(|value| value.get_mut("items"))
            .and_then(Value::as_array_mut)
        else {
            return Ok((json!({ "notifications": [], "unreadCount": 0 }), false));
        };

        let targets = target_ids.as_ref().map(|entries| {
            entries
                .iter()
                .cloned()
                .collect::<std::collections::HashSet<_>>()
        });
        let marked_at = now_unix_ms() as i64;
        let mut changed = false;

        for entry in items.iter_mut() {
            let read_at = entry.get("readAt");
            let entry_id = entry.get("id").and_then(Value::as_str);
            let should_mark = read_at.is_none_or(Value::is_null)
                && targets
                    .as_ref()
                    .is_none_or(|ids| entry_id.is_some_and(|candidate| ids.contains(candidate)));
            if should_mark {
                if let Some(object) = entry.as_object_mut() {
                    object.insert("readAt".to_string(), json!(marked_at));
                    changed = true;
                }
            }
        }

        Ok((
            notifications_payload_from_items(items.clone(), DEFAULT_NOTIFICATION_LIMIT),
            changed,
        ))
    })
    .await?;

    if changed {
        emit_profile_global_notification(
            state,
            profile_id,
            json!({
                "kind": "notification",
                "method": "codex-webui/notificationStateUpdated",
                "params": {
                    "unreadCount": payload.get("unreadCount").cloned().unwrap_or_else(|| json!(0))
                }
            }),
        )
        .await;
        emit_profile_config_updated(
            state,
            profile_id,
            json!({
                "notifications": {
                    "unreadCount": payload.get("unreadCount").cloned().unwrap_or_else(|| json!(0))
                }
            }),
        )
        .await;
    }

    Ok(payload)
}

async fn clear_notifications_payload(state: &AppState, profile_id: &str) -> ApiResult<Value> {
    let (payload, changed) = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(items) = ui_state
            .get_mut("notifications")
            .and_then(Value::as_object_mut)
            .and_then(|value| value.get_mut("items"))
            .and_then(Value::as_array_mut)
        else {
            return Ok((json!({ "notifications": [], "unreadCount": 0 }), false));
        };

        let changed = !items.is_empty();
        items.clear();
        Ok((
            notifications_payload_from_items(Vec::new(), DEFAULT_NOTIFICATION_LIMIT),
            changed,
        ))
    })
    .await?;

    if changed {
        emit_profile_global_notification(
            state,
            profile_id,
            json!({
                "kind": "notification",
                "method": "codex-webui/notificationStateUpdated",
                "params": {
                    "unreadCount": 0
                }
            }),
        )
        .await;
        emit_profile_config_updated(
            state,
            profile_id,
            json!({
                "notifications": {
                    "unreadCount": 0
                }
            }),
        )
        .await;
    }

    Ok(payload)
}

async fn update_notification_settings_payload(
    state: &AppState,
    profile_id: &str,
    patch: Value,
) -> ApiResult<Value> {
    let payload = with_ui_state_write(state, profile_id, |ui_state| {
        let notifications = ui_state
            .get_mut("notifications")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| api_error(StatusCode::INTERNAL_SERVER_ERROR, "notifications state is missing"))?;

        let current_settings = notifications.get("settings");
        let merged_settings = normalize_notification_settings_value(Some(&json!({
            "enabledEventTypes": patch.get("enabledEventTypes").cloned().unwrap_or_else(|| {
                current_settings
                    .and_then(|value| value.get("enabledEventTypes"))
                    .cloned()
                    .unwrap_or_else(|| default_notification_settings_value()["enabledEventTypes"].clone())
            }),
            "slackWebhookUrl": patch.get("slackWebhookUrl").cloned().unwrap_or_else(|| {
                current_settings
                    .and_then(|value| value.get("slackWebhookUrl"))
                    .cloned()
                    .unwrap_or(Value::Null)
            }),
            "webhookUrl": patch.get("webhookUrl").cloned().unwrap_or_else(|| {
                current_settings
                    .and_then(|value| value.get("webhookUrl"))
                    .cloned()
                    .unwrap_or(Value::Null)
            })
        })));

        notifications.insert("settings".to_string(), merged_settings.clone());
        let unread_count = notifications
            .get("items")
            .and_then(Value::as_array)
            .map(|items| unread_notification_count(items))
            .unwrap_or(0);

        Ok(json!({
            "settings": merged_settings,
            "unreadCount": unread_count
        }))
    })
    .await?;

    emit_profile_global_notification(
        state,
        profile_id,
        json!({
            "kind": "notification",
            "method": "codex-webui/notificationSettingsUpdated",
            "params": payload.clone()
        }),
    )
    .await;
    emit_profile_config_updated(
        state,
        profile_id,
        json!({
            "notifications": payload.clone()
        }),
    )
    .await;

    Ok(payload)
}

async fn save_session_filter_payload(
    state: &AppState,
    profile_id: &str,
    filter: Value,
) -> ApiResult<Value> {
    let name = filter
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "Filter name is required."))?;
    let filter_id = filter
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "filter.id is required."))?;

    let payload = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(saved_filters) = ui_state
            .get_mut("savedSessionFilters")
            .and_then(Value::as_array_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "saved filters state is missing",
            ));
        };

        let normalized_tags = filter
            .get("tags")
            .and_then(Value::as_array)
            .map(|tags| {
                let mut values = tags
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|tag| !tag.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                values.sort();
                values.dedup();
                values
            })
            .unwrap_or_default();

        let highlight = match filter.get("highlight").and_then(Value::as_str) {
            Some("attention") => "attention",
            Some("completed") => "completed",
            _ => "all",
        };

        let next_filter = json!({
            "id": filter_id,
            "name": name,
            "pinnedOnly": filter.get("pinnedOnly").and_then(Value::as_bool).unwrap_or(false),
            "runningOnly": filter.get("runningOnly").and_then(Value::as_bool).unwrap_or(false),
            "queuedOnly": filter.get("queuedOnly").and_then(Value::as_bool).unwrap_or(false),
            "highlight": highlight,
            "tags": normalized_tags
        });

        let mut next_saved_filters = vec![next_filter];
        next_saved_filters.extend(
            saved_filters
                .iter()
                .filter(|entry| entry.get("id").and_then(Value::as_str) != Some(filter_id))
                .cloned(),
        );
        next_saved_filters.truncate(40);
        *saved_filters = next_saved_filters;

        Ok(json!({
            "savedFilters": saved_filters.clone(),
            "knownTags": known_tags_from_ui_state(ui_state)
        }))
    })
    .await?;

    emit_profile_config_updated(
        state,
        profile_id,
        json!({
            "sessionOrganization": {
                "savedFilters": payload.get("savedFilters").cloned().unwrap_or_else(|| json!([])),
                "knownTags": payload.get("knownTags").cloned().unwrap_or_else(|| json!([]))
            }
        }),
    )
    .await;

    Ok(payload)
}

async fn delete_session_filter_payload(
    state: &AppState,
    profile_id: &str,
    filter_id: &str,
) -> ApiResult<Value> {
    let trimmed_filter_id = filter_id.trim();
    let payload = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(saved_filters) = ui_state
            .get_mut("savedSessionFilters")
            .and_then(Value::as_array_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "saved filters state is missing",
            ));
        };

        *saved_filters = saved_filters
            .iter()
            .filter(|entry| entry.get("id").and_then(Value::as_str) != Some(trimmed_filter_id))
            .cloned()
            .collect::<Vec<_>>();

        Ok(json!({
            "savedFilters": saved_filters.clone(),
            "knownTags": known_tags_from_ui_state(ui_state)
        }))
    })
    .await?;

    emit_profile_config_updated(
        state,
        profile_id,
        json!({
            "sessionOrganization": {
                "savedFilters": payload.get("savedFilters").cloned().unwrap_or_else(|| json!([])),
                "knownTags": payload.get("knownTags").cloned().unwrap_or_else(|| json!([]))
            }
        }),
    )
    .await;

    Ok(payload)
}

async fn save_prompt_preset_payload(
    state: &AppState,
    profile_id: &str,
    preset: Value,
) -> ApiResult<Value> {
    let preset_id = preset
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "preset.id is required."))?;
    let preset_name = preset
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "Preset name is required."))?;
    let preset_prompt = preset
        .get("prompt")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "Preset prompt is required."))?;

    let payload = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(prompt_presets) = ui_state
            .get_mut("promptPresets")
            .and_then(Value::as_array_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "prompt presets state is missing",
            ));
        };

        let now = now_unix_ms() as i64;
        let created_at = prompt_presets
            .iter()
            .find(|entry| entry.get("id").and_then(Value::as_str) == Some(preset_id))
            .and_then(|entry| entry.get("createdAt").and_then(Value::as_i64))
            .or_else(|| preset.get("createdAt").and_then(Value::as_i64))
            .unwrap_or(now);

        let next_preset = json!({
            "id": preset_id,
            "name": preset_name,
            "prompt": preset_prompt,
            "createdAt": created_at,
            "updatedAt": now
        });

        let mut next_prompt_presets = vec![next_preset];
        next_prompt_presets.extend(
            prompt_presets
                .iter()
                .filter(|entry| entry.get("id").and_then(Value::as_str) != Some(preset_id))
                .cloned(),
        );
        next_prompt_presets.truncate(80);
        next_prompt_presets.sort_by(|left, right| {
            let left_updated = left.get("updatedAt").and_then(Value::as_i64).unwrap_or(0);
            let right_updated = right.get("updatedAt").and_then(Value::as_i64).unwrap_or(0);
            right_updated.cmp(&left_updated)
        });
        *prompt_presets = next_prompt_presets;

        Ok(json!({
            "promptPresets": prompt_presets.clone()
        }))
    })
    .await?;

    emit_profile_config_updated(
        state,
        profile_id,
        json!({
            "promptPresets": payload.get("promptPresets").cloned().unwrap_or_else(|| json!([]))
        }),
    )
    .await;

    Ok(payload)
}

async fn delete_prompt_preset_payload(
    state: &AppState,
    profile_id: &str,
    preset_id: &str,
) -> ApiResult<Value> {
    let trimmed_preset_id = preset_id.trim();
    let payload = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(prompt_presets) = ui_state
            .get_mut("promptPresets")
            .and_then(Value::as_array_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "prompt presets state is missing",
            ));
        };

        *prompt_presets = prompt_presets
            .iter()
            .filter(|entry| entry.get("id").and_then(Value::as_str) != Some(trimmed_preset_id))
            .cloned()
            .collect::<Vec<_>>();
        prompt_presets.sort_by(|left, right| {
            let left_updated = left.get("updatedAt").and_then(Value::as_i64).unwrap_or(0);
            let right_updated = right.get("updatedAt").and_then(Value::as_i64).unwrap_or(0);
            right_updated.cmp(&left_updated)
        });

        Ok(json!({
            "promptPresets": prompt_presets.clone()
        }))
    })
    .await?;

    emit_profile_config_updated(
        state,
        profile_id,
        json!({
            "promptPresets": payload.get("promptPresets").cloned().unwrap_or_else(|| json!([]))
        }),
    )
    .await;

    Ok(payload)
}

async fn get_session_draft_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<Value> {
    with_ui_state_read(state, profile_id, |ui_state| {
        let stored = ui_state
            .get("draftsByThreadId")
            .and_then(Value::as_object)
            .and_then(|entries| entries.get(session_id));
        Ok(json!({
            "sessionId": session_id,
            "draft": stored
                .and_then(|entry| entry.get("draft"))
                .and_then(Value::as_str)
                .unwrap_or_default(),
            "intent": stored
                .and_then(|entry| entry.get("intent"))
                .cloned()
                .unwrap_or(Value::Null),
            "updatedAt": stored
                .and_then(|entry| entry.get("updatedAt"))
                .cloned()
                .unwrap_or(Value::Null)
        }))
    })
    .await
}

async fn save_session_draft_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    draft: &str,
    intent: &str,
) -> ApiResult<Value> {
    let trimmed = draft.trim();
    if trimmed.is_empty() {
        return clear_session_draft_payload(state, profile_id, session_id).await;
    }

    let normalized_intent = match intent {
        "steer" => "steer",
        "queue" => "queue",
        _ => "message",
    };
    let updated_at = now_unix_ms();
    with_ui_state_write(state, profile_id, |ui_state| {
        let Some(drafts_by_thread_id) = ui_state
            .get_mut("draftsByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "draft state is missing",
            ));
        };
        drafts_by_thread_id.insert(
            session_id.to_string(),
            json!({
                "draft": draft,
                "intent": normalized_intent,
                "updatedAt": updated_at
            }),
        );
        Ok(json!({
            "sessionId": session_id,
            "draft": draft,
            "intent": normalized_intent,
            "updatedAt": updated_at
        }))
    })
    .await
}

async fn clear_session_draft_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<Value> {
    with_ui_state_write(state, profile_id, |ui_state| {
        let Some(drafts_by_thread_id) = ui_state
            .get_mut("draftsByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "draft state is missing",
            ));
        };
        drafts_by_thread_id.remove(session_id);
        Ok(json!({
            "sessionId": session_id,
            "draft": "",
            "intent": Value::Null,
            "updatedAt": Value::Null
        }))
    })
    .await
}

async fn get_session_queue_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<Value> {
    with_ui_state_read(state, profile_id, |ui_state| {
        let stored = ui_state
            .get("queuesByThreadId")
            .and_then(Value::as_object)
            .and_then(|entries| entries.get(session_id));
        let items = stored
            .and_then(|entry| entry.get("items"))
            .cloned()
            .unwrap_or_else(|| json!([]));
        let resume_pending = stored
            .and_then(|entry| entry.get("resumePending"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let item_count = items.as_array().map(Vec::len).unwrap_or(0);
        Ok(json!({
            "sessionId": session_id,
            "items": items,
            "resumeRequired": resume_pending && item_count > 0,
            "updatedAt": stored
                .and_then(|entry| entry.get("updatedAt"))
                .cloned()
                .unwrap_or(Value::Null)
        }))
    })
    .await
}

fn string_array_from_value(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

async fn list_session_attachment_records(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> Result<Vec<StoredAttachmentRecord>> {
    let uploads_dir = resolve_runtime_profile(&state.config, profile_id)
        .data_dir
        .join("uploads")
        .join(session_id);
    let mut entries = match tokio_fs::read_dir(&uploads_dir).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "failed to read session uploads directory {}",
                    uploads_dir.display()
                )
            });
        }
    };

    let mut attachments = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let raw = match tokio_fs::read_to_string(&path).await {
            Ok(raw) => raw,
            Err(_) => continue,
        };
        if let Ok(record) = serde_json::from_str::<StoredAttachmentRecord>(&raw) {
            attachments.push(record);
        }
    }

    attachments.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    Ok(attachments)
}

fn sanitize_attachment_file_name(name: &str) -> String {
    let mut sanitized = String::new();
    let mut last_was_dash = false;
    for ch in name.chars() {
        let next = if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            ch
        } else {
            '-'
        };
        if next == '-' {
            if last_was_dash {
                continue;
            }
            last_was_dash = true;
        } else {
            last_was_dash = false;
        }
        sanitized.push(next);
    }
    let trimmed = sanitized.trim_matches('-');
    if trimmed.is_empty() {
        "attachment".to_string()
    } else {
        trimmed.to_string()
    }
}

fn attachment_storage_paths(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    attachment_id: &str,
    original_name: &str,
) -> (PathBuf, PathBuf) {
    let uploads_dir = resolve_runtime_profile(&state.config, profile_id)
        .data_dir
        .join("uploads")
        .join(session_id);
    let base = format!(
        "{attachment_id}-{}",
        sanitize_attachment_file_name(original_name)
    );
    (
        uploads_dir.join(&base),
        uploads_dir.join(format!("{base}.json")),
    )
}

fn attachment_kind_for_mime(mime_type: &str) -> &'static str {
    match mime_type {
        "image/png" | "image/jpeg" | "image/webp" | "image/gif" => "image",
        _ => "file",
    }
}

fn attachment_limit_error_message(max_upload_bytes: u64) -> String {
    let max_upload_mb = ((max_upload_bytes as f64) / (1024.0 * 1024.0)).round() as u64;
    format!("Upload exceeds the {max_upload_mb}MB limit.")
}

fn attachment_payload_from_record(record: &StoredAttachmentRecord) -> Value {
    json!({
        "id": record.id,
        "originalName": record.original_name,
        "path": record.path.clone().unwrap_or_default(),
        "mimeType": record
            .mime_type
            .clone()
            .unwrap_or_else(|| "application/octet-stream".to_string()),
        "size": record.size.unwrap_or(0),
        "kind": record.kind.clone().unwrap_or_else(|| "file".to_string()),
        "createdAt": record.created_at.clone().unwrap_or_default()
    })
}

async fn save_uploaded_attachment_records(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    uploads: Vec<AttachmentUploadPayload>,
) -> ApiResult<Vec<StoredAttachmentRecord>> {
    let mut stored = Vec::new();
    if uploads.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Select at least one file.",
        ));
    }

    let uploads_dir = resolve_runtime_profile(&state.config, profile_id)
        .data_dir
        .join("uploads")
        .join(session_id);
    tokio_fs::create_dir_all(&uploads_dir)
        .await
        .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;

    for upload in uploads {
        if upload.bytes.is_empty() {
            continue;
        }

        let size = upload.bytes.len() as u64;
        if size > state.config.max_upload_bytes {
            return Err(api_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                attachment_limit_error_message(state.config.max_upload_bytes),
            ));
        }

        let attachment_id = Uuid::new_v4().to_string();
        let original_name = if upload.name.trim().is_empty() {
            "attachment".to_string()
        } else {
            upload.name.trim().to_string()
        };
        let mime_type = upload
            .mime_type
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("application/octet-stream")
            .to_string();
        let (file_path, meta_path) = attachment_storage_paths(
            state,
            profile_id,
            session_id,
            &attachment_id,
            &original_name,
        );
        let record = StoredAttachmentRecord {
            id: attachment_id,
            original_name,
            path: Some(file_path.display().to_string()),
            mime_type: Some(mime_type.clone()),
            size: Some(size),
            kind: Some(attachment_kind_for_mime(&mime_type).to_string()),
            created_at: Some(now_unix_ms().to_string()),
        };

        tokio_fs::write(&file_path, &upload.bytes)
            .await
            .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
        let metadata = serde_json::to_vec_pretty(&record)
            .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
        tokio_fs::write(&meta_path, metadata)
            .await
            .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
        stored.push(record);
    }

    if stored.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Select at least one file.",
        ));
    }

    Ok(stored)
}

async fn emit_attachments_updated(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<()> {
    emit_session_notification(
        state,
        profile_id,
        session_id,
        json!({
            "kind": "notification",
            "method": "codex-webui/attachmentsUpdated",
            "params": {
                "attachments": list_session_attachments_payload(state, profile_id, session_id).await?
            }
        }),
    )
    .await;
    Ok(())
}

async fn resolve_queue_attachment_metadata(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    attachment_ids: Option<&Value>,
) -> ApiResult<(Vec<String>, Vec<String>)> {
    let requested_ids = string_array_from_value(attachment_ids);
    if requested_ids.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }

    let requested = requested_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let attachments = list_session_attachment_records(state, profile_id, session_id)
        .await
        .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    let filtered = attachments
        .into_iter()
        .filter(|attachment| requested.contains(attachment.id.as_str()))
        .collect::<Vec<_>>();

    Ok((
        filtered
            .iter()
            .map(|attachment| attachment.id.clone())
            .collect(),
        filtered
            .iter()
            .map(|attachment| attachment.original_name.clone())
            .collect(),
    ))
}

async fn emit_queue_updated(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    queue: Option<Value>,
) {
    let queue = match queue {
        Some(queue) => queue,
        None => match get_session_queue_payload(state, profile_id, session_id).await {
            Ok(queue) => queue,
            Err(_) => return,
        },
    };

    emit_session_notification(
        state,
        profile_id,
        session_id,
        json!({
            "kind": "notification",
            "method": "codex-webui/queueUpdated",
            "params": {
                "queue": queue
            }
        }),
    )
    .await;
    emit_session_summary_updated(state, profile_id, session_id, None).await;
    emit_runtime_profile_config_updated(state, profile_id).await;
}

async fn delete_attachment_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    attachment_id: &str,
) -> ApiResult<Value> {
    let attachments = list_session_attachment_records(state, profile_id, session_id)
        .await
        .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    let Some(target) = attachments
        .iter()
        .find(|attachment| attachment.id == attachment_id)
    else {
        return Err(api_error(StatusCode::NOT_FOUND, "Attachment not found."));
    };
    let (file_path, meta_path) = attachment_storage_paths(
        state,
        profile_id,
        session_id,
        attachment_id,
        &target.original_name,
    );
    let _ = tokio::join!(
        tokio_fs::remove_file(file_path),
        tokio_fs::remove_file(meta_path),
    );
    emit_attachments_updated(state, profile_id, session_id).await?;
    Ok(json!({ "ok": true }))
}

async fn enqueue_session_queue_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    prompt: &str,
    attachment_ids: Option<&Value>,
) -> ApiResult<Value> {
    let trimmed_prompt = prompt.trim();
    let (resolved_attachment_ids, attachment_names) =
        resolve_queue_attachment_metadata(state, profile_id, session_id, attachment_ids).await?;
    if trimmed_prompt.is_empty() && resolved_attachment_ids.is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "EMPTY_MESSAGE"));
    }

    cancel_scheduled_shutdown_for_activity(state, profile_id).await;

    let queue_item_id = Uuid::new_v4().to_string();
    with_ui_state_write(state, profile_id, |ui_state| {
        let Some(queues_by_thread_id) = ui_state
            .get_mut("queuesByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "queue state is missing",
            ));
        };

        let updated_at = now_unix_ms();
        let entry = queues_by_thread_id
            .entry(session_id.to_string())
            .or_insert_with(|| {
                json!({
                    "items": [],
                    "resumePending": false,
                    "updatedAt": updated_at
                })
            });
        let Some(queue) = entry.as_object_mut() else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "queue state had an unexpected shape",
            ));
        };
        let Some(items) = queue.get_mut("items").and_then(Value::as_array_mut) else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "queue items are missing",
            ));
        };
        items.push(json!({
            "id": queue_item_id,
            "prompt": trimmed_prompt,
            "attachmentIds": resolved_attachment_ids,
            "attachmentNames": attachment_names,
            "createdAt": updated_at
        }));
        queue.insert("resumePending".to_string(), json!(false));
        queue.insert("updatedAt".to_string(), json!(updated_at));
        Ok(())
    })
    .await?;

    let mut queue = get_session_queue_payload(state, profile_id, session_id).await?;
    if let Some(queue_object) = queue.as_object_mut() {
        queue_object.insert("enqueueAccepted".to_string(), json!(true));
        queue_object.insert("enqueueItemId".to_string(), json!(queue_item_id));
    }
    emit_queue_updated(state, profile_id, session_id, Some(queue.clone())).await;

    Ok(queue)
}

async fn remove_session_queue_item_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    queue_id: &str,
) -> ApiResult<Value> {
    let changed = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(queues_by_thread_id) = ui_state
            .get_mut("queuesByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "queue state is missing",
            ));
        };

        let Some(existing) = queues_by_thread_id.get_mut(session_id) else {
            return Ok(false);
        };
        let Some(queue) = existing.as_object_mut() else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "queue state had an unexpected shape",
            ));
        };
        let Some(items) = queue.get_mut("items").and_then(Value::as_array_mut) else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "queue items are missing",
            ));
        };

        let previous_len = items.len();
        items.retain(|item| item.get("id").and_then(Value::as_str) != Some(queue_id));
        if items.len() == previous_len {
            return Ok(false);
        }

        if items.is_empty() {
            queues_by_thread_id.remove(session_id);
        } else {
            queue.insert("updatedAt".to_string(), json!(now_unix_ms()));
        }
        Ok(true)
    })
    .await?;
    if !changed {
        return Err(api_error(StatusCode::NOT_FOUND, "QUEUE_ITEM_NOT_FOUND"));
    }

    let queue = get_session_queue_payload(state, profile_id, session_id).await?;
    emit_queue_updated(state, profile_id, session_id, Some(queue.clone())).await;
    maybe_schedule_global_shutdown(state, profile_id, None).await;
    Ok(queue)
}

async fn update_session_queue_item_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    queue_id: &str,
    prompt: Option<&str>,
    attachment_ids: Option<&Value>,
) -> ApiResult<Value> {
    let existing_queue = get_session_queue_payload(state, profile_id, session_id).await?;
    let queued_item = existing_queue
        .get("items")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.get("id").and_then(Value::as_str) == Some(queue_id))
                .cloned()
        })
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "QUEUE_ITEM_NOT_FOUND"))?;

    let next_prompt = prompt.map(str::to_string).unwrap_or_else(|| {
        queued_item
            .get("prompt")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    });
    let requested_attachment_ids = attachment_ids.cloned().unwrap_or_else(|| {
        queued_item
            .get("attachmentIds")
            .cloned()
            .unwrap_or_else(|| json!([]))
    });
    let (resolved_attachment_ids, attachment_names) = resolve_queue_attachment_metadata(
        state,
        profile_id,
        session_id,
        Some(&requested_attachment_ids),
    )
    .await?;
    if next_prompt.trim().is_empty() && resolved_attachment_ids.is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "EMPTY_MESSAGE"));
    }

    let changed = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(queues_by_thread_id) = ui_state
            .get_mut("queuesByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "queue state is missing",
            ));
        };
        let Some(existing) = queues_by_thread_id.get_mut(session_id) else {
            return Ok(false);
        };
        let Some(items) = existing.get_mut("items").and_then(Value::as_array_mut) else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "queue items are missing",
            ));
        };
        let Some(item) = items
            .iter_mut()
            .find(|item| item.get("id").and_then(Value::as_str) == Some(queue_id))
        else {
            return Ok(false);
        };
        let Some(item_object) = item.as_object_mut() else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "queue item had an unexpected shape",
            ));
        };
        item_object.insert("prompt".to_string(), json!(next_prompt.trim()));
        item_object.insert("attachmentIds".to_string(), json!(resolved_attachment_ids));
        item_object.insert("attachmentNames".to_string(), json!(attachment_names));
        if let Some(existing_object) = existing.as_object_mut() {
            existing_object.insert("updatedAt".to_string(), json!(now_unix_ms()));
        }
        Ok(true)
    })
    .await?;
    if !changed {
        return Err(api_error(StatusCode::NOT_FOUND, "QUEUE_ITEM_NOT_FOUND"));
    }

    let queue = get_session_queue_payload(state, profile_id, session_id).await?;
    emit_queue_updated(state, profile_id, session_id, Some(queue.clone())).await;
    Ok(queue)
}

async fn reorder_session_queue_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    ordered_ids: &[String],
) -> ApiResult<Value> {
    if ordered_ids.is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "QUEUE_ITEM_NOT_FOUND"));
    }

    let reordered = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(queues_by_thread_id) = ui_state
            .get_mut("queuesByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "queue state is missing",
            ));
        };
        let Some(existing) = queues_by_thread_id.get_mut(session_id) else {
            return Ok(false);
        };
        let Some(items) = existing.get_mut("items").and_then(Value::as_array_mut) else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "queue items are missing",
            ));
        };
        if ordered_ids.len() != items.len() {
            return Ok(false);
        }

        let items_by_id = items
            .iter()
            .filter_map(|item| {
                item.get("id")
                    .and_then(Value::as_str)
                    .map(|id| (id.to_string(), item.clone()))
            })
            .collect::<HashMap<_, _>>();
        let next_items = ordered_ids
            .iter()
            .filter_map(|queue_id| items_by_id.get(queue_id).cloned())
            .collect::<Vec<_>>();
        if next_items.len() != items.len()
            || ordered_ids.iter().collect::<HashSet<_>>().len() != ordered_ids.len()
        {
            return Ok(false);
        }

        *items = next_items;
        if let Some(existing_object) = existing.as_object_mut() {
            existing_object.insert("updatedAt".to_string(), json!(now_unix_ms()));
        }
        Ok(true)
    })
    .await?;
    if !reordered {
        return Err(api_error(StatusCode::NOT_FOUND, "QUEUE_ITEM_NOT_FOUND"));
    }

    let queue = get_session_queue_payload(state, profile_id, session_id).await?;
    emit_queue_updated(state, profile_id, session_id, Some(queue.clone())).await;
    Ok(queue)
}

async fn resume_session_queue_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<Value> {
    with_ui_state_write(state, profile_id, |ui_state| {
        let Some(queues_by_thread_id) = ui_state
            .get_mut("queuesByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "queue state is missing",
            ));
        };
        let Some(existing) = queues_by_thread_id.get_mut(session_id) else {
            return Ok(());
        };
        let Some(queue_object) = existing.as_object_mut() else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "queue state had an unexpected shape",
            ));
        };
        queue_object.insert("resumePending".to_string(), json!(false));
        queue_object.insert("updatedAt".to_string(), json!(now_unix_ms()));
        Ok(())
    })
    .await?;

    let queue = get_session_queue_payload(state, profile_id, session_id).await?;
    emit_queue_updated(state, profile_id, session_id, Some(queue.clone())).await;
    maybe_drain_queue(state, profile_id, session_id).await;
    Ok(queue)
}

async fn dispatch_session_queue_item_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    queue_id: &str,
    mode: &str,
) -> ApiResult<Value> {
    if mode != "message" && mode != "steer" {
        return Err(api_error(StatusCode::BAD_REQUEST, "INVALID_QUEUE_MODE"));
    }

    let queue = with_queue_dispatch_guard(state, profile_id, session_id, async {
        let stored_queue = get_session_queue_payload(state, profile_id, session_id).await?;
        let queued_item = stored_queue
            .get("items")
            .and_then(Value::as_array)
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item.get("id").and_then(Value::as_str) == Some(queue_id))
                    .cloned()
            })
            .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "QUEUE_ITEM_NOT_FOUND"))?;

        cancel_scheduled_shutdown_for_activity(state, profile_id).await;
        dispatch_queue_item(state, profile_id, session_id, &queued_item, mode).await?;
        let next_queue =
            remove_session_queue_item_after_dispatch(state, profile_id, session_id, queue_id)
                .await?;
        Ok(next_queue)
    })
    .await;

    match queue {
        Some(result) => result,
        None => Err(api_error(StatusCode::CONFLICT, "QUEUE_ALREADY_DISPATCHING")),
    }
}

async fn list_resume_pending_queues_payload(
    state: &AppState,
    profile_id: &str,
) -> ApiResult<Value> {
    let (entries, preferences_by_thread_id) = with_ui_state_read(state, profile_id, |ui_state| {
        let entries = ui_state
            .get("queuesByThreadId")
            .and_then(Value::as_object)
            .map(|queues| {
                queues
                    .iter()
                    .filter_map(|(session_id, queue)| {
                        let items = queue.get("items").and_then(Value::as_array)?;
                        let resume_pending = queue
                            .get("resumePending")
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        if !resume_pending || items.is_empty() {
                            return None;
                        }
                        Some((
                            session_id.clone(),
                            items.len(),
                            queue.get("updatedAt").and_then(Value::as_u64).unwrap_or(0),
                        ))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let preferences = ui_state
            .get("preferencesByThreadId")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        Ok((entries, preferences))
    })
    .await?;

    let mut paused = Vec::with_capacity(entries.len());
    for (session_id, pending_count, updated_at) in entries {
        let mut name = Value::Null;
        let mut cwd = preferences_by_thread_id
            .get(&session_id)
            .and_then(|entry| entry.get("cwd"))
            .cloned()
            .unwrap_or(Value::Null);

        if let Ok(thread) = read_thread_payload(state, profile_id, &session_id, false).await {
            if let Some(thread) = thread.as_object() {
                name = display_thread_name(
                    thread.get("name").and_then(Value::as_str),
                    thread.get("preview").and_then(Value::as_str),
                )
                .map(Value::from)
                .unwrap_or(Value::Null);
                if !thread.get("cwd").is_none_or(Value::is_null) {
                    cwd = thread.get("cwd").cloned().unwrap_or(Value::Null);
                }
            }
        }

        paused.push(json!({
            "sessionId": session_id,
            "name": name,
            "cwd": cwd,
            "pendingCount": pending_count,
            "updatedAt": updated_at
        }));
    }

    Ok(Value::Array(paused))
}

async fn mark_queues_pending_resume_payload(state: &AppState, profile_id: &str) -> ApiResult<bool> {
    with_ui_state_write(state, profile_id, |ui_state| {
        let Some(queues_by_thread_id) = ui_state
            .get_mut("queuesByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "queue state is missing",
            ));
        };

        let mut changed = false;
        for queue in queues_by_thread_id.values_mut() {
            let Some(queue_object) = queue.as_object_mut() else {
                continue;
            };
            let item_count = queue_object
                .get("items")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0);
            let resume_pending = queue_object
                .get("resumePending")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if item_count == 0 || resume_pending {
                continue;
            }
            queue_object.insert("resumePending".to_string(), json!(true));
            queue_object.insert("updatedAt".to_string(), json!(now_unix_ms()));
            changed = true;
        }

        Ok(changed)
    })
    .await
}

const CONFIG_SCHEMA_HEADER: &str =
    "#:schema https://developers.openai.com/codex/config-schema.json";

#[derive(Default)]
struct CodexTomlDefaults {
    model: Option<String>,
    model_reasoning_effort: Option<String>,
    plan_mode_reasoning_effort: Option<String>,
    personality: Option<String>,
    approval_policy: Option<String>,
    sandbox_mode: Option<String>,
    service_tier: String,
    network_access: Option<bool>,
}

fn config_toml_path(codex_home: &Path) -> PathBuf {
    codex_home.join("config.toml")
}

fn parse_toml_section_name(line: &str) -> Option<String> {
    let trimmed = line.trim();
    trimmed
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn trim_toml_value(value: &str) -> String {
    let mut trimmed = String::new();
    let mut escaped = false;
    let mut quote = None;
    for character in value.chars() {
        if escaped {
            trimmed.push(character);
            escaped = false;
            continue;
        }
        if character == '\\' {
            trimmed.push(character);
            escaped = true;
            continue;
        }
        if matches!(character, '"' | '\'') && (quote.is_none() || quote == Some(character)) {
            quote = if quote.is_some() {
                None
            } else {
                Some(character)
            };
            trimmed.push(character);
            continue;
        }
        if character == '#' && quote.is_none() {
            break;
        }
        trimmed.push(character);
    }
    trimmed.trim().to_string()
}

fn get_toml_value(raw: &str, section: Option<&str>, key: &str) -> Option<String> {
    let mut current_section: Option<String> = None;
    for line in raw.lines() {
        if let Some(next_section) = parse_toml_section_name(line) {
            current_section = Some(next_section);
            continue;
        }
        if current_section.as_deref() != section || !matches_toml_key(line, key) {
            continue;
        }
        let (_, value) = line.split_once('=')?;
        return Some(trim_toml_value(value));
    }
    None
}

fn parse_toml_string_value(value: Option<String>) -> Option<String> {
    let value = value?;
    if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
        return serde_json::from_str::<String>(&value).ok().or_else(|| {
            Some(
                value[1..value.len().saturating_sub(1)]
                    .replace("\\\"", "\"")
                    .replace("\\\\", "\\"),
            )
        });
    }
    if value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2 {
        return Some(value[1..value.len().saturating_sub(1)].to_string());
    }
    None
}

fn parse_toml_bool_value(value: Option<String>) -> Option<bool> {
    match value.as_deref() {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    }
}

fn read_codex_toml_defaults(codex_home: &Path) -> CodexTomlDefaults {
    let file_path = config_toml_path(codex_home);
    let Ok(raw) = fs::read_to_string(file_path) else {
        return CodexTomlDefaults {
            service_tier: "auto".to_string(),
            ..CodexTomlDefaults::default()
        };
    };

    let service_tier = parse_toml_string_value(get_toml_value(&raw, None, "service_tier"))
        .filter(|value| value == "fast" || value == "flex")
        .unwrap_or_else(|| "auto".to_string());

    CodexTomlDefaults {
        model: parse_toml_string_value(get_toml_value(&raw, None, "model")),
        model_reasoning_effort: parse_toml_string_value(get_toml_value(
            &raw,
            None,
            "model_reasoning_effort",
        )),
        plan_mode_reasoning_effort: parse_toml_string_value(get_toml_value(
            &raw,
            None,
            "plan_mode_reasoning_effort",
        )),
        personality: parse_toml_string_value(get_toml_value(&raw, None, "personality"))
            .filter(|value| matches!(value.as_str(), "none" | "friendly" | "pragmatic")),
        approval_policy: parse_toml_string_value(get_toml_value(&raw, None, "approval_policy")),
        sandbox_mode: parse_toml_string_value(get_toml_value(&raw, None, "sandbox_mode")),
        service_tier,
        network_access: parse_toml_bool_value(get_toml_value(
            &raw,
            Some("sandbox_workspace_write"),
            "network_access",
        )),
    }
}

fn matches_toml_key(line: &str, key: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed
        .strip_prefix(key)
        .is_some_and(|rest| rest.trim_start().starts_with('='))
}

fn normalize_toml_lines(raw: &str) -> Vec<String> {
    let mut lines = raw
        .split('\n')
        .map(|line| line.trim_end_matches('\r').to_string())
        .collect::<Vec<_>>();
    while lines.len() > 1 && lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines
}

fn stringify_toml_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn upsert_toml_value(raw: &str, section: Option<&str>, key: &str, value: Option<String>) -> String {
    let mut lines = normalize_toml_lines(raw);
    let mut current_section: Option<String> = None;
    let mut section_start = if section.is_none() {
        Some(0usize)
    } else {
        None
    };
    let mut section_end = lines.len();
    let mut replaced = false;

    for index in 0..lines.len() {
        if let Some(next_section) = parse_toml_section_name(&lines[index]) {
            if current_section.as_deref() == section && section_end == lines.len() {
                section_end = index;
            }
            current_section = Some(next_section.clone());
            if section.is_some() && section_start.is_none() && current_section.as_deref() == section
            {
                section_start = Some(index);
            }
            continue;
        }

        if current_section.as_deref() != section || !matches_toml_key(&lines[index], key) {
            continue;
        }

        replaced = true;
        if let Some(value) = &value {
            lines[index] = format!("{key} = {value}");
        } else {
            lines.remove(index);
            return upsert_toml_value(&lines.join("\n"), section, key, None);
        }
    }

    if !replaced {
        if let Some(value) = value {
            if section.is_none() {
                let insert_index = lines
                    .iter()
                    .position(|line| parse_toml_section_name(line).is_some())
                    .unwrap_or(lines.len());
                lines.insert(insert_index, format!("{key} = {value}"));
            } else if let Some(section_start) = section_start {
                lines.insert(
                    section_end.max(section_start + 1),
                    format!("{key} = {value}"),
                );
            } else {
                if !lines.is_empty() && lines.last().is_some_and(|line| !line.is_empty()) {
                    lines.push(String::new());
                }
                lines.push(format!("[{}]", section.unwrap_or_default()));
                lines.push(format!("{key} = {value}"));
            }
        }
    }

    while lines.len() > 1 && lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }

    format!("{}\n", lines.join("\n"))
}

async fn sync_codex_toml_with_preferences(codex_home: &Path, preferences: &Value) -> Result<()> {
    let file_path = config_toml_path(codex_home);
    if let Some(parent) = file_path.parent() {
        tokio_fs::create_dir_all(parent)
            .await
            .context("failed to create the Codex config directory")?;
    }

    let mut raw = match tokio_fs::read_to_string(&file_path).await {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error).context("failed to read config.toml"),
    };
    if raw.trim().is_empty() {
        raw = format!("{CONFIG_SCHEMA_HEADER}\n");
    }

    raw = upsert_toml_value(
        &raw,
        None,
        "model",
        preferences
            .get("model")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(stringify_toml_string),
    );
    raw = upsert_toml_value(
        &raw,
        None,
        "personality",
        preferences
            .get("personality")
            .and_then(Value::as_str)
            .filter(|value| matches!(*value, "none" | "friendly" | "pragmatic"))
            .map(stringify_toml_string),
    );
    raw = upsert_toml_value(
        &raw,
        None,
        "approval_policy",
        preferences
            .get("approvalPolicy")
            .and_then(Value::as_str)
            .map(stringify_toml_string),
    );
    raw = upsert_toml_value(
        &raw,
        None,
        "sandbox_mode",
        preferences
            .get("sandboxMode")
            .and_then(Value::as_str)
            .map(stringify_toml_string),
    );
    raw = upsert_toml_value(
        &raw,
        None,
        "service_tier",
        preferences
            .get("speed")
            .and_then(Value::as_str)
            .filter(|value| *value == "fast" || *value == "flex")
            .map(stringify_toml_string),
    );

    let effort_key = if preferences.get("mode").and_then(Value::as_str) == Some("plan") {
        "plan_mode_reasoning_effort"
    } else {
        "model_reasoning_effort"
    };
    raw = upsert_toml_value(
        &raw,
        None,
        effort_key,
        preferences
            .get("effort")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(stringify_toml_string),
    );
    raw = upsert_toml_value(
        &raw,
        Some("sandbox_workspace_write"),
        "network_access",
        Some(
            if preferences
                .get("networkAccess")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                "true"
            } else {
                "false"
            }
            .to_string(),
        ),
    );

    tokio_fs::write(&file_path, raw)
        .await
        .context("failed to write config.toml")
}

async fn resolve_allowed_directory(state: &AppState, requested_path: &str) -> ApiResult<String> {
    if requested_path.trim().is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "A working directory is required.",
        ));
    }

    let candidate = resolve_input_path(&state.config.project_root, requested_path);
    let resolved = tokio_fs::canonicalize(&candidate).await.map_err(|_| {
        api_error(
            StatusCode::BAD_REQUEST,
            "The selected working directory does not exist.",
        )
    })?;
    let metadata = tokio_fs::metadata(&resolved).await.map_err(|_| {
        api_error(
            StatusCode::BAD_REQUEST,
            "The selected working directory is invalid.",
        )
    })?;
    if !metadata.is_dir() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "The selected working directory must be a directory.",
        ));
    }

    let allowed_roots = resolved_allowed_roots(&state.config).await;
    if !allowed_roots
        .iter()
        .any(|root| path_is_within(root, &resolved))
    {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "The selected working directory is outside the allowed roots.",
        ));
    }

    Ok(resolved.display().to_string())
}

async fn normalize_git_repo_path(state: &AppState, git_repo_path: &Value) -> ApiResult<Value> {
    let Some(raw_path) = git_repo_path
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(Value::Null);
    };

    let candidate = resolve_input_path(&state.config.project_root, raw_path);
    let resolved = tokio_fs::canonicalize(&candidate).await.map_err(|_| {
        api_error(
            StatusCode::BAD_REQUEST,
            "The selected repository path does not exist.",
        )
    })?;
    let allowed_roots = resolved_allowed_roots(&state.config).await;
    if !allowed_roots
        .iter()
        .any(|root| path_is_within(root, &resolved))
    {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "The selected repository path is outside the allowed roots.",
        ));
    }

    Ok(Value::String(resolved.display().to_string()))
}

async fn resolve_git_repo_root(state: &AppState, repo_path: &str) -> ApiResult<String> {
    let normalized = normalize_git_repo_path(state, &Value::String(repo_path.to_string())).await?;
    let resolved_repo_path = normalized
        .as_str()
        .ok_or_else(|| {
            api_error(
                StatusCode::BAD_REQUEST,
                "The selected repository path is invalid.",
            )
        })?
        .to_string();

    let output = run_command_with_timeout(
        "git",
        vec![
            "-C".to_string(),
            resolved_repo_path.clone(),
            "rev-parse".to_string(),
            "--show-toplevel".to_string(),
        ],
        Duration::from_secs(10),
    )
    .await
    .map_err(|error| api_error(StatusCode::BAD_REQUEST, error.to_string()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            if stderr.is_empty() {
                "The selected path is not inside a Git repository.".to_string()
            } else {
                stderr
            },
        ));
    }

    let repo_root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if repo_root.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "The selected repository path is not inside a Git repository.",
        ));
    }
    Ok(repo_root)
}

async fn resolve_git_worktree_path(state: &AppState, worktree_path: &str) -> ApiResult<String> {
    if worktree_path.trim().is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "worktreePath is required.",
        ));
    }

    let candidate = resolve_input_path(&state.config.project_root, worktree_path);
    let existing = tokio_fs::canonicalize(&candidate).await.ok();
    let path_to_check = existing.unwrap_or_else(|| candidate.clone());
    let allowed_roots = resolved_allowed_roots(&state.config).await;
    if !allowed_roots
        .iter()
        .any(|root| path_is_within(root, &path_to_check))
    {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "The selected worktree path is outside the allowed roots.",
        ));
    }

    Ok(candidate.display().to_string())
}

async fn run_git_text_payload(
    _state: &AppState,
    repo_path: &str,
    args: Vec<String>,
) -> ApiResult<String> {
    let output = run_git_output_payload(repo_path, args.clone(), Duration::from_secs(20)).await?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

async fn run_git_output_payload(
    repo_path: &str,
    args: Vec<String>,
    timeout: Duration,
) -> ApiResult<std::process::Output> {
    let mut command_args = vec!["-C".to_string(), repo_path.to_string()];
    command_args.extend(args.clone());
    let output = run_command_with_timeout("git", command_args, timeout)
        .await
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error.to_string()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            if stderr.is_empty() {
                format!(
                    "git {} failed.",
                    args.first().map(String::as_str).unwrap_or("command")
                )
            } else {
                stderr
            },
        ));
    }
    Ok(output)
}

fn git_skip_dir(name: &str) -> bool {
    matches!(
        name,
        ".git" | "node_modules" | ".svelte-kit" | "build" | "dist" | ".next" | "coverage"
    )
}

async fn has_git_marker(path: &Path) -> bool {
    tokio_fs::metadata(path.join(".git"))
        .await
        .map(|metadata| metadata.is_dir() || metadata.is_file())
        .unwrap_or(false)
}

async fn list_git_child_directories(path: &Path) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    let Ok(mut entries) = tokio_fs::read_dir(path).await else {
        return directories;
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if git_skip_dir(name) {
            continue;
        }
        if entry
            .file_type()
            .await
            .map(|file_type| file_type.is_dir())
            .unwrap_or(false)
        {
            directories.push(entry.path());
        }
    }

    directories
}

async fn build_git_repository_payload(
    _state: &AppState,
    repo_path: &Path,
    allowed_roots: &[PathBuf],
) -> ApiResult<Value> {
    let normalized_repo_path = real_path_safe(repo_path).await;
    let Some(root_path) = allowed_roots
        .iter()
        .find(|candidate| path_is_within(candidate, &normalized_repo_path))
    else {
        return Err(api_error(
            StatusCode::NOT_FOUND,
            "The selected repository was not found within allowed roots.",
        ));
    };

    let current_branch = run_command_with_timeout(
        "git",
        vec![
            "-C".to_string(),
            normalized_repo_path.display().to_string(),
            "branch".to_string(),
            "--show-current".to_string(),
        ],
        Duration::from_secs(5),
    )
    .await
    .ok()
    .filter(|output| output.status.success())
    .and_then(|output| {
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!branch.is_empty()).then_some(branch)
    });
    let relative_path = normalized_repo_path
        .strip_prefix(root_path)
        .ok()
        .and_then(|value| {
            let text = value.display().to_string();
            (!text.is_empty()).then_some(text)
        })
        .unwrap_or_else(|| ".".to_string());

    Ok(json!({
        "path": normalized_repo_path.display().to_string(),
        "name": normalized_repo_path
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| normalized_repo_path.as_os_str().to_str().unwrap_or(".")),
        "rootPath": root_path.display().to_string(),
        "relativePath": relative_path,
        "currentBranch": current_branch
    }))
}

async fn invalidate_git_repository_cache(state: &AppState) {
    *state.git_repository_cache.lock().await = None;
}

fn parse_git_worktrees_payload(repo_path: &str, output: &str) -> Vec<Value> {
    let mut worktrees = Vec::new();
    let mut current: Option<serde_json::Map<String, Value>> = None;

    for raw_line in output.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.is_empty() {
            if let Some(entry) = current.take() {
                worktrees.push(Value::Object(entry));
            }
            continue;
        }

        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(entry) = current.take() {
                worktrees.push(Value::Object(entry));
            }
            let mut entry = serde_json::Map::new();
            entry.insert("path".to_string(), Value::String(path.to_string()));
            entry.insert("branch".to_string(), Value::Null);
            entry.insert("head".to_string(), Value::Null);
            entry.insert("bare".to_string(), Value::Bool(false));
            entry.insert("detached".to_string(), Value::Bool(false));
            entry.insert("locked".to_string(), Value::Bool(false));
            entry.insert("prunable".to_string(), Value::Bool(false));
            entry.insert("current".to_string(), Value::Bool(path == repo_path));
            current = Some(entry);
            continue;
        }

        let Some(entry) = current.as_mut() else {
            continue;
        };
        if let Some(head) = line.strip_prefix("HEAD ") {
            entry.insert("head".to_string(), Value::String(head.to_string()));
        } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
            entry.insert("branch".to_string(), Value::String(branch.to_string()));
        } else if line == "bare" {
            entry.insert("bare".to_string(), Value::Bool(true));
        } else if line == "detached" {
            entry.insert("detached".to_string(), Value::Bool(true));
        } else if line.starts_with("locked") {
            entry.insert("locked".to_string(), Value::Bool(true));
        } else if line.starts_with("prunable") {
            entry.insert("prunable".to_string(), Value::Bool(true));
        }
    }

    if let Some(entry) = current.take() {
        worktrees.push(Value::Object(entry));
    }

    worktrees
}

async fn list_git_worktrees_payload(state: &AppState, repo_path: &str) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let output = run_git_text_payload(
        state,
        &repo_root,
        vec![
            "worktree".to_string(),
            "list".to_string(),
            "--porcelain".to_string(),
        ],
    )
    .await?;
    Ok(json!({
        "repoPath": repo_root,
        "worktrees": parse_git_worktrees_payload(&repo_root, &output)
    }))
}

async fn create_git_worktree_payload(
    state: &AppState,
    repo_path: &str,
    worktree_path: &str,
    branch_name: Option<&str>,
    create_branch: bool,
    detach: bool,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let resolved_worktree_path = resolve_git_worktree_path(state, worktree_path).await?;
    let trimmed_branch_name = branch_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if !detach && trimmed_branch_name.is_none() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Provide a branch name or create a detached worktree.",
        ));
    }

    let mut args = vec!["worktree".to_string(), "add".to_string()];
    if detach {
        args.push("--detach".to_string());
    } else if create_branch {
        let Some(branch_name) = trimmed_branch_name.clone() else {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "Provide a branch name or create a detached worktree.",
            ));
        };
        args.push("-b".to_string());
        args.push(branch_name);
    }
    args.push(resolved_worktree_path.clone());
    if !detach && !create_branch {
        if let Some(branch_name) = trimmed_branch_name {
            args.push(branch_name);
        }
    }

    run_git_text_payload(state, &repo_root, args).await?;
    invalidate_git_repository_cache(state).await;
    let allowed_roots = resolved_allowed_roots(&state.config).await;
    if let Ok(repository) =
        build_git_repository_payload(state, Path::new(&resolved_worktree_path), &allowed_roots)
            .await
    {
        state
            .pinned_git_repositories
            .lock()
            .await
            .insert(resolved_worktree_path.clone(), repository);
    }
    list_git_worktrees_payload(state, &repo_root).await
}

async fn remove_git_worktree_payload(
    state: &AppState,
    repo_path: &str,
    worktree_path: &str,
    force: bool,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let resolved_worktree_path = resolve_git_worktree_path(state, worktree_path).await?;
    let mut args = vec!["worktree".to_string(), "remove".to_string()];
    if force {
        args.push("--force".to_string());
    }
    args.push(resolved_worktree_path.clone());
    run_git_text_payload(state, &repo_root, args).await?;
    invalidate_git_repository_cache(state).await;
    state
        .pinned_git_repositories
        .lock()
        .await
        .remove(&resolved_worktree_path);
    list_git_worktrees_payload(state, &repo_root).await
}

async fn list_git_repositories_payload(state: &AppState, force_refresh: bool) -> ApiResult<Value> {
    if !force_refresh {
        if let Some(cached) = state
            .git_repository_cache
            .lock()
            .await
            .clone()
            .filter(|cached| cached.created_at.elapsed() < GIT_REPOSITORY_CACHE_TTL)
        {
            let pinned = state
                .pinned_git_repositories
                .lock()
                .await
                .values()
                .cloned()
                .collect::<Vec<_>>();
            let mut repositories_by_path = HashMap::new();
            for repository in cached.repositories.into_iter().chain(pinned.into_iter()) {
                if let Some(path) = repository.get("path").and_then(Value::as_str) {
                    repositories_by_path.insert(path.to_string(), repository);
                }
            }
            let mut repositories = repositories_by_path.into_values().collect::<Vec<_>>();
            repositories.sort_by(|left, right| {
                left.get("relativePath")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .cmp(
                        right
                            .get("relativePath")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    )
                    .then_with(|| {
                        left.get("path")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .cmp(
                                right
                                    .get("path")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default(),
                            )
                    })
            });
            return Ok(json!({ "repositories": repositories }));
        }
    }

    let allowed_roots = resolved_allowed_roots(&state.config).await;
    let mut repositories = Vec::new();
    for root in &allowed_roots {
        let mut queue = VecDeque::from([(root.clone(), 0_u64)]);
        while let Some((current_path, depth)) = queue.pop_front() {
            if has_git_marker(&current_path).await {
                if let Ok(repository) =
                    build_git_repository_payload(state, &current_path, &allowed_roots).await
                {
                    repositories.push(repository);
                }
            }
            if depth >= state.config.git_discovery_depth {
                continue;
            }
            for child in list_git_child_directories(&current_path).await {
                queue.push_back((child, depth + 1));
            }
        }
    }

    let pinned = state
        .pinned_git_repositories
        .lock()
        .await
        .values()
        .cloned()
        .collect::<Vec<_>>();
    let mut repositories_by_path = HashMap::new();
    for repository in repositories.into_iter().chain(pinned.into_iter()) {
        if let Some(path) = repository.get("path").and_then(Value::as_str) {
            repositories_by_path.insert(path.to_string(), repository);
        }
    }
    let mut repositories = repositories_by_path.into_values().collect::<Vec<_>>();
    repositories.sort_by(|left, right| {
        left.get("relativePath")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(
                right
                    .get("relativePath")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
            .then_with(|| {
                left.get("path")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .cmp(
                        right
                            .get("path")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    )
            })
    });
    *state.git_repository_cache.lock().await = Some(CachedGitRepositories {
        created_at: Instant::now(),
        repositories: repositories.clone(),
    });
    Ok(json!({ "repositories": repositories }))
}

async fn get_git_status_payload(state: &AppState, repo_path: &str) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let allowed_roots = resolved_allowed_roots(&state.config).await;
    let repository =
        build_git_repository_payload(state, Path::new(&repo_root), &allowed_roots).await?;
    let output = run_git_text_payload(
        state,
        &repo_root,
        vec![
            "status".to_string(),
            "--porcelain=v1".to_string(),
            "--branch".to_string(),
        ],
    )
    .await?;
    let lines = output
        .lines()
        .map(|line| line.trim_end_matches('\r').to_string())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let header = lines
        .iter()
        .find(|line| line.starts_with("## "))
        .cloned()
        .unwrap_or_else(|| "## HEAD".to_string());
    let summary = header.trim_start_matches("## ").to_string();
    let (branch_part, tracking_part) = summary
        .split_once("...")
        .map(|(left, right)| (left.trim().to_string(), right.to_string()))
        .unwrap_or_else(|| (summary.trim().to_string(), String::new()));
    let branch = if branch_part == "HEAD (no branch)" {
        None
    } else {
        let trimmed = branch_part.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    };
    let extract_count = |needle: &str| -> u64 {
        tracking_part
            .split(needle)
            .nth(1)
            .map(str::trim_start)
            .and_then(|value| {
                value
                    .chars()
                    .take_while(|ch| ch.is_ascii_digit())
                    .collect::<String>()
                    .parse::<u64>()
                    .ok()
            })
            .unwrap_or(0)
    };
    let ahead = extract_count("ahead ");
    let behind = extract_count("behind ");

    let files = lines
        .iter()
        .filter(|line| !line.starts_with("## "))
        .map(|line| {
            let staged_code = line.chars().next().unwrap_or(' ');
            let unstaged_code = line.chars().nth(1).unwrap_or(' ');
            let raw_path = line.get(3..).unwrap_or_default();
            let (original_path, file_path) = raw_path
                .split_once(" -> ")
                .map(|(left, right)| (Some(left.to_string()), right.to_string()))
                .unwrap_or_else(|| (None, raw_path.to_string()));
            let map_code = |code: char| match code {
                'M' => "modified",
                'A' => "added",
                'D' => "deleted",
                'R' => "renamed",
                'C' => "copied",
                'U' => "unmerged",
                '?' => "untracked",
                '!' => "ignored",
                _ => "clean",
            };
            json!({
                "path": file_path,
                "originalPath": original_path,
                "stagedCode": staged_code.to_string(),
                "unstagedCode": unstaged_code.to_string(),
                "stagedLabel": map_code(staged_code),
                "unstagedLabel": map_code(unstaged_code),
                "hasStagedChanges": staged_code != ' ' && staged_code != '?',
                "hasUnstagedChanges": unstaged_code != ' ' && unstaged_code != '?',
                "isUntracked": staged_code == '?' && unstaged_code == '?'
            })
        })
        .collect::<Vec<_>>();

    let branches_output = run_git_text_payload(
        state,
        &repo_root,
        vec![
            "for-each-ref".to_string(),
            "refs/heads".to_string(),
            "--format=%(refname:short)\t%(HEAD)\t%(upstream:short)".to_string(),
        ],
    )
    .await?;
    let branches = branches_output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut parts = line.split('\t');
            let name = parts.next().unwrap_or_default();
            let current = parts.next().unwrap_or_default();
            let upstream = parts.next().unwrap_or_default();
            json!({
                "name": name,
                "current": current == "*",
                "upstream": if upstream.trim().is_empty() { Value::Null } else { Value::String(upstream.to_string()) }
            })
        })
        .collect::<Vec<_>>();

    let commits_output = run_git_text_payload(
        state,
        &repo_root,
        vec![
            "log".to_string(),
            "--max-count=12".to_string(),
            "--pretty=format:%H%x09%h%x09%an%x09%aI%x09%s".to_string(),
        ],
    )
    .await?;
    let commits = commits_output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut parts = line.split('\t');
            json!({
                "hash": parts.next().unwrap_or_default(),
                "shortHash": parts.next().unwrap_or_default(),
                "author": parts.next().unwrap_or_default(),
                "authoredAt": parts.next().unwrap_or_default(),
                "subject": parts.next().unwrap_or_default()
            })
        })
        .collect::<Vec<_>>();

    Ok(json!({
        "repo": {
            "path": repository.get("path").cloned().unwrap_or(Value::String(repo_root.clone())),
            "name": repository.get("name").cloned().unwrap_or(Value::Null),
            "rootPath": repository.get("rootPath").cloned().unwrap_or(Value::Null),
            "relativePath": repository.get("relativePath").cloned().unwrap_or(Value::Null),
            "currentBranch": branch.clone()
        },
        "branch": branch,
        "ahead": ahead,
        "behind": behind,
        "clean": files.is_empty(),
        "files": files,
        "branches": branches,
        "commits": commits
    }))
}

async fn get_git_file_payload(
    state: &AppState,
    repo_path: &str,
    file_path: &str,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let status_payload = get_git_status_payload(state, &repo_root).await?;
    let status = status_payload
        .get("files")
        .and_then(Value::as_array)
        .and_then(|files| {
            files
                .iter()
                .find(|entry| entry.get("path").and_then(Value::as_str) == Some(file_path))
        })
        .cloned()
        .unwrap_or(Value::Null);

    let candidate_path = resolve_git_repository_file_path(&repo_root, file_path).await?;
    let modified_bytes = tokio_fs::read(&candidate_path).await.unwrap_or_default();
    let modified_is_binary = modified_bytes.contains(&0);
    let modified_content = if modified_is_binary {
        String::new()
    } else {
        String::from_utf8_lossy(&modified_bytes).to_string()
    };

    let head_path = status
        .get("originalPath")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(file_path);
    let head_output = run_command_with_timeout(
        "git",
        vec![
            "-C".to_string(),
            repo_root.clone(),
            "show".to_string(),
            format!("HEAD:{}", head_path.replace('\\', "/")),
        ],
        Duration::from_secs(20),
    )
    .await
    .ok();
    let (original_content, original_is_binary) = if let Some(output) = head_output {
        if output.status.success() {
            let is_binary = output.stdout.contains(&0);
            (
                if is_binary {
                    String::new()
                } else {
                    String::from_utf8_lossy(&output.stdout).to_string()
                },
                is_binary,
            )
        } else {
            (String::new(), false)
        }
    } else {
        (String::new(), false)
    };

    let language = match Path::new(file_path)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .as_deref()
    {
        Some("ts" | "tsx") => "typescript",
        Some("js" | "mjs" | "cjs" | "jsx") => "javascript",
        Some("svelte" | "html") => "html",
        Some("json") => "json",
        Some("css") => "css",
        Some("scss") => "scss",
        Some("md") => "markdown",
        Some("yml" | "yaml") => "yaml",
        Some("sh") => "shell",
        Some("rs") => "rust",
        Some("py") => "python",
        Some("go") => "go",
        Some("java") => "java",
        Some("kt") => "kotlin",
        Some("swift") => "swift",
        _ => "plaintext",
    };

    Ok(json!({
        "repoPath": repo_root,
        "filePath": file_path,
        "originalPath": status.get("originalPath").cloned().unwrap_or(Value::Null),
        "originalContent": original_content,
        "modifiedContent": modified_content,
        "language": language,
        "isBinary": original_is_binary || modified_is_binary,
        "status": status
    }))
}

async fn resolve_git_repository_file_path(repo_root: &str, file_path: &str) -> ApiResult<PathBuf> {
    let repo_root_path = PathBuf::from(repo_root);
    let candidate_path = normalize_path(repo_root_path.join(file_path));
    let existing_path = tokio_fs::canonicalize(&candidate_path)
        .await
        .unwrap_or_else(|_| candidate_path.clone());
    if !path_is_within(&repo_root_path, &existing_path) {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "The selected file is outside the repository root.",
        ));
    }
    Ok(candidate_path)
}

async fn save_git_file_payload(
    state: &AppState,
    repo_path: &str,
    file_path: &str,
    content: &str,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let target_path = resolve_git_repository_file_path(&repo_root, file_path).await?;
    if let Some(parent) = target_path.parent() {
        tokio_fs::create_dir_all(parent)
            .await
            .map_err(|error| api_error(StatusCode::BAD_REQUEST, error.to_string()))?;
    }
    tokio_fs::write(&target_path, content)
        .await
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error.to_string()))?;
    get_git_file_payload(state, &repo_root, file_path).await
}

async fn stage_git_changes_payload(
    state: &AppState,
    repo_path: &str,
    file_path: Option<&str>,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let args = if let Some(file_path) = file_path.filter(|value| !value.trim().is_empty()) {
        vec!["add".to_string(), "--".to_string(), file_path.to_string()]
    } else {
        vec!["add".to_string(), "-A".to_string()]
    };
    run_git_text_payload(state, &repo_root, args).await?;
    get_git_status_payload(state, &repo_root).await
}

async fn unstage_git_changes_payload(
    state: &AppState,
    repo_path: &str,
    file_path: Option<&str>,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let args = if let Some(file_path) = file_path.filter(|value| !value.trim().is_empty()) {
        vec![
            "restore".to_string(),
            "--staged".to_string(),
            "--".to_string(),
            file_path.to_string(),
        ]
    } else {
        vec![
            "restore".to_string(),
            "--staged".to_string(),
            ".".to_string(),
        ]
    };
    run_git_text_payload(state, &repo_root, args).await?;
    get_git_status_payload(state, &repo_root).await
}

async fn fetch_git_repository_payload(state: &AppState, repo_path: &str) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    run_git_text_payload(
        state,
        &repo_root,
        vec![
            "fetch".to_string(),
            "--all".to_string(),
            "--prune".to_string(),
        ],
    )
    .await?;
    invalidate_git_repository_cache(state).await;
    get_git_status_payload(state, &repo_root).await
}

async fn pull_git_repository_payload(state: &AppState, repo_path: &str) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    run_git_text_payload(
        state,
        &repo_root,
        vec!["pull".to_string(), "--ff-only".to_string()],
    )
    .await?;
    invalidate_git_repository_cache(state).await;
    get_git_status_payload(state, &repo_root).await
}

async fn commit_git_changes_payload(
    state: &AppState,
    repo_path: &str,
    message: &str,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let trimmed_message = message.trim();
    if trimmed_message.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Commit message is required.",
        ));
    }
    run_git_text_payload(
        state,
        &repo_root,
        vec![
            "commit".to_string(),
            "-m".to_string(),
            trimmed_message.to_string(),
        ],
    )
    .await?;
    get_git_status_payload(state, &repo_root).await
}

async fn checkout_git_branch_payload(
    state: &AppState,
    repo_path: &str,
    branch_name: &str,
    create: bool,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let trimmed_branch_name = branch_name.trim();
    if trimmed_branch_name.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "branchName is required.",
        ));
    }
    let args = if create {
        vec![
            "switch".to_string(),
            "-c".to_string(),
            trimmed_branch_name.to_string(),
        ]
    } else {
        vec!["switch".to_string(), trimmed_branch_name.to_string()]
    };
    run_git_text_payload(state, &repo_root, args).await?;
    invalidate_git_repository_cache(state).await;
    get_git_status_payload(state, &repo_root).await
}

async fn get_git_commit_diff_payload(
    state: &AppState,
    repo_path: &str,
    commit_hash: &str,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let normalized_commit_hash = commit_hash.trim();
    if normalized_commit_hash.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "commitHash is required.",
        ));
    }
    let diff = run_git_text_payload(
        state,
        &repo_root,
        vec![
            "show".to_string(),
            "--format=".to_string(),
            "--find-renames".to_string(),
            "--find-copies".to_string(),
            "--no-ext-diff".to_string(),
            normalized_commit_hash.to_string(),
        ],
    )
    .await?;
    Ok(json!({
        "repoPath": repo_root,
        "commitHash": normalized_commit_hash,
        "diff": diff
    }))
}

async fn resolve_git_file_from_absolute_path_payload(
    state: &AppState,
    file_path: &str,
) -> ApiResult<Value> {
    if file_path.trim().is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "filePath is required."));
    }

    let normalized = real_path_safe(Path::new(file_path)).await;
    let target_metadata = tokio_fs::metadata(&normalized).await.ok();
    let repositories = list_git_repositories_payload(state, false)
        .await?
        .get("repositories")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut repositories = repositories;
    repositories.sort_by(|left, right| {
        right
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .len()
            .cmp(
                &left
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .len(),
            )
    });
    let allowed_roots = resolved_allowed_roots(&state.config).await;

    let mut resolved_repository = repositories.into_iter().find(|repository| {
        repository
            .get("path")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .is_some_and(|candidate| path_is_within(&candidate, &normalized))
    });

    if resolved_repository.is_none() {
        let mut current_path = if target_metadata
            .as_ref()
            .is_some_and(|metadata| metadata.is_dir())
        {
            normalized.clone()
        } else {
            normalized
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| normalized.clone())
        };

        while allowed_roots
            .iter()
            .any(|root| path_is_within(root, &current_path))
        {
            if has_git_marker(&current_path).await {
                if let Ok(repository) =
                    build_git_repository_payload(state, &current_path, &allowed_roots).await
                {
                    resolved_repository = Some(repository);
                    break;
                }
            }
            let parent = current_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| current_path.clone());
            if parent == current_path {
                break;
            }
            current_path = parent;
        }
    }

    if resolved_repository.is_none() {
        let stats = tokio_fs::metadata(&normalized).await.ok();
        if stats.as_ref().is_some_and(|metadata| metadata.is_dir()) {
            let max_depth = state.config.git_discovery_depth.saturating_add(2).max(3);
            let mut queue = VecDeque::from([(normalized.clone(), 0_u64)]);
            while let Some((current_path, depth)) = queue.pop_front() {
                if depth > 0 && has_git_marker(&current_path).await {
                    if let Ok(repository) =
                        build_git_repository_payload(state, &current_path, &allowed_roots).await
                    {
                        resolved_repository = Some(repository);
                        break;
                    }
                }
                if depth >= max_depth {
                    continue;
                }
                for child in list_git_child_directories(&current_path).await {
                    queue.push_back((child, depth + 1));
                }
            }
        }
    }

    let Some(repository) = resolved_repository else {
        return Err(api_error(
            StatusCode::NOT_FOUND,
            "The selected path could not be mapped to a Git repository within allowed roots.",
        ));
    };

    let repo_path = repository
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            api_error(
                StatusCode::BAD_REQUEST,
                "The selected repository path is invalid.",
            )
        })?
        .to_string();
    let repo_root = PathBuf::from(&repo_path);
    let relative_path = if target_metadata
        .as_ref()
        .is_some_and(|metadata| !metadata.is_dir())
    {
        normalized
            .strip_prefix(&repo_root)
            .ok()
            .and_then(|relative| {
                let text = relative
                    .components()
                    .filter_map(|component| match component {
                        Component::Normal(value) => value.to_str().map(str::to_string),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("/");
                (!text.is_empty()).then_some(text)
            })
    } else {
        None
    };

    Ok(json!({
        "repoPath": repo_path,
        "filePath": relative_path
    }))
}

fn parse_github_remote_payload(remote_name: &str, remote_url: &str) -> Option<Value> {
    let trimmed = remote_url.trim();
    if trimmed.is_empty() {
        return None;
    }

    let (host, owner, raw_name) = if let Some(rest) = trimmed.strip_prefix("git@") {
        let (host, path) = rest.split_once(':')?;
        let mut parts = path.split('/');
        let owner = parts.next()?;
        let raw_name = parts.collect::<Vec<_>>().join("/");
        (host.to_string(), owner.to_string(), raw_name)
    } else {
        let (_, rest) = trimmed.split_once("://")?;
        let rest = rest.strip_prefix("git@").unwrap_or(rest);
        let mut parts = rest.splitn(3, '/');
        let host = parts.next()?.to_string();
        let owner = parts.next()?.to_string();
        let raw_name = parts.next()?.to_string();
        (host, owner, raw_name)
    };

    let name = raw_name
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .to_string();
    if owner.is_empty() || name.is_empty() {
        return None;
    }

    Some(json!({
        "host": host,
        "owner": owner,
        "name": name,
        "remoteName": remote_name,
        "url": format!("https://{host}/{owner}/{name}")
    }))
}

async fn run_gh_text_payload(repo_path: &str, args: Vec<String>) -> ApiResult<String> {
    let mut command = Command::new("gh");
    command
        .args(args)
        .current_dir(repo_path)
        .envs(env::vars())
        .env("GH_PAGER", "cat")
        .env("GH_PROMPT_DISABLED", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("PAGER", "cat")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = command.spawn().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            api_error(
                StatusCode::BAD_REQUEST,
                "GitHub CLI (gh) is not installed on the server.",
            )
        } else {
            api_error(StatusCode::BAD_REQUEST, error.to_string())
        }
    })?;

    let output = tokio::time::timeout(Duration::from_secs(30), child.wait_with_output())
        .await
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "`gh` timed out"))?
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.contains("executable file not found") || stderr.contains("not found") {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "GitHub CLI (gh) is not installed on the server.",
            ));
        }
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            if stderr.is_empty() {
                "gh command failed.".to_string()
            } else {
                stderr
            },
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

async fn resolve_github_repository_payload(state: &AppState, repo_path: &str) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let remote_names = run_git_text_payload(state, &repo_root, vec!["remote".to_string()])
        .await?
        .lines()
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();

    let mut ordered_remote_names = vec!["origin".to_string()];
    ordered_remote_names.extend(
        remote_names
            .into_iter()
            .filter(|entry| entry != "origin")
            .collect::<Vec<_>>(),
    );

    for remote_name in ordered_remote_names {
        let remote_url = run_git_text_payload(
            state,
            &repo_root,
            vec![
                "config".to_string(),
                "--get".to_string(),
                format!("remote.{remote_name}.url"),
            ],
        )
        .await
        .unwrap_or_default();
        if let Some(parsed) = parse_github_remote_payload(&remote_name, &remote_url) {
            return Ok(parsed);
        }
    }

    Err(api_error(
        StatusCode::BAD_REQUEST,
        "No GitHub remote was found for the selected repository.",
    ))
}

fn map_github_pull_request_summary_payload(pull_request: &Value) -> Value {
    let merged_at = pull_request.get("merged_at").and_then(Value::as_str);
    let labels = pull_request
        .get("labels")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("name").and_then(Value::as_str))
                .map(|label| Value::String(label.to_string()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    json!({
        "number": pull_request.get("number").and_then(Value::as_u64).unwrap_or(0),
        "title": pull_request.get("title").and_then(Value::as_str).unwrap_or("Untitled PR"),
        "state": if merged_at.is_some() {
            "merged"
        } else if pull_request.get("state").and_then(Value::as_str) == Some("closed") {
            "closed"
        } else {
            "open"
        },
        "isDraft": pull_request.get("draft").and_then(Value::as_bool).unwrap_or(false),
        "url": pull_request.get("html_url").and_then(Value::as_str).unwrap_or_default(),
        "author": pull_request
            .get("user")
            .and_then(|value| value.get("login"))
            .and_then(Value::as_str)
            .map(Value::from)
            .unwrap_or(Value::Null),
        "authorUrl": pull_request
            .get("user")
            .and_then(|value| value.get("html_url"))
            .and_then(Value::as_str)
            .map(Value::from)
            .unwrap_or(Value::Null),
        "baseRefName": pull_request
            .get("base")
            .and_then(|value| value.get("ref"))
            .and_then(Value::as_str)
            .unwrap_or_default(),
        "headRefName": pull_request
            .get("head")
            .and_then(|value| value.get("ref"))
            .and_then(Value::as_str)
            .unwrap_or_default(),
        "updatedAt": pull_request
            .get("updated_at")
            .and_then(Value::as_str)
            .map(Value::from)
            .unwrap_or(Value::Null),
        "additions": pull_request.get("additions").and_then(Value::as_i64).unwrap_or(0),
        "deletions": pull_request.get("deletions").and_then(Value::as_i64).unwrap_or(0),
        "changedFiles": pull_request.get("changed_files").and_then(Value::as_i64).unwrap_or(0),
        "labels": labels
    })
}

fn map_github_pull_request_file_payload(file: &Value) -> Value {
    json!({
        "path": file.get("filename").and_then(Value::as_str).unwrap_or_default(),
        "previousPath": file
            .get("previous_filename")
            .and_then(Value::as_str)
            .map(Value::from)
            .unwrap_or(Value::Null),
        "status": file.get("status").and_then(Value::as_str).unwrap_or("modified"),
        "additions": file.get("additions").and_then(Value::as_i64).unwrap_or(0),
        "deletions": file.get("deletions").and_then(Value::as_i64).unwrap_or(0),
        "patch": file
            .get("patch")
            .and_then(Value::as_str)
            .map(Value::from)
            .unwrap_or(Value::Null)
    })
}

async fn list_github_pull_requests_payload(
    state: &AppState,
    repo_path: &str,
    pr_state: &str,
    limit: u64,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let repository = resolve_github_repository_payload(state, &repo_root).await?;
    let owner = repository
        .get("owner")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let name = repository
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let normalized_state = match pr_state {
        "closed" | "all" => pr_state,
        _ => "open",
    };
    let normalized_limit = limit.clamp(1, 50);
    let raw = run_gh_text_payload(
        &repo_root,
        vec![
            "api".to_string(),
            format!(
                "repos/{owner}/{name}/pulls?state={}&per_page={normalized_limit}",
                urlencoding::encode(normalized_state)
            ),
        ],
    )
    .await?;
    let pull_requests = serde_json::from_str::<Value>(&raw)
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error.to_string()))?;
    let summaries = pull_requests
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|entry| map_github_pull_request_summary_payload(&entry))
        .collect::<Vec<_>>();

    Ok(json!({
        "repository": repository,
        "pullRequests": summaries
    }))
}

async fn get_github_pull_request_payload(
    state: &AppState,
    repo_path: &str,
    number: u64,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let repository = resolve_github_repository_payload(state, &repo_root).await?;
    let owner = repository
        .get("owner")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let name = repository
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let pull_request_number = number.max(1);
    let pull_request_raw = run_gh_text_payload(
        &repo_root,
        vec![
            "api".to_string(),
            format!("repos/{owner}/{name}/pulls/{pull_request_number}"),
        ],
    )
    .await?;
    let files_raw = run_gh_text_payload(
        &repo_root,
        vec![
            "api".to_string(),
            format!("repos/{owner}/{name}/pulls/{pull_request_number}/files?per_page=100"),
        ],
    )
    .await?;
    let pull_request = serde_json::from_str::<Value>(&pull_request_raw)
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error.to_string()))?;
    let files = serde_json::from_str::<Value>(&files_raw)
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error.to_string()))?;

    let mut detail = map_github_pull_request_summary_payload(&pull_request)
        .as_object()
        .cloned()
        .unwrap_or_default();
    detail.insert(
        "body".to_string(),
        pull_request
            .get("body")
            .and_then(Value::as_str)
            .map(Value::from)
            .unwrap_or_else(|| Value::String(String::new())),
    );
    detail.insert(
        "reviewDecision".to_string(),
        pull_request
            .get("review_decision")
            .and_then(Value::as_str)
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    detail.insert(
        "mergeStateStatus".to_string(),
        pull_request
            .get("mergeable_state")
            .and_then(Value::as_str)
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    detail.insert(
        "commits".to_string(),
        Value::from(
            pull_request
                .get("commits")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        ),
    );
    detail.insert(
        "files".to_string(),
        Value::Array(
            files
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|entry| map_github_pull_request_file_payload(&entry))
                .collect(),
        ),
    );

    Ok(json!({
        "repository": repository,
        "pullRequest": Value::Object(detail)
    }))
}

async fn checkout_github_pull_request_payload(
    state: &AppState,
    repo_path: &str,
    number: u64,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let pull_request_number = number.max(1);
    run_gh_text_payload(
        &repo_root,
        vec![
            "pr".to_string(),
            "checkout".to_string(),
            pull_request_number.to_string(),
        ],
    )
    .await?;
    invalidate_git_repository_cache(state).await;
    get_git_status_payload(state, &repo_root).await
}

async fn normalize_session_preferences_payload(
    state: &AppState,
    profile_id: &str,
    preferences: Value,
) -> ApiResult<Value> {
    let defaults = session_preferences_defaults_payload(state, profile_id)
        .await
        .as_object()
        .cloned()
        .unwrap_or_default();
    let mut next_preferences = defaults;
    if let Some(overrides) = preferences.as_object() {
        for (key, value) in overrides {
            next_preferences.insert(key.clone(), value.clone());
        }
    }

    let cwd = next_preferences
        .get("cwd")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    next_preferences.insert(
        "cwd".to_string(),
        Value::String(resolve_allowed_directory(state, &cwd).await?),
    );
    let normalized_git_repo_path = normalize_git_repo_path(
        state,
        next_preferences.get("gitRepoPath").unwrap_or(&Value::Null),
    )
    .await?;
    next_preferences.insert("gitRepoPath".to_string(), normalized_git_repo_path);
    next_preferences.insert(
        "personality".to_string(),
        Value::String(
            next_preferences
                .get("personality")
                .and_then(Value::as_str)
                .filter(|value| matches!(*value, "none" | "friendly" | "pragmatic"))
                .unwrap_or("pragmatic")
                .to_string(),
        ),
    );

    Ok(Value::Object(next_preferences))
}

fn normalize_session_title_source(prompt: &str) -> String {
    prompt.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_placeholder_thread_name(name: Option<&str>) -> bool {
    name.map(str::trim)
        .is_none_or(|value| value.is_empty() || value == "New thread")
}

fn infer_session_display_title(prompt: &str) -> Option<String> {
    let normalized = normalize_session_title_source(prompt);
    if normalized.is_empty() {
        return None;
    }
    let candidate = normalized
        .chars()
        .take(60)
        .collect::<String>()
        .trim()
        .trim_end_matches(['.', '?', '!'])
        .trim()
        .to_string();
    if candidate.is_empty() {
        None
    } else if normalized.chars().count() > 60 {
        Some(format!("{candidate}..."))
    } else {
        Some(candidate)
    }
}

fn infer_persisted_session_title(prompt: &str) -> Option<String> {
    let normalized = normalize_session_title_source(prompt);
    let title = infer_session_display_title(prompt)?;
    (title != normalized).then_some(title)
}

fn display_thread_name(name: Option<&str>, preview: Option<&str>) -> Option<String> {
    if !is_placeholder_thread_name(name) {
        name.map(str::trim).map(str::to_string)
    } else {
        infer_session_display_title(preview.unwrap_or_default())
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct RolloutRecoveryInfoPayload {
    available: bool,
    issue: Option<String>,
    total_lines: usize,
    recoverable_lines: usize,
    skipped_lines: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RolloutRecoveryPlanPayload {
    info: RolloutRecoveryInfoPayload,
    recovered_content: String,
}

fn normalize_rollout_line(raw_line: &str) -> Option<String> {
    let trimmed = raw_line
        .trim_start_matches('\u{feff}')
        .replace('\0', "")
        .trim()
        .to_string();
    if trimmed.is_empty() {
        return None;
    }

    let mut candidates = vec![trimmed.clone()];
    if let (Some(first_brace), Some(last_brace)) = (trimmed.find('{'), trimmed.rfind('}')) {
        let sliced = trimmed[first_brace..=last_brace].trim().to_string();
        if !sliced.is_empty() && sliced != trimmed {
            candidates.push(sliced);
        }
    }

    for candidate in candidates {
        if let Ok(parsed) = serde_json::from_str::<Value>(&candidate) {
            if let Ok(normalized) = serde_json::to_string(&parsed) {
                return Some(normalized);
            }
        }
    }

    None
}

fn inspect_rollout_recovery_content(buffer: &[u8]) -> RolloutRecoveryPlanPayload {
    let mut issue = std::str::from_utf8(buffer)
        .err()
        .map(|_| "invalidUtf8".to_string());
    let decoded = String::from_utf8_lossy(buffer);
    let mut total_lines = 0_usize;
    let mut recoverable_lines = 0_usize;
    let mut skipped_lines = 0_usize;
    let mut recovered_lines = Vec::new();

    for raw_line in decoded.lines() {
        if raw_line.trim().is_empty() {
            continue;
        }

        total_lines += 1;
        let Some(normalized) = normalize_rollout_line(raw_line) else {
            skipped_lines += 1;
            continue;
        };

        recoverable_lines += 1;
        recovered_lines.push(normalized);
    }

    if issue.is_none() && skipped_lines > 0 {
        issue = Some("invalidJson".to_string());
    }

    RolloutRecoveryPlanPayload {
        info: RolloutRecoveryInfoPayload {
            available: recoverable_lines > 0
                && (issue.as_deref() == Some("invalidUtf8") || skipped_lines > 0),
            issue,
            total_lines,
            recoverable_lines,
            skipped_lines,
        },
        recovered_content: if recovered_lines.is_empty() {
            String::new()
        } else {
            format!("{}\n", recovered_lines.join("\n"))
        },
    }
}

#[derive(Clone, Default)]
struct SessionFilterCriteria {
    pinned_only: bool,
    running_only: bool,
    queued_only: bool,
    highlight: Option<String>,
    tags: Vec<String>,
}

#[derive(Clone, Default)]
struct SessionSummaryUiSnapshot {
    session_meta_by_thread_id: serde_json::Map<String, Value>,
    preferences_by_thread_id: serde_json::Map<String, Value>,
    highlights_by_thread_id: serde_json::Map<String, Value>,
    queue_counts_by_thread_id: HashMap<String, usize>,
}

fn session_filter_from_value(filter: Option<&Value>) -> SessionFilterCriteria {
    let mut tags = filter
        .and_then(|value| value.get("tags"))
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    tags.sort();
    tags.dedup();

    SessionFilterCriteria {
        pinned_only: filter
            .and_then(|value| value.get("pinnedOnly"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        running_only: filter
            .and_then(|value| value.get("runningOnly"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        queued_only: filter
            .and_then(|value| value.get("queuedOnly"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        highlight: filter
            .and_then(|value| value.get("highlight"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| *value == "attention" || *value == "completed")
            .map(str::to_string),
        tags,
    }
}

fn session_filter_from_query(query: Option<&str>) -> SessionFilterCriteria {
    let mut tags = query_param_values(query, "filterTag")
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    tags.sort();
    tags.dedup();

    SessionFilterCriteria {
        pinned_only: query_param_value(query, "filterPinned").as_deref() == Some("true"),
        running_only: query_param_value(query, "filterRunning").as_deref() == Some("true"),
        queued_only: query_param_value(query, "filterQueued").as_deref() == Some("true"),
        highlight: query_param_value(query, "filterHighlight")
            .map(|value| value.trim().to_string())
            .filter(|value| value == "attention" || value == "completed"),
        tags,
    }
}

fn session_sort_priority(status: Option<&str>) -> i32 {
    match status.unwrap_or_default() {
        "running" | "active" => 1,
        _ => 0,
    }
}

fn session_summary_matches_filter(summary: &Value, filter: &SessionFilterCriteria) -> bool {
    if filter.pinned_only
        && !summary
            .get("pinned")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        return false;
    }
    if filter.running_only
        && session_sort_priority(summary.get("status").and_then(Value::as_str)) == 0
    {
        return false;
    }
    if filter.queued_only
        && summary
            .get("queueCount")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            == 0
    {
        return false;
    }
    if let Some(highlight) = &filter.highlight {
        if summary
            .get("highlight")
            .and_then(|value| value.get("kind"))
            .and_then(Value::as_str)
            != Some(highlight.as_str())
        {
            return false;
        }
    }
    if filter.tags.is_empty() {
        return true;
    }

    let session_tags = summary
        .get("tags")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();

    filter
        .tags
        .iter()
        .all(|tag| session_tags.contains(tag.as_str()))
}

fn session_summary_matches_query(summary: &Value, needle: &str) -> bool {
    let haystack = format!(
        "{}\n{}",
        summary
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        summary
            .get("preview")
            .and_then(Value::as_str)
            .unwrap_or_default()
    )
    .to_lowercase();
    haystack.contains(needle)
}

fn sort_session_summaries(summaries: &mut [Value]) {
    summaries.sort_by(|left, right| {
        let pinned_difference = right
            .get("pinned")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            .cmp(&left.get("pinned").and_then(Value::as_bool).unwrap_or(false));
        if pinned_difference != std::cmp::Ordering::Equal {
            return pinned_difference;
        }

        let priority_difference =
            session_sort_priority(right.get("status").and_then(Value::as_str)).cmp(
                &session_sort_priority(left.get("status").and_then(Value::as_str)),
            );
        if priority_difference != std::cmp::Ordering::Equal {
            return priority_difference;
        }

        let updated_difference = right
            .get("updatedAt")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .cmp(&left.get("updatedAt").and_then(Value::as_i64).unwrap_or(0));
        if updated_difference != std::cmp::Ordering::Equal {
            return updated_difference;
        }

        right
            .get("createdAt")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .cmp(&left.get("createdAt").and_then(Value::as_i64).unwrap_or(0))
    });
}

fn session_summary_page(mut summaries: Vec<Value>, cursor: Option<&str>, limit: u64) -> Value {
    let window_size = limit.clamp(1, 200) as usize;
    let start = cursor
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let end = start.saturating_add(window_size).min(summaries.len());
    let next_cursor = (end < summaries.len()).then(|| end.to_string());
    let page = if start < summaries.len() {
        summaries.drain(start..end).collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    json!({
        "sessions": page,
        "nextCursor": next_cursor
    })
}

async fn read_session_summary_ui_snapshot(
    state: &AppState,
    profile_id: &str,
) -> ApiResult<SessionSummaryUiSnapshot> {
    with_ui_state_read(state, profile_id, |ui_state| {
        let queue_counts_by_thread_id = ui_state
            .get("queuesByThreadId")
            .and_then(Value::as_object)
            .map(|queues| {
                queues
                    .iter()
                    .map(|(thread_id, queue)| {
                        (
                            thread_id.clone(),
                            queue
                                .get("items")
                                .and_then(Value::as_array)
                                .map(Vec::len)
                                .unwrap_or(0),
                        )
                    })
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default();

        Ok(SessionSummaryUiSnapshot {
            session_meta_by_thread_id: ui_state
                .get("sessionMetaByThreadId")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
            preferences_by_thread_id: ui_state
                .get("preferencesByThreadId")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
            highlights_by_thread_id: ui_state
                .get("highlightsByThreadId")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
            queue_counts_by_thread_id,
        })
    })
    .await
}

fn build_session_summary_from_thread_payload(
    thread: &Value,
    snapshot: &SessionSummaryUiSnapshot,
    preferences_override: Option<Value>,
) -> ApiResult<Value> {
    let session_id = thread.get("id").and_then(Value::as_str).ok_or_else(|| {
        api_error(
            StatusCode::BAD_GATEWAY,
            "Codex app-server returned a thread without an id.",
        )
    })?;
    let meta = snapshot
        .session_meta_by_thread_id
        .get(session_id)
        .cloned()
        .unwrap_or_else(|| json!({ "pinned": false, "tags": [] }));
    let highlight = snapshot
        .highlights_by_thread_id
        .get(session_id)
        .cloned()
        .unwrap_or(Value::Null);
    let stored_preferences = snapshot
        .preferences_by_thread_id
        .get(session_id)
        .cloned()
        .unwrap_or(Value::Null);
    let preferences = preferences_override
        .filter(|value| !value.is_null())
        .or_else(|| (!stored_preferences.is_null()).then_some(stored_preferences))
        .unwrap_or_else(|| {
            json!({
                "cwd": thread.get("cwd").cloned().unwrap_or(Value::Null)
            })
        });
    let preview = thread
        .get("preview")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    Ok(json!({
        "id": session_id,
        "name": display_thread_name(
            thread.get("name").and_then(Value::as_str),
            Some(preview.as_str())
        ),
        "preview": preview,
        "queueCount": snapshot.queue_counts_by_thread_id.get(session_id).copied().unwrap_or(0),
        "highlight": highlight,
        "pinned": meta.get("pinned").and_then(Value::as_bool).unwrap_or(false),
        "tags": meta.get("tags").cloned().unwrap_or_else(|| json!([])),
        "cwd": thread
            .get("cwd")
            .cloned()
            .unwrap_or_else(|| preferences.get("cwd").cloned().unwrap_or(Value::Null)),
        "archived": thread.get("archived").and_then(Value::as_bool).unwrap_or(false),
        "createdAt": thread.get("createdAt").cloned().unwrap_or_else(|| json!(0)),
        "updatedAt": thread.get("updatedAt").cloned().unwrap_or_else(|| json!(0)),
        "status": normalized_thread_status(thread.get("status")).unwrap_or_else(|| "unknown".to_string()),
        "isSubagent": thread.get("isSubagent").and_then(Value::as_bool).unwrap_or(false),
        "agentNickname": thread.get("agentNickname").cloned().unwrap_or(Value::Null),
        "agentRole": thread.get("agentRole").cloned().unwrap_or(Value::Null),
        "preferences": preferences
    }))
}

async fn list_app_server_threads(
    state: &AppState,
    profile_id: &str,
    archived: bool,
) -> ApiResult<Vec<Value>> {
    let client = app_server_client(state, profile_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to codex app-server: {error}"),
            )
        })?;
    let mut cursor: Option<String> = None;
    let mut threads = Vec::new();

    loop {
        let response = client
            .request(
                "thread/list",
                json!({
                    "limit": 200,
                    "archived": archived,
                    "cursor": cursor.clone()
                }),
            )
            .await
            .map_err(|error| {
                api_error(
                    StatusCode::BAD_GATEWAY,
                    format!("Failed to list sessions: {error}"),
                )
            })?;
        let batch = response
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        threads.extend(batch);
        cursor = response
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if cursor.is_none() {
            break;
        }
    }

    Ok(threads)
}

async fn collect_session_summaries_payload(
    state: &AppState,
    profile_id: &str,
    archived: bool,
    filter: &SessionFilterCriteria,
) -> ApiResult<Vec<Value>> {
    let snapshot = read_session_summary_ui_snapshot(state, profile_id).await?;
    let mut summaries = Vec::new();

    for thread in list_app_server_threads(state, profile_id, archived).await? {
        if thread_is_subagent(&thread) {
            continue;
        }
        let summary = build_session_summary_from_thread_payload(&thread, &snapshot, None)?;
        if session_summary_matches_filter(&summary, filter) {
            summaries.push(summary);
        }
    }

    sort_session_summaries(&mut summaries);
    Ok(summaries)
}

async fn list_sessions_payload(
    state: &AppState,
    profile_id: &str,
    archived: bool,
    cursor: Option<&str>,
    limit: u64,
    filter: &SessionFilterCriteria,
) -> ApiResult<Value> {
    let sessions = collect_session_summaries_payload(state, profile_id, archived, filter).await?;
    Ok(session_summary_page(sessions, cursor, limit))
}

async fn search_sessions_payload(
    state: &AppState,
    profile_id: &str,
    query: &str,
    scope: &str,
    archived: bool,
    cursor: Option<&str>,
    limit: u64,
    filter: &SessionFilterCriteria,
) -> ApiResult<Value> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return list_sessions_payload(state, profile_id, archived, cursor, limit, filter).await;
    }

    let include_full_text = scope == "full";
    let sessions = collect_session_summaries_payload(state, profile_id, archived, filter).await?;
    let mut matched = Vec::new();

    for summary in sessions {
        if session_summary_matches_query(&summary, &needle) {
            matched.push(summary);
            continue;
        }

        if !include_full_text {
            continue;
        }

        let Some(session_id) = summary.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Ok(thread) = read_thread_payload(state, profile_id, session_id, true).await else {
            continue;
        };
        if thread
            .get("turns")
            .cloned()
            .unwrap_or_else(|| json!([]))
            .to_string()
            .to_lowercase()
            .contains(&needle)
        {
            matched.push(summary);
        }
    }
    Ok(session_summary_page(matched, cursor, limit))
}

async fn create_session_payload(
    state: &AppState,
    profile_id: &str,
    preferences: Value,
    name: Option<&str>,
) -> ApiResult<Value> {
    let next_preferences =
        normalize_session_preferences_payload(state, profile_id, preferences).await?;
    let session_preferences = next_preferences.as_object().cloned().ok_or_else(|| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Invalid preferences state.",
        )
    })?;
    let cwd = session_preferences
        .get("cwd")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "A working directory is required."))?;
    let client = app_server_client(state, profile_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to codex app-server: {error}"),
            )
        })?;
    let response = client
        .request(
            "thread/start",
            json!({
                "model": session_preferences.get("model").cloned().unwrap_or(Value::Null),
                "cwd": cwd,
                "approvalPolicy": session_preferences.get("approvalPolicy").cloned().unwrap_or_else(|| json!("on-request")),
                "sandbox": session_preferences.get("sandboxMode").cloned().unwrap_or_else(|| json!("workspace-write")),
                "personality": session_preferences.get("personality").cloned().unwrap_or(Value::Null),
                "serviceTier": match session_preferences.get("speed").and_then(Value::as_str) {
                    Some("fast") => Value::String("fast".to_string()),
                    Some("flex") => Value::String("flex".to_string()),
                    _ => Value::Null
                },
                "experimentalRawEvents": false,
                "persistExtendedHistory": true
            }),
        )
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to create the session: {error}"),
            )
        })?;
    let mut thread = response.get("thread").cloned().ok_or_else(|| {
        api_error(
            StatusCode::BAD_GATEWAY,
            "Codex app-server returned an invalid thread payload.",
        )
    })?;
    let session_id = thread.get("id").and_then(Value::as_str).ok_or_else(|| {
        api_error(
            StatusCode::BAD_GATEWAY,
            "Codex app-server returned a session without an id.",
        )
    })?;

    with_ui_state_write(state, profile_id, |ui_state| {
        let Some(preferences_by_thread_id) = ui_state
            .get_mut("preferencesByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "preferences state is missing",
            ));
        };
        preferences_by_thread_id.insert(session_id.to_string(), next_preferences.clone());
        Ok(())
    })
    .await?;

    let next_name = name
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "New thread");
    if let Some(next_name) = next_name {
        client
            .request(
                "thread/name/set",
                json!({
                    "threadId": session_id,
                    "name": next_name
                }),
            )
            .await
            .map_err(|error| {
                api_error(
                    StatusCode::BAD_GATEWAY,
                    format!("Failed to name the session: {error}"),
                )
            })?;
        if let Some(thread_object) = thread.as_object_mut() {
            thread_object.insert("name".to_string(), Value::String(next_name.to_string()));
        }
    }

    let snapshot = read_session_summary_ui_snapshot(state, profile_id).await?;
    let summary = build_session_summary_from_thread_payload(
        &thread,
        &snapshot,
        Some(next_preferences.clone()),
    )?;
    emit_profile_global_notification(
        state,
        profile_id,
        json!({
            "kind": "notification",
            "method": "codex-webui/sessionSummaryUpdated",
            "params": {
                "session": summary.clone()
            }
        }),
    )
    .await;

    Ok(summary)
}

fn is_unmaterialized_thread_error_message(message: &str) -> bool {
    let lowered = message.to_ascii_lowercase();
    lowered.contains("not materialized yet")
        || lowered.contains("includeturns is unavailable before first user message")
}

fn thread_agent_nickname(thread: &Value) -> Option<String> {
    thread
        .get("agentNickname")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            thread
                .get("agent_nickname")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
        .or_else(|| {
            thread
                .get("source")
                .and_then(|value| value.get("subagent"))
                .and_then(|value| value.get("thread_spawn"))
                .and_then(|value| value.get("agent_nickname"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
}

fn thread_agent_role(thread: &Value) -> Option<String> {
    thread
        .get("agentRole")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            thread
                .get("agent_role")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
        .or_else(|| {
            thread
                .get("source")
                .and_then(|value| value.get("subagent"))
                .and_then(|value| value.get("thread_spawn"))
                .and_then(|value| value.get("agent_role"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
}

fn thread_is_subagent(thread: &Value) -> bool {
    thread
        .get("isSubagent")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || thread
            .get("source")
            .and_then(|value| value.get("subagent"))
            .and_then(Value::as_object)
            .is_some_and(|value| !value.is_empty())
        || thread_agent_nickname(thread).is_some()
        || thread_agent_role(thread).is_some()
}

fn normalize_session_item_payload(item: &Value, turn_id: &str, item_index: usize) -> Value {
    let mut normalized = item.as_object().cloned().unwrap_or_default();
    if normalized
        .get("id")
        .and_then(Value::as_str)
        .is_none_or(|value| value.trim().is_empty())
    {
        normalized.insert(
            "id".to_string(),
            Value::String(format!("{turn_id}:item:{item_index}")),
        );
    }
    if normalized
        .get("type")
        .and_then(Value::as_str)
        .is_none_or(|value| value.trim().is_empty())
    {
        normalized.insert("type".to_string(), Value::String("unknown".to_string()));
    }
    Value::Object(normalized)
}

fn normalize_session_turn_payload(turn: &Value, turn_index: usize) -> Value {
    let mut normalized = turn.as_object().cloned().unwrap_or_default();
    let turn_id = normalized
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("turn-{turn_index}"));
    let items = normalized
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .enumerate()
        .map(|(item_index, item)| normalize_session_item_payload(item, &turn_id, item_index))
        .collect::<Vec<_>>();
    normalized.insert("id".to_string(), Value::String(turn_id));
    normalized.insert("items".to_string(), Value::Array(items));
    normalized.insert(
        "status".to_string(),
        Value::String(
            value_text(normalized.get("status").unwrap_or(&Value::Null))
                .unwrap_or_else(|| "unknown".to_string()),
        ),
    );
    normalized
        .entry("error".to_string())
        .or_insert_with(|| Value::Null);
    normalized
        .entry("startedAt".to_string())
        .or_insert_with(|| Value::Null);
    normalized
        .entry("completedAt".to_string())
        .or_insert_with(|| Value::Null);
    normalized
        .entry("durationMs".to_string())
        .or_insert_with(|| Value::Null);
    normalized.insert("detailState".to_string(), Value::String("full".to_string()));
    normalized.insert("hiddenItemCount".to_string(), Value::from(0));
    Value::Object(normalized)
}

fn normalize_thread_payload(thread: &Value) -> Value {
    let mut normalized = thread.as_object().cloned().unwrap_or_default();
    let turns = normalized
        .get("turns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .enumerate()
        .map(|(turn_index, turn)| normalize_session_turn_payload(turn, turn_index))
        .collect::<Vec<_>>();
    let preview = normalized
        .get("preview")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    normalized.insert(
        "name".to_string(),
        normalized
            .get("name")
            .and_then(Value::as_str)
            .map(|value| Value::String(value.to_string()))
            .unwrap_or(Value::Null),
    );
    normalized.insert("preview".to_string(), Value::String(preview));
    normalized.insert(
        "status".to_string(),
        Value::String(
            normalized_thread_status(normalized.get("status"))
                .unwrap_or_else(|| "unknown".to_string()),
        ),
    );
    normalized.insert("turns".to_string(), Value::Array(turns));
    normalized.insert(
        "isSubagent".to_string(),
        Value::Bool(thread_is_subagent(thread)),
    );
    normalized.insert(
        "agentNickname".to_string(),
        thread_agent_nickname(thread)
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    normalized.insert(
        "agentRole".to_string(),
        thread_agent_role(thread)
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    Value::Object(normalized)
}

fn active_turn_id_from_turns(turns: &[Value]) -> Option<String> {
    turns.iter().find_map(|turn| {
        (turn.get("status").and_then(Value::as_str) == Some("inProgress"))
            .then(|| {
                turn.get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_default()
            })
            .filter(|value| !value.is_empty())
    })
}

async fn list_session_attachments_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<Vec<Value>> {
    let attachments = list_session_attachment_records(state, profile_id, session_id)
        .await
        .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    Ok(attachments
        .iter()
        .map(attachment_payload_from_record)
        .collect())
}

async fn session_pending_requests_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> Vec<Value> {
    let runtime_key = runtime_session_key(
        resolve_runtime_profile_entry(&state.config, profile_id).0,
        session_id,
    );
    let mut requests = state
        .pending_server_requests
        .lock()
        .await
        .get(&runtime_key)
        .map(|entries| {
            entries
                .iter()
                .map(|(request_id, pending)| {
                    json!({
                        "id": request_id,
                        "method": pending.method,
                        "params": pending.params,
                        "createdAt": pending.created_at
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    requests.sort_by(|left, right| {
        right
            .get("createdAt")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(
                left.get("createdAt")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
    });
    requests
}

async fn read_thread_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    include_turns: bool,
) -> ApiResult<Value> {
    let client = app_server_client(state, profile_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to codex app-server: {error}"),
            )
        })?;
    let response = match client
        .request(
            "thread/read",
            json!({
                "threadId": session_id,
                "includeTurns": include_turns
            }),
        )
        .await
    {
        Ok(response) => response,
        Err(error)
            if include_turns && is_unmaterialized_thread_error_message(&error.to_string()) =>
        {
            client
                .request(
                    "thread/read",
                    json!({
                        "threadId": session_id,
                        "includeTurns": false
                    }),
                )
                .await
                .map_err(|fallback_error| {
                    api_error(
                        StatusCode::BAD_GATEWAY,
                        format!("Failed to read the session: {fallback_error}"),
                    )
                })?
        }
        Err(error) => {
            return Err(api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to read the session: {error}"),
            ));
        }
    };
    let thread = response.get("thread").cloned().ok_or_else(|| {
        api_error(
            StatusCode::BAD_GATEWAY,
            "Codex app-server returned an invalid thread payload.",
        )
    })?;
    Ok(normalize_thread_payload(&thread))
}

fn resolve_rollout_path(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    thread: &Value,
) -> Option<PathBuf> {
    let profile = resolve_runtime_profile(&state.config, profile_id);
    let created_at = thread
        .get("createdAt")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let created_at_seconds = if created_at > 10_000_000_000 {
        created_at / 1000
    } else {
        created_at
    };

    if created_at_seconds > 0 {
        if let Ok(base) = time::OffsetDateTime::from_unix_timestamp(created_at_seconds) {
            for offset in [0_i64, -1, 1] {
                let Some(candidate) = base.checked_add(time::Duration::days(offset)) else {
                    continue;
                };
                let date = candidate.date();
                let day_directory = profile
                    .codex_home
                    .join("sessions")
                    .join(date.year().to_string())
                    .join(format!("{:02}", u8::from(date.month())))
                    .join(format!("{:02}", date.day()));
                if let Ok(entries) = fs::read_dir(&day_directory) {
                    for entry in entries.flatten() {
                        if entry
                            .file_name()
                            .to_str()
                            .is_some_and(|name| name.ends_with(&format!("{session_id}.jsonl")))
                        {
                            return Some(entry.path());
                        }
                    }
                }
            }
        }
    }

    let archived_directory = profile.codex_home.join("archived_sessions");
    if let Ok(entries) = fs::read_dir(archived_directory) {
        for entry in entries.flatten() {
            if entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.ends_with(&format!("{session_id}.jsonl")))
            {
                return Some(entry.path());
            }
        }
    }

    None
}

async fn session_detail_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    limit: u64,
) -> ApiResult<Value> {
    let thread = read_thread_payload(state, profile_id, session_id, true).await?;
    let turns = thread
        .get("turns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let total_turns = turns.len();
    let window_size = limit.clamp(1, 200) as usize;
    let start = total_turns.saturating_sub(window_size);
    let visible_turns = turns[start..].to_vec();
    let active_turn_id = state
        .active_turns
        .lock()
        .await
        .get(&runtime_session_key(
            resolve_runtime_profile_entry(&state.config, profile_id).0,
            session_id,
        ))
        .cloned()
        .or_else(|| active_turn_id_from_turns(&turns));
    let preferences = with_ui_state_read(state, profile_id, |ui_state| {
        Ok(ui_state
            .get("preferencesByThreadId")
            .and_then(Value::as_object)
            .and_then(|entries| entries.get(session_id))
            .cloned()
            .unwrap_or_else(|| {
                json!({
                    "cwd": thread.get("cwd").cloned().unwrap_or(Value::Null)
                })
            }))
    })
    .await?;

    Ok(json!({
        "thread": {
            "id": thread.get("id").cloned().unwrap_or_else(|| json!(session_id)),
            "preview": thread.get("preview").cloned().unwrap_or_else(|| json!("")),
            "name": thread.get("name").cloned().unwrap_or(Value::Null),
            "cwd": thread.get("cwd").cloned().unwrap_or(Value::Null),
            "status": thread.get("status").cloned().unwrap_or_else(|| json!("unknown")),
            "createdAt": thread.get("createdAt").cloned().unwrap_or_else(|| json!(0)),
            "updatedAt": thread.get("updatedAt").cloned().unwrap_or_else(|| json!(0)),
            "isSubagent": thread.get("isSubagent").cloned().unwrap_or_else(|| json!(false)),
            "agentNickname": thread.get("agentNickname").cloned().unwrap_or(Value::Null),
            "agentRole": thread.get("agentRole").cloned().unwrap_or(Value::Null),
            "turns": visible_turns
        },
        "preferences": preferences,
        "attachments": list_session_attachments_payload(state, profile_id, session_id).await?,
        "queue": get_session_queue_payload(state, profile_id, session_id).await?,
        "pendingRequests": session_pending_requests_payload(state, profile_id, session_id).await,
        "activeTurnId": active_turn_id,
        "tokenUsage": thread.get("tokenUsage").cloned().unwrap_or(Value::Null),
        "hydration": {
            "state": "complete",
            "loadedTurns": total_turns.saturating_sub(start),
            "totalTurns": total_turns,
            "remainingTurns": start,
            "message": Value::Null,
            "recovery": {
                "available": false,
                "issue": Value::Null,
                "totalLines": Value::Null,
                "recoverableLines": Value::Null,
                "skippedLines": Value::Null
            }
        }
    }))
}

async fn session_older_turns_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    before_turn_id: &str,
    limit: u64,
) -> ApiResult<Value> {
    let thread = read_thread_payload(state, profile_id, session_id, true).await?;
    let turns = thread
        .get("turns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let Some(before_index) = turns
        .iter()
        .position(|turn| turn.get("id").and_then(Value::as_str) == Some(before_turn_id))
    else {
        return Ok(json!({
            "turns": [],
            "loadedTurns": turns.len(),
            "totalTurns": turns.len(),
            "remainingTurns": 0
        }));
    };
    let window_size = limit.clamp(1, 200) as usize;
    let start = before_index.saturating_sub(window_size);
    Ok(json!({
        "turns": turns[start..before_index].to_vec(),
        "loadedTurns": before_index,
        "totalTurns": turns.len(),
        "remainingTurns": start
    }))
}

async fn session_turn_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    turn_id: &str,
) -> ApiResult<Value> {
    let thread = read_thread_payload(state, profile_id, session_id, true).await?;
    let turn = thread
        .get("turns")
        .and_then(Value::as_array)
        .and_then(|turns| {
            turns
                .iter()
                .find(|turn| turn.get("id").and_then(Value::as_str) == Some(turn_id))
        })
        .cloned()
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Turn not found."))?;
    Ok(json!({ "turn": turn }))
}

async fn session_item_detail_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    turn_id: &str,
    item_id: &str,
) -> ApiResult<Value> {
    let turn = session_turn_payload(state, profile_id, session_id, turn_id)
        .await?
        .get("turn")
        .cloned()
        .unwrap_or(Value::Null);
    let mut item = turn
        .get("items")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.get("id").and_then(Value::as_str) == Some(item_id))
        })
        .cloned()
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Transcript item detail not found."))?;
    if let Some(item_object) = item.as_object_mut() {
        item_object.insert(
            "detailState".to_string(),
            Value::String("loaded".to_string()),
        );
    }
    Ok(json!({ "item": item }))
}

async fn search_session_turns_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    query: &str,
    cursor: Option<&str>,
    limit: u64,
) -> ApiResult<Value> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Ok(json!({
            "matches": [],
            "nextCursor": Value::Null,
            "totalMatches": 0
        }));
    }

    let thread = read_thread_payload(state, profile_id, session_id, true).await?;
    let turns = thread
        .get("turns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut matches = Vec::new();

    for (turn_index, turn) in turns.iter().enumerate() {
        let started_at = turn.get("startedAt").and_then(Value::as_i64);
        for item in turn
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let serialized = serde_json::to_string(&item).unwrap_or_default();
            let normalized = serialized.replace("\\n", " ").replace('\n', " ");
            let lowered = normalized.to_lowercase();
            let Some(match_index) = lowered.find(&needle) else {
                continue;
            };
            let normalized_chars = normalized.chars().collect::<Vec<_>>();
            let match_char_index = lowered[..match_index].chars().count();
            let snippet_start = match_char_index.saturating_sub(54);
            let snippet_end =
                (match_char_index + needle.chars().count() + 54).min(normalized_chars.len());
            matches.push(json!({
                "turnId": turn.get("id").cloned().unwrap_or(Value::Null),
                "turnIndex": turn_index,
                "itemId": item.get("id").cloned().unwrap_or(Value::Null),
                "itemType": item.get("type").cloned().unwrap_or(Value::Null),
                "preview": format!(
                    "{}{}{}",
                    if snippet_start > 0 { "..." } else { "" },
                    normalized_chars[snippet_start..snippet_end]
                        .iter()
                        .collect::<String>()
                        .trim(),
                    if snippet_end < normalized_chars.len() { "..." } else { "" }
                ),
                "startedAt": started_at,
                "requiresFullTurn": false,
                "requiresItemDetail": false
            }));
        }
    }

    let window_size = limit.clamp(1, 200) as usize;
    let start = cursor
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let end = start.saturating_add(window_size).min(matches.len());
    Ok(json!({
        "matches": if start < matches.len() { matches[start..end].to_vec() } else { Vec::<Value>::new() },
        "nextCursor": (end < matches.len()).then(|| end.to_string()),
        "totalMatches": matches.len()
    }))
}

async fn emit_session_notification(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    event: Value,
) {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id).0;
    let relay = {
        let relays = state.relays.lock().await;
        relays
            .get(&session_relay_key(resolved_profile_id, session_id))
            .cloned()
    };
    if let Some(relay) = relay {
        let _ = relay.send(event);
    }
}

async fn build_session_summary_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    preferences_override: Option<Value>,
) -> ApiResult<Value> {
    let thread = read_thread_payload(state, profile_id, session_id, false).await?;
    let snapshot = read_session_summary_ui_snapshot(state, profile_id).await?;
    let summary =
        build_session_summary_from_thread_payload(&thread, &snapshot, preferences_override)?;
    if summary.get("id").and_then(Value::as_str) != Some(session_id) {
        return Err(api_error(
            StatusCode::BAD_GATEWAY,
            "Session summary payload returned an unexpected session id.",
        ));
    }
    Ok(summary)
}

async fn emit_session_summary_updated(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    preferences_override: Option<Value>,
) {
    let summary =
        build_session_summary_payload(state, profile_id, session_id, preferences_override).await;
    if let Ok(summary) = summary {
        emit_profile_global_notification(
            state,
            profile_id,
            json!({
                "kind": "notification",
                "method": "codex-webui/sessionSummaryUpdated",
                "params": {
                    "session": summary
                }
            }),
        )
        .await;
    }
}

async fn update_session_organization_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    patch: Value,
) -> ApiResult<Value> {
    let payload = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(session_meta_by_thread_id) = ui_state
            .get_mut("sessionMetaByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "session metadata state is missing",
            ));
        };

        let current = session_meta_by_thread_id
            .get(session_id)
            .cloned()
            .unwrap_or_else(|| json!({ "pinned": false, "tags": [] }));
        let pinned = patch
            .get("pinned")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| {
                current
                    .get("pinned")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            });
        let mut tags = patch
            .get("tags")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| {
                current
                    .get("tags")
                    .and_then(Value::as_array)
                    .map(|entries| {
                        entries
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            });
        tags.sort();
        tags.dedup();

        let meta = json!({
            "pinned": pinned,
            "tags": tags
        });
        if !pinned
            && meta
                .get("tags")
                .and_then(Value::as_array)
                .is_some_and(|items| items.is_empty())
        {
            session_meta_by_thread_id.remove(session_id);
        } else {
            session_meta_by_thread_id.insert(session_id.to_string(), meta.clone());
        }

        Ok(json!({
            "meta": meta,
            "knownTags": known_tags_from_ui_state(ui_state)
        }))
    })
    .await?;

    emit_profile_config_updated(
        state,
        profile_id,
        json!({
            "sessionOrganization": {
                "knownTags": payload.get("knownTags").cloned().unwrap_or_else(|| json!([]))
            }
        }),
    )
    .await;
    emit_session_summary_updated(state, profile_id, session_id, None).await;

    Ok(payload)
}

async fn save_session_preferences_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    preferences: Value,
) -> ApiResult<Value> {
    let next_preferences =
        normalize_session_preferences_payload(state, profile_id, preferences).await?;
    with_ui_state_write(state, profile_id, |ui_state| {
        let Some(preferences_by_thread_id) = ui_state
            .get_mut("preferencesByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "preferences state is missing",
            ));
        };
        preferences_by_thread_id.insert(session_id.to_string(), next_preferences.clone());
        Ok(())
    })
    .await?;
    sync_codex_toml_with_preferences(
        &resolve_runtime_profile(&state.config, profile_id).codex_home,
        &next_preferences,
    )
    .await
    .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;

    emit_session_notification(
        state,
        profile_id,
        session_id,
        json!({
            "kind": "notification",
            "method": "codex-webui/preferencesUpdated",
            "params": {
                "preferences": next_preferences.clone()
            }
        }),
    )
    .await;
    emit_profile_config_updated(
        state,
        profile_id,
        json!({
            "defaults": next_preferences.clone()
        }),
    )
    .await;
    emit_session_summary_updated(
        state,
        profile_id,
        session_id,
        Some(next_preferences.clone()),
    )
    .await;

    Ok(next_preferences)
}

async fn rename_session_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    name: &str,
) -> ApiResult<Value> {
    let trimmed_name = name.trim();
    if trimmed_name.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Session name is required.",
        ));
    }

    app_server_client(state, profile_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to codex app-server: {error}"),
            )
        })?
        .request(
            "thread/name/set",
            json!({
                "threadId": session_id,
                "name": trimmed_name
            }),
        )
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to rename the session: {error}"),
            )
        })?;

    emit_session_notification(
        state,
        profile_id,
        session_id,
        json!({
            "kind": "notification",
            "method": "thread/name/updated",
            "params": {
                "threadId": session_id,
                "threadName": trimmed_name
            }
        }),
    )
    .await;
    emit_session_summary_updated(state, profile_id, session_id, None).await;

    Ok(json!({
        "ok": true,
        "name": trimmed_name
    }))
}

async fn resolve_selected_attachment_records(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    attachment_ids: Option<&Value>,
) -> ApiResult<Vec<StoredAttachmentRecord>> {
    let requested_attachment_ids = string_array_from_value(attachment_ids);
    let requested_attachment_set = requested_attachment_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let attachments = list_session_attachment_records(state, profile_id, session_id)
        .await
        .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;

    if requested_attachment_set.is_empty() {
        return Ok(Vec::new());
    }

    Ok(attachments
        .into_iter()
        .filter(|attachment| requested_attachment_set.contains(attachment.id.as_str()))
        .collect())
}

fn build_turn_input_payload(
    prompt: &str,
    attachments: &[StoredAttachmentRecord],
) -> (Vec<Value>, Vec<String>) {
    let mut additional_readable_roots = Vec::new();
    let mut readable_roots_seen = HashSet::new();
    let mut text_attachment_paths = Vec::new();
    let mut image_attachment_paths = Vec::new();

    for attachment in attachments {
        if let Some(path) = attachment.path.as_deref() {
            let readable_root = Path::new(path)
                .parent()
                .unwrap_or_else(|| Path::new(path))
                .display()
                .to_string();
            if readable_roots_seen.insert(readable_root.clone()) {
                additional_readable_roots.push(readable_root);
            }
            if attachment.kind.as_deref() == Some("image") {
                image_attachment_paths.push(path.to_string());
            } else {
                text_attachment_paths.push(path.to_string());
            }
        }
    }

    let mut input = vec![json!({
        "type": "text",
        "text": if text_attachment_paths.is_empty() {
            prompt.to_string()
        } else {
            format!(
                "{ATTACHMENT_PREAMBLE_START}\n{}\n{ATTACHMENT_PREAMBLE_END}\n\n{prompt}",
                text_attachment_paths.join("\n")
            )
        },
        "text_elements": []
    })];
    for image_path in image_attachment_paths {
        input.push(json!({
            "type": "localImage",
            "path": image_path
        }));
    }

    (input, additional_readable_roots)
}

async fn resolve_active_turn_id_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<Option<String>> {
    let runtime_key = runtime_session_key(
        resolve_runtime_profile_entry(&state.config, profile_id).0,
        session_id,
    );
    if let Some(turn_id) = state.active_turns.lock().await.get(&runtime_key).cloned() {
        return Ok(Some(turn_id));
    }

    let thread = read_thread_payload(state, profile_id, session_id, true).await?;
    let active_turn_id = thread
        .get("turns")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .and_then(active_turn_id_from_turns);
    if let Some(turn_id) = active_turn_id.clone() {
        state.active_turns.lock().await.insert(runtime_key, turn_id);
    }
    Ok(active_turn_id)
}

async fn send_turn_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    prompt: &str,
    attachment_ids: Option<&Value>,
    preferences: Value,
) -> ApiResult<Value> {
    let trimmed_prompt = prompt.trim();
    let attachments =
        resolve_selected_attachment_records(state, profile_id, session_id, attachment_ids).await?;

    if trimmed_prompt.is_empty() && attachments.is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "EMPTY_MESSAGE"));
    }

    cancel_scheduled_shutdown_for_activity(state, profile_id).await;

    let next_preferences =
        normalize_session_preferences_payload(state, profile_id, preferences).await?;
    let cwd = next_preferences
        .get("cwd")
        .and_then(Value::as_str)
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "A working directory is required."))?
        .to_string();
    let thread = read_thread_payload(state, profile_id, session_id, false).await?;
    let should_backfill_title =
        is_placeholder_thread_name(thread.get("name").and_then(Value::as_str));

    with_ui_state_write(state, profile_id, |ui_state| {
        let Some(preferences_by_thread_id) = ui_state
            .get_mut("preferencesByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "preferences state is missing",
            ));
        };
        preferences_by_thread_id.insert(session_id.to_string(), next_preferences.clone());
        Ok(())
    })
    .await?;

    emit_session_notification(
        state,
        profile_id,
        session_id,
        json!({
            "kind": "notification",
            "method": "codex-webui/preferencesUpdated",
            "params": {
                "preferences": next_preferences.clone()
            }
        }),
    )
    .await;

    let client = app_server_client(state, profile_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to codex app-server: {error}"),
            )
        })?;
    if thread.get("status").and_then(Value::as_str) == Some("notLoaded") {
        client
            .request(
                "thread/resume",
                json!({
                    "threadId": session_id,
                    "persistExtendedHistory": true
                }),
            )
            .await
            .map_err(|error| {
                api_error(
                    StatusCode::BAD_GATEWAY,
                    format!("Failed to resume the session before sending: {error}"),
                )
            })?;
    }

    let (input, attachment_readable_roots) = build_turn_input_payload(trimmed_prompt, &attachments);
    let mut readable_roots = vec![cwd.clone()];
    for readable_root in attachment_readable_roots {
        if !readable_roots.contains(&readable_root) {
            readable_roots.push(readable_root);
        }
    }

    let sandbox_mode = next_preferences
        .get("sandboxMode")
        .and_then(Value::as_str)
        .unwrap_or("workspace-write");
    let network_access = next_preferences
        .get("networkAccess")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let read_only_access = json!({
        "type": "restricted",
        "includePlatformDefaults": true,
        "readableRoots": readable_roots
    });
    let sandbox_policy = match sandbox_mode {
        "danger-full-access" => json!({
            "type": "dangerFullAccess"
        }),
        "read-only" => json!({
            "type": "readOnly",
            "access": read_only_access.clone(),
            "networkAccess": network_access
        }),
        _ => json!({
            "type": "workspaceWrite",
            "writableRoots": [cwd],
            "readOnlyAccess": read_only_access.clone(),
            "networkAccess": network_access,
            "excludeTmpdirEnvVar": false,
            "excludeSlashTmp": false
        }),
    };
    let model = next_preferences
        .get("model")
        .cloned()
        .unwrap_or(Value::Null);
    let response = client
        .request(
            "turn/start",
            json!({
                "threadId": session_id,
                "input": input,
                "cwd": next_preferences.get("cwd").cloned().unwrap_or(Value::Null),
                "approvalPolicy": next_preferences.get("approvalPolicy").cloned().unwrap_or_else(|| json!("on-request")),
                "sandboxPolicy": sandbox_policy,
                "model": model.clone(),
                "personality": next_preferences.get("personality").cloned().unwrap_or(Value::Null),
                "serviceTier": match next_preferences.get("speed").and_then(Value::as_str) {
                    Some("fast") => Value::String("fast".to_string()),
                    Some("flex") => Value::String("flex".to_string()),
                    _ => Value::Null
                },
                "effort": if next_preferences.get("mode").and_then(Value::as_str) == Some("plan") {
                    Value::Null
                } else {
                    next_preferences.get("effort").cloned().unwrap_or(Value::Null)
                },
                "collaborationMode": if next_preferences.get("mode").and_then(Value::as_str) == Some("plan") {
                    json!({
                        "mode": "plan",
                        "settings": {
                            "model": model,
                            "reasoning_effort": next_preferences.get("effort").cloned().unwrap_or(Value::Null),
                            "developer_instructions": Value::Null
                        }
                    })
                } else {
                    Value::Null
                }
            }),
        )
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to start the turn: {error}"),
            )
        })?;

    if let Some(turn_id) = response
        .get("turn")
        .and_then(|value| value.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
    {
        let runtime_key = runtime_session_key(
            resolve_runtime_profile_entry(&state.config, profile_id).0,
            session_id,
        );
        state.active_turns.lock().await.insert(runtime_key, turn_id);
    }

    clear_session_draft_payload(state, profile_id, session_id).await?;
    if should_backfill_title {
        if let Some(title) = infer_persisted_session_title(trimmed_prompt) {
            let _ = rename_session_payload(state, profile_id, session_id, &title).await;
        }
    }
    emit_session_summary_updated(
        state,
        profile_id,
        session_id,
        Some(next_preferences.clone()),
    )
    .await;

    Ok(json!({
        "ok": true,
        "turnId": response
            .get("turn")
            .and_then(|value| value.get("id"))
            .cloned()
            .unwrap_or(Value::Null)
    }))
}

async fn steer_turn_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    prompt: &str,
    attachment_ids: Option<&Value>,
) -> ApiResult<Value> {
    let trimmed_prompt = prompt.trim();
    if trimmed_prompt.is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "EMPTY_MESSAGE"));
    }

    let active_turn_id = resolve_active_turn_id_payload(state, profile_id, session_id).await?;
    let Some(active_turn_id) = active_turn_id else {
        return Err(api_error(StatusCode::CONFLICT, "NO_ACTIVE_TURN"));
    };

    let attachments =
        resolve_selected_attachment_records(state, profile_id, session_id, attachment_ids).await?;
    let (input, _) = build_turn_input_payload(trimmed_prompt, &attachments);
    let client = app_server_client(state, profile_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to codex app-server: {error}"),
            )
        })?;
    client
        .request(
            "turn/steer",
            json!({
                "threadId": session_id,
                "expectedTurnId": active_turn_id,
                "input": input
            }),
        )
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to steer the active turn: {error}"),
            )
        })?;

    clear_session_draft_payload(state, profile_id, session_id).await?;
    Ok(json!({
        "ok": true,
        "turnId": active_turn_id
    }))
}

async fn fork_session_payload(
    state: &AppState,
    profile_id: &str,
    source_session_id: &str,
    mode: &str,
    turn_id: Option<&str>,
    message_text: Option<&str>,
) -> ApiResult<Value> {
    let source_thread = read_thread_payload(state, profile_id, source_session_id, true).await?;
    let source_preferences = with_ui_state_read(state, profile_id, |ui_state| {
        Ok(ui_state
            .get("preferencesByThreadId")
            .and_then(Value::as_object)
            .and_then(|entries| entries.get(source_session_id))
            .cloned()
            .unwrap_or_else(|| {
                json!({
                    "cwd": source_thread.get("cwd").cloned().unwrap_or(Value::Null)
                })
            }))
    })
    .await?;
    let preferences =
        normalize_session_preferences_payload(state, profile_id, source_preferences).await?;
    let turns = source_thread
        .get("turns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let anchor_index = turn_id
        .filter(|value| !value.trim().is_empty())
        .and_then(|turn_id| {
            turns
                .iter()
                .position(|turn| turn.get("id").and_then(Value::as_str) == Some(turn_id))
        })
        .or_else(|| (!turns.is_empty()).then_some(turns.len() - 1));
    let visible_turns = anchor_index
        .map(|index| turns[..=index].to_vec())
        .unwrap_or_else(|| turns.clone());

    let strip_attachment_preamble = |value: &str| {
        let trimmed = value.trim();
        let Some(rest) = trimmed.strip_prefix(&format!("{ATTACHMENT_PREAMBLE_START}\n")) else {
            return trimmed.to_string();
        };
        let Some((_, tail)) = rest.split_once(&format!("\n{ATTACHMENT_PREAMBLE_END}")) else {
            return trimmed.to_string();
        };
        tail.trim_start_matches('\n').trim().to_string()
    };

    let mut selected_message_text = message_text
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if selected_message_text.is_none() {
        for turn in visible_turns.iter().rev() {
            let Some(items) = turn.get("items").and_then(Value::as_array) else {
                continue;
            };
            for item in items.iter().rev() {
                if item.get("type").and_then(Value::as_str) != Some("userMessage") {
                    continue;
                }
                let text = strip_attachment_preamble(
                    item.get("text")
                        .and_then(Value::as_str)
                        .or_else(|| item.get("message").and_then(Value::as_str))
                        .unwrap_or_default(),
                );
                if !text.is_empty() {
                    selected_message_text = Some(text);
                    break;
                }
            }
            if selected_message_text.is_some() {
                break;
            }
        }
    }
    if selected_message_text.is_none() {
        let preview = strip_attachment_preamble(
            source_thread
                .get("preview")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
        if !preview.is_empty() {
            selected_message_text = Some(preview);
        }
    }

    let mut draft = selected_message_text
        .as_deref()
        .map(strip_attachment_preamble)
        .unwrap_or_default();
    let source_name = display_thread_name(
        source_thread.get("name").and_then(Value::as_str),
        source_thread.get("preview").and_then(Value::as_str),
    );
    let next_name =
        infer_session_display_title(selected_message_text.as_deref().unwrap_or(draft.as_str()))
            .or_else(|| source_name.clone());

    if mode == "fork" {
        if draft.trim().is_empty() {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "There is no message to fork yet.",
            ));
        }

        let client = app_server_client(state, profile_id)
            .await
            .map_err(|error| {
                api_error(
                    StatusCode::BAD_GATEWAY,
                    format!("Failed to connect to codex app-server: {error}"),
                )
            })?;
        let response = client
            .request(
                "thread/fork",
                json!({
                    "threadId": source_session_id,
                    "model": preferences.get("model").cloned().unwrap_or(Value::Null),
                    "cwd": preferences.get("cwd").cloned().unwrap_or(Value::Null),
                    "approvalPolicy": preferences.get("approvalPolicy").cloned().unwrap_or_else(|| json!("on-request")),
                    "sandbox": preferences.get("sandboxMode").cloned().unwrap_or_else(|| json!("workspace-write")),
                    "serviceTier": match preferences.get("speed").and_then(Value::as_str) {
                        Some("fast") => Value::String("fast".to_string()),
                        Some("flex") => Value::String("flex".to_string()),
                        _ => Value::Null
                    },
                    "persistExtendedHistory": true
                }),
            )
            .await
            .map_err(|error| {
                api_error(
                    StatusCode::BAD_GATEWAY,
                    format!("Failed to fork the session: {error}"),
                )
            })?;
        let mut forked_thread = response.get("thread").cloned().ok_or_else(|| {
            api_error(
                StatusCode::BAD_GATEWAY,
                "Codex app-server returned an invalid fork payload.",
            )
        })?;
        let forked_session_id = forked_thread
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                api_error(
                    StatusCode::BAD_GATEWAY,
                    "Codex app-server returned a forked session without an id.",
                )
            })?
            .to_string();
        let rollback_turns = anchor_index
            .map(|index| turns.len().saturating_sub(index + 1))
            .unwrap_or(0);
        if rollback_turns > 0 {
            let rolled_back = client
                .request(
                    "thread/rollback",
                    json!({
                        "threadId": forked_session_id,
                        "numTurns": rollback_turns
                    }),
                )
                .await
                .map_err(|error| {
                    api_error(
                        StatusCode::BAD_GATEWAY,
                        format!("Failed to roll back the forked session: {error}"),
                    )
                })?;
            if let Some(thread) = rolled_back.get("thread").cloned() {
                forked_thread = thread;
            }
        }

        with_ui_state_write(state, profile_id, |ui_state| {
            let Some(preferences_by_thread_id) = ui_state
                .get_mut("preferencesByThreadId")
                .and_then(Value::as_object_mut)
            else {
                return Err(api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "preferences state is missing",
                ));
            };
            preferences_by_thread_id.insert(forked_session_id.clone(), preferences.clone());
            Ok(())
        })
        .await?;

        if let Some(name) = next_name
            .as_deref()
            .filter(|value| !is_placeholder_thread_name(Some(value)))
        {
            rename_session_payload(state, profile_id, &forked_session_id, name).await?;
            if let Some(thread_object) = forked_thread.as_object_mut() {
                thread_object.insert("name".to_string(), Value::String(name.to_string()));
            }
        }

        let snapshot = read_session_summary_ui_snapshot(state, profile_id).await?;
        let summary = build_session_summary_from_thread_payload(
            &forked_thread,
            &snapshot,
            Some(preferences.clone()),
        )?;
        emit_session_summary_updated(
            state,
            profile_id,
            &forked_session_id,
            Some(preferences.clone()),
        )
        .await;
        return Ok(json!({
            "session": summary,
            "draft": "",
            "mode": "fork"
        }));
    }

    if mode != "handoff" {
        return Err(api_error(StatusCode::BAD_REQUEST, "Unsupported fork mode."));
    }

    let source_name_for_handoff = source_name
        .clone()
        .unwrap_or_else(|| "Source thread".to_string());
    let preview = strip_attachment_preamble(
        source_thread
            .get("preview")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    );
    let mut entries = Vec::new();
    for turn in &visible_turns {
        let Some(items) = turn.get("items").and_then(Value::as_array) else {
            continue;
        };
        for item in items {
            let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
            if item_type != "userMessage" && item_type != "agentMessage" {
                continue;
            }
            let text = strip_attachment_preamble(
                item.get("text")
                    .and_then(Value::as_str)
                    .or_else(|| item.get("message").and_then(Value::as_str))
                    .unwrap_or_default(),
            );
            if !text.is_empty() {
                entries.push((item_type == "userMessage", text));
            }
        }
    }

    let mut sections = vec![format!(
        "Continue this task in a fresh thread.\n\nSource thread: {source_name_for_handoff}\nWorking directory: {}",
        source_thread
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or_default()
    )];
    if !preview.is_empty() {
        sections.push(format!("Current goal:\n{preview}"));
    }
    if let Some(selected_message_text) = selected_message_text
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        sections.push(format!("Focus request:\n{selected_message_text}"));
    }
    if !entries.is_empty() {
        sections.push(format!(
            "Recent context:\n{}",
            entries
                .iter()
                .rev()
                .take(8)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .map(|(is_user, text)| format!(
                    "- {}: {}",
                    if *is_user { "User" } else { "Assistant" },
                    text.split_whitespace().collect::<Vec<_>>().join(" ")
                ))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    sections.push(
        "Continue from this handoff, preserve any existing constraints, and begin with the most sensible next step."
            .to_string(),
    );
    draft = sections
        .into_iter()
        .map(|section| section.trim().to_string())
        .filter(|section| !section.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if draft.trim().is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "There is no thread context to hand off yet.",
        ));
    }

    let handoff_name = format!(
        "{} · Handoff",
        if source_name_for_handoff.trim().is_empty()
            || is_placeholder_thread_name(Some(source_name_for_handoff.as_str()))
        {
            infer_session_display_title(&draft).unwrap_or_else(|| "Thread".to_string())
        } else {
            source_name_for_handoff
        }
    );
    let session =
        create_session_payload(state, profile_id, preferences, Some(&handoff_name)).await?;
    let handoff_session_id = session
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            api_error(
                StatusCode::BAD_GATEWAY,
                "Forked session summary was invalid.",
            )
        })?
        .to_string();
    let saved_draft =
        save_session_draft_payload(state, profile_id, &handoff_session_id, &draft, "message")
            .await?;
    Ok(json!({
        "session": session,
        "draft": saved_draft
            .get("draft")
            .cloned()
            .unwrap_or_else(|| Value::String(draft)),
        "mode": "handoff"
    }))
}

async fn invalidate_session_lists(state: &AppState, profile_id: &str) {
    emit_profile_global_notification(
        state,
        profile_id,
        json!({
            "kind": "notification",
            "method": "codex-webui/sessionListsInvalidated",
            "params": {}
        }),
    )
    .await;
}

async fn archive_session_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<Value> {
    app_server_client(state, profile_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to codex app-server: {error}"),
            )
        })?
        .request(
            "thread/archive",
            json!({
                "threadId": session_id
            }),
        )
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to archive the session: {error}"),
            )
        })?;

    invalidate_session_lists(state, profile_id).await;
    Ok(json!({ "ok": true }))
}

async fn unarchive_session_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<Value> {
    app_server_client(state, profile_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to codex app-server: {error}"),
            )
        })?
        .request(
            "thread/unarchive",
            json!({
                "threadId": session_id
            }),
        )
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to unarchive the session: {error}"),
            )
        })?;

    invalidate_session_lists(state, profile_id).await;
    let session = build_session_summary_payload(state, profile_id, session_id, None).await?;
    Ok(json!({
        "ok": true,
        "session": session
    }))
}

fn sorted_prompt_presets_from_ui_state(ui_state: &Value) -> Vec<Value> {
    let mut prompt_presets = ui_state
        .get("promptPresets")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    prompt_presets.sort_by(|left, right| {
        right
            .get("updatedAt")
            .and_then(Value::as_i64)
            .unwrap_or_default()
            .cmp(
                &left
                    .get("updatedAt")
                    .and_then(Value::as_i64)
                    .unwrap_or_default(),
            )
    });
    prompt_presets
}

fn sorted_automations_from_ui_state(ui_state: &Value) -> Vec<Value> {
    let mut automations = ui_state
        .get("automations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    automations.sort_by(|left, right| {
        right
            .get("updatedAt")
            .and_then(Value::as_i64)
            .unwrap_or_default()
            .cmp(
                &left
                    .get("updatedAt")
                    .and_then(Value::as_i64)
                    .unwrap_or_default(),
            )
    });
    automations
}

fn recent_automation_runs_from_ui_state(ui_state: &Value, limit: usize) -> Vec<Value> {
    let mut automation_runs = ui_state
        .get("automationRuns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    automation_runs.sort_by(|left, right| {
        right
            .get("startedAt")
            .and_then(Value::as_i64)
            .unwrap_or_default()
            .cmp(
                &left
                    .get("startedAt")
                    .and_then(Value::as_i64)
                    .unwrap_or_default(),
            )
    });
    automation_runs.truncate(limit.max(1));
    automation_runs
}

fn automation_timer_key(profile_id: &str, automation_id: &str) -> String {
    format!("profile::{profile_id}::automation::{automation_id}")
}

fn trimmed_json_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn automation_schedule_mode(value: Option<&Value>) -> &'static str {
    match value.and_then(Value::as_str) {
        Some("interval") => "interval",
        _ => "manual",
    }
}

fn automation_target(value: Option<&Value>) -> &'static str {
    match value.and_then(Value::as_str) {
        Some("worktree") => "worktree",
        _ => "local",
    }
}

fn build_automation_thread_name(name: &str) -> String {
    format!("Automation · {}", name.trim())
}

fn build_automation_worktree_name(name: &str) -> String {
    let sanitized = name
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|character| match character {
            'a'..='z' | '0'..='9' => character,
            _ => '-',
        })
        .collect::<String>()
        .split('-')
        .filter(|entry| !entry.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    if sanitized.is_empty() {
        "automation".to_string()
    } else {
        sanitized.chars().take(48).collect()
    }
}

async fn emit_profile_automations_updated(state: &AppState, profile_id: &str) {
    let payload = with_ui_state_read(state, profile_id, |ui_state| {
        Ok(json!({
            "automations": {
                "items": sorted_automations_from_ui_state(ui_state),
                "recentRuns": recent_automation_runs_from_ui_state(ui_state, DEFAULT_AUTOMATION_RUN_HISTORY_LIMIT)
            }
        }))
    })
    .await;

    if let Ok(payload) = payload {
        emit_profile_config_updated(state, profile_id, payload).await;
    }
}

async fn clear_automation_timer(state: &AppState, profile_id: &str, automation_id: &str) {
    let timer_key = automation_timer_key(profile_id, automation_id);
    if let Some(handle) = state.automation_timers.lock().await.remove(&timer_key) {
        handle.abort();
    }
}

fn schedule_automation_timer(
    state: AppState,
    profile_id: String,
    automation: Value,
) -> futures_util::future::BoxFuture<'static, ()> {
    async move {
        let automation_id = trimmed_json_string(automation.get("id"));
        let enabled = automation
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let schedule_mode = automation_schedule_mode(automation.get("scheduleMode"));
        let next_run_at = automation.get("nextRunAt").and_then(Value::as_i64);

        let Some(automation_id) = automation_id else {
            return;
        };

        clear_automation_timer(&state, &profile_id, &automation_id).await;

        if !enabled || schedule_mode != "interval" {
            return;
        }

        let Some(next_run_at) = next_run_at else {
            return;
        };

        let timer_key = automation_timer_key(&profile_id, &automation_id);
        let sleep_ms = next_run_at.saturating_sub(now_unix_ms() as i64).max(0) as u64;
        let next_state = state.clone();
        let next_profile_id = profile_id.clone();
        let next_automation_id = automation_id.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
            next_state
                .automation_timers
                .lock()
                .await
                .remove(&automation_timer_key(&next_profile_id, &next_automation_id));
            if let Err(error) = run_automation_payload(
                &next_state,
                &next_profile_id,
                &next_automation_id,
                "schedule",
            )
            .await
            {
                warn!(
                    "scheduled automation run failed for {} on profile {}: {}",
                    next_automation_id, next_profile_id, error.message
                );
            }
        });
        state
            .automation_timers
            .lock()
            .await
            .insert(timer_key, handle);
    }
    .boxed()
}

async fn restore_automation_schedules(state: AppState) {
    let profile_ids = state.config.profiles.keys().cloned().collect::<Vec<_>>();
    for profile_id in profile_ids {
        let result = with_ui_state_read(&state, &profile_id, |ui_state| {
            Ok(sorted_automations_from_ui_state(ui_state))
        })
        .await;
        match result {
            Ok(automations) => {
                for automation in automations {
                    schedule_automation_timer(state.clone(), profile_id.clone(), automation).await;
                }
            }
            Err(error) => {
                warn!(
                    "failed to restore automation schedules for profile {}: {}",
                    profile_id, error.message
                );
            }
        }
    }
}

async fn save_automation_payload(
    state: &AppState,
    profile_id: &str,
    automation: Value,
) -> ApiResult<Value> {
    let automation_id = trimmed_json_string(automation.get("id"))
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "automation.id is required."))?;
    let automation_name = trimmed_json_string(automation.get("name"))
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "Automation name is required."))?;
    let automation_prompt = automation
        .get("prompt")
        .and_then(Value::as_str)
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "Automation prompt is required."))?;
    if automation_prompt.trim().is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Automation prompt is required.",
        ));
    }

    let schedule_mode = automation_schedule_mode(automation.get("scheduleMode"));
    let normalized_interval = if schedule_mode == "interval" {
        automation
            .get("intervalMinutes")
            .and_then(|value| {
                value
                    .as_i64()
                    .or_else(|| value.as_u64().map(|entry| entry as i64))
                    .or_else(|| value.as_f64().map(|entry| entry.round() as i64))
            })
            .map(|value| value.max(1))
            .ok_or_else(|| {
                api_error(
                    StatusCode::BAD_REQUEST,
                    "Automation interval must be at least 1 minute.",
                )
            })?
    } else {
        0
    };

    let normalized_target = automation_target(automation.get("target"));
    let repo_path = trimmed_json_string(automation.get("repoPath"));
    if normalized_target == "worktree" && repo_path.is_none() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Worktree automations require a repository.",
        ));
    }

    let now = now_unix_ms() as i64;
    let payload = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(automations) = ui_state.get_mut("automations").and_then(Value::as_array_mut) else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "automations state is missing",
            ));
        };

        let created_at = automations
            .iter()
            .find(|entry| entry.get("id").and_then(Value::as_str) == Some(automation_id.as_str()))
            .and_then(|entry| entry.get("createdAt").and_then(Value::as_i64))
            .or_else(|| automation.get("createdAt").and_then(Value::as_i64))
            .unwrap_or(now);
        let enabled = automation
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let next_run_at = if enabled && schedule_mode == "interval" {
            Some(now + normalized_interval * 60_000)
        } else {
            None
        };

        let next_automation = json!({
            "id": automation_id,
            "name": automation_name,
            "prompt": automation_prompt,
            "enabled": enabled,
            "scheduleMode": schedule_mode,
            "intervalMinutes": if schedule_mode == "interval" { Value::from(normalized_interval) } else { Value::Null },
            "target": normalized_target,
            "repoPath": repo_path.clone().map(Value::from).unwrap_or(Value::Null),
            "cwd": trimmed_json_string(automation.get("cwd")).map(Value::from).unwrap_or(Value::Null),
            "model": trimmed_json_string(automation.get("model")).map(Value::from).unwrap_or(Value::Null),
            "effort": trimmed_json_string(automation.get("effort")).map(Value::from).unwrap_or(Value::Null),
            "speed": trimmed_json_string(automation.get("speed")).map(Value::from).unwrap_or(Value::Null),
            "mode": trimmed_json_string(automation.get("mode")).map(Value::from).unwrap_or(Value::Null),
            "createdAt": created_at,
            "updatedAt": now,
            "lastRunAt": automation.get("lastRunAt").cloned().unwrap_or(Value::Null),
            "nextRunAt": next_run_at.map(Value::from).unwrap_or(Value::Null)
        });

        let mut next_automations = vec![next_automation];
        next_automations.extend(
            automations
                .iter()
                .filter(|entry| entry.get("id").and_then(Value::as_str) != Some(automation_id.as_str()))
                .cloned(),
        );
        next_automations.truncate(80);
        next_automations.sort_by(|left, right| {
            right
                .get("updatedAt")
                .and_then(Value::as_i64)
                .unwrap_or_default()
                .cmp(
                    &left
                        .get("updatedAt")
                        .and_then(Value::as_i64)
                        .unwrap_or_default(),
                )
        });
        *automations = next_automations;

        Ok(json!({
            "automations": automations.clone()
        }))
    })
    .await?;

    if let Some(saved_automation) = payload
        .get("automations")
        .and_then(Value::as_array)
        .and_then(|entries| {
            entries.iter().find(|entry| {
                entry.get("id").and_then(Value::as_str) == Some(automation_id.as_str())
            })
        })
        .cloned()
    {
        schedule_automation_timer(state.clone(), profile_id.to_string(), saved_automation).await;
    } else {
        clear_automation_timer(state, profile_id, &automation_id).await;
    }

    emit_profile_automations_updated(state, profile_id).await;
    Ok(payload)
}

async fn delete_automation_payload(
    state: &AppState,
    profile_id: &str,
    automation_id: &str,
) -> ApiResult<Value> {
    let trimmed_automation_id = automation_id.trim();
    clear_automation_timer(state, profile_id, trimmed_automation_id).await;

    let payload = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(automations) = ui_state
            .get_mut("automations")
            .and_then(Value::as_array_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "automations state is missing",
            ));
        };

        *automations = automations
            .iter()
            .filter(|entry| entry.get("id").and_then(Value::as_str) != Some(trimmed_automation_id))
            .cloned()
            .collect::<Vec<_>>();
        automations.sort_by(|left, right| {
            right
                .get("updatedAt")
                .and_then(Value::as_i64)
                .unwrap_or_default()
                .cmp(
                    &left
                        .get("updatedAt")
                        .and_then(Value::as_i64)
                        .unwrap_or_default(),
                )
        });

        Ok(json!({
            "automations": automations.clone()
        }))
    })
    .await?;

    emit_profile_automations_updated(state, profile_id).await;
    Ok(payload)
}

async fn run_automation_payload(
    state: &AppState,
    profile_id: &str,
    automation_id: &str,
    trigger: &str,
) -> ApiResult<Value> {
    let automation = with_ui_state_read(state, profile_id, |ui_state| {
        sorted_automations_from_ui_state(ui_state)
            .into_iter()
            .find(|entry| entry.get("id").and_then(Value::as_str) == Some(automation_id))
            .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Automation not found."))
    })
    .await?;

    let automation_name = trimmed_json_string(automation.get("name"))
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "Automation name is required."))?;
    let automation_prompt = automation
        .get("prompt")
        .and_then(Value::as_str)
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "Automation prompt is required."))?
        .to_string();
    let automation_target = automation_target(automation.get("target"));
    let repo_path = trimmed_json_string(automation.get("repoPath"));
    let mut cwd = trimmed_json_string(automation.get("cwd")).or_else(|| repo_path.clone());
    let mut git_repo_path = repo_path.clone();
    let mut worktree_path: Option<String> = None;
    let run_id = Uuid::new_v4().to_string();
    let now = now_unix_ms() as i64;
    let normalized_trigger = if trigger == "schedule" {
        "schedule"
    } else {
        "manual"
    };

    with_ui_state_write(state, profile_id, |ui_state| {
        let Some(automation_runs) = ui_state
            .get_mut("automationRuns")
            .and_then(Value::as_array_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "automation runs state is missing",
            ));
        };

        let next_run = json!({
            "id": run_id,
            "automationId": automation_id,
            "automationName": automation_name,
            "status": "running",
            "trigger": normalized_trigger,
            "sessionId": Value::Null,
            "repoPath": git_repo_path.clone().map(Value::from).unwrap_or(Value::Null),
            "cwd": cwd.clone().map(Value::from).unwrap_or(Value::Null),
            "worktreePath": Value::Null,
            "startedAt": now,
            "completedAt": Value::Null,
            "error": Value::Null
        });
        let mut next_runs = vec![next_run];
        next_runs.extend(
            automation_runs
                .iter()
                .filter(|entry| entry.get("id").and_then(Value::as_str) != Some(run_id.as_str()))
                .cloned(),
        );
        next_runs.truncate(200);
        *automation_runs = next_runs;
        Ok(())
    })
    .await?;
    emit_profile_automations_updated(state, profile_id).await;

    let result: ApiResult<Value> = async {
        if automation_target == "worktree" {
            let repo_root = repo_path.clone().ok_or_else(|| {
                api_error(
                    StatusCode::BAD_REQUEST,
                    "Worktree automations require a repository.",
                )
            })?;
            let repo_root_path = PathBuf::from(&repo_root);
            let time_suffix = now.to_string();
            let worktree_name = build_automation_worktree_name(&automation_name);
            let worktree = repo_root_path
                .parent()
                .unwrap_or_else(|| repo_root_path.as_path())
                .join(".codex-webui-worktrees")
                .join(&worktree_name)
                .join(&time_suffix);
            let branch_name = format!("automation/{worktree_name}-{time_suffix}");

            create_git_worktree_payload(
                state,
                &repo_root,
                &worktree.display().to_string(),
                Some(&branch_name),
                true,
                false,
            )
            .await
            .map_err(|error| {
                api_error(
                    error.status,
                    format!(
                        "Failed to create the automation worktree: {}",
                        error.message
                    ),
                )
            })?;

            let worktree_display = worktree.display().to_string();
            worktree_path = Some(worktree_display.clone());
            cwd = Some(worktree_display.clone());
            git_repo_path = Some(worktree_display);
        }

        let mut preferences = serde_json::Map::new();
        if let Some(cwd) = &cwd {
            preferences.insert("cwd".to_string(), json!(cwd));
        }
        if let Some(git_repo_path) = &git_repo_path {
            preferences.insert("gitRepoPath".to_string(), json!(git_repo_path));
        }
        for key in ["model", "effort", "speed", "mode"] {
            if let Some(value) = trimmed_json_string(automation.get(key)) {
                preferences.insert(key.to_string(), Value::String(value));
            }
        }

        let session = create_session_payload(
            state,
            profile_id,
            Value::Object(preferences.clone()),
            Some(&build_automation_thread_name(&automation_name)),
        )
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to create the automation session: {}", error.message),
            )
        })?;

        let session_id = session
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                api_error(
                    StatusCode::BAD_GATEWAY,
                    "Internal session creation returned an invalid payload.",
                )
            })?
            .to_string();

        with_ui_state_write(state, profile_id, |ui_state| {
            let Some(automation_runs) = ui_state
                .get_mut("automationRuns")
                .and_then(Value::as_array_mut)
            else {
                return Err(api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "automation runs state is missing",
                ));
            };

            if let Some(run) = automation_runs
                .iter_mut()
                .find(|entry| entry.get("id").and_then(Value::as_str) == Some(run_id.as_str()))
            {
                *run = json!({
                    "id": run_id,
                    "automationId": automation_id,
                    "automationName": automation_name,
                    "status": "started",
                    "trigger": normalized_trigger,
                    "sessionId": session_id,
                    "repoPath": git_repo_path.clone().map(Value::from).unwrap_or(Value::Null),
                    "cwd": cwd.clone().map(Value::from).unwrap_or(Value::Null),
                    "worktreePath": worktree_path.clone().map(Value::from).unwrap_or(Value::Null),
                    "startedAt": now,
                    "completedAt": Value::Null,
                    "error": Value::Null
                });
            }
            Ok(())
        })
        .await?;

        send_turn_payload(
            state,
            profile_id,
            &session_id,
            &automation_prompt,
            Some(&json!([])),
            Value::Object(preferences),
        )
        .await
        .map(|_| ())
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to send the automation prompt: {}", error.message),
            )
        })?;

        let updated_automation = with_ui_state_write(state, profile_id, |ui_state| {
            let Some(automations) = ui_state
                .get_mut("automations")
                .and_then(Value::as_array_mut)
            else {
                return Err(api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "automations state is missing",
                ));
            };

            let Some(automation_entry) = automations
                .iter_mut()
                .find(|entry| entry.get("id").and_then(Value::as_str) == Some(automation_id))
            else {
                return Err(api_error(StatusCode::NOT_FOUND, "Automation not found."));
            };

            let enabled = automation_entry
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let interval_minutes = automation_entry
                .get("intervalMinutes")
                .and_then(Value::as_i64);
            let next_run_at = if enabled
                && automation_schedule_mode(automation_entry.get("scheduleMode")) == "interval"
            {
                interval_minutes.map(|value| now + value.max(1) * 60_000)
            } else {
                None
            };

            if let Some(object) = automation_entry.as_object_mut() {
                object.insert("lastRunAt".to_string(), Value::from(now));
                object.insert("updatedAt".to_string(), Value::from(now));
                object.insert(
                    "nextRunAt".to_string(),
                    next_run_at.map(Value::from).unwrap_or(Value::Null),
                );
            }

            Ok(automation_entry.clone())
        })
        .await?;

        emit_profile_automations_updated(state, profile_id).await;
        let schedule_state = state.clone();
        let schedule_profile_id = profile_id.to_string();
        tokio::spawn(async move {
            schedule_automation_timer(schedule_state, schedule_profile_id, updated_automation)
                .await;
        });

        let run = with_ui_state_read(state, profile_id, |ui_state| {
            recent_automation_runs_from_ui_state(ui_state, 200)
                .into_iter()
                .find(|entry| entry.get("id").and_then(Value::as_str) == Some(run_id.as_str()))
                .ok_or_else(|| {
                    api_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Failed to read the automation run after dispatch.",
                    )
                })
        })
        .await?;

        Ok(json!({
            "ok": true,
            "session": session,
            "run": run
        }))
    }
    .await;

    if let Err(error) = &result {
        let error_message = error.message.clone();
        let completed_at = now_unix_ms() as i64;
        let _ = with_ui_state_write(state, profile_id, |ui_state| {
            let Some(automation_runs) = ui_state
                .get_mut("automationRuns")
                .and_then(Value::as_array_mut)
            else {
                return Err(api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "automation runs state is missing",
                ));
            };

            if let Some(run) = automation_runs
                .iter_mut()
                .find(|entry| entry.get("id").and_then(Value::as_str) == Some(run_id.as_str()))
            {
                let session_id = run.get("sessionId").cloned().unwrap_or(Value::Null);
                *run = json!({
                    "id": run_id,
                    "automationId": automation_id,
                    "automationName": automation_name,
                    "status": "failed",
                    "trigger": normalized_trigger,
                    "sessionId": session_id,
                    "repoPath": git_repo_path.clone().map(Value::from).unwrap_or(Value::Null),
                    "cwd": cwd.clone().map(Value::from).unwrap_or(Value::Null),
                    "worktreePath": worktree_path.clone().map(Value::from).unwrap_or(Value::Null),
                    "startedAt": now,
                    "completedAt": completed_at,
                    "error": error_message
                });
            }
            Ok(())
        })
        .await;
        emit_profile_automations_updated(state, profile_id).await;

        if normalized_trigger == "schedule" {
            let interval_minutes = automation
                .get("intervalMinutes")
                .and_then(Value::as_i64)
                .unwrap_or(1)
                .max(1);
            let next_run_at = completed_at + interval_minutes * 60_000;
            if let Ok(updated_automation) = with_ui_state_write(state, profile_id, |ui_state| {
                let Some(automations) = ui_state
                    .get_mut("automations")
                    .and_then(Value::as_array_mut)
                else {
                    return Err(api_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "automations state is missing",
                    ));
                };
                let Some(automation_entry) = automations
                    .iter_mut()
                    .find(|entry| entry.get("id").and_then(Value::as_str) == Some(automation_id))
                else {
                    return Err(api_error(StatusCode::NOT_FOUND, "Automation not found."));
                };
                if let Some(object) = automation_entry.as_object_mut() {
                    object.insert("nextRunAt".to_string(), Value::from(next_run_at));
                }
                Ok(automation_entry.clone())
            })
            .await
            {
                let schedule_state = state.clone();
                let schedule_profile_id = profile_id.to_string();
                tokio::spawn(async move {
                    schedule_automation_timer(
                        schedule_state,
                        schedule_profile_id,
                        updated_automation,
                    )
                    .await;
                });
                emit_profile_automations_updated(state, profile_id).await;
            }
        }
    }

    result
}

fn arena_store_path(config: &Config, profile_id: &str) -> PathBuf {
    resolve_runtime_profile(config, profile_id)
        .data_dir
        .join("arena-runs.json")
}

async fn read_arena_store_state(state: &AppState, profile_id: &str) -> Result<ArenaStoreState> {
    let _guard = ui_state_lock(state, profile_id).await.lock_owned().await;
    let path = arena_store_path(&state.config, profile_id);
    match tokio_fs::read_to_string(&path).await {
        Ok(raw) => match serde_json::from_str::<ArenaStoreState>(&raw) {
            Ok(parsed) => Ok(parsed),
            Err(_) => {
                let empty = ArenaStoreState::default();
                if let Some(parent) = path.parent() {
                    tokio_fs::create_dir_all(parent).await.ok();
                }
                tokio_fs::write(
                    &path,
                    serde_json::to_vec_pretty(&empty).unwrap_or_else(|_| b"{\"runs\":[]}".to_vec()),
                )
                .await
                .ok();
                Ok(empty)
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(ArenaStoreState::default())
        }
        Err(error) => Err(error).context("failed to read arena store"),
    }
}

async fn write_arena_store_state(
    state: &AppState,
    profile_id: &str,
    arena_state: &ArenaStoreState,
) -> Result<()> {
    let _guard = ui_state_lock(state, profile_id).await.lock_owned().await;
    let path = arena_store_path(&state.config, profile_id);
    if let Some(parent) = path.parent() {
        tokio_fs::create_dir_all(parent)
            .await
            .context("failed to create arena store directory")?;
    }
    let bytes =
        serde_json::to_vec_pretty(arena_state).context("failed to encode arena store state")?;
    tokio_fs::write(path, bytes)
        .await
        .context("failed to write arena store state")
}

fn value_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Value::Object(object) => {
            for key in ["text", "title", "value", "name", "status", "state"] {
                if let Some(text) = object.get(key).and_then(value_text) {
                    return Some(text);
                }
            }
            None
        }
        _ => None,
    }
}

fn normalized_thread_status(value: Option<&Value>) -> Option<String> {
    let Some(value) = value else {
        return None;
    };
    match value {
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Value::Object(object) => object
            .get("type")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| value_text(value)),
        _ => None,
    }
}

fn is_live_thread_status(status: &str) -> bool {
    matches!(status, "running" | "active")
}

fn extract_arena_response(turns: &[Value]) -> Option<String> {
    for turn in turns.iter().rev() {
        let Some(items) = turn.get("items").and_then(Value::as_array) else {
            continue;
        };
        for item in items.iter().rev() {
            if item.get("type").and_then(Value::as_str) != Some("agentMessage") {
                continue;
            }
            if let Some(text) = item.get("text").and_then(value_text) {
                return Some(text);
            }
        }
    }
    None
}

async fn list_arena_runs_payload(state: &AppState, profile_id: &str) -> ApiResult<Value> {
    let mut arena_state = read_arena_store_state(state, profile_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to read arena runs: {error}"),
            )
        })?;
    let mut changed = false;

    for run in &mut arena_state.runs {
        for contestant in &mut run.contestants {
            let thread = read_thread_payload(state, profile_id, &contestant.session_id, true).await;
            let Ok(thread) = thread else {
                continue;
            };
            let Some(thread) = thread.as_object() else {
                continue;
            };
            let status = normalized_thread_status(thread.get("status"))
                .unwrap_or_else(|| contestant.status.clone());
            let mut response = contestant.response.clone();
            if response.is_none() && !is_live_thread_status(&status) {
                if let Some(turns) = thread.get("turns").and_then(Value::as_array) {
                    response = extract_arena_response(turns);
                }
            }
            let updated_at = contestant.updated_at.max(
                thread
                    .get("updatedAt")
                    .and_then(Value::as_u64)
                    .unwrap_or(contestant.updated_at),
            );
            if status != contestant.status
                || response != contestant.response
                || updated_at != contestant.updated_at
            {
                contestant.status = status;
                contestant.response = response;
                contestant.updated_at = updated_at;
                changed = true;
            }
        }

        let next_status = if run
            .contestants
            .iter()
            .any(|contestant| is_live_thread_status(&contestant.status))
        {
            "running".to_string()
        } else {
            "completed".to_string()
        };
        let next_updated_at = run
            .contestants
            .iter()
            .map(|contestant| contestant.updated_at)
            .max()
            .unwrap_or(run.updated_at)
            .max(run.updated_at);
        if run.status != next_status || run.updated_at != next_updated_at {
            run.status = next_status;
            run.updated_at = next_updated_at;
            changed = true;
        }
    }

    arena_state
        .runs
        .sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    if changed {
        write_arena_store_state(state, profile_id, &arena_state)
            .await
            .map_err(|error| {
                api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to persist hydrated arena runs: {error}"),
                )
            })?;
    }

    Ok(json!({
        "runs": arena_state.runs
    }))
}

async fn start_arena_run_payload(
    state: &AppState,
    profile_id: &str,
    prompt: &str,
    contestants: &Value,
    preferences: &Value,
) -> ApiResult<Value> {
    let trimmed_prompt = prompt.trim();
    if trimmed_prompt.is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "Prompt is required."));
    }

    let mut normalized_contestants = Vec::<(String, String)>::new();
    let mut seen_models = std::collections::HashSet::new();
    for contestant in contestants
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .take(8)
    {
        let model = contestant
            .get("model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_default();
        let label = contestant
            .get("label")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| model.clone());
        if model.is_empty() || label.is_empty() || !seen_models.insert(model.clone()) {
            continue;
        }
        normalized_contestants.push((model, label));
        if normalized_contestants.len() >= 4 {
            break;
        }
    }

    if normalized_contestants.len() < 2 {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Choose at least two models for an arena run.",
        ));
    }

    let config_payload = get_config_payload(state, profile_id).await?;
    let mut base_preferences = config_payload
        .get("defaults")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(overrides) = preferences.as_object() {
        for (key, value) in overrides {
            if !value.is_null() {
                base_preferences.insert(key.clone(), value.clone());
            }
        }
    }

    let created_at = now_unix_ms();
    let title_source = trimmed_prompt
        .split('\n')
        .next()
        .unwrap_or(trimmed_prompt)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let title = if title_source.is_empty() {
        "Arena run".to_string()
    } else {
        title_source.chars().take(60).collect::<String>()
    };
    let client = app_server_client(state, profile_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to codex app-server: {error}"),
            )
        })?;
    let mut arena_contestants = Vec::new();

    for (model, label) in &normalized_contestants {
        let mut session_preferences = base_preferences.clone();
        session_preferences.insert("model".to_string(), Value::String(model.clone()));
        let cwd = session_preferences
            .get("cwd")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                api_error(
                    StatusCode::BAD_REQUEST,
                    "A working directory is required to start an arena run.",
                )
            })?;

        let response = client
            .request(
                "thread/start",
                json!({
                    "model": session_preferences.get("model").cloned().unwrap_or(Value::Null),
                    "cwd": cwd,
                    "approvalPolicy": session_preferences.get("approvalPolicy").cloned().unwrap_or_else(|| json!("on-request")),
                    "sandbox": session_preferences.get("sandboxMode").cloned().unwrap_or_else(|| json!("workspace-write")),
                    "personality": session_preferences.get("personality").cloned().unwrap_or(Value::Null),
                    "serviceTier": match session_preferences.get("speed").and_then(Value::as_str) {
                        Some("fast") => Value::String("fast".to_string()),
                        Some("flex") => Value::String("flex".to_string()),
                        _ => Value::Null
                    },
                    "experimentalRawEvents": false,
                    "persistExtendedHistory": true
                }),
            )
            .await
            .map_err(|error| {
                api_error(
                    StatusCode::BAD_GATEWAY,
                    format!("Failed to create an arena session: {error}"),
                )
            })?;
        let session_id = response
            .get("thread")
            .and_then(|thread| thread.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                api_error(
                    StatusCode::BAD_GATEWAY,
                    "Codex app-server returned an invalid arena session payload.",
                )
            })?;

        with_ui_state_write(state, profile_id, |ui_state| {
            let Some(preferences_by_thread_id) = ui_state
                .get_mut("preferencesByThreadId")
                .and_then(Value::as_object_mut)
            else {
                return Err(api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "preferences state is missing",
                ));
            };
            preferences_by_thread_id.insert(
                session_id.clone(),
                Value::Object(session_preferences.clone()),
            );
            Ok(())
        })
        .await?;

        let thread_name = format!("Arena · {} · {}", title, label)
            .chars()
            .take(120)
            .collect::<String>();
        client
            .request(
                "thread/name/set",
                json!({
                    "threadId": session_id,
                    "name": thread_name
                }),
            )
            .await
            .map_err(|error| {
                api_error(
                    StatusCode::BAD_GATEWAY,
                    format!("Failed to name an arena session: {error}"),
                )
            })?;

        arena_contestants.push(ArenaContestantRecord {
            id: Uuid::new_v4().to_string(),
            session_id,
            model: model.clone(),
            label: label.clone(),
            status: "running".to_string(),
            response: None,
            created_at,
            updated_at: created_at,
        });
    }

    let run = ArenaRunRecord {
        id: Uuid::new_v4().to_string(),
        prompt: trimmed_prompt.to_string(),
        cwd: base_preferences
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        status: "running".to_string(),
        created_at,
        updated_at: created_at,
        contestants: arena_contestants.clone(),
    };

    let mut arena_state = read_arena_store_state(state, profile_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to read arena runs: {error}"),
            )
        })?;
    arena_state.runs.retain(|entry| entry.id != run.id);
    arena_state.runs.insert(0, run.clone());
    arena_state.runs.truncate(60);
    write_arena_store_state(state, profile_id, &arena_state)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to persist arena runs: {error}"),
            )
        })?;

    for contestant in &arena_contestants {
        let mut session_preferences = base_preferences.clone();
        session_preferences.insert("model".to_string(), Value::String(contestant.model.clone()));
        let send_result = send_turn_payload(
            state,
            profile_id,
            &contestant.session_id,
            trimmed_prompt,
            Some(&json!([])),
            Value::Object(session_preferences),
        )
        .await;

        if let Err(error) = send_result {
            let mut current_state =
                read_arena_store_state(state, profile_id)
                    .await
                    .map_err(|read_error| {
                        api_error(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!(
                                "Failed to refresh arena runs after send failure: {read_error}"
                            ),
                        )
                    })?;
            if let Some(current_run) = current_state
                .runs
                .iter_mut()
                .find(|entry| entry.id == run.id)
            {
                current_run.updated_at = now_unix_ms();
                if let Some(current_contestant) = current_run
                    .contestants
                    .iter_mut()
                    .find(|entry| entry.id == contestant.id)
                {
                    current_contestant.status = "failed".to_string();
                    current_contestant.response = Some(error.to_string());
                    current_contestant.updated_at = current_run.updated_at;
                }
            }
            write_arena_store_state(state, profile_id, &current_state)
                .await
                .map_err(|write_error| {
                    api_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Failed to persist arena send failure: {write_error}"),
                    )
                })?;
        }
    }

    let payload = list_arena_runs_payload(state, profile_id).await?;
    let matching_run = payload
        .get("runs")
        .and_then(Value::as_array)
        .and_then(|runs| {
            runs.iter()
                .find(|entry| entry.get("id").and_then(Value::as_str) == Some(run.id.as_str()))
        })
        .cloned()
        .unwrap_or_else(|| serde_json::to_value(&run).unwrap_or(Value::Null));
    Ok(matching_run)
}

fn parse_front_matter(raw: &str) -> (Option<String>, Option<String>) {
    let Some(stripped) = raw.strip_prefix("---\n") else {
        return (None, None);
    };
    let Some((front_matter, _)) = stripped.split_once("\n---") else {
        return (None, None);
    };

    let mut name = None;
    let mut description = None;
    for line in front_matter.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "name" if !value.is_empty() => name = Some(value.to_string()),
            "description" if !value.is_empty() => description = Some(value.to_string()),
            _ => {}
        }
    }
    (name, description)
}

fn walk_matching_files(root: &Path, matcher: &dyn Fn(&Path) -> bool, results: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_matching_files(&path, matcher, results);
        } else if path.is_file() && matcher(&path) {
            results.push(path);
        }
    }
}

fn build_catalog_payload_for_codex_home(codex_home: &Path) -> Value {
    let skills_root = codex_home.join("skills");
    let plugins_root = codex_home.join("plugins");

    let mut skill_files = Vec::new();
    let mut plugin_skill_files = Vec::new();
    walk_matching_files(
        &skills_root,
        &|path| path.file_name().and_then(|value| value.to_str()) == Some("SKILL.md"),
        &mut skill_files,
    );
    walk_matching_files(
        &plugins_root,
        &|path| path.file_name().and_then(|value| value.to_str()) == Some("SKILL.md"),
        &mut plugin_skill_files,
    );

    let mut skills = skill_files
        .into_iter()
        .chain(plugin_skill_files)
        .map(|path| {
            let raw = fs::read_to_string(&path).unwrap_or_default();
            let (name, description) = parse_front_matter(&raw);
            let (normalized_relative, source, plugin_name) =
                if let Ok(relative) = path.strip_prefix(&skills_root) {
                    let relative_string = relative.to_string_lossy().replace('\\', "/");
                    let source = if relative_string.starts_with(".system/") {
                        "system"
                    } else {
                        "local"
                    };
                    (relative_string, source, Value::Null)
                } else if let Ok(relative) = path.strip_prefix(&plugins_root) {
                    let relative_string = relative.to_string_lossy().replace('\\', "/");
                    let plugin_name = relative_string
                        .split('/')
                        .next()
                        .filter(|value| !value.is_empty())
                        .map(|value| Value::String(value.to_string()))
                        .unwrap_or(Value::Null);
                    let source = if relative_string.starts_with(".system/") {
                        "system"
                    } else {
                        "plugin"
                    };
                    (relative_string, source, plugin_name)
                } else {
                    (
                        path.file_name()
                            .and_then(|value| value.to_str())
                            .unwrap_or("SKILL.md")
                            .to_string(),
                        "local",
                        Value::Null,
                    )
                };

            let skill_name = name
                .or_else(|| {
                    path.parent()
                        .and_then(|parent| parent.file_name())
                        .and_then(|value| value.to_str())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| "Skill".to_string());

            json!({
                "id": normalized_relative.trim_end_matches("/SKILL.md"),
                "name": skill_name,
                "description": description.unwrap_or_default(),
                "path": path.display().to_string(),
                "source": source,
                "pluginName": plugin_name
            })
        })
        .collect::<Vec<_>>();

    skills.sort_by(|left, right| {
        left.get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(
                right
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
    });

    let mut plugin_files = Vec::new();
    walk_matching_files(
        &plugins_root,
        &|path| path.ends_with(Path::new(".codex-plugin").join("plugin.json")),
        &mut plugin_files,
    );

    let mut plugins = plugin_files
        .into_iter()
        .map(|path| {
            let raw = fs::read_to_string(&path).unwrap_or_else(|_| "{}".to_string());
            let parsed: Value = serde_json::from_str(&raw).unwrap_or_else(|_| json!({}));
            let plugin_base = path
                .parent()
                .and_then(Path::parent)
                .map(Path::to_path_buf)
                .unwrap_or_else(|| path.clone());
            let interface = parsed.get("interface").cloned().unwrap_or_else(|| json!({}));
            let skills_dir = parsed
                .get("skills")
                .and_then(Value::as_str)
                .map(|value| plugin_base.join(value));
            let mut plugin_skill_entries = Vec::new();
            if let Some(skills_dir) = skills_dir {
                walk_matching_files(
                    &skills_dir,
                    &|candidate| {
                        candidate.file_name().and_then(|value| value.to_str()) == Some("SKILL.md")
                    },
                    &mut plugin_skill_entries,
                );
            }
            plugin_skill_entries.sort();

            let name = parsed
                .get("name")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string)
                .or_else(|| {
                    plugin_base
                        .file_name()
                        .and_then(|value| value.to_str())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| "plugin".to_string());
            let display_name = interface
                .get("displayName")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| name.clone());

            json!({
                "name": name,
                "displayName": display_name,
                "description": parsed.get("description").and_then(Value::as_str).unwrap_or_default(),
                "version": parsed.get("version").cloned().unwrap_or(Value::Null),
                "developerName": interface.get("developerName").cloned().unwrap_or(Value::Null),
                "category": interface.get("category").cloned().unwrap_or(Value::Null),
                "path": plugin_base.display().to_string(),
                "skills": plugin_skill_entries
                    .iter()
                    .filter_map(|skill_path| {
                        skill_path
                            .parent()
                            .and_then(Path::file_name)
                            .and_then(|value| value.to_str())
                            .map(str::to_string)
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect::<Vec<_>>();

    plugins.sort_by(|left, right| {
        left.get("displayName")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(
                right
                    .get("displayName")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
    });

    json!({
        "plugins": plugins,
        "skills": skills
    })
}

async fn get_catalog_payload(state: &AppState, profile_id: &str) -> ApiResult<Value> {
    let codex_home = resolve_runtime_profile(&state.config, profile_id)
        .codex_home
        .display()
        .to_string();

    if let Some(cached) = state.catalog_cache.lock().await.get(&codex_home).cloned() {
        if cached.created_at.elapsed() < CATALOG_CACHE_TTL {
            return Ok(cached.payload);
        }
    }

    let codex_home_path = resolve_runtime_profile(&state.config, profile_id)
        .codex_home
        .clone();
    let payload =
        tokio::task::spawn_blocking(move || build_catalog_payload_for_codex_home(&codex_home_path))
            .await
            .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;

    state.catalog_cache.lock().await.insert(
        codex_home,
        CachedCatalog {
            created_at: Instant::now(),
            payload: payload.clone(),
        },
    );

    Ok(payload)
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

            if matches!(
                route_path.as_str(),
                "/api/config"
                    | "/api/directories"
                    | "/api/editor"
                    | "/api/catalog"
                    | "/api/notifications"
                    | "/api/notifications/settings"
                    | "/api/session-filters"
                    | "/api/prompt-presets"
            ) {
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
                    "/api/config" => handle_config_api_http(state, request, auth).await,
                    "/api/directories" => handle_directories_api_http(state, request).await,
                    "/api/editor" => handle_editor_api_http(state, request, auth).await,
                    "/api/catalog" => handle_catalog_api_http(state, request, auth).await,
                    "/api/notifications" => {
                        handle_notifications_api_http(state, request, auth).await
                    }
                    "/api/notifications/settings" => {
                        handle_notification_settings_api_http(state, request, auth).await
                    }
                    "/api/session-filters" => {
                        handle_session_filters_api_http(state, request, auth).await
                    }
                    "/api/prompt-presets" => {
                        handle_prompt_presets_api_http(state, request, auth).await
                    }
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

            if route_path == "/api/git/repositories" || route_path.starts_with("/api/git/") {
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

                let mut response = handle_git_api_http(state, request, auth, &route_path).await;
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            if route_path == "/api/automations" || route_path.starts_with("/api/automations/") {
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
                    handle_automations_api_http(state, request, auth, &route_path).await;
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            if route_path == "/api/arena" {
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

                let mut response = handle_arena_api_http(state, request, auth).await;
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            if route_path == "/api/sessions" {
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

                let mut response = handle_sessions_api_http(state, request, auth).await;
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            if route_path.starts_with("/api/sessions/") && route_path.ends_with("/organization") {
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
                let session_id = route_path
                    .strip_prefix("/api/sessions/")
                    .and_then(|suffix| suffix.strip_suffix("/organization"))
                    .unwrap_or_default()
                    .trim_end_matches('/')
                    .to_string();
                let mut response =
                    handle_session_organization_api_http(state, &session_id, request, auth).await;
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            if route_path.starts_with("/api/sessions/") && route_path.ends_with("/name") {
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
                let session_id = route_path
                    .strip_prefix("/api/sessions/")
                    .and_then(|suffix| suffix.strip_suffix("/name"))
                    .unwrap_or_default()
                    .trim_end_matches('/')
                    .to_string();
                let mut response =
                    handle_session_name_api_http(state, &session_id, request, auth).await;
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            if route_path.starts_with("/api/sessions/") && route_path.ends_with("/archive") {
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
                let session_id = route_path
                    .strip_prefix("/api/sessions/")
                    .and_then(|suffix| suffix.strip_suffix("/archive"))
                    .unwrap_or_default()
                    .trim_end_matches('/')
                    .to_string();
                let mut response =
                    handle_session_archive_api_http(state, &session_id, request, auth, true).await;
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            if route_path.starts_with("/api/sessions/") && route_path.ends_with("/unarchive") {
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
                let session_id = route_path
                    .strip_prefix("/api/sessions/")
                    .and_then(|suffix| suffix.strip_suffix("/unarchive"))
                    .unwrap_or_default()
                    .trim_end_matches('/')
                    .to_string();
                let mut response =
                    handle_session_archive_api_http(state, &session_id, request, auth, false).await;
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            if route_path.starts_with("/api/sessions/") && route_path.ends_with("/fork") {
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
                let session_id = route_path
                    .strip_prefix("/api/sessions/")
                    .and_then(|suffix| suffix.strip_suffix("/fork"))
                    .unwrap_or_default()
                    .trim_end_matches('/')
                    .to_string();
                let mut response =
                    handle_session_fork_api_http(state, &session_id, request, auth).await;
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            if let Some(session_id) = route_path
                .strip_prefix("/api/sessions/")
                .filter(|value| !value.is_empty() && !value.contains('/'))
                .map(str::to_string)
            {
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
                let mut response = handle_session_api_http(state, &session_id, request, auth).await;
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            if route_path.starts_with("/api/sessions/") && route_path.ends_with("/draft") {
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
                let session_id = route_path
                    .strip_prefix("/api/sessions/")
                    .and_then(|suffix| suffix.strip_suffix("/draft"))
                    .unwrap_or_default()
                    .trim_end_matches('/')
                    .to_string();
                let mut response =
                    handle_session_draft_api_http(state, request, auth, &session_id).await;
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            if route_path.starts_with("/api/sessions/") && route_path.ends_with("/messages") {
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
                let session_id = route_path
                    .strip_prefix("/api/sessions/")
                    .and_then(|suffix| suffix.strip_suffix("/messages"))
                    .unwrap_or_default()
                    .trim_end_matches('/')
                    .to_string();
                let mut response =
                    handle_session_messages_api_http(state, request, auth, &session_id).await;
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            if route_path.starts_with("/api/sessions/") && route_path.ends_with("/steer") {
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
                let session_id = route_path
                    .strip_prefix("/api/sessions/")
                    .and_then(|suffix| suffix.strip_suffix("/steer"))
                    .unwrap_or_default()
                    .trim_end_matches('/')
                    .to_string();
                let mut response =
                    handle_session_steer_api_http(state, request, auth, &session_id).await;
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            if route_path.starts_with("/api/sessions/") && route_path.contains("/queue") {
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
                let session_id = route_path
                    .strip_prefix("/api/sessions/")
                    .and_then(|suffix| suffix.split("/queue").next())
                    .unwrap_or_default()
                    .trim_end_matches('/')
                    .to_string();
                let mut response =
                    handle_session_queue_api_http(state, request, auth, &session_id, &route_path)
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

            if route_path.starts_with("/api/sessions/") && route_path.ends_with("/search") {
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
                let session_id = route_path
                    .strip_prefix("/api/sessions/")
                    .and_then(|suffix| suffix.strip_suffix("/search"))
                    .unwrap_or_default()
                    .trim_end_matches('/')
                    .to_string();
                let mut response =
                    handle_session_search_api_http(state, request, auth, &session_id).await;
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            if route_path.starts_with("/api/sessions/") && route_path.ends_with("/turns") {
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
                let session_id = route_path
                    .strip_prefix("/api/sessions/")
                    .and_then(|suffix| suffix.strip_suffix("/turns"))
                    .unwrap_or_default()
                    .trim_end_matches('/')
                    .to_string();
                let mut response =
                    handle_session_turns_api_http(state, request, auth, &session_id).await;
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            if route_path.starts_with("/api/sessions/")
                && route_path.contains("/turns/")
                && route_path.contains("/items/")
            {
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
                let mut segments = route_path
                    .trim_start_matches("/api/sessions/")
                    .split("/turns/");
                let session_id = segments
                    .next()
                    .unwrap_or_default()
                    .trim_end_matches('/')
                    .to_string();
                let rest = segments.next().unwrap_or_default();
                let mut turn_segments = rest.split("/items/");
                let turn_id = turn_segments
                    .next()
                    .unwrap_or_default()
                    .trim_end_matches('/')
                    .to_string();
                let item_id = turn_segments
                    .next()
                    .unwrap_or_default()
                    .trim_end_matches('/')
                    .to_string();
                let mut response = handle_session_item_detail_api_http(
                    state,
                    request,
                    auth,
                    &session_id,
                    &turn_id,
                    &item_id,
                )
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

            if route_path.starts_with("/api/sessions/")
                && route_path.trim_end_matches('/').ends_with("/attachments")
                && !route_path.contains("/attachments/")
            {
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
                let session_id = route_path
                    .trim_end_matches('/')
                    .strip_prefix("/api/sessions/")
                    .and_then(|suffix| suffix.strip_suffix("/attachments"))
                    .unwrap_or_default()
                    .trim_end_matches('/')
                    .to_string();
                let mut response =
                    handle_session_attachments_api_http(state, request, auth, &session_id).await;
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            if route_path.starts_with("/api/sessions/") && route_path.contains("/attachments/") {
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
                let mut segments = route_path
                    .trim_start_matches("/api/sessions/")
                    .split("/attachments/");
                let session_id = segments
                    .next()
                    .unwrap_or_default()
                    .trim_end_matches('/')
                    .to_string();
                let attachment_id = segments
                    .next()
                    .unwrap_or_default()
                    .trim_end_matches('/')
                    .to_string();
                let mut response = handle_session_attachment_api_http(
                    state,
                    request,
                    auth,
                    &session_id,
                    &attachment_id,
                )
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

            if route_path.starts_with("/api/sessions/") && route_path.contains("/turns/") {
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
                let mut segments = route_path
                    .trim_start_matches("/api/sessions/")
                    .split("/turns/");
                let session_id = segments
                    .next()
                    .unwrap_or_default()
                    .trim_end_matches('/')
                    .to_string();
                let turn_id = segments
                    .next()
                    .unwrap_or_default()
                    .trim_end_matches('/')
                    .to_string();
                let mut response =
                    handle_session_turn_api_http(state, request, auth, &session_id, &turn_id).await;
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            if route_path.starts_with("/api/sessions/") && route_path.ends_with("/abort") {
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
                let session_id = route_path
                    .strip_prefix("/api/sessions/")
                    .and_then(|suffix| suffix.strip_suffix("/abort"))
                    .unwrap_or_default()
                    .trim_end_matches('/')
                    .to_string();
                let mut response =
                    handle_session_abort_api_http(state, request, auth, &session_id).await;
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            if route_path.starts_with("/api/sessions/") && route_path.ends_with("/approval") {
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
                let session_id = route_path
                    .strip_prefix("/api/sessions/")
                    .and_then(|suffix| suffix.strip_suffix("/approval"))
                    .unwrap_or_default()
                    .trim_end_matches('/')
                    .to_string();
                let mut response =
                    handle_session_approval_api_http(state, request, auth, &session_id).await;
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            if route_path.starts_with("/api/sessions/") && route_path.ends_with("/recovery") {
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
                let session_id = route_path
                    .strip_prefix("/api/sessions/")
                    .and_then(|suffix| suffix.strip_suffix("/recovery"))
                    .unwrap_or_default()
                    .trim_end_matches('/')
                    .to_string();
                let mut response =
                    handle_session_recovery_api_http(state, request, auth, &session_id).await;
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            if route_path.starts_with("/api/sessions/") && route_path.ends_with("/stream") {
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
                let session_id = route_path
                    .strip_prefix("/api/sessions/")
                    .and_then(|suffix| suffix.strip_suffix("/stream"))
                    .unwrap_or_default()
                    .trim_end_matches('/')
                    .to_string();
                let mut response =
                    handle_session_stream_api_http(state, request, auth, &session_id).await;
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            if route_path == "/api/events/stream" {
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
                let mut response = handle_events_stream_http(state, request, auth).await;
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

                let mut response = json_error(StatusCode::NOT_FOUND, "Not found.");
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

fn env_choice(var: &str, allowed: &[&str]) -> Option<String> {
    env::var(var)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| allowed.iter().any(|allowed_value| value == allowed_value))
}

fn env_bool(var: &str) -> Option<bool> {
    match env::var(var).ok()?.trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

async fn session_preferences_defaults_payload(state: &AppState, profile_id: &str) -> Value {
    let (_, profile) = resolve_runtime_profile_entry(&state.config, profile_id);
    let codex_defaults = read_codex_toml_defaults(&profile.codex_home);
    let allowed_roots = resolved_allowed_roots(&state.config).await;
    let default_cwd = allowed_roots
        .first()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| state.config.project_root.display().to_string());
    let mode = env_choice("CODEX_WEBUI_DEFAULT_MODE", &["default", "plan"])
        .unwrap_or_else(|| "default".to_string());
    let speed = env_choice("CODEX_WEBUI_DEFAULT_SPEED", &["auto", "fast", "flex"])
        .or_else(|| {
            (codex_defaults.service_tier == "fast" || codex_defaults.service_tier == "flex")
                .then(|| codex_defaults.service_tier.clone())
        })
        .unwrap_or_else(|| "auto".to_string());
    let sandbox_mode = env_choice(
        "CODEX_WEBUI_DEFAULT_SANDBOX",
        &["read-only", "workspace-write", "danger-full-access"],
    )
    .or_else(|| codex_defaults.sandbox_mode.clone())
    .unwrap_or_else(|| "workspace-write".to_string());
    let approval_policy = env_choice(
        "CODEX_WEBUI_DEFAULT_APPROVAL_POLICY",
        &["never", "on-request", "on-failure", "untrusted"],
    )
    .or_else(|| codex_defaults.approval_policy.clone())
    .unwrap_or_else(|| "on-request".to_string());
    let effort = env_choice(
        "CODEX_WEBUI_DEFAULT_EFFORT",
        &["minimal", "low", "medium", "high", "xhigh"],
    )
    .or_else(|| {
        if mode == "plan" {
            codex_defaults.plan_mode_reasoning_effort.clone()
        } else {
            codex_defaults.model_reasoning_effort.clone()
        }
    })
    .unwrap_or_else(|| "medium".to_string());
    let personality = env_choice(
        "CODEX_WEBUI_DEFAULT_PERSONALITY",
        &["none", "friendly", "pragmatic"],
    )
    .or_else(|| codex_defaults.personality.clone())
    .unwrap_or_else(|| "pragmatic".to_string());

    json!({
        "cwd": default_cwd,
        "model": env::var("CODEX_WEBUI_DEFAULT_MODEL")
            .ok()
            .map(Value::String)
            .or_else(|| codex_defaults.model.clone().map(Value::String))
            .unwrap_or(Value::Null),
        "effort": effort,
        "speed": speed,
        "personality": personality,
        "mode": mode,
        "sendOnEnter": env_bool("CODEX_WEBUI_DEFAULT_SEND_ON_ENTER").unwrap_or(false),
        "sandboxMode": sandbox_mode,
        "approvalPolicy": approval_policy,
        "networkAccess": env_bool("CODEX_WEBUI_DEFAULT_NETWORK")
            .unwrap_or(codex_defaults.network_access.unwrap_or(false)),
        "autoApproveMode": env_choice(
            "CODEX_WEBUI_DEFAULT_AUTO_APPROVE",
            &["manual", "turn", "session"]
        )
        .unwrap_or_else(|| "manual".to_string()),
        "steeringResumeMode": env_choice(
            "CODEX_WEBUI_DEFAULT_STEERING_RESUME",
            &["ask", "auto"]
        )
        .unwrap_or_else(|| "ask".to_string()),
        "shutdownOnCompletion": false,
        "gitRepoPath": Value::Null
    })
}

async fn config_models_payload(state: &AppState, profile_id: &str) -> ApiResult<Vec<Value>> {
    let client = app_server_client(state, profile_id)
        .await
        .map_err(|error| api_error(StatusCode::BAD_GATEWAY, error.to_string()))?;
    let response = client
        .request("model/list", json!({ "includeHidden": false }))
        .await
        .map_err(|error| api_error(StatusCode::BAD_GATEWAY, error.to_string()))?;
    Ok(response
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|model| {
            json!({
                "id": model.get("id").and_then(Value::as_str).unwrap_or_default(),
                "displayName": model
                    .get("displayName")
                    .or_else(|| model.get("model"))
                    .or_else(|| model.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                "description": model.get("description").and_then(Value::as_str).unwrap_or_default(),
                "defaultReasoningEffort": model
                    .get("defaultReasoningEffort")
                    .and_then(Value::as_str)
                    .unwrap_or("medium"),
                "supportedReasoningEfforts": model
                    .get("supportedReasoningEfforts")
                    .and_then(Value::as_array)
                    .map(|entries| {
                        entries
                            .iter()
                            .filter_map(|entry| {
                                entry
                                    .get("reasoningEffort")
                                    .or_else(|| entry.get("effort"))
                                    .or(Some(entry))
                                    .and_then(Value::as_str)
                                    .map(str::to_string)
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
                "additionalSpeedTiers": model
                    .get("additionalSpeedTiers")
                    .and_then(Value::as_array)
                    .map(|entries| {
                        entries
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
                "inputModalities": model
                    .get("inputModalities")
                    .and_then(Value::as_array)
                    .map(|entries| {
                        entries
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
                "supportsPersonality": model
                    .get("supportsPersonality")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                "isDefault": model.get("isDefault").and_then(Value::as_bool).unwrap_or(false)
            })
        })
        .collect())
}

async fn config_collaboration_modes_payload(
    state: &AppState,
    profile_id: &str,
) -> ApiResult<Vec<Value>> {
    let client = app_server_client(state, profile_id)
        .await
        .map_err(|error| api_error(StatusCode::BAD_GATEWAY, error.to_string()))?;
    let response = client
        .request("collaborationMode/list", json!({}))
        .await
        .map_err(|error| api_error(StatusCode::BAD_GATEWAY, error.to_string()))?;
    Ok(response
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|mode| {
            json!({
                "name": mode.get("name").and_then(Value::as_str).unwrap_or_default(),
                "mode": mode.get("mode").cloned().unwrap_or(Value::Null),
                "model": mode.get("model").cloned().unwrap_or(Value::Null),
                "reasoning_effort": mode
                    .get("reasoning_effort")
                    .cloned()
                    .unwrap_or(Value::Null)
            })
        })
        .collect())
}

async fn get_config_payload(state: &AppState, profile_id: &str) -> ApiResult<Value> {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    let defaults = session_preferences_defaults_payload(state, profile_id).await;
    let allowed_roots = list_directories_payload(state, None)
        .await?
        .get("allowedRoots")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let notifications =
        get_notifications_payload(state, profile_id, DEFAULT_NOTIFICATION_LIMIT).await?;
    let autostart = get_autostart_state(&state.config)
        .await
        .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    let theme_override = read_stored_theme_settings(&state.config, profile_id)
        .await
        .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    let theme = theme_override.unwrap_or_else(|| json!({}));
    let models = config_models_payload(state, profile_id).await?;
    let collaboration_modes = config_collaboration_modes_payload(state, profile_id).await?;
    let account_state = get_account_state(state, profile_id)
        .await
        .map_err(|error| api_error(StatusCode::BAD_GATEWAY, error.to_string()))?;
    let (shutdown_available, _) = system_shutdown_capability(&state.config).await;
    let paused_queues = list_resume_pending_queues_payload(state, profile_id)
        .await
        .unwrap_or_else(|_| json!([]));
    let (
        saved_filters,
        known_tags,
        prompt_presets,
        automations,
        recent_runs,
        notification_settings,
        shutdown_after_queue_completes,
        scheduled_shutdown,
    ) = with_ui_state_read(state, profile_id, |ui_state| {
        let notification_settings = ui_state
            .get("notifications")
            .and_then(Value::as_object)
            .and_then(|notifications| notifications.get("settings"))
            .map(|value| normalize_notification_settings_value(Some(value)))
            .unwrap_or_else(default_notification_settings_value);

        Ok((
            ui_state
                .get("savedSessionFilters")
                .cloned()
                .unwrap_or_else(|| json!([])),
            known_tags_from_ui_state(ui_state),
            sorted_prompt_presets_from_ui_state(ui_state),
            sorted_automations_from_ui_state(ui_state),
            recent_automation_runs_from_ui_state(ui_state, DEFAULT_AUTOMATION_RUN_HISTORY_LIMIT),
            notification_settings,
            ui_state
                .get("global")
                .and_then(|value| value.get("shutdownAfterQueueCompletes"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            ui_state
                .get("global")
                .and_then(|value| value.get("scheduledShutdown"))
                .cloned()
                .unwrap_or(Value::Null),
        ))
    })
    .await?;

    let next_scheduled_shutdown = if shutdown_available
        && scheduled_shutdown
            .get("scheduledFor")
            .and_then(Value::as_u64)
            .is_some_and(|value| value > now_unix_ms())
    {
        scheduled_shutdown
    } else {
        Value::Null
    };
    let mut profiles = state
        .config
        .profiles
        .iter()
        .map(|(id, profile)| {
            json!({
                "id": id,
                "label": profile.label,
                "codexHome": profile.codex_home.display().to_string(),
                "active": id == &resolved_profile_id
            })
        })
        .collect::<Vec<_>>();
    profiles.sort_by(|left, right| {
        right
            .get("active")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            .cmp(&left.get("active").and_then(Value::as_bool).unwrap_or(false))
            .then_with(|| {
                left.get("label")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .cmp(
                        right
                            .get("label")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    )
            })
    });

    let profile = resolve_runtime_profile(&state.config, profile_id);
    let account = account_state
        .get("account")
        .cloned()
        .unwrap_or_else(|| json!({}));

    Ok(json!({
        "models": models,
        "collaborationModes": collaboration_modes,
        "allowedRoots": allowed_roots,
        "defaults": defaults,
        "paths": {
            "codexHome": profile.codex_home.display().to_string(),
            "configFilePath": config_toml_path(&profile.codex_home).display().to_string()
        },
        "git": {
            "discoveryDepth": state.config.git_discovery_depth
        },
        "autostart": autostart,
        "systemShutdown": {
            "available": shutdown_available,
            "delaySeconds": state.config.system_shutdown_delay_seconds,
            "armed": shutdown_available
                && state.config.system_shutdown_enabled
                && shutdown_after_queue_completes
        },
        "startup": {
            "pausedQueues": paused_queues,
            "scheduledShutdown": next_scheduled_shutdown
        },
        "notifications": {
            "unreadCount": notifications.get("unreadCount").cloned().unwrap_or_else(|| json!(0)),
            "settings": notification_settings
        },
        "sessionOrganization": {
            "savedFilters": saved_filters,
            "knownTags": known_tags
        },
        "promptPresets": prompt_presets,
        "automations": {
            "items": automations,
            "recentRuns": recent_runs
        },
        "account": {
            "type": account.get("type").cloned().unwrap_or(Value::Null),
            "email": account.get("email").cloned().unwrap_or(Value::Null),
            "planType": account.get("planType").cloned().unwrap_or(Value::Null),
            "requiresOpenaiAuth": account_state
                .get("requiresOpenaiAuth")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        },
        "theme": theme,
        "profiles": profiles
    }))
}

async fn update_config_payload(
    state: &AppState,
    profile_id: &str,
    payload: Value,
) -> ApiResult<Value> {
    let mut event_patch = serde_json::Map::new();

    if let Some(theme) = payload.get("theme").filter(|value| !value.is_null()) {
        let saved_theme = write_stored_theme_settings(&state.config, profile_id, theme)
            .await
            .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
        event_patch.insert("theme".to_string(), saved_theme);
    }

    if let Some(enabled) = payload
        .get("autostart")
        .and_then(|value| value.get("enabled"))
        .and_then(Value::as_bool)
    {
        let autostart = save_autostart_enabled(&state.config, enabled)
            .await
            .map_err(|error| api_error(StatusCode::BAD_REQUEST, error.to_string()))?;
        event_patch.insert("autostart".to_string(), autostart);
    }

    if let Some(armed) = payload
        .get("systemShutdown")
        .and_then(|value| value.get("armed"))
        .and_then(Value::as_bool)
    {
        with_ui_state_write(state, profile_id, |ui_state| {
            let Some(global) = ui_state.get_mut("global").and_then(Value::as_object_mut) else {
                return Err(api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "global state is missing",
                ));
            };
            global.insert("shutdownAfterQueueCompletes".to_string(), json!(armed));
            if !armed {
                global.insert("scheduledShutdown".to_string(), Value::Null);
            }
            Ok(())
        })
        .await?;
        if armed {
            maybe_schedule_global_shutdown(state, profile_id, None).await;
        } else {
            clear_scheduled_shutdown(state, profile_id).await;
        }
    }

    if !event_patch.is_empty() {
        emit_profile_config_updated(state, profile_id, Value::Object(event_patch)).await;
    }
    if payload.get("systemShutdown").is_some() {
        emit_runtime_profile_config_updated(state, profile_id).await;
    }

    get_config_payload(state, profile_id).await
}

async fn handle_config_api_http(state: AppState, request: Request, auth: AuthContext) -> Response {
    let result = match request.method() {
        &Method::GET => get_config_payload(&state, &auth.profile_id).await,
        &Method::PATCH => {
            if auth.role != UserRole::Admin {
                return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
            }

            let body = to_bytes(request.into_body(), usize::MAX)
                .await
                .context("failed to read config request body");
            match body {
                Ok(body) => {
                    let payload: Value =
                        serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
                    update_config_payload(&state, &auth.profile_id, payload).await
                }
                Err(_) => Err(api_error(
                    StatusCode::BAD_REQUEST,
                    "Failed to read config request body.",
                )),
            }
        }
        _ => return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed."),
    };

    match result {
        Ok(payload) => {
            let mut response = Json(payload).into_response();
            *response.status_mut() = StatusCode::CREATED;
            response
        }
        Err(error) => json_error(error.status, &error.message),
    }
}

async fn handle_git_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    route_path: &str,
) -> Response {
    let method = request.method().clone();
    if method != Method::GET && auth.role != UserRole::Admin {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    let query = request.uri().query().map(str::to_string);
    let body = if matches!(
        method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    ) {
        match to_bytes(request.into_body(), usize::MAX)
            .await
            .context("failed to read git request body")
        {
            Ok(body) => Some(body),
            Err(_) => {
                return json_error(StatusCode::BAD_REQUEST, "Failed to read git request body.");
            }
        }
    } else {
        None
    };
    let payload = body
        .as_ref()
        .map(|body| serde_json::from_slice::<Value>(body).unwrap_or_else(|_| json!({})))
        .unwrap_or_else(|| json!({}));

    let result = if route_path == "/api/git/repositories" {
        if method != Method::GET {
            return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
        }
        list_git_repositories_payload(&state, true).await
    } else if route_path == "/api/git/status" {
        if method != Method::GET {
            return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
        }
        let Some(repo_path) = query_param_value(query.as_deref(), "repoPath") else {
            return json_error(StatusCode::BAD_REQUEST, "repoPath is required.");
        };
        get_git_status_payload(&state, &repo_path).await
    } else if route_path == "/api/git/file/resolve" {
        if method != Method::GET {
            return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
        }
        let Some(file_path) = query_param_value(query.as_deref(), "filePath") else {
            return json_error(StatusCode::BAD_REQUEST, "filePath is required.");
        };
        resolve_git_file_from_absolute_path_payload(&state, &file_path).await
    } else if route_path == "/api/git/file" {
        match method {
            Method::GET => {
                let Some(repo_path) = query_param_value(query.as_deref(), "repoPath") else {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        "repoPath and filePath are required.",
                    );
                };
                let Some(file_path) = query_param_value(query.as_deref(), "filePath") else {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        "repoPath and filePath are required.",
                    );
                };
                get_git_file_payload(&state, &repo_path, &file_path).await
            }
            Method::PUT => {
                let Some(repo_path) = payload.get("repoPath").and_then(Value::as_str) else {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        "repoPath, filePath, and content are required.",
                    );
                };
                let Some(file_path) = payload.get("filePath").and_then(Value::as_str) else {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        "repoPath, filePath, and content are required.",
                    );
                };
                let Some(content) = payload.get("content").and_then(Value::as_str) else {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        "repoPath, filePath, and content are required.",
                    );
                };
                save_git_file_payload(&state, repo_path, file_path, content).await
            }
            _ => return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed."),
        }
    } else if route_path == "/api/git/stage" {
        if method != Method::POST {
            return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
        }
        let Some(repo_path) = payload.get("repoPath").and_then(Value::as_str) else {
            return json_error(StatusCode::BAD_REQUEST, "repoPath is required.");
        };
        stage_git_changes_payload(
            &state,
            repo_path,
            payload.get("filePath").and_then(Value::as_str),
        )
        .await
    } else if route_path == "/api/git/unstage" {
        if method != Method::POST {
            return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
        }
        let Some(repo_path) = payload.get("repoPath").and_then(Value::as_str) else {
            return json_error(StatusCode::BAD_REQUEST, "repoPath is required.");
        };
        unstage_git_changes_payload(
            &state,
            repo_path,
            payload.get("filePath").and_then(Value::as_str),
        )
        .await
    } else if route_path == "/api/git/fetch" {
        if method != Method::POST {
            return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
        }
        let Some(repo_path) = payload.get("repoPath").and_then(Value::as_str) else {
            return json_error(StatusCode::BAD_REQUEST, "repoPath is required.");
        };
        fetch_git_repository_payload(&state, repo_path).await
    } else if route_path == "/api/git/pull" {
        if method != Method::POST {
            return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
        }
        let Some(repo_path) = payload.get("repoPath").and_then(Value::as_str) else {
            return json_error(StatusCode::BAD_REQUEST, "repoPath is required.");
        };
        pull_git_repository_payload(&state, repo_path).await
    } else if route_path == "/api/git/commit" {
        if method != Method::POST {
            return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
        }
        let Some(repo_path) = payload.get("repoPath").and_then(Value::as_str) else {
            return json_error(
                StatusCode::BAD_REQUEST,
                "repoPath and message are required.",
            );
        };
        let Some(message) = payload.get("message").and_then(Value::as_str) else {
            return json_error(
                StatusCode::BAD_REQUEST,
                "repoPath and message are required.",
            );
        };
        commit_git_changes_payload(&state, repo_path, message).await
    } else if route_path == "/api/git/checkout" {
        if method != Method::POST {
            return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
        }
        let Some(repo_path) = payload.get("repoPath").and_then(Value::as_str) else {
            return json_error(
                StatusCode::BAD_REQUEST,
                "repoPath and branchName are required.",
            );
        };
        let Some(branch_name) = payload.get("branchName").and_then(Value::as_str) else {
            return json_error(
                StatusCode::BAD_REQUEST,
                "repoPath and branchName are required.",
            );
        };
        checkout_git_branch_payload(
            &state,
            repo_path,
            branch_name,
            payload
                .get("create")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        )
        .await
    } else if route_path == "/api/git/worktrees" {
        match method {
            Method::GET => {
                let repo_path = query_param_value(query.as_deref(), "repoPath").unwrap_or_default();
                list_git_worktrees_payload(&state, &repo_path).await
            }
            Method::POST => {
                let repo_path = payload
                    .get("repoPath")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let worktree_path = payload
                    .get("worktreePath")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                create_git_worktree_payload(
                    &state,
                    repo_path,
                    worktree_path,
                    payload.get("branchName").and_then(Value::as_str),
                    payload
                        .get("createBranch")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    payload
                        .get("detach")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                )
                .await
            }
            Method::DELETE => {
                let repo_path = payload
                    .get("repoPath")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let worktree_path = payload
                    .get("worktreePath")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                remove_git_worktree_payload(
                    &state,
                    repo_path,
                    worktree_path,
                    payload
                        .get("force")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                )
                .await
            }
            _ => return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed."),
        }
    } else if route_path == "/api/git/commit/diff" {
        if method != Method::GET {
            return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
        }
        let Some(repo_path) = query_param_value(query.as_deref(), "repoPath") else {
            return json_error(
                StatusCode::BAD_REQUEST,
                "repoPath and commitHash are required.",
            );
        };
        let Some(commit_hash) = query_param_value(query.as_deref(), "commitHash") else {
            return json_error(
                StatusCode::BAD_REQUEST,
                "repoPath and commitHash are required.",
            );
        };
        get_git_commit_diff_payload(&state, &repo_path, &commit_hash).await
    } else if route_path == "/api/git/github/pulls" {
        if method != Method::GET {
            return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
        }
        let Some(repo_path) = query_param_value(query.as_deref(), "repoPath") else {
            return json_error(StatusCode::BAD_REQUEST, "repoPath is required.");
        };
        let pr_state =
            query_param_value(query.as_deref(), "state").unwrap_or_else(|| "open".to_string());
        let limit = query_param_value(query.as_deref(), "limit")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(20);
        list_github_pull_requests_payload(&state, &repo_path, &pr_state, limit).await
    } else if let Some(suffix) = route_path.strip_prefix("/api/git/github/pulls/") {
        if let Some(number_text) = suffix.strip_suffix("/checkout") {
            if method != Method::POST {
                return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
            }
            let Ok(number) = number_text.parse::<u64>() else {
                return json_error(
                    StatusCode::BAD_REQUEST,
                    "repoPath and pull request number are required.",
                );
            };
            let Some(repo_path) = payload.get("repoPath").and_then(Value::as_str) else {
                return json_error(
                    StatusCode::BAD_REQUEST,
                    "repoPath and pull request number are required.",
                );
            };
            checkout_github_pull_request_payload(&state, repo_path, number).await
        } else {
            if method != Method::GET {
                return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
            }
            let Ok(number) = suffix.parse::<u64>() else {
                return json_error(
                    StatusCode::BAD_REQUEST,
                    "repoPath and pull request number are required.",
                );
            };
            let Some(repo_path) = query_param_value(query.as_deref(), "repoPath") else {
                return json_error(
                    StatusCode::BAD_REQUEST,
                    "repoPath and pull request number are required.",
                );
            };
            get_github_pull_request_payload(&state, &repo_path, number).await
        }
    } else {
        return json_error(StatusCode::NOT_FOUND, "Not found.");
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

async fn handle_sessions_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
) -> Response {
    let query = request.uri().query().map(str::to_string);
    let result = match request.method() {
        &Method::GET => {
            let archived =
                query_param_value(query.as_deref(), "archived").as_deref() == Some("true");
            let cursor = query_param_value(query.as_deref(), "cursor");
            let limit = query_param_value(query.as_deref(), "limit")
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(20);
            let search_query = query_param_value(query.as_deref(), "query").unwrap_or_default();
            let scope = query_param_value(query.as_deref(), "scope")
                .unwrap_or_else(|| "summary".to_string());
            let filter = session_filter_from_query(query.as_deref());

            if search_query.trim().is_empty() {
                list_sessions_payload(
                    &state,
                    &auth.profile_id,
                    archived,
                    cursor.as_deref(),
                    limit,
                    &filter,
                )
                .await
            } else {
                search_sessions_payload(
                    &state,
                    &auth.profile_id,
                    &search_query,
                    &scope,
                    archived,
                    cursor.as_deref(),
                    limit,
                    &filter,
                )
                .await
            }
        }
        &Method::POST => {
            if auth.role != UserRole::Admin {
                return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
            }

            let body = to_bytes(request.into_body(), usize::MAX)
                .await
                .context("failed to read session create body");
            match body {
                Ok(body) => {
                    let payload: Value =
                        serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
                    create_session_payload(
                        &state,
                        &auth.profile_id,
                        payload
                            .get("preferences")
                            .cloned()
                            .unwrap_or_else(|| json!({})),
                        payload.get("name").and_then(Value::as_str),
                    )
                    .await
                }
                Err(_) => Err(api_error(
                    StatusCode::BAD_REQUEST,
                    "Failed to read session create body.",
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

async fn handle_session_api_http(
    state: AppState,
    session_id: &str,
    request: Request,
    auth: AuthContext,
) -> Response {
    let result = match request.method() {
        &Method::GET => {
            let limit = query_param_value(request.uri().query(), "limit")
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(20);
            session_detail_payload(&state, &auth.profile_id, session_id, limit).await
        }
        &Method::PATCH => {
            if auth.role != UserRole::Admin {
                return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
            }

            let body = to_bytes(request.into_body(), usize::MAX)
                .await
                .context("failed to read session update body");
            match body {
                Ok(body) => {
                    let payload: Value =
                        serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
                    save_session_preferences_payload(
                        &state,
                        &auth.profile_id,
                        session_id,
                        payload
                            .get("preferences")
                            .cloned()
                            .unwrap_or_else(|| json!({})),
                    )
                    .await
                }
                Err(_) => Err(api_error(
                    StatusCode::BAD_REQUEST,
                    "Failed to read session update body.",
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

async fn handle_session_fork_api_http(
    state: AppState,
    session_id: &str,
    request: Request,
    auth: AuthContext,
) -> Response {
    if request.method() != Method::POST {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }
    if auth.role != UserRole::Admin {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    let result = match to_bytes(request.into_body(), usize::MAX)
        .await
        .context("failed to read session fork body")
    {
        Ok(body) => {
            let payload: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
            fork_session_payload(
                &state,
                &auth.profile_id,
                session_id,
                payload
                    .get("mode")
                    .and_then(Value::as_str)
                    .unwrap_or("fork"),
                payload.get("turnId").and_then(Value::as_str),
                payload.get("messageText").and_then(Value::as_str),
            )
            .await
        }
        Err(_) => Err(api_error(
            StatusCode::BAD_REQUEST,
            "Failed to read session fork body.",
        )),
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

async fn handle_session_organization_api_http(
    state: AppState,
    session_id: &str,
    request: Request,
    auth: AuthContext,
) -> Response {
    if request.method() != Method::PATCH {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }
    if auth.role != UserRole::Admin {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    let result = match to_bytes(request.into_body(), usize::MAX)
        .await
        .context("failed to read session organization body")
    {
        Ok(body) => {
            let payload: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
            update_session_organization_payload(&state, &auth.profile_id, session_id, payload).await
        }
        Err(_) => Err(api_error(
            StatusCode::BAD_REQUEST,
            "Failed to read session organization body.",
        )),
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

async fn handle_session_name_api_http(
    state: AppState,
    session_id: &str,
    request: Request,
    auth: AuthContext,
) -> Response {
    if request.method() != Method::POST {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }
    if auth.role != UserRole::Admin {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    let result = match to_bytes(request.into_body(), usize::MAX)
        .await
        .context("failed to read session name body")
    {
        Ok(body) => {
            let payload: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
            rename_session_payload(
                &state,
                &auth.profile_id,
                session_id,
                payload
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
            .await
        }
        Err(_) => Err(api_error(
            StatusCode::BAD_REQUEST,
            "Failed to read session name body.",
        )),
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

async fn handle_session_archive_api_http(
    state: AppState,
    session_id: &str,
    request: Request,
    auth: AuthContext,
    archived: bool,
) -> Response {
    if request.method() != Method::POST {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }
    if auth.role != UserRole::Admin {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    let result = if archived {
        archive_session_payload(&state, &auth.profile_id, session_id).await
    } else {
        unarchive_session_payload(&state, &auth.profile_id, session_id).await
    };

    match result {
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

async fn handle_catalog_api_http(state: AppState, request: Request, auth: AuthContext) -> Response {
    if request.method() != Method::GET {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }

    match get_catalog_payload(&state, &auth.profile_id).await {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

async fn handle_notifications_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
) -> Response {
    let result = match request.method() {
        &Method::GET => {
            let limit = query_param_value(request.uri().query(), "limit")
                .and_then(|value| value.parse::<usize>().ok())
                .map(|value| value.clamp(1, 200))
                .unwrap_or(DEFAULT_NOTIFICATION_LIMIT);
            get_notifications_payload(&state, &auth.profile_id, limit).await
        }
        &Method::PATCH => {
            if auth.role != UserRole::Admin {
                return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
            }
            let body = to_bytes(request.into_body(), usize::MAX)
                .await
                .context("failed to read notifications request body");
            match body {
                Ok(body) => {
                    let payload: Value =
                        serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
                    let ids = payload.get("ids").and_then(Value::as_array).map(|values| {
                        values
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    });
                    mark_notifications_read_payload(&state, &auth.profile_id, ids).await
                }
                Err(_) => Err(api_error(
                    StatusCode::BAD_REQUEST,
                    "Failed to read notifications request body.",
                )),
            }
        }
        &Method::DELETE => {
            if auth.role != UserRole::Admin {
                return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
            }
            clear_notifications_payload(&state, &auth.profile_id).await
        }
        _ => return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed."),
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

async fn handle_notification_settings_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
) -> Response {
    if request.method() != Method::PATCH {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }
    if auth.role != UserRole::Admin {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .context("failed to read notification settings request body");
    let result = match body {
        Ok(body) => {
            let payload: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
            update_notification_settings_payload(&state, &auth.profile_id, payload).await
        }
        Err(_) => Err(api_error(
            StatusCode::BAD_REQUEST,
            "Failed to read notification settings request body.",
        )),
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

async fn handle_session_filters_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
) -> Response {
    if auth.role != UserRole::Admin {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    let result = match request.method() {
        &Method::POST => {
            let body = to_bytes(request.into_body(), usize::MAX)
                .await
                .context("failed to read session filters request body");
            match body {
                Ok(body) => {
                    let payload: Value =
                        serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
                    save_session_filter_payload(
                        &state,
                        &auth.profile_id,
                        payload.get("filter").cloned().unwrap_or_else(|| json!({})),
                    )
                    .await
                }
                Err(_) => Err(api_error(
                    StatusCode::BAD_REQUEST,
                    "Failed to read session filters request body.",
                )),
            }
        }
        &Method::DELETE => {
            let filter_id =
                query_param_value(request.uri().query(), "filterId").unwrap_or_default();
            delete_session_filter_payload(&state, &auth.profile_id, &filter_id).await
        }
        _ => return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed."),
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

async fn handle_prompt_presets_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
) -> Response {
    if auth.role != UserRole::Admin {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    let result = match request.method() {
        &Method::POST => {
            let body = to_bytes(request.into_body(), usize::MAX)
                .await
                .context("failed to read prompt presets request body");
            match body {
                Ok(body) => {
                    let payload: Value =
                        serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
                    save_prompt_preset_payload(
                        &state,
                        &auth.profile_id,
                        payload.get("preset").cloned().unwrap_or_else(|| json!({})),
                    )
                    .await
                }
                Err(_) => Err(api_error(
                    StatusCode::BAD_REQUEST,
                    "Failed to read prompt presets request body.",
                )),
            }
        }
        &Method::DELETE => {
            let preset_id =
                query_param_value(request.uri().query(), "presetId").unwrap_or_default();
            delete_prompt_preset_payload(&state, &auth.profile_id, &preset_id).await
        }
        _ => return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed."),
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

async fn handle_session_draft_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    session_id: &str,
) -> Response {
    let result = match request.method() {
        &Method::GET => get_session_draft_payload(&state, &auth.profile_id, session_id).await,
        &Method::PATCH => {
            if auth.role != UserRole::Admin {
                return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
            }
            let body = to_bytes(request.into_body(), usize::MAX)
                .await
                .context("failed to read draft request body");
            match body {
                Ok(body) => {
                    let payload: Value =
                        serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
                    save_session_draft_payload(
                        &state,
                        &auth.profile_id,
                        session_id,
                        payload
                            .get("draft")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                        payload
                            .get("intent")
                            .and_then(Value::as_str)
                            .unwrap_or("message"),
                    )
                    .await
                }
                Err(_) => Err(api_error(
                    StatusCode::BAD_REQUEST,
                    "Failed to read draft request body.",
                )),
            }
        }
        &Method::DELETE => {
            if auth.role != UserRole::Admin {
                return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
            }
            clear_session_draft_payload(&state, &auth.profile_id, session_id).await
        }
        _ => return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed."),
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

async fn handle_session_messages_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    session_id: &str,
) -> Response {
    if request.method() != Method::POST {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }
    if auth.role != UserRole::Admin {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    let result = match to_bytes(request.into_body(), usize::MAX)
        .await
        .context("failed to read session message body")
    {
        Ok(body) => {
            let payload: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
            send_turn_payload(
                &state,
                &auth.profile_id,
                session_id,
                payload
                    .get("prompt")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                payload.get("attachmentIds"),
                payload
                    .get("preferences")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
            )
            .await
        }
        Err(_) => Err(api_error(
            StatusCode::BAD_REQUEST,
            "Failed to read session message body.",
        )),
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

async fn handle_session_steer_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    session_id: &str,
) -> Response {
    if request.method() != Method::POST {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }
    if auth.role != UserRole::Admin {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    let result = match to_bytes(request.into_body(), usize::MAX)
        .await
        .context("failed to read session steer body")
    {
        Ok(body) => {
            let payload: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
            steer_turn_payload(
                &state,
                &auth.profile_id,
                session_id,
                payload
                    .get("prompt")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                payload.get("attachmentIds"),
            )
            .await
        }
        Err(_) => Err(api_error(
            StatusCode::BAD_REQUEST,
            "Failed to read session steer body.",
        )),
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

async fn handle_session_attachments_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    session_id: &str,
) -> Response {
    let method = request.method().clone();
    match method {
        Method::GET => {
            match list_session_attachments_payload(&state, &auth.profile_id, session_id).await {
                Ok(attachments) => Json(json!({ "attachments": attachments })).into_response(),
                Err(error) => json_error(error.status, &error.message),
            }
        }
        Method::POST => {
            if auth.role != UserRole::Admin {
                return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
            }

            let multipart = match Multipart::from_request(request, &()).await {
                Ok(multipart) => multipart,
                Err(_) => {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        "Failed to read attachment upload body.",
                    );
                }
            };
            let mut multipart = multipart;
            let mut uploads = Vec::new();

            loop {
                let field = match multipart.next_field().await {
                    Ok(Some(field)) => field,
                    Ok(None) => break,
                    Err(_) => {
                        return json_error(
                            StatusCode::BAD_REQUEST,
                            "Failed to read attachment upload body.",
                        );
                    }
                };

                if field.name() != Some("files") {
                    continue;
                }

                let file_name = field
                    .file_name()
                    .map(str::to_string)
                    .unwrap_or_else(|| "attachment".to_string());
                let mime_type = field.content_type().map(str::to_string);
                let bytes = match field.bytes().await {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        return json_error(
                            StatusCode::BAD_REQUEST,
                            "Failed to read attachment upload body.",
                        );
                    }
                };
                if bytes.is_empty() {
                    continue;
                }

                uploads.push(AttachmentUploadPayload {
                    name: file_name,
                    mime_type,
                    bytes: bytes.to_vec(),
                });
            }

            match save_uploaded_attachment_records(&state, &auth.profile_id, session_id, uploads)
                .await
            {
                Ok(stored) => {
                    if let Err(error) =
                        emit_attachments_updated(&state, &auth.profile_id, session_id).await
                    {
                        return json_error(error.status, &error.message);
                    }
                    let mut response = Json(json!({
                        "attachments": stored
                            .iter()
                            .map(attachment_payload_from_record)
                            .collect::<Vec<_>>()
                    }))
                    .into_response();
                    *response.status_mut() = StatusCode::CREATED;
                    response
                }
                Err(error) => json_error(error.status, &error.message),
            }
        }
        _ => json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed."),
    }
}

async fn handle_session_attachment_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    session_id: &str,
    attachment_id: &str,
) -> Response {
    if request.method() != Method::DELETE {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }
    if auth.role != UserRole::Admin {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    match delete_attachment_payload(&state, &auth.profile_id, session_id, attachment_id).await {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

async fn handle_session_queue_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    session_id: &str,
    route_path: &str,
) -> Response {
    let queue_prefix = format!("/api/sessions/{session_id}/queue");
    let suffix = route_path.strip_prefix(&queue_prefix).unwrap_or_default();
    let requires_admin = request.method() != Method::GET;
    if requires_admin && auth.role != UserRole::Admin {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    let result = if suffix.is_empty() {
        match request.method() {
            &Method::GET => get_session_queue_payload(&state, &auth.profile_id, session_id).await,
            &Method::POST => {
                let body = to_bytes(request.into_body(), usize::MAX)
                    .await
                    .context("failed to read queue request body");
                match body {
                    Ok(body) => {
                        let payload: Value =
                            serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
                        enqueue_session_queue_payload(
                            &state,
                            &auth.profile_id,
                            session_id,
                            payload
                                .get("prompt")
                                .and_then(Value::as_str)
                                .unwrap_or_default(),
                            payload.get("attachmentIds"),
                        )
                        .await
                    }
                    Err(_) => Err(api_error(
                        StatusCode::BAD_REQUEST,
                        "Failed to read queue request body.",
                    )),
                }
            }
            _ => Err(api_error(
                StatusCode::METHOD_NOT_ALLOWED,
                "Method not allowed.",
            )),
        }
    } else if suffix == "/resume" {
        if request.method() != Method::POST {
            Err(api_error(
                StatusCode::METHOD_NOT_ALLOWED,
                "Method not allowed.",
            ))
        } else {
            resume_session_queue_payload(&state, &auth.profile_id, session_id).await
        }
    } else if suffix == "/reorder" {
        if request.method() != Method::POST {
            Err(api_error(
                StatusCode::METHOD_NOT_ALLOWED,
                "Method not allowed.",
            ))
        } else {
            let body = to_bytes(request.into_body(), usize::MAX)
                .await
                .context("failed to read queue reorder request body");
            match body {
                Ok(body) => {
                    let payload: Value =
                        serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
                    let queue_ids = string_array_from_value(payload.get("queueIds"));
                    reorder_session_queue_payload(&state, &auth.profile_id, session_id, &queue_ids)
                        .await
                }
                Err(_) => Err(api_error(
                    StatusCode::BAD_REQUEST,
                    "Failed to read queue reorder request body.",
                )),
            }
        }
    } else {
        let queue_id = suffix.trim_start_matches('/');
        if queue_id.is_empty() || queue_id.contains('/') {
            Err(api_error(StatusCode::NOT_FOUND, "Not found."))
        } else {
            match request.method() {
                &Method::DELETE => {
                    remove_session_queue_item_payload(
                        &state,
                        &auth.profile_id,
                        session_id,
                        queue_id,
                    )
                    .await
                }
                &Method::PATCH => {
                    let body = to_bytes(request.into_body(), usize::MAX)
                        .await
                        .context("failed to read queue update request body");
                    match body {
                        Ok(body) => {
                            let payload: Value =
                                serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
                            update_session_queue_item_payload(
                                &state,
                                &auth.profile_id,
                                session_id,
                                queue_id,
                                payload.get("prompt").and_then(Value::as_str),
                                payload.get("attachmentIds"),
                            )
                            .await
                        }
                        Err(_) => Err(api_error(
                            StatusCode::BAD_REQUEST,
                            "Failed to read queue update request body.",
                        )),
                    }
                }
                &Method::POST => {
                    let body = to_bytes(request.into_body(), usize::MAX)
                        .await
                        .context("failed to read queue dispatch request body");
                    match body {
                        Ok(body) => {
                            let payload: Value =
                                serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
                            dispatch_session_queue_item_payload(
                                &state,
                                &auth.profile_id,
                                session_id,
                                queue_id,
                                payload
                                    .get("mode")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default(),
                            )
                            .await
                        }
                        Err(_) => Err(api_error(
                            StatusCode::BAD_REQUEST,
                            "Failed to read queue dispatch request body.",
                        )),
                    }
                }
                _ => Err(api_error(
                    StatusCode::METHOD_NOT_ALLOWED,
                    "Method not allowed.",
                )),
            }
        }
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

async fn handle_session_abort_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    session_id: &str,
) -> Response {
    if request.method() != Method::POST {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }
    if auth.role != UserRole::Admin {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    match abort_turn_payload(&state, &auth.profile_id, session_id).await {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

async fn handle_session_search_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    session_id: &str,
) -> Response {
    if request.method() != Method::GET {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }

    let query = query_param_value(request.uri().query(), "query").unwrap_or_default();
    let cursor = query_param_value(request.uri().query(), "cursor");
    let limit = query_param_value(request.uri().query(), "limit")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(20);
    match search_session_turns_payload(
        &state,
        &auth.profile_id,
        session_id,
        &query,
        cursor.as_deref(),
        limit,
    )
    .await
    {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

async fn handle_session_turns_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    session_id: &str,
) -> Response {
    if request.method() != Method::GET {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }

    let before_turn_id =
        query_param_value(request.uri().query(), "beforeTurnId").unwrap_or_default();
    let limit = query_param_value(request.uri().query(), "limit")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(20);
    match session_older_turns_payload(&state, &auth.profile_id, session_id, &before_turn_id, limit)
        .await
    {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

async fn handle_session_turn_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    session_id: &str,
    turn_id: &str,
) -> Response {
    if request.method() != Method::GET {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }

    match session_turn_payload(&state, &auth.profile_id, session_id, turn_id).await {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

async fn handle_session_item_detail_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    session_id: &str,
    turn_id: &str,
    item_id: &str,
) -> Response {
    if request.method() != Method::GET {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }

    match session_item_detail_payload(&state, &auth.profile_id, session_id, turn_id, item_id).await
    {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

async fn handle_session_approval_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    session_id: &str,
) -> Response {
    if request.method() != Method::POST {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }
    if auth.role != UserRole::Admin {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    let result = match to_bytes(request.into_body(), usize::MAX)
        .await
        .context("failed to read approval request body")
    {
        Ok(body) => {
            let payload: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
            let request_id = payload
                .get("requestId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            resolve_server_request_payload(
                &state,
                &auth.profile_id,
                session_id,
                &request_id,
                payload.get("result").cloned().unwrap_or(Value::Null),
            )
            .await
        }
        Err(_) => Err(api_error(
            StatusCode::BAD_REQUEST,
            "Failed to read approval request body.",
        )),
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

async fn handle_session_recovery_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    session_id: &str,
) -> Response {
    let app_error_response = |status: StatusCode, code: &str, message: &str| {
        let mut response = Json(json!({
            "code": code,
            "message": message,
            "status": status.as_u16()
        }))
        .into_response();
        *response.status_mut() = status;
        response
    };

    if request.method() != Method::POST {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }
    if auth.role != UserRole::Admin {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    let thread = match read_thread_payload(&state, &auth.profile_id, session_id, false).await {
        Ok(thread) => thread,
        Err(error) => return json_error(error.status, &error.message),
    };
    let Some(rollout_path) = resolve_rollout_path(&state, &auth.profile_id, session_id, &thread)
    else {
        return app_error_response(
            StatusCode::NOT_FOUND,
            "SESSION_ROLLOUT_NOT_FOUND",
            "No persisted rollout file was found for this session.",
        );
    };
    let rollout_buffer = match tokio_fs::read(&rollout_path).await {
        Ok(buffer) => buffer,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return app_error_response(
                StatusCode::NOT_FOUND,
                "SESSION_ROLLOUT_NOT_FOUND",
                "No persisted rollout file was found for this session.",
            );
        }
        Err(error) => {
            return json_error(StatusCode::BAD_GATEWAY, &error.to_string());
        }
    };
    let plan = inspect_rollout_recovery_content(&rollout_buffer);
    if !plan.info.available
        || plan.info.recoverable_lines == 0
        || plan.recovered_content.trim().is_empty()
    {
        return app_error_response(
            StatusCode::CONFLICT,
            "SESSION_ROLLOUT_NOT_RECOVERABLE",
            "This session history could not be recovered automatically.",
        );
    }

    let backup_path = PathBuf::from(format!("{}.bak-{}", rollout_path.display(), now_unix_ms()));
    if let Err(error) = tokio_fs::copy(&rollout_path, &backup_path).await {
        return json_error(StatusCode::BAD_GATEWAY, &error.to_string());
    }
    if let Err(error) = tokio_fs::write(&rollout_path, plan.recovered_content.as_bytes()).await {
        return json_error(StatusCode::BAD_GATEWAY, &error.to_string());
    }

    append_runtime_error_log(
        &state.config,
        "rust-gateway",
        "recovered corrupted rollout",
        json!({
            "threadId": session_id,
            "rolloutPath": rollout_path.display().to_string(),
            "backupPath": backup_path.display().to_string(),
            "recovery": plan.info
        }),
    );

    Json(json!({
        "ok": true,
        "sessionId": session_id,
        "backupPath": backup_path.display().to_string(),
        "recoveredAt": now_unix_ms(),
        "totalLines": plan.info.total_lines,
        "recoveredLines": plan.info.recoverable_lines,
        "skippedLines": plan.info.skipped_lines
    }))
    .into_response()
}

async fn handle_automations_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    route_path: &str,
) -> Response {
    if auth.role != UserRole::Admin {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    let result = if route_path == "/api/automations" {
        match request.method() {
            &Method::POST => {
                let body = to_bytes(request.into_body(), usize::MAX)
                    .await
                    .context("failed to read automations request body");
                match body {
                    Ok(body) => {
                        let payload: Value =
                            serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
                        save_automation_payload(
                            &state,
                            &auth.profile_id,
                            payload
                                .get("automation")
                                .cloned()
                                .unwrap_or_else(|| json!({})),
                        )
                        .await
                    }
                    Err(_) => Err(api_error(
                        StatusCode::BAD_REQUEST,
                        "Failed to read automations request body.",
                    )),
                }
            }
            &Method::DELETE => {
                let automation_id =
                    query_param_value(request.uri().query(), "automationId").unwrap_or_default();
                delete_automation_payload(&state, &auth.profile_id, &automation_id).await
            }
            _ => return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed."),
        }
    } else if request.method() == Method::POST && route_path.ends_with("/run") {
        let automation_id = route_path
            .strip_prefix("/api/automations/")
            .and_then(|suffix| suffix.strip_suffix("/run"))
            .unwrap_or_default()
            .trim()
            .to_string();
        let body = to_bytes(request.into_body(), usize::MAX)
            .await
            .context("failed to read automation run request body");
        match body {
            Ok(body) => {
                let payload: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
                let trigger = if payload.get("trigger").and_then(Value::as_str) == Some("schedule")
                {
                    "schedule"
                } else {
                    "manual"
                };
                run_automation_payload(&state, &auth.profile_id, &automation_id, trigger).await
            }
            Err(_) => Err(api_error(
                StatusCode::BAD_REQUEST,
                "Failed to read automation run request body.",
            )),
        }
    } else {
        return json_error(StatusCode::NOT_FOUND, "Not found.");
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

async fn handle_arena_api_http(state: AppState, request: Request, auth: AuthContext) -> Response {
    let result = match request.method() {
        &Method::GET => list_arena_runs_payload(&state, &auth.profile_id).await,
        &Method::POST => {
            if auth.role != UserRole::Admin {
                return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
            }
            let body = to_bytes(request.into_body(), usize::MAX)
                .await
                .context("failed to read arena request body");
            match body {
                Ok(body) => {
                    let payload: Value =
                        serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
                    start_arena_run_payload(
                        &state,
                        &auth.profile_id,
                        payload
                            .get("prompt")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                        payload.get("contestants").unwrap_or(&Value::Null),
                        payload.get("preferences").unwrap_or(&Value::Null),
                    )
                    .await
                }
                Err(_) => Err(api_error(
                    StatusCode::BAD_REQUEST,
                    "Failed to read arena request body.",
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

fn encode_sse_event(event: &str, payload: &Value) -> Bytes {
    let body = serde_json::to_string(payload).unwrap_or_else(|_| "null".to_string());
    Bytes::from(format!("event: {event}\ndata: {body}\n\n"))
}

fn sse_response(receiver: broadcast::Receiver<Value>, ready_payload: Value) -> Response {
    struct SseState {
        ready: Option<Bytes>,
        receiver: broadcast::Receiver<Value>,
        keepalive: Pin<Box<tokio::time::Sleep>>,
    }

    let stream = futures_util::stream::unfold(
        SseState {
            ready: Some(encode_sse_event("ready", &ready_payload)),
            receiver,
            keepalive: Box::pin(tokio::time::sleep(Duration::from_secs(15))),
        },
        |mut state| async move {
            if let Some(ready) = state.ready.take() {
                return Some((Ok::<Bytes, Infallible>(ready), state));
            }

            loop {
                tokio::select! {
                    _ = &mut state.keepalive => {
                        state.keepalive.as_mut().reset(tokio::time::Instant::now() + Duration::from_secs(15));
                        return Some((Ok(Bytes::from_static(b": ping\n\n")), state));
                    }
                    result = state.receiver.recv() => {
                        match result {
                            Ok(event) => {
                                state.keepalive.as_mut().reset(tokio::time::Instant::now() + Duration::from_secs(15));
                                return Some((Ok(encode_sse_event("message", &event)), state));
                            }
                            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                                warn!("sse relay lagged: skipped {skipped} messages");
                            }
                            Err(broadcast::error::RecvError::Closed) => return None,
                        }
                    }
                }
            }
        },
    );

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CACHE_CONTROL, "no-cache, no-transform")
        .header(header::CONNECTION, "keep-alive")
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header("x-accel-buffering", "no")
        .body(Body::from_stream(stream))
        .unwrap_or_else(|error| json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()))
}

async fn handle_events_stream_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
) -> Response {
    if request.method() != Method::GET {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }

    match ensure_global_relay(&state, &auth.profile_id).await {
        Ok(relay) => sse_response(relay.subscribe(), json!({ "scope": "global" })),
        Err(error) => json_error(StatusCode::BAD_GATEWAY, &error.to_string()),
    }
}

async fn handle_session_stream_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    session_id: &str,
) -> Response {
    if request.method() != Method::GET {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }

    match ensure_stream_relay(&state, &auth.profile_id, session_id).await {
        Ok(relay) => sse_response(relay.subscribe(), json!({ "threadId": session_id })),
        Err(error) => json_error(StatusCode::BAD_GATEWAY, &error.to_string()),
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

    match method {
        "config/get" => get_config_payload(state, &auth.profile_id)
            .await
            .map_err(anyhow::Error::from),
        "config/update" => update_config_payload(state, &auth.profile_id, params)
            .await
            .map_err(anyhow::Error::from),
        "notifications/list" => {
            let limit = params
                .get("limit")
                .and_then(Value::as_u64)
                .map(|value| value.clamp(1, 200) as usize)
                .unwrap_or(DEFAULT_NOTIFICATION_LIMIT);
            get_notifications_payload(state, &auth.profile_id, limit)
                .await
                .map_err(anyhow::Error::from)
        }
        "audit/list" => {
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(120) as usize;
            list_audit_log(&state.config, limit).await
        }
        "notifications/markRead" => {
            let ids = params.get("ids").and_then(Value::as_array).map(|entries| {
                entries
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            });
            mark_notifications_read_payload(state, &auth.profile_id, ids)
                .await
                .map_err(anyhow::Error::from)
        }
        "notifications/clear" => clear_notifications_payload(state, &auth.profile_id)
            .await
            .map_err(anyhow::Error::from),
        "notifications/settings/update" => {
            let payload = json!({
                "enabledEventTypes": params.get("enabledEventTypes").cloned().unwrap_or(Value::Null),
                "slackWebhookUrl": params.get("slackWebhookUrl").cloned().unwrap_or(Value::Null),
                "webhookUrl": params.get("webhookUrl").cloned().unwrap_or(Value::Null)
            });
            update_notification_settings_payload(state, &auth.profile_id, payload)
                .await
                .map_err(anyhow::Error::from)
        }
        "automations/save" => save_automation_payload(
            state,
            &auth.profile_id,
            params.get("automation").cloned().unwrap_or(Value::Null),
        )
        .await
        .map_err(anyhow::Error::from),
        "automations/delete" => {
            let automation_id = require_string(&params, "automationId")?;
            delete_automation_payload(state, &auth.profile_id, &automation_id)
                .await
                .map_err(anyhow::Error::from)
        }
        "automations/run" => {
            let automation_id = require_string(&params, "automationId")?;
            let trigger = if params.get("trigger").and_then(Value::as_str) == Some("schedule") {
                "schedule"
            } else {
                "manual"
            };
            run_automation_payload(state, &auth.profile_id, &automation_id, trigger)
                .await
                .map_err(anyhow::Error::from)
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
        "catalog/get" => get_catalog_payload(state, &auth.profile_id)
            .await
            .map_err(anyhow::Error::from),
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
                .filter(|value| !value.is_empty());
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(20);
            let filter = session_filter_from_value(params.get("filter"));
            list_sessions_payload(state, &auth.profile_id, archived, cursor, limit, &filter)
                .await
                .map_err(anyhow::Error::from)
        }
        "sessions/search" => {
            let archived = params
                .get("archived")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let query_raw = require_string(&params, "query")?;
            let scope = if params.get("scope").and_then(Value::as_str) == Some("full") {
                "full"
            } else {
                "summary"
            };
            let cursor = params
                .get("cursor")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty());
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(20);
            let filter = session_filter_from_value(params.get("filter"));
            search_sessions_payload(
                state,
                &auth.profile_id,
                &query_raw,
                scope,
                archived,
                cursor,
                limit,
                &filter,
            )
            .await
            .map_err(anyhow::Error::from)
        }
        "session/create" => create_session_payload(
            state,
            &auth.profile_id,
            params
                .get("preferences")
                .cloned()
                .unwrap_or_else(|| json!({})),
            params.get("name").and_then(Value::as_str),
        )
        .await
        .map_err(anyhow::Error::from),
        "session/organization/update" => {
            let session_id = require_string(&params, "sessionId")?;
            update_session_organization_payload(state, &auth.profile_id, &session_id, params)
                .await
                .map_err(anyhow::Error::from)
        }
        "sessionFilters/save" => save_session_filter_payload(
            state,
            &auth.profile_id,
            params.get("filter").cloned().unwrap_or(Value::Null),
        )
        .await
        .map_err(anyhow::Error::from),
        "sessionFilters/delete" => {
            let filter_id = require_string(&params, "filterId")?;
            delete_session_filter_payload(state, &auth.profile_id, &filter_id)
                .await
                .map_err(anyhow::Error::from)
        }
        "promptPresets/save" => save_prompt_preset_payload(
            state,
            &auth.profile_id,
            params.get("preset").cloned().unwrap_or(Value::Null),
        )
        .await
        .map_err(anyhow::Error::from),
        "promptPresets/delete" => {
            let preset_id = require_string(&params, "presetId")?;
            delete_prompt_preset_payload(state, &auth.profile_id, &preset_id)
                .await
                .map_err(anyhow::Error::from)
        }
        "session/get" => {
            let session_id = require_string(&params, "sessionId")?;
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(20);
            session_detail_payload(state, &auth.profile_id, &session_id, limit)
                .await
                .map_err(anyhow::Error::from)
        }
        "session/fork" => {
            let session_id = require_string(&params, "sessionId")?;
            fork_session_payload(
                state,
                &auth.profile_id,
                &session_id,
                params.get("mode").and_then(Value::as_str).unwrap_or("fork"),
                params.get("turnId").and_then(Value::as_str),
                params.get("messageText").and_then(Value::as_str),
            )
            .await
            .map_err(anyhow::Error::from)
        }
        "session/search" => {
            let session_id = require_string(&params, "sessionId")?;
            let query_raw = require_string(&params, "query")?;
            let cursor = params
                .get("cursor")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty());
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(20);
            search_session_turns_payload(
                state,
                &auth.profile_id,
                &session_id,
                &query_raw,
                cursor,
                limit,
            )
            .await
            .map_err(anyhow::Error::from)
        }
        "session/olderTurns/get" => {
            let session_id = require_string(&params, "sessionId")?;
            let before_turn_id = require_string(&params, "beforeTurnId")?;
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(20);
            session_older_turns_payload(
                state,
                &auth.profile_id,
                &session_id,
                &before_turn_id,
                limit,
            )
            .await
            .map_err(anyhow::Error::from)
        }
        "session/turn/get" => {
            let session_id = require_string(&params, "sessionId")?;
            let turn_id = require_string(&params, "turnId")?;
            session_turn_payload(state, &auth.profile_id, &session_id, &turn_id)
                .await
                .map_err(anyhow::Error::from)
        }
        "session/itemDetail/get" => {
            let session_id = require_string(&params, "sessionId")?;
            let turn_id = require_string(&params, "turnId")?;
            let item_id = require_string(&params, "itemId")?;
            session_item_detail_payload(state, &auth.profile_id, &session_id, &turn_id, &item_id)
                .await
                .map_err(anyhow::Error::from)
        }
        "session/draft/get" => {
            let session_id = require_string(&params, "sessionId")?;
            get_session_draft_payload(state, &auth.profile_id, &session_id)
                .await
                .map_err(anyhow::Error::from)
        }
        "session/draft/save" => {
            let session_id = require_string(&params, "sessionId")?;
            save_session_draft_payload(
                state,
                &auth.profile_id,
                &session_id,
                params
                    .get("draft")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                params
                    .get("intent")
                    .and_then(Value::as_str)
                    .unwrap_or("message"),
            )
            .await
            .map_err(anyhow::Error::from)
        }
        "session/draft/clear" => {
            let session_id = require_string(&params, "sessionId")?;
            clear_session_draft_payload(state, &auth.profile_id, &session_id)
                .await
                .map_err(anyhow::Error::from)
        }
        "session/queue/get" => {
            let session_id = require_string(&params, "sessionId")?;
            get_session_queue_payload(state, &auth.profile_id, &session_id)
                .await
                .map_err(anyhow::Error::from)
        }
        "session/queue/enqueue" => {
            let session_id = require_string(&params, "sessionId")?;
            enqueue_session_queue_payload(
                state,
                &auth.profile_id,
                &session_id,
                params
                    .get("prompt")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                params.get("attachmentIds"),
            )
            .await
            .map_err(anyhow::Error::from)
        }
        "session/queue/resume" => {
            let session_id = require_string(&params, "sessionId")?;
            resume_session_queue_payload(state, &auth.profile_id, &session_id)
                .await
                .map_err(anyhow::Error::from)
        }
        "session/queue/remove" => {
            let session_id = require_string(&params, "sessionId")?;
            let queue_id = require_string(&params, "queueId")?;
            remove_session_queue_item_payload(state, &auth.profile_id, &session_id, &queue_id)
                .await
                .map_err(anyhow::Error::from)
        }
        "session/queue/update" => {
            let session_id = require_string(&params, "sessionId")?;
            let queue_id = require_string(&params, "queueId")?;
            update_session_queue_item_payload(
                state,
                &auth.profile_id,
                &session_id,
                &queue_id,
                params.get("prompt").and_then(Value::as_str),
                params.get("attachmentIds"),
            )
            .await
            .map_err(anyhow::Error::from)
        }
        "session/queue/reorder" => {
            let session_id = require_string(&params, "sessionId")?;
            let queue_ids = string_array_from_value(params.get("queueIds"));
            reorder_session_queue_payload(state, &auth.profile_id, &session_id, &queue_ids)
                .await
                .map_err(anyhow::Error::from)
        }
        "session/queue/dispatch" => {
            let session_id = require_string(&params, "sessionId")?;
            let queue_id = require_string(&params, "queueId")?;
            dispatch_session_queue_item_payload(
                state,
                &auth.profile_id,
                &session_id,
                &queue_id,
                &require_string(&params, "mode")?,
            )
            .await
            .map_err(anyhow::Error::from)
        }
        "session/savePreferences" => {
            let session_id = require_string(&params, "sessionId")?;
            save_session_preferences_payload(
                state,
                &auth.profile_id,
                &session_id,
                params
                    .get("preferences")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
            )
            .await
            .map_err(anyhow::Error::from)
        }
        "session/rename" => {
            let session_id = require_string(&params, "sessionId")?;
            rename_session_payload(
                state,
                &auth.profile_id,
                &session_id,
                &require_string(&params, "name")?,
            )
            .await
            .map_err(anyhow::Error::from)
        }
        "session/archive" => {
            let session_id = require_string(&params, "sessionId")?;
            archive_session_payload(state, &auth.profile_id, &session_id)
                .await
                .map_err(anyhow::Error::from)
        }
        "session/unarchive" => {
            let session_id = require_string(&params, "sessionId")?;
            unarchive_session_payload(state, &auth.profile_id, &session_id)
                .await
                .map_err(anyhow::Error::from)
        }
        "turn/send" => {
            let session_id = require_string(&params, "sessionId")?;
            send_turn_payload(
                state,
                &auth.profile_id,
                &session_id,
                params
                    .get("prompt")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                params.get("attachmentIds"),
                params
                    .get("preferences")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
            )
            .await
            .map_err(anyhow::Error::from)
        }
        "turn/steer" => {
            let session_id = require_string(&params, "sessionId")?;
            let prompt = require_string(&params, "prompt")?;
            steer_turn_payload(
                state,
                &auth.profile_id,
                &session_id,
                &prompt,
                params.get("attachmentIds"),
            )
            .await
            .map_err(anyhow::Error::from)
        }
        "turn/abort" => {
            let session_id = require_string(&params, "sessionId")?;
            abort_turn_payload(state, &auth.profile_id, &session_id)
                .await
                .map_err(anyhow::Error::from)
        }
        "approval/resolve" => {
            let session_id = require_string(&params, "sessionId")?;
            let request_id = require_string(&params, "requestId")?;
            resolve_server_request_payload(
                state,
                &auth.profile_id,
                &session_id,
                &request_id,
                params.get("result").cloned().unwrap_or(Value::Null),
            )
            .await
            .map_err(anyhow::Error::from)
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
            upload_attachments(state, &auth.profile_id, &session_id, files)
                .await
                .map_err(anyhow::Error::from)
        }
        "attachments/delete" => {
            let session_id = require_string(&params, "sessionId")?;
            let attachment_id = require_string(&params, "attachmentId")?;
            delete_attachment_payload(state, &auth.profile_id, &session_id, &attachment_id)
                .await
                .map_err(anyhow::Error::from)
        }
        "account/get" => get_account_state(state, &auth.profile_id).await,
        "account/login/start" => start_account_login(state, &auth.profile_id, &params).await,
        "account/login/cancel" => cancel_account_login(state, &auth.profile_id, &params).await,
        "account/logout" => logout_account(state, &auth.profile_id).await,
        "arena/list" => list_arena_runs_payload(state, &auth.profile_id)
            .await
            .map_err(anyhow::Error::from),
        "arena/start" => start_arena_run_payload(
            state,
            &auth.profile_id,
            params
                .get("prompt")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            params.get("contestants").unwrap_or(&Value::Null),
            params.get("preferences").unwrap_or(&Value::Null),
        )
        .await
        .map_err(anyhow::Error::from),
        "git/repositories/list" => list_git_repositories_payload(state, false)
            .await
            .map_err(anyhow::Error::from),
        "git/status" => get_git_status_payload(state, &require_string(&params, "repoPath")?)
            .await
            .map_err(anyhow::Error::from),
        "git/github/pulls" => list_github_pull_requests_payload(
            state,
            &require_string(&params, "repoPath")?,
            params
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or("open"),
            params.get("limit").and_then(Value::as_u64).unwrap_or(20),
        )
        .await
        .map_err(anyhow::Error::from),
        "git/github/pull" => {
            let number = params
                .get("number")
                .and_then(Value::as_u64)
                .ok_or_else(|| anyhow!("number is required"))?;
            get_github_pull_request_payload(state, &require_string(&params, "repoPath")?, number)
                .await
                .map_err(anyhow::Error::from)
        }
        "git/github/pull/checkout" => {
            let number = params
                .get("number")
                .and_then(Value::as_u64)
                .ok_or_else(|| anyhow!("number is required"))?;
            checkout_github_pull_request_payload(
                state,
                &require_string(&params, "repoPath")?,
                number,
            )
            .await
            .map_err(anyhow::Error::from)
        }
        "git/worktrees/list" => {
            list_git_worktrees_payload(state, &require_string(&params, "repoPath")?)
                .await
                .map_err(anyhow::Error::from)
        }
        "git/worktrees/create" => create_git_worktree_payload(
            state,
            &require_string(&params, "repoPath")?,
            &require_string(&params, "worktreePath")?,
            params.get("branchName").and_then(Value::as_str),
            params
                .get("createBranch")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            params
                .get("detach")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        )
        .await
        .map_err(anyhow::Error::from),
        "git/worktrees/remove" => remove_git_worktree_payload(
            state,
            &require_string(&params, "repoPath")?,
            &require_string(&params, "worktreePath")?,
            params
                .get("force")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        )
        .await
        .map_err(anyhow::Error::from),
        "git/file/get" => get_git_file_payload(
            state,
            &require_string(&params, "repoPath")?,
            &require_string(&params, "filePath")?,
        )
        .await
        .map_err(anyhow::Error::from),
        "git/file/resolve" => resolve_git_file_from_absolute_path_payload(
            state,
            &require_string(&params, "filePath")?,
        )
        .await
        .map_err(anyhow::Error::from),
        "git/file/save" => save_git_file_payload(
            state,
            &require_string(&params, "repoPath")?,
            &require_string(&params, "filePath")?,
            params
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )
        .await
        .map_err(anyhow::Error::from),
        "git/stage" => stage_git_changes_payload(
            state,
            &require_string(&params, "repoPath")?,
            params.get("filePath").and_then(Value::as_str),
        )
        .await
        .map_err(anyhow::Error::from),
        "git/unstage" => unstage_git_changes_payload(
            state,
            &require_string(&params, "repoPath")?,
            params.get("filePath").and_then(Value::as_str),
        )
        .await
        .map_err(anyhow::Error::from),
        "git/fetch" => fetch_git_repository_payload(state, &require_string(&params, "repoPath")?)
            .await
            .map_err(anyhow::Error::from),
        "git/pull" => pull_git_repository_payload(state, &require_string(&params, "repoPath")?)
            .await
            .map_err(anyhow::Error::from),
        "git/commit" => commit_git_changes_payload(
            state,
            &require_string(&params, "repoPath")?,
            &require_string(&params, "message")?,
        )
        .await
        .map_err(anyhow::Error::from),
        "git/commit/diff" => get_git_commit_diff_payload(
            state,
            &require_string(&params, "repoPath")?,
            &require_string(&params, "commitHash")?,
        )
        .await
        .map_err(anyhow::Error::from),
        "git/checkout" => checkout_git_branch_payload(
            state,
            &require_string(&params, "repoPath")?,
            &require_string(&params, "branchName")?,
            params
                .get("create")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        )
        .await
        .map_err(anyhow::Error::from),
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
            let uploaded = upload_attachments(state, &auth.profile_id, &session_id, vec![upload])
                .await
                .map_err(anyhow::Error::from)?;
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
    let relays = {
        let relays = state.relays.lock().await;
        relays
            .iter()
            .filter(|(key, _)| key.contains(GLOBAL_RELAY_KEY))
            .map(|(_, relay)| relay.clone())
            .collect::<Vec<_>>()
    };

    for relay in relays {
        let _ = relay.send(event.clone());
    }
}

async fn emit_profile_global_notification(state: &AppState, profile_id: &str, event: Value) {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id).0;
    let relay = {
        let relays = state.relays.lock().await;
        relays.get(&global_relay_key(resolved_profile_id)).cloned()
    };

    if let Some(relay) = relay {
        let _ = relay.send(event);
    }
}

async fn emit_profile_config_updated(state: &AppState, profile_id: &str, params: Value) {
    emit_profile_global_notification(
        state,
        profile_id,
        json!({
            "kind": "notification",
            "method": "codex-webui/configUpdated",
            "params": params
        }),
    )
    .await;
}

async fn enqueue_profile_notification(
    state: &AppState,
    profile_id: &str,
    notification_type: &str,
    session_id: Option<&str>,
    payload: Value,
) {
    if !is_valid_notification_event_type(notification_type) {
        return;
    }

    let enabled = match with_ui_state_read(state, profile_id, |ui_state| {
        let enabled_event_types = ui_state
            .get("notifications")
            .and_then(|value| value.get("settings"))
            .and_then(|value| value.get("enabledEventTypes"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| {
                default_notification_settings_value()["enabledEventTypes"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            });
        Ok(enabled_event_types
            .iter()
            .filter_map(Value::as_str)
            .any(|entry| entry == notification_type))
    })
    .await
    {
        Ok(enabled) => enabled,
        Err(_) => false,
    };
    if !enabled {
        return;
    }

    let session_name = if let Some(session_id) = session_id {
        build_session_summary_payload(state, profile_id, session_id, None)
            .await
            .ok()
            .and_then(|summary| summary.get("name").cloned())
            .unwrap_or(Value::Null)
    } else {
        Value::Null
    };

    let notification = json!({
        "id": Uuid::new_v4().to_string(),
        "type": notification_type,
        "createdAt": now_unix_ms(),
        "readAt": Value::Null,
        "sessionId": session_id.map(Value::from).unwrap_or(Value::Null),
        "sessionName": session_name,
        "payload": payload
    });

    let unread_count = match with_ui_state_write(state, profile_id, |ui_state| {
        let Some(items) = ui_state
            .get_mut("notifications")
            .and_then(Value::as_object_mut)
            .and_then(|notifications| notifications.get_mut("items"))
            .and_then(Value::as_array_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "notifications state is missing",
            ));
        };

        items.insert(0, notification.clone());
        if items.len() > 200 {
            items.truncate(200);
        }
        Ok(unread_notification_count(items))
    })
    .await
    {
        Ok(unread_count) => unread_count,
        Err(_) => return,
    };

    emit_profile_global_notification(
        state,
        profile_id,
        json!({
            "kind": "notification",
            "method": "codex-webui/notificationAdded",
            "params": {
                "notification": notification,
                "unreadCount": unread_count
            }
        }),
    )
    .await;
    emit_profile_config_updated(
        state,
        profile_id,
        json!({
            "notifications": {
                "unreadCount": unread_count
            }
        }),
    )
    .await;
}

async fn emit_runtime_profile_config_updated(state: &AppState, profile_id: &str) {
    let (shutdown_available, _) = system_shutdown_capability(&state.config).await;
    let (shutdown_after_queue_completes, scheduled_shutdown) =
        match with_ui_state_read(state, profile_id, |ui_state| {
            Ok((
                ui_state
                    .get("global")
                    .and_then(|value| value.get("shutdownAfterQueueCompletes"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                ui_state
                    .get("global")
                    .and_then(|value| value.get("scheduledShutdown"))
                    .cloned()
                    .unwrap_or(Value::Null),
            ))
        })
        .await
        {
            Ok(values) => values,
            Err(_) => return,
        };

    let next_scheduled_shutdown = if shutdown_available
        && scheduled_shutdown
            .get("scheduledFor")
            .and_then(Value::as_u64)
            .is_some_and(|value| value > now_unix_ms())
    {
        scheduled_shutdown
    } else {
        Value::Null
    };
    let paused_queues = list_resume_pending_queues_payload(state, profile_id)
        .await
        .unwrap_or_else(|_| json!([]));

    emit_profile_config_updated(
        state,
        profile_id,
        json!({
            "systemShutdown": {
                "available": shutdown_available,
                "delaySeconds": state.config.system_shutdown_delay_seconds,
                "armed": shutdown_available
                    && state.config.system_shutdown_enabled
                    && shutdown_after_queue_completes
            },
            "startup": {
                "pausedQueues": paused_queues,
                "scheduledShutdown": next_scheduled_shutdown
            }
        }),
    )
    .await;
}

async fn with_queue_dispatch_guard<T, F>(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    work: F,
) -> Option<T>
where
    F: Future<Output = T>,
{
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    let key = runtime_session_key(&resolved_profile_id, session_id);
    {
        let mut current = state.queue_dispatching.lock().await;
        if current.contains(&key) {
            return None;
        }
        current.insert(key.clone());
    }

    let result = work.await;
    state.queue_dispatching.lock().await.remove(&key);
    Some(result)
}

async fn remove_session_queue_item_after_dispatch(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    queue_id: &str,
) -> ApiResult<Value> {
    with_ui_state_write(state, profile_id, |ui_state| {
        let Some(queues_by_thread_id) = ui_state
            .get_mut("queuesByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "queue state is missing",
            ));
        };
        let Some(existing) = queues_by_thread_id.get_mut(session_id) else {
            return Err(api_error(StatusCode::NOT_FOUND, "QUEUE_ITEM_NOT_FOUND"));
        };
        let Some(queue) = existing.as_object_mut() else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "queue state had an unexpected shape",
            ));
        };
        let Some(items) = queue.get_mut("items").and_then(Value::as_array_mut) else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "queue items are missing",
            ));
        };
        let previous_len = items.len();
        items.retain(|item| item.get("id").and_then(Value::as_str) != Some(queue_id));
        if items.len() == previous_len {
            return Err(api_error(StatusCode::NOT_FOUND, "QUEUE_ITEM_NOT_FOUND"));
        }

        if items.is_empty() {
            queues_by_thread_id.remove(session_id);
        } else {
            queue.insert("resumePending".to_string(), json!(false));
            queue.insert("updatedAt".to_string(), json!(now_unix_ms()));
        }
        Ok(())
    })
    .await?;

    let queue = get_session_queue_payload(state, profile_id, session_id).await?;
    emit_queue_updated(state, profile_id, session_id, Some(queue.clone())).await;
    Ok(queue)
}

async fn dispatch_queue_item(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    queued_item: &Value,
    mode: &str,
) -> ApiResult<()> {
    let prompt = queued_item
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let attachment_ids = queued_item
        .get("attachmentIds")
        .cloned()
        .unwrap_or_else(|| json!([]));

    if mode == "steer" {
        steer_turn_payload(state, profile_id, session_id, prompt, Some(&attachment_ids))
            .await
            .map(|_| ())
    } else {
        send_turn_payload(
            state,
            profile_id,
            session_id,
            prompt,
            Some(&attachment_ids),
            json!({}),
        )
        .await
        .map(|_| ())
    }
}

async fn session_has_active_turn(state: &AppState, profile_id: &str, session_id: &str) -> bool {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id).0;
    if state
        .active_turns
        .lock()
        .await
        .contains_key(&runtime_session_key(resolved_profile_id, session_id))
    {
        return true;
    }

    let thread = match read_thread_payload(state, profile_id, session_id, true).await {
        Ok(payload) => payload,
        Err(_) => return true,
    };
    let Some(thread) = thread.as_object() else {
        return true;
    };
    if !is_live_thread_status(
        &normalized_thread_status(thread.get("status")).unwrap_or_else(|| "unknown".to_string()),
    ) {
        return false;
    }

    thread
        .get("turns")
        .and_then(Value::as_array)
        .is_some_and(|turns| {
            turns
                .iter()
                .any(|turn| turn.get("status").and_then(Value::as_str) == Some("inProgress"))
        })
}

async fn has_outstanding_queued_work(state: &AppState, profile_id: &str) -> bool {
    with_ui_state_read(state, profile_id, |ui_state| {
        Ok(ui_state
            .get("queuesByThreadId")
            .and_then(Value::as_object)
            .is_some_and(|queues| {
                queues.values().any(|queue| {
                    queue
                        .get("items")
                        .and_then(Value::as_array)
                        .is_some_and(|items| !items.is_empty())
                })
            }))
    })
    .await
    .unwrap_or(true)
}

async fn has_active_work_across_threads(state: &AppState, profile_id: &str) -> bool {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id).0;
    if state
        .active_turns
        .lock()
        .await
        .keys()
        .any(|key| key.starts_with(&format!("profile::{resolved_profile_id}::")))
    {
        return true;
    }

    let client = match app_server_client(state, profile_id).await {
        Ok(client) => client,
        Err(_) => return true,
    };
    let mut cursor: Option<String> = None;
    loop {
        let payload = match client
            .request(
                "thread/list",
                json!({
                    "limit": 200,
                    "archived": false,
                    "cursor": cursor
                }),
            )
            .await
        {
            Ok(payload) => payload,
            Err(_) => return true,
        };
        if payload
            .get("data")
            .and_then(Value::as_array)
            .is_some_and(|threads| {
                threads.iter().any(|thread| {
                    is_live_thread_status(
                        &normalized_thread_status(thread.get("status"))
                            .unwrap_or_else(|| "unknown".to_string()),
                    )
                })
            })
        {
            return true;
        }

        cursor = payload
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if cursor.is_none() {
            break;
        }
    }

    false
}

async fn clear_scheduled_shutdown(state: &AppState, profile_id: &str) {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    if let Some(handle) = state
        .shutdown_timers
        .lock()
        .await
        .remove(&resolved_profile_id)
    {
        handle.abort();
    }
    let _ = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(global) = ui_state.get_mut("global").and_then(Value::as_object_mut) else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "global state is missing",
            ));
        };
        global.insert("scheduledShutdown".to_string(), Value::Null);
        Ok(())
    })
    .await;
    emit_runtime_profile_config_updated(state, profile_id).await;
}

async fn cancel_scheduled_shutdown_for_activity(state: &AppState, profile_id: &str) {
    let scheduled = with_ui_state_read(state, profile_id, |ui_state| {
        Ok(ui_state
            .get("global")
            .and_then(|value| value.get("scheduledShutdown"))
            .cloned()
            .unwrap_or(Value::Null))
    })
    .await
    .unwrap_or(Value::Null);

    if !scheduled.is_null() {
        clear_scheduled_shutdown(state, profile_id).await;
    }
}

async fn execute_scheduled_shutdown(state: &AppState, profile_id: &str) {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    state
        .shutdown_timers
        .lock()
        .await
        .remove(&resolved_profile_id);
    let _ = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(global) = ui_state.get_mut("global").and_then(Value::as_object_mut) else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "global state is missing",
            ));
        };
        global.insert("scheduledShutdown".to_string(), Value::Null);
        Ok(())
    })
    .await;
    emit_runtime_profile_config_updated(state, profile_id).await;

    let (available, plan) = system_shutdown_capability(&state.config).await;
    let Some(plan) = plan.filter(|_| available) else {
        emit_profile_global_notification(
            state,
            profile_id,
            json!({
                "kind": "notification",
                "method": "codex-webui/shutdownFailed",
                "params": {
                    "message": "System shutdown is unavailable for this server user."
                }
            }),
        )
        .await;
        return;
    };

    let command = plan.command.clone();
    let args = plan.args.clone();
    if let Err(error) = Command::new(&command).args(&args).spawn() {
        emit_profile_global_notification(
            state,
            profile_id,
            json!({
                "kind": "notification",
                "method": "codex-webui/shutdownFailed",
                "params": {
                    "message": error.to_string()
                }
            }),
        )
        .await;
    }
}

async fn arm_scheduled_shutdown(state: &AppState, profile_id: &str, scheduled_shutdown: Value) {
    let Some(scheduled_for) = scheduled_shutdown
        .get("scheduledFor")
        .and_then(Value::as_u64)
    else {
        return;
    };
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    if let Some(handle) = state
        .shutdown_timers
        .lock()
        .await
        .remove(&resolved_profile_id)
    {
        handle.abort();
    }

    let delay_ms = scheduled_for.saturating_sub(now_unix_ms());
    let shutdown_state = state.clone();
    let shutdown_profile_id = profile_id.to_string();
    let handle = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        execute_scheduled_shutdown(&shutdown_state, &shutdown_profile_id).await;
    });
    state
        .shutdown_timers
        .lock()
        .await
        .insert(resolved_profile_id, handle);
}

async fn maybe_schedule_global_shutdown(
    state: &AppState,
    profile_id: &str,
    completed_turn_id: Option<&str>,
) {
    if !state.config.system_shutdown_enabled {
        return;
    }

    let (available, _) = system_shutdown_capability(&state.config).await;
    if !available {
        return;
    }

    let existing_scheduled = with_ui_state_read(state, profile_id, |ui_state| {
        Ok((
            ui_state
                .get("global")
                .and_then(|value| value.get("shutdownAfterQueueCompletes"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            ui_state
                .get("global")
                .and_then(|value| value.get("scheduledShutdown"))
                .cloned()
                .unwrap_or(Value::Null),
        ))
    })
    .await;
    let Ok((shutdown_after_queue_completes, scheduled_shutdown)) = existing_scheduled else {
        return;
    };
    if !shutdown_after_queue_completes {
        return;
    }
    if scheduled_shutdown
        .get("scheduledFor")
        .and_then(Value::as_u64)
        .is_some_and(|value| value > now_unix_ms())
    {
        arm_scheduled_shutdown(state, profile_id, scheduled_shutdown).await;
        return;
    }
    if has_outstanding_queued_work(state, profile_id).await
        || has_active_work_across_threads(state, profile_id).await
    {
        return;
    }

    let scheduled_shutdown = json!({
        "sessionId": Value::Null,
        "scheduledFor": now_unix_ms() + state.config.system_shutdown_delay_seconds * 1000,
        "delaySeconds": state.config.system_shutdown_delay_seconds
    });
    if with_ui_state_write(state, profile_id, |ui_state| {
        let Some(global) = ui_state.get_mut("global").and_then(Value::as_object_mut) else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "global state is missing",
            ));
        };
        global.insert("scheduledShutdown".to_string(), scheduled_shutdown.clone());
        Ok(())
    })
    .await
    .is_err()
    {
        return;
    }

    arm_scheduled_shutdown(state, profile_id, scheduled_shutdown.clone()).await;
    emit_profile_global_notification(
        state,
        profile_id,
        json!({
            "kind": "notification",
            "method": "codex-webui/shutdownScheduled",
            "params": {
                "delaySeconds": state.config.system_shutdown_delay_seconds,
                "turnId": completed_turn_id.map(Value::from).unwrap_or(Value::Null),
                "scheduledFor": scheduled_shutdown.get("scheduledFor").cloned().unwrap_or(Value::Null),
                "sessionId": Value::Null
            }
        }),
    )
    .await;
    enqueue_profile_notification(
        state,
        profile_id,
        "shutdownScheduled",
        None,
        json!({
            "delaySeconds": state.config.system_shutdown_delay_seconds,
            "scheduledFor": scheduled_shutdown.get("scheduledFor").cloned().unwrap_or(Value::Null),
            "turnId": completed_turn_id.map(Value::from).unwrap_or(Value::Null)
        }),
    )
    .await;
    emit_runtime_profile_config_updated(state, profile_id).await;
}

async fn maybe_drain_queue(state: &AppState, profile_id: &str, session_id: &str) {
    let _ = with_queue_dispatch_guard(state, profile_id, session_id, async {
        let queue = match get_session_queue_payload(state, profile_id, session_id).await {
            Ok(queue) => queue,
            Err(_) => return,
        };
        if queue
            .get("items")
            .and_then(Value::as_array)
            .is_none_or(|items| items.is_empty())
        {
            maybe_schedule_global_shutdown(state, profile_id, None).await;
            return;
        }
        if queue
            .get("resumeRequired")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return;
        }
        if session_has_active_turn(state, profile_id, session_id).await {
            return;
        }

        let queued_item = queue
            .get("items")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .cloned();
        let Some(queued_item) = queued_item else {
            return;
        };
        let queue_id = queued_item
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        match dispatch_queue_item(state, profile_id, session_id, &queued_item, "message").await {
            Ok(()) => {
                let _ = remove_session_queue_item_after_dispatch(
                    state, profile_id, session_id, &queue_id,
                )
                .await;
            }
            Err(error) => {
                emit_session_notification(
                    state,
                    profile_id,
                    session_id,
                    json!({
                        "kind": "notification",
                        "method": "codex-webui/queueDispatchFailed",
                        "params": {
                            "queueId": queue_id,
                            "code": Value::Null,
                            "message": error.message
                        }
                    }),
                )
                .await;
                enqueue_profile_notification(
                    state,
                    profile_id,
                    "queueDispatchFailed",
                    Some(session_id),
                    json!({
                        "queueId": queue_id,
                        "code": Value::Null,
                        "message": error.message
                    }),
                )
                .await;
            }
        }
    })
    .await;
}

fn notification_thread_id(method: &str, params: &Value) -> Option<String> {
    params
        .get("threadId")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            method
                .starts_with("thread/")
                .then(|| {
                    params
                        .get("thread")
                        .and_then(|thread| thread.get("id"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .flatten()
        })
}

fn notification_turn_id(params: &Value) -> Option<String> {
    params
        .get("turnId")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            params
                .get("turn")
                .and_then(|turn| turn.get("id"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

fn pending_request_id(raw_id: &Value) -> String {
    raw_id
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| raw_id.to_string())
}

async fn set_session_highlight(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    highlight: Option<Value>,
) {
    let result = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(highlights_by_thread_id) = ui_state
            .get_mut("highlightsByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "highlight state is missing",
            ));
        };

        if let Some(highlight) = highlight {
            highlights_by_thread_id.insert(session_id.to_string(), highlight);
        } else {
            highlights_by_thread_id.remove(session_id);
        }

        Ok(())
    })
    .await;

    if result.is_ok() {
        emit_session_summary_updated(state, profile_id, session_id, None).await;
    }
}

async fn handle_profile_server_request(
    state: &AppState,
    profile_id: &str,
    request: &backend::codex_app_server::AppServerRequest,
) {
    let Some(session_id) = request
        .params
        .get("threadId")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return;
    };

    let preferences = with_ui_state_read(state, profile_id, |ui_state| {
        Ok(ui_state
            .get("preferencesByThreadId")
            .and_then(Value::as_object)
            .and_then(|entries| entries.get(&session_id))
            .cloned()
            .unwrap_or(Value::Null))
    })
    .await
    .unwrap_or(Value::Null);
    let auto_approve_mode = preferences
        .get("autoApproveMode")
        .and_then(Value::as_str)
        .unwrap_or("manual");

    let auto_approve_result = match request.method.as_str() {
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval"
            if auto_approve_mode == "turn" || auto_approve_mode == "session" =>
        {
            Some(json!({
                "decision": if auto_approve_mode == "session" { "acceptForSession" } else { "accept" }
            }))
        }
        "item/permissions/requestApproval"
            if auto_approve_mode == "turn" || auto_approve_mode == "session" =>
        {
            Some(json!({
                "scope": auto_approve_mode,
                "permissions": request.params.get("permissions").cloned().unwrap_or_else(|| json!({}))
            }))
        }
        _ => None,
    };

    if let Some(result) = auto_approve_result {
        if let Ok(client) = app_server_client(state, profile_id).await {
            if client.respond(request.id.clone(), result).await.is_ok() {
                emit_session_notification(
                    state,
                    profile_id,
                    &session_id,
                    json!({
                        "kind": "notification",
                        "method": "codex-webui/autoApproved",
                        "params": {
                            "requestId": pending_request_id(&request.id),
                            "requestMethod": request.method,
                            "autoApproveMode": auto_approve_mode
                        }
                    }),
                )
                .await;
                return;
            }
        }
    }

    let request_id = pending_request_id(&request.id);
    let runtime_key = runtime_session_key(
        resolve_runtime_profile_entry(&state.config, profile_id).0,
        &session_id,
    );
    state
        .pending_server_requests
        .lock()
        .await
        .entry(runtime_key)
        .or_default()
        .insert(
            request_id.clone(),
            PendingServerRequestEntry {
                raw_id: request.id.clone(),
                method: request.method.clone(),
                params: request.params.clone(),
                created_at: now_rfc3339(),
            },
        );

    emit_session_notification(
        state,
        profile_id,
        &session_id,
        json!({
            "kind": "serverRequest",
            "id": request_id.clone(),
            "method": request.method,
            "params": request.params
        }),
    )
    .await;
    emit_profile_global_notification(
        state,
        profile_id,
        json!({
            "kind": "notification",
            "method": "codex-webui/sessionAttention",
            "params": {
                "sessionId": session_id,
                "reason": "approval"
            }
        }),
    )
    .await;
    enqueue_profile_notification(
        state,
        profile_id,
        "sessionAttention",
        Some(&session_id),
        json!({
            "reason": "approval",
            "requestId": request_id,
            "requestMethod": request.method
        }),
    )
    .await;
    set_session_highlight(
        state,
        profile_id,
        &session_id,
        Some(json!({
            "kind": "attention",
            "at": now_unix_ms()
        })),
    )
    .await;
}

async fn abort_turn_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<Value> {
    let active_turn_id = resolve_active_turn_id_payload(state, profile_id, session_id).await?;
    let Some(turn_id) = active_turn_id else {
        return Ok(json!({ "interrupted": false }));
    };

    app_server_client(state, profile_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to codex app-server: {error}"),
            )
        })?
        .request(
            "turn/interrupt",
            json!({
                "threadId": session_id,
                "turnId": turn_id
            }),
        )
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to abort the session: {error}"),
            )
        })?;

    Ok(json!({ "interrupted": true }))
}

async fn resolve_server_request_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    request_id: &str,
    result: Value,
) -> ApiResult<Value> {
    let runtime_key = runtime_session_key(
        resolve_runtime_profile_entry(&state.config, profile_id).0,
        session_id,
    );
    let pending = state
        .pending_server_requests
        .lock()
        .await
        .get(&runtime_key)
        .and_then(|entries| entries.get(request_id))
        .cloned();

    let Some(pending) = pending else {
        return Err(api_error(StatusCode::NOT_FOUND, "SERVER_REQUEST_NOT_FOUND"));
    };

    let client = app_server_client(state, profile_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to codex app-server: {error}"),
            )
        })?;
    client
        .respond(pending.raw_id.clone(), result)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to resolve the server request: {error}"),
            )
        })?;

    let remaining = {
        let mut pending_requests = state.pending_server_requests.lock().await;
        let remaining = pending_requests
            .get_mut(&runtime_key)
            .map(|entries| {
                entries.remove(request_id);
                entries.len()
            })
            .unwrap_or(0);
        if remaining == 0 {
            pending_requests.remove(&runtime_key);
        }
        remaining
    };

    emit_session_notification(
        state,
        profile_id,
        session_id,
        json!({
            "kind": "notification",
            "method": "serverRequest/resolved",
            "params": {
                "threadId": session_id,
                "requestId": request_id
            }
        }),
    )
    .await;
    if remaining == 0 {
        set_session_highlight(state, profile_id, session_id, None).await;
    }

    Ok(json!({ "ok": true }))
}

async fn handle_profile_runtime_notification(
    state: &AppState,
    profile_id: &str,
    notification: &AppServerNotification,
) {
    let Some(session_id) = notification_thread_id(&notification.method, &notification.params)
    else {
        return;
    };
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    let runtime_key = runtime_session_key(&resolved_profile_id, &session_id);

    match notification.method.as_str() {
        "turn/started" => {
            if let Some(turn_id) = notification_turn_id(&notification.params) {
                state.active_turns.lock().await.insert(runtime_key, turn_id);
            }
            cancel_scheduled_shutdown_for_activity(state, profile_id).await;
            set_session_highlight(state, profile_id, &session_id, None).await;
        }
        "turn/completed" => {
            let turn_id = notification_turn_id(&notification.params);
            let mut active_turns = state.active_turns.lock().await;
            if turn_id
                .as_ref()
                .is_none_or(|turn_id| active_turns.get(&runtime_key) == Some(turn_id))
            {
                active_turns.remove(&runtime_key);
            }
            drop(active_turns);
            maybe_drain_queue(state, profile_id, &session_id).await;
            maybe_schedule_global_shutdown(state, profile_id, turn_id.as_deref()).await;
            emit_profile_global_notification(
                state,
                profile_id,
                json!({
                    "kind": "notification",
                    "method": "codex-webui/sessionAttention",
                    "params": {
                        "sessionId": session_id,
                        "reason": "completed"
                    }
                }),
            )
            .await;
            enqueue_profile_notification(
                state,
                profile_id,
                "sessionCompleted",
                Some(&session_id),
                json!({
                    "turnId": turn_id.clone().map(Value::String).unwrap_or(Value::Null)
                }),
            )
            .await;
            set_session_highlight(
                state,
                profile_id,
                &session_id,
                Some(json!({
                    "kind": "completed",
                    "at": now_unix_ms()
                })),
            )
            .await;
        }
        "thread/status/changed" => {
            let status = normalized_thread_status(notification.params.get("status"))
                .unwrap_or_else(|| "unknown".to_string());
            if is_live_thread_status(&status) {
                cancel_scheduled_shutdown_for_activity(state, profile_id).await;
            } else {
                state.active_turns.lock().await.remove(&runtime_key);
                maybe_drain_queue(state, profile_id, &session_id).await;
                maybe_schedule_global_shutdown(state, profile_id, None).await;
            }
        }
        "thread/archived" | "thread/unarchived" => {
            emit_profile_global_notification(
                state,
                profile_id,
                json!({
                    "kind": "notification",
                    "method": "codex-webui/sessionListsInvalidated",
                    "params": {
                        "threadId": session_id,
                        "archived": notification.method == "thread/archived"
                    }
                }),
            )
            .await;
        }
        _ => {}
    }

    if let Some(event) = map_app_server_session_notification(notification) {
        emit_session_notification(state, profile_id, &session_id, event).await;
    }

    if matches!(
        notification.method.as_str(),
        "turn/started" | "turn/completed" | "thread/name/updated" | "thread/status/changed"
    ) {
        emit_session_summary_updated(state, profile_id, &session_id, None).await;
    }
}

async fn restore_persisted_shutdown_state(state: &AppState, profile_id: &str) -> ApiResult<()> {
    let (shutdown_after_queue_completes, scheduled_shutdown) =
        with_ui_state_read(state, profile_id, |ui_state| {
            Ok((
                ui_state
                    .get("global")
                    .and_then(|value| value.get("shutdownAfterQueueCompletes"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                ui_state
                    .get("global")
                    .and_then(|value| value.get("scheduledShutdown"))
                    .cloned()
                    .unwrap_or(Value::Null),
            ))
        })
        .await?;

    let (shutdown_available, _) = system_shutdown_capability(&state.config).await;
    if !state.config.system_shutdown_enabled || !shutdown_available {
        with_ui_state_write(state, profile_id, |ui_state| {
            let Some(global) = ui_state.get_mut("global").and_then(Value::as_object_mut) else {
                return Err(api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "global state is missing",
                ));
            };
            global.insert("shutdownAfterQueueCompletes".to_string(), json!(false));
            global.insert("scheduledShutdown".to_string(), Value::Null);
            Ok(())
        })
        .await?;
        return Ok(());
    }

    if scheduled_shutdown
        .get("scheduledFor")
        .and_then(Value::as_u64)
        .is_some_and(|value| value > now_unix_ms())
    {
        arm_scheduled_shutdown(state, profile_id, scheduled_shutdown).await;
    } else if shutdown_after_queue_completes {
        maybe_schedule_global_shutdown(state, profile_id, None).await;
    }

    Ok(())
}

async fn restore_runtime_profile_state(state: AppState, profile_id: String) {
    if let Err(error) = mark_queues_pending_resume_payload(&state, &profile_id).await {
        warn!("failed to mark queued sessions as pending resume for {profile_id}: {error}");
    }
    if let Err(error) = restore_persisted_shutdown_state(&state, &profile_id).await {
        warn!("failed to restore shutdown state for {profile_id}: {error}");
    }
    emit_runtime_profile_config_updated(&state, &profile_id).await;

    loop {
        let client = match app_server_client(&state, &profile_id).await {
            Ok(client) => client,
            Err(error) => {
                warn!("failed to create app-server client for {profile_id}: {error:#}");
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        let _ = client
            .request("model/list", json!({ "includeHidden": false }))
            .await;
        let mut notifications = client.subscribe_notifications();
        let mut requests = client.subscribe_requests();

        loop {
            tokio::select! {
                notification = notifications.recv() => match notification {
                    Ok(notification) => {
                        handle_profile_runtime_notification(&state, &profile_id, &notification).await;
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(
                            "runtime app-server relay lagged for {profile_id}: skipped {skipped} messages"
                        );
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                request = requests.recv() => match request {
                    Ok(request) => {
                        handle_profile_server_request(&state, &profile_id, &request).await;
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(
                            "runtime app-server request relay lagged for {profile_id}: skipped {skipped} messages"
                        );
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
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

fn normalize_token_usage_payload(value: Option<&Value>) -> Value {
    let Some(record) = value.and_then(Value::as_object) else {
        return Value::Null;
    };

    let normalize_breakdown = |input: Option<&Value>| {
        let breakdown = input.and_then(Value::as_object);
        json!({
            "totalTokens": breakdown
                .and_then(|value| value.get("totalTokens"))
                .and_then(Value::as_u64)
                .unwrap_or(0),
            "inputTokens": breakdown
                .and_then(|value| value.get("inputTokens"))
                .and_then(Value::as_u64)
                .unwrap_or(0),
            "cachedInputTokens": breakdown
                .and_then(|value| value.get("cachedInputTokens"))
                .and_then(Value::as_u64)
                .unwrap_or(0),
            "outputTokens": breakdown
                .and_then(|value| value.get("outputTokens"))
                .and_then(Value::as_u64)
                .unwrap_or(0),
            "reasoningOutputTokens": breakdown
                .and_then(|value| value.get("reasoningOutputTokens"))
                .and_then(Value::as_u64)
                .unwrap_or(0)
        })
    };

    json!({
        "total": normalize_breakdown(record.get("total")),
        "last": normalize_breakdown(record.get("last")),
        "modelContextWindow": record
            .get("modelContextWindow")
            .and_then(Value::as_u64)
            .map(Value::from)
            .unwrap_or(Value::Null)
    })
}

fn summarize_command_payload(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::Array(entries)) => {
            let summary = entries
                .iter()
                .filter_map(value_text)
                .collect::<Vec<_>>()
                .join(" ");
            let trimmed = summary.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        Some(Value::String(command)) => {
            let trimmed = command.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        Some(other) => value_text(other),
        None => None,
    }
}

fn summarize_tool_invocation_payload(value: Option<&Value>) -> Option<String> {
    let record = value.and_then(Value::as_object)?;
    let tool_name = ["toolName", "name", "tool", "method", "displayName"]
        .iter()
        .find_map(|key| record.get(*key).and_then(value_text));
    let server_name = ["serverName", "server"]
        .iter()
        .find_map(|key| record.get(*key).and_then(value_text));

    match (server_name, tool_name) {
        (Some(server_name), Some(tool_name)) => Some(format!("{server_name} · {tool_name}")),
        (Some(server_name), None) => Some(server_name),
        (None, Some(tool_name)) => Some(tool_name),
        (None, None) => None,
    }
}

fn prepare_session_stream_item_payload(item: &Value, turn_id: &str) -> Value {
    let mut normalized = normalize_session_item_payload(item, turn_id, 0)
        .as_object()
        .cloned()
        .unwrap_or_default();
    let item_type = normalized
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");

    match item_type {
        "contextCompaction" => {
            normalized.insert("title".to_string(), json!("Context compression"));
            normalized.insert("detailState".to_string(), json!("inline"));
            normalized.insert(
                "detailPreview".to_string(),
                json!("Compressing conversation context"),
            );
        }
        "commandExecution" => {
            normalized.insert("title".to_string(), json!("Command"));
            normalized.insert("detailState".to_string(), json!("deferred"));
            normalized.insert(
                "detailPreview".to_string(),
                summarize_command_payload(normalized.get("command"))
                    .map(Value::String)
                    .unwrap_or(Value::Null),
            );
            if !normalized.contains_key("parsed_cmd") && normalized.contains_key("parsedCmd") {
                if let Some(parsed_cmd) = normalized.get("parsedCmd").cloned() {
                    normalized.insert("parsed_cmd".to_string(), parsed_cmd);
                }
            }
        }
        "fileChange" => {
            let changes = normalized
                .get("changes")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let first_path = changes
                .iter()
                .find_map(|entry| entry.get("path").and_then(value_text));
            normalized.insert("title".to_string(), json!("Files changed"));
            normalized.insert("detailState".to_string(), json!("deferred"));
            normalized.insert("changeCount".to_string(), json!(changes.len()));
            normalized.insert(
                "firstChangePath".to_string(),
                first_path.clone().map(Value::String).unwrap_or(Value::Null),
            );
            normalized.insert(
                "detailPreview".to_string(),
                first_path.map(Value::String).unwrap_or_else(|| {
                    if changes.is_empty() {
                        Value::Null
                    } else {
                        Value::String(format!("{} files", changes.len()))
                    }
                }),
            );
            normalized.insert(
                "changes".to_string(),
                Value::Array(
                    changes
                        .into_iter()
                        .map(|entry| {
                            json!({
                                "path": entry.get("path").and_then(value_text).unwrap_or_else(|| "Code edit".to_string()),
                                "kind": entry.get("kind").and_then(value_text).unwrap_or_else(|| "update".to_string())
                            })
                        })
                        .collect(),
                ),
            );
        }
        "webSearch" => {
            normalized.insert("title".to_string(), json!("Web search"));
            normalized.insert("detailState".to_string(), json!("deferred"));
            normalized.insert(
                "detailPreview".to_string(),
                value_text(normalized.get("query").unwrap_or(&Value::Null))
                    .or_else(|| summarize_tool_invocation_payload(normalized.get("action")))
                    .map(Value::String)
                    .unwrap_or(Value::Null),
            );
        }
        "mcpToolCall" | "dynamicToolCall" => {
            normalized.insert(
                "title".to_string(),
                Value::String(if item_type == "mcpToolCall" {
                    "MCP call".to_string()
                } else {
                    "Tool call".to_string()
                }),
            );
            normalized.insert("detailState".to_string(), json!("deferred"));
            normalized.insert(
                "detailPreview".to_string(),
                summarize_tool_invocation_payload(normalized.get("invocation"))
                    .or_else(|| value_text(normalized.get("tool").unwrap_or(&Value::Null)))
                    .map(Value::String)
                    .unwrap_or(Value::Null),
            );
        }
        _ => {
            normalized
                .entry("detailState".to_string())
                .or_insert_with(|| json!("inline"));
        }
    }

    Value::Object(normalized)
}

fn map_app_server_session_notification(notification: &AppServerNotification) -> Option<Value> {
    let params = notification.params.as_object().cloned().unwrap_or_default();
    let mut mapped = params.clone();

    match notification.method.as_str() {
        "turn/started" | "turn/completed" => {
            let fallback_turn_id = mapped
                .get("turnId")
                .and_then(Value::as_str)
                .unwrap_or("turn-0")
                .to_string();
            let fallback_status = if notification.method == "turn/started" {
                "inProgress"
            } else {
                "completed"
            };
            let turn = mapped.get("turn").cloned().unwrap_or_else(|| {
                json!({
                    "id": fallback_turn_id,
                    "status": fallback_status,
                    "items": []
                })
            });
            mapped.insert("turn".to_string(), normalize_session_turn_payload(&turn, 0));
        }
        "item/started" | "item/completed" => {
            let turn_id = mapped
                .get("turnId")
                .and_then(Value::as_str)
                .unwrap_or("turn-0");
            let item = mapped.get("item").cloned().unwrap_or_else(
                || json!({ "id": mapped.get("itemId").cloned().unwrap_or(Value::Null) }),
            );
            mapped.insert(
                "item".to_string(),
                prepare_session_stream_item_payload(&item, turn_id),
            );
        }
        "thread/name/updated" => {
            mapped.insert(
                "threadName".to_string(),
                mapped
                    .get("threadName")
                    .and_then(value_text)
                    .map(Value::String)
                    .unwrap_or(Value::Null),
            );
        }
        "thread/status/changed" => {
            mapped.insert(
                "status".to_string(),
                Value::String(
                    normalized_thread_status(mapped.get("status"))
                        .unwrap_or_else(|| "unknown".to_string()),
                ),
            );
        }
        "thread/tokenUsage/updated" => {
            mapped.insert(
                "tokenUsage".to_string(),
                normalize_token_usage_payload(mapped.get("tokenUsage")),
            );
        }
        "item/commandExecution/outputDelta" => {
            let delta = mapped
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            mapped.insert("delta".to_string(), Value::String(delta.clone()));
            mapped.insert("deltaLength".to_string(), json!(delta.chars().count()));
        }
        _ => {}
    }

    Some(json!({
        "kind": "notification",
        "method": notification.method,
        "params": Value::Object(mapped)
    }))
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
    profile_id: &str,
    session_id: &str,
    files: Vec<UploadFilePayload>,
) -> ApiResult<Value> {
    let mut uploads = Vec::new();
    for file in files {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(file.data_base64)
            .map_err(|error| api_error(StatusCode::BAD_REQUEST, error.to_string()))?;
        uploads.push(AttachmentUploadPayload {
            name: file.name,
            mime_type: file.mime_type,
            bytes,
        });
    }
    let stored = save_uploaded_attachment_records(state, profile_id, session_id, uploads).await?;
    emit_attachments_updated(state, profile_id, session_id).await?;
    Ok(json!({
        "attachments": stored
            .iter()
            .map(attachment_payload_from_record)
            .collect::<Vec<_>>()
    }))
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
    if cwd.join("build/static").exists() || cwd.join("svelte.config.js").exists() {
        return cwd.clone();
    }

    if cwd
        .file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value == "backend")
    {
        if let Some(parent) = cwd.parent() {
            let parent = parent.to_path_buf();
            if parent.join("build/static").exists() || parent.join("svelte.config.js").exists() {
                return parent;
            }
        }
    }

    cwd.clone()
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

    fn init_test_git_repo(repo_path: &Path) {
        fs::create_dir_all(repo_path).unwrap();
        let commands = [
            vec!["init".to_string(), repo_path.display().to_string()],
            vec![
                "-C".to_string(),
                repo_path.display().to_string(),
                "config".to_string(),
                "user.name".to_string(),
                "Codex WebUI".to_string(),
            ],
            vec![
                "-C".to_string(),
                repo_path.display().to_string(),
                "config".to_string(),
                "user.email".to_string(),
                "codex-webui@example.com".to_string(),
            ],
        ];
        for command in commands {
            let output = std::process::Command::new("git")
                .args(command)
                .output()
                .unwrap();
            assert!(output.status.success(), "git setup command failed");
        }

        fs::write(repo_path.join("README.md"), "init\n").unwrap();
        let add = std::process::Command::new("git")
            .args(["-C", repo_path.to_str().unwrap(), "add", "README.md"])
            .output()
            .unwrap();
        assert!(add.status.success(), "git add failed");
        let commit = std::process::Command::new("git")
            .args(["-C", repo_path.to_str().unwrap(), "commit", "-m", "init"])
            .output()
            .unwrap();
        assert!(commit.status.success(), "git commit failed");
    }

    fn test_state(
        project_root: PathBuf,
        allowed_roots: Vec<PathBuf>,
        codex_home: PathBuf,
    ) -> AppState {
        let profile_id = "default".to_string();
        let profile_data_dir = project_root
            .join(".data")
            .join("profiles")
            .join(&profile_id);
        let mut profiles = HashMap::new();
        profiles.insert(
            profile_id.clone(),
            RuntimeProfile {
                label: "Default".to_string(),
                codex_home,
                data_dir: profile_data_dir,
            },
        );

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
                codex_bin: "codex".to_string(),
                max_upload_bytes: 20 * 1024 * 1024,
                git_discovery_depth: 1,
                system_shutdown_enabled: false,
                system_shutdown_delay_seconds: 30,
                system_shutdown_command_override: None,
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
            catalog_cache: Arc::new(Mutex::new(HashMap::new())),
            git_repository_cache: Arc::new(Mutex::new(None)),
            pinned_git_repositories: Arc::new(Mutex::new(HashMap::new())),
            inflight_requests: Arc::new(Mutex::new(HashMap::new())),
            quota_cache: Arc::new(Mutex::new(HashMap::new())),
            relays: Arc::new(Mutex::new(HashMap::new())),
            terminals: Arc::new(Mutex::new(HashMap::new())),
            ui_state_locks: Arc::new(Mutex::new(HashMap::new())),
            automation_timers: Arc::new(Mutex::new(HashMap::new())),
            queue_dispatching: Arc::new(Mutex::new(HashSet::new())),
            active_turns: Arc::new(Mutex::new(HashMap::new())),
            pending_server_requests: Arc::new(Mutex::new(HashMap::new())),
            shutdown_timers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn test_state_with_static_dir_and_base_path(
        project_root: PathBuf,
        allowed_roots: Vec<PathBuf>,
        codex_home: PathBuf,
        static_dir: PathBuf,
        base_path: &str,
    ) -> AppState {
        let mut state = test_state(project_root, allowed_roots, codex_home);
        let mut config = (*state.config).clone();
        config.static_dir = static_dir;
        config.base_path = base_path.to_string();
        state.config = Arc::new(config);
        state
    }

    fn test_state_with_fake_app_server(
        project_root: PathBuf,
        allowed_roots: Vec<PathBuf>,
        codex_home: PathBuf,
    ) -> AppState {
        let mut state = test_state(project_root, allowed_roots, codex_home);
        let fake_server_path = state.config.project_root.join("fake-codex-test.py");
        fs::write(
            &fake_server_path,
            r#"#!/usr/bin/env python3
import json
import sys

threads = {}
thread_counter = 0
timestamp_counter = 0
turn_counter = 0

for raw_line in sys.stdin:
    line = raw_line.strip()
    if not line:
        continue

    payload = json.loads(line)
    request_id = payload.get("id")
    method = payload.get("method")
    params = payload.get("params") or {}

    def write(message):
        sys.stdout.write(json.dumps(message) + "\n")
        sys.stdout.flush()

    if method == "initialize":
        write({
            "id": request_id,
            "result": {
                "serverInfo": {
                    "name": "fake-codex",
                    "title": "Fake Codex App Server",
                    "version": "0.1.0"
                }
            }
        })
    elif method == "initialized":
        write({
            "method": "fake/ready",
            "params": {}
        })
    elif method == "thread/start":
        thread_counter += 1
        timestamp_counter += 1
        thread_id = f"thread-{thread_counter}"
        thread = {
            "id": thread_id,
            "name": "New thread",
            "preview": "",
            "cwd": params.get("cwd", ""),
            "archived": False,
            "createdAt": timestamp_counter,
            "updatedAt": timestamp_counter,
            "status": "idle",
            "isSubagent": False,
            "agentNickname": None,
            "agentRole": None,
            "turns": []
        }
        threads[thread_id] = thread
        write({
            "id": request_id,
            "result": {
                "thread": thread
            }
        })
    elif method == "thread/name/set":
        thread_id = params.get("threadId", "")
        timestamp_counter += 1
        if thread_id in threads:
            threads[thread_id]["name"] = params.get("name", "")
            threads[thread_id]["updatedAt"] = timestamp_counter
        write({
            "id": request_id,
            "result": {
                "ok": True
            }
        })
    elif method == "thread/seed":
        thread = params.get("thread") or {}
        thread_id = thread.get("id")
        if isinstance(thread_id, str) and thread_id:
            threads[thread_id] = thread
        write({
            "id": request_id,
            "result": {
                "ok": True
            }
        })
    elif method == "thread/read":
        thread_id = params.get("threadId", "")
        thread = threads.get(thread_id, {
            "id": thread_id,
            "name": "New thread",
            "preview": "",
            "cwd": "",
            "archived": False,
            "createdAt": 0,
            "updatedAt": 0,
            "status": "idle",
            "isSubagent": False,
            "agentNickname": None,
            "agentRole": None,
            "turns": []
        })
        write({
            "id": request_id,
            "result": {
                "thread": thread
            }
        })
    elif method == "thread/list":
        archived = bool(params.get("archived", False))
        limit = max(1, min(int(params.get("limit", 20) or 20), 200))
        cursor = str(params.get("cursor") or "").strip()
        start = int(cursor) if cursor.isdigit() else 0
        data = [thread for thread in threads.values() if bool(thread.get("archived", False)) == archived]
        data.sort(key=lambda thread: int(thread.get("updatedAt", 0)), reverse=True)
        end = min(start + limit, len(data))
        next_cursor = str(end) if end < len(data) else None
        write({
            "id": request_id,
            "result": {
                "data": data[start:end] if start < len(data) else [],
                "nextCursor": next_cursor
            }
        })
    elif method == "thread/resume":
        thread_id = params.get("threadId", "")
        if thread_id in threads:
            threads[thread_id]["status"] = "idle"
            threads[thread_id]["resumeCount"] = int(threads[thread_id].get("resumeCount", 0) or 0) + 1
        write({
            "id": request_id,
            "result": {
                "ok": True
            }
        })
    elif method == "thread/fork":
        source_thread_id = params.get("threadId", "")
        source_thread = threads.get(source_thread_id)
        if not isinstance(source_thread, dict):
            write({
                "id": request_id,
                "error": {
                    "code": -32000,
                    "message": "thread not found"
                }
            })
            continue
        thread_counter += 1
        timestamp_counter += 1
        forked_thread = json.loads(json.dumps(source_thread))
        forked_thread["id"] = f"fork-{thread_counter}"
        forked_thread["createdAt"] = timestamp_counter
        forked_thread["updatedAt"] = timestamp_counter
        forked_thread["forkedFrom"] = source_thread_id
        threads[forked_thread["id"]] = forked_thread
        write({
            "id": request_id,
            "result": {
                "thread": forked_thread
            }
        })
    elif method == "thread/rollback":
        thread_id = params.get("threadId", "")
        num_turns = max(0, int(params.get("numTurns", 0) or 0))
        thread = threads.get(thread_id)
        if not isinstance(thread, dict):
            write({
                "id": request_id,
                "error": {
                    "code": -32000,
                    "message": "thread not found"
                }
            })
            continue
        turns = list(thread.get("turns") or [])
        if num_turns > 0:
            turns = turns[:-num_turns] if num_turns < len(turns) else []
        thread["turns"] = turns
        thread["rollbackCount"] = int(thread.get("rollbackCount", 0) or 0) + num_turns
        thread["updatedAt"] = timestamp_counter
        threads[thread_id] = thread
        write({
            "id": request_id,
            "result": {
                "thread": thread
            }
        })
    elif method == "turn/start":
        thread_id = params.get("threadId", "")
        turn_counter += 1
        timestamp_counter += 1
        turn_id = f"turn-{turn_counter}"
        thread = threads.get(thread_id) or {
            "id": thread_id,
            "name": "New thread",
            "preview": "",
            "cwd": params.get("cwd", ""),
            "archived": False,
            "createdAt": timestamp_counter,
            "updatedAt": timestamp_counter,
            "status": "idle",
            "isSubagent": False,
            "agentNickname": None,
            "agentRole": None,
            "turns": []
        }
        input_items = params.get("input") or []
        text_item = next(
            (
                item for item in input_items
                if isinstance(item, dict) and item.get("type") == "text"
            ),
            {}
        )
        text_value = text_item.get("text") if isinstance(text_item, dict) else ""
        if not isinstance(text_value, str):
            text_value = ""
        turn = {
            "id": turn_id,
            "status": "inProgress",
            "error": None,
            "startedAt": timestamp_counter,
            "completedAt": None,
            "durationMs": None,
            "items": [
                {
                    "id": f"{turn_id}:user:0",
                    "type": "userMessage",
                    "text": text_value
                }
            ]
        }
        thread["turns"] = list(thread.get("turns") or []) + [turn]
        thread["preview"] = text_value.strip()
        thread["status"] = "running"
        thread["updatedAt"] = timestamp_counter
        thread["lastTurnStart"] = params
        threads[thread_id] = thread
        write({
            "id": request_id,
            "result": {
                "turn": {
                    "id": turn_id
                }
            }
        })
    elif method == "turn/steer":
        thread_id = params.get("threadId", "")
        expected_turn_id = params.get("expectedTurnId", "")
        if thread_id in threads:
            threads[thread_id]["lastTurnSteer"] = params
        write({
            "id": request_id,
            "result": {
                "turnId": expected_turn_id
            }
        })
    elif method == "turn/interrupt":
        write({
            "id": request_id,
            "result": {
                "interrupted": True
            }
        })
    elif method == "account/read":
        write({
            "id": request_id,
            "result": {
                "account": {
                    "type": "chatgpt",
                    "email": "demo@example.com",
                    "planType": "plus"
                },
                "requiresOpenaiAuth": False
            }
        })
    elif method == "model/list":
        write({
            "id": request_id,
            "result": {
                "data": [
                    {
                        "id": "gpt-5",
                        "displayName": "GPT-5",
                        "description": "Default coding model",
                        "defaultReasoningEffort": "medium",
                        "supportedReasoningEfforts": ["low", "medium", "high"],
                        "additionalSpeedTiers": ["fast", "flex"],
                        "inputModalities": ["text", "image"],
                        "isDefault": True
                    }
                ]
            }
        })
    elif method == "collaborationMode/list":
        write({
            "id": request_id,
            "result": {
                "data": [
                    {
                        "name": "Default",
                        "mode": "default",
                        "model": None,
                        "reasoning_effort": None
                    },
                    {
                        "name": "Plan",
                        "mode": "plan",
                        "model": None,
                        "reasoning_effort": "high"
                    }
                ]
            }
        })
    else:
        write({
            "id": request_id,
            "error": {
                "code": -32000,
                "message": f"unknown method: {method}"
            }
        })
"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&fake_server_path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&fake_server_path, permissions).unwrap();
        }
        state.app_servers = AppServerManager::new(AppServerClientConfig {
            codex_bin: fake_server_path.display().to_string(),
            ..AppServerClientConfig::default()
        });
        state
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

    #[test]
    fn maps_session_item_notifications_for_stream_clients() {
        let mapped = map_app_server_session_notification(&AppServerNotification {
            method: "item/started".to_string(),
            params: json!({
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "item-1",
                "item": {
                    "id": "item-1",
                    "type": "commandExecution",
                    "command": ["sed", "-n", "1,20p", "src/main.rs"],
                    "cwd": "/tmp/project"
                }
            }),
        })
        .expect("notification should map");

        assert_eq!(
            mapped,
            json!({
                "kind": "notification",
                "method": "item/started",
                "params": {
                    "threadId": "thread-1",
                    "turnId": "turn-1",
                    "itemId": "item-1",
                    "item": {
                        "id": "item-1",
                        "type": "commandExecution",
                        "command": ["sed", "-n", "1,20p", "src/main.rs"],
                        "cwd": "/tmp/project",
                        "title": "Command",
                        "detailState": "deferred",
                        "detailPreview": "sed -n 1,20p src/main.rs"
                    }
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn git_worktree_payloads_use_rust_git_helpers() {
        let sandbox = unique_test_dir("git-worktrees");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        let repo = workspace.join("repo");
        let worktree = workspace.join(".codex-webui-worktrees").join("feature");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&codex_home).unwrap();
        init_test_git_repo(&repo);

        let state = test_state(workspace.clone(), vec![workspace.clone()], codex_home);

        let created = create_git_worktree_payload(
            &state,
            repo.to_str().unwrap(),
            worktree.to_str().unwrap(),
            Some("feature/test"),
            true,
            false,
        )
        .await
        .unwrap();
        assert_eq!(
            created.get("repoPath").and_then(Value::as_str),
            Some(repo.to_str().unwrap())
        );
        assert!(
            created
                .get("worktrees")
                .and_then(Value::as_array)
                .is_some_and(|worktrees| {
                    worktrees.iter().any(|entry| {
                        entry.get("path").and_then(Value::as_str)
                            == Some(worktree.to_str().unwrap())
                            && entry.get("branch").and_then(Value::as_str) == Some("feature/test")
                    })
                })
        );

        let listed = list_git_worktrees_payload(&state, repo.to_str().unwrap())
            .await
            .unwrap();
        assert!(
            listed
                .get("worktrees")
                .and_then(Value::as_array)
                .is_some_and(|worktrees| worktrees.len() >= 2)
        );
        let repositories = list_git_repositories_payload(&state, false).await.unwrap();
        assert!(
            repositories
                .get("repositories")
                .and_then(Value::as_array)
                .is_some_and(|repositories| repositories.iter().any(|entry| {
                    entry.get("path").and_then(Value::as_str) == Some(worktree.to_str().unwrap())
                }))
        );

        let removed = remove_git_worktree_payload(
            &state,
            repo.to_str().unwrap(),
            worktree.to_str().unwrap(),
            false,
        )
        .await
        .unwrap();
        assert!(
            removed
                .get("worktrees")
                .and_then(Value::as_array)
                .is_some_and(|worktrees| {
                    !worktrees.iter().any(|entry| {
                        entry.get("path").and_then(Value::as_str)
                            == Some(worktree.to_str().unwrap())
                    })
                })
        );

        let _ = fs::remove_dir_all(sandbox);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn git_read_payloads_use_rust_helpers() {
        let sandbox = unique_test_dir("git-read");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        let repo = workspace.join("repo");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&codex_home).unwrap();
        init_test_git_repo(&repo);
        fs::write(repo.join("README.md"), "changed\n").unwrap();
        fs::write(repo.join("notes.txt"), "todo\n").unwrap();

        let state = test_state(workspace.clone(), vec![workspace.clone()], codex_home);

        let repositories = list_git_repositories_payload(&state, false).await.unwrap();
        assert!(
            repositories
                .get("repositories")
                .and_then(Value::as_array)
                .is_some_and(|repositories| repositories.iter().any(|entry| {
                    entry.get("path").and_then(Value::as_str) == Some(repo.to_str().unwrap())
                }))
        );

        let status = get_git_status_payload(&state, repo.to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(
            status
                .get("repo")
                .and_then(|value| value.get("path"))
                .and_then(Value::as_str),
            Some(repo.to_str().unwrap())
        );
        assert_eq!(status.get("clean").and_then(Value::as_bool), Some(false));
        assert!(
            status
                .get("files")
                .and_then(Value::as_array)
                .is_some_and(|files| files.iter().any(|entry| {
                    entry.get("path").and_then(Value::as_str) == Some("README.md")
                        && entry.get("unstagedLabel").and_then(Value::as_str) == Some("modified")
                }))
        );
        assert!(
            status
                .get("files")
                .and_then(Value::as_array)
                .is_some_and(|files| files.iter().any(|entry| {
                    entry.get("path").and_then(Value::as_str) == Some("notes.txt")
                        && entry.get("isUntracked").and_then(Value::as_bool) == Some(true)
                }))
        );

        let file_payload = get_git_file_payload(&state, repo.to_str().unwrap(), "README.md")
            .await
            .unwrap();
        assert_eq!(
            file_payload.get("originalContent").and_then(Value::as_str),
            Some("init\n")
        );
        assert_eq!(
            file_payload.get("modifiedContent").and_then(Value::as_str),
            Some("changed\n")
        );

        let diff_payload = get_git_commit_diff_payload(&state, repo.to_str().unwrap(), "HEAD")
            .await
            .unwrap();
        assert!(
            diff_payload
                .get("diff")
                .and_then(Value::as_str)
                .is_some_and(|diff| diff.contains("README.md"))
        );

        let resolved = resolve_git_file_from_absolute_path_payload(
            &state,
            repo.join("README.md").to_str().unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(
            resolved.get("repoPath").and_then(Value::as_str),
            Some(repo.to_str().unwrap())
        );
        assert_eq!(
            resolved.get("filePath").and_then(Value::as_str),
            Some("README.md")
        );

        let _ = fs::remove_dir_all(sandbox);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn git_write_payloads_use_rust_helpers() {
        let sandbox = unique_test_dir("git-write");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        let repo = workspace.join("repo");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&codex_home).unwrap();
        init_test_git_repo(&repo);

        let state = test_state(workspace.clone(), vec![workspace.clone()], codex_home);

        let saved = save_git_file_payload(&state, repo.to_str().unwrap(), "src/new.txt", "hello\n")
            .await
            .unwrap();
        assert_eq!(
            saved.get("modifiedContent").and_then(Value::as_str),
            Some("hello\n")
        );

        let staged = stage_git_changes_payload(&state, repo.to_str().unwrap(), Some("src/new.txt"))
            .await
            .unwrap();
        assert!(
            staged
                .get("files")
                .and_then(Value::as_array)
                .is_some_and(|files| files.iter().any(|entry| {
                    entry.get("path").and_then(Value::as_str) == Some("src/new.txt")
                        && entry.get("hasStagedChanges").and_then(Value::as_bool) == Some(true)
                }))
        );

        let unstaged =
            unstage_git_changes_payload(&state, repo.to_str().unwrap(), Some("src/new.txt"))
                .await
                .unwrap();
        assert!(
            unstaged
                .get("files")
                .and_then(Value::as_array)
                .is_some_and(|files| files.iter().any(|entry| {
                    entry.get("isUntracked").and_then(Value::as_bool) == Some(true)
                }))
        );

        stage_git_changes_payload(&state, repo.to_str().unwrap(), None)
            .await
            .unwrap();
        let committed = commit_git_changes_payload(&state, repo.to_str().unwrap(), "add new file")
            .await
            .unwrap();
        assert_eq!(committed.get("clean").and_then(Value::as_bool), Some(true));
        assert_eq!(
            committed
                .get("commits")
                .and_then(Value::as_array)
                .and_then(|commits| commits.first())
                .and_then(|commit| commit.get("subject"))
                .and_then(Value::as_str),
            Some("add new file")
        );

        let switched =
            checkout_git_branch_payload(&state, repo.to_str().unwrap(), "feature/test", true)
                .await
                .unwrap();
        assert_eq!(
            switched.get("branch").and_then(Value::as_str),
            Some("feature/test")
        );

        let _ = fs::remove_dir_all(sandbox);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn git_http_handlers_use_rust_routes() {
        let sandbox = unique_test_dir("git-http");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        let repo = workspace.join("repo");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&codex_home).unwrap();
        init_test_git_repo(&repo);

        let state = test_state(workspace.clone(), vec![workspace.clone()], codex_home);
        let auth = AuthContext {
            role: UserRole::Admin,
            profile_id: "default".to_string(),
        };

        let repositories_request = Request::builder()
            .method(Method::GET)
            .uri("/api/git/repositories")
            .body(Body::empty())
            .unwrap();
        let repositories_response = handle_git_api_http(
            state.clone(),
            repositories_request,
            auth.clone(),
            "/api/git/repositories",
        )
        .await;
        assert_eq!(repositories_response.status(), StatusCode::OK);
        let repositories_body = to_bytes(repositories_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let repositories_payload: Value = serde_json::from_slice(&repositories_body).unwrap();
        assert_eq!(
            repositories_payload
                .get("repositories")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
        );

        fs::create_dir_all(repo.join("src")).unwrap();
        fs::write(repo.join("src").join("queue.rs"), "pub fn run() {}\n").unwrap();
        let stage_request = Request::builder()
            .method(Method::POST)
            .uri("/api/git/stage")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "repoPath": repo.display().to_string(),
                    "filePath": "src/queue.rs"
                })
                .to_string(),
            ))
            .unwrap();
        let stage_response =
            handle_git_api_http(state.clone(), stage_request, auth, "/api/git/stage").await;
        assert_eq!(stage_response.status(), StatusCode::OK);
        let stage_body = to_bytes(stage_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let stage_payload: Value = serde_json::from_slice(&stage_body).unwrap();
        assert_eq!(
            stage_payload
                .get("files")
                .and_then(Value::as_array)
                .map(|files| files.iter().any(|entry| {
                    entry.get("path").and_then(Value::as_str) == Some("src/queue.rs")
                        && entry
                            .get("hasStagedChanges")
                            .and_then(Value::as_bool)
                            .unwrap_or(false)
                })),
            Some(true)
        );

        let _ = fs::remove_dir_all(sandbox);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn static_asset_handler_rewrites_base_path_and_uses_spa_fallbacks() {
        let sandbox = unique_test_dir("static-assets");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        let static_dir = workspace.join("static");
        fs::create_dir_all(static_dir.join("_app").join("immutable")).unwrap();
        fs::create_dir_all(&codex_home).unwrap();
        fs::write(
            static_dir.join("index.html"),
            "<html><body>/__CODEX_WEBUI_BASE__/index</body></html>",
        )
        .unwrap();
        fs::write(
            static_dir.join("200.html"),
            "<html><body>/__CODEX_WEBUI_BASE__/fallback</body></html>",
        )
        .unwrap();
        fs::write(
            static_dir.join("_app").join("immutable").join("app.js"),
            "window.__BASE__ = '/__CODEX_WEBUI_BASE__';",
        )
        .unwrap();

        let state = test_state_with_static_dir_and_base_path(
            workspace.clone(),
            vec![workspace.clone()],
            codex_home,
            static_dir,
            "/absproxy/4173",
        );

        let root_response = serve_static_asset(state.clone(), "/").await;
        assert_eq!(root_response.status(), StatusCode::OK);
        assert_eq!(
            root_response.headers().get(header::CACHE_CONTROL),
            Some(&HeaderValue::from_static("no-cache"))
        );
        let root_body = to_bytes(root_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let root_text = String::from_utf8(root_body.to_vec()).unwrap();
        assert!(root_text.contains("/absproxy/4173/index"));
        assert!(!root_text.contains(STATIC_BASE_PLACEHOLDER));

        let session_response = serve_static_asset(state.clone(), "/sessions/thread-1").await;
        assert_eq!(session_response.status(), StatusCode::OK);
        let session_body = to_bytes(session_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let session_text = String::from_utf8(session_body.to_vec()).unwrap();
        assert!(session_text.contains("/absproxy/4173/fallback"));
        assert!(!session_text.contains(STATIC_BASE_PLACEHOLDER));

        let asset_response = serve_static_asset(state, "/_app/immutable/app.js").await;
        assert_eq!(asset_response.status(), StatusCode::OK);
        assert_eq!(
            asset_response.headers().get(header::CACHE_CONTROL),
            Some(&HeaderValue::from_static(
                "public, max-age=31536000, immutable"
            ))
        );
        let asset_body = to_bytes(asset_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let asset_text = String::from_utf8(asset_body.to_vec()).unwrap();
        assert!(asset_text.contains("/absproxy/4173"));
        assert!(!asset_text.contains(STATIC_BASE_PLACEHOLDER));

        let _ = fs::remove_dir_all(sandbox);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn static_asset_handler_rejects_invalid_and_missing_paths() {
        let sandbox = unique_test_dir("static-assets-404");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        let static_dir = workspace.join("static");
        fs::create_dir_all(&static_dir).unwrap();
        fs::create_dir_all(&codex_home).unwrap();
        fs::write(static_dir.join("index.html"), "<html>ok</html>").unwrap();
        fs::write(static_dir.join("200.html"), "<html>fallback</html>").unwrap();

        let state = test_state_with_static_dir_and_base_path(
            workspace.clone(),
            vec![workspace.clone()],
            codex_home,
            static_dir,
            "",
        );

        let invalid_response = serve_static_asset(state.clone(), "/../../secret.txt").await;
        assert_eq!(invalid_response.status(), StatusCode::NOT_FOUND);

        let missing_asset_response = serve_static_asset(state, "/missing.css").await;
        assert_eq!(missing_asset_response.status(), StatusCode::NOT_FOUND);

        let _ = fs::remove_dir_all(sandbox);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn git_fetch_and_pull_payloads_use_rust_helpers() {
        let sandbox = unique_test_dir("git-fetch-pull");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        let seed = workspace.join("seed");
        let remote = workspace.join("remote.git");
        let local = workspace.join("local");
        let updater = workspace.join("updater");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&codex_home).unwrap();
        init_test_git_repo(&seed);

        let current_branch = {
            let output = std::process::Command::new("git")
                .args(["-C", seed.to_str().unwrap(), "branch", "--show-current"])
                .output()
                .unwrap();
            assert!(output.status.success());
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };

        let clone_bare = std::process::Command::new("git")
            .args([
                "clone",
                "--bare",
                seed.to_str().unwrap(),
                remote.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(clone_bare.status.success(), "git clone --bare failed");

        let clone_local = std::process::Command::new("git")
            .args(["clone", remote.to_str().unwrap(), local.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(clone_local.status.success(), "git clone local failed");

        let clone_updater = std::process::Command::new("git")
            .args(["clone", remote.to_str().unwrap(), updater.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(clone_updater.status.success(), "git clone updater failed");
        for args in [
            vec![
                "-C",
                updater.to_str().unwrap(),
                "config",
                "user.name",
                "Codex WebUI",
            ],
            vec![
                "-C",
                updater.to_str().unwrap(),
                "config",
                "user.email",
                "codex-webui@example.com",
            ],
        ] {
            let output = std::process::Command::new("git")
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "git updater config failed");
        }

        fs::write(updater.join("README.md"), "remote update\n").unwrap();
        let add = std::process::Command::new("git")
            .args(["-C", updater.to_str().unwrap(), "add", "README.md"])
            .output()
            .unwrap();
        assert!(add.status.success(), "git add updater failed");
        let commit = std::process::Command::new("git")
            .args([
                "-C",
                updater.to_str().unwrap(),
                "commit",
                "-m",
                "remote update",
            ])
            .output()
            .unwrap();
        assert!(commit.status.success(), "git commit updater failed");
        let push = std::process::Command::new("git")
            .args([
                "-C",
                updater.to_str().unwrap(),
                "push",
                "origin",
                &current_branch,
            ])
            .output()
            .unwrap();
        assert!(push.status.success(), "git push updater failed");

        let state = test_state(workspace.clone(), vec![workspace.clone()], codex_home);

        let fetched = fetch_git_repository_payload(&state, local.to_str().unwrap())
            .await
            .unwrap();
        assert!(
            fetched
                .get("behind")
                .and_then(Value::as_u64)
                .is_some_and(|value| value >= 1)
        );

        let pulled = pull_git_repository_payload(&state, local.to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(pulled.get("clean").and_then(Value::as_bool), Some(true));
        assert_eq!(
            fs::read_to_string(local.join("README.md")).unwrap(),
            "remote update\n"
        );

        let _ = fs::remove_dir_all(sandbox);
    }

    #[test]
    fn parses_github_remote_urls() {
        let ssh =
            parse_github_remote_payload("origin", "git@github.com:openai/codex-webui.git").unwrap();
        assert_eq!(ssh.get("host").and_then(Value::as_str), Some("github.com"));
        assert_eq!(ssh.get("owner").and_then(Value::as_str), Some("openai"));
        assert_eq!(ssh.get("name").and_then(Value::as_str), Some("codex-webui"));

        let https =
            parse_github_remote_payload("upstream", "https://github.com/openai/codex-webui.git")
                .unwrap();
        assert_eq!(
            https.get("remoteName").and_then(Value::as_str),
            Some("upstream")
        );
        assert_eq!(
            https.get("url").and_then(Value::as_str),
            Some("https://github.com/openai/codex-webui")
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resolves_github_repository_payload_from_git_remote() {
        let sandbox = unique_test_dir("github-remote");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        let repo = workspace.join("repo");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&codex_home).unwrap();
        init_test_git_repo(&repo);

        let remote = std::process::Command::new("git")
            .args([
                "-C",
                repo.to_str().unwrap(),
                "remote",
                "add",
                "origin",
                "git@github.com:openai/codex-webui.git",
            ])
            .output()
            .unwrap();
        assert!(remote.status.success(), "git remote add failed");

        let state = test_state(workspace.clone(), vec![workspace.clone()], codex_home);
        let repository = resolve_github_repository_payload(&state, repo.to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(
            repository.get("owner").and_then(Value::as_str),
            Some("openai")
        );
        assert_eq!(
            repository.get("name").and_then(Value::as_str),
            Some("codex-webui")
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

    #[tokio::test]
    async fn notification_helpers_update_ui_state_and_counts() {
        let sandbox = unique_test_dir("notifications");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&codex_home).unwrap();

        let state = test_state(workspace.clone(), vec![workspace], codex_home);
        let ui_state_path = profile_ui_state_path(&state.config, "default");
        fs::create_dir_all(ui_state_path.parent().unwrap()).unwrap();
        fs::write(
            &ui_state_path,
            serde_json::to_vec_pretty(&json!({
                "global": {
                    "shutdownAfterQueueCompletes": false,
                    "scheduledShutdown": Value::Null
                },
                "notifications": {
                    "items": [
                        {
                            "id": "n1",
                            "type": "sessionCompleted",
                            "createdAt": 20,
                            "readAt": Value::Null,
                            "sessionId": "s1",
                            "sessionName": "One",
                            "payload": {}
                        },
                        {
                            "id": "n2",
                            "type": "sessionAttention",
                            "createdAt": 10,
                            "readAt": Value::Null,
                            "sessionId": "s2",
                            "sessionName": "Two",
                            "payload": {}
                        }
                    ],
                    "settings": {
                        "enabledEventTypes": ["sessionCompleted"],
                        "slackWebhookUrl": "",
                        "webhookUrl": Value::Null
                    }
                },
                "sessionMetaByThreadId": {},
                "savedSessionFilters": [],
                "promptPresets": [],
                "automations": [],
                "automationRuns": [],
                "preferencesByThreadId": {},
                "draftsByThreadId": {},
                "queuesByThreadId": {},
                "highlightsByThreadId": {}
            }))
            .unwrap(),
        )
        .unwrap();

        let listed = get_notifications_payload(&state, "default", 80)
            .await
            .unwrap();
        assert_eq!(listed.get("unreadCount").and_then(Value::as_u64), Some(2));

        let marked =
            mark_notifications_read_payload(&state, "default", Some(vec!["n1".to_string()]))
                .await
                .unwrap();
        assert_eq!(marked.get("unreadCount").and_then(Value::as_u64), Some(1));

        let settings = update_notification_settings_payload(
            &state,
            "default",
            json!({
                "enabledEventTypes": ["sessionAttention", "invalid"],
                "slackWebhookUrl": " https://hooks.slack.test/one ",
                "webhookUrl": ""
            }),
        )
        .await
        .unwrap();
        assert_eq!(
            settings
                .get("settings")
                .and_then(|value| value.get("enabledEventTypes"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            vec![json!("sessionAttention")]
        );
        assert_eq!(
            settings
                .get("settings")
                .and_then(|value| value.get("slackWebhookUrl"))
                .and_then(Value::as_str),
            Some("https://hooks.slack.test/one")
        );
        assert!(
            settings
                .get("settings")
                .and_then(|value| value.get("webhookUrl"))
                .is_some_and(Value::is_null)
        );

        let cleared = clear_notifications_payload(&state, "default")
            .await
            .unwrap();
        assert_eq!(cleared.get("unreadCount").and_then(Value::as_u64), Some(0));
        assert_eq!(
            cleared
                .get("notifications")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(0)
        );

        let _ = fs::remove_dir_all(sandbox);
    }

    #[tokio::test]
    async fn theme_settings_round_trip_through_rust_store() {
        let sandbox = unique_test_dir("theme-settings");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&codex_home).unwrap();

        let state = test_state(workspace.clone(), vec![workspace], codex_home);
        let expected = json!({
            "light": {
                "bg": "#fffef7",
                "sidebar": "#f8f1de"
            },
            "dark": {
                "bg": "#181713",
                "sidebar": "#12110d"
            }
        });

        write_stored_theme_settings(&state.config, "default", &expected)
            .await
            .expect("theme settings should save");
        let restored = read_stored_theme_settings(&state.config, "default")
            .await
            .expect("theme settings should load");

        assert_eq!(restored, Some(expected));

        let _ = fs::remove_dir_all(sandbox);
    }

    #[tokio::test]
    async fn syncs_codex_toml_with_preferences_for_plan_mode() {
        let sandbox = unique_test_dir("sync-codex-toml");
        let codex_home = sandbox.join("codex-home");
        fs::create_dir_all(&codex_home).unwrap();

        sync_codex_toml_with_preferences(
            &codex_home,
            &json!({
                "model": "gpt-5.4",
                "approvalPolicy": "on-request",
                "sandboxMode": "workspace-write",
                "speed": "fast",
                "mode": "plan",
                "effort": "high",
                "networkAccess": true
            }),
        )
        .await
        .expect("config.toml should sync");

        let raw = fs::read_to_string(config_toml_path(&codex_home)).unwrap();
        assert!(raw.contains("model = \"gpt-5.4\""));
        assert!(raw.contains("approval_policy = \"on-request\""));
        assert!(raw.contains("sandbox_mode = \"workspace-write\""));
        assert!(raw.contains("service_tier = \"fast\""));
        assert!(raw.contains("plan_mode_reasoning_effort = \"high\""));
        assert!(raw.contains("[sandbox_workspace_write]"));
        assert!(raw.contains("network_access = true"));

        let _ = fs::remove_dir_all(sandbox);
    }

    #[tokio::test]
    async fn updates_session_organization_and_known_tags() {
        let sandbox = unique_test_dir("session-organization");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&codex_home).unwrap();

        let state = test_state(workspace.clone(), vec![workspace], codex_home);
        let ui_state_path = profile_ui_state_path(&state.config, "default");
        fs::create_dir_all(ui_state_path.parent().unwrap()).unwrap();
        fs::write(
            &ui_state_path,
            serde_json::to_vec_pretty(&json!({
                "global": {
                    "shutdownAfterQueueCompletes": false,
                    "scheduledShutdown": Value::Null
                },
                "notifications": {
                    "items": [],
                    "settings": default_notification_settings_value()
                },
                "sessionMetaByThreadId": {
                    "session-1": {
                        "pinned": false,
                        "tags": ["alpha"]
                    }
                },
                "savedSessionFilters": [],
                "promptPresets": [],
                "automations": [],
                "automationRuns": [],
                "preferencesByThreadId": {},
                "draftsByThreadId": {},
                "queuesByThreadId": {},
                "highlightsByThreadId": {}
            }))
            .unwrap(),
        )
        .unwrap();

        let payload = update_session_organization_payload(
            &state,
            "default",
            "session-1",
            json!({
                "pinned": true,
                "tags": ["beta", "alpha", "beta", " "]
            }),
        )
        .await
        .expect("session organization should update");

        assert_eq!(
            payload.get("meta"),
            Some(&json!({
                "pinned": true,
                "tags": ["alpha", "beta"]
            }))
        );
        assert_eq!(
            payload
                .get("knownTags")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            vec![json!("alpha"), json!("beta")]
        );

        let _ = fs::remove_dir_all(sandbox);
    }

    #[tokio::test]
    async fn saves_filters_and_prompt_presets_with_normalization() {
        let sandbox = unique_test_dir("ui-state-saves");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&codex_home).unwrap();

        let state = test_state(workspace.clone(), vec![workspace], codex_home);
        let ui_state_path = profile_ui_state_path(&state.config, "default");
        fs::create_dir_all(ui_state_path.parent().unwrap()).unwrap();
        fs::write(
            &ui_state_path,
            serde_json::to_vec_pretty(&json!({
                "global": {
                    "shutdownAfterQueueCompletes": false,
                    "scheduledShutdown": Value::Null
                },
                "notifications": {
                    "items": [],
                    "settings": default_notification_settings_value()
                },
                "sessionMetaByThreadId": {
                    "thread-1": {
                        "pinned": true,
                        "tags": ["alpha", "beta"]
                    }
                },
                "savedSessionFilters": [],
                "promptPresets": [],
                "automations": [],
                "automationRuns": [],
                "preferencesByThreadId": {},
                "draftsByThreadId": {},
                "queuesByThreadId": {},
                "highlightsByThreadId": {}
            }))
            .unwrap(),
        )
        .unwrap();

        let filters = save_session_filter_payload(
            &state,
            "default",
            json!({
                "id": "filter-1",
                "name": "  Important  ",
                "pinnedOnly": true,
                "runningOnly": false,
                "queuedOnly": true,
                "highlight": "completed",
                "tags": ["beta", "alpha", "beta", ""]
            }),
        )
        .await
        .unwrap();
        assert_eq!(
            filters
                .get("savedFilters")
                .and_then(Value::as_array)
                .and_then(|entries| entries.first())
                .and_then(|entry| entry.get("name"))
                .and_then(Value::as_str),
            Some("Important")
        );
        assert_eq!(
            filters
                .get("savedFilters")
                .and_then(Value::as_array)
                .and_then(|entries| entries.first())
                .and_then(|entry| entry.get("tags"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            vec![json!("alpha"), json!("beta")]
        );
        assert_eq!(
            filters
                .get("knownTags")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            vec![json!("alpha"), json!("beta")]
        );

        let presets = save_prompt_preset_payload(
            &state,
            "default",
            json!({
                "id": "preset-1",
                "name": "  Draft reply  ",
                "prompt": "Use the existing repo style.",
                "createdAt": 5
            }),
        )
        .await
        .unwrap();
        let first_preset = presets
            .get("promptPresets")
            .and_then(Value::as_array)
            .and_then(|entries| entries.first())
            .cloned()
            .unwrap();
        assert_eq!(
            first_preset.get("name").and_then(Value::as_str),
            Some("Draft reply")
        );
        assert_eq!(
            first_preset.get("createdAt").and_then(Value::as_i64),
            Some(5)
        );
        assert!(
            first_preset
                .get("updatedAt")
                .and_then(Value::as_i64)
                .is_some_and(|value| value >= 5)
        );

        let deleted_filters = delete_session_filter_payload(&state, "default", "filter-1")
            .await
            .unwrap();
        assert_eq!(
            deleted_filters
                .get("savedFilters")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(0)
        );

        let deleted_presets = delete_prompt_preset_payload(&state, "default", "preset-1")
            .await
            .unwrap();
        assert_eq!(
            deleted_presets
                .get("promptPresets")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(0)
        );

        let _ = fs::remove_dir_all(sandbox);
    }

    #[tokio::test]
    async fn saves_and_deletes_automations_with_normalization() {
        let sandbox = unique_test_dir("automation-saves");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&codex_home).unwrap();

        let state = test_state(workspace.clone(), vec![workspace], codex_home);
        let ui_state_path = profile_ui_state_path(&state.config, "default");
        fs::create_dir_all(ui_state_path.parent().unwrap()).unwrap();
        fs::write(
            &ui_state_path,
            serde_json::to_vec_pretty(&json!({
                "global": {
                    "shutdownAfterQueueCompletes": false,
                    "scheduledShutdown": Value::Null
                },
                "notifications": {
                    "items": [],
                    "settings": default_notification_settings_value()
                },
                "sessionMetaByThreadId": {},
                "savedSessionFilters": [],
                "promptPresets": [],
                "automations": [],
                "automationRuns": [],
                "preferencesByThreadId": {},
                "draftsByThreadId": {},
                "queuesByThreadId": {},
                "highlightsByThreadId": {}
            }))
            .unwrap(),
        )
        .unwrap();

        let saved = save_automation_payload(
            &state,
            "default",
            json!({
                "id": "auto-1",
                "name": "  Morning Review  ",
                "prompt": "Check the repo state.",
                "enabled": true,
                "scheduleMode": "interval",
                "intervalMinutes": 5,
                "target": "local",
                "repoPath": "",
                "cwd": " /tmp/review ",
                "model": "gpt-5.4",
                "effort": "high",
                "speed": "fast",
                "mode": "plan"
            }),
        )
        .await
        .unwrap();

        let first_automation = saved
            .get("automations")
            .and_then(Value::as_array)
            .and_then(|entries| entries.first())
            .cloned()
            .unwrap();
        assert_eq!(
            first_automation.get("name").and_then(Value::as_str),
            Some("Morning Review")
        );
        assert_eq!(
            first_automation.get("scheduleMode").and_then(Value::as_str),
            Some("interval")
        );
        assert_eq!(
            first_automation
                .get("intervalMinutes")
                .and_then(Value::as_i64),
            Some(5)
        );
        assert_eq!(
            first_automation.get("cwd").and_then(Value::as_str),
            Some("/tmp/review")
        );
        assert!(
            first_automation
                .get("nextRunAt")
                .and_then(Value::as_i64)
                .is_some()
        );

        let deleted = delete_automation_payload(&state, "default", "auto-1")
            .await
            .unwrap();
        assert_eq!(
            deleted
                .get("automations")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(0)
        );

        let _ = fs::remove_dir_all(sandbox);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn create_session_payload_uses_app_server_and_persists_preferences() {
        let sandbox = unique_test_dir("session-create-rust");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&codex_home).unwrap();

        let state =
            test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
        let created = create_session_payload(
            &state,
            "default",
            json!({
                "cwd": workspace.display().to_string(),
                "model": "gpt-5.4",
                "approvalPolicy": "on-request",
                "sandboxMode": "workspace-write"
            }),
            Some("Review docs"),
        )
        .await
        .unwrap();

        assert_eq!(
            created.get("name").and_then(Value::as_str),
            Some("Review docs")
        );
        let session_id = created
            .get("id")
            .and_then(Value::as_str)
            .unwrap()
            .to_string();

        let stored_preferences = with_ui_state_read(&state, "default", |ui_state| {
            Ok(ui_state
                .get("preferencesByThreadId")
                .and_then(Value::as_object)
                .and_then(|entries| entries.get(&session_id))
                .cloned()
                .unwrap_or(Value::Null))
        })
        .await
        .unwrap();
        assert_eq!(
            stored_preferences.get("model").and_then(Value::as_str),
            Some("gpt-5.4")
        );

        let thread = app_server_client(&state, "default")
            .await
            .unwrap()
            .request(
                "thread/read",
                json!({
                    "threadId": session_id,
                    "includeTurns": false
                }),
            )
            .await
            .unwrap();
        assert_eq!(
            thread
                .get("thread")
                .and_then(|value| value.get("name"))
                .and_then(Value::as_str),
            Some("Review docs")
        );

        let _ = fs::remove_dir_all(sandbox);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rust_session_list_and_search_use_app_server_threads() {
        let sandbox = unique_test_dir("session-list-rust");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&codex_home).unwrap();

        let state =
            test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
        let first = create_session_payload(
            &state,
            "default",
            json!({ "cwd": workspace.display().to_string() }),
            Some("Build Docs"),
        )
        .await
        .unwrap();
        let second = create_session_payload(
            &state,
            "default",
            json!({ "cwd": workspace.display().to_string() }),
            Some("Fix Queue"),
        )
        .await
        .unwrap();
        let first_id = first.get("id").and_then(Value::as_str).unwrap().to_string();
        let second_id = second
            .get("id")
            .and_then(Value::as_str)
            .unwrap()
            .to_string();

        with_ui_state_write(&state, "default", |ui_state| {
            let Some(session_meta) = ui_state
                .get_mut("sessionMetaByThreadId")
                .and_then(Value::as_object_mut)
            else {
                return Err(api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "session meta state is missing",
                ));
            };
            session_meta.insert(
                first_id.clone(),
                json!({
                    "pinned": true,
                    "tags": ["docs"]
                }),
            );
            let Some(queues) = ui_state
                .get_mut("queuesByThreadId")
                .and_then(Value::as_object_mut)
            else {
                return Err(api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "queue state is missing",
                ));
            };
            queues.insert(
                second_id.clone(),
                json!({
                    "items": [
                        {
                            "id": "queue-1",
                            "prompt": "follow up"
                        }
                    ],
                    "resumePending": false,
                    "updatedAt": 10
                }),
            );
            Ok(())
        })
        .await
        .unwrap();

        let pinned_only = list_sessions_payload(
            &state,
            "default",
            false,
            None,
            20,
            &SessionFilterCriteria {
                pinned_only: true,
                ..SessionFilterCriteria::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            pinned_only
                .get("sessions")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(
            pinned_only
                .get("sessions")
                .and_then(Value::as_array)
                .and_then(|entries| entries.first())
                .and_then(|entry| entry.get("id"))
                .and_then(Value::as_str),
            Some(first_id.as_str())
        );

        let queued_only = list_sessions_payload(
            &state,
            "default",
            false,
            None,
            20,
            &SessionFilterCriteria {
                queued_only: true,
                ..SessionFilterCriteria::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            queued_only
                .get("sessions")
                .and_then(Value::as_array)
                .and_then(|entries| entries.first())
                .and_then(|entry| entry.get("id"))
                .and_then(Value::as_str),
            Some(second_id.as_str())
        );

        let matched = search_sessions_payload(
            &state,
            "default",
            "queue",
            "summary",
            false,
            None,
            20,
            &SessionFilterCriteria::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            matched
                .get("sessions")
                .and_then(Value::as_array)
                .and_then(|entries| entries.first())
                .and_then(|entry| entry.get("name"))
                .and_then(Value::as_str),
            Some("Fix Queue")
        );

        let uppercase = search_sessions_payload(
            &state,
            "default",
            "BUILD",
            "summary",
            false,
            None,
            20,
            &SessionFilterCriteria::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            uppercase
                .get("sessions")
                .and_then(Value::as_array)
                .and_then(|entries| entries.first())
                .and_then(|entry| entry.get("name"))
                .and_then(Value::as_str),
            Some("Build Docs")
        );

        app_server_client(&state, "default")
            .await
            .unwrap()
            .request(
                "thread/seed",
                json!({
                    "thread": {
                        "id": "thread-full",
                        "name": "Research notes",
                        "preview": "Unrelated summary",
                        "cwd": workspace.display().to_string(),
                        "archived": false,
                        "createdAt": 3,
                        "updatedAt": 4,
                        "status": "idle",
                        "isSubagent": false,
                        "agentNickname": Value::Null,
                        "agentRole": Value::Null,
                        "turns": [
                            {
                                "id": "turn-1",
                                "status": "completed",
                                "error": Value::Null,
                                "startedAt": 30,
                                "completedAt": 40,
                                "durationMs": 10,
                                "items": [
                                    {
                                        "id": "item-1",
                                        "type": "assistantMessage",
                                        "text": "The websocket duplicate send race originates from optimistic queue replay."
                                    }
                                ]
                            }
                        ]
                    }
                }),
            )
            .await
            .unwrap();

        let full_text = search_sessions_payload(
            &state,
            "default",
            "optimistic queue replay",
            "full",
            false,
            None,
            20,
            &SessionFilterCriteria::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            full_text
                .get("sessions")
                .and_then(Value::as_array)
                .and_then(|entries| entries.first())
                .and_then(|entry| entry.get("id"))
                .and_then(Value::as_str),
            Some("thread-full")
        );

        let _ = fs::remove_dir_all(sandbox);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn abort_turn_payload_uses_known_active_turn() {
        let sandbox = unique_test_dir("abort-turn-rust");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&codex_home).unwrap();

        let state =
            test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
        state.active_turns.lock().await.insert(
            runtime_session_key("default", "thread-1"),
            "turn-123".to_string(),
        );

        let payload = abort_turn_payload(&state, "default", "thread-1")
            .await
            .unwrap();
        assert_eq!(
            payload.get("interrupted").and_then(Value::as_bool),
            Some(true)
        );

        let _ = fs::remove_dir_all(sandbox);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn session_detail_and_turn_search_payloads_use_rust_thread_reads() {
        let sandbox = unique_test_dir("session-detail-rust");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&codex_home).unwrap();

        let state =
            test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
        app_server_client(&state, "default")
            .await
            .unwrap()
            .request(
                "thread/seed",
                json!({
                    "thread": {
                        "id": "thread-1",
                        "name": "Investigate bug",
                        "preview": "Investigate websocket bug",
                        "cwd": workspace.display().to_string(),
                        "archived": false,
                        "createdAt": 1,
                        "updatedAt": 2,
                        "status": "running",
                        "isSubagent": false,
                        "turns": [
                            {
                                "id": "turn-1",
                                "status": "completed",
                                "error": Value::Null,
                                "startedAt": 10,
                                "completedAt": 20,
                                "durationMs": 10,
                                "items": [
                                    {
                                        "id": "item-1",
                                        "type": "userMessage",
                                        "text": "Find the websocket bug"
                                    },
                                    {
                                        "id": "item-2",
                                        "type": "agentMessage",
                                        "text": "Investigating the websocket bug now"
                                    }
                                ]
                            },
                            {
                                "id": "turn-2",
                                "status": "inProgress",
                                "error": Value::Null,
                                "startedAt": 30,
                                "completedAt": Value::Null,
                                "durationMs": Value::Null,
                                "items": [
                                    {
                                        "id": "item-3",
                                        "type": "reasoning",
                                        "text": "Need to inspect websocket state handling"
                                    }
                                ]
                            }
                        ],
                        "tokenUsage": {
                            "total": { "totalTokens": 12, "inputTokens": 6, "cachedInputTokens": 0, "outputTokens": 6, "reasoningOutputTokens": 2 },
                            "last": { "totalTokens": 7, "inputTokens": 3, "cachedInputTokens": 0, "outputTokens": 4, "reasoningOutputTokens": 1 },
                            "modelContextWindow": 1000
                        }
                    }
                }),
            )
            .await
            .unwrap();

        let detail = session_detail_payload(&state, "default", "thread-1", 1)
            .await
            .unwrap();
        assert_eq!(
            detail
                .get("thread")
                .and_then(|value| value.get("turns"))
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(
            detail.get("activeTurnId").and_then(Value::as_str),
            Some("turn-2")
        );
        assert_eq!(
            detail
                .get("hydration")
                .and_then(|value| value.get("remainingTurns"))
                .and_then(Value::as_u64),
            Some(1)
        );

        let older = session_older_turns_payload(&state, "default", "thread-1", "turn-2", 5)
            .await
            .unwrap();
        assert_eq!(
            older.get("turns").and_then(Value::as_array).map(Vec::len),
            Some(1)
        );

        let turn = session_turn_payload(&state, "default", "thread-1", "turn-1")
            .await
            .unwrap();
        assert_eq!(
            turn.get("turn")
                .and_then(|value| value.get("id"))
                .and_then(Value::as_str),
            Some("turn-1")
        );

        let item = session_item_detail_payload(&state, "default", "thread-1", "turn-1", "item-2")
            .await
            .unwrap();
        assert_eq!(
            item.get("item")
                .and_then(|value| value.get("detailState"))
                .and_then(Value::as_str),
            Some("loaded")
        );

        let search =
            search_session_turns_payload(&state, "default", "thread-1", "websocket", None, 20)
                .await
                .unwrap();
        assert_eq!(
            search
                .get("matches")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(3)
        );

        let _ = fs::remove_dir_all(sandbox);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn send_turn_payload_uses_app_server_and_updates_session_state() {
        let sandbox = unique_test_dir("turn-send-rust");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&codex_home).unwrap();

        let state =
            test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
        let runtime_profile = resolve_runtime_profile(&state.config, "default");
        let uploads_dir = runtime_profile.data_dir.join("uploads").join("thread-1");
        fs::create_dir_all(&uploads_dir).unwrap();

        let text_attachment_path = workspace.join("notes.md");
        let image_attachment_path = workspace.join("diagram.png");
        fs::write(&text_attachment_path, "attachment").unwrap();
        fs::write(&image_attachment_path, "png").unwrap();
        fs::write(
            uploads_dir.join("att-file.json"),
            serde_json::to_vec(&json!({
                "id": "att-file",
                "originalName": "notes.md",
                "path": text_attachment_path.display().to_string(),
                "mimeType": "text/markdown",
                "size": 10,
                "kind": "file",
                "createdAt": "2026-04-20T00:00:00Z"
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            uploads_dir.join("att-image.json"),
            serde_json::to_vec(&json!({
                "id": "att-image",
                "originalName": "diagram.png",
                "path": image_attachment_path.display().to_string(),
                "mimeType": "image/png",
                "size": 12,
                "kind": "image",
                "createdAt": "2026-04-20T00:00:01Z"
            }))
            .unwrap(),
        )
        .unwrap();

        app_server_client(&state, "default")
            .await
            .unwrap()
            .request(
                "thread/seed",
                json!({
                    "thread": {
                        "id": "thread-1",
                        "name": "New thread",
                        "preview": "",
                        "cwd": workspace.display().to_string(),
                        "archived": false,
                        "createdAt": 1,
                        "updatedAt": 1,
                        "status": "notLoaded",
                        "isSubagent": false,
                        "agentNickname": Value::Null,
                        "agentRole": Value::Null,
                        "turns": []
                    }
                }),
            )
            .await
            .unwrap();

        save_session_draft_payload(&state, "default", "thread-1", "Draft to clear", "message")
            .await
            .unwrap();

        let prompt = "Inspect the duplicated websocket send behaviour and capture the root cause before patching it.";
        let payload = send_turn_payload(
            &state,
            "default",
            "thread-1",
            prompt,
            Some(&json!(["att-file", "att-image"])),
            json!({
                "cwd": workspace.display().to_string(),
                "model": "gpt-5",
                "approvalPolicy": "on-request",
                "sandboxMode": "workspace-write",
                "speed": "fast",
                "effort": "high",
                "networkAccess": true
            }),
        )
        .await
        .unwrap();

        assert_eq!(payload.get("ok").and_then(Value::as_bool), Some(true));
        assert_eq!(
            payload.get("turnId").and_then(Value::as_str),
            Some("turn-1")
        );

        let thread = read_thread_payload(&state, "default", "thread-1", true)
            .await
            .unwrap();
        assert_eq!(
            thread.get("status").and_then(Value::as_str),
            Some("running")
        );
        assert_eq!(thread.get("resumeCount").and_then(Value::as_u64), Some(1));
        assert_eq!(
            thread.get("name").and_then(Value::as_str),
            infer_persisted_session_title(prompt).as_deref()
        );
        assert_eq!(
            thread.get("turns").and_then(Value::as_array).map(Vec::len),
            Some(1)
        );

        let last_turn_start = thread.get("lastTurnStart").cloned().unwrap_or(Value::Null);
        assert_eq!(
            last_turn_start.get("serviceTier").and_then(Value::as_str),
            Some("fast")
        );
        let input = last_turn_start
            .get("input")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(input.len(), 2);
        let first_text = input
            .first()
            .and_then(|value| value.get("text"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(first_text.contains(ATTACHMENT_PREAMBLE_START));
        assert!(first_text.contains(ATTACHMENT_PREAMBLE_END));
        assert!(first_text.contains(text_attachment_path.to_str().unwrap()));
        assert!(first_text.contains(prompt.trim()));
        assert_eq!(
            input
                .get(1)
                .and_then(|value| value.get("path"))
                .and_then(Value::as_str),
            Some(image_attachment_path.to_str().unwrap())
        );

        let stored_preferences = with_ui_state_read(&state, "default", |ui_state| {
            Ok(ui_state
                .get("preferencesByThreadId")
                .and_then(Value::as_object)
                .and_then(|entries| entries.get("thread-1"))
                .cloned()
                .unwrap_or(Value::Null))
        })
        .await
        .unwrap();
        assert_eq!(
            stored_preferences.get("model").and_then(Value::as_str),
            Some("gpt-5")
        );

        let draft = get_session_draft_payload(&state, "default", "thread-1")
            .await
            .unwrap();
        assert_eq!(draft.get("draft").and_then(Value::as_str), Some(""));

        let runtime_key = runtime_session_key(
            resolve_runtime_profile_entry(&state.config, "default").0,
            "thread-1",
        );
        assert_eq!(
            state.active_turns.lock().await.get(&runtime_key).cloned(),
            Some("turn-1".to_string())
        );

        let _ = fs::remove_dir_all(sandbox);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn steer_turn_payload_uses_active_turn_from_thread_reads() {
        let sandbox = unique_test_dir("turn-steer-rust");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&codex_home).unwrap();

        let state =
            test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
        let runtime_profile = resolve_runtime_profile(&state.config, "default");
        let uploads_dir = runtime_profile.data_dir.join("uploads").join("thread-1");
        fs::create_dir_all(&uploads_dir).unwrap();

        let text_attachment_path = workspace.join("handoff.md");
        fs::write(&text_attachment_path, "handoff").unwrap();
        fs::write(
            uploads_dir.join("att-file.json"),
            serde_json::to_vec(&json!({
                "id": "att-file",
                "originalName": "handoff.md",
                "path": text_attachment_path.display().to_string(),
                "mimeType": "text/markdown",
                "size": 7,
                "kind": "file",
                "createdAt": "2026-04-20T00:00:00Z"
            }))
            .unwrap(),
        )
        .unwrap();

        app_server_client(&state, "default")
            .await
            .unwrap()
            .request(
                "thread/seed",
                json!({
                    "thread": {
                        "id": "thread-1",
                        "name": "Investigate queue",
                        "preview": "Investigate queue",
                        "cwd": workspace.display().to_string(),
                        "archived": false,
                        "createdAt": 1,
                        "updatedAt": 2,
                        "status": "running",
                        "isSubagent": false,
                        "agentNickname": Value::Null,
                        "agentRole": Value::Null,
                        "turns": [
                            {
                                "id": "turn-1",
                                "status": "completed",
                                "error": Value::Null,
                                "startedAt": 10,
                                "completedAt": 20,
                                "durationMs": 10,
                                "items": []
                            },
                            {
                                "id": "turn-2",
                                "status": "inProgress",
                                "error": Value::Null,
                                "startedAt": 30,
                                "completedAt": Value::Null,
                                "durationMs": Value::Null,
                                "items": []
                            }
                        ]
                    }
                }),
            )
            .await
            .unwrap();

        save_session_draft_payload(&state, "default", "thread-1", "Steer draft", "steer")
            .await
            .unwrap();

        let payload = steer_turn_payload(
            &state,
            "default",
            "thread-1",
            "Focus on the queue deduplication race first.",
            Some(&json!(["att-file"])),
        )
        .await
        .unwrap();

        assert_eq!(payload.get("ok").and_then(Value::as_bool), Some(true));
        assert_eq!(
            payload.get("turnId").and_then(Value::as_str),
            Some("turn-2")
        );

        let thread = read_thread_payload(&state, "default", "thread-1", true)
            .await
            .unwrap();
        let last_turn_steer = thread.get("lastTurnSteer").cloned().unwrap_or(Value::Null);
        assert_eq!(
            last_turn_steer
                .get("expectedTurnId")
                .and_then(Value::as_str),
            Some("turn-2")
        );
        let first_text = last_turn_steer
            .get("input")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|value| value.get("text"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(first_text.contains(ATTACHMENT_PREAMBLE_START));
        assert!(first_text.contains(text_attachment_path.to_str().unwrap()));
        assert!(first_text.contains("Focus on the queue deduplication race first."));

        let runtime_key = runtime_session_key(
            resolve_runtime_profile_entry(&state.config, "default").0,
            "thread-1",
        );
        assert_eq!(
            state.active_turns.lock().await.get(&runtime_key).cloned(),
            Some("turn-2".to_string())
        );

        let draft = get_session_draft_payload(&state, "default", "thread-1")
            .await
            .unwrap();
        assert_eq!(draft.get("draft").and_then(Value::as_str), Some(""));

        let _ = fs::remove_dir_all(sandbox);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fork_session_payload_uses_app_server_fork_and_rollback() {
        let sandbox = unique_test_dir("session-fork-rust");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&codex_home).unwrap();

        let state =
            test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
        app_server_client(&state, "default")
            .await
            .unwrap()
            .request(
                "thread/seed",
                json!({
                    "thread": {
                        "id": "thread-1",
                        "name": "New thread",
                        "preview": "Fix duplicate queue dispatches in websocket transport",
                        "cwd": workspace.display().to_string(),
                        "archived": false,
                        "createdAt": 1,
                        "updatedAt": 2,
                        "status": "idle",
                        "isSubagent": false,
                        "agentNickname": Value::Null,
                        "agentRole": Value::Null,
                        "turns": [
                            {
                                "id": "turn-1",
                                "status": "completed",
                                "error": Value::Null,
                                "startedAt": 10,
                                "completedAt": 20,
                                "durationMs": 10,
                                "items": [
                                    {
                                        "id": "item-1",
                                        "type": "userMessage",
                                        "text": "Fix duplicate queue dispatches in websocket transport"
                                    }
                                ]
                            },
                            {
                                "id": "turn-2",
                                "status": "completed",
                                "error": Value::Null,
                                "startedAt": 30,
                                "completedAt": 40,
                                "durationMs": 10,
                                "items": [
                                    {
                                        "id": "item-2",
                                        "type": "userMessage",
                                        "text": "Also capture the race with a regression test"
                                    }
                                ]
                            }
                        ]
                    }
                }),
            )
            .await
            .unwrap();

        save_session_preferences_payload(
            &state,
            "default",
            "thread-1",
            json!({
                "cwd": workspace.display().to_string(),
                "model": "gpt-5",
                "approvalPolicy": "on-request",
                "sandboxMode": "workspace-write",
                "speed": "fast",
                "effort": "high",
                "networkAccess": true
            }),
        )
        .await
        .unwrap();

        let payload =
            fork_session_payload(&state, "default", "thread-1", "fork", Some("turn-1"), None)
                .await
                .unwrap();

        assert_eq!(payload.get("mode").and_then(Value::as_str), Some("fork"));
        assert_eq!(
            payload
                .get("session")
                .and_then(|value| value.get("id"))
                .and_then(Value::as_str),
            Some("fork-1")
        );
        assert_eq!(
            payload
                .get("session")
                .and_then(|value| value.get("name"))
                .and_then(Value::as_str),
            Some("Fix duplicate queue dispatches in websocket transport")
        );

        let forked_thread = read_thread_payload(&state, "default", "fork-1", true)
            .await
            .unwrap();
        assert_eq!(
            forked_thread.get("forkedFrom").and_then(Value::as_str),
            Some("thread-1")
        );
        assert_eq!(
            forked_thread.get("rollbackCount").and_then(Value::as_u64),
            Some(1)
        );
        assert_eq!(
            forked_thread
                .get("turns")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
        );

        let stored_preferences = with_ui_state_read(&state, "default", |ui_state| {
            Ok(ui_state
                .get("preferencesByThreadId")
                .and_then(Value::as_object)
                .and_then(|entries| entries.get("fork-1"))
                .cloned()
                .unwrap_or(Value::Null))
        })
        .await
        .unwrap();
        assert_eq!(
            stored_preferences.get("model").and_then(Value::as_str),
            Some("gpt-5")
        );

        let _ = fs::remove_dir_all(sandbox);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn delete_attachment_payload_removes_attachment_files() {
        let sandbox = unique_test_dir("attachment-delete-rust");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&codex_home).unwrap();

        let state =
            test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
        let runtime_profile = resolve_runtime_profile(&state.config, "default");
        let uploads_dir = runtime_profile.data_dir.join("uploads").join("thread-1");
        fs::create_dir_all(&uploads_dir).unwrap();
        let stored_file = uploads_dir.join("att-1-notes.md");
        let stored_meta = uploads_dir.join("att-1-notes.md.json");
        fs::write(&stored_file, "notes").unwrap();
        fs::write(
            &stored_meta,
            serde_json::to_vec(&json!({
                "id": "att-1",
                "originalName": "notes.md",
                "path": stored_file.display().to_string(),
                "mimeType": "text/markdown",
                "size": 5,
                "kind": "file",
                "createdAt": "2026-04-20T00:00:00Z"
            }))
            .unwrap(),
        )
        .unwrap();

        let payload = delete_attachment_payload(&state, "default", "thread-1", "att-1")
            .await
            .unwrap();
        assert_eq!(payload.get("ok").and_then(Value::as_bool), Some(true));
        assert!(!stored_file.exists());
        assert!(!stored_meta.exists());
        assert!(
            list_session_attachment_records(&state, "default", "thread-1")
                .await
                .unwrap()
                .is_empty()
        );

        let _ = fs::remove_dir_all(sandbox);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn upload_attachments_store_files_without_internal_backend() {
        let sandbox = unique_test_dir("attachment-upload-rust");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&codex_home).unwrap();

        let state =
            test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);

        let payload = upload_attachments(
            &state,
            "default",
            "thread-1",
            vec![
                UploadFilePayload {
                    name: "notes.md".to_string(),
                    mime_type: Some("text/markdown".to_string()),
                    data_base64: base64::engine::general_purpose::STANDARD.encode(b"notes"),
                },
                UploadFilePayload {
                    name: "diagram.png".to_string(),
                    mime_type: Some("image/png".to_string()),
                    data_base64: base64::engine::general_purpose::STANDARD.encode(b"pngdata"),
                },
            ],
        )
        .await
        .unwrap();

        let returned = payload
            .get("attachments")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(returned.len(), 2);

        let stored = list_session_attachment_records(&state, "default", "thread-1")
            .await
            .unwrap();
        assert_eq!(stored.len(), 2);
        assert!(stored.iter().any(|attachment| {
            attachment.original_name == "notes.md"
                && attachment.kind.as_deref() == Some("file")
                && attachment.size == Some(5)
        }));
        assert!(stored.iter().any(|attachment| {
            attachment.original_name == "diagram.png"
                && attachment.kind.as_deref() == Some("image")
                && attachment.size == Some(7)
        }));

        let _ = fs::remove_dir_all(sandbox);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn session_attachments_http_handlers_use_rust_storage() {
        let sandbox = unique_test_dir("attachment-http-rust");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&codex_home).unwrap();

        let state =
            test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
        let boundary = "codex-webui-boundary";
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"notes.md\"\r\nContent-Type: text/markdown\r\n\r\nnotes\r\n--{boundary}--\r\n"
        );
        let request = Request::builder()
            .method(Method::POST)
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(body))
            .unwrap();

        let response = handle_session_attachments_api_http(
            state.clone(),
            request,
            AuthContext {
                role: UserRole::Admin,
                profile_id: "default".to_string(),
            },
            "thread-1",
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let response_body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let payload: Value = serde_json::from_slice(&response_body).unwrap();
        assert_eq!(
            payload
                .get("attachments")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
        );

        let get_request = Request::builder()
            .method(Method::GET)
            .body(Body::empty())
            .unwrap();
        let get_response = handle_session_attachments_api_http(
            state.clone(),
            get_request,
            AuthContext {
                role: UserRole::Admin,
                profile_id: "default".to_string(),
            },
            "thread-1",
        )
        .await;
        assert_eq!(get_response.status(), StatusCode::OK);
        let get_body = to_bytes(get_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let get_payload: Value = serde_json::from_slice(&get_body).unwrap();
        assert_eq!(
            get_payload
                .get("attachments")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
        );

        let _ = fs::remove_dir_all(sandbox);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn session_recovery_http_handler_recovers_rollout_file() {
        let sandbox = unique_test_dir("session-recovery-http");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&codex_home).unwrap();

        let state = test_state_with_fake_app_server(
            workspace.clone(),
            vec![workspace.clone()],
            codex_home.clone(),
        );
        let created_at = time::OffsetDateTime::now_utc().unix_timestamp();
        let created_date = time::OffsetDateTime::from_unix_timestamp(created_at)
            .unwrap()
            .date();
        let rollout_dir = codex_home
            .join("sessions")
            .join(created_date.year().to_string())
            .join(format!("{:02}", u8::from(created_date.month())))
            .join(format!("{:02}", created_date.day()));
        fs::create_dir_all(&rollout_dir).unwrap();
        let rollout_path = rollout_dir.join("2026-04-21-thread-1.jsonl");
        fs::write(&rollout_path, b"{\"step\":1}\n\xff\n{\"step\":2}\n").unwrap();

        app_server_client(&state, "default")
            .await
            .unwrap()
            .request(
                "thread/seed",
                json!({
                    "thread": {
                        "id": "thread-1",
                        "name": "Recover rollout",
                        "preview": "Recover rollout",
                        "cwd": workspace.display().to_string(),
                        "archived": false,
                        "createdAt": created_at,
                        "updatedAt": created_at,
                        "status": "idle",
                        "isSubagent": false,
                        "agentNickname": Value::Null,
                        "agentRole": Value::Null,
                        "turns": []
                    }
                }),
            )
            .await
            .unwrap();

        let request = Request::builder()
            .method(Method::POST)
            .body(Body::empty())
            .unwrap();
        let response = handle_session_recovery_api_http(
            state.clone(),
            request,
            AuthContext {
                role: UserRole::Admin,
                profile_id: "default".to_string(),
            },
            "thread-1",
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let payload: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload.get("ok").and_then(Value::as_bool), Some(true));
        assert_eq!(
            payload.get("recoveredLines").and_then(Value::as_u64),
            Some(2)
        );
        assert_eq!(payload.get("skippedLines").and_then(Value::as_u64), Some(1));
        assert_eq!(
            fs::read_to_string(&rollout_path).unwrap(),
            "{\"step\":1}\n{\"step\":2}\n"
        );
        assert!(
            Path::new(
                payload
                    .get("backupPath")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            )
            .exists()
        );

        let _ = fs::remove_dir_all(sandbox);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn get_config_payload_uses_rust_state_and_app_server_metadata() {
        let sandbox = unique_test_dir("config-rust");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&codex_home).unwrap();
        fs::write(
            config_toml_path(&codex_home),
            "model = \"gpt-5.4\"\nservice_tier = \"fast\"\napproval_policy = \"on-request\"\nsandbox_mode = \"workspace-write\"\n[sandbox_workspace_write]\nnetwork_access = true\n",
        )
        .unwrap();

        let state =
            test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);

        let payload = get_config_payload(&state, "default").await.unwrap();
        assert_eq!(
            payload
                .get("models")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(
            payload
                .get("collaborationModes")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(2)
        );
        assert_eq!(
            payload
                .get("defaults")
                .and_then(|value| value.get("model"))
                .and_then(Value::as_str),
            Some("gpt-5.4")
        );
        assert_eq!(
            payload
                .get("defaults")
                .and_then(|value| value.get("speed"))
                .and_then(Value::as_str),
            Some("fast")
        );
        assert_eq!(
            payload
                .get("git")
                .and_then(|value| value.get("discoveryDepth"))
                .and_then(Value::as_u64),
            Some(1)
        );
        assert_eq!(
            payload
                .get("profiles")
                .and_then(Value::as_array)
                .and_then(|profiles| profiles.first())
                .and_then(|profile| profile.get("label"))
                .and_then(Value::as_str),
            Some("Default")
        );
        assert_eq!(
            payload
                .get("account")
                .and_then(|value| value.get("email"))
                .and_then(Value::as_str),
            Some("demo@example.com")
        );

        let _ = fs::remove_dir_all(sandbox);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resolve_server_request_payload_uses_pending_request_store() {
        let sandbox = unique_test_dir("approval-rust");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&codex_home).unwrap();

        let state =
            test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
        handle_profile_server_request(
            &state,
            "default",
            &backend::codex_app_server::AppServerRequest {
                id: json!("srv-1"),
                method: "input/request".to_string(),
                params: json!({
                    "threadId": "thread-1",
                    "question": "Continue?"
                }),
            },
        )
        .await;

        let pending_before = state
            .pending_server_requests
            .lock()
            .await
            .get(&runtime_session_key("default", "thread-1"))
            .and_then(|entries| entries.get("srv-1"))
            .cloned();
        assert!(pending_before.is_some());

        let highlighted = with_ui_state_read(&state, "default", |ui_state| {
            Ok(ui_state
                .get("highlightsByThreadId")
                .and_then(Value::as_object)
                .and_then(|entries| entries.get("thread-1"))
                .cloned()
                .unwrap_or(Value::Null))
        })
        .await
        .unwrap();
        assert_eq!(
            highlighted.get("kind").and_then(Value::as_str),
            Some("attention")
        );

        let payload = resolve_server_request_payload(
            &state,
            "default",
            "thread-1",
            "srv-1",
            json!({ "answer": "yes" }),
        )
        .await
        .unwrap();
        assert_eq!(payload.get("ok").and_then(Value::as_bool), Some(true));

        let pending_after = state
            .pending_server_requests
            .lock()
            .await
            .get(&runtime_session_key("default", "thread-1"))
            .cloned();
        assert!(pending_after.is_none());

        let highlight_after = with_ui_state_read(&state, "default", |ui_state| {
            Ok(ui_state
                .get("highlightsByThreadId")
                .and_then(Value::as_object)
                .and_then(|entries| entries.get("thread-1"))
                .cloned()
                .unwrap_or(Value::Null))
        })
        .await
        .unwrap();
        assert!(highlight_after.is_null());

        let _ = fs::remove_dir_all(sandbox);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resolve_server_request_payload_returns_not_found_without_pending_request() {
        let sandbox = unique_test_dir("approval-missing");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&codex_home).unwrap();

        let state =
            test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);

        let error = resolve_server_request_payload(
            &state,
            "default",
            "thread-1",
            "missing-request",
            json!({ "answer": "yes" }),
        )
        .await
        .expect_err("missing request should fail");

        assert_eq!(error.status, StatusCode::NOT_FOUND);
        assert_eq!(error.message, "SERVER_REQUEST_NOT_FOUND");

        let _ = fs::remove_dir_all(sandbox);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn runtime_notifications_emit_session_stream_events_from_rust_relay() {
        let sandbox = unique_test_dir("session-stream-rust");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&codex_home).unwrap();

        let state =
            test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
        let relay = ensure_stream_relay(&state, "default", "thread-1")
            .await
            .expect("relay should initialize");
        let mut receiver = relay.subscribe();

        handle_profile_runtime_notification(
            &state,
            "default",
            &AppServerNotification {
                method: "item/started".to_string(),
                params: json!({
                    "threadId": "thread-1",
                    "turnId": "turn-1",
                    "itemId": "item-1",
                    "item": {
                        "id": "item-1",
                        "type": "commandExecution",
                        "command": ["sed", "-n", "1,20p", "src/main.rs"]
                    }
                }),
            },
        )
        .await;

        let item_started = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("item event should arrive")
            .expect("item event should be readable");
        assert_eq!(
            item_started.get("method").and_then(Value::as_str),
            Some("item/started")
        );
        assert_eq!(
            item_started
                .get("params")
                .and_then(|value| value.get("item"))
                .and_then(|value| value.get("title"))
                .and_then(Value::as_str),
            Some("Command")
        );

        handle_profile_runtime_notification(
            &state,
            "default",
            &AppServerNotification {
                method: "item/commandExecution/outputDelta".to_string(),
                params: json!({
                    "threadId": "thread-1",
                    "turnId": "turn-1",
                    "itemId": "item-1",
                    "delta": "hello"
                }),
            },
        )
        .await;

        let command_delta = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("command delta should arrive")
            .expect("command delta should be readable");
        assert_eq!(
            command_delta
                .get("params")
                .and_then(|value| value.get("deltaLength"))
                .and_then(Value::as_u64),
            Some(5)
        );

        handle_profile_runtime_notification(
            &state,
            "default",
            &AppServerNotification {
                method: "thread/status/changed".to_string(),
                params: json!({
                    "threadId": "thread-1",
                    "status": { "type": "completed" }
                }),
            },
        )
        .await;

        let status_changed = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("status event should arrive")
            .expect("status event should be readable");
        assert_eq!(
            status_changed
                .get("params")
                .and_then(|value| value.get("status"))
                .and_then(Value::as_str),
            Some("completed")
        );

        handle_profile_runtime_notification(
            &state,
            "default",
            &AppServerNotification {
                method: "thread/tokenUsage/updated".to_string(),
                params: json!({
                    "threadId": "thread-1",
                    "tokenUsage": {
                        "total": {
                            "totalTokens": 15,
                            "inputTokens": 7,
                            "cachedInputTokens": 1,
                            "outputTokens": 8,
                            "reasoningOutputTokens": 2
                        },
                        "last": {
                            "totalTokens": 10,
                            "inputTokens": 4,
                            "cachedInputTokens": 1,
                            "outputTokens": 6,
                            "reasoningOutputTokens": 1
                        },
                        "modelContextWindow": 2000
                    }
                }),
            },
        )
        .await;

        let token_usage = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("token usage event should arrive")
            .expect("token usage event should be readable");
        assert_eq!(
            token_usage
                .get("params")
                .and_then(|value| value.get("tokenUsage"))
                .and_then(|value| value.get("total"))
                .and_then(|value| value.get("totalTokens"))
                .and_then(Value::as_u64),
            Some(15)
        );

        let _ = fs::remove_dir_all(sandbox);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn runtime_notifications_emit_global_events_without_internal_sse() {
        let sandbox = unique_test_dir("global-stream-rust");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&codex_home).unwrap();

        let state =
            test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
        let relay = ensure_global_relay(&state, "default")
            .await
            .expect("global relay should initialize");
        let mut receiver = relay.subscribe();

        handle_profile_runtime_notification(
            &state,
            "default",
            &AppServerNotification {
                method: "turn/completed".to_string(),
                params: json!({
                    "threadId": "thread-1",
                    "turnId": "turn-1",
                    "turn": {
                        "id": "turn-1",
                        "status": "completed",
                        "items": []
                    }
                }),
            },
        )
        .await;

        let mut saw_completion_attention = false;
        for _ in 0..6 {
            let event = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
                .await
                .expect("completion event should arrive")
                .expect("completion event should be readable");
            if event.get("method").and_then(Value::as_str) == Some("codex-webui/sessionAttention")
                && event
                    .get("params")
                    .and_then(|value| value.get("reason"))
                    .and_then(Value::as_str)
                    == Some("completed")
            {
                saw_completion_attention = true;
                break;
            }
        }
        assert!(saw_completion_attention);

        handle_profile_runtime_notification(
            &state,
            "default",
            &AppServerNotification {
                method: "thread/archived".to_string(),
                params: json!({
                    "threadId": "thread-1"
                }),
            },
        )
        .await;

        let mut saw_invalidation = false;
        for _ in 0..6 {
            let event = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
                .await
                .expect("invalidation event should arrive")
                .expect("invalidation event should be readable");
            if event.get("method").and_then(Value::as_str)
                == Some("codex-webui/sessionListsInvalidated")
            {
                saw_invalidation = true;
                break;
            }
        }
        assert!(saw_invalidation);

        let _ = fs::remove_dir_all(sandbox);
    }

    #[tokio::test]
    async fn arena_list_falls_back_to_stored_runs_when_sessions_cannot_be_loaded() {
        let sandbox = unique_test_dir("arena-list");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&codex_home).unwrap();

        let state = test_state(workspace.clone(), vec![workspace], codex_home);
        let arena_path = arena_store_path(&state.config, "default");
        fs::create_dir_all(arena_path.parent().unwrap()).unwrap();
        fs::write(
            &arena_path,
            serde_json::to_vec_pretty(&ArenaStoreState {
                runs: vec![ArenaRunRecord {
                    id: "arena-1".to_string(),
                    prompt: "compare models".to_string(),
                    cwd: "/tmp/project".to_string(),
                    status: "running".to_string(),
                    created_at: 100,
                    updated_at: 110,
                    contestants: vec![ArenaContestantRecord {
                        id: "contestant-1".to_string(),
                        session_id: "session-1".to_string(),
                        model: "gpt-5.4".to_string(),
                        label: "Primary".to_string(),
                        status: "running".to_string(),
                        response: None,
                        created_at: 100,
                        updated_at: 110,
                    }],
                }],
            })
            .unwrap(),
        )
        .unwrap();

        let payload = list_arena_runs_payload(&state, "default").await.unwrap();
        let first_run = payload
            .get("runs")
            .and_then(Value::as_array)
            .and_then(|runs| runs.first())
            .cloned()
            .unwrap();
        assert_eq!(first_run.get("id").and_then(Value::as_str), Some("arena-1"));
        assert_eq!(
            first_run.get("status").and_then(Value::as_str),
            Some("running")
        );
        assert_eq!(
            first_run
                .get("contestants")
                .and_then(Value::as_array)
                .and_then(|contestants| contestants.first())
                .and_then(|contestant| contestant.get("sessionId"))
                .and_then(Value::as_str),
            Some("session-1")
        );

        let _ = fs::remove_dir_all(sandbox);
    }

    #[tokio::test]
    async fn saves_drafts_and_reads_queue_payloads() {
        let sandbox = unique_test_dir("draft-queue");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&codex_home).unwrap();

        let state = test_state(workspace.clone(), vec![workspace], codex_home);
        let ui_state_path = profile_ui_state_path(&state.config, "default");
        fs::create_dir_all(ui_state_path.parent().unwrap()).unwrap();
        fs::write(
            &ui_state_path,
            serde_json::to_vec_pretty(&json!({
                "global": {
                    "shutdownAfterQueueCompletes": false,
                    "scheduledShutdown": Value::Null
                },
                "notifications": {
                    "items": [],
                    "settings": default_notification_settings_value()
                },
                "sessionMetaByThreadId": {},
                "savedSessionFilters": [],
                "promptPresets": [],
                "automations": [],
                "automationRuns": [],
                "preferencesByThreadId": {},
                "draftsByThreadId": {},
                "queuesByThreadId": {
                    "thread-1": {
                        "items": [
                            {
                                "id": "queue-1",
                                "prompt": "follow up",
                                "attachmentIds": ["att-1"],
                                "attachmentNames": ["notes.txt"],
                                "createdAt": 15
                            }
                        ],
                        "resumePending": true,
                        "updatedAt": 20
                    }
                },
                "highlightsByThreadId": {}
            }))
            .unwrap(),
        )
        .unwrap();

        let saved =
            save_session_draft_payload(&state, "default", "thread-1", "Draft message", "queue")
                .await
                .unwrap();
        assert_eq!(
            saved.get("draft").and_then(Value::as_str),
            Some("Draft message")
        );
        assert_eq!(saved.get("intent").and_then(Value::as_str), Some("queue"));

        let cleared = clear_session_draft_payload(&state, "default", "thread-1")
            .await
            .unwrap();
        assert_eq!(cleared.get("draft").and_then(Value::as_str), Some(""));
        assert!(cleared.get("intent").is_some_and(Value::is_null));

        let queue = get_session_queue_payload(&state, "default", "thread-1")
            .await
            .unwrap();
        assert_eq!(
            queue.get("resumeRequired").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            queue
                .get("items")
                .and_then(Value::as_array)
                .and_then(|items| items.first())
                .and_then(|item| item.get("attachmentNames"))
                .and_then(Value::as_array)
                .and_then(|names| names.first())
                .and_then(Value::as_str),
            Some("notes.txt")
        );

        let _ = fs::remove_dir_all(sandbox);
    }

    #[tokio::test]
    async fn queue_write_helpers_mutate_queue_state() {
        let sandbox = unique_test_dir("queue-write-helpers");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&codex_home).unwrap();

        let state = test_state(workspace.clone(), vec![workspace], codex_home);

        let first = enqueue_session_queue_payload(&state, "default", "thread-1", "first", None)
            .await
            .unwrap();
        let first_id = first
            .get("enqueueItemId")
            .and_then(Value::as_str)
            .unwrap()
            .to_string();
        let second = enqueue_session_queue_payload(&state, "default", "thread-1", "second", None)
            .await
            .unwrap();
        let second_id = second
            .get("enqueueItemId")
            .and_then(Value::as_str)
            .unwrap()
            .to_string();

        let reordered = reorder_session_queue_payload(
            &state,
            "default",
            "thread-1",
            &[second_id.clone(), first_id.clone()],
        )
        .await
        .unwrap();
        assert_eq!(
            reordered
                .get("items")
                .and_then(Value::as_array)
                .and_then(|items| items.first())
                .and_then(|item| item.get("id"))
                .and_then(Value::as_str),
            Some(second_id.as_str())
        );

        let empty_attachments = json!([]);
        let updated = update_session_queue_item_payload(
            &state,
            "default",
            "thread-1",
            &first_id,
            Some("first updated"),
            Some(&empty_attachments),
        )
        .await
        .unwrap();
        let updated_item = updated
            .get("items")
            .and_then(Value::as_array)
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item.get("id").and_then(Value::as_str) == Some(first_id.as_str()))
            })
            .cloned()
            .unwrap();
        assert_eq!(
            updated_item.get("prompt").and_then(Value::as_str),
            Some("first updated")
        );

        let removed = remove_session_queue_item_payload(&state, "default", "thread-1", &second_id)
            .await
            .unwrap();
        assert_eq!(
            removed.get("items").and_then(Value::as_array).map(Vec::len),
            Some(1)
        );

        let _ = fs::remove_dir_all(sandbox);
    }

    #[tokio::test]
    async fn marks_resume_pending_queues_and_lists_paused_entries() {
        let sandbox = unique_test_dir("queue-resume-pending");
        let workspace = sandbox.join("workspace");
        let codex_home = sandbox.join("codex-home");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&codex_home).unwrap();

        let state = test_state(workspace.clone(), vec![workspace], codex_home);
        let ui_state_path = profile_ui_state_path(&state.config, "default");
        fs::create_dir_all(ui_state_path.parent().unwrap()).unwrap();
        fs::write(
            &ui_state_path,
            serde_json::to_vec_pretty(&json!({
                "global": {
                    "shutdownAfterQueueCompletes": false,
                    "scheduledShutdown": Value::Null
                },
                "notifications": {
                    "items": [],
                    "settings": default_notification_settings_value()
                },
                "sessionMetaByThreadId": {},
                "savedSessionFilters": [],
                "promptPresets": [],
                "automations": [],
                "automationRuns": [],
                "preferencesByThreadId": {
                    "thread-1": {
                        "cwd": "/tmp/project"
                    }
                },
                "draftsByThreadId": {},
                "queuesByThreadId": {
                    "thread-1": {
                        "items": [
                            {
                                "id": "queue-1",
                                "prompt": "follow up",
                                "attachmentIds": [],
                                "attachmentNames": [],
                                "createdAt": 15
                            }
                        ],
                        "resumePending": false,
                        "updatedAt": 20
                    }
                },
                "highlightsByThreadId": {}
            }))
            .unwrap(),
        )
        .unwrap();

        assert!(
            mark_queues_pending_resume_payload(&state, "default")
                .await
                .unwrap()
        );
        let paused = list_resume_pending_queues_payload(&state, "default")
            .await
            .unwrap();
        let first = paused
            .as_array()
            .and_then(|items| items.first())
            .cloned()
            .unwrap();
        assert_eq!(
            first.get("sessionId").and_then(Value::as_str),
            Some("thread-1")
        );
        assert_eq!(first.get("pendingCount").and_then(Value::as_u64), Some(1));
        assert_eq!(
            first.get("cwd").and_then(Value::as_str),
            Some("/tmp/project")
        );

        let _ = fs::remove_dir_all(sandbox);
    }

    #[test]
    fn catalog_builder_discovers_plugins_and_skills() {
        let sandbox = unique_test_dir("catalog");
        let codex_home = sandbox.join(".codex");
        let local_skill_dir = codex_home.join("skills").join("my-skill");
        let system_skill_dir = codex_home.join("skills").join(".system").join("sys-skill");
        let plugin_base = codex_home.join("plugins").join("sample-plugin");
        let plugin_skill_dir = plugin_base.join("skills").join("plugin-skill");

        fs::create_dir_all(&local_skill_dir).unwrap();
        fs::create_dir_all(&system_skill_dir).unwrap();
        fs::create_dir_all(plugin_base.join(".codex-plugin")).unwrap();
        fs::create_dir_all(&plugin_skill_dir).unwrap();

        fs::write(
            local_skill_dir.join("SKILL.md"),
            "---\nname: Local Skill\ndescription: Local description\n---\nbody\n",
        )
        .unwrap();
        fs::write(
            system_skill_dir.join("SKILL.md"),
            "---\nname: System Skill\ndescription: System description\n---\nbody\n",
        )
        .unwrap();
        fs::write(
            plugin_skill_dir.join("SKILL.md"),
            "---\nname: Plugin Skill\ndescription: Plugin description\n---\nbody\n",
        )
        .unwrap();
        fs::write(
            plugin_base.join(".codex-plugin").join("plugin.json"),
            serde_json::to_vec_pretty(&json!({
                "name": "sample-plugin",
                "description": "Plugin description",
                "version": "1.2.3",
                "skills": "skills",
                "interface": {
                    "displayName": "Sample Plugin",
                    "developerName": "Codex Web UI",
                    "category": "tools"
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let payload = build_catalog_payload_for_codex_home(&codex_home);
        let skills = payload
            .get("skills")
            .and_then(Value::as_array)
            .cloned()
            .unwrap();
        let plugins = payload
            .get("plugins")
            .and_then(Value::as_array)
            .cloned()
            .unwrap();

        assert!(skills.iter().any(|entry| {
            entry.get("name").and_then(Value::as_str) == Some("Local Skill")
                && entry.get("source").and_then(Value::as_str) == Some("local")
        }));
        assert!(skills.iter().any(|entry| {
            entry.get("name").and_then(Value::as_str) == Some("System Skill")
                && entry.get("source").and_then(Value::as_str) == Some("system")
        }));
        assert!(skills.iter().any(|entry| {
            entry.get("name").and_then(Value::as_str) == Some("Plugin Skill")
                && entry.get("pluginName").and_then(Value::as_str) == Some("sample-plugin")
        }));
        assert!(plugins.iter().any(|entry| {
            entry.get("displayName").and_then(Value::as_str) == Some("Sample Plugin")
                && entry
                    .get("skills")
                    .and_then(Value::as_array)
                    .is_some_and(|skills| skills.contains(&json!("plugin-skill")))
        }));

        let _ = fs::remove_dir_all(sandbox);
    }
}
