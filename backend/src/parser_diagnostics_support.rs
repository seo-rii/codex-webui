use super::*;

fn comparable_summary_payload(summary: Option<&Value>) -> Value {
    let Some(summary) = summary else {
        return Value::Null;
    };
    json!({
        "name": summary.get("name").cloned().unwrap_or(Value::Null),
        "preview": summary.get("preview").cloned().unwrap_or(Value::Null),
        "status": summary.get("status").cloned().unwrap_or(Value::Null),
        "createdAt": summary.get("createdAt").cloned().unwrap_or(Value::Null),
        "updatedAt": summary.get("updatedAt").cloned().unwrap_or(Value::Null),
        "archived": summary.get("archived").cloned().unwrap_or(Value::Null),
        "isSubagent": summary.get("isSubagent").cloned().unwrap_or(Value::Null)
    })
}

fn comparable_goal_payload(goal: &Value) -> Value {
    if goal.is_null() {
        return Value::Null;
    }
    json!({
        "objective": goal.get("objective").cloned().unwrap_or(Value::Null),
        "status": goal.get("status").cloned().unwrap_or(Value::Null),
        "tokenBudget": goal.get("tokenBudget").cloned().unwrap_or(Value::Null),
        "tokensUsed": goal.get("tokensUsed").cloned().unwrap_or(Value::Null)
    })
}

fn item_text(item: &Value) -> Option<String> {
    item.get("text")
        .and_then(Value::as_str)
        .or_else(|| item.get("message").and_then(Value::as_str))
        .map(|value| {
            let trimmed = value.trim();
            if trimmed.chars().count() > 120 {
                format!("{}...", trimmed.chars().take(120).collect::<String>())
            } else {
                trimmed.to_string()
            }
        })
        .filter(|value| !value.is_empty())
}

fn comparable_turns_payload(turns: &[Value], limit: usize) -> Value {
    let start = turns.len().saturating_sub(limit);
    Value::Array(
        turns[start..]
            .iter()
            .map(|turn| {
                let items = turn
                    .get("items")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .map(|item| {
                        json!({
                            "id": item.get("id").cloned().unwrap_or(Value::Null),
                            "type": item.get("type").cloned().unwrap_or(Value::Null),
                            "text": item_text(item).map(Value::String).unwrap_or(Value::Null)
                        })
                    })
                    .collect::<Vec<_>>();
                json!({
                    "id": turn.get("id").cloned().unwrap_or(Value::Null),
                    "status": turn.get("status").cloned().unwrap_or(Value::Null),
                    "itemCount": turn.get("items").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
                    "items": items
                })
            })
            .collect(),
    )
}

fn push_value_mismatch(
    mismatches: &mut Vec<Value>,
    category: &str,
    field: &str,
    local: Value,
    native: Value,
) {
    if local == native {
        return;
    }
    mismatches.push(json!({
        "category": category,
        "field": field,
        "local": local,
        "native": native
    }));
}

fn push_object_field_mismatches(
    mismatches: &mut Vec<Value>,
    category: &str,
    local: &Value,
    native: &Value,
    fields: &[&str],
) {
    for field in fields {
        push_value_mismatch(
            mismatches,
            category,
            field,
            local.get(*field).cloned().unwrap_or(Value::Null),
            native.get(*field).cloned().unwrap_or(Value::Null),
        );
    }
}

pub(crate) async fn compare_parser_with_native_session_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    limit: u64,
) -> ApiResult<Value> {
    let normalized_limit = limit.clamp(1, 20) as usize;
    let snapshot = read_session_summary_ui_snapshot(state, profile_id).await?;
    let local_thread = read_local_thread_metadata_payload(state, profile_id, session_id).await?;
    let local_summary = match local_thread.as_ref() {
        Some(thread) => Some(build_session_summary_from_thread_payload(
            thread, &snapshot, None, None,
        )?),
        None => None,
    };
    let local_detail =
        local_session_diagnostics_payload(state, profile_id, session_id, normalized_limit as u64)
            .await?;
    let local_goal = cached_session_goal_or_null_payload(state, profile_id, session_id).await;

    let client = app_server_client_for_session(state, profile_id, session_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to codex app-server: {error}"),
            )
        })?;
    let native_response = client
        .request(
            "thread/read",
            json!({
                "threadId": session_id,
                "includeTurns": true
            }),
        )
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to read native Codex thread: {error}"),
            )
        })?;
    let native_thread = native_response
        .get("thread")
        .cloned()
        .unwrap_or(Value::Null);
    let native_summary =
        build_session_summary_from_thread_payload(&native_thread, &snapshot, None, None)?;
    let native_goal = fetch_session_goal_payload(state, profile_id, session_id)
        .await
        .unwrap_or(Value::Null);

    let local_summary_comparable = comparable_summary_payload(local_summary.as_ref());
    let native_summary_comparable = comparable_summary_payload(Some(&native_summary));
    let local_goal_comparable = comparable_goal_payload(&local_goal);
    let native_goal_comparable = comparable_goal_payload(&native_goal);
    let local_turns = local_detail
        .as_ref()
        .and_then(|detail| detail.get("thread"))
        .and_then(|thread| thread.get("turns"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let native_turns = native_thread
        .get("turns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let local_recent_turns = comparable_turns_payload(&local_turns, normalized_limit);
    let native_recent_turns = comparable_turns_payload(&native_turns, normalized_limit);

    let mut mismatches = Vec::new();
    if local_summary.is_none() {
        mismatches.push(json!({
            "category": "parser",
            "field": "localThread",
            "local": Value::Null,
            "native": "available"
        }));
    }
    push_object_field_mismatches(
        &mut mismatches,
        "summary",
        &local_summary_comparable,
        &native_summary_comparable,
        &[
            "name",
            "preview",
            "status",
            "createdAt",
            "updatedAt",
            "archived",
            "isSubagent",
        ],
    );
    push_object_field_mismatches(
        &mut mismatches,
        "goal",
        &local_goal_comparable,
        &native_goal_comparable,
        &["objective", "status", "tokenBudget", "tokensUsed"],
    );
    push_value_mismatch(
        &mut mismatches,
        "recentTurns",
        "turns",
        local_recent_turns.clone(),
        native_recent_turns.clone(),
    );

    Ok(json!({
        "sessionId": session_id,
        "limit": normalized_limit,
        "ok": mismatches.is_empty(),
        "mismatchCount": mismatches.len(),
        "mismatches": mismatches,
        "local": {
            "available": local_summary.is_some(),
            "summary": local_summary_comparable,
            "goal": local_goal_comparable,
            "recentTurns": local_recent_turns,
            "hydration": local_detail
                .as_ref()
                .and_then(|detail| detail.get("hydration"))
                .cloned()
                .unwrap_or(Value::Null)
        },
        "native": {
            "available": !native_thread.is_null(),
            "summary": native_summary_comparable,
            "goal": native_goal_comparable,
            "recentTurns": native_recent_turns
        }
    }))
}
