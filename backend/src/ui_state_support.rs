use super::*;

const UI_STATE_SCHEMA_VERSION: u64 = 1;
const UI_STATE_WRITE_DEBOUNCE: Duration = Duration::from_millis(250);
const UI_STATE_WRITE_RETRY_DELAY: Duration = Duration::from_secs(1);
const STORED_NOTIFICATION_NAME_MAX_CHARS: usize = 160;
const STORED_NOTIFICATION_PAYLOAD_MAX_BYTES: usize = 16 * 1024;

pub(crate) fn compact_stored_notification(notification: &mut Value) {
    let Some(notification) = notification.as_object_mut() else {
        return;
    };
    if let Some(name) = notification.get("sessionName").and_then(Value::as_str)
        && name.chars().count() > STORED_NOTIFICATION_NAME_MAX_CHARS
    {
        let mut truncated = name
            .chars()
            .take(STORED_NOTIFICATION_NAME_MAX_CHARS)
            .collect::<String>();
        truncated.push_str("...");
        notification.insert("sessionName".to_string(), Value::String(truncated));
    }
    if let Some(payload) = notification.get_mut("payload")
        && let Ok(encoded) = serde_json::to_vec(payload)
        && encoded.len() > STORED_NOTIFICATION_PAYLOAD_MAX_BYTES
    {
        *payload = json!({
            "truncated": true,
            "originalBytes": encoded.len()
        });
    }
}

pub(crate) fn default_notification_settings_value() -> Value {
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
        "schemaVersion": UI_STATE_SCHEMA_VERSION,
        "global": {
            "shutdownAfterQueueCompletes": false,
            "shutdownAfterQueueCompletesPrimed": false,
            "scheduledShutdown": Value::Null,
            "scheduledShutdownBlockedReason": Value::Null,
            "dataRecoveryEvents": []
        },
        "notifications": {
            "items": [],
            "settings": default_notification_settings_value(),
            "webhookFailures": []
        },
        "sessionFoldersByName": {},
        "sessionMetaByThreadId": {},
        "savedSessionFilters": [],
        "promptPresets": [],
        "automations": [],
        "automationRuns": [],
        "preferencesByThreadId": {},
        "skillsByThreadId": {},
        "draftsByThreadId": {},
        "queuesByThreadId": {},
        "goalsByThreadId": {},
        "highlightsByThreadId": {},
        "languageBridgeByThreadId": {},
        "runtimeStatusByThreadId": {}
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

    if root.get("schemaVersion").and_then(Value::as_u64) != Some(UI_STATE_SCHEMA_VERSION) {
        root.insert("schemaVersion".to_string(), json!(UI_STATE_SCHEMA_VERSION));
    }

    if !root.get("global").is_some_and(Value::is_object) {
        root.insert(
            "global".to_string(),
            json!({
                "shutdownAfterQueueCompletes": false,
                "shutdownAfterQueueCompletesPrimed": false,
                "scheduledShutdown": Value::Null,
                "scheduledShutdownBlockedReason": Value::Null,
                "dataRecoveryEvents": []
            }),
        );
    }

    if let Some(global) = root.get_mut("global").and_then(Value::as_object_mut) {
        if !global
            .get("shutdownAfterQueueCompletesPrimed")
            .is_some_and(Value::is_boolean)
        {
            global.insert(
                "shutdownAfterQueueCompletesPrimed".to_string(),
                json!(false),
            );
        }
        if !global
            .get("scheduledShutdownBlockedReason")
            .is_some_and(|value| value.is_null() || value.is_string())
        {
            global.insert("scheduledShutdownBlockedReason".to_string(), Value::Null);
        }
        if !global
            .get("dataRecoveryEvents")
            .is_some_and(Value::is_array)
        {
            global.insert("dataRecoveryEvents".to_string(), json!([]));
        }
    }

    if !root.get("notifications").is_some_and(Value::is_object) {
        root.insert(
            "notifications".to_string(),
            json!({
                "items": [],
                "settings": default_notification_settings_value(),
                "webhookFailures": []
            }),
        );
    }

    if let Some(notifications) = root.get_mut("notifications").and_then(Value::as_object_mut) {
        if !notifications.get("items").is_some_and(Value::is_array) {
            notifications.insert("items".to_string(), json!([]));
        }
        if let Some(items) = notifications.get_mut("items").and_then(Value::as_array_mut) {
            for notification in items {
                compact_stored_notification(notification);
            }
        }
        if !notifications
            .get("webhookFailures")
            .is_some_and(Value::is_array)
        {
            notifications.insert("webhookFailures".to_string(), json!([]));
        }
        let normalized_settings =
            normalize_notification_settings_value(notifications.get("settings"));
        notifications.insert("settings".to_string(), normalized_settings);
    }

    for (key, default_value) in [
        ("sessionFoldersByName", json!({})),
        ("sessionMetaByThreadId", json!({})),
        ("savedSessionFilters", json!([])),
        ("promptPresets", json!([])),
        ("automations", json!([])),
        ("automationRuns", json!([])),
        ("preferencesByThreadId", json!({})),
        ("skillsByThreadId", json!({})),
        ("draftsByThreadId", json!({})),
        ("queuesByThreadId", json!({})),
        ("goalsByThreadId", json!({})),
        ("highlightsByThreadId", json!({})),
        ("languageBridgeByThreadId", json!({})),
        ("runtimeStatusByThreadId", json!({})),
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

fn append_data_recovery_event(
    ui_state: &mut Value,
    kind: &str,
    path: &std::path::Path,
    backup_path: &std::path::Path,
    restored_from_backup: bool,
    source_backup_path: Option<&std::path::Path>,
) {
    ensure_ui_state_sections(ui_state);
    let Some(events) = ui_state
        .get_mut("global")
        .and_then(Value::as_object_mut)
        .and_then(|global| global.get_mut("dataRecoveryEvents"))
        .and_then(Value::as_array_mut)
    else {
        return;
    };

    events.insert(
        0,
        json!({
            "id": Uuid::new_v4().to_string(),
            "kind": kind,
            "at": now_unix_ms(),
            "path": path.display().to_string(),
            "backupPath": backup_path.display().to_string(),
            "sourceBackupPath": source_backup_path
                .map(|path| Value::String(path.display().to_string()))
                .unwrap_or(Value::Null),
            "restoredFromBackup": restored_from_backup
        }),
    );
    if events.len() > 20 {
        events.truncate(20);
    }
}

pub(crate) fn is_valid_notification_event_type(value: &str) -> bool {
    matches!(
        value,
        "sessionCompleted" | "sessionAttention" | "queueDispatchFailed" | "shutdownScheduled"
    )
}

pub(crate) fn normalize_notification_settings_value(value: Option<&Value>) -> Value {
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

pub(crate) fn profile_ui_state_path(config: &Config, profile_id: &str) -> PathBuf {
    resolve_runtime_profile(config, profile_id)
        .data_dir
        .join("ui-state.json")
}

pub(crate) async fn ui_state_lock(state: &AppState, profile_id: &str) -> Arc<Mutex<()>> {
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
            let stable_backup_path = path.with_extension("json.bak");
            if let Ok(backup_raw) = tokio_fs::read_to_string(&stable_backup_path).await {
                if let Ok(mut recovered) = serde_json::from_str::<Value>(&backup_raw) {
                    ensure_ui_state_sections(&mut recovered);
                    append_data_recovery_event(
                        &mut recovered,
                        "uiState",
                        &path,
                        &backup_path,
                        true,
                        Some(&stable_backup_path),
                    );
                    write_file_atomically(
                        &path,
                        serde_json::to_vec_pretty(&recovered)
                            .expect("recovered ui-state should serialize"),
                    )
                    .await
                    .context("failed to restore ui-state from backup")?;
                    return Ok(recovered);
                }
            }
            let mut fallback = default_ui_state_value();
            append_data_recovery_event(&mut fallback, "uiState", &path, &backup_path, false, None);
            write_file_atomically(
                &path,
                serde_json::to_vec_pretty(&fallback).expect("default ui-state should serialize"),
            )
            .await
            .context("failed to recreate ui-state file after corruption")?;
            Ok(fallback)
        }
    }
}

async fn ensure_cached_profile_ui_state(state: &AppState, profile_id: &str) -> Result<()> {
    if state.ui_state_cache.lock().await.contains_key(profile_id) {
        return Ok(());
    }

    let ui_state = read_profile_ui_state(&state.config, profile_id).await?;
    cache_profile_ui_state(state, profile_id, ui_state).await;
    Ok(())
}

async fn cache_profile_ui_state(state: &AppState, profile_id: &str, ui_state: Value) {
    let dirty_profiles = state
        .ui_state_persistence
        .lock()
        .await
        .iter()
        .filter_map(|(profile_id, persistence)| {
            (persistence.revision != persistence.persisted_revision).then(|| profile_id.clone())
        })
        .collect::<HashSet<_>>();
    let mut cache = state.ui_state_cache.lock().await;
    if cache.contains_key(profile_id) {
        return;
    }
    if !cache.contains_key(profile_id) && cache.len() >= UI_STATE_CACHE_MAX_ENTRIES {
        if let Some(evicted_profile_id) = cache
            .keys()
            .find(|cached_profile_id| {
                cached_profile_id.as_str() != profile_id
                    && !dirty_profiles.contains(cached_profile_id.as_str())
            })
            .cloned()
        {
            cache.remove(&evicted_profile_id);
        }
    }
    cache.insert(profile_id.to_string(), Arc::new(Mutex::new(ui_state)));
}

async fn write_profile_ui_state(config: &Config, profile_id: &str, ui_state: &Value) -> Result<()> {
    let path = profile_ui_state_path(config, profile_id);
    // This file can grow to several MiB. Pretty-printing it for every status
    // transition dominated gateway CPU without providing runtime value.
    let bytes = serde_json::to_vec(ui_state).context("failed to serialize ui-state")?;
    if let Ok(previous_bytes) = tokio_fs::read(&path).await {
        write_file_atomically(&path.with_extension("json.bak"), previous_bytes)
            .await
            .context("failed to write ui-state backup")?;
    }
    write_file_atomically(&path, bytes)
        .await
        .context("failed to write ui-state file")?;
    Ok(())
}

async fn persist_cached_profile_ui_state(
    state: &AppState,
    profile_id: &str,
    revision: u64,
) -> Result<()> {
    let lock = ui_state_lock(state, profile_id).await;
    let _guard = lock.lock().await;
    ensure_cached_profile_ui_state(state, profile_id).await?;
    let cached = state
        .ui_state_cache
        .lock()
        .await
        .get(profile_id)
        .cloned()
        .ok_or_else(|| anyhow!("cached ui-state disappeared before persistence"))?;
    let snapshot = cached.lock().await.clone();
    write_profile_ui_state(&state.config, profile_id, &snapshot).await?;
    let mut persistence = state.ui_state_persistence.lock().await;
    let entry = persistence.entry(profile_id.to_string()).or_default();
    entry.persisted_revision = entry.persisted_revision.max(revision);
    Ok(())
}

async fn run_ui_state_writer(state: AppState, profile_id: String) {
    tokio::time::sleep(UI_STATE_WRITE_DEBOUNCE).await;
    loop {
        let revision = state
            .ui_state_persistence
            .lock()
            .await
            .get(&profile_id)
            .map(|entry| entry.revision)
            .unwrap_or_default();
        if let Err(error) = persist_cached_profile_ui_state(&state, &profile_id, revision).await {
            warn!(
                profile_id,
                "failed to persist coalesced ui-state update: {error:#}"
            );
            tokio::time::sleep(UI_STATE_WRITE_RETRY_DELAY).await;
            continue;
        }

        let mut persistence = state.ui_state_persistence.lock().await;
        let entry = persistence.entry(profile_id.clone()).or_default();
        if entry.revision == entry.persisted_revision {
            entry.writer_running = false;
            return;
        }
        drop(persistence);
        tokio::time::sleep(UI_STATE_WRITE_DEBOUNCE).await;
    }
}

async fn mark_ui_state_dirty(state: &AppState, profile_id: &str) -> u64 {
    let mut persistence = state.ui_state_persistence.lock().await;
    let entry = persistence.entry(profile_id.to_string()).or_default();
    entry.revision = entry.revision.saturating_add(1);
    entry.revision
}

async fn start_ui_state_writer(state: &AppState, profile_id: &str) {
    let should_spawn = {
        let mut persistence = state.ui_state_persistence.lock().await;
        let entry = persistence.entry(profile_id.to_string()).or_default();
        if entry.writer_running {
            false
        } else {
            entry.writer_running = true;
            true
        }
    };
    if should_spawn {
        tokio::spawn(run_ui_state_writer(state.clone(), profile_id.to_string()));
    }
}

async fn schedule_ui_state_write(state: &AppState, profile_id: &str) -> u64 {
    let revision = mark_ui_state_dirty(state, profile_id).await;
    start_ui_state_writer(state, profile_id).await;
    revision
}

pub(crate) async fn flush_pending_ui_state_writes(state: &AppState) {
    for attempt in 0..3 {
        let dirty_profiles = state
            .ui_state_persistence
            .lock()
            .await
            .iter()
            .filter_map(|(profile_id, entry)| {
                (entry.revision != entry.persisted_revision)
                    .then(|| (profile_id.clone(), entry.revision))
            })
            .collect::<Vec<_>>();
        if dirty_profiles.is_empty() {
            return;
        }
        let mut failed = false;
        for (profile_id, revision) in dirty_profiles {
            if let Err(error) = persist_cached_profile_ui_state(state, &profile_id, revision).await
            {
                failed = true;
                warn!(
                    profile_id,
                    attempt = attempt + 1,
                    "failed to flush ui-state during shutdown: {error:#}"
                );
            }
        }
        if failed {
            tokio::time::sleep(UI_STATE_WRITE_RETRY_DELAY).await;
        }
    }
    warn!("ui-state remained dirty after shutdown flush retries");
}

pub(crate) async fn write_file_atomically(path: &Path, bytes: Vec<u8>) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("state file path has no parent directory"))?;
    tokio_fs::create_dir_all(parent)
        .await
        .context("failed to create state directory")?;
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("state file path has no file name"))?;
    let temp_path = parent.join(format!(
        ".codex-webui-state-{file_name}-{}.tmp",
        Uuid::new_v4()
    ));

    let write_result = async {
        let mut temp_file = tokio_fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)
            .await?;
        temp_file.write_all(&bytes).await?;
        temp_file.sync_all().await?;
        drop(temp_file);
        tokio_fs::rename(&temp_path, path).await?;
        if let Ok(parent_dir) = tokio_fs::File::open(parent).await {
            let _ = parent_dir.sync_all().await;
        }
        std::io::Result::Ok(())
    }
    .await;
    if let Err(error) = write_result {
        let _ = tokio_fs::remove_file(&temp_path).await;
        return Err(error).context("failed to atomically write state file");
    }

    Ok(())
}

fn theme_settings_path(config: &Config, profile_id: &str) -> PathBuf {
    resolve_runtime_profile(config, profile_id)
        .data_dir
        .join("theme-settings.json")
}

pub(crate) async fn read_stored_theme_settings(
    config: &Config,
    profile_id: &str,
) -> Result<Option<Value>> {
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

pub(crate) async fn write_stored_theme_settings(
    config: &Config,
    profile_id: &str,
    theme: &Value,
) -> Result<Value> {
    let path = theme_settings_path(config, profile_id);
    let payload = theme.clone();
    let bytes = serde_json::to_vec_pretty(&payload).context("failed to encode theme settings")?;
    write_file_atomically(&path, bytes)
        .await
        .context("failed to write theme settings")?;
    Ok(payload)
}

pub(crate) async fn with_ui_state_read<R, F>(
    state: &AppState,
    profile_id: &str,
    reader: F,
) -> ApiResult<R>
where
    F: FnOnce(&Value) -> ApiResult<R>,
{
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    let lock = ui_state_lock(state, &resolved_profile_id).await;
    let _guard = lock.lock().await;
    ensure_cached_profile_ui_state(state, &resolved_profile_id)
        .await
        .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    let cached = state
        .ui_state_cache
        .lock()
        .await
        .get(&resolved_profile_id)
        .cloned()
        .ok_or_else(|| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "cached ui-state is missing",
            )
        })?;
    let ui_state = cached.lock().await;
    reader(&ui_state)
}

pub(crate) async fn with_ui_state_write<R, F>(
    state: &AppState,
    profile_id: &str,
    writer: F,
) -> ApiResult<R>
where
    F: FnOnce(&mut Value) -> ApiResult<R>,
{
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    let lock = ui_state_lock(state, &resolved_profile_id).await;
    let _guard = lock.lock().await;
    ensure_cached_profile_ui_state(state, &resolved_profile_id)
        .await
        .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    let cached = state
        .ui_state_cache
        .lock()
        .await
        .get(&resolved_profile_id)
        .cloned()
        .ok_or_else(|| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "cached ui-state is missing",
            )
        })?;
    let (result, snapshot) = {
        let mut ui_state = cached.lock().await;
        let result = writer(&mut ui_state);
        let snapshot = result.as_ref().ok().map(|_| ui_state.clone());
        (result, snapshot)
    };
    let Some(snapshot) = snapshot else {
        schedule_ui_state_write(state, &resolved_profile_id).await;
        return result;
    };
    let result = result?;
    let revision = mark_ui_state_dirty(state, &resolved_profile_id).await;
    if let Err(error) = write_profile_ui_state(&state.config, &resolved_profile_id, &snapshot).await
    {
        start_ui_state_writer(state, &resolved_profile_id).await;
        return Err(api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            error.to_string(),
        ));
    }
    let mut persistence = state.ui_state_persistence.lock().await;
    let entry = persistence.entry(resolved_profile_id).or_default();
    entry.persisted_revision = entry.persisted_revision.max(revision);
    Ok(result)
}

pub(crate) async fn with_ui_state_write_debounced<R, F>(
    state: &AppState,
    profile_id: &str,
    writer: F,
) -> ApiResult<R>
where
    F: FnOnce(&mut Value) -> ApiResult<(R, bool)>,
{
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    let lock = ui_state_lock(state, &resolved_profile_id).await;
    let _guard = lock.lock().await;
    ensure_cached_profile_ui_state(state, &resolved_profile_id)
        .await
        .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    let cached = state
        .ui_state_cache
        .lock()
        .await
        .get(&resolved_profile_id)
        .cloned()
        .ok_or_else(|| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "cached ui-state is missing",
            )
        })?;
    let result = {
        let mut ui_state = cached.lock().await;
        writer(&mut ui_state)
    };
    let (result, changed) = match result {
        Ok(result) => result,
        Err(error) => {
            schedule_ui_state_write(state, &resolved_profile_id).await;
            return Err(error);
        }
    };
    if changed {
        schedule_ui_state_write(state, &resolved_profile_id).await;
    }
    Ok(result)
}

pub(crate) fn known_tags_from_ui_state(ui_state: &Value) -> Vec<String> {
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

pub(crate) fn session_folders_from_ui_state(ui_state: &Value) -> Vec<Value> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    if let Some(entries) = ui_state
        .get("sessionMetaByThreadId")
        .and_then(Value::as_object)
    {
        for tags in entries
            .values()
            .filter_map(Value::as_object)
            .filter_map(|entry| entry.get("tags"))
            .filter_map(Value::as_array)
        {
            for tag in tags.iter().filter_map(Value::as_str) {
                let trimmed = tag.trim();
                if !trimmed.is_empty() {
                    *counts.entry(trimmed.to_string()).or_insert(0) += 1;
                }
            }
        }
    }

    let mut folders: HashMap<String, Value> = HashMap::new();
    if let Some(entries) = ui_state
        .get("sessionFoldersByName")
        .and_then(Value::as_object)
    {
        for (key, entry) in entries {
            let name = entry
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(key)
                .trim();
            if name.is_empty() {
                continue;
            }
            folders.insert(
                name.to_string(),
                json!({
                    "name": name,
                    "pinned": entry.get("pinned").and_then(Value::as_bool).unwrap_or(false),
                    "sessionCount": counts.get(name).copied().unwrap_or(0),
                    "createdAt": entry.get("createdAt").cloned().unwrap_or(Value::Null),
                    "updatedAt": entry.get("updatedAt").cloned().unwrap_or(Value::Null)
                }),
            );
        }
    }

    for (name, count) in counts {
        folders.entry(name.clone()).or_insert_with(|| {
            json!({
                "name": name,
                "pinned": false,
                "sessionCount": count,
                "createdAt": Value::Null,
                "updatedAt": Value::Null
            })
        });
    }

    let mut values = folders.into_values().collect::<Vec<_>>();
    values.sort_by(|left, right| {
        let left_pinned = left.get("pinned").and_then(Value::as_bool).unwrap_or(false);
        let right_pinned = right
            .get("pinned")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        right_pinned.cmp(&left_pinned).then_with(|| {
            left.get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_lowercase()
                .cmp(
                    &right
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_lowercase(),
                )
        })
    });
    values
}

pub(crate) fn session_selected_skills_from_ui_state(
    ui_state: &Value,
    session_id: &str,
) -> Vec<Value> {
    ui_state
        .get("skillsByThreadId")
        .and_then(Value::as_object)
        .and_then(|entries| entries.get(session_id))
        .map(|value| selected_skills_from_value(Some(value)))
        .unwrap_or_default()
}

pub(crate) fn sorted_prompt_presets_from_ui_state(ui_state: &Value) -> Vec<Value> {
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

pub(crate) fn sorted_automations_from_ui_state(ui_state: &Value) -> Vec<Value> {
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

pub(crate) fn recent_automation_runs_from_ui_state(ui_state: &Value, limit: usize) -> Vec<Value> {
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
