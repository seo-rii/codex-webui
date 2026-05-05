use super::*;

pub(crate) async fn get_session_queue_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<Value> {
    with_ui_state_read(state, profile_id, |ui_state| {
        let stored = ui_state
            .get("queuesByThreadId")
            .and_then(Value::as_object)
            .and_then(|entries| entries.get(session_id));
        let items = stored
            .and_then(|entry| entry.get("items"))
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        let mut normalized = item.as_object().cloned().unwrap_or_default();
                        normalized.insert(
                            "skills".to_string(),
                            Value::Array(selected_skills_from_value(item.get("skills"))),
                        );
                        Value::Object(normalized)
                    })
                    .collect::<Vec<_>>()
            })
            .map(Value::Array)
            .unwrap_or_else(|| json!([]));
        let resume_pending = stored
            .and_then(|entry| entry.get("resumePending"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let item_count = items.as_array().map(Vec::len).unwrap_or(0);
        Ok(json!({
            "sessionId": session_id,
            "items": items,
            "resumeRequired": resume_pending && item_count > 0,
            "updatedAt": stored
                .and_then(|entry| entry.get("updatedAt"))
                .cloned()
                .unwrap_or(Value::Null)
        }))
    })
    .await
}

pub(crate) async fn emit_queue_updated(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    queue: Option<Value>,
) {
    let queue = match queue {
        Some(queue) => queue,
        None => match get_session_queue_payload(state, profile_id, session_id).await {
            Ok(queue) => queue,
            Err(_) => return,
        },
    };

    emit_session_notification(
        state,
        profile_id,
        session_id,
        json!({
            "kind": "notification",
            "method": "codex-webui/queueUpdated",
            "params": {
                "queue": queue
            }
        }),
    )
    .await;
    let state = state.clone();
    let profile_id = profile_id.to_string();
    let session_id = session_id.to_string();
    tokio::spawn(async move {
        emit_session_summary_updated(&state, &profile_id, &session_id, None, None).await;
        emit_runtime_profile_config_updated(&state, &profile_id).await;
    });
}

pub(crate) async fn list_resume_pending_queues_payload(
    state: &AppState,
    profile_id: &str,
) -> ApiResult<Value> {
    let (entries, preferences_by_thread_id) = with_ui_state_read(state, profile_id, |ui_state| {
        let entries = ui_state
            .get("queuesByThreadId")
            .and_then(Value::as_object)
            .map(|queues| {
                queues
                    .iter()
                    .filter_map(|(session_id, queue)| {
                        let items = queue.get("items").and_then(Value::as_array)?;
                        let resume_pending = queue
                            .get("resumePending")
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        if !resume_pending || items.is_empty() {
                            return None;
                        }
                        Some((
                            session_id.clone(),
                            items.len(),
                            queue.get("updatedAt").and_then(Value::as_u64).unwrap_or(0),
                        ))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let preferences = ui_state
            .get("preferencesByThreadId")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        Ok((entries, preferences))
    })
    .await?;

    let mut paused = Vec::with_capacity(entries.len());
    for (session_id, pending_count, updated_at) in entries {
        let mut name = Value::Null;
        let mut cwd = preferences_by_thread_id
            .get(&session_id)
            .and_then(|entry| entry.get("cwd"))
            .cloned()
            .unwrap_or(Value::Null);

        let state_thread =
            read_state_thread_metadata_by_session_id(state, profile_id, &session_id, None)
                .await
                .ok()
                .flatten();
        let thread = if state_thread.is_some() {
            state_thread
        } else {
            read_rollout_thread_metadata_by_session_id(state, profile_id, &session_id)
                .await
                .ok()
                .flatten()
        };
        if let Some(thread) = thread.as_ref().and_then(Value::as_object) {
            name = display_thread_name(
                thread.get("name").and_then(Value::as_str),
                thread.get("preview").and_then(Value::as_str),
            )
            .map(Value::from)
            .unwrap_or(Value::Null);
            if !thread.get("cwd").is_none_or(Value::is_null) {
                cwd = thread.get("cwd").cloned().unwrap_or(Value::Null);
            }
        }

        paused.push(json!({
            "sessionId": session_id,
            "name": name,
            "cwd": cwd,
            "pendingCount": pending_count,
            "updatedAt": updated_at
        }));
    }

    Ok(Value::Array(paused))
}

pub(crate) async fn mark_queues_pending_resume_payload(
    state: &AppState,
    profile_id: &str,
) -> ApiResult<bool> {
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

        let mut changed = false;
        for queue in queues_by_thread_id.values_mut() {
            let Some(queue_object) = queue.as_object_mut() else {
                continue;
            };
            let item_count = queue_object
                .get("items")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0);
            let resume_pending = queue_object
                .get("resumePending")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if item_count == 0 || resume_pending {
                continue;
            }
            queue_object.insert("resumePending".to_string(), json!(true));
            queue_object.insert("updatedAt".to_string(), json!(now_unix_ms()));
            changed = true;
        }

        Ok(changed)
    })
    .await
}
