use super::*;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RolloutRecoveryInfoPayload {
    pub(crate) available: bool,
    pub(crate) issue: Option<String>,
    pub(crate) total_lines: usize,
    pub(crate) recoverable_lines: usize,
    pub(crate) skipped_lines: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RolloutRecoveryPlanPayload {
    pub(crate) info: RolloutRecoveryInfoPayload,
    pub(crate) recovered_content: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RolloutRecoveryActionError {
    pub(crate) status: StatusCode,
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

impl RolloutRecoveryActionError {
    pub(crate) fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    pub(crate) fn into_ws_error(self) -> anyhow::Error {
        anyhow!(
            json!({
                "code": self.code,
                "message": self.message,
                "status": self.status.as_u16()
            })
            .to_string()
        )
    }
}

pub(crate) fn is_rollout_history_corruption_error(message: &str) -> bool {
    let lowered = message.to_lowercase();
    lowered.contains("stream did not contain valid utf-8")
        || lowered.contains("failed to load thread history")
        || lowered.contains("failed to load rollout")
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

fn expand_rollout_path_from_error_message(
    state: &AppState,
    profile_id: &str,
    message: &str,
) -> Option<PathBuf> {
    let jsonl_end = message.find(".jsonl")?.saturating_add(".jsonl".len());
    let before = &message[..jsonl_end];
    let start = before
        .rfind(['`', '\'', '"'])
        .map(|index| index.saturating_add(1))
        .or_else(|| {
            before
                .rfind(char::is_whitespace)
                .map(|index| index.saturating_add(1))
        })
        .unwrap_or(0);
    let raw_path = before[start..jsonl_end].trim();
    if raw_path.is_empty() {
        return None;
    }

    let profile = resolve_runtime_profile(&state.config, profile_id);
    let expanded = if let Some(suffix) = raw_path.strip_prefix("~/") {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join(suffix))
            .unwrap_or_else(|| PathBuf::from(raw_path))
    } else {
        PathBuf::from(raw_path)
    };
    let normalized = normalize_path(expanded);
    let normalized_codex_home = normalize_path(profile.codex_home.clone());
    if normalized
        .extension()
        .and_then(|extension| extension.to_str())
        != Some("jsonl")
        || !path_is_within(&normalized_codex_home, &normalized)
    {
        return None;
    }
    Some(normalized)
}

async fn resolve_session_rollout_path_for_recovery(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    error_message: Option<&str>,
) -> Option<PathBuf> {
    if let Ok(Some(thread)) =
        read_local_thread_metadata_payload(state, profile_id, session_id).await
    {
        if let Some(path) = resolve_rollout_path(state, profile_id, session_id, &thread) {
            return Some(path);
        }
    }

    for archived in [false, true] {
        if let Ok(candidates) = list_rollout_candidates_payload(state, profile_id, archived).await {
            if let Some(path) = candidates
                .iter()
                .find(|candidate| candidate.get("id").and_then(Value::as_str) == Some(session_id))
                .and_then(|candidate| candidate.get("path").and_then(Value::as_str))
                .map(PathBuf::from)
            {
                return Some(path);
            }
        }
    }

    if let Some(path) = error_message
        .and_then(|message| expand_rollout_path_from_error_message(state, profile_id, message))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(&format!("{session_id}.jsonl")))
        })
    {
        return Some(path);
    }

    if error_message.is_none() {
        if let Ok(thread) = read_thread_metadata_payload(state, profile_id, session_id).await {
            if let Some(path) = resolve_rollout_path(state, profile_id, session_id, &thread) {
                return Some(path);
            }
        }
    }

    None
}

pub(crate) async fn inspect_session_rollout_recovery_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    error_message: Option<&str>,
) -> Option<Value> {
    let rollout_path =
        resolve_session_rollout_path_for_recovery(state, profile_id, session_id, error_message)
            .await?;
    let rollout_buffer = tokio_fs::read(rollout_path).await.ok()?;
    let plan = inspect_rollout_recovery_content(&rollout_buffer);
    if !plan.info.available
        || plan.info.recoverable_lines == 0
        || plan.recovered_content.trim().is_empty()
    {
        return None;
    }
    Some(json!(plan.info))
}

pub(crate) fn inspect_rollout_recovery_content(buffer: &[u8]) -> RolloutRecoveryPlanPayload {
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

pub(crate) async fn recover_session_rollout_payload(
    state: &AppState,
    profile_id: &str,
    role: UserRole,
    session_id: &str,
) -> Result<Value, RolloutRecoveryActionError> {
    if !role_has_admin_access(role) {
        return Err(RolloutRecoveryActionError::new(
            StatusCode::FORBIDDEN,
            "FORBIDDEN_ROLE",
            "This action requires an admin role.",
        ));
    }

    let Some(rollout_path) =
        resolve_session_rollout_path_for_recovery(state, profile_id, session_id, None).await
    else {
        return Err(RolloutRecoveryActionError::new(
            StatusCode::NOT_FOUND,
            "SESSION_ROLLOUT_NOT_FOUND",
            "No persisted rollout file was found for this session.",
        ));
    };

    let rollout_buffer = tokio_fs::read(&rollout_path).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            RolloutRecoveryActionError::new(
                StatusCode::NOT_FOUND,
                "SESSION_ROLLOUT_NOT_FOUND",
                "No persisted rollout file was found for this session.",
            )
        } else {
            RolloutRecoveryActionError::new(
                StatusCode::BAD_GATEWAY,
                "SESSION_ROLLOUT_READ_FAILED",
                error.to_string(),
            )
        }
    })?;

    let plan = inspect_rollout_recovery_content(&rollout_buffer);
    if !plan.info.available
        || plan.info.recoverable_lines == 0
        || plan.recovered_content.trim().is_empty()
    {
        return Err(RolloutRecoveryActionError::new(
            StatusCode::CONFLICT,
            "SESSION_ROLLOUT_NOT_RECOVERABLE",
            "This session history could not be recovered automatically.",
        ));
    }

    let backup_path = PathBuf::from(format!("{}.bak-{}", rollout_path.display(), now_unix_ms()));
    tokio_fs::copy(&rollout_path, &backup_path)
        .await
        .map_err(|error| {
            RolloutRecoveryActionError::new(
                StatusCode::BAD_GATEWAY,
                "SESSION_ROLLOUT_BACKUP_FAILED",
                error.to_string(),
            )
        })?;
    write_file_atomically(&rollout_path, plan.recovered_content.as_bytes().to_vec())
        .await
        .map_err(|error| {
            RolloutRecoveryActionError::new(
                StatusCode::BAD_GATEWAY,
                "SESSION_ROLLOUT_WRITE_FAILED",
                error.to_string(),
            )
        })?;

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

    Ok(json!({
        "ok": true,
        "sessionId": session_id,
        "backupPath": backup_path.display().to_string(),
        "recoveredAt": now_unix_ms(),
        "totalLines": plan.info.total_lines,
        "recoveredLines": plan.info.recoverable_lines,
        "skippedLines": plan.info.skipped_lines
    }))
}
