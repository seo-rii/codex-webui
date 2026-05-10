use super::*;

const QUEUE_DRAIN_RETRY_DELAYS_MS: [u64; 6] = [250, 750, 1_500, 3_000, 5_000, 10_000];
const QUEUE_ACTIVE_RECONCILE_AFTER_MS: u64 = 5_000;

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

    match local_session_has_active_turn_payload(state, profile_id, session_id).await {
        Ok(Some(true)) => return SessionTurnActivity::Active,
        Ok(Some(false)) => {
            state.active_turns.lock().await.remove(&runtime_key);
            return SessionTurnActivity::Idle;
        }
        Ok(None) => {}
        Err(_) => return SessionTurnActivity::Unknown,
    }

    let client = match app_server_client(state, profile_id).await {
        Ok(client) => client,
        Err(_) => return SessionTurnActivity::Unknown,
    };
    let response = match client
        .request_with_timeout(
            "thread/read",
            json!({
                "threadId": session_id,
                "includeTurns": false
            }),
            Duration::from_millis(500),
            false,
        )
        .await
    {
        Ok(response) => response,
        Err(_) => return SessionTurnActivity::Unknown,
    };
    let thread = response.get("thread").cloned().unwrap_or(Value::Null);
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
        let runtime_status_updated_at = with_ui_state_read(state, profile_id, |ui_state| {
            Ok(ui_state
                .get("runtimeStatusByThreadId")
                .and_then(Value::as_object)
                .and_then(|entries| entries.get(session_id))
                .and_then(|entry| entry.get("updatedAt"))
                .and_then(Value::as_u64))
        })
        .await
        .ok()
        .flatten();
        if runtime_status_updated_at.is_some_and(|updated_at| {
            now_unix_ms().saturating_sub(updated_at) < QUEUE_ACTIVE_RECONCILE_AFTER_MS
        }) {
            return SessionTurnActivity::Active;
        }
        state.active_turns.lock().await.remove(&runtime_key);
    }
    SessionTurnActivity::Idle
}

pub(crate) async fn maybe_drain_queue(state: &AppState, profile_id: &str, session_id: &str) {
    maybe_drain_queue_with_attempt(state, profile_id, session_id, 0).await;
}

pub(crate) fn spawn_queue_drain(state: &AppState, profile_id: &str, session_id: &str) {
    let state = state.clone();
    let profile_id = profile_id.to_string();
    let session_id = session_id.to_string();
    tokio::spawn(async move {
        maybe_drain_queue(&state, &profile_id, &session_id).await;
    });
}

async fn maybe_drain_queue_with_attempt(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    attempt: usize,
) {
    let guarded = with_queue_dispatch_guard(state, profile_id, session_id, async {
        let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id).0;
        let runtime_key = runtime_session_key(resolved_profile_id, session_id);
        if state
            .pending_turn_starts
            .lock()
            .await
            .contains(&runtime_key)
        {
            schedule_queue_drain_retry(state, profile_id, session_id, attempt);
            return;
        }

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
            SessionTurnActivity::Active => {
                schedule_queue_drain_retry(state, profile_id, session_id, attempt);
                return;
            }
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
                let mut error_params = serde_json::Map::new();
                error_params.insert("queueId".to_string(), json!(queue_id));
                if let Some(Value::Object(object)) = structured_error_value(&error.message) {
                    for (key, value) in object {
                        error_params.insert(key, value);
                    }
                } else {
                    error_params.insert("code".to_string(), Value::Null);
                    error_params.insert("message".to_string(), json!(error.message.clone()));
                }
                error_params
                    .entry("code".to_string())
                    .or_insert(Value::Null);
                error_params
                    .entry("message".to_string())
                    .or_insert_with(|| json!(error.message.clone()));
                let error_params = Value::Object(error_params);
                emit_session_notification(
                    state,
                    profile_id,
                    session_id,
                    json!({
                        "kind": "notification",
                        "method": "codex-webui/queueDispatchFailed",
                        "params": error_params.clone()
                    }),
                )
                .await;
                enqueue_profile_notification(
                    state,
                    profile_id,
                    "queueDispatchFailed",
                    Some(session_id),
                    error_params,
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
