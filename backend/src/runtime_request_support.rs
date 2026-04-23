use super::*;

fn pending_request_id(raw_id: &Value) -> String {
    raw_id
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| raw_id.to_string())
}

pub(crate) async fn set_session_highlight(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    highlight: Option<Value>,
) {
    let result = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(highlights_by_thread_id) = ui_state
            .get_mut("highlightsByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "highlight state is missing",
            ));
        };

        if let Some(highlight) = highlight {
            highlights_by_thread_id.insert(session_id.to_string(), highlight);
        } else {
            highlights_by_thread_id.remove(session_id);
        }

        Ok(())
    })
    .await;

    if result.is_ok() {
        emit_session_summary_updated(state, profile_id, session_id, None, None).await;
    }
}

pub(crate) async fn handle_profile_server_request(
    state: &AppState,
    profile_id: &str,
    request: &backend::codex_app_server::AppServerRequest,
) {
    let Some(session_id) = request
        .params
        .get("threadId")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return;
    };

    let preferences = with_ui_state_read(state, profile_id, |ui_state| {
        Ok(ui_state
            .get("preferencesByThreadId")
            .and_then(Value::as_object)
            .and_then(|entries| entries.get(&session_id))
            .cloned()
            .unwrap_or(Value::Null))
    })
    .await
    .unwrap_or(Value::Null);
    let auto_approve_mode = preferences
        .get("autoApproveMode")
        .and_then(Value::as_str)
        .unwrap_or("manual");

    let auto_approve_result = match request.method.as_str() {
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval"
            if auto_approve_mode == "turn" || auto_approve_mode == "session" =>
        {
            Some(json!({
                "decision": if auto_approve_mode == "session" { "acceptForSession" } else { "accept" }
            }))
        }
        "item/permissions/requestApproval"
            if auto_approve_mode == "turn" || auto_approve_mode == "session" =>
        {
            Some(json!({
                "scope": auto_approve_mode,
                "permissions": request.params.get("permissions").cloned().unwrap_or_else(|| json!({}))
            }))
        }
        _ => None,
    };

    if let Some(result) = auto_approve_result {
        if let Ok(client) = app_server_client(state, profile_id).await {
            if client.respond(request.id.clone(), result).await.is_ok() {
                emit_session_notification(
                    state,
                    profile_id,
                    &session_id,
                    json!({
                        "kind": "notification",
                        "method": "codex-webui/autoApproved",
                        "params": {
                            "requestId": pending_request_id(&request.id),
                            "requestMethod": request.method,
                            "autoApproveMode": auto_approve_mode
                        }
                    }),
                )
                .await;
                return;
            }
        }
    }

    let request_id = pending_request_id(&request.id);
    let runtime_key = runtime_session_key(
        resolve_runtime_profile_entry(&state.config, profile_id).0,
        &session_id,
    );
    state
        .pending_server_requests
        .lock()
        .await
        .entry(runtime_key)
        .or_default()
        .insert(
            request_id.clone(),
            PendingServerRequestEntry {
                raw_id: request.id.clone(),
                method: request.method.clone(),
                params: request.params.clone(),
                created_at: now_rfc3339(),
            },
        );

    emit_session_notification(
        state,
        profile_id,
        &session_id,
        json!({
            "kind": "serverRequest",
            "id": request_id.clone(),
            "method": request.method,
            "params": request.params
        }),
    )
    .await;
    emit_profile_global_notification(
        state,
        profile_id,
        json!({
            "kind": "notification",
            "method": "codex-webui/sessionAttention",
            "params": {
                "sessionId": session_id,
                "reason": "approval"
            }
        }),
    )
    .await;
    enqueue_profile_notification(
        state,
        profile_id,
        "sessionAttention",
        Some(&session_id),
        json!({
            "reason": "approval",
            "requestId": request_id,
            "requestMethod": request.method
        }),
    )
    .await;
    set_session_highlight(
        state,
        profile_id,
        &session_id,
        Some(json!({
            "kind": "attention",
            "at": now_unix_ms()
        })),
    )
    .await;
}

pub(crate) async fn abort_turn_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<Value> {
    let active_turn_id = resolve_active_turn_id_payload(state, profile_id, session_id).await?;
    let Some(turn_id) = active_turn_id else {
        return Ok(json!({ "interrupted": false }));
    };

    app_server_client(state, profile_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to codex app-server: {error}"),
            )
        })?
        .request(
            "turn/interrupt",
            json!({
                "threadId": session_id,
                "turnId": turn_id
            }),
        )
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to abort the session: {error}"),
            )
        })?;

    Ok(json!({ "interrupted": true }))
}

pub(crate) async fn resolve_server_request_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    request_id: &str,
    result: Value,
) -> ApiResult<Value> {
    let runtime_key = runtime_session_key(
        resolve_runtime_profile_entry(&state.config, profile_id).0,
        session_id,
    );
    let pending = state
        .pending_server_requests
        .lock()
        .await
        .get(&runtime_key)
        .and_then(|entries| entries.get(request_id))
        .cloned();

    let Some(pending) = pending else {
        return Err(api_error(StatusCode::NOT_FOUND, "SERVER_REQUEST_NOT_FOUND"));
    };

    let client = app_server_client(state, profile_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to codex app-server: {error}"),
            )
        })?;
    client
        .respond(pending.raw_id.clone(), result)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to resolve the server request: {error}"),
            )
        })?;

    let remaining = {
        let mut pending_requests = state.pending_server_requests.lock().await;
        let remaining = pending_requests
            .get_mut(&runtime_key)
            .map(|entries| {
                entries.remove(request_id);
                entries.len()
            })
            .unwrap_or(0);
        if remaining == 0 {
            pending_requests.remove(&runtime_key);
        }
        remaining
    };

    emit_session_notification(
        state,
        profile_id,
        session_id,
        json!({
            "kind": "notification",
            "method": "serverRequest/resolved",
            "params": {
                "threadId": session_id,
                "requestId": request_id
            }
        }),
    )
    .await;
    if remaining == 0 {
        set_session_highlight(state, profile_id, session_id, None).await;
    }

    Ok(json!({ "ok": true }))
}
