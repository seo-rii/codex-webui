use super::*;

const RUNTIME_ACTIVITY_RECONCILE_INTERVAL_SECS: u64 = 15;
const RUNTIME_ACTIVITY_RECONCILE_LIMIT: usize = 8;
const RUNTIME_ACTIVITY_RECONCILE_TIMEOUT_MS: u64 = 1_000;

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
        let pinned_addrs = match resolve_notification_webhook_public_addrs(
            &state.config,
            &url,
            field,
        )
        .await
        {
            Ok(pinned_addrs) => pinned_addrs,
            Err(error) => {
                record_notification_webhook_failure(
                    &state,
                    &profile_id,
                    &notification,
                    field,
                    &error.message,
                )
                .await;
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
        };
        if let Err(error) =
            send_notification_webhook_with_retries(&state, &url, &payload, pinned_addrs).await
        {
            record_notification_webhook_failure(
                &state,
                &profile_id,
                &notification,
                field,
                &error.to_string(),
            )
            .await;
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
    pinned_addrs: Option<(String, Vec<std::net::SocketAddr>)>,
) -> Result<()> {
    let http = if let Some((host, addrs)) = pinned_addrs {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .resolve_to_addrs(&host, &addrs)
            .build()
            .context("failed to build pinned webhook client")?
    } else {
        state.http.clone()
    };
    let mut last_error: Option<anyhow::Error> = None;
    for (attempt, delay_ms) in [0_u64, 500, 2_000].into_iter().enumerate() {
        if delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }
        match http.post(url).json(payload).send().await {
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
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    let state_for_task = state.clone();
    let profile_id = profile_id.to_string();
    let handle = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(180)).await;
        let (shutdown_available, _) = system_shutdown_capability(&state_for_task.config).await;
        let (shutdown_after_queue_completes, scheduled_shutdown, scheduled_shutdown_blocked_reason) =
            match with_ui_state_read(&state_for_task, &profile_id, |ui_state| {
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
                    ui_state
                        .get("global")
                        .and_then(|value| value.get("scheduledShutdownBlockedReason"))
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
        let paused_queues = list_resume_pending_queues_payload(&state_for_task, &profile_id)
            .await
            .unwrap_or_else(|_| json!([]));

        emit_profile_config_updated(
            &state_for_task,
            &profile_id,
            json!({
                "systemShutdown": {
                    "available": shutdown_available,
                    "delaySeconds": state_for_task.config.system_shutdown_delay_seconds,
                    "armed": shutdown_available
                        && state_for_task.config.system_shutdown_enabled
                        && shutdown_after_queue_completes
                },
                "startup": {
                    "pausedQueues": paused_queues,
                    "scheduledShutdown": next_scheduled_shutdown,
                    "scheduledShutdownBlockedReason": scheduled_shutdown_blocked_reason
                }
            }),
        )
        .await;
    });
    let mut tasks = state.runtime_config_update_tasks.lock().await;
    if let Some(existing) = tasks.insert(resolved_profile_id, handle) {
        existing.abort();
    }
}

fn notification_thread_id(method: &str, params: &Value) -> Option<String> {
    params
        .get("threadId")
        .or_else(|| params.get("thread_id"))
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

fn notification_thread_name(params: &Value) -> Option<String> {
    ["threadName", "thread_name", "name", "title"]
        .iter()
        .find_map(|key| params.get(*key).and_then(value_text))
        .or_else(|| {
            params.get("thread").and_then(|thread| {
                ["threadName", "thread_name", "name", "title"]
                    .iter()
                    .find_map(|key| thread.get(*key).and_then(value_text))
            })
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

async fn mark_runtime_session_attention(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    attention_reason: &str,
    reason: &str,
) {
    let _ = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(highlights_by_thread_id) = ui_state
            .get_mut("highlightsByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "highlight state is missing",
            ));
        };
        highlights_by_thread_id.insert(
            session_id.to_string(),
            json!({
                "kind": "attention",
                "at": now_unix_ms(),
                "reason": attention_reason
            }),
        );
        Ok(())
    })
    .await;
    emit_profile_global_notification(
        state,
        profile_id,
        json!({
            "kind": "notification",
            "method": "codex-webui/sessionAttention",
            "params": {
                "sessionId": session_id,
                "reason": attention_reason,
                "message": reason
            }
        }),
    )
    .await;
    let notification_type = "sessionAttention";
    let enabled = with_ui_state_read(state, profile_id, |ui_state| {
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
        Ok(enabled_event_types
            .iter()
            .filter_map(Value::as_str)
            .any(|entry| entry == notification_type))
    })
    .await
    .unwrap_or(false);
    if !enabled {
        return;
    }

    let notification = json!({
        "id": Uuid::new_v4().to_string(),
        "type": notification_type,
        "createdAt": now_unix_ms(),
        "readAt": Value::Null,
        "sessionId": session_id,
        "sessionName": Value::Null,
        "payload": {
            "reason": attention_reason,
            "message": reason
        }
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

pub(crate) async fn clear_profile_runtime_activity_after_app_server_exit(
    state: &AppState,
    profile_id: &str,
    reason: Option<&str>,
) -> Vec<String> {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    let runtime_key_prefix = format!("profile::{resolved_profile_id}::session-runtime::");
    let now_ms = now_unix_ms();
    let reason_value = reason
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(Value::from)
        .unwrap_or(Value::Null);
    let mut affected_session_ids = HashSet::new();

    {
        let mut active_turns = state.active_turns.lock().await;
        let stale_keys = active_turns
            .keys()
            .filter(|key| key.starts_with(&runtime_key_prefix))
            .cloned()
            .collect::<Vec<_>>();
        for key in stale_keys {
            active_turns.remove(&key);
            if let Some(session_id) = key.strip_prefix(&runtime_key_prefix) {
                affected_session_ids.insert(session_id.to_string());
            }
        }
    }
    {
        let mut pending_turn_starts = state.pending_turn_starts.lock().await;
        let stale_keys = pending_turn_starts
            .iter()
            .filter(|key| key.starts_with(&runtime_key_prefix))
            .cloned()
            .collect::<Vec<_>>();
        for key in stale_keys {
            pending_turn_starts.remove(&key);
            if let Some(session_id) = key.strip_prefix(&runtime_key_prefix) {
                affected_session_ids.insert(session_id.to_string());
            }
        }
    }

    let runtime_status_session_ids = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(runtime_status_by_thread_id) = ui_state
            .get_mut("runtimeStatusByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "runtime status state is missing",
            ));
        };
        let mut changed_session_ids = Vec::new();
        for (session_id, status_value) in runtime_status_by_thread_id.iter_mut() {
            let status = normalized_thread_status(Some(status_value));
            if !status.as_deref().is_some_and(is_live_thread_status) {
                continue;
            }
            *status_value = json!({
                "status": "failed",
                "updatedAt": now_ms,
                "reason": reason_value.clone(),
            });
            changed_session_ids.push(session_id.clone());
        }
        Ok(changed_session_ids)
    })
    .await
    .unwrap_or_default();
    affected_session_ids.extend(runtime_status_session_ids);

    let affected_session_ids = affected_session_ids.into_iter().collect::<Vec<_>>();
    for session_id in &affected_session_ids {
        clear_session_pending_requests(state, profile_id, session_id).await;
        complete_active_automation_runs_for_session(
            state,
            profile_id,
            session_id,
            "failed",
            Some(reason_value.as_str().unwrap_or("codex app-server exited")),
        )
        .await;
        emit_session_notification(
            state,
            profile_id,
            session_id,
            json!({
                "kind": "notification",
                "method": "thread/status/changed",
                "params": {
                    "threadId": session_id,
                    "status": "failed",
                    "reason": reason_value.clone(),
                }
            }),
        )
        .await;
        mark_runtime_session_attention(
            state,
            profile_id,
            session_id,
            "stopped",
            reason_value.as_str().unwrap_or("codex app-server exited"),
        )
        .await;
    }
    affected_session_ids
}

pub(crate) async fn clear_runtime_activity_after_app_server_client_exit(
    state: &AppState,
    profile_id: &str,
    client_key: &str,
    reason: Option<&str>,
) -> Vec<String> {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    let now_ms = now_unix_ms();
    let reason_value = reason
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(Value::from)
        .unwrap_or(Value::Null);
    let mut affected_session_ids =
        session_ids_for_app_server_client(state, &resolved_profile_id, client_key).await;

    {
        let mut active_turns = state.active_turns.lock().await;
        for session_id in &affected_session_ids {
            active_turns.remove(&runtime_session_key(&resolved_profile_id, session_id));
        }
    }
    {
        let mut pending_turn_starts = state.pending_turn_starts.lock().await;
        for session_id in &affected_session_ids {
            pending_turn_starts.remove(&runtime_session_key(&resolved_profile_id, session_id));
        }
    }

    let status_session_ids = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(runtime_status_by_thread_id) = ui_state
            .get_mut("runtimeStatusByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "runtime status state is missing",
            ));
        };
        let mut changed_session_ids = Vec::new();
        for session_id in &affected_session_ids {
            let Some(status_value) = runtime_status_by_thread_id.get_mut(session_id) else {
                continue;
            };
            let status = normalized_thread_status(Some(status_value));
            if !status.as_deref().is_some_and(is_live_thread_status) {
                continue;
            }
            *status_value = json!({
                "status": "failed",
                "updatedAt": now_ms,
                "reason": reason_value.clone(),
            });
            changed_session_ids.push(session_id.clone());
        }
        Ok(changed_session_ids)
    })
    .await
    .unwrap_or_default();
    affected_session_ids.extend(status_session_ids);

    let mut affected_session_ids = affected_session_ids.into_iter().collect::<Vec<_>>();
    affected_session_ids.sort();
    clear_app_server_assignments_for_sessions(state, profile_id, &affected_session_ids).await;
    for session_id in &affected_session_ids {
        clear_session_pending_requests(state, profile_id, session_id).await;
        complete_active_automation_runs_for_session(
            state,
            profile_id,
            session_id,
            "failed",
            Some(reason_value.as_str().unwrap_or("codex app-server exited")),
        )
        .await;
        emit_session_notification(
            state,
            profile_id,
            session_id,
            json!({
                "kind": "notification",
                "method": "thread/status/changed",
                "params": {
                    "threadId": session_id,
                    "status": "failed",
                    "reason": reason_value.clone(),
                }
            }),
        )
        .await;
        mark_runtime_session_attention(
            state,
            profile_id,
            session_id,
            "stopped",
            reason_value.as_str().unwrap_or("codex app-server exited"),
        )
        .await;
    }
    affected_session_ids
}

pub(crate) async fn clear_stale_session_runtime_activity_if_app_server_missing(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    stale_after_ms: u64,
    reason: &str,
) -> bool {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id).0;
    let client_key = app_server_client_key_for_session(state, profile_id, session_id).await;
    if state
        .app_servers
        .client_key_has_active_process(&resolved_profile_id, &client_key)
        .await
    {
        return false;
    }

    let status_is_stale =
        with_ui_state_read(state, profile_id, |ui_state| {
            Ok(ui_state
                .get("runtimeStatusByThreadId")
                .and_then(Value::as_object)
                .and_then(|entries| entries.get(session_id))
                .is_some_and(|status| {
                    normalized_thread_status(Some(status))
                        .as_deref()
                        .is_some_and(is_live_thread_status)
                        && status.get("updatedAt").and_then(Value::as_u64).is_some_and(
                            |updated_at| now_unix_ms().saturating_sub(updated_at) >= stale_after_ms,
                        )
                }))
        })
        .await
        .unwrap_or(false);
    if !status_is_stale {
        return false;
    }

    clear_app_server_assignments_for_sessions(state, profile_id, &[session_id.to_string()]).await;
    mark_runtime_session_terminal_after_reconcile(state, profile_id, session_id, "failed", reason)
        .await;
    true
}

async fn cached_runtime_session_ids(
    state: &AppState,
    profile_id: &str,
    limit: usize,
) -> Vec<String> {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    let runtime_key_prefix = format!("profile::{resolved_profile_id}::session-runtime::");
    let mut session_ids = HashSet::new();
    {
        let active_turns = state.active_turns.lock().await;
        for key in active_turns.keys() {
            let Some(session_id) = key.strip_prefix(&runtime_key_prefix) else {
                continue;
            };
            session_ids.insert(session_id.to_string());
            if session_ids.len() >= limit {
                return session_ids.into_iter().collect();
            }
        }
    }
    {
        let pending_turn_starts = state.pending_turn_starts.lock().await;
        for key in pending_turn_starts.iter() {
            let Some(session_id) = key.strip_prefix(&runtime_key_prefix) else {
                continue;
            };
            session_ids.insert(session_id.to_string());
            if session_ids.len() >= limit {
                return session_ids.into_iter().collect();
            }
        }
    }
    let live_status_session_ids = with_ui_state_read(state, profile_id, |ui_state| {
        Ok(ui_state
            .get("runtimeStatusByThreadId")
            .and_then(Value::as_object)
            .map(|statuses| {
                statuses
                    .iter()
                    .filter_map(|(session_id, status_value)| {
                        normalized_thread_status(Some(status_value))
                            .as_deref()
                            .is_some_and(is_live_thread_status)
                            .then_some(session_id.clone())
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default())
    })
    .await
    .unwrap_or_default();
    for session_id in live_status_session_ids {
        session_ids.insert(session_id);
        if session_ids.len() >= limit {
            break;
        }
    }
    session_ids.into_iter().collect()
}

async fn mark_runtime_session_terminal_after_reconcile(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    status: &str,
    reason: &str,
) {
    let runtime_key = runtime_session_key(
        &resolve_runtime_profile_entry(&state.config, profile_id).0,
        session_id,
    );
    state.active_turns.lock().await.remove(&runtime_key);
    state.pending_turn_starts.lock().await.remove(&runtime_key);
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
                "updatedAt": now_unix_ms(),
                "reason": reason
            }),
        );
        Ok(())
    })
    .await;
    let (automation_status, automation_error) = automation_status_for_thread_status(status);
    let automation_error = if status == "completed" {
        None
    } else {
        automation_error.as_deref().or(Some(reason))
    };
    complete_active_automation_runs_for_session(
        state,
        profile_id,
        session_id,
        &automation_status,
        automation_error,
    )
    .await;
    clear_session_pending_requests(state, profile_id, session_id).await;
    emit_session_notification(
        state,
        profile_id,
        session_id,
        json!({
            "kind": "notification",
            "method": "thread/status/changed",
            "params": {
                "threadId": session_id,
                "status": status,
                "reason": reason
            }
        }),
    )
    .await;
    if status == "completed" {
        spawn_queue_drain(state, profile_id, session_id);
        maybe_schedule_global_shutdown(state, profile_id, None).await;
    } else {
        mark_runtime_session_attention(state, profile_id, session_id, "failed", reason).await;
    }
    emit_session_summary_updated(state, profile_id, session_id, None, Some(status)).await;
}

pub(crate) async fn reconcile_lost_runtime_activity_for_profile(
    state: &AppState,
    profile_id: &str,
) -> Vec<String> {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    if !state
        .app_servers
        .profile_has_active_process(&resolved_profile_id)
        .await
    {
        let affected_session_ids = clear_profile_runtime_activity_after_app_server_exit(
            state,
            profile_id,
            Some("codex app-server is not running"),
        )
        .await;
        for session_id in &affected_session_ids {
            emit_session_summary_updated(state, profile_id, session_id, None, Some("failed")).await;
        }
        return affected_session_ids;
    }

    let session_ids =
        cached_runtime_session_ids(state, profile_id, RUNTIME_ACTIVITY_RECONCILE_LIMIT).await;
    if session_ids.is_empty() {
        return Vec::new();
    }
    let mut reconciled = Vec::new();
    for session_id in session_ids {
        if clear_stale_session_runtime_activity_if_app_server_missing(
            state,
            profile_id,
            &session_id,
            RUNTIME_ACTIVITY_RECONCILE_INTERVAL_SECS.saturating_mul(1_000),
            "codex app-server is not running",
        )
        .await
        {
            reconciled.push(session_id);
            continue;
        }
        let client = match app_server_client_for_session(state, profile_id, &session_id).await {
            Ok(client) => client,
            Err(_) => continue,
        };
        let response = client
            .request_with_timeout(
                "thread/read",
                json!({
                    "threadId": session_id,
                    "includeTurns": false
                }),
                Duration::from_millis(RUNTIME_ACTIVITY_RECONCILE_TIMEOUT_MS),
                false,
            )
            .await;
        let Some(thread) = response
            .ok()
            .and_then(|response| response.get("thread").cloned())
        else {
            continue;
        };
        let status =
            normalized_thread_status(thread.get("status")).unwrap_or_else(|| "unknown".to_string());
        if is_live_thread_status(&status) {
            continue;
        }
        let terminal_status = if status == "completed" {
            "completed"
        } else {
            "failed"
        };
        let reason = if terminal_status == "completed" {
            "codex app-server reports the turn is complete"
        } else {
            "codex app-server no longer has an active turn"
        };
        mark_runtime_session_terminal_after_reconcile(
            state,
            profile_id,
            &session_id,
            terminal_status,
            reason,
        )
        .await;
        reconciled.push(session_id);
    }
    reconciled
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
        if let Some(event) = map_app_server_global_notification(notification) {
            emit_profile_global_notification(state, profile_id, event).await;
        }
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
                complete_active_automation_runs_for_session(
                    state,
                    profile_id,
                    &session_id,
                    "completed",
                    None,
                )
                .await;
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
            if let Some(turn) = notification.params.get("turn") {
                spawn_language_bridge_response_translation_for_completed_turn(
                    state,
                    profile_id,
                    &session_id,
                    turn,
                )
                .await;
            }
        }
        "thread/name/updated" => {
            let thread_name = notification_thread_name(&notification.params);
            if let Err(error) =
                save_session_title_metadata(state, profile_id, &session_id, thread_name.as_deref())
                    .await
            {
                warn!(
                    "failed to persist updated thread name for {profile_id}/{session_id}: {error}"
                );
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
                state.pending_turn_starts.lock().await.remove(&runtime_key);
                state.active_turns.lock().await.remove(&runtime_key);
                clear_session_pending_requests(state, profile_id, &session_id).await;
                set_runtime_session_status(state, profile_id, &session_id, &status).await;
                let (automation_status, automation_error) =
                    automation_status_for_thread_status(&status);
                complete_active_automation_runs_for_session(
                    state,
                    profile_id,
                    &session_id,
                    &automation_status,
                    automation_error.as_deref(),
                )
                .await;
                spawn_queue_drain(state, profile_id, &session_id);
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

    if let Some(mut event) = map_app_server_session_notification(notification) {
        if matches!(
            notification.method.as_str(),
            "thread/goal/updated" | "thread/goal/cleared"
        ) {
            let goal = event
                .get("params")
                .and_then(|params| params.get("goal"))
                .cloned()
                .unwrap_or(Value::Null);
            cache_session_goal_payload(state, profile_id, &session_id, &goal).await;
            emit_profile_global_notification(state, profile_id, event.clone()).await;
        }
        if notification.method == "turn/completed" {
            if let Some(turn) = event
                .get_mut("params")
                .and_then(Value::as_object_mut)
                .and_then(|params| params.get_mut("turn"))
            {
                let mut turns = vec![turn.clone()];
                if apply_language_bridge_translations_to_turns(
                    state,
                    profile_id,
                    &session_id,
                    &mut turns,
                )
                .await
                .is_ok()
                {
                    if let Some(translated_turn) = turns.into_iter().next() {
                        *turn = translated_turn;
                    }
                }
            }
        }
        emit_session_notification(state, profile_id, &session_id, event).await;
    }
    if let Some(frame_event) = map_app_server_computer_frame_notification(notification) {
        emit_session_notification(state, profile_id, &session_id, frame_event).await;
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
            "thread/status/changed" => normalized_thread_status(notification.params.get("status")),
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
            global.insert("scheduledShutdownBlockedReason".to_string(), Value::Null);
            Ok(())
        })
        .await?;
        return Ok(());
    }

    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id).0;
    let runtime_key_prefix = format!("profile::{resolved_profile_id}::");
    let has_cached_active_work = state
        .active_turns
        .lock()
        .await
        .keys()
        .any(|key| key.starts_with(&runtime_key_prefix))
        || state
            .pending_turn_starts
            .lock()
            .await
            .iter()
            .any(|key| key.starts_with(&runtime_key_prefix));
    let has_work_now = has_outstanding_queued_work(state, profile_id).await
        || if state.app_servers.active_process_count().await == 0 {
            has_cached_active_work
        } else {
            has_active_work_across_threads(state, profile_id).await
        };
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
            global.insert("scheduledShutdownBlockedReason".to_string(), Value::Null);
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
}

pub(crate) fn register_runtime_profile_monitor(
    state: &AppState,
    profile_id: &str,
    client_key: &str,
    mut notifications: broadcast::Receiver<AppServerNotification>,
    mut requests: broadcast::Receiver<backend::codex_app_server::AppServerRequest>,
) {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    let monitor_key = format!("{resolved_profile_id}::{client_key}");
    let Ok(mut monitors) = state.runtime_profile_monitors.lock() else {
        warn!("runtime profile monitor registry is poisoned for {resolved_profile_id}");
        return;
    };
    if monitors
        .get(&monitor_key)
        .is_some_and(|handle| !handle.is_finished())
    {
        return;
    }
    if let Some(handle) = monitors.remove(&monitor_key) {
        handle.abort();
    }

    let monitor_state = state.clone();
    let monitor_profile_id = resolved_profile_id.clone();
    let monitor_client_key = client_key.to_string();
    let handle = tokio::spawn(async move {
        let mut automation_reconcile_interval =
            tokio::time::interval(tokio::time::Duration::from_secs(60));
        automation_reconcile_interval
            .set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut runtime_activity_reconcile_interval = tokio::time::interval(
            tokio::time::Duration::from_secs(RUNTIME_ACTIVITY_RECONCILE_INTERVAL_SECS),
        );
        runtime_activity_reconcile_interval
            .set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        runtime_activity_reconcile_interval.tick().await;
        loop {
            tokio::select! {
                notification = notifications.recv() => match notification {
                    Ok(notification) => {
                        if notification.method == "codex-webui/app-server/exited" {
                            let affected_session_ids =
                                clear_runtime_activity_after_app_server_client_exit(
                                &monitor_state,
                                &monitor_profile_id,
                                &monitor_client_key,
                                notification.params.get("reason").and_then(Value::as_str),
                            )
                            .await;
                            for session_id in affected_session_ids {
                                emit_session_summary_updated(
                                    &monitor_state,
                                    &monitor_profile_id,
                                    &session_id,
                                    None,
                                    Some("failed"),
                                )
                                .await;
                            }
                        } else {
                            handle_profile_runtime_notification(
                                &monitor_state,
                                &monitor_profile_id,
                                &notification,
                            )
                            .await;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(
                            "runtime app-server relay lagged for {monitor_profile_id}: skipped {skipped} messages"
                        );
                        let reconciled = reconcile_lost_runtime_activity_for_profile(
                            &monitor_state,
                            &monitor_profile_id,
                        )
                        .await;
                        if !reconciled.is_empty() {
                            warn!(
                                "reconciled {} lost runtime session(s) after app-server relay lag for {monitor_profile_id}",
                                reconciled.len()
                            );
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                request = requests.recv() => match request {
                    Ok(request) => {
                        handle_profile_server_request(
                            &monitor_state,
                            &monitor_profile_id,
                            &monitor_client_key,
                            &request,
                        )
                            .await;
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(
                            "runtime app-server request relay lagged for {monitor_profile_id}: skipped {skipped} messages"
                        );
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                _ = automation_reconcile_interval.tick() => {
                    let reconciled = reconcile_stale_automation_runs_for_profile(
                        &monitor_state,
                        &monitor_profile_id,
                    )
                    .await;
                    if reconciled > 0 {
                        tracing::debug!(
                            "reconciled {reconciled} stale automation run(s) for {monitor_profile_id}"
                        );
                    }
                },
                _ = runtime_activity_reconcile_interval.tick() => {
                    let reconciled = reconcile_lost_runtime_activity_for_profile(
                        &monitor_state,
                        &monitor_profile_id,
                    )
                    .await;
                    if !reconciled.is_empty() {
                        warn!(
                            "reconciled {} lost runtime session(s) for {monitor_profile_id}",
                            reconciled.len()
                        );
                    }
                },
            }
        }
    });
    monitors.insert(monitor_key, handle);
}
