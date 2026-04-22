use super::*;

fn is_unmaterialized_thread_error_message(message: &str) -> bool {
    let lowered = message.to_ascii_lowercase();
    lowered.contains("not materialized yet")
        || lowered.contains("includeturns is unavailable before first user message")
}

fn thread_agent_nickname(thread: &Value) -> Option<String> {
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

fn thread_agent_role(thread: &Value) -> Option<String> {
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

pub(crate) fn thread_is_subagent(thread: &Value) -> bool {
    thread
        .get("isSubagent")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || thread
            .get("source")
            .and_then(|value| value.get("subagent"))
            .and_then(Value::as_object)
            .is_some_and(|value| !value.is_empty())
        || thread_agent_nickname(thread).is_some()
        || thread_agent_role(thread).is_some()
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
    if normalized
        .get("type")
        .and_then(Value::as_str)
        .is_none_or(|value| value.trim().is_empty())
    {
        normalized.insert("type".to_string(), Value::String("unknown".to_string()));
    }
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
        .map(|(item_index, item)| normalize_session_item_payload(item, &turn_id, item_index))
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
    turns.iter().find_map(|turn| {
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

async fn session_pending_requests_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> Vec<Value> {
    let runtime_key = runtime_session_key(
        resolve_runtime_profile_entry(&state.config, profile_id).0,
        session_id,
    );
    let mut requests = state
        .pending_server_requests
        .lock()
        .await
        .get(&runtime_key)
        .map(|entries| {
            entries
                .iter()
                .map(|(request_id, pending)| {
                    json!({
                        "id": request_id,
                        "method": pending.method,
                        "params": pending.params,
                        "createdAt": pending.created_at
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    requests.sort_by(|left, right| {
        right
            .get("createdAt")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(
                left.get("createdAt")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
    });
    requests
}

pub(crate) async fn read_thread_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    include_turns: bool,
) -> ApiResult<Value> {
    let client = app_server_client(state, profile_id)
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

pub(crate) fn resolve_rollout_path(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    thread: &Value,
) -> Option<PathBuf> {
    let profile = resolve_runtime_profile(&state.config, profile_id);
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

pub(crate) async fn session_detail_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    limit: u64,
) -> ApiResult<Value> {
    let thread = read_thread_payload(state, profile_id, session_id, true).await?;
    let turns = thread
        .get("turns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let total_turns = turns.len();
    let window_size = limit.clamp(1, 200) as usize;
    let start = total_turns.saturating_sub(window_size);
    let visible_turns = turns[start..].to_vec();
    let active_turn_id = state
        .active_turns
        .lock()
        .await
        .get(&runtime_session_key(
            resolve_runtime_profile_entry(&state.config, profile_id).0,
            session_id,
        ))
        .cloned()
        .or_else(|| active_turn_id_from_turns(&turns));
    let preferences = with_ui_state_read(state, profile_id, |ui_state| {
        Ok(ui_state
            .get("preferencesByThreadId")
            .and_then(Value::as_object)
            .and_then(|entries| entries.get(session_id))
            .cloned()
            .unwrap_or_else(|| {
                json!({
                    "cwd": thread.get("cwd").cloned().unwrap_or(Value::Null)
                })
            }))
    })
    .await?;
    let selected_skills = with_ui_state_read(state, profile_id, |ui_state| {
        Ok(Value::Array(session_selected_skills_from_ui_state(
            ui_state, session_id,
        )))
    })
    .await?;

    Ok(json!({
        "thread": {
            "id": thread.get("id").cloned().unwrap_or_else(|| json!(session_id)),
            "preview": thread.get("preview").cloned().unwrap_or_else(|| json!("")),
            "name": thread.get("name").cloned().unwrap_or(Value::Null),
            "cwd": thread.get("cwd").cloned().unwrap_or(Value::Null),
            "status": thread.get("status").cloned().unwrap_or_else(|| json!("unknown")),
            "createdAt": thread.get("createdAt").cloned().unwrap_or_else(|| json!(0)),
            "updatedAt": thread.get("updatedAt").cloned().unwrap_or_else(|| json!(0)),
            "isSubagent": thread.get("isSubagent").cloned().unwrap_or_else(|| json!(false)),
            "agentNickname": thread.get("agentNickname").cloned().unwrap_or(Value::Null),
            "agentRole": thread.get("agentRole").cloned().unwrap_or(Value::Null),
            "turns": visible_turns
        },
        "preferences": preferences,
        "selectedSkills": selected_skills,
        "attachments": list_session_attachments_payload(state, profile_id, session_id).await?,
        "queue": get_session_queue_payload(state, profile_id, session_id).await?,
        "pendingRequests": session_pending_requests_payload(state, profile_id, session_id).await,
        "activeTurnId": active_turn_id,
        "tokenUsage": thread.get("tokenUsage").cloned().unwrap_or(Value::Null),
        "hydration": {
            "state": "complete",
            "loadedTurns": total_turns.saturating_sub(start),
            "totalTurns": total_turns,
            "remainingTurns": start,
            "message": Value::Null,
            "recovery": {
                "available": false,
                "issue": Value::Null,
                "totalLines": Value::Null,
                "recoverableLines": Value::Null,
                "skippedLines": Value::Null
            }
        }
    }))
}

pub(crate) async fn session_older_turns_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    before_turn_id: &str,
    limit: u64,
) -> ApiResult<Value> {
    let thread = read_thread_payload(state, profile_id, session_id, true).await?;
    let turns = thread
        .get("turns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let Some(before_index) = turns
        .iter()
        .position(|turn| turn.get("id").and_then(Value::as_str) == Some(before_turn_id))
    else {
        return Ok(json!({
            "turns": [],
            "loadedTurns": turns.len(),
            "totalTurns": turns.len(),
            "remainingTurns": 0
        }));
    };
    let window_size = limit.clamp(1, 200) as usize;
    let start = before_index.saturating_sub(window_size);
    Ok(json!({
        "turns": turns[start..before_index].to_vec(),
        "loadedTurns": before_index,
        "totalTurns": turns.len(),
        "remainingTurns": start
    }))
}

pub(crate) async fn session_turn_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    turn_id: &str,
) -> ApiResult<Value> {
    let thread = read_thread_payload(state, profile_id, session_id, true).await?;
    let turn = thread
        .get("turns")
        .and_then(Value::as_array)
        .and_then(|turns| {
            turns
                .iter()
                .find(|turn| turn.get("id").and_then(Value::as_str) == Some(turn_id))
        })
        .cloned()
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Turn not found."))?;
    Ok(json!({ "turn": turn }))
}

pub(crate) async fn session_item_detail_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    turn_id: &str,
    item_id: &str,
) -> ApiResult<Value> {
    let turn = session_turn_payload(state, profile_id, session_id, turn_id)
        .await?
        .get("turn")
        .cloned()
        .unwrap_or(Value::Null);
    let mut item = turn
        .get("items")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.get("id").and_then(Value::as_str) == Some(item_id))
        })
        .cloned()
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Transcript item detail not found."))?;
    if let Some(item_object) = item.as_object_mut() {
        item_object.insert(
            "detailState".to_string(),
            Value::String("loaded".to_string()),
        );
    }
    Ok(json!({ "item": item }))
}

pub(crate) async fn search_session_turns_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    query: &str,
    cursor: Option<&str>,
    limit: u64,
) -> ApiResult<Value> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Ok(json!({
            "matches": [],
            "nextCursor": Value::Null,
            "totalMatches": 0
        }));
    }

    let thread = read_thread_payload(state, profile_id, session_id, true).await?;
    let turns = thread
        .get("turns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut matches = Vec::new();

    for (turn_index, turn) in turns.iter().enumerate() {
        let started_at = turn.get("startedAt").and_then(Value::as_i64);
        for item in turn
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let serialized = serde_json::to_string(&item).unwrap_or_default();
            let normalized = serialized.replace("\\n", " ").replace('\n', " ");
            let lowered = normalized.to_lowercase();
            let Some(match_index) = lowered.find(&needle) else {
                continue;
            };
            let normalized_chars = normalized.chars().collect::<Vec<_>>();
            let match_char_index = lowered[..match_index].chars().count();
            let snippet_start = match_char_index.saturating_sub(54);
            let snippet_end =
                (match_char_index + needle.chars().count() + 54).min(normalized_chars.len());
            matches.push(json!({
                "turnId": turn.get("id").cloned().unwrap_or(Value::Null),
                "turnIndex": turn_index,
                "itemId": item.get("id").cloned().unwrap_or(Value::Null),
                "itemType": item.get("type").cloned().unwrap_or(Value::Null),
                "preview": format!(
                    "{}{}{}",
                    if snippet_start > 0 { "..." } else { "" },
                    normalized_chars[snippet_start..snippet_end]
                        .iter()
                        .collect::<String>()
                        .trim(),
                    if snippet_end < normalized_chars.len() { "..." } else { "" }
                ),
                "startedAt": started_at,
                "requiresFullTurn": false,
                "requiresItemDetail": false
            }));
        }
    }

    let window_size = limit.clamp(1, 200) as usize;
    let start = cursor
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let end = start.saturating_add(window_size).min(matches.len());
    Ok(json!({
        "matches": if start < matches.len() { matches[start..end].to_vec() } else { Vec::<Value>::new() },
        "nextCursor": (end < matches.len()).then(|| end.to_string()),
        "totalMatches": matches.len()
    }))
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
            .get(&session_relay_key(resolved_profile_id, session_id))
            .cloned()
    };
    if let Some(relay) = relay {
        let _ = relay.send(event);
    }
}

pub(crate) async fn update_session_organization_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    patch: Value,
) -> ApiResult<Value> {
    let payload = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(session_meta_by_thread_id) = ui_state
            .get_mut("sessionMetaByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "session metadata state is missing",
            ));
        };

        let current = session_meta_by_thread_id
            .get(session_id)
            .cloned()
            .unwrap_or_else(|| json!({ "pinned": false, "tags": [] }));
        let pinned = patch
            .get("pinned")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| {
                current
                    .get("pinned")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            });
        let mut tags = patch
            .get("tags")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| {
                current
                    .get("tags")
                    .and_then(Value::as_array)
                    .map(|entries| {
                        entries
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            });
        tags.sort();
        tags.dedup();

        let meta = json!({
            "pinned": pinned,
            "tags": tags
        });
        if !pinned
            && meta
                .get("tags")
                .and_then(Value::as_array)
                .is_some_and(|items| items.is_empty())
        {
            session_meta_by_thread_id.remove(session_id);
        } else {
            session_meta_by_thread_id.insert(session_id.to_string(), meta.clone());
        }

        Ok(json!({
            "meta": meta,
            "knownTags": known_tags_from_ui_state(ui_state)
        }))
    })
    .await?;

    emit_profile_config_updated(
        state,
        profile_id,
        json!({
            "sessionOrganization": {
                "knownTags": payload.get("knownTags").cloned().unwrap_or_else(|| json!([]))
            }
        }),
    )
    .await;
    emit_session_summary_updated(state, profile_id, session_id, None).await;

    Ok(payload)
}

pub(crate) async fn save_session_preferences_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    preferences: Value,
) -> ApiResult<Value> {
    let next_preferences =
        normalize_session_preferences_payload(state, profile_id, preferences).await?;
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
        preferences_by_thread_id.insert(session_id.to_string(), next_preferences.clone());
        Ok(())
    })
    .await?;
    sync_codex_toml_with_preferences(
        &resolve_runtime_profile(&state.config, profile_id).codex_home,
        &next_preferences,
    )
    .await
    .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;

    emit_session_notification(
        state,
        profile_id,
        session_id,
        json!({
            "kind": "notification",
            "method": "codex-webui/preferencesUpdated",
            "params": {
                "preferences": next_preferences.clone()
            }
        }),
    )
    .await;
    emit_profile_config_updated(
        state,
        profile_id,
        json!({
            "defaults": next_preferences.clone()
        }),
    )
    .await;
    emit_session_summary_updated(
        state,
        profile_id,
        session_id,
        Some(next_preferences.clone()),
    )
    .await;

    Ok(next_preferences)
}

pub(crate) async fn save_session_skills_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    skills: Option<&Value>,
) -> ApiResult<Value> {
    let next_skills = selected_skills_from_value(skills);
    with_ui_state_write(state, profile_id, |ui_state| {
        let Some(skills_by_thread_id) = ui_state
            .get_mut("skillsByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "skills state is missing",
            ));
        };
        skills_by_thread_id.insert(session_id.to_string(), Value::Array(next_skills.clone()));
        Ok(())
    })
    .await?;

    emit_session_notification(
        state,
        profile_id,
        session_id,
        json!({
            "kind": "notification",
            "method": "codex-webui/skillsUpdated",
            "params": {
                "selectedSkills": next_skills.clone()
            }
        }),
    )
    .await;

    Ok(Value::Array(next_skills))
}

pub(crate) async fn rename_session_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    name: &str,
) -> ApiResult<Value> {
    let trimmed_name = name.trim();
    if trimmed_name.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Session name is required.",
        ));
    }

    app_server_client(state, profile_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to codex app-server: {error}"),
            )
        })?
        .request(
            "thread/name/set",
            json!({
                "threadId": session_id,
                "name": trimmed_name
            }),
        )
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to rename the session: {error}"),
            )
        })?;

    emit_session_notification(
        state,
        profile_id,
        session_id,
        json!({
            "kind": "notification",
            "method": "thread/name/updated",
            "params": {
                "threadId": session_id,
                "threadName": trimmed_name
            }
        }),
    )
    .await;
    emit_session_summary_updated(state, profile_id, session_id, None).await;

    Ok(json!({
        "ok": true,
        "name": trimmed_name
    }))
}

pub(crate) async fn archive_session_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<Value> {
    app_server_client(state, profile_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to codex app-server: {error}"),
            )
        })?
        .request(
            "thread/archive",
            json!({
                "threadId": session_id
            }),
        )
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to archive the session: {error}"),
            )
        })?;

    invalidate_session_lists(state, profile_id).await;
    Ok(json!({ "ok": true }))
}

pub(crate) async fn unarchive_session_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<Value> {
    app_server_client(state, profile_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to codex app-server: {error}"),
            )
        })?
        .request(
            "thread/unarchive",
            json!({
                "threadId": session_id
            }),
        )
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to unarchive the session: {error}"),
            )
        })?;

    invalidate_session_lists(state, profile_id).await;
    let session = build_session_summary_payload(state, profile_id, session_id, None).await?;
    Ok(json!({
        "ok": true,
        "session": session
    }))
}
