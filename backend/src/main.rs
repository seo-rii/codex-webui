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

mod app_util_support;
mod arena_support;
mod attachment_support;
mod auth_support;
mod automation_support;
mod aux_http_support;
mod catalog_support;
mod codex_runtime_support;
mod config_http_support;
mod config_support;
mod event_mapping_support;
mod git_support;
mod github_support;
mod http_router_support;
mod queue_runtime_support;
mod relay_support;
mod request_cache_support;
mod rollout_recovery_support;
mod runtime_env_support;
mod runtime_notification_support;
mod session_http_support;
mod session_route_dispatch_support;
mod session_state_support;
mod session_summary_support;
mod static_support;
mod system_support;
mod terminal_support;
mod thread_support;
mod transport_http_support;
mod turn_support;
mod ui_state_support;
mod workspace_support;
mod ws_dispatch_support;

use app_util_support::*;
use arena_support::*;
use attachment_support::*;
use auth_support::*;
use automation_support::*;
use aux_http_support::*;
use catalog_support::*;
use codex_runtime_support::*;
use config_http_support::*;
use config_support::*;
use event_mapping_support::*;
use git_support::*;
use github_support::*;
use http_router_support::*;
use queue_runtime_support::*;
use relay_support::*;
use request_cache_support::*;
use rollout_recovery_support::*;
use runtime_env_support::*;
use runtime_notification_support::*;
use session_http_support::*;
use session_route_dispatch_support::*;
use session_state_support::*;
use session_summary_support::*;
use static_support::*;
use system_support::*;
use terminal_support::*;
use thread_support::*;
use transport_http_support::*;
use turn_support::*;
use ui_state_support::*;
use workspace_support::*;
use ws_dispatch_support::*;

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

fn selected_skills_from_value(value: Option<&Value>) -> Vec<Value> {
    let Some(entries) = value.and_then(Value::as_array) else {
        return Vec::new();
    };

    let mut seen = HashSet::new();
    entries
        .iter()
        .filter_map(|entry| {
            let object = entry.as_object()?;
            let name = object.get("name").and_then(Value::as_str)?.trim();
            let path = object.get("path").and_then(Value::as_str)?.trim();
            if name.is_empty() || path.is_empty() {
                return None;
            }
            let id = object
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(path);
            let key = format!("{name}\u{0}{path}");
            if !seen.insert(key) {
                return None;
            }
            Some(json!({
                "id": id,
                "name": name,
                "path": path
            }))
        })
        .collect()
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

            if route_path == "/api/sessions" || route_path.starts_with("/api/sessions/") {
                return handle_session_route_http(
                    state,
                    &jar,
                    request,
                    &route_path,
                    cors_origin.as_deref(),
                    requested_headers.as_deref(),
                )
                .await;
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
            None,
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
            None,
            Some("Build Docs"),
        )
        .await
        .unwrap();
        let second = create_session_payload(
            &state,
            "default",
            json!({ "cwd": workspace.display().to_string() }),
            None,
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
        let selected_skills = json!([
            {
                "id": "skill-1",
                "name": "imagegen",
                "path": "/skills/imagegen/SKILL.md"
            }
        ]);
        let payload = send_turn_payload(
            &state,
            "default",
            "thread-1",
            prompt,
            Some(&json!(["att-file", "att-image"])),
            Some(&selected_skills),
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
        assert_eq!(input.len(), 3);
        let first_text = input
            .first()
            .and_then(|value| value.get("text"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(first_text.contains("$imagegen"));
        assert!(first_text.contains(ATTACHMENT_PREAMBLE_START));
        assert!(first_text.contains(ATTACHMENT_PREAMBLE_END));
        assert!(first_text.contains(text_attachment_path.to_str().unwrap()));
        assert!(first_text.contains(prompt.trim()));
        assert_eq!(
            input
                .get(2)
                .and_then(|value| value.get("path"))
                .and_then(Value::as_str),
            Some(image_attachment_path.to_str().unwrap())
        );
        assert_eq!(
            input
                .get(1)
                .and_then(|value| value.get("type"))
                .and_then(Value::as_str),
            Some("skill")
        );
        assert_eq!(
            input
                .get(1)
                .and_then(|value| value.get("name"))
                .and_then(Value::as_str),
            Some("imagegen")
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
            None,
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

        let queue_skills = json!([
            {
                "id": "skill-queue",
                "name": "openai-docs",
                "path": "/skills/openai-docs/SKILL.md"
            }
        ]);
        let first = enqueue_session_queue_payload(
            &state,
            "default",
            "thread-1",
            "first",
            Some(&queue_skills),
            None,
        )
        .await
        .unwrap();
        let first_id = first
            .get("enqueueItemId")
            .and_then(Value::as_str)
            .unwrap()
            .to_string();
        let second =
            enqueue_session_queue_payload(&state, "default", "thread-1", "second", None, None)
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
            Some(&queue_skills),
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
        assert_eq!(
            updated_item
                .get("skills")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
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
