use super::*;
use crate::thread_listing_support::{
    build_session_summary_from_thread_payload, create_session_payload, emit_session_summary_updated,
};
use crate::thread_read_support::read_thread_payload;
use crate::thread_support::rename_session_payload;

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
