use super::*;
use crate::thread_read_support::{
    active_turn_id_from_turns, emit_session_notification, read_thread_payload,
};
use crate::thread_listing_support::emit_session_summary_updated;
use crate::thread_support::rename_session_payload;

async fn resolve_selected_attachment_records(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    attachment_ids: Option<&Value>,
) -> ApiResult<Vec<StoredAttachmentRecord>> {
    let requested_attachment_ids = string_array_from_value(attachment_ids);
    let requested_attachment_set = requested_attachment_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let attachments = list_session_attachment_records(state, profile_id, session_id)
        .await
        .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;

    if requested_attachment_set.is_empty() {
        return Ok(Vec::new());
    }

    Ok(attachments
        .into_iter()
        .filter(|attachment| requested_attachment_set.contains(attachment.id.as_str()))
        .collect())
}

fn build_turn_input_payload(
    prompt: &str,
    attachments: &[StoredAttachmentRecord],
    selected_skills: &[Value],
) -> (Vec<Value>, Vec<String>) {
    let mut additional_readable_roots = Vec::new();
    let mut readable_roots_seen = HashSet::new();
    let mut text_attachment_paths = Vec::new();
    let mut image_attachment_paths = Vec::new();

    for attachment in attachments {
        if let Some(path) = attachment.path.as_deref() {
            let readable_root = Path::new(path)
                .parent()
                .unwrap_or_else(|| Path::new(path))
                .display()
                .to_string();
            if readable_roots_seen.insert(readable_root.clone()) {
                additional_readable_roots.push(readable_root);
            }
            if attachment.kind.as_deref() == Some("image") {
                image_attachment_paths.push(path.to_string());
            } else {
                text_attachment_paths.push(path.to_string());
            }
        }
    }

    let skill_markers = selected_skills
        .iter()
        .filter_map(|skill| skill.get("name").and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .filter(|value| !prompt.contains(&format!("${value}")))
        .map(|value| format!("${value}"))
        .collect::<Vec<_>>();
    let text_body = if skill_markers.is_empty() {
        prompt.to_string()
    } else if prompt.trim().is_empty() {
        skill_markers.join("\n")
    } else {
        format!("{}\n\n{prompt}", skill_markers.join("\n"))
    };

    let mut input = vec![json!({
        "type": "text",
        "text": if text_attachment_paths.is_empty() {
            text_body.clone()
        } else {
            format!(
                "{ATTACHMENT_PREAMBLE_START}\n{}\n{ATTACHMENT_PREAMBLE_END}\n\n{text_body}",
                text_attachment_paths.join("\n")
            )
        },
        "text_elements": []
    })];
    input.extend(selected_skills.iter().cloned().map(|skill| {
        json!({
            "type": "skill",
            "name": skill.get("name").and_then(Value::as_str).unwrap_or_default(),
            "path": skill.get("path").and_then(Value::as_str).unwrap_or_default()
        })
    }));
    for image_path in image_attachment_paths {
        input.push(json!({
            "type": "localImage",
            "path": image_path
        }));
    }

    (input, additional_readable_roots)
}

pub(crate) async fn resolve_active_turn_id_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<Option<String>> {
    let runtime_key = runtime_session_key(
        resolve_runtime_profile_entry(&state.config, profile_id).0,
        session_id,
    );
    if let Some(turn_id) = state.active_turns.lock().await.get(&runtime_key).cloned() {
        return Ok(Some(turn_id));
    }

    let thread = read_thread_payload(state, profile_id, session_id, true).await?;
    let active_turn_id = thread
        .get("turns")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .and_then(active_turn_id_from_turns);
    if let Some(turn_id) = active_turn_id.clone() {
        state.active_turns.lock().await.insert(runtime_key, turn_id);
    }
    Ok(active_turn_id)
}

pub(crate) async fn send_turn_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    prompt: &str,
    attachment_ids: Option<&Value>,
    selected_skills: Option<&Value>,
    preferences: Value,
) -> ApiResult<Value> {
    let trimmed_prompt = prompt.trim();
    let attachments =
        resolve_selected_attachment_records(state, profile_id, session_id, attachment_ids).await?;
    let requested_selected_skills = selected_skills_from_value(selected_skills);

    if trimmed_prompt.is_empty() && attachments.is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "EMPTY_MESSAGE"));
    }

    cancel_scheduled_shutdown_for_activity(state, profile_id).await;

    let next_preferences =
        normalize_session_preferences_payload(state, profile_id, preferences).await?;
    let cwd = next_preferences
        .get("cwd")
        .and_then(Value::as_str)
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "A working directory is required."))?
        .to_string();
    let thread = read_thread_payload(state, profile_id, session_id, false).await?;
    let should_backfill_title =
        is_placeholder_thread_name(thread.get("name").and_then(Value::as_str));
    let next_selected_skills = if requested_selected_skills.is_empty() {
        with_ui_state_read(state, profile_id, |ui_state| {
            Ok(session_selected_skills_from_ui_state(ui_state, session_id))
        })
        .await?
    } else {
        requested_selected_skills
    };

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

    let client = app_server_client(state, profile_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to codex app-server: {error}"),
            )
        })?;
    if thread.get("status").and_then(Value::as_str) == Some("notLoaded") {
        client
            .request(
                "thread/resume",
                json!({
                    "threadId": session_id,
                    "persistExtendedHistory": true
                }),
            )
            .await
            .map_err(|error| {
                api_error(
                    StatusCode::BAD_GATEWAY,
                    format!("Failed to resume the session before sending: {error}"),
                )
            })?;
    }

    let (input, attachment_readable_roots) =
        build_turn_input_payload(trimmed_prompt, &attachments, &next_selected_skills);
    let mut readable_roots = vec![cwd.clone()];
    for readable_root in attachment_readable_roots {
        if !readable_roots.contains(&readable_root) {
            readable_roots.push(readable_root);
        }
    }

    let sandbox_mode = next_preferences
        .get("sandboxMode")
        .and_then(Value::as_str)
        .unwrap_or("workspace-write");
    let network_access = next_preferences
        .get("networkAccess")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let read_only_access = json!({
        "type": "restricted",
        "includePlatformDefaults": true,
        "readableRoots": readable_roots
    });
    let sandbox_policy = match sandbox_mode {
        "danger-full-access" => json!({
            "type": "dangerFullAccess"
        }),
        "read-only" => json!({
            "type": "readOnly",
            "access": read_only_access.clone(),
            "networkAccess": network_access
        }),
        _ => json!({
            "type": "workspaceWrite",
            "writableRoots": [cwd],
            "readOnlyAccess": read_only_access.clone(),
            "networkAccess": network_access,
            "excludeTmpdirEnvVar": false,
            "excludeSlashTmp": false
        }),
    };
    let model = next_preferences
        .get("model")
        .cloned()
        .unwrap_or(Value::Null);
    let response = client
        .request(
            "turn/start",
            json!({
                "threadId": session_id,
                "input": input,
                "cwd": next_preferences.get("cwd").cloned().unwrap_or(Value::Null),
                "approvalPolicy": next_preferences.get("approvalPolicy").cloned().unwrap_or_else(|| json!("on-request")),
                "sandboxPolicy": sandbox_policy,
                "model": model.clone(),
                "personality": next_preferences.get("personality").cloned().unwrap_or(Value::Null),
                "serviceTier": match next_preferences.get("speed").and_then(Value::as_str) {
                    Some("fast") => Value::String("fast".to_string()),
                    Some("flex") => Value::String("flex".to_string()),
                    _ => Value::Null
                },
                "effort": if next_preferences.get("mode").and_then(Value::as_str) == Some("plan") {
                    Value::Null
                } else {
                    next_preferences.get("effort").cloned().unwrap_or(Value::Null)
                },
                "collaborationMode": if next_preferences.get("mode").and_then(Value::as_str) == Some("plan") {
                    json!({
                        "mode": "plan",
                        "settings": {
                            "model": model,
                            "reasoning_effort": next_preferences.get("effort").cloned().unwrap_or(Value::Null),
                            "developer_instructions": Value::Null
                        }
                    })
                } else {
                    Value::Null
                }
            }),
        )
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to start the turn: {error}"),
            )
        })?;

    if let Some(turn_id) = response
        .get("turn")
        .and_then(|value| value.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
    {
        let runtime_key = runtime_session_key(
            resolve_runtime_profile_entry(&state.config, profile_id).0,
            session_id,
        );
        state.active_turns.lock().await.insert(runtime_key, turn_id);
    }

    clear_session_draft_payload(state, profile_id, session_id).await?;
    if should_backfill_title {
        if let Some(title) = infer_persisted_session_title(trimmed_prompt) {
            let _ = rename_session_payload(state, profile_id, session_id, &title).await;
        }
    }
    emit_session_summary_updated(
        state,
        profile_id,
        session_id,
        Some(next_preferences.clone()),
    )
    .await;

    Ok(json!({
        "ok": true,
        "turnId": response
            .get("turn")
            .and_then(|value| value.get("id"))
            .cloned()
            .unwrap_or(Value::Null)
    }))
}

pub(crate) async fn steer_turn_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    prompt: &str,
    attachment_ids: Option<&Value>,
    selected_skills: Option<&Value>,
) -> ApiResult<Value> {
    let trimmed_prompt = prompt.trim();
    if trimmed_prompt.is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "EMPTY_MESSAGE"));
    }

    let active_turn_id = resolve_active_turn_id_payload(state, profile_id, session_id).await?;
    let Some(active_turn_id) = active_turn_id else {
        return Err(api_error(StatusCode::CONFLICT, "NO_ACTIVE_TURN"));
    };

    let attachments =
        resolve_selected_attachment_records(state, profile_id, session_id, attachment_ids).await?;
    let requested_selected_skills = selected_skills_from_value(selected_skills);
    let next_selected_skills = if requested_selected_skills.is_empty() {
        with_ui_state_read(state, profile_id, |ui_state| {
            Ok(session_selected_skills_from_ui_state(ui_state, session_id))
        })
        .await?
    } else {
        requested_selected_skills
    };
    let (input, _) = build_turn_input_payload(trimmed_prompt, &attachments, &next_selected_skills);
    let client = app_server_client(state, profile_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to codex app-server: {error}"),
            )
        })?;
    client
        .request(
            "turn/steer",
            json!({
                "threadId": session_id,
                "expectedTurnId": active_turn_id,
                "input": input
            }),
        )
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to steer the active turn: {error}"),
            )
        })?;

    clear_session_draft_payload(state, profile_id, session_id).await?;
    Ok(json!({
        "ok": true,
        "turnId": active_turn_id
    }))
}
