use super::*;

pub(crate) async fn with_queue_dispatch_guard<T, F>(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    work: F,
) -> Option<T>
where
    F: Future<Output = T>,
{
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    let key = runtime_session_key(&resolved_profile_id, session_id);
    {
        let mut current = state.queue_dispatching.lock().await;
        if current.contains(&key) {
            return None;
        }
        current.insert(key.clone());
    }

    let result = work.await;
    state.queue_dispatching.lock().await.remove(&key);
    Some(result)
}

pub(crate) async fn remove_session_queue_item_after_dispatch(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    queue_id: &str,
) -> ApiResult<Value> {
    with_ui_state_write(state, profile_id, |ui_state| {
        let Some(queues_by_thread_id) = ui_state
            .get_mut("queuesByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "queue state is missing",
            ));
        };
        let Some(existing) = queues_by_thread_id.get_mut(session_id) else {
            return Err(api_error(StatusCode::NOT_FOUND, "QUEUE_ITEM_NOT_FOUND"));
        };
        let Some(queue) = existing.as_object_mut() else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "queue state had an unexpected shape",
            ));
        };
        let Some(items) = queue.get_mut("items").and_then(Value::as_array_mut) else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "queue items are missing",
            ));
        };
        let previous_len = items.len();
        items.retain(|item| item.get("id").and_then(Value::as_str) != Some(queue_id));
        if items.len() == previous_len {
            return Err(api_error(StatusCode::NOT_FOUND, "QUEUE_ITEM_NOT_FOUND"));
        }

        if items.is_empty() {
            queues_by_thread_id.remove(session_id);
        } else {
            queue.insert("resumePending".to_string(), json!(false));
            queue.insert("updatedAt".to_string(), json!(now_unix_ms()));
        }
        Ok(())
    })
    .await?;

    let queue = get_session_queue_payload(state, profile_id, session_id).await?;
    emit_queue_updated(state, profile_id, session_id, Some(queue.clone())).await;
    Ok(queue)
}

pub(crate) async fn dispatch_queue_item(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    queued_item: &Value,
    mode: &str,
) -> ApiResult<()> {
    let prompt = queued_item
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let attachment_ids = queued_item
        .get("attachmentIds")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let selected_skills = queued_item
        .get("skills")
        .cloned()
        .unwrap_or_else(|| json!([]));

    if mode == "steer" {
        steer_turn_payload(
            state,
            profile_id,
            session_id,
            prompt,
            Some(&attachment_ids),
            Some(&selected_skills),
        )
        .await
        .map(|_| ())
    } else {
        send_turn_payload(
            state,
            profile_id,
            session_id,
            prompt,
            Some(&attachment_ids),
            Some(&selected_skills),
            json!({}),
        )
        .await
        .map(|_| ())
    }
}

pub(crate) async fn session_has_active_turn(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> bool {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id).0;
    if state
        .active_turns
        .lock()
        .await
        .contains_key(&runtime_session_key(resolved_profile_id, session_id))
    {
        return true;
    }

    let thread = match read_thread_payload(state, profile_id, session_id, true).await {
        Ok(payload) => payload,
        Err(_) => return true,
    };
    let Some(thread) = thread.as_object() else {
        return true;
    };
    if !is_live_thread_status(
        &normalized_thread_status(thread.get("status")).unwrap_or_else(|| "unknown".to_string()),
    ) {
        return false;
    }

    thread
        .get("turns")
        .and_then(Value::as_array)
        .is_some_and(|turns| {
            turns
                .iter()
                .any(|turn| turn.get("status").and_then(Value::as_str) == Some("inProgress"))
        })
}

pub(crate) async fn has_outstanding_queued_work(state: &AppState, profile_id: &str) -> bool {
    with_ui_state_read(state, profile_id, |ui_state| {
        Ok(ui_state
            .get("queuesByThreadId")
            .and_then(Value::as_object)
            .is_some_and(|queues| {
                queues.values().any(|queue| {
                    queue
                        .get("items")
                        .and_then(Value::as_array)
                        .is_some_and(|items| !items.is_empty())
                })
            }))
    })
    .await
    .unwrap_or(true)
}

pub(crate) async fn has_active_work_across_threads(state: &AppState, profile_id: &str) -> bool {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id).0;
    if state
        .active_turns
        .lock()
        .await
        .keys()
        .any(|key| key.starts_with(&format!("profile::{resolved_profile_id}::")))
    {
        return true;
    }

    let client = match app_server_client(state, profile_id).await {
        Ok(client) => client,
        Err(_) => return true,
    };
    let mut cursor: Option<String> = None;
    loop {
        let payload = match client
            .request(
                "thread/list",
                json!({
                    "limit": 200,
                    "archived": false,
                    "cursor": cursor
                }),
            )
            .await
        {
            Ok(payload) => payload,
            Err(_) => return true,
        };
        if payload
            .get("data")
            .and_then(Value::as_array)
            .is_some_and(|threads| {
                threads.iter().any(|thread| {
                    is_live_thread_status(
                        &normalized_thread_status(thread.get("status"))
                            .unwrap_or_else(|| "unknown".to_string()),
                    )
                })
            })
        {
            return true;
        }

        cursor = payload
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if cursor.is_none() {
            break;
        }
    }

    false
}

pub(crate) async fn clear_scheduled_shutdown(state: &AppState, profile_id: &str) {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    if let Some(handle) = state
        .shutdown_timers
        .lock()
        .await
        .remove(&resolved_profile_id)
    {
        handle.abort();
    }
    let _ = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(global) = ui_state.get_mut("global").and_then(Value::as_object_mut) else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "global state is missing",
            ));
        };
        global.insert("scheduledShutdown".to_string(), Value::Null);
        Ok(())
    })
    .await;
    emit_runtime_profile_config_updated(state, profile_id).await;
}

pub(crate) async fn cancel_scheduled_shutdown_for_activity(state: &AppState, profile_id: &str) {
    let scheduled = with_ui_state_read(state, profile_id, |ui_state| {
        Ok(ui_state
            .get("global")
            .and_then(|value| value.get("scheduledShutdown"))
            .cloned()
            .unwrap_or(Value::Null))
    })
    .await
    .unwrap_or(Value::Null);

    if !scheduled.is_null() {
        clear_scheduled_shutdown(state, profile_id).await;
    }
}

pub(crate) async fn execute_scheduled_shutdown(state: &AppState, profile_id: &str) {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    state
        .shutdown_timers
        .lock()
        .await
        .remove(&resolved_profile_id);
    let _ = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(global) = ui_state.get_mut("global").and_then(Value::as_object_mut) else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "global state is missing",
            ));
        };
        global.insert("scheduledShutdown".to_string(), Value::Null);
        Ok(())
    })
    .await;
    emit_runtime_profile_config_updated(state, profile_id).await;

    let (available, plan) = system_shutdown_capability(&state.config).await;
    let Some(plan) = plan.filter(|_| available) else {
        emit_profile_global_notification(
            state,
            profile_id,
            json!({
                "kind": "notification",
                "method": "codex-webui/shutdownFailed",
                "params": {
                    "message": "System shutdown is unavailable for this server user."
                }
            }),
        )
        .await;
        return;
    };

    let command = plan.command.clone();
    let args = plan.args.clone();
    if let Err(error) = Command::new(&command).args(&args).spawn() {
        emit_profile_global_notification(
            state,
            profile_id,
            json!({
                "kind": "notification",
                "method": "codex-webui/shutdownFailed",
                "params": {
                    "message": error.to_string()
                }
            }),
        )
        .await;
    }
}

pub(crate) async fn arm_scheduled_shutdown(
    state: &AppState,
    profile_id: &str,
    scheduled_shutdown: Value,
) {
    let Some(scheduled_for) = scheduled_shutdown
        .get("scheduledFor")
        .and_then(Value::as_u64)
    else {
        return;
    };
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    if let Some(handle) = state
        .shutdown_timers
        .lock()
        .await
        .remove(&resolved_profile_id)
    {
        handle.abort();
    }

    let delay_ms = scheduled_for.saturating_sub(now_unix_ms());
    let shutdown_state = state.clone();
    let shutdown_profile_id = profile_id.to_string();
    let handle = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        execute_scheduled_shutdown(&shutdown_state, &shutdown_profile_id).await;
    });
    state
        .shutdown_timers
        .lock()
        .await
        .insert(resolved_profile_id, handle);
}

pub(crate) async fn maybe_schedule_global_shutdown(
    state: &AppState,
    profile_id: &str,
    completed_turn_id: Option<&str>,
) {
    if !state.config.system_shutdown_enabled {
        return;
    }

    let (available, _) = system_shutdown_capability(&state.config).await;
    if !available {
        return;
    }

    let existing_scheduled = with_ui_state_read(state, profile_id, |ui_state| {
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
    .await;
    let Ok((shutdown_after_queue_completes, scheduled_shutdown)) = existing_scheduled else {
        return;
    };
    if !shutdown_after_queue_completes {
        return;
    }
    if scheduled_shutdown
        .get("scheduledFor")
        .and_then(Value::as_u64)
        .is_some_and(|value| value > now_unix_ms())
    {
        arm_scheduled_shutdown(state, profile_id, scheduled_shutdown).await;
        return;
    }
    if has_outstanding_queued_work(state, profile_id).await
        || has_active_work_across_threads(state, profile_id).await
    {
        return;
    }

    let scheduled_shutdown = json!({
        "sessionId": Value::Null,
        "scheduledFor": now_unix_ms() + state.config.system_shutdown_delay_seconds * 1000,
        "delaySeconds": state.config.system_shutdown_delay_seconds
    });
    if with_ui_state_write(state, profile_id, |ui_state| {
        let Some(global) = ui_state.get_mut("global").and_then(Value::as_object_mut) else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "global state is missing",
            ));
        };
        global.insert("scheduledShutdown".to_string(), scheduled_shutdown.clone());
        Ok(())
    })
    .await
    .is_err()
    {
        return;
    }

    arm_scheduled_shutdown(state, profile_id, scheduled_shutdown.clone()).await;
    emit_profile_global_notification(
        state,
        profile_id,
        json!({
            "kind": "notification",
            "method": "codex-webui/shutdownScheduled",
            "params": {
                "delaySeconds": state.config.system_shutdown_delay_seconds,
                "turnId": completed_turn_id.map(Value::from).unwrap_or(Value::Null),
                "scheduledFor": scheduled_shutdown.get("scheduledFor").cloned().unwrap_or(Value::Null),
                "sessionId": Value::Null
            }
        }),
    )
    .await;
    enqueue_profile_notification(
        state,
        profile_id,
        "shutdownScheduled",
        None,
        json!({
            "delaySeconds": state.config.system_shutdown_delay_seconds,
            "scheduledFor": scheduled_shutdown.get("scheduledFor").cloned().unwrap_or(Value::Null),
            "turnId": completed_turn_id.map(Value::from).unwrap_or(Value::Null)
        }),
    )
    .await;
    emit_runtime_profile_config_updated(state, profile_id).await;
}

pub(crate) async fn maybe_drain_queue(state: &AppState, profile_id: &str, session_id: &str) {
    let _ = with_queue_dispatch_guard(state, profile_id, session_id, async {
        let queue = match get_session_queue_payload(state, profile_id, session_id).await {
            Ok(queue) => queue,
            Err(_) => return,
        };
        if queue
            .get("items")
            .and_then(Value::as_array)
            .is_none_or(|items| items.is_empty())
        {
            maybe_schedule_global_shutdown(state, profile_id, None).await;
            return;
        }
        if queue
            .get("resumeRequired")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return;
        }
        if session_has_active_turn(state, profile_id, session_id).await {
            return;
        }

        let queued_item = queue
            .get("items")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .cloned();
        let Some(queued_item) = queued_item else {
            return;
        };
        let queue_id = queued_item
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        match dispatch_queue_item(state, profile_id, session_id, &queued_item, "message").await {
            Ok(()) => {
                let _ = remove_session_queue_item_after_dispatch(
                    state, profile_id, session_id, &queue_id,
                )
                .await;
            }
            Err(error) => {
                emit_session_notification(
                    state,
                    profile_id,
                    session_id,
                    json!({
                        "kind": "notification",
                        "method": "codex-webui/queueDispatchFailed",
                        "params": {
                            "queueId": queue_id,
                            "code": Value::Null,
                            "message": error.message
                        }
                    }),
                )
                .await;
                enqueue_profile_notification(
                    state,
                    profile_id,
                    "queueDispatchFailed",
                    Some(session_id),
                    json!({
                        "queueId": queue_id,
                        "code": Value::Null,
                        "message": error.message
                    }),
                )
                .await;
            }
        }
    })
    .await;
}
