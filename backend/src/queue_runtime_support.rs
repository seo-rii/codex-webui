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
    let runtime_key = runtime_session_key(resolved_profile_id, session_id);
    let cached_active_turn_id = state.active_turns.lock().await.get(&runtime_key).cloned();

    let thread = match read_thread_payload(state, profile_id, session_id, true).await {
        Ok(payload) => payload,
        Err(_) => return true,
    };
    let Some(thread) = thread.as_object() else {
        return true;
    };
    let status =
        normalized_thread_status(thread.get("status")).unwrap_or_else(|| "unknown".to_string());
    if !is_live_thread_status(&status) {
        state.active_turns.lock().await.remove(&runtime_key);
        return false;
    }

    let active_turn_id = thread
        .get("turns")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .and_then(active_turn_id_from_turns);
    if let Some(turn_id) = active_turn_id {
        state.active_turns.lock().await.insert(runtime_key, turn_id);
        return true;
    }

    if cached_active_turn_id.is_some() {
        state.active_turns.lock().await.remove(&runtime_key);
    }
    false
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
