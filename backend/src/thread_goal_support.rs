use super::*;

fn canonical_goal_status(value: &str) -> Option<&'static str> {
    match value {
        "active" => Some("active"),
        "paused" | "pause" => Some("paused"),
        "blocked" | "block" => Some("blocked"),
        "usageLimited" | "usage_limited" | "usage-limited" | "usagelimited" => Some("usageLimited"),
        "budgetLimited" | "budget_limited" | "budget-limited" | "budgetlimited" => {
            Some("budgetLimited")
        }
        "complete" | "completed" => Some("complete"),
        _ => None,
    }
}

fn normalize_goal_status(value: Option<&Value>) -> String {
    value_text(value.unwrap_or(&Value::Null))
        .and_then(|status| canonical_goal_status(status.as_str()).map(str::to_string))
        .unwrap_or_else(|| "active".to_string())
}

pub(crate) fn normalize_thread_goal_payload(goal: &Value, fallback_thread_id: &str) -> Value {
    let object = goal.as_object().cloned().unwrap_or_default();
    json!({
        "threadId": object
            .get("threadId")
            .or_else(|| object.get("thread_id"))
            .and_then(Value::as_str)
            .unwrap_or(fallback_thread_id),
        "objective": object
            .get("objective")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        "status": normalize_goal_status(object.get("status")),
        "tokenBudget": object
            .get("tokenBudget")
            .or_else(|| object.get("token_budget"))
            .cloned()
            .unwrap_or(Value::Null),
        "tokensUsed": object
            .get("tokensUsed")
            .or_else(|| object.get("tokens_used"))
            .and_then(Value::as_i64)
            .unwrap_or(0),
        "timeUsedSeconds": object
            .get("timeUsedSeconds")
            .or_else(|| object.get("time_used_seconds"))
            .and_then(Value::as_i64)
            .unwrap_or(0),
        "createdAt": object
            .get("createdAt")
            .or_else(|| object.get("created_at"))
            .and_then(Value::as_i64)
            .unwrap_or(0),
        "updatedAt": object
            .get("updatedAt")
            .or_else(|| object.get("updated_at"))
            .and_then(Value::as_i64)
            .unwrap_or(0)
    })
}

fn goal_from_response(response: &Value, session_id: &str) -> Value {
    response
        .get("goal")
        .filter(|goal| !goal.is_null())
        .map(|goal| normalize_thread_goal_payload(goal, session_id))
        .unwrap_or(Value::Null)
}

pub(crate) async fn cache_session_goal_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    goal: &Value,
) {
    let goal = goal.clone();
    let _ = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(goals_by_thread_id) = ui_state
            .get_mut("goalsByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "goal cache state is missing",
            ));
        };
        if goal.is_null() {
            goals_by_thread_id.remove(session_id);
        } else {
            goals_by_thread_id.insert(session_id.to_string(), goal);
        }
        Ok(())
    })
    .await;
}

pub(crate) async fn cached_session_goal_or_null_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> Value {
    with_ui_state_read(state, profile_id, |ui_state| {
        Ok(ui_state
            .get("goalsByThreadId")
            .and_then(Value::as_object)
            .and_then(|goals| goals.get(session_id))
            .cloned()
            .unwrap_or(Value::Null))
    })
    .await
    .unwrap_or(Value::Null)
}

fn is_goal_disabled_error(error: &anyhow::Error) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    message.contains("goal") && message.contains("disabled")
}

async fn request_thread_goal(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    method: &str,
    params: Value,
    dedicate_unassigned_session: bool,
) -> ApiResult<Value> {
    let client_result = if dedicate_unassigned_session {
        app_server_client_for_goal_session(state, profile_id, session_id).await
    } else {
        app_server_client_for_session(state, profile_id, session_id).await
    };
    let client = client_result.map_err(|error| {
        api_error(
            StatusCode::BAD_GATEWAY,
            format!("Failed to connect to codex app-server: {error}"),
        )
    })?;
    match client.request(method, params.clone()).await {
        Ok(response) => Ok(response),
        Err(error) if is_goal_disabled_error(&error) => {
            client
                .request(
                    "config/batchWrite",
                    json!({
                        "edits": [
                            {
                                "keyPath": "features.goals",
                                "value": true,
                                "mergeStrategy": "replace"
                            }
                        ],
                        "filePath": null,
                        "expectedVersion": null,
                        "reloadUserConfig": true
                    }),
                )
                .await
                .map_err(|enable_error| {
                    api_error(
                        StatusCode::BAD_GATEWAY,
                        format!("Failed to enable Codex goals feature: {enable_error}"),
                    )
                })?;
            client.request(method, params).await.map_err(|retry_error| {
                api_error(
                    StatusCode::BAD_GATEWAY,
                    format!("Failed to proxy Codex goal request: {retry_error}"),
                )
            })
        }
        Err(error) => Err(api_error(
            StatusCode::BAD_GATEWAY,
            format!("Failed to proxy Codex goal request: {error}"),
        )),
    }
}

pub(crate) async fn fetch_session_goal_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<Value> {
    let response = request_thread_goal(
        state,
        profile_id,
        session_id,
        "thread/goal/get",
        json!({ "threadId": session_id }),
        false,
    )
    .await?;
    let goal = goal_from_response(&response, session_id);
    cache_session_goal_payload(state, profile_id, session_id, &goal).await;
    Ok(goal)
}

pub(crate) async fn get_session_goal_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<Value> {
    Ok(json!({
        "goal": fetch_session_goal_payload(state, profile_id, session_id).await?
    }))
}

pub(crate) async fn set_session_goal_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    params: Value,
) -> ApiResult<Value> {
    let mut app_server_params = serde_json::Map::new();
    app_server_params.insert("threadId".to_string(), json!(session_id));

    if let Some(objective) = params
        .get("objective")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        app_server_params.insert("objective".to_string(), json!(objective));
    }

    if let Some(status) = params
        .get("status")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let normalized = canonical_goal_status(status).ok_or_else(|| {
            api_error(
                StatusCode::BAD_REQUEST,
                "Invalid goal status. Use active, paused, blocked, usageLimited, budgetLimited, or complete.",
            )
        })?;
        app_server_params.insert("status".to_string(), json!(normalized));
    }

    if params.get("tokenBudget").is_some() {
        app_server_params.insert(
            "tokenBudget".to_string(),
            params.get("tokenBudget").cloned().unwrap_or(Value::Null),
        );
    } else if params.get("token_budget").is_some() {
        app_server_params.insert(
            "tokenBudget".to_string(),
            params.get("token_budget").cloned().unwrap_or(Value::Null),
        );
    }

    if app_server_params.len() == 1 {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Goal updates require an objective, status, or token budget.",
        ));
    }

    let response = request_thread_goal(
        state,
        profile_id,
        session_id,
        "thread/goal/set",
        Value::Object(app_server_params),
        true,
    )
    .await?;
    let goal = goal_from_response(&response, session_id);
    cache_session_goal_payload(state, profile_id, session_id, &goal).await;
    Ok(json!({
        "goal": goal
    }))
}

pub(crate) async fn clear_session_goal_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<Value> {
    let response = request_thread_goal(
        state,
        profile_id,
        session_id,
        "thread/goal/clear",
        json!({ "threadId": session_id }),
        false,
    )
    .await?;
    cache_session_goal_payload(state, profile_id, session_id, &Value::Null).await;
    Ok(json!({
        "goal": Value::Null,
        "cleared": response
            .get("cleared")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }))
}
