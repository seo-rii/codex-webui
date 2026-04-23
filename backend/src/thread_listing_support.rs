use super::*;

const SESSION_THREAD_CACHE_TTL: Duration = Duration::from_secs(3);
const SESSION_SEARCH_TEXT_CACHE_TTL: Duration = Duration::from_secs(45);

fn session_thread_cache_key(profile_id: &str, archived: bool) -> String {
    format!("{profile_id}:archived={archived}")
}

fn session_search_text_cache_key(profile_id: &str, session_id: &str) -> String {
    format!("{profile_id}:{session_id}")
}

async fn invalidate_session_listing_cache(
    state: &AppState,
    profile_id: &str,
    session_id: Option<&str>,
) {
    let thread_prefix = format!("{profile_id}:");
    state
        .session_thread_cache
        .lock()
        .await
        .retain(|key, _| !key.starts_with(&thread_prefix));

    match session_id {
        Some(session_id) => {
            state
                .session_search_text_cache
                .lock()
                .await
                .remove(&session_search_text_cache_key(profile_id, session_id));
        }
        None => {
            state
                .session_search_text_cache
                .lock()
                .await
                .retain(|key, _| !key.starts_with(&thread_prefix));
        }
    }
}

pub(crate) fn build_session_summary_from_thread_payload(
    thread: &Value,
    snapshot: &SessionSummaryUiSnapshot,
    preferences_override: Option<Value>,
) -> ApiResult<Value> {
    let session_id = thread.get("id").and_then(Value::as_str).ok_or_else(|| {
        api_error(
            StatusCode::BAD_GATEWAY,
            "Codex app-server returned a thread without an id.",
        )
    })?;
    let meta = snapshot
        .session_meta_by_thread_id
        .get(session_id)
        .cloned()
        .unwrap_or_else(|| json!({ "pinned": false, "tags": [] }));
    let highlight = snapshot
        .highlights_by_thread_id
        .get(session_id)
        .cloned()
        .unwrap_or(Value::Null);
    let stored_preferences = snapshot
        .preferences_by_thread_id
        .get(session_id)
        .cloned()
        .unwrap_or(Value::Null);
    let preferences = preferences_override
        .filter(|value| !value.is_null())
        .or_else(|| (!stored_preferences.is_null()).then_some(stored_preferences))
        .unwrap_or_else(|| {
            json!({
                "cwd": thread.get("cwd").cloned().unwrap_or(Value::Null)
            })
        });
    let preview = thread
        .get("preview")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    Ok(json!({
        "id": session_id,
        "name": display_thread_name(
            thread.get("name").and_then(Value::as_str),
            Some(preview.as_str())
        ),
        "preview": preview,
        "queueCount": snapshot.queue_counts_by_thread_id.get(session_id).copied().unwrap_or(0),
        "highlight": highlight,
        "pinned": meta.get("pinned").and_then(Value::as_bool).unwrap_or(false),
        "tags": meta.get("tags").cloned().unwrap_or_else(|| json!([])),
        "cwd": thread
            .get("cwd")
            .cloned()
            .unwrap_or_else(|| preferences.get("cwd").cloned().unwrap_or(Value::Null)),
        "archived": thread.get("archived").and_then(Value::as_bool).unwrap_or(false),
        "createdAt": thread.get("createdAt").cloned().unwrap_or_else(|| json!(0)),
        "updatedAt": thread.get("updatedAt").cloned().unwrap_or_else(|| json!(0)),
        "status": normalized_thread_status(thread.get("status"))
            .unwrap_or_else(|| "unknown".to_string()),
        "isSubagent": thread.get("isSubagent").and_then(Value::as_bool).unwrap_or(false),
        "agentNickname": thread.get("agentNickname").cloned().unwrap_or(Value::Null),
        "agentRole": thread.get("agentRole").cloned().unwrap_or(Value::Null),
        "preferences": preferences
    }))
}

async fn list_app_server_threads(
    state: &AppState,
    profile_id: &str,
    archived: bool,
) -> ApiResult<Vec<Value>> {
    let cache_key = session_thread_cache_key(profile_id, archived);
    {
        let mut cache = state.session_thread_cache.lock().await;
        cache.retain(|_, entry| entry.created_at.elapsed() < SESSION_THREAD_CACHE_TTL);
        if let Some(cached) = cache.get(&cache_key) {
            return Ok(cached.threads.clone());
        }
    }

    let client = app_server_client(state, profile_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to codex app-server: {error}"),
            )
        })?;
    let mut cursor: Option<String> = None;
    let mut threads = Vec::new();

    loop {
        let response = client
            .request(
                "thread/list",
                json!({
                    "limit": 200,
                    "archived": archived,
                    "cursor": cursor.clone()
                }),
            )
            .await
            .map_err(|error| {
                api_error(
                    StatusCode::BAD_GATEWAY,
                    format!("Failed to list sessions: {error}"),
                )
            })?;
        let batch = response
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        threads.extend(batch);
        cursor = response
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if cursor.is_none() {
            break;
        }
    }

    state.session_thread_cache.lock().await.insert(
        cache_key,
        CachedSessionThreads {
            created_at: Instant::now(),
            threads: threads.clone(),
        },
    );
    Ok(threads)
}

async fn cached_session_search_text(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<String> {
    let cache_key = session_search_text_cache_key(profile_id, session_id);
    {
        let mut cache = state.session_search_text_cache.lock().await;
        cache.retain(|_, entry| entry.created_at.elapsed() < SESSION_SEARCH_TEXT_CACHE_TTL);
        if let Some(cached) = cache.get(&cache_key) {
            return Ok(cached.text.clone());
        }
    }

    let thread = read_thread_payload(state, profile_id, session_id, true).await?;
    let turns = thread.get("turns").cloned().unwrap_or_else(|| json!([]));
    let text = tokio::task::spawn_blocking(move || turns.to_string().to_lowercase())
        .await
        .map_err(|error| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to index the session for search: {error}"),
            )
        })?;

    state.session_search_text_cache.lock().await.insert(
        cache_key,
        CachedSessionSearchText {
            created_at: Instant::now(),
            text: text.clone(),
        },
    );
    Ok(text)
}

async fn collect_session_summaries_payload(
    state: &AppState,
    profile_id: &str,
    archived: bool,
    filter: &SessionFilterCriteria,
) -> ApiResult<Vec<Value>> {
    let snapshot = read_session_summary_ui_snapshot(state, profile_id).await?;
    let mut summaries = Vec::new();

    for thread in list_app_server_threads(state, profile_id, archived).await? {
        if thread_is_subagent(&thread) {
            continue;
        }
        let summary = build_session_summary_from_thread_payload(&thread, &snapshot, None)?;
        if session_summary_matches_filter(&summary, filter) {
            summaries.push(summary);
        }
    }

    sort_session_summaries(&mut summaries);
    Ok(summaries)
}

pub(crate) async fn list_sessions_payload(
    state: &AppState,
    profile_id: &str,
    archived: bool,
    cursor: Option<&str>,
    limit: u64,
    filter: &SessionFilterCriteria,
) -> ApiResult<Value> {
    let sessions = collect_session_summaries_payload(state, profile_id, archived, filter).await?;
    Ok(session_summary_page(sessions, cursor, limit))
}

pub(crate) async fn search_sessions_payload(
    state: &AppState,
    profile_id: &str,
    query: &str,
    scope: &str,
    archived: bool,
    cursor: Option<&str>,
    limit: u64,
    filter: &SessionFilterCriteria,
) -> ApiResult<Value> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return list_sessions_payload(state, profile_id, archived, cursor, limit, filter).await;
    }

    let include_full_text = scope == "full";
    let sessions = collect_session_summaries_payload(state, profile_id, archived, filter).await?;
    let mut matched = Vec::new();

    for summary in sessions {
        if session_summary_matches_query(&summary, &needle) {
            matched.push(summary);
            continue;
        }

        if !include_full_text {
            continue;
        }

        let Some(session_id) = summary.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Ok(full_text) = cached_session_search_text(state, profile_id, session_id).await else {
            continue;
        };
        if full_text.contains(&needle) {
            matched.push(summary);
        }
    }
    Ok(session_summary_page(matched, cursor, limit))
}

pub(crate) async fn create_session_payload(
    state: &AppState,
    profile_id: &str,
    preferences: Value,
    selected_skills: Option<&Value>,
    name: Option<&str>,
) -> ApiResult<Value> {
    let next_preferences =
        normalize_session_preferences_payload(state, profile_id, preferences).await?;
    let next_selected_skills = selected_skills_from_value(selected_skills);
    let session_preferences = next_preferences.as_object().cloned().ok_or_else(|| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Invalid preferences state.",
        )
    })?;
    let cwd = session_preferences
        .get("cwd")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "A working directory is required."))?;
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
            "thread/start",
            json!({
                "model": session_preferences.get("model").cloned().unwrap_or(Value::Null),
                "cwd": cwd,
                "approvalPolicy": session_preferences.get("approvalPolicy").cloned().unwrap_or_else(|| json!("on-request")),
                "sandbox": session_preferences.get("sandboxMode").cloned().unwrap_or_else(|| json!("workspace-write")),
                "personality": session_preferences.get("personality").cloned().unwrap_or(Value::Null),
                "config": preferences_model_context_config(&next_preferences),
                "serviceTier": match session_preferences.get("speed").and_then(Value::as_str) {
                    Some("fast") => Value::String("fast".to_string()),
                    Some("flex") => Value::String("flex".to_string()),
                    _ => Value::Null
                },
                "experimentalRawEvents": false,
                "persistExtendedHistory": true
            }),
        )
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to create the session: {error}"),
            )
        })?;
    let mut thread = response.get("thread").cloned().ok_or_else(|| {
        api_error(
            StatusCode::BAD_GATEWAY,
            "Codex app-server returned an invalid thread payload.",
        )
    })?;
    let session_id = thread
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            api_error(
                StatusCode::BAD_GATEWAY,
                "Codex app-server returned a session without an id.",
            )
        })?;

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
        preferences_by_thread_id.insert(session_id.clone(), next_preferences.clone());
        let Some(skills_by_thread_id) = ui_state
            .get_mut("skillsByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "skills state is missing",
            ));
        };
        skills_by_thread_id.insert(session_id.clone(), Value::Array(next_selected_skills));
        Ok(())
    })
    .await?;

    let next_name = name
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "New thread");
    if let Some(next_name) = next_name {
        client
            .request(
                "thread/name/set",
                json!({
                    "threadId": session_id.clone(),
                    "name": next_name
                }),
            )
            .await
            .map_err(|error| {
                api_error(
                    StatusCode::BAD_GATEWAY,
                    format!("Failed to name the session: {error}"),
                )
            })?;
        if let Some(thread_object) = thread.as_object_mut() {
            thread_object.insert("name".to_string(), Value::String(next_name.to_string()));
        }
    }

    let snapshot = read_session_summary_ui_snapshot(state, profile_id).await?;
    let summary = build_session_summary_from_thread_payload(
        &thread,
        &snapshot,
        Some(next_preferences.clone()),
    )?;
    invalidate_session_listing_cache(state, profile_id, Some(&session_id)).await;
    emit_profile_global_notification(
        state,
        profile_id,
        json!({
            "kind": "notification",
            "method": "codex-webui/sessionSummaryUpdated",
            "params": {
                "session": summary.clone()
            }
        }),
    )
    .await;

    Ok(summary)
}

pub(crate) async fn build_session_summary_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    preferences_override: Option<Value>,
) -> ApiResult<Value> {
    let thread = read_thread_metadata_payload(state, profile_id, session_id).await?;
    let snapshot = read_session_summary_ui_snapshot(state, profile_id).await?;
    let summary =
        build_session_summary_from_thread_payload(&thread, &snapshot, preferences_override)?;
    if summary.get("id").and_then(Value::as_str) != Some(session_id) {
        return Err(api_error(
            StatusCode::BAD_GATEWAY,
            "Session summary payload returned an unexpected session id.",
        ));
    }
    Ok(summary)
}

pub(crate) async fn emit_session_summary_updated(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    preferences_override: Option<Value>,
) {
    invalidate_session_listing_cache(state, profile_id, Some(session_id)).await;
    let summary =
        build_session_summary_payload(state, profile_id, session_id, preferences_override).await;
    if let Ok(summary) = summary {
        emit_profile_global_notification(
            state,
            profile_id,
            json!({
                "kind": "notification",
                "method": "codex-webui/sessionSummaryUpdated",
                "params": {
                    "session": summary
                }
            }),
        )
        .await;
    }
}

pub(crate) async fn invalidate_session_lists(state: &AppState, profile_id: &str) {
    invalidate_session_listing_cache(state, profile_id, None).await;
    emit_profile_global_notification(
        state,
        profile_id,
        json!({
            "kind": "notification",
            "method": "codex-webui/sessionListsInvalidated",
            "params": {}
        }),
    )
    .await;
}
