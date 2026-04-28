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
    if role != UserRole::Admin {
        return Err(RolloutRecoveryActionError::new(
            StatusCode::FORBIDDEN,
            "FORBIDDEN_ROLE",
            "This action requires an admin role.",
        ));
    }

    let thread = read_thread_metadata_payload(state, profile_id, session_id)
        .await
        .map_err(|error| {
            RolloutRecoveryActionError::new(
                error.status,
                if error.status == StatusCode::NOT_FOUND {
                    "SESSION_NOT_FOUND"
                } else {
                    "SESSION_METADATA_READ_FAILED"
                },
                error.message,
            )
        })?;

    let Some(rollout_path) = resolve_rollout_path(state, profile_id, session_id, &thread) else {
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
