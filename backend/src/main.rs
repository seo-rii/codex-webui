use std::{
    collections::{HashMap, HashSet, VecDeque},
    env, fs,
    future::Future,
    net::SocketAddr,
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
mod audit_support;
mod auth_support;
mod auth_transport_support;
mod automation_support;
mod autostart_support;
mod aux_http_support;
mod catalog_support;
mod codex_runtime_support;
mod config_http_support;
mod config_support;
mod constants_support;
mod event_mapping_support;
mod git_discovery_support;
mod git_read_support;
mod git_worktree_support;
mod git_write_support;
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
mod session_preferences_support;
mod session_preset_state_support;
mod session_queue_dispatch_support;
mod session_queue_http_support;
mod session_queue_mutation_support;
mod session_queue_support;
mod session_route_dispatch_support;
mod session_summary_support;
mod session_transcript_http_support;
mod shared_types_support;
mod shutdown_queue_support;
mod static_support;
mod system_support;
mod terminal_support;
mod thread_detail_support;
mod thread_listing_support;
mod thread_read_support;
mod thread_support;
mod turn_execution_support;
mod turn_fork_support;
mod ui_state_support;
mod workspace_support;
mod ws_dispatch_support;
mod ws_method_support;
mod ws_transport_support;

use app_util_support::*;
use arena_support::*;
use attachment_support::*;
use audit_support::*;
use auth_support::*;
use auth_transport_support::*;
use automation_support::*;
use autostart_support::*;
use aux_http_support::*;
use catalog_support::*;
use codex_runtime_support::*;
use config_http_support::*;
use config_support::*;
use constants_support::*;
use event_mapping_support::*;
use git_discovery_support::*;
use git_read_support::*;
use git_worktree_support::*;
use git_write_support::*;
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
use session_preferences_support::*;
use session_preset_state_support::*;
use session_queue_dispatch_support::*;
use session_queue_http_support::*;
use session_queue_mutation_support::*;
use session_queue_support::*;
use session_route_dispatch_support::*;
use session_summary_support::*;
use session_transcript_http_support::*;
use shared_types_support::*;
use shutdown_queue_support::*;
use static_support::*;
use system_support::*;
use terminal_support::*;
use thread_detail_support::*;
use thread_listing_support::*;
use thread_read_support::*;
use thread_support::*;
use turn_execution_support::*;
use turn_fork_support::*;
use ui_state_support::*;
use workspace_support::*;
use ws_dispatch_support::*;
use ws_method_support::*;
use ws_transport_support::*;

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
            session_thread_cache: Arc::new(Mutex::new(HashMap::new())),
            session_search_text_cache: Arc::new(Mutex::new(HashMap::new())),
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
#[cfg(test)]
mod main_tests;
