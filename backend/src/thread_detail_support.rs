use super::*;

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
