use super::*;

pub(crate) async fn normalize_session_preferences_payload(
    state: &AppState,
    profile_id: &str,
    preferences: Value,
) -> ApiResult<Value> {
    let defaults = session_preferences_defaults_payload(state, profile_id)
        .await
        .as_object()
        .cloned()
        .unwrap_or_default();
    let mut next_preferences = defaults;
    if let Some(overrides) = preferences.as_object() {
        for (key, value) in overrides {
            next_preferences.insert(key.clone(), value.clone());
        }
    }

    let cwd = next_preferences
        .get("cwd")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    next_preferences.insert(
        "cwd".to_string(),
        Value::String(resolve_allowed_directory(state, &cwd).await?),
    );
    let normalized_git_repo_path = normalize_git_repo_path(
        state,
        next_preferences.get("gitRepoPath").unwrap_or(&Value::Null),
    )
    .await?;
    next_preferences.insert("gitRepoPath".to_string(), normalized_git_repo_path);
    next_preferences.insert(
        "personality".to_string(),
        Value::String(
            next_preferences
                .get("personality")
                .and_then(Value::as_str)
                .filter(|value| matches!(*value, "none" | "friendly" | "pragmatic"))
                .unwrap_or("pragmatic")
                .to_string(),
        ),
    );
    next_preferences.insert(
        "modelContextWindow".to_string(),
        preferences_model_context_window(&Value::Object(next_preferences.clone()))
            .map(Value::from)
            .unwrap_or(Value::Null),
    );

    Ok(Value::Object(next_preferences))
}
