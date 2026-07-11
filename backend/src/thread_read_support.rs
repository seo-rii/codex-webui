use super::*;

pub(crate) fn is_unmaterialized_thread_error_message(message: &str) -> bool {
    let lowered = message.to_ascii_lowercase();
    lowered.contains("not materialized yet")
        || lowered.contains("includeturns is unavailable before first user message")
        || lowered.contains("thread not loaded")
        || lowered.contains("no rollout found for thread id")
}

pub(crate) fn thread_agent_nickname(thread: &Value) -> Option<String> {
    thread
        .get("agentNickname")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            thread
                .get("agent_nickname")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
        .or_else(|| {
            thread
                .get("source")
                .and_then(|value| value.get("subagent"))
                .and_then(|value| value.get("thread_spawn"))
                .and_then(|value| value.get("agent_nickname"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
}

pub(crate) fn thread_agent_role(thread: &Value) -> Option<String> {
    thread
        .get("agentRole")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            thread
                .get("agent_role")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
        .or_else(|| {
            thread
                .get("source")
                .and_then(|value| value.get("subagent"))
                .and_then(|value| value.get("thread_spawn"))
                .and_then(|value| value.get("agent_role"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
}

pub(crate) fn thread_source_marks_subagent(source: &Value) -> bool {
    if let Some(subagent) = source.get("subagent") {
        match subagent {
            Value::Object(value) if !value.is_empty() => return true,
            Value::String(value) if !value.trim().is_empty() => return true,
            _ => {}
        }
    }

    let Some(source_text) = source
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return false;
    };
    let source_text = source_text.to_ascii_lowercase();
    // Codex keeps non-interactive worker sessions out of the default thread list.
    // The web UI maps those to the existing hidden-subagent path for compatibility.
    source_text == "exec"
        || source_text == "subagent"
        || source_text.starts_with("subagent_")
        || source_text.starts_with("internal_")
        || source_text == "memory_consolidation"
}

pub(crate) fn thread_is_subagent(thread: &Value) -> bool {
    thread
        .get("isSubagent")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || thread
            .get("spawnedSubagent")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        || thread
            .get("spawned_subagent")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        || thread
            .get("spawned_subagent")
            .and_then(Value::as_i64)
            .is_some_and(|value| value != 0)
        || thread
            .get("source")
            .is_some_and(thread_source_marks_subagent)
        || thread_agent_nickname(thread).is_some()
        || thread_agent_role(thread).is_some()
}

fn normalize_session_item_type_name(item_type: &str) -> &str {
    match item_type {
        "agent_message" | "assistant_message" | "assistantMessage" => "agentMessage",
        "user_message" => "userMessage",
        "command_execution" => "commandExecution",
        "file_change" => "fileChange",
        "mcp_tool_call" => "mcpToolCall",
        "dynamic_tool_call" => "dynamicToolCall",
        "web_search" => "webSearch",
        "image_view" => "imageView",
        "context_compaction" => "contextCompaction",
        "image_generation" => "imageGeneration",
        "collab_agent_tool_call" => "collabAgentToolCall",
        "entered_review_mode" => "enteredReviewMode",
        "exited_review_mode" => "exitedReviewMode",
        _ => item_type,
    }
}

pub(crate) fn is_internal_session_item_type(item_type: &str) -> bool {
    matches!(
        item_type,
        "task_complete"
            | "turn_aborted"
            | "turn_started"
            | "turn_completed"
            | "agent_reasoning_section_break"
    )
}

pub(crate) const EMPTY_ASSISTANT_RESPONSE_CODE: &str = "EMPTY_ASSISTANT_RESPONSE";

pub(crate) fn session_turn_has_visible_agent_output(turn: &Value) -> bool {
    turn.get("items")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|item| {
                let item_type = item
                    .get("type")
                    .and_then(Value::as_str)
                    .map(normalize_session_item_type_name)
                    .unwrap_or("unknown");
                item_type != "userMessage" && !is_internal_session_item_type(item_type)
            })
        })
}

fn turn_error_message(turn: &Value) -> Option<String> {
    turn.get("error")
        .and_then(|error| {
            error
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| error.as_str())
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub(crate) fn mark_turn_without_agent_output_failed(
    turn: &mut Value,
    item_id_suffix: impl AsRef<str>,
) -> bool {
    if session_turn_has_visible_agent_output(turn) {
        return false;
    }
    let status = turn.get("status").and_then(Value::as_str);
    let existing_error_message = turn_error_message(turn);
    if !matches!(
        status,
        Some("completed" | "done" | "success" | "failed" | "error" | "systemError")
    ) || (existing_error_message.is_none()
        && !matches!(status, Some("completed" | "done" | "success")))
    {
        return false;
    }
    let Some(message) = existing_error_message else {
        // `turn/completed` notifications intentionally omit authoritative history in
        // current Codex builds. An empty completed turn is hydrated via thread/read;
        // only an explicit Codex error is enough evidence to synthesize a failure.
        return false;
    };

    let turn_id = turn
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("turn")
        .to_string();
    let error_payload = turn
        .get("error")
        .filter(|value| !value.is_null())
        .cloned()
        .unwrap_or_else(|| {
            json!({
                "code": EMPTY_ASSISTANT_RESPONSE_CODE,
                "message": message
            })
        });
    if let Some(turn_object) = turn.as_object_mut() {
        turn_object.insert("status".to_string(), Value::String("failed".to_string()));
        turn_object.insert("error".to_string(), error_payload);
        let items = turn_object
            .entry("items".to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Some(items) = items.as_array_mut() {
            items.push(json!({
                "id": format!("{}:empty-assistant-response:{}", turn_id, item_id_suffix.as_ref()),
                "type": "agentMessage",
                "text": message,
                "phase": "final_answer",
                "status": "failed"
            }));
        }
        return true;
    }

    false
}

pub(crate) fn normalize_session_item_payload(
    item: &Value,
    turn_id: &str,
    item_index: usize,
) -> Value {
    let mut normalized = item.as_object().cloned().unwrap_or_default();
    if normalized
        .get("id")
        .and_then(Value::as_str)
        .is_none_or(|value| value.trim().is_empty())
    {
        normalized.insert(
            "id".to_string(),
            Value::String(format!("{turn_id}:item:{item_index}")),
        );
    }
    let normalized_type = normalized
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(normalize_session_item_type_name)
        .unwrap_or("unknown");
    normalized.insert(
        "type".to_string(),
        Value::String(normalized_type.to_string()),
    );
    Value::Object(normalized)
}

pub(crate) fn normalize_session_turn_payload(turn: &Value, turn_index: usize) -> Value {
    let mut normalized = turn.as_object().cloned().unwrap_or_default();
    let turn_id = normalized
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("turn-{turn_index}"));
    let items = normalized
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .enumerate()
        .filter_map(|(item_index, item)| {
            let normalized_item = normalize_session_item_payload(item, &turn_id, item_index);
            let item_type = normalized_item
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            (!is_internal_session_item_type(item_type)).then_some(normalized_item)
        })
        .collect::<Vec<_>>();
    normalized.insert("id".to_string(), Value::String(turn_id));
    normalized.insert("items".to_string(), Value::Array(items));
    normalized.insert(
        "status".to_string(),
        Value::String(
            value_text(normalized.get("status").unwrap_or(&Value::Null))
                .unwrap_or_else(|| "unknown".to_string()),
        ),
    );
    normalized
        .entry("error".to_string())
        .or_insert_with(|| Value::Null);
    normalized
        .entry("startedAt".to_string())
        .or_insert_with(|| Value::Null);
    normalized
        .entry("completedAt".to_string())
        .or_insert_with(|| Value::Null);
    normalized
        .entry("durationMs".to_string())
        .or_insert_with(|| Value::Null);
    normalized.insert("detailState".to_string(), Value::String("full".to_string()));
    normalized.insert("hiddenItemCount".to_string(), Value::from(0));
    Value::Object(normalized)
}

fn normalize_thread_payload(thread: &Value) -> Value {
    let mut normalized = thread.as_object().cloned().unwrap_or_default();
    let turns = normalized
        .get("turns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .enumerate()
        .map(|(turn_index, turn)| normalize_session_turn_payload(turn, turn_index))
        .collect::<Vec<_>>();
    let preview = normalized
        .get("preview")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    normalized.insert(
        "name".to_string(),
        normalized
            .get("name")
            .and_then(Value::as_str)
            .map(|value| Value::String(value.to_string()))
            .unwrap_or(Value::Null),
    );
    normalized.insert("preview".to_string(), Value::String(preview));
    normalized.insert(
        "status".to_string(),
        Value::String(
            normalized_thread_status(normalized.get("status"))
                .unwrap_or_else(|| "unknown".to_string()),
        ),
    );
    normalized.insert("turns".to_string(), Value::Array(turns));
    normalized.insert(
        "isSubagent".to_string(),
        Value::Bool(thread_is_subagent(thread)),
    );
    normalized.insert(
        "agentNickname".to_string(),
        thread_agent_nickname(thread)
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    normalized.insert(
        "agentRole".to_string(),
        thread_agent_role(thread)
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    Value::Object(normalized)
}

pub(crate) fn active_turn_id_from_turns(turns: &[Value]) -> Option<String> {
    turns.iter().rev().find_map(|turn| {
        (turn.get("status").and_then(Value::as_str) == Some("inProgress"))
            .then(|| {
                turn.get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_default()
            })
            .filter(|value| !value.is_empty())
    })
}

pub(crate) async fn list_session_attachments_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<Vec<Value>> {
    let attachments = list_session_attachment_records(state, profile_id, session_id)
        .await
        .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    Ok(attachments
        .iter()
        .map(attachment_payload_from_record)
        .collect())
}

pub(crate) async fn read_thread_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    include_turns: bool,
) -> ApiResult<Value> {
    let client = app_server_client_for_session(state, profile_id, session_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to codex app-server: {error}"),
            )
        })?;
    let response = match client
        .request(
            "thread/read",
            json!({
                "threadId": session_id,
                "includeTurns": include_turns
            }),
        )
        .await
    {
        Ok(response) => response,
        Err(error)
            if include_turns && is_unmaterialized_thread_error_message(&error.to_string()) =>
        {
            client
                .request(
                    "thread/read",
                    json!({
                        "threadId": session_id,
                        "includeTurns": false
                    }),
                )
                .await
                .map_err(|fallback_error| {
                    api_error(
                        StatusCode::BAD_GATEWAY,
                        format!("Failed to read the session: {fallback_error}"),
                    )
                })?
        }
        Err(error) => {
            return Err(api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to read the session: {error}"),
            ));
        }
    };
    let thread = response.get("thread").cloned().ok_or_else(|| {
        api_error(
            StatusCode::BAD_GATEWAY,
            "Codex app-server returned an invalid thread payload.",
        )
    })?;
    Ok(normalize_thread_payload(&thread))
}

pub(crate) async fn read_thread_metadata_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<Value> {
    match read_thread_payload(state, profile_id, session_id, false).await {
        Ok(thread) => Ok(thread),
        Err(read_error) => {
            if let Some(thread) =
                read_state_thread_metadata_by_session_id(state, profile_id, session_id, None)
                    .await?
            {
                return Ok(thread);
            }
            if let Some(thread) =
                read_rollout_thread_metadata_by_session_id(state, profile_id, session_id).await?
            {
                return Ok(thread);
            }

            let client = match app_server_client(state, profile_id).await {
                Ok(client) => client,
                Err(_) => return Err(read_error),
            };

            for archived in [false, true] {
                let mut cursor: Option<String> = None;
                loop {
                    let response = match client
                        .request(
                            "thread/list",
                            json!({
                                "limit": 200,
                                "archived": archived,
                                "cursor": cursor.clone()
                            }),
                        )
                        .await
                    {
                        Ok(response) => response,
                        Err(_) => return Err(read_error),
                    };
                    let batch = response
                        .get("data")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    if let Some(thread) = batch
                        .iter()
                        .find(|thread| thread.get("id").and_then(Value::as_str) == Some(session_id))
                    {
                        return Ok(normalize_thread_payload(thread));
                    }

                    cursor = response
                        .get("nextCursor")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string);
                    if cursor.is_none() {
                        break;
                    }
                }
            }

            Err(read_error)
        }
    }
}

pub(crate) async fn read_local_thread_metadata_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<Option<Value>> {
    if let Some(thread) =
        read_state_thread_metadata_by_session_id(state, profile_id, session_id, None).await?
    {
        return Ok(Some(thread));
    }
    read_rollout_thread_metadata_by_session_id(state, profile_id, session_id).await
}

pub(crate) async fn resolve_session_profile_id(
    state: &AppState,
    requested_profile_id: &str,
    session_id: &str,
) -> String {
    let requested_runtime_key = runtime_session_key(requested_profile_id, session_id);
    let requested_has_assignment = state
        .session_app_server_assignments
        .lock()
        .await
        .contains_key(&requested_runtime_key);
    if requested_has_assignment {
        return requested_profile_id.to_string();
    }
    if let Some(thread) =
        read_local_thread_metadata_payload(state, requested_profile_id, session_id)
            .await
            .ok()
            .flatten()
    {
        // A moved thread can remain in the source profile's state DB. Treat that
        // metadata as stale unless the profile still owns a materialized rollout.
        if resolve_rollout_path(state, requested_profile_id, session_id, &thread).is_some() {
            return requested_profile_id.to_string();
        }
    }

    for candidate_profile_id in runtime_profiles_snapshot(&state.config).1.keys() {
        if candidate_profile_id == requested_profile_id {
            continue;
        }
        let candidate_runtime_key = runtime_session_key(candidate_profile_id, session_id);
        if state
            .session_app_server_assignments
            .lock()
            .await
            .contains_key(&candidate_runtime_key)
        {
            return candidate_profile_id.clone();
        }
        if let Some(thread) =
            read_local_thread_metadata_payload(state, candidate_profile_id, session_id)
                .await
                .ok()
                .flatten()
        {
            if resolve_rollout_path(state, candidate_profile_id, session_id, &thread).is_some() {
                return candidate_profile_id.clone();
            }
        }
    }

    requested_profile_id.to_string()
}

pub(crate) fn resolve_rollout_path(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    thread: &Value,
) -> Option<PathBuf> {
    let profile = resolve_runtime_profile(&state.config, profile_id);
    if let Some(path) = thread
        .get("rolloutPath")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .filter(|path| path.is_file() && path.starts_with(&profile.codex_home))
    {
        return Some(path);
    }
    let created_at = thread
        .get("createdAt")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let created_at_seconds = if created_at > 10_000_000_000 {
        created_at / 1000
    } else {
        created_at
    };

    if created_at_seconds > 0 {
        if let Ok(base) = time::OffsetDateTime::from_unix_timestamp(created_at_seconds) {
            for offset in [0_i64, -1, 1] {
                let Some(candidate) = base.checked_add(time::Duration::days(offset)) else {
                    continue;
                };
                let date = candidate.date();
                let day_directory = profile
                    .codex_home
                    .join("sessions")
                    .join(date.year().to_string())
                    .join(format!("{:02}", u8::from(date.month())))
                    .join(format!("{:02}", date.day()));
                if let Ok(entries) = fs::read_dir(&day_directory) {
                    for entry in entries.flatten() {
                        if entry
                            .file_name()
                            .to_str()
                            .is_some_and(|name| name.ends_with(&format!("{session_id}.jsonl")))
                        {
                            return Some(entry.path());
                        }
                    }
                }
            }
        }
    }

    let archived_directory = profile.codex_home.join("archived_sessions");
    if let Ok(entries) = fs::read_dir(archived_directory) {
        for entry in entries.flatten() {
            if entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.ends_with(&format!("{session_id}.jsonl")))
            {
                return Some(entry.path());
            }
        }
    }

    None
}

pub(crate) async fn emit_session_notification(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    event: Value,
) {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id).0;
    let relay = {
        let relays = state.relays.lock().await;
        relays
            .get(&session_relay_key(&resolved_profile_id, session_id))
            .cloned()
    };
    if let Some(relay) = relay {
        let _ = relay.send(event);
    }
}
