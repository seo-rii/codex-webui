use super::*;
use crate::thread_read_support::read_thread_payload;
use crate::turn_execution_support::send_turn_payload;

pub(crate) fn arena_store_path(config: &Config, profile_id: &str) -> PathBuf {
    resolve_runtime_profile(config, profile_id)
        .data_dir
        .join("arena-runs.json")
}

async fn read_arena_store_state(state: &AppState, profile_id: &str) -> Result<ArenaStoreState> {
    let _guard = ui_state_lock(state, profile_id).await.lock_owned().await;
    let path = arena_store_path(&state.config, profile_id);
    match tokio_fs::read_to_string(&path).await {
        Ok(raw) => match serde_json::from_str::<ArenaStoreState>(&raw) {
            Ok(parsed) => Ok(parsed),
            Err(_) => {
                let empty = ArenaStoreState::default();
                write_file_atomically(
                    &path,
                    serde_json::to_vec_pretty(&empty).unwrap_or_else(|_| b"{\"runs\":[]}".to_vec()),
                )
                .await
                .ok();
                Ok(empty)
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(ArenaStoreState::default())
        }
        Err(error) => Err(error).context("failed to read arena store"),
    }
}

async fn write_arena_store_state(
    state: &AppState,
    profile_id: &str,
    arena_state: &ArenaStoreState,
) -> Result<()> {
    let _guard = ui_state_lock(state, profile_id).await.lock_owned().await;
    let path = arena_store_path(&state.config, profile_id);
    let bytes =
        serde_json::to_vec_pretty(arena_state).context("failed to encode arena store state")?;
    write_file_atomically(&path, bytes)
        .await
        .context("failed to write arena store state")
}

fn extract_arena_response(turns: &[Value]) -> Option<String> {
    for turn in turns.iter().rev() {
        let Some(items) = turn.get("items").and_then(Value::as_array) else {
            continue;
        };
        for item in items.iter().rev() {
            if item.get("type").and_then(Value::as_str) != Some("agentMessage") {
                continue;
            }
            if let Some(text) = item.get("text").and_then(value_text) {
                return Some(text);
            }
        }
    }
    None
}

pub(crate) async fn list_arena_runs_payload(
    state: &AppState,
    profile_id: &str,
) -> ApiResult<Value> {
    let mut arena_state = read_arena_store_state(state, profile_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to read arena runs: {error}"),
            )
        })?;
    let mut changed = false;

    for run in &mut arena_state.runs {
        for contestant in &mut run.contestants {
            let thread = read_thread_payload(state, profile_id, &contestant.session_id, true).await;
            let Ok(thread) = thread else {
                continue;
            };
            let Some(thread) = thread.as_object() else {
                continue;
            };
            let status = normalized_thread_status(thread.get("status"))
                .unwrap_or_else(|| contestant.status.clone());
            let mut response = contestant.response.clone();
            if response.is_none() && !is_live_thread_status(&status) {
                if let Some(turns) = thread.get("turns").and_then(Value::as_array) {
                    response = extract_arena_response(turns);
                }
            }
            let updated_at = contestant.updated_at.max(
                thread
                    .get("updatedAt")
                    .and_then(Value::as_u64)
                    .unwrap_or(contestant.updated_at),
            );
            if status != contestant.status
                || response != contestant.response
                || updated_at != contestant.updated_at
            {
                contestant.status = status;
                contestant.response = response;
                contestant.updated_at = updated_at;
                changed = true;
            }
        }

        let next_status = if run
            .contestants
            .iter()
            .any(|contestant| is_live_thread_status(&contestant.status))
        {
            "running".to_string()
        } else {
            "completed".to_string()
        };
        let next_updated_at = run
            .contestants
            .iter()
            .map(|contestant| contestant.updated_at)
            .max()
            .unwrap_or(run.updated_at)
            .max(run.updated_at);
        if run.status != next_status || run.updated_at != next_updated_at {
            run.status = next_status;
            run.updated_at = next_updated_at;
            changed = true;
        }
    }

    arena_state
        .runs
        .sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    if changed {
        write_arena_store_state(state, profile_id, &arena_state)
            .await
            .map_err(|error| {
                api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to persist hydrated arena runs: {error}"),
                )
            })?;
    }

    Ok(json!({
        "runs": arena_state.runs
    }))
}

pub(crate) async fn start_arena_run_payload(
    state: &AppState,
    profile_id: &str,
    prompt: &str,
    contestants: &Value,
    preferences: &Value,
) -> ApiResult<Value> {
    let trimmed_prompt = prompt.trim();
    if trimmed_prompt.is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "Prompt is required."));
    }

    let mut normalized_contestants = Vec::<(String, String)>::new();
    let mut seen_models = std::collections::HashSet::new();
    for contestant in contestants
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .take(8)
    {
        let model = contestant
            .get("model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_default();
        let label = contestant
            .get("label")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| model.clone());
        if model.is_empty() || label.is_empty() || !seen_models.insert(model.clone()) {
            continue;
        }
        normalized_contestants.push((model, label));
        if normalized_contestants.len() >= 4 {
            break;
        }
    }

    if normalized_contestants.len() < 2 {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Choose at least two models for an arena run.",
        ));
    }

    let config_payload = get_config_payload(state, profile_id).await?;
    let mut base_preferences = config_payload
        .get("defaults")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(overrides) = preferences.as_object() {
        for (key, value) in overrides {
            if !value.is_null() {
                base_preferences.insert(key.clone(), value.clone());
            }
        }
    }

    let created_at = now_unix_ms();
    let title_source = trimmed_prompt
        .split('\n')
        .next()
        .unwrap_or(trimmed_prompt)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let title = if title_source.is_empty() {
        "Arena run".to_string()
    } else {
        title_source.chars().take(60).collect::<String>()
    };
    let client = app_server_client(state, profile_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to codex app-server: {error}"),
            )
        })?;
    let mut arena_contestants = Vec::new();

    for (model, label) in &normalized_contestants {
        let mut session_preferences = base_preferences.clone();
        session_preferences.insert("model".to_string(), Value::String(model.clone()));
        let cwd = session_preferences
            .get("cwd")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                api_error(
                    StatusCode::BAD_REQUEST,
                    "A working directory is required to start an arena run.",
                )
            })?;

        let response = client
            .request(
                "thread/start",
                json!({
                    "model": session_preferences.get("model").cloned().unwrap_or(Value::Null),
                    "cwd": cwd,
                    "approvalPolicy": session_preferences.get("approvalPolicy").cloned().unwrap_or_else(|| json!("on-request")),
                    "sandbox": session_preferences.get("sandboxMode").cloned().unwrap_or_else(|| json!("workspace-write")),
                    "personality": session_preferences.get("personality").cloned().unwrap_or(Value::Null),
                    "config": preferences_model_context_config(&Value::Object(session_preferences.clone())),
                    "serviceTier": match session_preferences.get("speed").and_then(Value::as_str) {
                        Some("fast") => Value::String("fast".to_string()),
                        Some("flex") => Value::String("flex".to_string()),
                        _ => Value::Null
                    },
                    "experimentalRawEvents": false,
                    "persistExtendedHistory": true
                }),
            )
            .await
            .map_err(|error| {
                api_error(
                    StatusCode::BAD_GATEWAY,
                    format!("Failed to create an arena session: {error}"),
                )
            })?;
        let session_id = response
            .get("thread")
            .and_then(|thread| thread.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                api_error(
                    StatusCode::BAD_GATEWAY,
                    "Codex app-server returned an invalid arena session payload.",
                )
            })?;

        with_ui_state_write(state, profile_id, |ui_state| {
            let Some(preferences_by_thread_id) = ui_state
                .get_mut("preferencesByThreadId")
                .and_then(Value::as_object_mut)
            else {
                return Err(api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "preferences state is missing",
                ));
            };
            preferences_by_thread_id.insert(
                session_id.clone(),
                Value::Object(session_preferences.clone()),
            );
            Ok(())
        })
        .await?;

        let thread_name = format!("Arena · {} · {}", title, label)
            .chars()
            .take(120)
            .collect::<String>();
        client
            .request(
                "thread/name/set",
                json!({
                    "threadId": session_id,
                    "name": thread_name
                }),
            )
            .await
            .map_err(|error| {
                api_error(
                    StatusCode::BAD_GATEWAY,
                    format!("Failed to name an arena session: {error}"),
                )
            })?;

        arena_contestants.push(ArenaContestantRecord {
            id: Uuid::new_v4().to_string(),
            session_id,
            model: model.clone(),
            label: label.clone(),
            status: "running".to_string(),
            response: None,
            created_at,
            updated_at: created_at,
        });
    }

    let run = ArenaRunRecord {
        id: Uuid::new_v4().to_string(),
        prompt: trimmed_prompt.to_string(),
        cwd: base_preferences
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        status: "running".to_string(),
        created_at,
        updated_at: created_at,
        contestants: arena_contestants.clone(),
    };

    let mut arena_state = read_arena_store_state(state, profile_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to read arena runs: {error}"),
            )
        })?;
    arena_state.runs.retain(|entry| entry.id != run.id);
    arena_state.runs.insert(0, run.clone());
    arena_state.runs.truncate(60);
    write_arena_store_state(state, profile_id, &arena_state)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to persist arena runs: {error}"),
            )
        })?;

    for contestant in &arena_contestants {
        let mut session_preferences = base_preferences.clone();
        session_preferences.insert("model".to_string(), Value::String(contestant.model.clone()));
        let send_result = send_turn_payload(
            state,
            profile_id,
            &contestant.session_id,
            trimmed_prompt,
            Some(&json!([])),
            None,
            Value::Object(session_preferences),
        )
        .await;

        if let Err(error) = send_result {
            let mut current_state =
                read_arena_store_state(state, profile_id)
                    .await
                    .map_err(|read_error| {
                        api_error(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!(
                                "Failed to refresh arena runs after send failure: {read_error}"
                            ),
                        )
                    })?;
            if let Some(current_run) = current_state
                .runs
                .iter_mut()
                .find(|entry| entry.id == run.id)
            {
                current_run.updated_at = now_unix_ms();
                if let Some(current_contestant) = current_run
                    .contestants
                    .iter_mut()
                    .find(|entry| entry.id == contestant.id)
                {
                    current_contestant.status = "failed".to_string();
                    current_contestant.response = Some(error.to_string());
                    current_contestant.updated_at = current_run.updated_at;
                }
            }
            write_arena_store_state(state, profile_id, &current_state)
                .await
                .map_err(|write_error| {
                    api_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Failed to persist arena send failure: {write_error}"),
                    )
                })?;
        }
    }

    let payload = list_arena_runs_payload(state, profile_id).await?;
    let matching_run = payload
        .get("runs")
        .and_then(Value::as_array)
        .and_then(|runs| {
            runs.iter()
                .find(|entry| entry.get("id").and_then(Value::as_str) == Some(run.id.as_str()))
        })
        .cloned()
        .unwrap_or_else(|| serde_json::to_value(&run).unwrap_or(Value::Null));
    Ok(matching_run)
}
