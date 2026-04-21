use super::*;
use crate::thread_support::{
    active_turn_id_from_turns, build_session_summary_from_thread_payload, create_session_payload,
    emit_session_notification, emit_session_summary_updated, read_thread_payload,
    rename_session_payload,
};

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

pub(crate) async fn fork_session_payload(
    state: &AppState,
    profile_id: &str,
    source_session_id: &str,
    mode: &str,
    turn_id: Option<&str>,
    message_text: Option<&str>,
) -> ApiResult<Value> {
    let source_thread = read_thread_payload(state, profile_id, source_session_id, true).await?;
    let source_preferences = with_ui_state_read(state, profile_id, |ui_state| {
        Ok(ui_state
            .get("preferencesByThreadId")
            .and_then(Value::as_object)
            .and_then(|entries| entries.get(source_session_id))
            .cloned()
            .unwrap_or_else(|| {
                json!({
                    "cwd": source_thread.get("cwd").cloned().unwrap_or(Value::Null)
                })
            }))
    })
    .await?;
    let preferences =
        normalize_session_preferences_payload(state, profile_id, source_preferences).await?;
    let turns = source_thread
        .get("turns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let anchor_index = turn_id
        .filter(|value| !value.trim().is_empty())
        .and_then(|turn_id| {
            turns
                .iter()
                .position(|turn| turn.get("id").and_then(Value::as_str) == Some(turn_id))
        })
        .or_else(|| (!turns.is_empty()).then_some(turns.len() - 1));
    let visible_turns = anchor_index
        .map(|index| turns[..=index].to_vec())
        .unwrap_or_else(|| turns.clone());

    let strip_attachment_preamble = |value: &str| {
        let trimmed = value.trim();
        let Some(rest) = trimmed.strip_prefix(&format!("{ATTACHMENT_PREAMBLE_START}\n")) else {
            return trimmed.to_string();
        };
        let Some((_, tail)) = rest.split_once(&format!("\n{ATTACHMENT_PREAMBLE_END}")) else {
            return trimmed.to_string();
        };
        tail.trim_start_matches('\n').trim().to_string()
    };

    let mut selected_message_text = message_text
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if selected_message_text.is_none() {
        for turn in visible_turns.iter().rev() {
            let Some(items) = turn.get("items").and_then(Value::as_array) else {
                continue;
            };
            for item in items.iter().rev() {
                if item.get("type").and_then(Value::as_str) != Some("userMessage") {
                    continue;
                }
                let text = strip_attachment_preamble(
                    item.get("text")
                        .and_then(Value::as_str)
                        .or_else(|| item.get("message").and_then(Value::as_str))
                        .unwrap_or_default(),
                );
                if !text.is_empty() {
                    selected_message_text = Some(text);
                    break;
                }
            }
            if selected_message_text.is_some() {
                break;
            }
        }
    }
    if selected_message_text.is_none() {
        let preview = strip_attachment_preamble(
            source_thread
                .get("preview")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
        if !preview.is_empty() {
            selected_message_text = Some(preview);
        }
    }

    let mut draft = selected_message_text
        .as_deref()
        .map(strip_attachment_preamble)
        .unwrap_or_default();
    let source_name = display_thread_name(
        source_thread.get("name").and_then(Value::as_str),
        source_thread.get("preview").and_then(Value::as_str),
    );
    let next_name =
        infer_session_display_title(selected_message_text.as_deref().unwrap_or(draft.as_str()))
            .or_else(|| source_name.clone());

    if mode == "fork" {
        if draft.trim().is_empty() {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "There is no message to fork yet.",
            ));
        }

        let client = app_server_client(state, profile_id)
            .await
            .map_err(|error| {
                api_error(
                    StatusCode::BAD_GATEWAY,
                    format!("Failed to connect to codex app-server: {error}"),
                )
            })?;
        let response = client
            .request(
                "thread/fork",
                json!({
                    "threadId": source_session_id,
                    "model": preferences.get("model").cloned().unwrap_or(Value::Null),
                    "cwd": preferences.get("cwd").cloned().unwrap_or(Value::Null),
                    "approvalPolicy": preferences.get("approvalPolicy").cloned().unwrap_or_else(|| json!("on-request")),
                    "sandbox": preferences.get("sandboxMode").cloned().unwrap_or_else(|| json!("workspace-write")),
                    "serviceTier": match preferences.get("speed").and_then(Value::as_str) {
                        Some("fast") => Value::String("fast".to_string()),
                        Some("flex") => Value::String("flex".to_string()),
                        _ => Value::Null
                    },
                    "persistExtendedHistory": true
                }),
            )
            .await
            .map_err(|error| {
                api_error(
                    StatusCode::BAD_GATEWAY,
                    format!("Failed to fork the session: {error}"),
                )
            })?;
        let mut forked_thread = response.get("thread").cloned().ok_or_else(|| {
            api_error(
                StatusCode::BAD_GATEWAY,
                "Codex app-server returned an invalid fork payload.",
            )
        })?;
        let forked_session_id = forked_thread
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                api_error(
                    StatusCode::BAD_GATEWAY,
                    "Codex app-server returned a forked session without an id.",
                )
            })?
            .to_string();
        let rollback_turns = anchor_index
            .map(|index| turns.len().saturating_sub(index + 1))
            .unwrap_or(0);
        if rollback_turns > 0 {
            let rolled_back = client
                .request(
                    "thread/rollback",
                    json!({
                        "threadId": forked_session_id,
                        "numTurns": rollback_turns
                    }),
                )
                .await
                .map_err(|error| {
                    api_error(
                        StatusCode::BAD_GATEWAY,
                        format!("Failed to roll back the forked session: {error}"),
                    )
                })?;
            if let Some(thread) = rolled_back.get("thread").cloned() {
                forked_thread = thread;
            }
        }

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
            preferences_by_thread_id.insert(forked_session_id.clone(), preferences.clone());
            Ok(())
        })
        .await?;

        if let Some(name) = next_name
            .as_deref()
            .filter(|value| !is_placeholder_thread_name(Some(value)))
        {
            rename_session_payload(state, profile_id, &forked_session_id, name).await?;
            if let Some(thread_object) = forked_thread.as_object_mut() {
                thread_object.insert("name".to_string(), Value::String(name.to_string()));
            }
        }

        let snapshot = read_session_summary_ui_snapshot(state, profile_id).await?;
        let summary = build_session_summary_from_thread_payload(
            &forked_thread,
            &snapshot,
            Some(preferences.clone()),
        )?;
        emit_session_summary_updated(
            state,
            profile_id,
            &forked_session_id,
            Some(preferences.clone()),
        )
        .await;
        return Ok(json!({
            "session": summary,
            "draft": "",
            "mode": "fork"
        }));
    }

    if mode != "handoff" {
        return Err(api_error(StatusCode::BAD_REQUEST, "Unsupported fork mode."));
    }

    let source_name_for_handoff = source_name
        .clone()
        .unwrap_or_else(|| "Source thread".to_string());
    let preview = strip_attachment_preamble(
        source_thread
            .get("preview")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    );
    let mut entries = Vec::new();
    for turn in &visible_turns {
        let Some(items) = turn.get("items").and_then(Value::as_array) else {
            continue;
        };
        for item in items {
            let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
            if item_type != "userMessage" && item_type != "agentMessage" {
                continue;
            }
            let text = strip_attachment_preamble(
                item.get("text")
                    .and_then(Value::as_str)
                    .or_else(|| item.get("message").and_then(Value::as_str))
                    .unwrap_or_default(),
            );
            if !text.is_empty() {
                entries.push((item_type == "userMessage", text));
            }
        }
    }

    let mut sections = vec![format!(
        "Continue this task in a fresh thread.\n\nSource thread: {source_name_for_handoff}\nWorking directory: {}",
        source_thread
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or_default()
    )];
    if !preview.is_empty() {
        sections.push(format!("Current goal:\n{preview}"));
    }
    if let Some(selected_message_text) = selected_message_text
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        sections.push(format!("Focus request:\n{selected_message_text}"));
    }
    if !entries.is_empty() {
        sections.push(format!(
            "Recent context:\n{}",
            entries
                .iter()
                .rev()
                .take(8)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .map(|(is_user, text)| format!(
                    "- {}: {}",
                    if *is_user { "User" } else { "Assistant" },
                    text.split_whitespace().collect::<Vec<_>>().join(" ")
                ))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    sections.push(
        "Continue from this handoff, preserve any existing constraints, and begin with the most sensible next step."
            .to_string(),
    );
    draft = sections
        .into_iter()
        .map(|section| section.trim().to_string())
        .filter(|section| !section.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if draft.trim().is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "There is no thread context to hand off yet.",
        ));
    }

    let handoff_name = format!(
        "{} · Handoff",
        if source_name_for_handoff.trim().is_empty()
            || is_placeholder_thread_name(Some(source_name_for_handoff.as_str()))
        {
            infer_session_display_title(&draft).unwrap_or_else(|| "Thread".to_string())
        } else {
            source_name_for_handoff
        }
    );
    let session =
        create_session_payload(state, profile_id, preferences, None, Some(&handoff_name)).await?;
    let handoff_session_id = session
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            api_error(
                StatusCode::BAD_GATEWAY,
                "Forked session summary was invalid.",
            )
        })?
        .to_string();
    let saved_draft =
        save_session_draft_payload(state, profile_id, &handoff_session_id, &draft, "message")
            .await?;
    Ok(json!({
        "session": session,
        "draft": saved_draft
            .get("draft")
            .cloned()
            .unwrap_or_else(|| Value::String(draft)),
        "mode": "handoff"
    }))
}
