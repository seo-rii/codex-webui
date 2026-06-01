use super::*;

pub(crate) async fn memory_status_payload(
    state: &AppState,
    profile_id: &str,
    session_id: Option<&str>,
) -> ApiResult<Value> {
    let profile = resolve_runtime_profile(&state.config, profile_id);
    let profile_label = profile.label.clone();
    let codex_home = profile.codex_home.clone();
    let memory_root = codex_home.join("memories");
    let config_path = config_toml_path(&codex_home);
    let defaults_codex_home = codex_home.clone();
    let defaults =
        tokio::task::spawn_blocking(move || read_codex_toml_defaults(&defaults_codex_home))
            .await
            .unwrap_or_default();
    let scan_root = memory_root.clone();
    let scan = tokio::task::spawn_blocking(move || {
        let exists = scan_root.exists();
        let mut file_count: u64 = 0;
        let mut directory_count: u64 = 0;
        let mut total_bytes: u64 = 0;
        let mut latest_modified_ms: Option<u64> = None;
        let mut stack = if exists {
            vec![scan_root.clone()]
        } else {
            Vec::new()
        };
        while let Some(path) = stack.pop() {
            let Ok(entries) = fs::read_dir(&path) else {
                continue;
            };
            for entry in entries.flatten() {
                let entry_path = entry.path();
                let Ok(metadata) = entry.metadata() else {
                    continue;
                };
                let modified_ms = metadata
                    .modified()
                    .ok()
                    .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                    .map(|duration| duration.as_millis() as u64);
                if let Some(modified_ms) = modified_ms {
                    latest_modified_ms = Some(latest_modified_ms.unwrap_or(0).max(modified_ms));
                }
                if metadata.is_dir() {
                    directory_count += 1;
                    stack.push(entry_path);
                } else if metadata.is_file() {
                    file_count += 1;
                    total_bytes = total_bytes.saturating_add(metadata.len());
                }
            }
        }
        json!({
            "exists": exists,
            "fileCount": file_count,
            "directoryCount": directory_count,
            "totalBytes": total_bytes,
            "latestModifiedAt": latest_modified_ms
        })
    })
    .await
    .unwrap_or_else(|_| {
        json!({
            "exists": false,
            "fileCount": 0,
            "directoryCount": 0,
            "totalBytes": 0,
            "latestModifiedAt": Value::Null
        })
    });

    Ok(json!({
        "profileId": resolve_runtime_profile_entry(&state.config, profile_id).0,
        "profileLabel": profile_label,
        "paths": {
            "codexHome": codex_home.display().to_string(),
            "configFilePath": config_path.display().to_string(),
            "memoryRoot": memory_root.display().to_string()
        },
        "storage": scan,
        "settings": {
            "disableOnExternalContext": defaults.memories_disable_on_external_context.unwrap_or(false),
            "generateMemories": defaults.memories_generate_memories.unwrap_or(true),
            "useMemories": defaults.memories_use_memories.unwrap_or(true),
            "maxRawMemoriesForConsolidation": defaults.memories_max_raw_memories_for_consolidation.unwrap_or(256),
            "maxUnusedDays": defaults.memories_max_unused_days.unwrap_or(30),
            "maxRolloutAgeDays": defaults.memories_max_rollout_age_days.unwrap_or(10),
            "maxRolloutsPerStartup": defaults.memories_max_rollouts_per_startup.unwrap_or(2),
            "minRolloutIdleHours": defaults.memories_min_rollout_idle_hours.unwrap_or(6),
            "minRateLimitRemainingPercent": defaults.memories_min_rate_limit_remaining_percent.unwrap_or(25),
            "extractModel": defaults.memories_extract_model.clone().map(Value::String).unwrap_or(Value::Null),
            "consolidationModel": defaults.memories_consolidation_model.clone().map(Value::String).unwrap_or(Value::Null),
            "configured": {
                "disableOnExternalContext": defaults.memories_disable_on_external_context.map(Value::Bool).unwrap_or(Value::Null),
                "generateMemories": defaults.memories_generate_memories.map(Value::Bool).unwrap_or(Value::Null),
                "useMemories": defaults.memories_use_memories.map(Value::Bool).unwrap_or(Value::Null),
                "maxRawMemoriesForConsolidation": defaults.memories_max_raw_memories_for_consolidation.map(Value::from).unwrap_or(Value::Null),
                "maxUnusedDays": defaults.memories_max_unused_days.map(Value::from).unwrap_or(Value::Null),
                "maxRolloutAgeDays": defaults.memories_max_rollout_age_days.map(Value::from).unwrap_or(Value::Null),
                "maxRolloutsPerStartup": defaults.memories_max_rollouts_per_startup.map(Value::from).unwrap_or(Value::Null),
                "minRolloutIdleHours": defaults.memories_min_rollout_idle_hours.map(Value::from).unwrap_or(Value::Null),
                "minRateLimitRemainingPercent": defaults.memories_min_rate_limit_remaining_percent.map(Value::from).unwrap_or(Value::Null),
                "extractModel": defaults.memories_extract_model.map(Value::String).unwrap_or(Value::Null),
                "consolidationModel": defaults.memories_consolidation_model.map(Value::String).unwrap_or(Value::Null)
            }
        },
        "selectedSession": session_id
            .filter(|session_id| !session_id.trim().is_empty())
            .map(|session_id| {
                json!({
                    "sessionId": session_id,
                    "memoryMode": Value::Null,
                    "modeSource": "notExposedByThreadRead"
                })
            })
            .unwrap_or(Value::Null)
    }))
}

pub(crate) async fn reset_memory_payload(state: &AppState, profile_id: &str) -> ApiResult<Value> {
    let client = app_server_client(state, profile_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to codex app-server: {error}"),
            )
        })?;
    client
        .request("memory/reset", Value::Null)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to reset Codex memory: {error}"),
            )
        })?;
    let status = memory_status_payload(state, profile_id, None).await?;
    Ok(json!({
        "ok": true,
        "memory": status
    }))
}

pub(crate) async fn set_session_memory_mode_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    mode: &str,
) -> ApiResult<Value> {
    let normalized_mode = match mode {
        "enabled" | "disabled" => mode,
        _ => {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "Invalid memory mode. Use enabled or disabled.",
            ));
        }
    };
    let client = app_server_client_for_session_turn(state, profile_id, session_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to codex app-server: {error}"),
            )
        })?;
    client
        .request(
            "thread/memoryMode/set",
            json!({
                "threadId": session_id,
                "mode": normalized_mode
            }),
        )
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to update Codex thread memory mode: {error}"),
            )
        })?;
    Ok(json!({
        "ok": true,
        "sessionId": session_id,
        "memoryMode": normalized_mode
    }))
}
