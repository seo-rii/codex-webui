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
