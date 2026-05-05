use super::*;

async fn clear_completed_session_highlight_on_open(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) {
    let has_completed_highlight = with_ui_state_read(state, profile_id, |ui_state| {
        Ok(ui_state
            .get("highlightsByThreadId")
            .and_then(Value::as_object)
            .and_then(|entries| entries.get(session_id))
            .and_then(|highlight| highlight.get("kind"))
            .and_then(Value::as_str)
            == Some("completed"))
    })
    .await
    .unwrap_or(false);

    if has_completed_highlight {
        set_session_highlight(state, profile_id, session_id, None).await;
    }
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

fn summarize_session_turn_for_detail_payload(turn: &Value, turn_index: usize) -> Value {
    let mut summarized = turn.as_object().cloned().unwrap_or_default();
    let turn_id = summarized
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("turn-{turn_index}"));
    let items = summarized
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .enumerate()
        .filter_map(|(item_index, item)| {
            let normalized = normalize_session_item_payload(item, &turn_id, item_index);
            let item_type = normalized
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            if is_internal_session_item_type(item_type) {
                return None;
            }
            Some(match item_type {
                "commandExecution" | "fileChange" | "mcpToolCall" | "dynamicToolCall"
                | "webSearch" => prepare_session_deferred_item_payload(item, &turn_id, item_index),
                _ => normalized,
            })
        })
        .collect::<Vec<_>>();
    summarized.insert("id".to_string(), Value::String(turn_id));
    summarized.insert("items".to_string(), Value::Array(items));
    summarized.insert(
        "status".to_string(),
        Value::String(
            value_text(summarized.get("status").unwrap_or(&Value::Null))
                .unwrap_or_else(|| "unknown".to_string()),
        ),
    );
    summarized
        .entry("error".to_string())
        .or_insert_with(|| Value::Null);
    summarized
        .entry("startedAt".to_string())
        .or_insert_with(|| Value::Null);
    summarized
        .entry("completedAt".to_string())
        .or_insert_with(|| Value::Null);
    summarized
        .entry("durationMs".to_string())
        .or_insert_with(|| Value::Null);
    summarized.insert("detailState".to_string(), Value::String("full".to_string()));
    summarized.insert("hiddenItemCount".to_string(), Value::from(0));
    Value::Object(summarized)
}

pub(crate) async fn session_detail_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    limit: u64,
) -> ApiResult<Value> {
    let (
        thread,
        visible_turns,
        total_turns,
        start,
        hydration_state,
        hydration_message,
        hydration_recovery,
    ) = match read_thread_payload(state, profile_id, session_id, true).await {
        Ok(thread) => {
            let turns = thread
                .get("turns")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let total_turns = turns.len();
            let window_size = limit.clamp(1, 200) as usize;
            let start = total_turns.saturating_sub(window_size);
            let visible_turns = turns[start..]
                .iter()
                .enumerate()
                .map(|(visible_index, turn)| {
                    summarize_session_turn_for_detail_payload(turn, start + visible_index)
                })
                .collect::<Vec<_>>();
            (
                thread,
                visible_turns,
                total_turns,
                start,
                "complete",
                Value::Null,
                json!({
                    "available": false,
                    "issue": Value::Null,
                    "totalLines": Value::Null,
                    "recoverableLines": Value::Null,
                    "skippedLines": Value::Null
                }),
            )
        }
        Err(error) => {
            let thread = read_thread_metadata_payload(state, profile_id, session_id)
                .await
                .map_err(|_| error.clone())?;
            let rollout_path = resolve_rollout_path(state, profile_id, session_id, &thread)
                .ok_or_else(|| error.clone())?;
            let rollout_buffer = tokio_fs::read(&rollout_path)
                .await
                .map_err(|_| error.clone())?;
            let plan = inspect_rollout_recovery_content(&rollout_buffer);
            if !plan.info.available
                || plan.info.recoverable_lines == 0
                || plan.recovered_content.trim().is_empty()
            {
                return Err(error);
            }

            let turns = thread
                .get("turns")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let total_turns = turns.len();
            let window_size = limit.clamp(1, 200) as usize;
            let start = total_turns.saturating_sub(window_size);
            let visible_turns = turns[start..]
                .iter()
                .enumerate()
                .map(|(visible_index, turn)| {
                    summarize_session_turn_for_detail_payload(turn, start + visible_index)
                })
                .collect::<Vec<_>>();
            (
                thread,
                visible_turns,
                total_turns,
                start,
                "error",
                Value::String(error.message),
                json!(plan.info),
            )
        }
    };
    let turns = thread
        .get("turns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let runtime_key = runtime_session_key(
        resolve_runtime_profile_entry(&state.config, profile_id).0,
        session_id,
    );
    let active_turn_id_from_payload = active_turn_id_from_turns(&turns);
    let cached_active_turn_id = state.active_turns.lock().await.get(&runtime_key).cloned();
    let active_turn_id = active_turn_id_from_payload
        .clone()
        .or_else(|| cached_active_turn_id.clone());
    if let Some(turn_id) = active_turn_id_from_payload {
        state
            .active_turns
            .lock()
            .await
            .insert(runtime_key.clone(), turn_id);
    } else if active_turn_id.is_none() {
        state.active_turns.lock().await.remove(&runtime_key);
    }
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
    let goal = session_goal_or_null_payload(state, profile_id, session_id).await;
    clear_completed_session_highlight_on_open(state, profile_id, session_id).await;

    let payload = json!({
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
        "goal": goal,
        "attachments": list_session_attachments_payload(state, profile_id, session_id).await?,
        "queue": get_session_queue_payload(state, profile_id, session_id).await?,
        "pendingRequests": session_pending_requests_payload(state, profile_id, session_id).await,
        "activeTurnId": active_turn_id,
        "tokenUsage": thread.get("tokenUsage").cloned().unwrap_or(Value::Null),
        "hydration": {
            "state": hydration_state,
            "loadedTurns": total_turns.saturating_sub(start),
            "totalTurns": total_turns,
            "remainingTurns": start,
            "message": hydration_message,
            "recovery": hydration_recovery
        }
    });
    Ok(augment_session_detail_payload(payload))
}

fn augment_session_detail_payload(mut payload: Value) -> Value {
    let turns = payload
        .get("thread")
        .and_then(|thread| thread.get("turns"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let (turn_ids, turn_versions) = session_detail_turn_state(&turns);
    let metadata_version = payload_cache_version(&session_detail_metadata_payload(&payload));
    let state_hash = session_detail_state_hash(&metadata_version, &turn_ids, &turn_versions);

    if let Some(payload_object) = payload.as_object_mut() {
        payload_object.insert("turnIds".to_string(), json!(turn_ids));
        payload_object.insert("turnVersions".to_string(), json!(turn_versions));
        payload_object.insert(
            "metadataVersion".to_string(),
            Value::String(metadata_version),
        );
        payload_object.insert("stateHash".to_string(), Value::String(state_hash));
    }
    payload
}

fn session_detail_turn_state(turns: &[Value]) -> (Vec<String>, HashMap<String, String>) {
    let mut turn_ids = Vec::new();
    let mut turn_versions = HashMap::new();

    for turn in turns {
        let Some(turn_id) = turn.get("id").and_then(Value::as_str) else {
            continue;
        };
        turn_ids.push(turn_id.to_string());
        turn_versions.insert(turn_id.to_string(), payload_cache_version(turn));
    }

    (turn_ids, turn_versions)
}

fn session_detail_metadata_payload(payload: &Value) -> Value {
    let mut metadata = payload.clone();
    if let Some(metadata_object) = metadata.as_object_mut() {
        for key in [
            "cacheVersion",
            "notModified",
            "turnIds",
            "turnVersions",
            "metadataVersion",
            "stateHash",
        ] {
            metadata_object.remove(key);
        }
        if let Some(thread_object) = metadata_object
            .get_mut("thread")
            .and_then(Value::as_object_mut)
        {
            thread_object.insert("turns".to_string(), json!([]));
        }
    }
    metadata
}

pub(crate) fn session_detail_turn_versions_from_value(
    value: Option<&Value>,
) -> Option<HashMap<String, String>> {
    value.and_then(Value::as_object).map(|versions| {
        versions
            .iter()
            .filter_map(|(turn_id, version)| {
                version
                    .as_str()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|version| (turn_id.clone(), version.to_string()))
            })
            .collect::<HashMap<_, _>>()
    })
}

pub(crate) fn session_detail_state_hash(
    metadata_version: &str,
    turn_ids: &[String],
    turn_versions: &HashMap<String, String>,
) -> String {
    let mut source = format!("metadata={metadata_version}\n");
    for turn_id in turn_ids {
        source.push_str(turn_id);
        source.push('\t');
        source.push_str(
            turn_versions
                .get(turn_id)
                .map(String::as_str)
                .unwrap_or_default(),
        );
        source.push('\n');
    }
    fnv1a32_hex(source.as_bytes())
}

pub(crate) fn cacheable_session_detail_response(
    payload: Value,
    known_version: Option<&str>,
    known_turn_versions: Option<HashMap<String, String>>,
    known_state_hash: Option<&str>,
) -> Value {
    let version = payload_cache_version(&payload);
    if known_version
        .map(str::trim)
        .is_some_and(|candidate| !candidate.is_empty() && candidate == version)
    {
        return json!({
            "cacheVersion": version,
            "notModified": true
        });
    }

    if known_version.is_some()
        && known_state_hash
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
        && known_turn_versions.is_some()
    {
        let known_versions = known_turn_versions.unwrap_or_default();
        let current_versions = session_detail_turn_versions_from_value(payload.get("turnVersions"))
            .unwrap_or_default();
        let turns = payload
            .get("thread")
            .and_then(|thread| thread.get("turns"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let turn_ids = payload
            .get("turnIds")
            .and_then(Value::as_array)
            .map(|ids| {
                ids.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let current_turn_ids = turn_ids.iter().cloned().collect::<HashSet<_>>();
        let turn_upserts = turns
            .into_iter()
            .filter(|turn| {
                let Some(turn_id) = turn.get("id").and_then(Value::as_str) else {
                    return false;
                };
                current_versions.get(turn_id) != known_versions.get(turn_id)
            })
            .collect::<Vec<_>>();
        let turn_removes = known_versions
            .keys()
            .filter(|turn_id| !current_turn_ids.contains(*turn_id))
            .cloned()
            .collect::<Vec<_>>();
        let mut thread = payload.get("thread").cloned().unwrap_or_else(|| json!({}));
        if let Some(thread_object) = thread.as_object_mut() {
            thread_object.insert("turns".to_string(), json!([]));
        }

        return json!({
            "cacheVersion": version,
            "notModified": false,
            "patch": {
                "baseCacheVersion": known_version.unwrap_or_default(),
                "baseStateHash": known_state_hash.unwrap_or_default(),
                "finalCacheVersion": version,
                "finalStateHash": payload.get("stateHash").cloned().unwrap_or(Value::Null),
                "metadataVersion": payload.get("metadataVersion").cloned().unwrap_or(Value::Null),
                "turnIds": turn_ids,
                "turnVersions": current_versions,
                "turnUpserts": turn_upserts,
                "turnRemoves": turn_removes,
                "thread": thread,
                "preferences": payload.get("preferences").cloned().unwrap_or(Value::Null),
                "selectedSkills": payload.get("selectedSkills").cloned().unwrap_or_else(|| json!([])),
                "goal": payload.get("goal").cloned().unwrap_or(Value::Null),
                "attachments": payload.get("attachments").cloned().unwrap_or_else(|| json!([])),
                "queue": payload.get("queue").cloned().unwrap_or(Value::Null),
                "pendingRequests": payload.get("pendingRequests").cloned().unwrap_or_else(|| json!([])),
                "activeTurnId": payload.get("activeTurnId").cloned().unwrap_or(Value::Null),
                "tokenUsage": payload.get("tokenUsage").cloned().unwrap_or(Value::Null),
                "hydration": payload.get("hydration").cloned().unwrap_or(Value::Null)
            }
        });
    }

    let mut next_payload = payload;
    if let Some(payload_object) = next_payload.as_object_mut() {
        payload_object.insert("cacheVersion".to_string(), Value::String(version));
        payload_object.insert("notModified".to_string(), Value::Bool(false));
    }
    next_payload
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
    let visible_turns = turns[start..before_index]
        .iter()
        .enumerate()
        .map(|(visible_index, turn)| {
            summarize_session_turn_for_detail_payload(turn, start + visible_index)
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "turns": visible_turns,
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
