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

pub(crate) async fn clear_session_pending_requests(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) {
    let runtime_key = runtime_session_key(
        resolve_runtime_profile_entry(&state.config, profile_id).0,
        session_id,
    );
    let removed = state
        .pending_server_requests
        .lock()
        .await
        .remove(&runtime_key)
        .unwrap_or_default();
    let removed_request_ids = removed.keys().cloned().collect::<HashSet<_>>();
    for request_id in removed.keys() {
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
    }
    if !removed.is_empty() {
        let unread_count = with_ui_state_write(state, profile_id, |ui_state| {
            let Some(items) = ui_state
                .get_mut("notifications")
                .and_then(Value::as_object_mut)
                .and_then(|notifications| notifications.get_mut("items"))
                .and_then(Value::as_array_mut)
            else {
                return Ok(None);
            };
            let before = items.len();
            items.retain(|item| {
                let payload = item.get("payload").unwrap_or(&Value::Null);
                let request_id = payload.get("requestId").and_then(Value::as_str);
                let is_stale_approval = item.get("type").and_then(Value::as_str)
                    == Some("sessionAttention")
                    && item.get("sessionId").and_then(Value::as_str) == Some(session_id)
                    && payload
                        .get("reason")
                        .and_then(Value::as_str)
                        .is_none_or(|reason| reason == "approval")
                    && request_id.is_none_or(|id| removed_request_ids.contains(id));
                !is_stale_approval
            });
            if items.len() == before {
                return Ok(None);
            }
            Ok(Some(unread_notification_count(items)))
        })
        .await
        .ok()
        .flatten();
        if let Some(unread_count) = unread_count {
            emit_profile_global_notification(
                state,
                profile_id,
                json!({
                    "kind": "notification",
                    "method": "codex-webui/notificationStateUpdated",
                    "params": {
                        "unreadCount": unread_count
                    }
                }),
            )
            .await;
            emit_profile_config_updated(
                state,
                profile_id,
                json!({
                    "notifications": {
                        "unreadCount": unread_count
                    }
                }),
            )
            .await;
        }
        let clear_approval_highlight = with_ui_state_read(state, profile_id, |ui_state| {
            Ok(ui_state
                .get("highlightsByThreadId")
                .and_then(Value::as_object)
                .and_then(|entries| entries.get(session_id))
                .is_some_and(|highlight| {
                    highlight.get("kind").and_then(Value::as_str) == Some("attention")
                        && highlight
                            .get("reason")
                            .and_then(Value::as_str)
                            .is_none_or(|reason| reason == "approval")
                }))
        })
        .await
        .unwrap_or(false);
        if clear_approval_highlight {
            let _ = with_ui_state_write(state, profile_id, |ui_state| {
                if let Some(highlights_by_thread_id) = ui_state
                    .get_mut("highlightsByThreadId")
                    .and_then(Value::as_object_mut)
                {
                    highlights_by_thread_id.remove(session_id);
                }
                Ok(())
            })
            .await;
        }
    }
}

async fn session_accepts_server_request(state: &AppState, runtime_key: &str) -> bool {
    state.active_turns.lock().await.contains_key(runtime_key)
        || state.pending_turn_starts.lock().await.contains(runtime_key)
}

pub(crate) async fn handle_profile_server_request(
    state: &AppState,
    profile_id: &str,
    client_key: &str,
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

    let runtime_key = runtime_session_key(
        resolve_runtime_profile_entry(&state.config, profile_id).0,
        &session_id,
    );
    if !session_accepts_server_request(state, &runtime_key).await {
        if let Ok(client) = app_server_client_by_key(state, profile_id, client_key).await {
            let _ = client
                .reject(
                    request.id.clone(),
                    "Session is no longer active; ignoring stale app-server request.".to_string(),
                )
                .await;
        }
        return;
    }

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
        if let Ok(client) = app_server_client_by_key(state, profile_id, client_key).await {
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
    let created_at_ms = now_unix_ms();
    {
        let mut pending_requests = state.pending_server_requests.lock().await;
        let entries = pending_requests.entry(runtime_key).or_default();
        entries.insert(
            request_id.clone(),
            PendingServerRequestEntry {
                raw_id: request.id.clone(),
                method: request.method.clone(),
                params: request.params.clone(),
                created_at: now_rfc3339(),
                created_at_ms,
            },
        );
        if entries.len() > PENDING_SERVER_REQUEST_MAX_PER_SESSION {
            let mut ordered = entries
                .iter()
                .filter(|(id, _)| id.as_str() != request_id)
                .map(|(id, pending)| (id.clone(), pending.created_at_ms))
                .collect::<Vec<_>>();
            ordered.sort_by_key(|(_, created_at)| *created_at);
            for (id, _) in ordered {
                if entries.len() <= PENDING_SERVER_REQUEST_MAX_PER_SESSION {
                    break;
                }
                entries.remove(&id);
            }
        }
    }

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
            "at": now_unix_ms(),
            "reason": "approval"
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

    app_server_client_for_session(state, profile_id, session_id)
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

    let client = app_server_client_for_session(state, profile_id, session_id)
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

pub(crate) async fn send_computer_input_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    mut input: Value,
) -> ApiResult<Value> {
    let event_type = input
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if !matches!(
        event_type.as_str(),
        "click" | "doubleclick" | "double_click" | "scroll" | "key" | "text"
    ) {
        return Err(api_error(StatusCode::BAD_REQUEST, "INVALID_COMPUTER_INPUT"));
    }

    if let Some(record) = input.as_object_mut() {
        record.insert("type".to_string(), Value::String(event_type.clone()));
        record.insert(
            "threadId".to_string(),
            Value::String(session_id.to_string()),
        );
        record.insert("sequence".to_string(), json!(now_unix_ms()));
        record.insert("transport".to_string(), json!("websocket"));
    }

    let input_text = format!(
        "Computer input event from codex-webui: {}",
        serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string())
    );
    let runtime_key = runtime_session_key(
        resolve_runtime_profile_entry(&state.config, profile_id).0,
        session_id,
    );
    let pending_request_id = {
        let pending_requests = state.pending_server_requests.lock().await;
        pending_requests.get(&runtime_key).and_then(|entries| {
            let mut candidates = entries
                .iter()
                .filter(|(_, pending)| {
                    pending.method == "item/tool/call"
                        && pending
                            .params
                            .get("namespace")
                            .and_then(Value::as_str)
                            .is_some_and(|namespace| namespace.eq_ignore_ascii_case("computer"))
                })
                .map(|(id, pending)| (id.clone(), pending.created_at_ms))
                .collect::<Vec<_>>();
            candidates.sort_by_key(|(_, created_at)| *created_at);
            candidates.into_iter().next().map(|(id, _)| id)
        })
    };

    let routed: &str;
    let upstream: Value;
    if let Some(request_id) = pending_request_id {
        let result = json!({
            "contentItems": [
                {
                    "type": "inputText",
                    "text": input_text
                }
            ],
            "success": true
        });
        upstream =
            resolve_server_request_payload(state, profile_id, session_id, &request_id, result)
                .await?;
        routed = "pendingDynamicTool";
    } else {
        let client = app_server_client_for_session(state, profile_id, session_id)
            .await
            .map_err(|error| {
                api_error(
                    StatusCode::BAD_GATEWAY,
                    format!("Failed to connect to codex app-server: {error}"),
                )
            })?;
        let tool = input
            .get("tool")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| match event_type.as_str() {
                "doubleclick" | "double_click" => "double_click".to_string(),
                "scroll" => "scroll".to_string(),
                "key" => "key".to_string(),
                "text" => "type".to_string(),
                _ => "click".to_string(),
            });
        let server = input
            .get("server")
            .and_then(Value::as_str)
            .unwrap_or("computer-use")
            .to_string();
        let mcp_result = client
            .request_with_timeout(
                "mcpServer/tool/call".to_string(),
                json!({
                    "threadId": session_id,
                    "server": server,
                    "tool": tool,
                    "arguments": input,
                    "_meta": {
                        "codexWebui": {
                            "transport": "websocket",
                            "fallback": true
                        }
                    }
                }),
                Duration::from_secs(8),
                false,
            )
            .await;
        match mcp_result {
            Ok(result) => {
                upstream = result;
                routed = "mcpServerTool";
            }
            Err(error) => {
                if resolve_active_turn_id_payload(state, profile_id, session_id)
                    .await?
                    .is_some()
                {
                    upstream = steer_turn_payload(
                        state,
                        profile_id,
                        session_id,
                        &input_text,
                        None,
                        None,
                        None,
                        None,
                    )
                    .await?;
                    routed = "turnSteer";
                } else {
                    upstream = client
                        .request_with_timeout(
                            "thread/inject_items".to_string(),
                            json!({
                                "threadId": session_id,
                                "items": [
                                    {
                                        "type": "message",
                                        "role": "user",
                                        "content": [
                                            {
                                                "type": "input_text",
                                                "text": input_text
                                            }
                                        ]
                                    }
                                ]
                            }),
                            Duration::from_secs(8),
                            false,
                        )
                        .await
                        .map_err(|inject_error| {
                            api_error(
                                StatusCode::BAD_GATEWAY,
                                format!(
                                    "Failed to deliver computer input: MCP call failed: {error}; inject fallback failed: {inject_error}"
                                ),
                            )
                        })?;
                    routed = "threadInject";
                }
            }
        }
    }

    emit_session_notification(
        state,
        profile_id,
        session_id,
        json!({
            "kind": "notification",
            "method": "codex-webui/computerInput",
            "params": {
                "threadId": session_id,
                "input": input,
                "routed": routed,
                "updatedAt": now_unix_ms()
            }
        }),
    )
    .await;

    Ok(json!({
        "ok": true,
        "routed": routed,
        "upstream": upstream
    }))
}
