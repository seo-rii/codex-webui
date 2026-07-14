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

fn rollout_line_is_semantically_empty(raw_line: &str) -> bool {
    raw_line
        .trim_start_matches('\u{feff}')
        .chars()
        .filter(|character| *character != '\0')
        .all(char::is_whitespace)
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
    let raw_path = before[start..jsonl_end]
        .trim()
        .trim_matches(['`', '\'', '"']);
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
    if let Some(path) = error_message
        .and_then(|message| expand_rollout_path_from_error_message(state, profile_id, message))
        .filter(|path| {
            path.exists()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(&format!("{session_id}.jsonl")))
        })
    {
        return Some(path);
    }

    if let Ok(Some(thread)) =
        read_local_thread_metadata_payload(state, profile_id, session_id).await
    {
        if let Some(path) = resolve_rollout_path(state, profile_id, session_id, &thread) {
            if path.exists() {
                return Some(path);
            }
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
                if path.exists() {
                    return Some(path);
                }
            }
        }
    }

    let profile = resolve_runtime_profile(&state.config, profile_id);
    let sessions_root = normalize_path(profile.codex_home.join("sessions"));
    let mut pending_dirs = vec![sessions_root.clone()];
    while let Some(dir) = pending_dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = normalize_path(entry.path());
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                pending_dirs.push(path);
                continue;
            }
            if !file_type.is_file()
                || !path_is_within(&sessions_root, &path)
                || path.extension().and_then(|extension| extension.to_str()) != Some("jsonl")
            {
                continue;
            }
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(&format!("{session_id}.jsonl")))
            {
                return Some(path);
            }
        }
    }

    if error_message.is_none() {
        if let Ok(thread) = read_thread_metadata_payload(state, profile_id, session_id).await {
            if let Some(path) = resolve_rollout_path(state, profile_id, session_id, &thread) {
                if path.exists() {
                    return Some(path);
                }
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
    let info = tokio::task::spawn_blocking(move || inspect_rollout_recovery_file(&rollout_path))
        .await
        .ok()?
        .ok()?;
    if !info.available || info.recoverable_lines == 0 {
        return None;
    }
    Some(json!(info))
}

fn process_rollout_recovery_reader(
    reader: &mut dyn std::io::BufRead,
    mut recovered: Option<&mut dyn std::io::Write>,
    defer_unterminated_tail: bool,
) -> std::io::Result<(RolloutRecoveryInfoPayload, bool)> {
    let mut raw_line = Vec::new();
    let mut invalid_utf8 = false;
    let mut deferred_unterminated_tail = false;
    let mut total_lines = 0_usize;
    let mut recoverable_lines = 0_usize;
    let mut skipped_lines = 0_usize;

    loop {
        raw_line.clear();
        let bytes_read = reader.read_until(b'\n', &mut raw_line)?;
        if bytes_read == 0 {
            break;
        }
        let terminated_with_newline = raw_line.last() == Some(&b'\n');
        while raw_line
            .last()
            .is_some_and(|byte| matches!(byte, b'\n' | b'\r'))
        {
            raw_line.pop();
        }
        let decoded = std::str::from_utf8(&raw_line).ok();
        if decoded.is_some_and(rollout_line_is_semantically_empty) {
            continue;
        }
        let lossy_decoded;
        let decoded_for_recovery = if let Some(decoded) = decoded {
            decoded
        } else {
            lossy_decoded = String::from_utf8_lossy(&raw_line);
            &lossy_decoded
        };
        let normalized = normalize_rollout_line(decoded_for_recovery);
        if normalized.is_none() && !terminated_with_newline && defer_unterminated_tail {
            deferred_unterminated_tail = true;
            continue;
        }

        total_lines = total_lines.saturating_add(1);
        invalid_utf8 |= decoded.is_none();
        if let Some(normalized) = normalized {
            recoverable_lines = recoverable_lines.saturating_add(1);
            if let Some(writer) = recovered.as_deref_mut() {
                writer.write_all(normalized.as_bytes())?;
                writer.write_all(b"\n")?;
            }
        } else {
            skipped_lines = skipped_lines.saturating_add(1);
        }
    }

    let issue = if invalid_utf8 {
        Some("invalidUtf8".to_string())
    } else if skipped_lines > 0 {
        Some("invalidJson".to_string())
    } else {
        None
    };
    Ok((
        RolloutRecoveryInfoPayload {
            available: recoverable_lines > 0 && issue.is_some(),
            issue,
            total_lines,
            recoverable_lines,
            skipped_lines,
        },
        deferred_unterminated_tail,
    ))
}

pub(crate) fn inspect_rollout_recovery_file(
    path: &Path,
) -> std::io::Result<RolloutRecoveryInfoPayload> {
    type RecoveryCacheEntry = (
        u64,
        Option<std::time::SystemTime>,
        RolloutRecoveryInfoPayload,
    );
    static RECOVERY_CACHE: std::sync::OnceLock<
        std::sync::Mutex<HashMap<PathBuf, RecoveryCacheEntry>>,
    > = std::sync::OnceLock::new();
    static RECOVERY_LOCKS: std::sync::OnceLock<
        std::sync::Mutex<HashMap<PathBuf, Arc<std::sync::Mutex<()>>>>,
    > = std::sync::OnceLock::new();

    let cache_key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let path_lock = {
        let mut locks = RECOVERY_LOCKS
            .get_or_init(|| std::sync::Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if locks.len() >= 128 && !locks.contains_key(&cache_key) {
            locks.retain(|_, lock| Arc::strong_count(lock) > 1);
        }
        Arc::clone(
            locks
                .entry(cache_key.clone())
                .or_insert_with(|| Arc::new(std::sync::Mutex::new(()))),
        )
    };
    let _path_guard = path_lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let metadata = fs::metadata(path)?;
    let file_len = metadata.len();
    let modified_at = metadata.modified().ok();
    let defer_unterminated_tail = modified_at
        .and_then(|modified| modified.elapsed().ok())
        .is_none_or(|age| age < std::time::Duration::from_secs(3));
    if let Some((_, _, info)) = RECOVERY_CACHE
        .get_or_init(|| std::sync::Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&cache_key)
        .filter(|(cached_len, cached_modified, _)| {
            *cached_len == file_len && *cached_modified == modified_at
        })
        .cloned()
    {
        return Ok(info);
    }

    let file = fs::File::open(path)?;
    let mut reader = std::io::BufReader::new(file);
    let (info, deferred_tail) =
        process_rollout_recovery_reader(&mut reader, None, defer_unterminated_tail)?;
    if deferred_tail {
        return Ok(info);
    }
    let mut cache = RECOVERY_CACHE
        .get_or_init(|| std::sync::Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if cache.len() >= 128 && !cache.contains_key(&cache_key) {
        cache.clear();
    }
    cache.insert(cache_key, (file_len, modified_at, info.clone()));
    Ok(info)
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

    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id).0;
    let session_lock = session_operation_lock(state, &resolved_profile_id, session_id).await;
    let _session_guard = session_lock.lock().await;
    let runtime_key = runtime_session_key(&resolved_profile_id, session_id);
    if state.active_turns.lock().await.contains_key(&runtime_key)
        || state
            .pending_turn_starts
            .lock()
            .await
            .contains(&runtime_key)
        || state.queue_dispatching.lock().await.contains(&runtime_key)
    {
        return Err(RolloutRecoveryActionError::new(
            StatusCode::CONFLICT,
            "SESSION_ACTIVE",
            "Stop the active session before recovering its rollout.",
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
    let source_metadata = tokio_fs::metadata(&rollout_path).await.map_err(|error| {
        RolloutRecoveryActionError::new(
            StatusCode::NOT_FOUND,
            "SESSION_ROLLOUT_NOT_FOUND",
            error.to_string(),
        )
    })?;
    let source_len = source_metadata.len();
    let source_modified = source_metadata.modified().ok();

    let parent = rollout_path.parent().ok_or_else(|| {
        RolloutRecoveryActionError::new(
            StatusCode::BAD_GATEWAY,
            "SESSION_ROLLOUT_WRITE_FAILED",
            "Rollout path has no parent directory.",
        )
    })?;
    let recovered_temp_path = parent.join(format!(".codex-webui-recovery-{}.tmp", Uuid::new_v4()));
    let source_path = rollout_path.clone();
    let temp_path = recovered_temp_path.clone();
    let recovery_info = tokio::task::spawn_blocking(move || {
        let source = fs::File::open(source_path)?;
        let mut reader = std::io::BufReader::new(source);
        let mut recovered = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(temp_path)?;
        let (info, _) = process_rollout_recovery_reader(&mut reader, Some(&mut recovered), false)?;
        recovered.sync_all()?;
        Ok::<_, std::io::Error>(info)
    })
    .await
    .map_err(|error| {
        RolloutRecoveryActionError::new(
            StatusCode::BAD_GATEWAY,
            "SESSION_ROLLOUT_READ_FAILED",
            error.to_string(),
        )
    })?
    .map_err(|error| {
        RolloutRecoveryActionError::new(
            if error.kind() == std::io::ErrorKind::NotFound {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::BAD_GATEWAY
            },
            "SESSION_ROLLOUT_READ_FAILED",
            error.to_string(),
        )
    })?;

    if !recovery_info.available || recovery_info.recoverable_lines == 0 {
        let _ = tokio_fs::remove_file(&recovered_temp_path).await;
        return Err(RolloutRecoveryActionError::new(
            StatusCode::CONFLICT,
            "SESSION_ROLLOUT_NOT_RECOVERABLE",
            "This session history could not be recovered automatically.",
        ));
    }

    let current_metadata = tokio_fs::metadata(&rollout_path).await.map_err(|error| {
        RolloutRecoveryActionError::new(
            StatusCode::CONFLICT,
            "SESSION_ROLLOUT_CHANGED",
            error.to_string(),
        )
    })?;
    if current_metadata.len() != source_len || current_metadata.modified().ok() != source_modified {
        let _ = tokio_fs::remove_file(&recovered_temp_path).await;
        return Err(RolloutRecoveryActionError::new(
            StatusCode::CONFLICT,
            "SESSION_ROLLOUT_CHANGED",
            "The rollout changed while recovery was running. Retry after the session is idle.",
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
    let current_metadata = tokio_fs::metadata(&rollout_path).await.map_err(|error| {
        RolloutRecoveryActionError::new(
            StatusCode::CONFLICT,
            "SESSION_ROLLOUT_CHANGED",
            error.to_string(),
        )
    })?;
    if current_metadata.len() != source_len || current_metadata.modified().ok() != source_modified {
        let _ = tokio_fs::remove_file(&recovered_temp_path).await;
        return Err(RolloutRecoveryActionError::new(
            StatusCode::CONFLICT,
            "SESSION_ROLLOUT_CHANGED",
            "The rollout changed while its backup was being created. The original was preserved.",
        ));
    }
    tokio_fs::rename(&recovered_temp_path, &rollout_path)
        .await
        .map_err(|error| {
            let _ = fs::remove_file(&recovered_temp_path);
            RolloutRecoveryActionError::new(
                StatusCode::BAD_GATEWAY,
                "SESSION_ROLLOUT_WRITE_FAILED",
                error.to_string(),
            )
        })?;
    if let Ok(parent_dir) = tokio_fs::File::open(parent).await {
        let _ = parent_dir.sync_all().await;
    }

    append_runtime_error_log(
        &state.config,
        "rust-gateway",
        "recovered corrupted rollout",
        json!({
            "threadId": session_id,
            "rolloutPath": rollout_path.display().to_string(),
            "backupPath": backup_path.display().to_string(),
            "recovery": recovery_info
        }),
    );

    Ok(json!({
        "ok": true,
        "sessionId": session_id,
        "backupPath": backup_path.display().to_string(),
        "recoveredAt": now_unix_ms(),
        "totalLines": recovery_info.total_lines,
        "recoveredLines": recovery_info.recoverable_lines,
        "skippedLines": recovery_info.skipped_lines
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_rollout_line() -> String {
        serde_json::to_string(&json!({
            "timestamp": "2026-07-13T00:00:00Z",
            "type": "event_msg",
            "payload": { "type": "user_message", "message": "hello" }
        }))
        .unwrap()
    }

    #[test]
    fn recovery_inspection_defers_an_unterminated_json_tail() {
        let input = format!("{}\n{{\"partial\":", valid_rollout_line());
        let mut reader = std::io::Cursor::new(input.into_bytes());

        let (info, deferred) = process_rollout_recovery_reader(&mut reader, None, true).unwrap();

        assert!(deferred);
        assert!(!info.available);
        assert_eq!(info.issue, None);
        assert_eq!(info.total_lines, 1);
        assert_eq!(info.recoverable_lines, 1);
        assert_eq!(info.skipped_lines, 0);
    }

    #[test]
    fn recovery_inspection_defers_an_unterminated_utf8_tail() {
        let mut input = format!("{}\n{{\"partial\":\"", valid_rollout_line()).into_bytes();
        input.push(0xff);
        let mut reader = std::io::Cursor::new(input);

        let (info, deferred) = process_rollout_recovery_reader(&mut reader, None, true).unwrap();

        assert!(deferred);
        assert!(!info.available);
        assert_eq!(info.issue, None);
        assert_eq!(info.skipped_lines, 0);
    }

    #[test]
    fn recovery_inspection_reports_a_newline_terminated_invalid_line() {
        let input = format!("{}\n{{\"broken\":\n", valid_rollout_line());
        let mut reader = std::io::Cursor::new(input.into_bytes());

        let (info, deferred) = process_rollout_recovery_reader(&mut reader, None, true).unwrap();

        assert!(!deferred);
        assert!(info.available);
        assert_eq!(info.issue.as_deref(), Some("invalidJson"));
        assert_eq!(info.skipped_lines, 1);
    }

    #[test]
    fn recovery_inspection_reports_a_stable_unterminated_invalid_tail() {
        let input = format!("{}\n{{\"broken\":", valid_rollout_line());
        let mut reader = std::io::Cursor::new(input.into_bytes());

        let (info, deferred) = process_rollout_recovery_reader(&mut reader, None, false).unwrap();

        assert!(!deferred);
        assert!(info.available);
        assert_eq!(info.issue.as_deref(), Some("invalidJson"));
        assert_eq!(info.skipped_lines, 1);
    }

    #[test]
    fn recovery_inspection_ignores_bom_and_unicode_whitespace_lines() {
        let input = format!("\u{feff}\u{2003}\0\n{}\n", valid_rollout_line());
        let mut reader = std::io::Cursor::new(input.into_bytes());

        let (info, deferred) = process_rollout_recovery_reader(&mut reader, None, true).unwrap();

        assert!(!deferred);
        assert!(!info.available);
        assert_eq!(info.total_lines, 1);
        assert_eq!(info.recoverable_lines, 1);
        assert_eq!(info.skipped_lines, 0);
    }
}
