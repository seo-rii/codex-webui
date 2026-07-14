use super::*;
use std::sync::atomic::AtomicBool;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum UserRole {
    Owner,
    Admin,
    Viewer,
}

#[derive(Clone, Debug)]
pub(crate) struct AuthContext {
    pub(crate) role: UserRole,
    pub(crate) profile_id: String,
}

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) config: Arc<Config>,
    pub(crate) app_servers: AppServerManager,
    pub(crate) http: reqwest::Client,
    pub(crate) login_attempts: Arc<Mutex<HashMap<String, Vec<u128>>>>,
    pub(crate) response_cache: Arc<Mutex<HashMap<String, CachedResponse>>>,
    pub(crate) session_thread_cache: Arc<Mutex<HashMap<String, CachedSessionThreads>>>,
    pub(crate) session_thread_cache_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    pub(crate) session_search_text_cache: Arc<Mutex<HashMap<String, CachedSessionSearchText>>>,
    pub(crate) static_asset_cache: Arc<Mutex<HashMap<String, CachedStaticAsset>>>,
    pub(crate) catalog_cache: Arc<Mutex<HashMap<String, CachedCatalog>>>,
    pub(crate) git_repository_cache: Arc<Mutex<Option<CachedGitRepositories>>>,
    pub(crate) pinned_git_repositories: Arc<Mutex<HashMap<String, Value>>>,
    pub(crate) git_operation_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    pub(crate) inflight_requests: Arc<Mutex<HashMap<String, InflightRequest>>>,
    pub(crate) profile_request_slots: Arc<Mutex<HashMap<String, Arc<Semaphore>>>>,
    pub(crate) quota_cache: Arc<Mutex<HashMap<String, CachedQuota>>>,
    pub(crate) quota_refreshes: Arc<Mutex<HashSet<String>>>,
    pub(crate) attachment_storage_usage_cache:
        Arc<Mutex<HashMap<String, CachedAttachmentStorageUsage>>>,
    pub(crate) attachment_storage_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    pub(crate) relays: Arc<Mutex<HashMap<String, broadcast::Sender<Value>>>>,
    pub(crate) terminals: Arc<Mutex<HashMap<String, Arc<TerminalSession>>>>,
    pub(crate) session_summary_update_tasks:
        Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    pub(crate) runtime_config_update_tasks:
        Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    pub(crate) ui_state_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    pub(crate) ui_state_cache: Arc<Mutex<HashMap<String, Arc<Mutex<Value>>>>>,
    pub(crate) ui_state_persistence: Arc<Mutex<HashMap<String, UiStatePersistenceState>>>,
    pub(crate) automation_timers: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    pub(crate) queue_dispatching: Arc<Mutex<HashSet<String>>>,
    pub(crate) queue_drain_retries: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    pub(crate) session_operation_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    pub(crate) session_app_server_assignments: Arc<Mutex<HashMap<String, String>>>,
    pub(crate) active_turns: Arc<Mutex<HashMap<String, String>>>,
    pub(crate) pending_turn_starts: Arc<Mutex<HashSet<String>>>,
    pub(crate) recent_client_user_messages: Arc<Mutex<HashMap<String, Instant>>>,
    pub(crate) pending_server_requests:
        Arc<Mutex<HashMap<String, HashMap<String, PendingServerRequestEntry>>>>,
    pub(crate) account_login_flows: Arc<Mutex<HashMap<String, PendingAccountLoginFlow>>>,
    pub(crate) shutdown_timers: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    pub(crate) runtime_profile_monitors:
        Arc<std::sync::Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    pub(crate) preserve_app_servers_on_shutdown: Arc<AtomicBool>,
    pub(crate) shutdown_notify: Arc<Notify>,
    pub(crate) restart_plan: Arc<Mutex<Option<RestartPlan>>>,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct UiStatePersistenceState {
    pub(crate) revision: u64,
    pub(crate) persisted_revision: u64,
    pub(crate) writer_running: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct CachedAttachmentStorageUsage {
    pub(crate) scanned_at: Instant,
    pub(crate) bytes: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct PendingAccountLoginFlow {
    pub(crate) profile_id: String,
    pub(crate) state: String,
    pub(crate) code_verifier: String,
    pub(crate) redirect_uri: String,
    pub(crate) return_url: String,
    pub(crate) created_at: Instant,
}

#[derive(Clone, Debug)]
pub(crate) struct RestartPlan {
    pub(crate) command: String,
    pub(crate) args: Vec<String>,
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) mode: &'static str,
}

#[derive(Clone)]
pub(crate) struct CachedResponse {
    pub(crate) created_at: Instant,
    pub(crate) method: String,
    pub(crate) params_hash: String,
    pub(crate) response_bytes: usize,
    pub(crate) message: ServerEnvelope,
}

pub(crate) struct InflightRequest {
    pub(crate) created_at: Instant,
    pub(crate) method: String,
    pub(crate) params_hash: String,
    pub(crate) waiters: Vec<mpsc::Sender<ServerEnvelope>>,
}

#[derive(Clone)]
pub(crate) struct CachedSessionThreads {
    pub(crate) created_at: Instant,
    pub(crate) threads: Vec<Value>,
    pub(crate) next_cursor: String,
}

#[derive(Clone)]
pub(crate) struct CachedSessionSearchText {
    pub(crate) created_at: Instant,
    pub(crate) text_bytes: usize,
    pub(crate) text: String,
}

#[derive(Clone)]
pub(crate) struct CachedQuota {
    pub(crate) created_at: Instant,
    pub(crate) payload: Value,
}

#[derive(Clone)]
pub(crate) struct CachedStaticAsset {
    pub(crate) created_at: Instant,
    pub(crate) bytes: Bytes,
    pub(crate) content_type: &'static str,
    pub(crate) cache_control: &'static str,
}

#[derive(Clone)]
pub(crate) struct CachedCatalog {
    pub(crate) created_at: Instant,
    pub(crate) payload: Value,
}

#[derive(Clone)]
pub(crate) struct CachedGitRepositories {
    pub(crate) created_at: Instant,
    pub(crate) repositories: Vec<Value>,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(crate) struct PendingServerRequestEntry {
    pub(crate) raw_id: Value,
    pub(crate) client_key: String,
    pub(crate) method: String,
    pub(crate) params: Value,
    pub(crate) created_at: String,
    pub(crate) created_at_ms: u64,
}

pub(crate) struct TerminalSession {
    pub(crate) summary: Mutex<TerminalSummaryState>,
    pub(crate) buffer: Mutex<String>,
    pub(crate) stdin: Mutex<Option<tokio::process::ChildStdin>>,
    pub(crate) relay: broadcast::Sender<Value>,
    pub(crate) pid: Option<u32>,
    pub(crate) process_identity: Option<TerminalProcessIdentity>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TerminalProcessIdentity {
    pub(crate) pid: u32,
    pub(crate) process_group_id: u32,
    pub(crate) start_time_ticks: u64,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct TerminalSummaryState {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) cwd: String,
    #[serde(rename = "createdAt")]
    pub(crate) created_at: u64,
    #[serde(rename = "lastActivityAt")]
    pub(crate) last_activity_at: u64,
    pub(crate) status: String,
    #[serde(rename = "exitCode")]
    pub(crate) exit_code: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AuthFile {
    pub(crate) tokens: Option<AuthTokens>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AuthTokens {
    pub(crate) access_token: Option<String>,
    pub(crate) account_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UsageResponseShape {
    pub(crate) email: Option<String>,
    #[serde(alias = "planType")]
    pub(crate) plan_type: Option<String>,
    #[serde(alias = "rateLimit")]
    pub(crate) rate_limit: Option<UsageRateLimitShape>,
    #[serde(alias = "additionalRateLimits")]
    pub(crate) additional_rate_limits: Option<Vec<UsageAdditionalRateLimitShape>>,
    pub(crate) credits: Option<Value>,
    #[serde(alias = "spendControl")]
    pub(crate) spend_control: Option<Value>,
    #[serde(alias = "rateLimitReachedType")]
    pub(crate) rate_limit_reached_type: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UsageRateLimitShape {
    #[serde(alias = "primaryWindow")]
    pub(crate) primary_window: Option<UsageWindowShape>,
    #[serde(alias = "secondaryWindow")]
    pub(crate) secondary_window: Option<UsageWindowShape>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UsageAdditionalRateLimitShape {
    #[serde(alias = "limitName")]
    pub(crate) limit_name: Option<String>,
    #[serde(alias = "meteredFeature")]
    pub(crate) metered_feature: Option<String>,
    #[serde(alias = "rateLimit")]
    pub(crate) rate_limit: Option<UsageRateLimitShape>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UsageWindowShape {
    #[serde(alias = "usedPercent")]
    pub(crate) used_percent: Option<f64>,
    #[serde(alias = "resetAfterSeconds")]
    pub(crate) reset_after_seconds: Option<i64>,
    #[serde(alias = "resetAt", alias = "resetsAt")]
    pub(crate) reset_at: Option<i64>,
    #[serde(alias = "limitWindowSeconds")]
    pub(crate) limit_window_seconds: Option<i64>,
    #[serde(alias = "windowDurationMins", alias = "windowMinutes")]
    pub(crate) window_duration_mins: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum ClientEnvelope {
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
pub(crate) enum ServerEnvelope {
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
        #[serde(rename = "profileId")]
        profile_id: String,
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
    ResyncRequired {
        reason: String,
    },
    Pong {
        nonce: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
pub(crate) struct UploadFilePayload {
    pub(crate) name: String,
    pub(crate) mime_type: Option<String>,
    pub(crate) data_base64: String,
}

#[derive(Clone, Debug)]
pub(crate) struct AttachmentUploadPayload {
    pub(crate) name: String,
    pub(crate) mime_type: Option<String>,
    pub(crate) bytes: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StoredAttachmentRecord {
    pub(crate) id: String,
    #[serde(alias = "originalName")]
    pub(crate) original_name: String,
    pub(crate) path: Option<String>,
    #[serde(alias = "mimeType")]
    pub(crate) mime_type: Option<String>,
    pub(crate) size: Option<u64>,
    pub(crate) kind: Option<String>,
    #[serde(alias = "createdAt")]
    pub(crate) created_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct AuditLogEntry {
    pub(crate) id: String,
    pub(crate) at: u64,
    pub(crate) role: String,
    pub(crate) method: String,
    pub(crate) target: Option<String>,
    pub(crate) ok: bool,
    pub(crate) error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArenaContestantRecord {
    pub(crate) id: String,
    pub(crate) session_id: String,
    pub(crate) model: String,
    pub(crate) label: String,
    pub(crate) status: String,
    pub(crate) response: Option<String>,
    pub(crate) created_at: u64,
    pub(crate) updated_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArenaRunRecord {
    pub(crate) id: String,
    pub(crate) prompt: String,
    pub(crate) cwd: String,
    pub(crate) status: String,
    pub(crate) created_at: u64,
    pub(crate) updated_at: u64,
    pub(crate) contestants: Vec<ArenaContestantRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(crate) struct ArenaStoreState {
    pub(crate) runs: Vec<ArenaRunRecord>,
}

#[derive(Clone, Debug)]
pub(crate) struct ApiError {
    pub(crate) status: StatusCode,
    pub(crate) message: String,
}

pub(crate) type ApiResult<T> = std::result::Result<T, ApiError>;

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

impl TerminalSession {
    pub(crate) async fn summary(&self) -> TerminalSummaryState {
        self.summary.lock().await.clone()
    }

    pub(crate) async fn snapshot(&self) -> (TerminalSummaryState, String) {
        let summary = self.summary().await;
        let buffer = self.buffer.lock().await.clone();
        (summary, buffer)
    }

    pub(crate) async fn write_input(&self, data: &str) -> Result<()> {
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

    pub(crate) async fn append_output(&self, text: &str) {
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

    pub(crate) async fn mark_exited(&self, exit_code: Option<i32>) {
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
