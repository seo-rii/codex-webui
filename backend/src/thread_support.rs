use super::*;

pub(crate) async fn update_session_organization_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    patch: Value,
) -> ApiResult<Value> {
    let payload = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(session_meta_by_thread_id) = ui_state
            .get_mut("sessionMetaByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "session metadata state is missing",
            ));
        };

        let current = session_meta_by_thread_id
            .get(session_id)
            .cloned()
            .unwrap_or_else(|| json!({ "pinned": false, "tags": [] }));
        let pinned = patch
            .get("pinned")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| {
                current
                    .get("pinned")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            });
        let mut tags = patch
            .get("tags")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| {
                current
                    .get("tags")
                    .and_then(Value::as_array)
                    .map(|entries| {
                        entries
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            });
        tags.sort();
        tags.dedup();

        let mut meta_object = current.as_object().cloned().unwrap_or_default();
        meta_object.insert("pinned".to_string(), json!(pinned));
        meta_object.insert("tags".to_string(), json!(tags));
        let has_title = meta_object
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty());
        let has_tags = meta_object
            .get("tags")
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty());
        if !pinned && !has_tags && !has_title {
            session_meta_by_thread_id.remove(session_id);
        } else {
            let meta = Value::Object(meta_object);
            session_meta_by_thread_id.insert(session_id.to_string(), meta.clone());
        }

        Ok(json!({
            "meta": session_meta_by_thread_id
                .get(session_id)
                .cloned()
                .unwrap_or_else(|| json!({ "pinned": false, "tags": [] })),
            "knownTags": known_tags_from_ui_state(ui_state)
        }))
    })
    .await?;

    emit_profile_config_updated(
        state,
        profile_id,
        json!({
            "sessionOrganization": {
                "knownTags": payload.get("knownTags").cloned().unwrap_or_else(|| json!([]))
            }
        }),
    )
    .await;
    emit_session_summary_updated(state, profile_id, session_id, None, None).await;

    Ok(payload)
}

pub(crate) async fn save_session_title_metadata(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    name: Option<&str>,
) -> ApiResult<()> {
    let next_name = name
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "New thread")
        .map(str::to_string);

    with_ui_state_write(state, profile_id, |ui_state| {
        let Some(session_meta_by_thread_id) = ui_state
            .get_mut("sessionMetaByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "session metadata state is missing",
            ));
        };

        let current = session_meta_by_thread_id
            .get(session_id)
            .cloned()
            .unwrap_or_else(|| json!({ "pinned": false, "tags": [] }));
        let mut meta_object = current.as_object().cloned().unwrap_or_default();
        if let Some(next_name) = &next_name {
            meta_object.insert("name".to_string(), Value::String(next_name.clone()));
            meta_object.insert("nameUpdatedAt".to_string(), json!(now_unix_ms()));
        } else {
            meta_object.remove("name");
            meta_object.remove("nameUpdatedAt");
        }

        let pinned = meta_object
            .get("pinned")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let has_tags = meta_object
            .get("tags")
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty());
        let has_title = meta_object
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty());

        if !pinned && !has_tags && !has_title {
            session_meta_by_thread_id.remove(session_id);
        } else {
            meta_object
                .entry("pinned".to_string())
                .or_insert_with(|| json!(false));
            meta_object
                .entry("tags".to_string())
                .or_insert_with(|| json!([]));
            session_meta_by_thread_id.insert(session_id.to_string(), Value::Object(meta_object));
        }
        Ok(())
    })
    .await
}

pub(crate) async fn save_session_preferences_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    preferences: Value,
) -> ApiResult<Value> {
    let next_preferences =
        normalize_session_preferences_payload(state, profile_id, preferences).await?;
    with_ui_state_write(state, profile_id, |ui_state| {
        let Some(preferences_by_thread_id) = ui_state
            .get_mut("preferencesByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "preferences state is missing",
            ));
        };
        preferences_by_thread_id.insert(session_id.to_string(), next_preferences.clone());
        Ok(())
    })
    .await?;
    sync_codex_toml_with_preferences(
        &resolve_runtime_profile(&state.config, profile_id).codex_home,
        &next_preferences,
    )
    .await
    .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;

    emit_session_notification(
        state,
        profile_id,
        session_id,
        json!({
            "kind": "notification",
            "method": "codex-webui/preferencesUpdated",
            "params": {
                "preferences": next_preferences.clone()
            }
        }),
    )
    .await;
    emit_profile_config_updated(
        state,
        profile_id,
        json!({
            "defaults": next_preferences.clone()
        }),
    )
    .await;
    emit_session_summary_updated(
        state,
        profile_id,
        session_id,
        Some(next_preferences.clone()),
        None,
    )
    .await;

    Ok(next_preferences)
}

pub(crate) async fn save_session_skills_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    skills: Option<&Value>,
) -> ApiResult<Value> {
    let next_skills = selected_skills_from_value(skills);
    with_ui_state_write(state, profile_id, |ui_state| {
        let Some(skills_by_thread_id) = ui_state
            .get_mut("skillsByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "skills state is missing",
            ));
        };
        skills_by_thread_id.insert(session_id.to_string(), Value::Array(next_skills.clone()));
        Ok(())
    })
    .await?;

    emit_session_notification(
        state,
        profile_id,
        session_id,
        json!({
            "kind": "notification",
            "method": "codex-webui/skillsUpdated",
            "params": {
                "selectedSkills": next_skills.clone()
            }
        }),
    )
    .await;

    Ok(Value::Array(next_skills))
}

pub(crate) async fn rename_session_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    name: &str,
) -> ApiResult<Value> {
    let trimmed_name = name.trim();
    if trimmed_name.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Session name is required.",
        ));
    }

    app_server_client(state, profile_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to codex app-server: {error}"),
            )
        })?
        .request(
            "thread/name/set",
            json!({
                "threadId": session_id,
                "name": trimmed_name
            }),
        )
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to rename the session: {error}"),
            )
        })?;

    save_session_title_metadata(state, profile_id, session_id, Some(trimmed_name)).await?;
    emit_session_notification(
        state,
        profile_id,
        session_id,
        json!({
            "kind": "notification",
            "method": "thread/name/updated",
            "params": {
                "threadId": session_id,
                "threadName": trimmed_name
            }
        }),
    )
    .await;
    emit_session_summary_updated(state, profile_id, session_id, None, None).await;

    Ok(json!({
        "ok": true,
        "name": trimmed_name
    }))
}

pub(crate) async fn archive_session_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<Value> {
    app_server_client(state, profile_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to codex app-server: {error}"),
            )
        })?
        .request(
            "thread/archive",
            json!({
                "threadId": session_id
            }),
        )
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to archive the session: {error}"),
            )
        })?;

    invalidate_session_lists(state, profile_id).await;
    Ok(json!({ "ok": true }))
}

pub(crate) async fn unarchive_session_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<Value> {
    app_server_client(state, profile_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to codex app-server: {error}"),
            )
        })?
        .request(
            "thread/unarchive",
            json!({
                "threadId": session_id
            }),
        )
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to unarchive the session: {error}"),
            )
        })?;

    invalidate_session_lists(state, profile_id).await;
    let session = build_session_summary_payload(state, profile_id, session_id, None, None).await?;
    Ok(json!({
        "ok": true,
        "session": session
    }))
}
