use std::{
    collections::{HashMap, HashSet, VecDeque},
    env, fs,
    future::Future,
    net::SocketAddr,
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{
        ConnectInfo, FromRequest, Multipart, Request, State,
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
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
    runtime::Builder as TokioRuntimeBuilder,
    sync::{Mutex, Notify, OwnedSemaphorePermit, Semaphore, broadcast, mpsc},
};
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

const SERVER_THREAD_STACK_BYTES: usize = 16 * 1024 * 1024;

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
mod restart_support;
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
mod session_rollout_index_support;
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
use restart_support::*;
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
use session_rollout_index_support::*;
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

fn main() -> Result<()> {
    let config = Arc::new(Config::from_env()?);
    install_panic_logger(config.clone());

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let server_threads = runtime_thread_count_from_env(
        "CODEX_WEBUI_SERVER_THREADS",
        std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(4)
            .max(4),
        2,
        64,
    );
    let blocking_threads = runtime_thread_count_from_env(
        "CODEX_WEBUI_BLOCKING_THREADS",
        server_threads.saturating_mul(8).max(32),
        8,
        512,
    );

    info!(
        server_threads,
        blocking_threads,
        server_thread_stack_bytes = SERVER_THREAD_STACK_BYTES,
        "starting codex-webui runtime"
    );

    let runtime = TokioRuntimeBuilder::new_multi_thread()
        .enable_all()
        .worker_threads(server_threads)
        .max_blocking_threads(blocking_threads)
        .thread_stack_size(SERVER_THREAD_STACK_BYTES)
        .thread_name("codex-webui-server")
        .build()
        .context("failed to build codex-webui server runtime")?;

    runtime.block_on(run_gateway(config))
}

fn runtime_thread_count_from_env(
    name: &str,
    fallback: usize,
    minimum: usize,
    maximum: usize,
) -> usize {
    env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
        .clamp(minimum, maximum)
}

async fn run_gateway(config: Arc<Config>) -> Result<()> {
    let result = async {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .build()
            .context("failed to build reqwest client")?;

        let state = AppState {
            config: config.clone(),
            app_servers: AppServerManager::new(AppServerClientConfig {
                codex_bin: config.codex_bin.clone(),
                stderr_log_path: Some(runtime_logs_dir(&config).join("codex-app-server.log")),
                handoff_dir: config
                    .app_server_handoff_enabled
                    .then(|| config.data_dir.join("app-server-handoff")),
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
            git_operation_locks: Arc::new(Mutex::new(HashMap::new())),
            inflight_requests: Arc::new(Mutex::new(HashMap::new())),
            profile_request_slots: Arc::new(Mutex::new(HashMap::new())),
            quota_cache: Arc::new(Mutex::new(HashMap::new())),
            relays: Arc::new(Mutex::new(HashMap::new())),
            terminals: Arc::new(Mutex::new(HashMap::new())),
            ui_state_locks: Arc::new(Mutex::new(HashMap::new())),
            ui_state_cache: Arc::new(Mutex::new(HashMap::new())),
            automation_timers: Arc::new(Mutex::new(HashMap::new())),
            queue_dispatching: Arc::new(Mutex::new(HashSet::new())),
            queue_drain_retries: Arc::new(Mutex::new(HashMap::new())),
            active_turns: Arc::new(Mutex::new(HashMap::new())),
            pending_turn_starts: Arc::new(Mutex::new(HashSet::new())),
            pending_server_requests: Arc::new(Mutex::new(HashMap::new())),
            shutdown_timers: Arc::new(Mutex::new(HashMap::new())),
            preserve_app_servers_on_shutdown: Arc::new(AtomicBool::new(false)),
            shutdown_notify: Arc::new(Notify::new()),
            restart_plan: Arc::new(Mutex::new(None)),
        };

        tokio::spawn(restore_automation_schedules(state.clone()));
        spawn_terminal_cleanup_loop(state.clone(), Duration::from_secs(60));
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

        let server_result = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown_signal(state.shutdown_notify.clone()))
        .await
        .context("axum server terminated unexpectedly");
        let restart_plan = state.restart_plan.lock().await.take();
        if state
            .preserve_app_servers_on_shutdown
            .load(Ordering::SeqCst)
        {
            let _ = state.app_servers.detach_all().await;
        } else {
            let _ = state.app_servers.close_all().await;
        }
        if let Some(plan) = restart_plan {
            spawn_gateway_restart(&config, plan)
                .await
                .context("failed to start replacement gateway")?;
        }
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

async fn shutdown_signal(shutdown_notify: Arc<Notify>) {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            warn!("failed to install ctrl-c handler: {error}");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => {
                warn!("failed to install SIGTERM handler: {error}");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = shutdown_notify.notified() => {},
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
#[cfg(test)]
mod main_tests;
