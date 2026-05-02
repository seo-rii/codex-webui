use super::*;

const SHUTDOWN_AFTER_QUEUE_COMPLETES_PRIMED_KEY: &str = "shutdownAfterQueueCompletesPrimed";

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
        global.insert("scheduledShutdownBlockedReason".to_string(), Value::Null);
        Ok(())
    })
    .await;
    emit_runtime_profile_config_updated(state, profile_id).await;
}

async fn set_scheduled_shutdown_blocked_reason(
    state: &AppState,
    profile_id: &str,
    reason: Option<&str>,
) {
    let _ = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(global) = ui_state.get_mut("global").and_then(Value::as_object_mut) else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "global state is missing",
            ));
        };
        global.insert(
            "scheduledShutdownBlockedReason".to_string(),
            reason.map(Value::from).unwrap_or(Value::Null),
        );
        Ok(())
    })
    .await;
}

pub(crate) async fn cancel_scheduled_shutdown_for_activity(state: &AppState, profile_id: &str) {
    let (should_prime, scheduled) = with_ui_state_read(state, profile_id, |ui_state| {
        Ok((
            ui_state
                .get("global")
                .and_then(Value::as_object)
                .is_some_and(|global| {
                    global
                        .get("shutdownAfterQueueCompletes")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                        && !global
                            .get(SHUTDOWN_AFTER_QUEUE_COMPLETES_PRIMED_KEY)
                            .and_then(Value::as_bool)
                            .unwrap_or(false)
                }),
            ui_state
                .get("global")
                .and_then(|value| value.get("scheduledShutdown"))
                .cloned()
                .unwrap_or(Value::Null),
        ))
    })
    .await
    .unwrap_or((false, Value::Null));

    if should_prime {
        let _ = with_ui_state_write(state, profile_id, |ui_state| {
            let Some(global) = ui_state.get_mut("global").and_then(Value::as_object_mut) else {
                return Err(api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "global state is missing",
                ));
            };
            global.insert(
                SHUTDOWN_AFTER_QUEUE_COMPLETES_PRIMED_KEY.to_string(),
                json!(true),
            );
            Ok(())
        })
        .await;
    }

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
        global.insert("scheduledShutdownBlockedReason".to_string(), Value::Null);
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

pub(crate) async fn force_scheduled_shutdown_payload(
    state: &AppState,
    profile_id: &str,
) -> ApiResult<Value> {
    let (available, _) = system_shutdown_capability(&state.config).await;
    if !available {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "System shutdown is unavailable for this server user.",
        ));
    }

    execute_scheduled_shutdown(state, profile_id).await;
    Ok(json!({ "ok": true }))
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

    let existing_scheduled = with_ui_state_read(state, profile_id, |ui_state| {
        Ok((
            ui_state
                .get("global")
                .and_then(|value| value.get("shutdownAfterQueueCompletes"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            ui_state
                .get("global")
                .and_then(|value| value.get(SHUTDOWN_AFTER_QUEUE_COMPLETES_PRIMED_KEY))
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
    let Ok((shutdown_after_queue_completes, shutdown_primed, scheduled_shutdown)) =
        existing_scheduled
    else {
        return;
    };
    if !shutdown_after_queue_completes {
        return;
    }
    if !shutdown_primed {
        return;
    }

    let (available, _) = system_shutdown_capability(&state.config).await;
    if !available {
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
    if has_outstanding_queued_work(state, profile_id).await {
        set_scheduled_shutdown_blocked_reason(state, profile_id, Some("queuedWork")).await;
        emit_runtime_profile_config_updated(state, profile_id).await;
        return;
    }
    if has_active_work_across_threads(state, profile_id).await {
        set_scheduled_shutdown_blocked_reason(state, profile_id, Some("activeWork")).await;
        emit_runtime_profile_config_updated(state, profile_id).await;
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
        global.insert("scheduledShutdownBlockedReason".to_string(), Value::Null);
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
