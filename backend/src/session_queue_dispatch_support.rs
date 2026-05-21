use super::*;

pub(crate) async fn resume_session_queue_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
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
            remove_session_queue_item_after_dispatch(state, profile_id, session_id, queue_id)
                .await?;
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
