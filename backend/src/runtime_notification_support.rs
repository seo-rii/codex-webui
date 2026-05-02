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

    let (enabled, notification_settings) = match with_ui_state_read(state, profile_id, |ui_state| {
        let notification_settings = normalize_notification_settings_value(
            ui_state
                .get("notifications")
                .and_then(|value| value.get("settings")),
        );
        let enabled_event_types = notification_settings
            .get("enabledEventTypes")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| {
                default_notification_settings_value()["enabledEventTypes"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            });
        Ok((
            enabled_event_types
                .iter()
                .filter_map(Value::as_str)
                .any(|entry| entry == notification_type),
            notification_settings,
        ))
    })
    .await
    {
        Ok(result) => result,
        Err(_) => (false, default_notification_settings_value()),
    };
    if !enabled {
        return;
    }

    let session_name = if let Some(session_id) = session_id {
        build_session_summary_payload(state, profile_id, session_id, None, None)
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

    let delivery_state = state.clone();
    let delivery_profile_id = profile_id.to_string();
    tokio::spawn(async move {
        deliver_notification_webhooks(
            delivery_state,
            delivery_profile_id,
            notification,
            notification_settings,
        )
        .await;
    });
}

pub(crate) fn notification_webhook_deliveries(
    notification: &Value,
    settings: &Value,
) -> Vec<(String, Value)> {
    let mut deliveries = Vec::new();
    if let Some(url) = settings
        .get("webhookUrl")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        deliveries.push((
            url.to_string(),
            json!({
                "event": notification.get("type").cloned().unwrap_or(Value::Null),
                "notification": notification
            }),
        ));
    }
    if let Some(url) = settings
        .get("slackWebhookUrl")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let event_type = notification
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("notification");
        let session_name = notification
            .get("sessionName")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("Codex WebUI");
        deliveries.push((
            url.to_string(),
            json!({
                "text": format!("{event_type}: {session_name}"),
                "codexWebui": {
                    "event": event_type,
                    "notification": notification
                }
            }),
        ));
    }
    deliveries
}

async fn deliver_notification_webhooks(
    state: AppState,
    profile_id: String,
    notification: Value,
    settings: Value,
) {
    for (url, payload) in notification_webhook_deliveries(&notification, &settings) {
        let field = if settings
            .get("slackWebhookUrl")
            .and_then(Value::as_str)
            .is_some_and(|candidate| candidate.trim() == url)
        {
            "slackWebhookUrl"
        } else {
            "webhookUrl"
        };
        if let Err(error) = validate_notification_webhook_resolves_public_str(&url, field).await {
            append_runtime_error_log(
                &state.config,
                "notification-webhook",
                "webhook delivery skipped invalid URL",
                json!({
                    "profileId": profile_id,
                    "notificationId": notification.get("id").cloned().unwrap_or(Value::Null),
                    "eventType": notification.get("type").cloned().unwrap_or(Value::Null),
                    "field": field,
                    "error": redact_user_facing_error(&error.message)
                }),
            );
            continue;
        }
        if let Err(error) = send_notification_webhook_with_retries(&state, &url, &payload).await {
            append_runtime_error_log(
                &state.config,
                "notification-webhook",
                "webhook delivery failed",
                json!({
                    "profileId": profile_id,
                    "notificationId": notification.get("id").cloned().unwrap_or(Value::Null),
                    "eventType": notification.get("type").cloned().unwrap_or(Value::Null),
                    "url": redact_user_facing_error(&url),
                    "error": redact_user_facing_error(&error.to_string())
                }),
            );
        }
    }
}

async fn send_notification_webhook_with_retries(
    state: &AppState,
    url: &str,
    payload: &Value,
) -> Result<()> {
    let mut last_error: Option<anyhow::Error> = None;
    for (attempt, delay_ms) in [0_u64, 500, 2_000].into_iter().enumerate() {
        if delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }
        match state.http.post(url).json(payload).send().await {
            Ok(response) if response.status().is_success() => return Ok(()),
            Ok(response) => {
                last_error = Some(anyhow!(
                    "webhook returned HTTP {} on attempt {}",
                    response.status(),
                    attempt + 1
                ));
            }
            Err(error) => {
                last_error = Some(anyhow!(
                    "webhook request failed on attempt {}: {}",
                    attempt + 1,
                    error
                ));
            }
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow!("webhook delivery failed")))
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

async fn session_has_cached_runtime_activity(state: &AppState, runtime_key: &str) -> bool {
    state.active_turns.lock().await.contains_key(runtime_key)
        || state.pending_turn_starts.lock().await.contains(runtime_key)
}

pub(crate) async fn set_runtime_session_status(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    status: &str,
) {
    let _ = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(runtime_status_by_thread_id) = ui_state
            .get_mut("runtimeStatusByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "runtime status state is missing",
            ));
        };
        runtime_status_by_thread_id.insert(
            session_id.to_string(),
            json!({
                "status": status,
                "updatedAt": now_unix_ms()
            }),
        );
        Ok(())
    })
    .await;
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
            state.pending_turn_starts.lock().await.remove(&runtime_key);
            if let Some(turn_id) = notification_turn_id(&notification.params) {
                state
                    .active_turns
                    .lock()
                    .await
                    .insert(runtime_key.clone(), turn_id);
            }
            set_runtime_session_status(state, profile_id, &session_id, "running").await;
            cancel_scheduled_shutdown_for_activity(state, profile_id).await;
            set_session_highlight(state, profile_id, &session_id, None).await;
        }
        "turn/completed" => {
            state.pending_turn_starts.lock().await.remove(&runtime_key);
            let turn_id = notification_turn_id(&notification.params);
            let mut active_turns = state.active_turns.lock().await;
            if turn_id
                .as_ref()
                .is_none_or(|turn_id| active_turns.get(&runtime_key) == Some(turn_id))
            {
                active_turns.remove(&runtime_key);
            }
            drop(active_turns);
            let still_active = session_has_cached_runtime_activity(state, &runtime_key).await;
            set_runtime_session_status(
                state,
                profile_id,
                &session_id,
                if still_active { "running" } else { "completed" },
            )
            .await;
            if still_active {
                set_session_highlight(state, profile_id, &session_id, None).await;
            } else {
                spawn_queue_drain(state, profile_id, &session_id);
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
                if session_stream_has_subscribers(state, profile_id, &session_id).await {
                    set_session_highlight(state, profile_id, &session_id, None).await;
                } else {
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
            }
        }
        "thread/status/changed" => {
            let status = normalized_thread_status(notification.params.get("status"))
                .unwrap_or_else(|| "unknown".to_string());
            if is_live_thread_status(&status) {
                state.pending_turn_starts.lock().await.remove(&runtime_key);
                set_runtime_session_status(state, profile_id, &session_id, "running").await;
                cancel_scheduled_shutdown_for_activity(state, profile_id).await;
            } else {
                let still_active = session_has_cached_runtime_activity(state, &runtime_key).await;
                if still_active {
                    set_runtime_session_status(state, profile_id, &session_id, "running").await;
                } else {
                    state.pending_turn_starts.lock().await.remove(&runtime_key);
                    state.active_turns.lock().await.remove(&runtime_key);
                    set_runtime_session_status(state, profile_id, &session_id, &status).await;
                    spawn_queue_drain(state, profile_id, &session_id);
                    maybe_schedule_global_shutdown(state, profile_id, None).await;
                }
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

    if let Some(mut event) = map_app_server_session_notification(notification) {
        if notification.method == "thread/status/changed"
            && normalized_thread_status(notification.params.get("status"))
                .as_deref()
                .is_some_and(|status| !is_live_thread_status(status))
            && session_has_cached_runtime_activity(state, &runtime_key).await
        {
            if let Some(params) = event.get_mut("params").and_then(Value::as_object_mut) {
                params.insert("status".to_string(), Value::String("running".to_string()));
            }
        }
        emit_session_notification(state, profile_id, &session_id, event).await;
    }

    if matches!(
        notification.method.as_str(),
        "turn/started" | "turn/completed" | "thread/name/updated" | "thread/status/changed"
    ) {
        let status_override = match notification.method.as_str() {
            "turn/started" => Some("running".to_string()),
            "turn/completed" => Some(
                if session_has_cached_runtime_activity(state, &runtime_key).await {
                    "running"
                } else {
                    "completed"
                }
                .to_string(),
            ),
            "thread/status/changed" => {
                let status = normalized_thread_status(notification.params.get("status"));
                if status
                    .as_deref()
                    .is_some_and(|status| !is_live_thread_status(status))
                    && session_has_cached_runtime_activity(state, &runtime_key).await
                {
                    Some("running".to_string())
                } else {
                    status
                }
            }
            _ => None,
        };
        emit_session_summary_updated(
            state,
            profile_id,
            &session_id,
            None,
            status_override.as_deref(),
        )
        .await;
    }
}

pub(crate) async fn restore_persisted_shutdown_state(
    state: &AppState,
    profile_id: &str,
) -> ApiResult<()> {
    let (shutdown_after_queue_completes, shutdown_primed, scheduled_shutdown) =
        with_ui_state_read(state, profile_id, |ui_state| {
            Ok((
                ui_state
                    .get("global")
                    .and_then(|value| value.get("shutdownAfterQueueCompletes"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                ui_state
                    .get("global")
                    .and_then(|value| value.get("shutdownAfterQueueCompletesPrimed"))
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
            global.insert(
                "shutdownAfterQueueCompletesPrimed".to_string(),
                json!(false),
            );
            global.insert("scheduledShutdown".to_string(), Value::Null);
            Ok(())
        })
        .await?;
        return Ok(());
    }

    let has_work_now = has_outstanding_queued_work(state, profile_id).await
        || has_active_work_across_threads(state, profile_id).await;
    if shutdown_after_queue_completes && !shutdown_primed && has_work_now {
        with_ui_state_write(state, profile_id, |ui_state| {
            let Some(global) = ui_state.get_mut("global").and_then(Value::as_object_mut) else {
                return Err(api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "global state is missing",
                ));
            };
            global.insert("shutdownAfterQueueCompletesPrimed".to_string(), json!(true));
            Ok(())
        })
        .await?;
    }

    if scheduled_shutdown
        .get("scheduledFor")
        .and_then(Value::as_u64)
        .is_some_and(|value| value > now_unix_ms())
    {
        arm_scheduled_shutdown(state, profile_id, scheduled_shutdown).await;
    } else if shutdown_after_queue_completes && has_work_now {
        maybe_schedule_global_shutdown(state, profile_id, None).await;
    } else if shutdown_after_queue_completes && shutdown_primed {
        with_ui_state_write(state, profile_id, |ui_state| {
            let Some(global) = ui_state.get_mut("global").and_then(Value::as_object_mut) else {
                return Err(api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "global state is missing",
                ));
            };
            global.insert(
                "shutdownAfterQueueCompletesPrimed".to_string(),
                json!(false),
            );
            global.insert("scheduledShutdown".to_string(), Value::Null);
            Ok(())
        })
        .await?;
        emit_runtime_profile_config_updated(state, profile_id).await;
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

    let mut reconnect_delay = Duration::from_secs(1);
    loop {
        let client = match app_server_client(&state, &profile_id).await {
            Ok(client) => client,
            Err(error) => {
                warn!(
                    retry_after_ms = reconnect_delay.as_millis(),
                    "failed to create app-server client for {profile_id}: {error:#}"
                );
                tokio::time::sleep(reconnect_delay).await;
                reconnect_delay = reconnect_delay
                    .saturating_mul(2)
                    .min(Duration::from_secs(60));
                continue;
            }
        };
        let _ = client
            .request("model/list", json!({ "includeHidden": false }))
            .await;
        reconnect_delay = Duration::from_secs(1);
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

        tokio::time::sleep(reconnect_delay).await;
        reconnect_delay = reconnect_delay
            .saturating_mul(2)
            .min(Duration::from_secs(60));
    }
}
