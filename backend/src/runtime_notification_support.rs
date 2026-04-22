use super::*;

pub(crate) async fn enqueue_profile_notification(
    state: &AppState,
    profile_id: &str,
    notification_type: &str,
    session_id: Option<&str>,
    payload: Value,
) {
    if !is_valid_notification_event_type(notification_type) {
        return;
    }

    let enabled = match with_ui_state_read(state, profile_id, |ui_state| {
        let enabled_event_types = ui_state
            .get("notifications")
            .and_then(|value| value.get("settings"))
            .and_then(|value| value.get("enabledEventTypes"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| {
                default_notification_settings_value()["enabledEventTypes"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            });
        Ok(enabled_event_types
            .iter()
            .filter_map(Value::as_str)
            .any(|entry| entry == notification_type))
    })
    .await
    {
        Ok(enabled) => enabled,
        Err(_) => false,
    };
    if !enabled {
        return;
    }

    let session_name = if let Some(session_id) = session_id {
        build_session_summary_payload(state, profile_id, session_id, None)
            .await
            .ok()
            .and_then(|summary| summary.get("name").cloned())
            .unwrap_or(Value::Null)
    } else {
        Value::Null
    };

    let notification = json!({
        "id": Uuid::new_v4().to_string(),
        "type": notification_type,
        "createdAt": now_unix_ms(),
        "readAt": Value::Null,
        "sessionId": session_id.map(Value::from).unwrap_or(Value::Null),
        "sessionName": session_name,
        "payload": payload
    });

    let unread_count = match with_ui_state_write(state, profile_id, |ui_state| {
        let Some(items) = ui_state
            .get_mut("notifications")
            .and_then(Value::as_object_mut)
            .and_then(|notifications| notifications.get_mut("items"))
            .and_then(Value::as_array_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "notifications state is missing",
            ));
        };

        items.insert(0, notification.clone());
        if items.len() > 200 {
            items.truncate(200);
        }
        Ok(unread_notification_count(items))
    })
    .await
    {
        Ok(unread_count) => unread_count,
        Err(_) => return,
    };

    emit_profile_global_notification(
        state,
        profile_id,
        json!({
            "kind": "notification",
            "method": "codex-webui/notificationAdded",
            "params": {
                "notification": notification,
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

pub(crate) async fn emit_runtime_profile_config_updated(state: &AppState, profile_id: &str) {
    let (shutdown_available, _) = system_shutdown_capability(&state.config).await;
    let (shutdown_after_queue_completes, scheduled_shutdown) =
        match with_ui_state_read(state, profile_id, |ui_state| {
            Ok((
                ui_state
                    .get("global")
                    .and_then(|value| value.get("shutdownAfterQueueCompletes"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                ui_state
                    .get("global")
                    .and_then(|value| value.get("scheduledShutdown"))
                    .cloned()
                    .unwrap_or(Value::Null),
            ))
        })
        .await
        {
            Ok(values) => values,
            Err(_) => return,
        };

    let next_scheduled_shutdown = if shutdown_available
        && scheduled_shutdown
            .get("scheduledFor")
            .and_then(Value::as_u64)
            .is_some_and(|value| value > now_unix_ms())
    {
        scheduled_shutdown
    } else {
        Value::Null
    };
    let paused_queues = list_resume_pending_queues_payload(state, profile_id)
        .await
        .unwrap_or_else(|_| json!([]));

    emit_profile_config_updated(
        state,
        profile_id,
        json!({
            "systemShutdown": {
                "available": shutdown_available,
                "delaySeconds": state.config.system_shutdown_delay_seconds,
                "armed": shutdown_available
                    && state.config.system_shutdown_enabled
                    && shutdown_after_queue_completes
            },
            "startup": {
                "pausedQueues": paused_queues,
                "scheduledShutdown": next_scheduled_shutdown
            }
        }),
    )
    .await;
}

fn notification_thread_id(method: &str, params: &Value) -> Option<String> {
    params
        .get("threadId")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            method
                .starts_with("thread/")
                .then(|| {
                    params
                        .get("thread")
                        .and_then(|thread| thread.get("id"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .flatten()
        })
}

fn notification_turn_id(params: &Value) -> Option<String> {
    params
        .get("turnId")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            params
                .get("turn")
                .and_then(|turn| turn.get("id"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

fn pending_request_id(raw_id: &Value) -> String {
    raw_id
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| raw_id.to_string())
}

async fn set_session_highlight(
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
        emit_session_summary_updated(state, profile_id, session_id, None).await;
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

pub(crate) async fn handle_profile_runtime_notification(
    state: &AppState,
    profile_id: &str,
    notification: &AppServerNotification,
) {
    let Some(session_id) = notification_thread_id(&notification.method, &notification.params)
    else {
        return;
    };
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    let runtime_key = runtime_session_key(&resolved_profile_id, &session_id);

    match notification.method.as_str() {
        "turn/started" => {
            if let Some(turn_id) = notification_turn_id(&notification.params) {
                state.active_turns.lock().await.insert(runtime_key, turn_id);
            }
            cancel_scheduled_shutdown_for_activity(state, profile_id).await;
            set_session_highlight(state, profile_id, &session_id, None).await;
        }
        "turn/completed" => {
            let turn_id = notification_turn_id(&notification.params);
            let mut active_turns = state.active_turns.lock().await;
            if turn_id
                .as_ref()
                .is_none_or(|turn_id| active_turns.get(&runtime_key) == Some(turn_id))
            {
                active_turns.remove(&runtime_key);
            }
            drop(active_turns);
            maybe_drain_queue(state, profile_id, &session_id).await;
            maybe_schedule_global_shutdown(state, profile_id, turn_id.as_deref()).await;
            emit_profile_global_notification(
                state,
                profile_id,
                json!({
                    "kind": "notification",
                    "method": "codex-webui/sessionAttention",
                    "params": {
                        "sessionId": session_id,
                        "reason": "completed"
                    }
                }),
            )
            .await;
            enqueue_profile_notification(
                state,
                profile_id,
                "sessionCompleted",
                Some(&session_id),
                json!({
                    "turnId": turn_id.clone().map(Value::String).unwrap_or(Value::Null)
                }),
            )
            .await;
            set_session_highlight(
                state,
                profile_id,
                &session_id,
                Some(json!({
                    "kind": "completed",
                    "at": now_unix_ms()
                })),
            )
            .await;
        }
        "thread/status/changed" => {
            let status = normalized_thread_status(notification.params.get("status"))
                .unwrap_or_else(|| "unknown".to_string());
            if is_live_thread_status(&status) {
                cancel_scheduled_shutdown_for_activity(state, profile_id).await;
            } else {
                state.active_turns.lock().await.remove(&runtime_key);
                maybe_drain_queue(state, profile_id, &session_id).await;
                maybe_schedule_global_shutdown(state, profile_id, None).await;
            }
        }
        "thread/archived" | "thread/unarchived" => {
            emit_profile_global_notification(
                state,
                profile_id,
                json!({
                    "kind": "notification",
                    "method": "codex-webui/sessionListsInvalidated",
                    "params": {
                        "threadId": session_id,
                        "archived": notification.method == "thread/archived"
                    }
                }),
            )
            .await;
        }
        _ => {}
    }

    if let Some(event) = map_app_server_session_notification(notification) {
        emit_session_notification(state, profile_id, &session_id, event).await;
    }

    if matches!(
        notification.method.as_str(),
        "turn/started" | "turn/completed" | "thread/name/updated" | "thread/status/changed"
    ) {
        emit_session_summary_updated(state, profile_id, &session_id, None).await;
    }
}

async fn restore_persisted_shutdown_state(state: &AppState, profile_id: &str) -> ApiResult<()> {
    let (shutdown_after_queue_completes, scheduled_shutdown) =
        with_ui_state_read(state, profile_id, |ui_state| {
            Ok((
                ui_state
                    .get("global")
                    .and_then(|value| value.get("shutdownAfterQueueCompletes"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                ui_state
                    .get("global")
                    .and_then(|value| value.get("scheduledShutdown"))
                    .cloned()
                    .unwrap_or(Value::Null),
            ))
        })
        .await?;

    let (shutdown_available, _) = system_shutdown_capability(&state.config).await;
    if !state.config.system_shutdown_enabled || !shutdown_available {
        with_ui_state_write(state, profile_id, |ui_state| {
            let Some(global) = ui_state.get_mut("global").and_then(Value::as_object_mut) else {
                return Err(api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "global state is missing",
                ));
            };
            global.insert("shutdownAfterQueueCompletes".to_string(), json!(false));
            global.insert("scheduledShutdown".to_string(), Value::Null);
            Ok(())
        })
        .await?;
        return Ok(());
    }

    if scheduled_shutdown
        .get("scheduledFor")
        .and_then(Value::as_u64)
        .is_some_and(|value| value > now_unix_ms())
    {
        arm_scheduled_shutdown(state, profile_id, scheduled_shutdown).await;
    } else if shutdown_after_queue_completes {
        maybe_schedule_global_shutdown(state, profile_id, None).await;
    }

    Ok(())
}

pub(crate) async fn restore_runtime_profile_state(state: AppState, profile_id: String) {
    if let Err(error) = mark_queues_pending_resume_payload(&state, &profile_id).await {
        warn!("failed to mark queued sessions as pending resume for {profile_id}: {error}");
    }
    if let Err(error) = restore_persisted_shutdown_state(&state, &profile_id).await {
        warn!("failed to restore shutdown state for {profile_id}: {error}");
    }
    emit_runtime_profile_config_updated(&state, &profile_id).await;

    loop {
        let client = match app_server_client(&state, &profile_id).await {
            Ok(client) => client,
            Err(error) => {
                warn!("failed to create app-server client for {profile_id}: {error:#}");
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        let _ = client
            .request("model/list", json!({ "includeHidden": false }))
            .await;
        let mut notifications = client.subscribe_notifications();
        let mut requests = client.subscribe_requests();

        loop {
            tokio::select! {
                notification = notifications.recv() => match notification {
                    Ok(notification) => {
                        handle_profile_runtime_notification(&state, &profile_id, &notification).await;
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(
                            "runtime app-server relay lagged for {profile_id}: skipped {skipped} messages"
                        );
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                request = requests.recv() => match request {
                    Ok(request) => {
                        handle_profile_server_request(&state, &profile_id, &request).await;
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(
                            "runtime app-server request relay lagged for {profile_id}: skipped {skipped} messages"
                        );
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
