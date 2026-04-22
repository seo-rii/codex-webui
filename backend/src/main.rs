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
mod http_entry_support;
mod http_router_support;
mod notification_state_support;
mod queue_runtime_support;
mod relay_support;
mod request_cache_support;
mod rollout_recovery_support;
mod runtime_env_support;
mod runtime_notification_support;
mod runtime_request_support;
mod session_attachment_http_support;
mod session_draft_state_support;
mod session_http_support;
mod session_mutation_http_support;
mod session_preset_state_support;
mod session_queue_http_support;
mod session_queue_state_support;
mod session_route_dispatch_support;
mod session_summary_support;
mod session_transcript_http_support;
mod shared_types_support;
mod static_support;
mod system_support;
mod terminal_support;
mod thread_detail_support;
mod thread_listing_support;
mod thread_support;
mod transport_http_support;
mod turn_execution_support;
mod turn_fork_support;
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
use http_entry_support::*;
use http_router_support::*;
use notification_state_support::*;
use queue_runtime_support::*;
use relay_support::*;
use request_cache_support::*;
use rollout_recovery_support::*;
use runtime_env_support::*;
use runtime_notification_support::*;
use runtime_request_support::*;
use session_attachment_http_support::*;
use session_draft_state_support::*;
use session_http_support::*;
use session_mutation_http_support::*;
use session_preset_state_support::*;
use session_queue_http_support::*;
use session_queue_state_support::*;
use session_route_dispatch_support::*;
use session_summary_support::*;
use session_transcript_http_support::*;
use shared_types_support::*;
use static_support::*;
use system_support::*;
use terminal_support::*;
use thread_detail_support::*;
use thread_listing_support::*;
use thread_support::*;
use transport_http_support::*;
use turn_execution_support::*;
use turn_fork_support::*;
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

fn api_error(status: StatusCode, message: impl Into<String>) -> ApiError {
    ApiError {
        status,
        message: message.into(),
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

#[cfg(test)]
mod main_tests;
