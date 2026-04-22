use super::*;

pub(crate) async fn get_session_draft_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<Value> {
    with_ui_state_read(state, profile_id, |ui_state| {
        let stored = ui_state
            .get("draftsByThreadId")
            .and_then(Value::as_object)
            .and_then(|entries| entries.get(session_id));
        Ok(json!({
            "sessionId": session_id,
            "draft": stored
                .and_then(|entry| entry.get("draft"))
                .and_then(Value::as_str)
                .unwrap_or_default(),
            "intent": stored
                .and_then(|entry| entry.get("intent"))
                .cloned()
                .unwrap_or(Value::Null),
            "updatedAt": stored
                .and_then(|entry| entry.get("updatedAt"))
                .cloned()
                .unwrap_or(Value::Null)
        }))
    })
    .await
}

pub(crate) async fn save_session_draft_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    draft: &str,
    intent: &str,
) -> ApiResult<Value> {
    let trimmed = draft.trim();
    if trimmed.is_empty() {
        return clear_session_draft_payload(state, profile_id, session_id).await;
    }

    let normalized_intent = match intent {
        "steer" => "steer",
        "queue" => "queue",
        _ => "message",
    };
    let updated_at = now_unix_ms();
    with_ui_state_write(state, profile_id, |ui_state| {
        let Some(drafts_by_thread_id) = ui_state
            .get_mut("draftsByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "draft state is missing",
            ));
        };
        drafts_by_thread_id.insert(
            session_id.to_string(),
            json!({
                "draft": draft,
                "intent": normalized_intent,
                "updatedAt": updated_at
            }),
        );
        Ok(json!({
            "sessionId": session_id,
            "draft": draft,
            "intent": normalized_intent,
            "updatedAt": updated_at
        }))
    })
    .await
}

pub(crate) async fn clear_session_draft_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<Value> {
    with_ui_state_write(state, profile_id, |ui_state| {
        let Some(drafts_by_thread_id) = ui_state
            .get_mut("draftsByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "draft state is missing",
            ));
        };
        drafts_by_thread_id.remove(session_id);
        Ok(json!({
            "sessionId": session_id,
            "draft": "",
            "intent": Value::Null,
            "updatedAt": Value::Null
        }))
    })
    .await
}

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
    emit_session_summary_updated(state, profile_id, session_id, None).await;
    emit_runtime_profile_config_updated(state, profile_id).await;
}

pub(crate) async fn enqueue_session_queue_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    prompt: &str,
    selected_skills: Option<&Value>,
    attachment_ids: Option<&Value>,
) -> ApiResult<Value> {
    let trimmed_prompt = prompt.trim();
    let (resolved_attachment_ids, attachment_names) =
        resolve_queue_attachment_metadata(state, profile_id, session_id, attachment_ids).await?;
    let next_selected_skills = selected_skills_from_value(selected_skills);
    if trimmed_prompt.is_empty() && resolved_attachment_ids.is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "EMPTY_MESSAGE"));
    }

    cancel_scheduled_shutdown_for_activity(state, profile_id).await;

    let queue_item_id = Uuid::new_v4().to_string();
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

        let updated_at = now_unix_ms();
        let entry = queues_by_thread_id
            .entry(session_id.to_string())
            .or_insert_with(|| {
                json!({
                    "items": [],
                    "resumePending": false,
                    "updatedAt": updated_at
                })
            });
        let Some(queue) = entry.as_object_mut() else {
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
        items.push(json!({
            "id": queue_item_id,
            "prompt": trimmed_prompt,
            "skills": next_selected_skills,
            "attachmentIds": resolved_attachment_ids,
            "attachmentNames": attachment_names,
            "createdAt": updated_at
        }));
        queue.insert("resumePending".to_string(), json!(false));
        queue.insert("updatedAt".to_string(), json!(updated_at));
        Ok(())
    })
    .await?;

    let mut queue = get_session_queue_payload(state, profile_id, session_id).await?;
    if let Some(queue_object) = queue.as_object_mut() {
        queue_object.insert("enqueueAccepted".to_string(), json!(true));
        queue_object.insert("enqueueItemId".to_string(), json!(queue_item_id));
    }
    emit_queue_updated(state, profile_id, session_id, Some(queue.clone())).await;

    Ok(queue)
}

pub(crate) async fn remove_session_queue_item_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    queue_id: &str,
) -> ApiResult<Value> {
    let changed = with_ui_state_write(state, profile_id, |ui_state| {
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
            return Ok(false);
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
            return Ok(false);
        }

        if items.is_empty() {
            queues_by_thread_id.remove(session_id);
        } else {
            queue.insert("updatedAt".to_string(), json!(now_unix_ms()));
        }
        Ok(true)
    })
    .await?;
    if !changed {
        return Err(api_error(StatusCode::NOT_FOUND, "QUEUE_ITEM_NOT_FOUND"));
    }

    let queue = get_session_queue_payload(state, profile_id, session_id).await?;
    emit_queue_updated(state, profile_id, session_id, Some(queue.clone())).await;
    maybe_schedule_global_shutdown(state, profile_id, None).await;
    Ok(queue)
}

pub(crate) async fn update_session_queue_item_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    queue_id: &str,
    prompt: Option<&str>,
    selected_skills: Option<&Value>,
    attachment_ids: Option<&Value>,
) -> ApiResult<Value> {
    let existing_queue = get_session_queue_payload(state, profile_id, session_id).await?;
    let queued_item = existing_queue
        .get("items")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.get("id").and_then(Value::as_str) == Some(queue_id))
                .cloned()
        })
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "QUEUE_ITEM_NOT_FOUND"))?;

    let next_prompt = prompt.map(str::to_string).unwrap_or_else(|| {
        queued_item
            .get("prompt")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    });
    let requested_attachment_ids = attachment_ids.cloned().unwrap_or_else(|| {
        queued_item
            .get("attachmentIds")
            .cloned()
            .unwrap_or_else(|| json!([]))
    });
    let next_selected_skills = selected_skills.cloned().unwrap_or_else(|| {
        queued_item
            .get("skills")
            .cloned()
            .unwrap_or_else(|| json!([]))
    });
    let (resolved_attachment_ids, attachment_names) = resolve_queue_attachment_metadata(
        state,
        profile_id,
        session_id,
        Some(&requested_attachment_ids),
    )
    .await?;
    if next_prompt.trim().is_empty() && resolved_attachment_ids.is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "EMPTY_MESSAGE"));
    }

    let changed = with_ui_state_write(state, profile_id, |ui_state| {
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
            return Ok(false);
        };
        let Some(items) = existing.get_mut("items").and_then(Value::as_array_mut) else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "queue items are missing",
            ));
        };
        let Some(item) = items
            .iter_mut()
            .find(|item| item.get("id").and_then(Value::as_str) == Some(queue_id))
        else {
            return Ok(false);
        };
        let Some(item_object) = item.as_object_mut() else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "queue item had an unexpected shape",
            ));
        };
        item_object.insert("prompt".to_string(), json!(next_prompt.trim()));
        item_object.insert(
            "skills".to_string(),
            Value::Array(selected_skills_from_value(Some(&next_selected_skills))),
        );
        item_object.insert("attachmentIds".to_string(), json!(resolved_attachment_ids));
        item_object.insert("attachmentNames".to_string(), json!(attachment_names));
        if let Some(existing_object) = existing.as_object_mut() {
            existing_object.insert("updatedAt".to_string(), json!(now_unix_ms()));
        }
        Ok(true)
    })
    .await?;
    if !changed {
        return Err(api_error(StatusCode::NOT_FOUND, "QUEUE_ITEM_NOT_FOUND"));
    }

    let queue = get_session_queue_payload(state, profile_id, session_id).await?;
    emit_queue_updated(state, profile_id, session_id, Some(queue.clone())).await;
    Ok(queue)
}

pub(crate) async fn reorder_session_queue_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    ordered_ids: &[String],
) -> ApiResult<Value> {
    if ordered_ids.is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "QUEUE_ITEM_NOT_FOUND"));
    }

    let reordered = with_ui_state_write(state, profile_id, |ui_state| {
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
            return Ok(false);
        };
        let Some(items) = existing.get_mut("items").and_then(Value::as_array_mut) else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "queue items are missing",
            ));
        };
        if ordered_ids.len() != items.len() {
            return Ok(false);
        }

        let items_by_id = items
            .iter()
            .filter_map(|item| {
                item.get("id")
                    .and_then(Value::as_str)
                    .map(|id| (id.to_string(), item.clone()))
            })
            .collect::<HashMap<_, _>>();
        let next_items = ordered_ids
            .iter()
            .filter_map(|queue_id| items_by_id.get(queue_id).cloned())
            .collect::<Vec<_>>();
        if next_items.len() != items.len()
            || ordered_ids.iter().collect::<HashSet<_>>().len() != ordered_ids.len()
        {
            return Ok(false);
        }

        *items = next_items;
        if let Some(existing_object) = existing.as_object_mut() {
            existing_object.insert("updatedAt".to_string(), json!(now_unix_ms()));
        }
        Ok(true)
    })
    .await?;
    if !reordered {
        return Err(api_error(StatusCode::NOT_FOUND, "QUEUE_ITEM_NOT_FOUND"));
    }

    let queue = get_session_queue_payload(state, profile_id, session_id).await?;
    emit_queue_updated(state, profile_id, session_id, Some(queue.clone())).await;
    Ok(queue)
}

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
    maybe_drain_queue(state, profile_id, session_id).await;
    Ok(queue)
}

pub(crate) async fn dispatch_session_queue_item_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    queue_id: &str,
    mode: &str,
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
        dispatch_queue_item(state, profile_id, session_id, &queued_item, mode).await?;
        let next_queue =
            remove_session_queue_item_after_dispatch(state, profile_id, session_id, queue_id)
                .await?;
        Ok(next_queue)
    })
    .await;

    match queue {
        Some(result) => result,
        None => Err(api_error(StatusCode::CONFLICT, "QUEUE_ALREADY_DISPATCHING")),
    }
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

        if let Ok(thread) = read_thread_payload(state, profile_id, &session_id, false).await {
            if let Some(thread) = thread.as_object() {
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
