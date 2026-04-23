use super::*;

const QUEUE_DRAIN_RETRY_DELAYS_MS: [u64; 6] = [250, 750, 1_500, 3_000, 5_000, 10_000];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionTurnActivity {
    Active,
    Idle,
    Unknown,
}

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

async fn cancel_queue_drain_retry(state: &AppState, profile_id: &str, session_id: &str) {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id).0;
    let key = runtime_session_key(resolved_profile_id, session_id);
    if let Some(handle) = state.queue_drain_retries.lock().await.remove(&key) {
        handle.abort();
    }
}

fn schedule_queue_drain_retry(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    attempt: usize,
) {
    let delay_ms = QUEUE_DRAIN_RETRY_DELAYS_MS
        .get(attempt)
        .copied()
        .unwrap_or_else(|| QUEUE_DRAIN_RETRY_DELAYS_MS[QUEUE_DRAIN_RETRY_DELAYS_MS.len() - 1]);
    if attempt == QUEUE_DRAIN_RETRY_DELAYS_MS.len() {
        warn!(
            profile_id = %profile_id,
            session_id = %session_id,
            "queued session drain is still waiting for readable session state"
        );
    }

    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    let key = runtime_session_key(&resolved_profile_id, session_id);
    let state_for_registration = state.clone();
    let profile_id = profile_id.to_string();
    let session_id = session_id.to_string();
    tokio::spawn(async move {
        let mut retries = state_for_registration.queue_drain_retries.lock().await;
        if retries.contains_key(&key) {
            return;
        }

        let state_for_retry = state_for_registration.clone();
        let retry_key = key.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            state_for_retry
                .queue_drain_retries
                .lock()
                .await
                .remove(&retry_key);
            maybe_drain_queue_with_attempt(&state_for_retry, &profile_id, &session_id, attempt + 1)
                .await;
        });
        retries.insert(key, handle);
    });
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

async fn session_turn_activity(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> SessionTurnActivity {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id).0;
    let runtime_key = runtime_session_key(resolved_profile_id, session_id);
    let cached_active_turn_id = state.active_turns.lock().await.get(&runtime_key).cloned();

    let thread = match read_thread_payload(state, profile_id, session_id, true).await {
        Ok(payload) => payload,
        Err(_) => return SessionTurnActivity::Unknown,
    };
    let Some(thread) = thread.as_object() else {
        return SessionTurnActivity::Unknown;
    };
    let status =
        normalized_thread_status(thread.get("status")).unwrap_or_else(|| "unknown".to_string());
    if !is_live_thread_status(&status) {
        state.active_turns.lock().await.remove(&runtime_key);
        return SessionTurnActivity::Idle;
    }

    let active_turn_id = thread
        .get("turns")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .and_then(active_turn_id_from_turns);
    if let Some(turn_id) = active_turn_id {
        state.active_turns.lock().await.insert(runtime_key, turn_id);
        return SessionTurnActivity::Active;
    }

    if cached_active_turn_id.is_some() {
        state.active_turns.lock().await.remove(&runtime_key);
    }
    SessionTurnActivity::Idle
}

pub(crate) async fn maybe_drain_queue(state: &AppState, profile_id: &str, session_id: &str) {
    maybe_drain_queue_with_attempt(state, profile_id, session_id, 0).await;
}

async fn maybe_drain_queue_with_attempt(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    attempt: usize,
) {
    let guarded = with_queue_dispatch_guard(state, profile_id, session_id, async {
        let queue = match get_session_queue_payload(state, profile_id, session_id).await {
            Ok(queue) => queue,
            Err(_) => return,
        };
        if queue
            .get("items")
            .and_then(Value::as_array)
            .is_none_or(|items| items.is_empty())
        {
            cancel_queue_drain_retry(state, profile_id, session_id).await;
            maybe_schedule_global_shutdown(state, profile_id, None).await;
            return;
        }
        if queue
            .get("resumeRequired")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            cancel_queue_drain_retry(state, profile_id, session_id).await;
            return;
        }
        match session_turn_activity(state, profile_id, session_id).await {
            SessionTurnActivity::Active => return,
            SessionTurnActivity::Unknown => {
                schedule_queue_drain_retry(state, profile_id, session_id, attempt);
                return;
            }
            SessionTurnActivity::Idle => {}
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
    if guarded.is_none() {
        schedule_queue_drain_retry(state, profile_id, session_id, attempt);
    }
}
