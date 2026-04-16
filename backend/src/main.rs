use std::{
    collections::HashMap,
    env,
    fs,
    net::{SocketAddr, TcpListener},
    path::PathBuf,
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context, Result};
use axum::{
    body::{to_bytes, Body},
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Request, State,
    },
    http::{
        header::{self, HeaderValue},
        HeaderMap, Method, StatusCode, Uri,
    },
    response::{IntoResponse, Redirect, Response},
    routing::{any, get},
    Json, Router,
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use futures_util::{SinkExt, StreamExt, TryStreamExt};
use hmac::{Hmac, Mac};
use reqwest::multipart::{Form, Part};
use scrypt::{scrypt, Params as ScryptParams};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
    sync::{broadcast, mpsc, Mutex},
};
use tokio_util::io::StreamReader;
use time::Duration as CookieDuration;
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

const AUTH_COOKIE: &str = "codex_webui_auth";
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

#[derive(Clone, Debug)]
struct Config {
    project_root: PathBuf,
    codex_home: PathBuf,
    base_path: String,
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
    session_secret: Option<String>,
    cookie_same_site: SameSiteMode,
    cookie_secure_mode: CookieSecureMode,
    cors_allowed_origins: Vec<String>,
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

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    http: reqwest::Client,
    login_attempts: Arc<Mutex<HashMap<String, Vec<u128>>>>,
    response_cache: Arc<Mutex<HashMap<String, CachedResponse>>>,
    quota_cache: Arc<Mutex<Option<CachedQuota>>>,
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
    Request { id: String, method: String, params: Value },
    Ping { nonce: Option<String> },
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
    GlobalEvent { event: Value },
    Pong { nonce: Option<String> },
}

#[derive(Debug, Deserialize)]
struct LoginPayload {
    password: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UploadFilePayload {
    name: String,
    mime_type: Option<String>,
    data_base64: String,
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
        writer.flush().await.context("failed to flush terminal input")?;
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
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = Arc::new(Config::from_env()?);
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("failed to build reqwest client")?;

    let mut child = spawn_internal_node(&config).await?;
    wait_for_internal_node(&config, &http).await?;

    let state = AppState {
        config: config.clone(),
        http,
        login_attempts: Arc::new(Mutex::new(HashMap::new())),
        response_cache: Arc::new(Mutex::new(HashMap::new())),
        quota_cache: Arc::new(Mutex::new(None)),
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
    let result = axum::serve(listener, router).await;

    let _ = child.kill().await;
    result.context("axum server terminated unexpectedly")
}

impl Config {
    fn from_env() -> Result<Self> {
        let cwd = env::current_dir().context("failed to read current directory")?;
        load_dotenv(&cwd);
        let project_root = resolve_project_root(&cwd);
        let base_path = normalize_base_path(env::var("CODEX_WEBUI_BASE_PATH").ok());
        let public_host = env::var("HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
        let public_port = parse_port(env::var("PORT").ok(), 4173)?;
        let internal_port = parse_port(env::var("CODEX_WEBUI_INTERNAL_PORT").ok(), choose_free_port()?)?;
        let internal_proxy_token = Uuid::new_v4().to_string();
        let node_entry = project_root.join("build/index.js");
        if !node_entry.exists() {
            return Err(anyhow!(
                "missing internal SvelteKit build at {}. Run `pnpm build` in codex-webui first.",
                node_entry.display()
            ));
        }

        Ok(Self {
            project_root,
            codex_home: resolve_codex_home()?,
            base_path,
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
            session_secret: env::var("CODEX_WEBUI_SESSION_SECRET").ok(),
            cookie_same_site: parse_same_site(env::var("CODEX_WEBUI_COOKIE_SAMESITE").ok().as_deref()),
            cookie_secure_mode: parse_secure_mode(env::var("CODEX_WEBUI_COOKIE_SECURE").ok().as_deref()),
            cors_allowed_origins: parse_cors_origins(env::var("CODEX_WEBUI_CORS_ALLOWED_ORIGINS").ok())?,
        })
    }
}

async fn handle_http(
    State(state): State<AppState>,
    jar: CookieJar,
    request: Request,
) -> Response {
    let method = request.method().clone();
    let uri = request.uri().clone();
    let headers = request.headers().clone();
    let path = uri.path().to_string();

    match normalize_request_path(&state.config.base_path, &path) {
        NormalizedPath::Redirect(target) => return Redirect::temporary(&target).into_response(),
        NormalizedPath::OutsideBase => return (StatusCode::NOT_FOUND, "Not found").into_response(),
        NormalizedPath::Route(route_path) => {
            if route_path.starts_with("/api/auth/") {
                return handle_auth_http(state, jar, method, route_path, headers, request)
                    .await
                    .into_response();
            }

            if route_path.starts_with("/api/") {
                return (StatusCode::NOT_FOUND, "This backend only exposes auth over HTTP.").into_response();
            }

            return proxy_to_internal(state, method, uri, headers, request).await;
        }
    }
}

async fn handle_ws(
    State(state): State<AppState>,
    jar: CookieJar,
    ws: WebSocketUpgrade,
) -> Response {
    if !is_authenticated(&state.config, &jar) {
        return (StatusCode::UNAUTHORIZED, "Authentication required.").into_response();
    }

    ws.on_upgrade(move |socket| websocket_session(socket, state))
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
        (Method::GET, "/api/auth/session") => {
            let authenticated = is_authenticated(&state.config, &jar);
            Ok((jar, Json(json!({ "authenticated": authenticated }))).into_response())
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
    let payload: LoginPayload = serde_json::from_slice(&body).unwrap_or(LoginPayload { password: None });
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

    let verified = verify_password(&state.config, &password).map_err(|error| error.to_string())?;
    if !verified {
        record_login_failure(&state, &identifier).await;
        return Ok(json_error(StatusCode::UNAUTHORIZED, "Invalid password."));
    }

    clear_login_failures(&state, &identifier).await;
    let next_jar = issue_auth_cookie(&state.config, jar, secure_request)
        .map_err(|error| error.to_string())?;
    Ok((next_jar, Json(json!({ "ok": true }))).into_response())
}

fn auth_logout(jar: CookieJar) -> Response {
    let mut cookie = Cookie::new(AUTH_COOKIE, "");
    cookie.set_path("/");
    cookie.set_max_age(CookieDuration::seconds(0));
    (jar.remove(cookie), Json(json!({ "ok": true }))).into_response()
}

async fn proxy_to_internal(
    state: AppState,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    request: Request,
) -> Response {
    let target = format!(
        "{}{}",
        state.config.internal_base_url,
        uri.path_and_query()
            .map(|value| value.as_str())
            .unwrap_or(uri.path())
    );

    let body = match to_bytes(request.into_body(), usize::MAX).await {
        Ok(bytes) => bytes,
        Err(_) => return (StatusCode::BAD_REQUEST, "Failed to read request body.").into_response(),
    };

    match forward_request(
        &state,
        method,
        &target,
        headers,
        body.to_vec(),
        None,
        None,
    )
    .await
    {
        Ok(response) => response,
        Err(error) => {
            error!("proxy error: {error:#}");
            json_error(StatusCode::BAD_GATEWAY, "Failed to proxy frontend request.")
        }
    }
}

async fn websocket_session(socket: WebSocket, state: AppState) {
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
                tokio::spawn(async move {
                    if let Err(error) =
                        handle_ws_message(&state, &out_tx, &subscriptions, payload).await
                    {
                        error!("websocket request failed: {error:#}");
                    }
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
    payload: ClientEnvelope,
) -> Result<()> {
    match payload {
        ClientEnvelope::Ping { nonce } => {
            let _ = out_tx.send(ServerEnvelope::Pong { nonce });
        }
        ClientEnvelope::Request { id, method, params } => {
            if let Some(cached) = cached_response(state, &id).await {
                let _ = out_tx.send(cached);
                return Ok(());
            }

            let message = match execute_ws_method(state, out_tx, subscriptions, &method, params).await {
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

            cache_response(state, &id, message.clone()).await;
            let _ = out_tx.send(message);
        }
    }

    Ok(())
}

async fn execute_ws_method(
    state: &AppState,
    out_tx: &mpsc::UnboundedSender<ServerEnvelope>,
    subscriptions: &Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    method: &str,
    params: Value,
) -> Result<Value> {
    match method {
        "config/get" => internal_json_request(state, Method::GET, "/api/config", None).await,
        "config/update" => {
            let payload = json!({
                "systemShutdown": params.get("systemShutdown").cloned().unwrap_or_else(|| json!({}))
            });
            internal_json_request(state, Method::PATCH, "/api/config", Some(payload)).await
        }
        "runtime/status" => codex_runtime_status(state, false).await,
        "runtime/checkUpdate" => codex_runtime_status(state, true).await,
        "runtime/quota" => codex_quota_status(
            state,
            params.get("refresh").and_then(Value::as_bool).unwrap_or(false),
        )
        .await,
        "catalog/get" => internal_json_request(state, Method::GET, "/api/catalog", None).await,
        "editor/file/get" => {
            let file_path_raw = require_string(&params, "filePath")?;
            let file_path = urlencoding::encode(&file_path_raw);
            internal_json_request(
                state,
                Method::GET,
                &format!("/api/editor?filePath={file_path}"),
                None,
            )
            .await
        }
        "editor/file/save" => {
            let payload = json!({
                "filePath": require_string(&params, "filePath")?,
                "content": params.get("content").cloned().unwrap_or_else(|| Value::String(String::new()))
            });
            internal_json_request(state, Method::PUT, "/api/editor", Some(payload)).await
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
            let limit = params
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(20);
            let mut path = format!("/api/sessions?archived={archived}&limit={limit}");
            if let Some(cursor) = cursor {
                path.push_str(&format!("&cursor={cursor}"));
            }
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
            let limit = params
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(20);
            let mut path = format!("/api/sessions?query={query}&scope={scope}&archived={archived}&limit={limit}");
            if let Some(cursor) = cursor {
                path.push_str(&format!("&cursor={cursor}"));
            }
            internal_json_request(
                state,
                Method::GET,
                &path,
                None,
            )
            .await
        }
        "session/create" => {
            let payload = json!({
                "preferences": params.get("preferences").cloned().unwrap_or_else(|| json!({})),
                "name": params.get("name").cloned().unwrap_or(Value::Null),
            });
            internal_json_request(state, Method::POST, "/api/sessions", Some(payload)).await
        }
        "session/get" => {
            let session_id = require_string(&params, "sessionId")?;
            let limit = params
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(20);
            internal_json_request(
                state,
                Method::GET,
                &format!("/api/sessions/{session_id}?limit={limit}"),
                None,
            )
            .await
        }
        "session/olderTurns/get" => {
            let session_id = require_string(&params, "sessionId")?;
            let before_turn_id = require_string(&params, "beforeTurnId")?;
            let before_turn_id = urlencoding::encode(&before_turn_id);
            let limit = params
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(20);
            internal_json_request(
                state,
                Method::GET,
                &format!("/api/sessions/{session_id}/turns?beforeTurnId={before_turn_id}&limit={limit}"),
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
            let path_value = params
                .get("currentPath")
                .and_then(Value::as_str)
                .map(urlencoding::encode)
                .map(|encoded| format!("/api/directories?path={encoded}"))
                .unwrap_or_else(|| "/api/directories".to_string());
            internal_json_request(state, Method::GET, &path_value, None).await
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
        "account/get" => internal_json_request(state, Method::GET, "/api/account", None).await,
        "account/login/start" => {
            let payload = json!({
                "type": require_string(&params, "type")?,
                "apiKey": params.get("apiKey").cloned().unwrap_or(Value::Null)
            });
            internal_json_request(state, Method::POST, "/api/account/login", Some(payload)).await
        }
        "account/login/cancel" => {
            let payload = json!({
                "loginId": require_string(&params, "loginId")?
            });
            internal_json_request(state, Method::POST, "/api/account/login/cancel", Some(payload)).await
        }
        "account/logout" => internal_json_request(state, Method::POST, "/api/account/logout", Some(json!({}))).await,
        "git/repositories/list" => internal_json_request(state, Method::GET, "/api/git/repositories", None).await,
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
            let cwd = params.get("cwd").and_then(Value::as_str).map(str::to_string);
            let title = params.get("title").and_then(Value::as_str).map(str::to_string);
            create_terminal(state.clone(), cwd, title).await
        }
        "terminal/read" => {
            let terminal_id = require_string(&params, "terminalId")?;
            read_terminal(state, &terminal_id).await
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
            subscribe_session(state.clone(), out_tx.clone(), subscriptions.clone(), session_id.clone()).await?;
            Ok(json!({ "subscribed": true, "sessionId": session_id }))
        }
        "session/unsubscribe" => {
            let session_id = require_string(&params, "sessionId")?;
            let mut current = subscriptions.lock().await;
            if let Some(handle) = current.remove(&session_id) {
                handle.abort();
            }
            Ok(json!({ "subscribed": false, "sessionId": session_id }))
        }
        "terminal/subscribe" => {
            let terminal_id = require_string(&params, "terminalId")?;
            subscribe_terminal(state.clone(), out_tx.clone(), subscriptions.clone(), terminal_id.clone()).await?;
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
            subscribe_global(state.clone(), out_tx.clone(), subscriptions.clone()).await?;
            Ok(json!({ "subscribed": true, "scope": "global" }))
        }
        "events/unsubscribe" => {
            let mut current = subscriptions.lock().await;
            if let Some(handle) = current.remove(GLOBAL_RELAY_KEY) {
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
    session_id: String,
) -> Result<()> {
    let relay = ensure_stream_relay(&state, &session_id).await?;
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
    if let Some(existing) = current.insert(session_id, handle) {
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
                    warn!("websocket lagged on terminal {terminal_key}: skipped {skipped} messages");
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
) -> Result<()> {
    let relay = ensure_global_relay(&state).await?;
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
    if let Some(existing) = current.insert(GLOBAL_RELAY_KEY.to_string(), handle) {
        existing.abort();
    }
    Ok(())
}

async fn ensure_stream_relay(state: &AppState, session_id: &str) -> Result<broadcast::Sender<Value>> {
    let mut relays = state.relays.lock().await;
    if let Some(existing) = relays.get(session_id) {
        return Ok(existing.clone());
    }

    let (sender, _) = broadcast::channel(256);
    relays.insert(session_id.to_string(), sender.clone());

    let state = state.clone();
    let session_id = session_id.to_string();
    let relay_sender = sender.clone();
    tokio::spawn(async move {
        loop {
            if let Err(error) = stream_session_events(state.clone(), relay_sender.clone(), session_id.clone()).await {
                warn!("session stream relay failed for {session_id}: {error:#}");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    });

    Ok(sender)
}

async fn ensure_global_relay(state: &AppState) -> Result<broadcast::Sender<Value>> {
    let mut relays = state.relays.lock().await;
    if let Some(existing) = relays.get(GLOBAL_RELAY_KEY) {
        return Ok(existing.clone());
    }

    let (sender, _) = broadcast::channel(256);
    relays.insert(GLOBAL_RELAY_KEY.to_string(), sender.clone());

    let state = state.clone();
    let relay_sender = sender.clone();
    tokio::spawn(async move {
        loop {
            if let Err(error) = stream_global_events(state.clone(), relay_sender.clone()).await {
                warn!("global stream relay failed: {error:#}");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    });

    Ok(sender)
}

async fn stream_session_events(state: AppState, sender: broadcast::Sender<Value>, session_id: String) -> Result<()> {
    let target = internal_url(&state.config, &format!("/api/sessions/{session_id}/stream"));
    let response = state
        .http
        .get(target)
        .header(INTERNAL_HEADER, &state.config.internal_proxy_token)
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

async fn stream_global_events(state: AppState, sender: broadcast::Sender<Value>) -> Result<()> {
    let target = internal_url(&state.config, "/api/events/stream");
    let response = state
        .http
        .get(target)
        .header(INTERNAL_HEADER, &state.config.internal_proxy_token)
        .send()
        .await
        .context("failed to connect to internal global SSE stream")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("internal global SSE request failed with {status}: {body}"));
    }

    let stream = response
        .bytes_stream()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::Other, error));
    let reader = StreamReader::new(stream);
    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = lines.next_line().await.context("failed to read global SSE line")? {
        if let Some(data) = line.strip_prefix("data: ") {
            let payload: Value = serde_json::from_str(data).context("invalid global SSE json payload")?;
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
    state.terminals
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
    let resolved = fs::canonicalize(&candidate)
        .with_context(|| format!("terminal working directory is invalid: {}", candidate.display()))?;
    let metadata = fs::metadata(&resolved)
        .with_context(|| format!("failed to inspect {}", resolved.display()))?;
    if !metadata.is_dir() {
        anyhow::bail!("terminal working directory must be a directory.");
    }

    let config = internal_json_request(state, Method::GET, "/api/config", None).await?;
    let allowed_roots = config
        .get("allowedRoots")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry| entry.get("path").and_then(Value::as_str).map(str::to_string))
        .collect::<Vec<_>>();

    let allowed = allowed_roots.iter().any(|root| {
        let root_path = PathBuf::from(root);
        resolved == root_path || resolved.starts_with(&root_path)
    });

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

async fn create_terminal(state: AppState, cwd: Option<String>, title: Option<String>) -> Result<Value> {
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

async fn codex_quota_status(state: &AppState, refresh: bool) -> Result<Value> {
    if !refresh {
        let cache = state.quota_cache.lock().await;
        if let Some(cached) = cache.as_ref() {
            if cached.created_at.elapsed() < QUOTA_CACHE_TTL {
                return Ok(cached.payload.clone());
            }
        }
    }

    let payload = match fetch_codex_quota(state).await {
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
    *cache = Some(CachedQuota {
        created_at: Instant::now(),
        payload: payload.clone(),
    });

    Ok(payload)
}

async fn fetch_codex_quota(state: &AppState) -> Result<Value> {
    let auth = read_codex_auth(&state.config)?;
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

    let payload: UsageResponseShape = response.json().await.context("invalid Codex quota response")?;
    let five_hour = normalize_quota_window(payload.rate_limit.as_ref().and_then(|rate_limit| rate_limit.primary_window.as_ref()));
    let weekly = normalize_quota_window(payload.rate_limit.as_ref().and_then(|rate_limit| rate_limit.secondary_window.as_ref()));

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

fn read_codex_auth(config: &Config) -> Result<AuthFile> {
    let auth_path = config.codex_home.join("auth.json");
    let raw = fs::read_to_string(&auth_path)
        .with_context(|| format!("missing Codex auth file at {}.", auth_path.display()))?;
    serde_json::from_str(&raw).context("invalid Codex auth.json")
}

fn normalize_quota_window(window: Option<&UsageWindowShape>) -> Option<Value> {
    let window = window?;
    let used_percent = (window.used_percent.unwrap_or(0.0)).clamp(0.0, 100.0).round() as u64;
    let reset_after_seconds = window
        .reset_after_seconds
        .filter(|value| *value > 0)
        .map(|value| value as u64);
    let reset_at = reset_after_seconds.map(|seconds| now_unix_ms().saturating_add(seconds.saturating_mul(1000)));

    Some(json!({
        "usedPercent": used_percent,
        "remainingPercent": 100_u64.saturating_sub(used_percent),
        "resetAfterSeconds": reset_after_seconds,
        "resetAt": reset_at,
    }))
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
    run_command_with_timeout(which_command(), vec![name.to_string()], Duration::from_secs(2))
        .await
        .map(|output| output.status.success())
        .unwrap_or(false)
}

async fn resolve_binary_path(command: &str) -> Option<String> {
    let candidate = PathBuf::from(command);
    if candidate.exists() {
        return Some(candidate.display().to_string());
    }

    let output = run_command_with_timeout(which_command(), vec![command.to_string()], Duration::from_secs(2))
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let path = String::from_utf8_lossy(&output.stdout).lines().next()?.trim().to_string();
    if path.is_empty() {
        None
    } else {
        Some(path)
    }
}

fn which_command() -> &'static str {
    if cfg!(windows) {
        "where"
    } else {
        "which"
    }
}

fn npm_command() -> &'static str {
    if cfg!(windows) {
        "npm.cmd"
    } else {
        "npm"
    }
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

async fn upload_attachments(state: &AppState, session_id: &str, files: Vec<UploadFilePayload>) -> Result<Value> {
    let mut form = Form::new();
    for file in files {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(file.data_base64)
            .context("invalid base64 attachment payload")?;
        let part = Part::bytes(bytes)
            .file_name(file.name.clone())
            .mime_str(file.mime_type.as_deref().unwrap_or("application/octet-stream"))
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
        .request(reqwest::Method::from_bytes(method.as_str().as_bytes())?, target.to_string())
        .header(INTERNAL_HEADER, &state.config.internal_proxy_token);

    for (name, value) in headers.iter() {
        if name == header::HOST || name == header::CONTENT_LENGTH || name.as_str() == INTERNAL_HEADER {
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
    let status = StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let upstream_headers = upstream.headers().clone();
    let bytes = upstream.bytes().await.context("failed to read upstream response")?;

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

fn verify_password(config: &Config, input: &str) -> Result<bool> {
    if let Some(password) = &config.password {
        return Ok(password.as_bytes().ct_eq(input.as_bytes()).into());
    }

    let Some(password_hash) = &config.password_hash else {
        return Err(anyhow!(
            "Set CODEX_WEBUI_PASSWORD_HASH or CODEX_WEBUI_PASSWORD before using the Rust gateway."
        ));
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
    scrypt(input.as_bytes(), &salt, &params, &mut derived).context("failed to derive password hash")?;
    Ok(derived.ct_eq(&expected).into())
}

fn issue_auth_cookie(config: &Config, jar: CookieJar, secure_request: bool) -> Result<CookieJar> {
    let secure = resolve_cookie_secure(config, secure_request)?;
    let cookie_value = make_auth_token(config)?;
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

fn resolve_cookie_secure(config: &Config, secure_request: bool) -> Result<bool> {
    if config.cookie_same_site == SameSiteMode::None && config.cookie_secure_mode == CookieSecureMode::Never {
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

fn make_auth_token(config: &Config) -> Result<String> {
    let now = now_millis();
    let expires = now + 7 * 24 * 60 * 60 * 1000;
    let nonce = Uuid::new_v4().simple().to_string();
    let payload = format!("{now}.{expires}.{nonce}");
    let signature = sign(config, &payload)?;
    Ok(format!("{payload}.{signature}"))
}

fn is_authenticated(config: &Config, jar: &CookieJar) -> bool {
    let Some(cookie) = jar.get(AUTH_COOKIE) else {
        return false;
    };
    let token = cookie.value();
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 4 {
        return false;
    }
    let payload = parts[..3].join(".");
    let Ok(expected) = sign(config, &payload) else {
        return false;
    };
    if expected.as_bytes().ct_eq(parts[3].as_bytes()).unwrap_u8() != 1 {
        return false;
    }
    parts[1]
        .parse::<u128>()
        .map(|expires| now_millis() < expires)
        .unwrap_or(false)
}

fn sign(config: &Config, payload: &str) -> Result<String> {
    let secret = config
        .session_secret
        .clone()
        .or_else(|| config.password_hash.clone())
        .or_else(|| config.password.clone())
        .unwrap_or_default();
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).context("failed to initialize HMAC")?;
    mac.update(payload.as_bytes());
    Ok(URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
}

fn request_is_secure(headers: &HeaderMap) -> bool {
    if let Some(forwarded) = headers.get("x-forwarded-proto").and_then(|value| value.to_str().ok()) {
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
    if config.cors_allowed_origins.iter().any(|allowed| allowed == origin) {
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
        Some(value) => value.parse::<u16>().with_context(|| format!("invalid port: {value}")),
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
    if let Some(value) = env::var_os("CODEX_WEBUI_CODEX_HOME").or_else(|| env::var_os("CODEX_HOME")) {
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
    if cwd.join("build/index.js").exists() {
        return cwd.clone();
    }

    if cwd
        .file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value == "backend")
    {
        if let Some(parent) = cwd.parent() {
            let parent = parent.to_path_buf();
            if parent.join("build/index.js").exists() {
                return parent;
            }
        }
    }

    cwd.clone()
}

fn internal_url(config: &Config, path: &str) -> String {
    let route = if config.base_path.is_empty() {
        path.to_string()
    } else {
        format!("{}{}", config.base_path, path)
    };
    format!("{}{}", config.internal_base_url, route)
}

async fn spawn_internal_node(config: &Config) -> Result<Child> {
    let mut command = Command::new(&config.node_binary);
    command
        .arg(&config.node_entry)
        .current_dir(&config.project_root)
        .envs(env::vars())
        .env("HOST", "127.0.0.1")
        .env("PORT", config.internal_port.to_string())
        .env("CODEX_WEBUI_INTERNAL_PROXY_TOKEN", &config.internal_proxy_token)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let mut child = command.spawn().context("failed to spawn internal Node backend")?;
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if !line.trim().is_empty() {
                    info!("[internal-node] {line}");
                }
            }
        });
    }
    Ok(child)
}

async fn wait_for_internal_node(config: &Config, http: &reqwest::Client) -> Result<()> {
    let target = format!(
        "{}{}",
        config.internal_base_url,
        with_base(&config.base_path, "/")
    );

    for _ in 0..100 {
        if let Ok(response) = http
            .get(&target)
            .header(INTERNAL_HEADER, &config.internal_proxy_token)
            .send()
            .await
        {
            if response.status().is_success() || response.status().is_redirection() {
                info!("Internal Node backend is ready at {}", config.internal_base_url);
                return Ok(());
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

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
