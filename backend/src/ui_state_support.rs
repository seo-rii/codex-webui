use super::*;

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
        "global": {
            "shutdownAfterQueueCompletes": false,
            "shutdownAfterQueueCompletesPrimed": false,
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
        "skillsByThreadId": {},
        "draftsByThreadId": {},
        "queuesByThreadId": {},
        "highlightsByThreadId": {}
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

    if !root.get("global").is_some_and(Value::is_object) {
        root.insert(
            "global".to_string(),
            json!({
                "shutdownAfterQueueCompletes": false,
                "shutdownAfterQueueCompletesPrimed": false,
                "scheduledShutdown": Value::Null
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
    }

    if !root.get("notifications").is_some_and(Value::is_object) {
        root.insert(
            "notifications".to_string(),
            json!({
                "items": [],
                "settings": default_notification_settings_value()
            }),
        );
    }

    if let Some(notifications) = root.get_mut("notifications").and_then(Value::as_object_mut) {
        if !notifications.get("items").is_some_and(Value::is_array) {
            notifications.insert("items".to_string(), json!([]));
        }
        let normalized_settings =
            normalize_notification_settings_value(notifications.get("settings"));
        notifications.insert("settings".to_string(), normalized_settings);
    }

    for (key, default_value) in [
        ("sessionMetaByThreadId", json!({})),
        ("savedSessionFilters", json!([])),
        ("promptPresets", json!([])),
        ("automations", json!([])),
        ("automationRuns", json!([])),
        ("preferencesByThreadId", json!({})),
        ("skillsByThreadId", json!({})),
        ("draftsByThreadId", json!({})),
        ("queuesByThreadId", json!({})),
        ("highlightsByThreadId", json!({})),
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
            let fallback = default_ui_state_value();
            tokio_fs::write(
                &path,
                serde_json::to_vec_pretty(&fallback).expect("default ui-state should serialize"),
            )
            .await
            .context("failed to recreate ui-state file after corruption")?;
            Ok(fallback)
        }
    }
}

async fn write_profile_ui_state(config: &Config, profile_id: &str, ui_state: &Value) -> Result<()> {
    let path = profile_ui_state_path(config, profile_id);
    if let Some(parent) = path.parent() {
        tokio_fs::create_dir_all(parent)
            .await
            .context("failed to create profile data directory")?;
    }
    let bytes = serde_json::to_vec_pretty(ui_state).context("failed to serialize ui-state")?;
    tokio_fs::write(&path, bytes)
        .await
        .context("failed to write ui-state file")?;
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
    if let Some(parent) = path.parent() {
        tokio_fs::create_dir_all(parent)
            .await
            .context("failed to create theme settings directory")?;
    }
    let payload = theme.clone();
    let bytes = serde_json::to_vec_pretty(&payload).context("failed to encode theme settings")?;
    tokio_fs::write(&path, bytes)
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
    let ui_state = read_profile_ui_state(&state.config, &resolved_profile_id)
        .await
        .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
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
    let mut ui_state = read_profile_ui_state(&state.config, &resolved_profile_id)
        .await
        .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    let result = writer(&mut ui_state)?;
    write_profile_ui_state(&state.config, &resolved_profile_id, &ui_state)
        .await
        .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
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
