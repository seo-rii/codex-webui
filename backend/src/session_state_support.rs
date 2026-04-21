use super::*;

fn ui_state_notification_items(ui_state: &Value) -> Vec<Value> {
    ui_state
        .get("notifications")
        .and_then(|value| value.get("items"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

pub(crate) fn unread_notification_count(items: &[Value]) -> usize {
    items
        .iter()
        .filter(|entry| entry.get("readAt").is_none_or(Value::is_null))
        .count()
}

fn notifications_payload_from_items(mut items: Vec<Value>, limit: usize) -> Value {
    items.sort_by(|left, right| {
        let left_created = left.get("createdAt").and_then(Value::as_i64).unwrap_or(0);
        let right_created = right.get("createdAt").and_then(Value::as_i64).unwrap_or(0);
        right_created.cmp(&left_created)
    });
    let unread_count = unread_notification_count(&items);
    let limited = items.into_iter().take(limit.max(1)).collect::<Vec<_>>();
    json!({
        "notifications": limited,
        "unreadCount": unread_count
    })
}

pub(crate) async fn get_notifications_payload(
    state: &AppState,
    profile_id: &str,
    limit: usize,
) -> ApiResult<Value> {
    with_ui_state_read(state, profile_id, |ui_state| {
        Ok(notifications_payload_from_items(
            ui_state_notification_items(ui_state),
            limit,
        ))
    })
    .await
}

pub(crate) async fn mark_notifications_read_payload(
    state: &AppState,
    profile_id: &str,
    ids: Option<Vec<String>>,
) -> ApiResult<Value> {
    let target_ids = ids.map(|items| {
        items
            .into_iter()
            .filter_map(|item| {
                let trimmed = item.trim().to_string();
                (!trimmed.is_empty()).then_some(trimmed)
            })
            .collect::<Vec<_>>()
    });

    let (payload, changed) = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(items) = ui_state
            .get_mut("notifications")
            .and_then(Value::as_object_mut)
            .and_then(|value| value.get_mut("items"))
            .and_then(Value::as_array_mut)
        else {
            return Ok((json!({ "notifications": [], "unreadCount": 0 }), false));
        };

        let targets = target_ids.as_ref().map(|entries| {
            entries
                .iter()
                .cloned()
                .collect::<std::collections::HashSet<_>>()
        });
        let marked_at = now_unix_ms() as i64;
        let mut changed = false;

        for entry in items.iter_mut() {
            let read_at = entry.get("readAt");
            let entry_id = entry.get("id").and_then(Value::as_str);
            let should_mark = read_at.is_none_or(Value::is_null)
                && targets
                    .as_ref()
                    .is_none_or(|ids| entry_id.is_some_and(|candidate| ids.contains(candidate)));
            if should_mark {
                if let Some(object) = entry.as_object_mut() {
                    object.insert("readAt".to_string(), json!(marked_at));
                    changed = true;
                }
            }
        }

        Ok((
            notifications_payload_from_items(items.clone(), DEFAULT_NOTIFICATION_LIMIT),
            changed,
        ))
    })
    .await?;

    if changed {
        emit_profile_global_notification(
            state,
            profile_id,
            json!({
                "kind": "notification",
                "method": "codex-webui/notificationStateUpdated",
                "params": {
                    "unreadCount": payload.get("unreadCount").cloned().unwrap_or_else(|| json!(0))
                }
            }),
        )
        .await;
        emit_profile_config_updated(
            state,
            profile_id,
            json!({
                "notifications": {
                    "unreadCount": payload.get("unreadCount").cloned().unwrap_or_else(|| json!(0))
                }
            }),
        )
        .await;
    }

    Ok(payload)
}

pub(crate) async fn clear_notifications_payload(
    state: &AppState,
    profile_id: &str,
) -> ApiResult<Value> {
    let (payload, changed) = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(items) = ui_state
            .get_mut("notifications")
            .and_then(Value::as_object_mut)
            .and_then(|value| value.get_mut("items"))
            .and_then(Value::as_array_mut)
        else {
            return Ok((json!({ "notifications": [], "unreadCount": 0 }), false));
        };

        let changed = !items.is_empty();
        items.clear();
        Ok((
            notifications_payload_from_items(Vec::new(), DEFAULT_NOTIFICATION_LIMIT),
            changed,
        ))
    })
    .await?;

    if changed {
        emit_profile_global_notification(
            state,
            profile_id,
            json!({
                "kind": "notification",
                "method": "codex-webui/notificationStateUpdated",
                "params": {
                    "unreadCount": 0
                }
            }),
        )
        .await;
        emit_profile_config_updated(
            state,
            profile_id,
            json!({
                "notifications": {
                    "unreadCount": 0
                }
            }),
        )
        .await;
    }

    Ok(payload)
}

pub(crate) async fn update_notification_settings_payload(
    state: &AppState,
    profile_id: &str,
    patch: Value,
) -> ApiResult<Value> {
    let payload = with_ui_state_write(state, profile_id, |ui_state| {
        let notifications = ui_state
            .get_mut("notifications")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "notifications state is missing",
                )
            })?;

        let current_settings = notifications.get("settings");
        let merged_settings = normalize_notification_settings_value(Some(&json!({
            "enabledEventTypes": patch.get("enabledEventTypes").cloned().unwrap_or_else(|| {
                current_settings
                    .and_then(|value| value.get("enabledEventTypes"))
                    .cloned()
                    .unwrap_or_else(|| default_notification_settings_value()["enabledEventTypes"].clone())
            }),
            "slackWebhookUrl": patch.get("slackWebhookUrl").cloned().unwrap_or_else(|| {
                current_settings
                    .and_then(|value| value.get("slackWebhookUrl"))
                    .cloned()
                    .unwrap_or(Value::Null)
            }),
            "webhookUrl": patch.get("webhookUrl").cloned().unwrap_or_else(|| {
                current_settings
                    .and_then(|value| value.get("webhookUrl"))
                    .cloned()
                    .unwrap_or(Value::Null)
            })
        })));

        notifications.insert("settings".to_string(), merged_settings.clone());
        let unread_count = notifications
            .get("items")
            .and_then(Value::as_array)
            .map(|items| unread_notification_count(items))
            .unwrap_or(0);

        Ok(json!({
            "settings": merged_settings,
            "unreadCount": unread_count
        }))
    })
    .await?;

    emit_profile_global_notification(
        state,
        profile_id,
        json!({
            "kind": "notification",
            "method": "codex-webui/notificationSettingsUpdated",
            "params": payload.clone()
        }),
    )
    .await;
    emit_profile_config_updated(
        state,
        profile_id,
        json!({
            "notifications": payload.clone()
        }),
    )
    .await;

    Ok(payload)
}

pub(crate) async fn save_session_filter_payload(
    state: &AppState,
    profile_id: &str,
    filter: Value,
) -> ApiResult<Value> {
    let name = filter
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "Filter name is required."))?;
    let filter_id = filter
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "filter.id is required."))?;

    let payload = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(saved_filters) = ui_state
            .get_mut("savedSessionFilters")
            .and_then(Value::as_array_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "saved filters state is missing",
            ));
        };

        let normalized_tags = filter
            .get("tags")
            .and_then(Value::as_array)
            .map(|tags| {
                let mut values = tags
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|tag| !tag.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                values.sort();
                values.dedup();
                values
            })
            .unwrap_or_default();

        let highlight = match filter.get("highlight").and_then(Value::as_str) {
            Some("attention") => "attention",
            Some("completed") => "completed",
            _ => "all",
        };

        let next_filter = json!({
            "id": filter_id,
            "name": name,
            "pinnedOnly": filter.get("pinnedOnly").and_then(Value::as_bool).unwrap_or(false),
            "runningOnly": filter.get("runningOnly").and_then(Value::as_bool).unwrap_or(false),
            "queuedOnly": filter.get("queuedOnly").and_then(Value::as_bool).unwrap_or(false),
            "highlight": highlight,
            "tags": normalized_tags
        });

        let mut next_saved_filters = vec![next_filter];
        next_saved_filters.extend(
            saved_filters
                .iter()
                .filter(|entry| entry.get("id").and_then(Value::as_str) != Some(filter_id))
                .cloned(),
        );
        next_saved_filters.truncate(40);
        *saved_filters = next_saved_filters;

        Ok(json!({
            "savedFilters": saved_filters.clone(),
            "knownTags": known_tags_from_ui_state(ui_state)
        }))
    })
    .await?;

    emit_profile_config_updated(
        state,
        profile_id,
        json!({
            "sessionOrganization": {
                "savedFilters": payload.get("savedFilters").cloned().unwrap_or_else(|| json!([])),
                "knownTags": payload.get("knownTags").cloned().unwrap_or_else(|| json!([]))
            }
        }),
    )
    .await;

    Ok(payload)
}

pub(crate) async fn delete_session_filter_payload(
    state: &AppState,
    profile_id: &str,
    filter_id: &str,
) -> ApiResult<Value> {
    let trimmed_filter_id = filter_id.trim();
    let payload = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(saved_filters) = ui_state
            .get_mut("savedSessionFilters")
            .and_then(Value::as_array_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "saved filters state is missing",
            ));
        };

        *saved_filters = saved_filters
            .iter()
            .filter(|entry| entry.get("id").and_then(Value::as_str) != Some(trimmed_filter_id))
            .cloned()
            .collect::<Vec<_>>();

        Ok(json!({
            "savedFilters": saved_filters.clone(),
            "knownTags": known_tags_from_ui_state(ui_state)
        }))
    })
    .await?;

    emit_profile_config_updated(
        state,
        profile_id,
        json!({
            "sessionOrganization": {
                "savedFilters": payload.get("savedFilters").cloned().unwrap_or_else(|| json!([])),
                "knownTags": payload.get("knownTags").cloned().unwrap_or_else(|| json!([]))
            }
        }),
    )
    .await;

    Ok(payload)
}

pub(crate) async fn save_prompt_preset_payload(
    state: &AppState,
    profile_id: &str,
    preset: Value,
) -> ApiResult<Value> {
    let preset_id = preset
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "preset.id is required."))?;
    let preset_name = preset
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "Preset name is required."))?;
    let preset_prompt = preset
        .get("prompt")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "Preset prompt is required."))?;

    let payload = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(prompt_presets) = ui_state
            .get_mut("promptPresets")
            .and_then(Value::as_array_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "prompt presets state is missing",
            ));
        };

        let now = now_unix_ms() as i64;
        let created_at = prompt_presets
            .iter()
            .find(|entry| entry.get("id").and_then(Value::as_str) == Some(preset_id))
            .and_then(|entry| entry.get("createdAt").and_then(Value::as_i64))
            .or_else(|| preset.get("createdAt").and_then(Value::as_i64))
            .unwrap_or(now);

        let next_preset = json!({
            "id": preset_id,
            "name": preset_name,
            "prompt": preset_prompt,
            "createdAt": created_at,
            "updatedAt": now
        });

        let mut next_prompt_presets = vec![next_preset];
        next_prompt_presets.extend(
            prompt_presets
                .iter()
                .filter(|entry| entry.get("id").and_then(Value::as_str) != Some(preset_id))
                .cloned(),
        );
        next_prompt_presets.truncate(80);
        next_prompt_presets.sort_by(|left, right| {
            let left_updated = left.get("updatedAt").and_then(Value::as_i64).unwrap_or(0);
            let right_updated = right.get("updatedAt").and_then(Value::as_i64).unwrap_or(0);
            right_updated.cmp(&left_updated)
        });
        *prompt_presets = next_prompt_presets;

        Ok(json!({
            "promptPresets": prompt_presets.clone()
        }))
    })
    .await?;

    emit_profile_config_updated(
        state,
        profile_id,
        json!({
            "promptPresets": payload.get("promptPresets").cloned().unwrap_or_else(|| json!([]))
        }),
    )
    .await;

    Ok(payload)
}

pub(crate) async fn delete_prompt_preset_payload(
    state: &AppState,
    profile_id: &str,
    preset_id: &str,
) -> ApiResult<Value> {
    let trimmed_preset_id = preset_id.trim();
    let payload = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(prompt_presets) = ui_state
            .get_mut("promptPresets")
            .and_then(Value::as_array_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "prompt presets state is missing",
            ));
        };

        *prompt_presets = prompt_presets
            .iter()
            .filter(|entry| entry.get("id").and_then(Value::as_str) != Some(trimmed_preset_id))
            .cloned()
            .collect::<Vec<_>>();
        prompt_presets.sort_by(|left, right| {
            let left_updated = left.get("updatedAt").and_then(Value::as_i64).unwrap_or(0);
            let right_updated = right.get("updatedAt").and_then(Value::as_i64).unwrap_or(0);
            right_updated.cmp(&left_updated)
        });

        Ok(json!({
            "promptPresets": prompt_presets.clone()
        }))
    })
    .await?;

    emit_profile_config_updated(
        state,
        profile_id,
        json!({
            "promptPresets": payload.get("promptPresets").cloned().unwrap_or_else(|| json!([]))
        }),
    )
    .await;

    Ok(payload)
}

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
