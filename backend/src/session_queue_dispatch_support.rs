use super::*;

pub(crate) async fn resume_session_queue_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<Value> {
    let runtime_key = runtime_session_key(
        &resolve_runtime_profile_entry(&state.config, profile_id).0,
        session_id,
    );
    if state.queue_dispatching.lock().await.contains(&runtime_key) {
        return Err(api_error(StatusCode::CONFLICT, "QUEUE_ALREADY_DISPATCHING"));
    }
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
            return Ok(());
        };
        let Some(queue_object) = existing.as_object_mut() else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "queue state had an unexpected shape",
            ));
        };
        queue_object.insert("resumePending".to_string(), json!(false));
        queue_object.insert("updatedAt".to_string(), json!(now_unix_ms()));
        if let Some(items) = queue_object.get_mut("items").and_then(Value::as_array_mut) {
            for item in items {
                if let Some(item_object) = item.as_object_mut() {
                    item_object.remove("status");
                    item_object.remove("failedAt");
                    item_object.remove("error");
                }
            }
        }
        Ok(())
    })
    .await?;

    let queue = get_session_queue_payload(state, profile_id, session_id).await?;
    emit_queue_updated(state, profile_id, session_id, Some(queue.clone())).await;
    spawn_queue_drain(state, profile_id, session_id);
    Ok(queue)
}

pub(crate) async fn dispatch_session_queue_item_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    queue_id: &str,
    mode: &str,
    expected_turn_id: Option<&str>,
) -> ApiResult<Value> {
    if mode != "message" && mode != "steer" {
        return Err(api_error(StatusCode::BAD_REQUEST, "INVALID_QUEUE_MODE"));
    }

    let queue = with_queue_dispatch_guard(state, profile_id, session_id, async {
        recover_orphaned_session_queue_dispatch_claims(state, profile_id, session_id).await?;
        let stored_queue = get_session_queue_payload(state, profile_id, session_id).await?;
        let queued_item = stored_queue
            .get("items")
            .and_then(Value::as_array)
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item.get("id").and_then(Value::as_str) == Some(queue_id))
                    .cloned()
            })
            .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "QUEUE_ITEM_NOT_FOUND"))?;

        cancel_scheduled_shutdown_for_activity(state, profile_id).await;
        dispatch_queue_item(
            state,
            profile_id,
            session_id,
            &queued_item,
            mode,
            expected_turn_id,
        )
        .await?;
        let next_queue =
            match remove_session_queue_item_after_dispatch(state, profile_id, session_id, queue_id)
                .await
            {
                Ok(queue) => queue,
                Err(error) => {
                    release_session_queue_item_dispatch_claim(
                        state, profile_id, session_id, queue_id,
                    )
                    .await;
                    return Err(error);
                }
            };
        let should_continue = next_queue
            .get("items")
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty())
            && !next_queue
                .get("resumeRequired")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        if should_continue {
            spawn_queue_drain(state, profile_id, session_id);
        }
        Ok(next_queue)
    })
    .await;

    match queue {
        Some(result) => result,
        None => {
            spawn_queue_drain(state, profile_id, session_id);
            let mut queue = get_session_queue_payload(state, profile_id, session_id).await?;
            if let Some(queue_object) = queue.as_object_mut() {
                queue_object.insert("dispatchAlreadyInProgress".to_string(), json!(true));
            }
            Ok(queue)
        }
    }
}
