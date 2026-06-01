use super::*;

const SESSION_THREAD_CACHE_TTL: Duration = Duration::from_secs(3);
const SESSION_SEARCH_TEXT_CACHE_TTL: Duration = Duration::from_secs(45);

fn session_thread_cache_key(
    profile_id: &str,
    archived: bool,
    cursor: Option<&str>,
    limit: u64,
) -> String {
    format!(
        "{profile_id}:archived={archived}:cursor={}:limit={}",
        cursor.unwrap_or_default(),
        limit.clamp(1, 200)
    )
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
    status_override: Option<&str>,
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
    let thread_name = thread.get("name").and_then(Value::as_str);
    let inferred_preview_title = infer_session_display_title(&preview);
    let meta_name = meta
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "New thread");
    let display_name = if let Some(meta_name) = meta_name {
        let thread_name_trimmed = thread_name.map(str::trim).unwrap_or_default();
        if !is_placeholder_thread_name(thread_name)
            && inferred_preview_title.as_deref() != Some(thread_name_trimmed)
        {
            display_thread_name(thread_name, Some(preview.as_str()))
        } else {
            Some(meta_name.to_string())
        }
    } else {
        display_thread_name(thread_name, Some(preview.as_str()))
    };
    let thread_status = normalized_thread_status(thread.get("status"));
    let thread_updated_at = thread
        .get("updatedAt")
        .and_then(Value::as_i64)
        .map(normalize_session_timestamp)
        .unwrap_or(0);
    let runtime_status_value = snapshot.runtime_status_by_thread_id.get(session_id);
    let runtime_status =
        runtime_status_value.and_then(|value| normalized_thread_status(Some(value)));
    let runtime_status_updated_at = runtime_status_value
        .and_then(|value| value.get("updatedAt"))
        .and_then(Value::as_i64)
        .map(normalize_session_timestamp)
        .unwrap_or(0);
    let has_direct_live_evidence = snapshot.active_thread_ids.contains(session_id);
    let has_status_override = status_override
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    let mut status = status_override
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(
            || match (thread_status.as_deref(), runtime_status.as_deref()) {
                (Some(thread_status), Some(runtime_status)) => {
                    if runtime_status_updated_at > 0
                        && runtime_status_updated_at >= thread_updated_at
                    {
                        Some(runtime_status.to_string())
                    } else {
                        Some(thread_status.to_string())
                    }
                }
                (Some(thread_status), None) => Some(thread_status.to_string()),
                (None, Some(runtime_status)) => Some(runtime_status.to_string()),
                _ => None,
            },
        )
        .unwrap_or_else(|| "unknown".to_string());
    if !has_status_override
        && snapshot.active_thread_ids.contains(session_id)
        && !matches!(
            status.as_str(),
            "failed" | "error" | "cancelled" | "canceled" | "aborted"
        )
    {
        status = "running".to_string();
    }
    if !has_status_override && is_live_thread_status(&status) && !has_direct_live_evidence {
        status = "completed".to_string();
    }
    if snapshot.loaded_thread_ids_available
        && is_live_thread_status(&status)
        && !snapshot.active_thread_ids.contains(session_id)
        && !snapshot.loaded_thread_ids.contains(session_id)
    {
        status = "completed".to_string();
    }

    Ok(json!({
        "id": session_id,
        "name": display_name,
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
        "status": status,
        "isSubagent": thread_is_subagent(thread),
        "agentNickname": thread_agent_nickname(thread).map(Value::String).unwrap_or(Value::Null),
        "agentRole": thread_agent_role(thread).map(Value::String).unwrap_or(Value::Null),
        "preferences": preferences
    }))
}

pub(crate) fn project_thread_listing_payload(thread: &Value) -> Value {
    let object = thread.as_object().cloned().unwrap_or_default();
    let mut projected = serde_json::Map::new();
    for key in [
        "id",
        "name",
        "preview",
        "cwd",
        "archived",
        "createdAt",
        "updatedAt",
        "status",
        "isSubagent",
        "spawnedSubagent",
        "spawned_subagent",
        "agentNickname",
        "agentRole",
        "agent_nickname",
        "agent_role",
        "source",
    ] {
        if let Some(value) = object.get(key) {
            projected.insert(key.to_string(), value.clone());
        }
    }
    Value::Object(projected)
}

async fn list_app_server_thread_batch(
    state: &AppState,
    profile_id: &str,
    archived: bool,
    cursor: Option<&str>,
    limit: u64,
) -> ApiResult<(Vec<Value>, Option<String>)> {
    let normalized_limit = limit.clamp(1, 200);
    let cache_key = session_thread_cache_key(profile_id, archived, cursor, normalized_limit);
    {
        let mut cache = state.session_thread_cache.lock().await;
        cache.retain(|_, entry| entry.created_at.elapsed() < SESSION_THREAD_CACHE_TTL);
        if let Some(cached) = cache.get(&cache_key) {
            let next_cursor = (!cached.next_cursor.is_empty()).then(|| cached.next_cursor.clone());
            return Ok((cached.threads.clone(), next_cursor));
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
    let response = client
        .request(
            "thread/list",
            json!({
                "limit": normalized_limit,
                "archived": archived,
                "cursor": cursor,
                "useStateDbOnly": true
            }),
        )
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to list sessions: {error}"),
            )
        })?;
    let threads = response
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|thread| project_thread_listing_payload(&thread))
        .collect::<Vec<_>>();
    let next_cursor = response
        .get("nextCursor")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    {
        let mut cache = state.session_thread_cache.lock().await;
        cache.insert(
            cache_key,
            CachedSessionThreads {
                created_at: Instant::now(),
                threads: threads.clone(),
                next_cursor: next_cursor.clone().unwrap_or_default(),
            },
        );
        cache.retain(|_, entry| entry.created_at.elapsed() < SESSION_THREAD_CACHE_TTL);
        if cache.len() > SESSION_THREAD_CACHE_MAX_ENTRIES {
            let mut entries = cache
                .iter()
                .map(|(key, entry)| (key.clone(), entry.created_at))
                .collect::<Vec<_>>();
            entries.sort_by_key(|(_, created_at)| *created_at);
            for (key, _) in entries {
                if cache.len() <= SESSION_THREAD_CACHE_MAX_ENTRIES {
                    break;
                }
                cache.remove(&key);
            }
        }
    }

    Ok((threads, next_cursor))
}

async fn list_app_server_threads(
    state: &AppState,
    profile_id: &str,
    archived: bool,
) -> ApiResult<Vec<Value>> {
    let mut cursor: Option<String> = None;
    let mut threads = Vec::new();

    loop {
        let (batch, next_cursor) =
            list_app_server_thread_batch(state, profile_id, archived, cursor.as_deref(), 200)
                .await?;
        threads.extend(batch);
        cursor = next_cursor;
        if cursor.is_none() {
            break;
        }
    }
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

    {
        let mut cache = state.session_search_text_cache.lock().await;
        if text.len() <= SESSION_SEARCH_TEXT_CACHE_MAX_BYTES {
            cache.insert(
                cache_key,
                CachedSessionSearchText {
                    created_at: Instant::now(),
                    text_bytes: text.len(),
                    text: text.clone(),
                },
            );
        }
        cache.retain(|_, entry| entry.created_at.elapsed() < SESSION_SEARCH_TEXT_CACHE_TTL);
        let mut total_bytes = cache.values().map(|entry| entry.text_bytes).sum::<usize>();
        if cache.len() > SESSION_SEARCH_TEXT_CACHE_MAX_ENTRIES
            || total_bytes > SESSION_SEARCH_TEXT_CACHE_MAX_BYTES
        {
            let mut entries = cache
                .iter()
                .map(|(key, entry)| (key.clone(), entry.created_at))
                .collect::<Vec<_>>();
            entries.sort_by_key(|(_, created_at)| *created_at);
            for (key, _) in entries {
                if cache.len() <= SESSION_SEARCH_TEXT_CACHE_MAX_ENTRIES
                    && total_bytes <= SESSION_SEARCH_TEXT_CACHE_MAX_BYTES
                {
                    break;
                }
                if let Some(removed) = cache.remove(&key) {
                    total_bytes = total_bytes.saturating_sub(removed.text_bytes);
                }
            }
        }
    }
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
        let summary = build_session_summary_from_thread_payload(&thread, &snapshot, None, None)?;
        if session_summary_matches_filter(&summary, filter) {
            summaries.push(summary);
        }
    }

    sort_session_summaries(&mut summaries);
    Ok(summaries)
}

fn candidate_matches_session_filter_snapshot(
    candidate: &Value,
    snapshot: &SessionSummaryUiSnapshot,
    filter: &SessionFilterCriteria,
) -> bool {
    let Some(session_id) = candidate.get("id").and_then(Value::as_str) else {
        return false;
    };
    if filter.pinned_only
        && !snapshot
            .session_meta_by_thread_id
            .get(session_id)
            .and_then(|value| value.get("pinned"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        return false;
    }
    if filter.queued_only
        && snapshot
            .queue_counts_by_thread_id
            .get(session_id)
            .copied()
            .unwrap_or_default()
            == 0
    {
        return false;
    }
    if let Some(highlight) = &filter.highlight {
        if snapshot
            .highlights_by_thread_id
            .get(session_id)
            .and_then(|value| value.get("kind"))
            .and_then(Value::as_str)
            != Some(highlight.as_str())
        {
            return false;
        }
    }
    let session_tags = snapshot
        .session_meta_by_thread_id
        .get(session_id)
        .and_then(|value| value.get("tags"))
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();

    if filter.untagged_only && !session_tags.is_empty() {
        return false;
    }

    if filter.tags.is_empty() {
        return true;
    }

    filter
        .tags
        .iter()
        .all(|tag| session_tags.contains(tag.as_str()))
}

async fn scan_rollout_sessions_with_query_payload(
    state: &AppState,
    profile_id: &str,
    archived: bool,
    cursor: Option<&str>,
    limit: u64,
    filter: &SessionFilterCriteria,
    needle: Option<&str>,
    include_full_text: bool,
) -> ApiResult<Option<Value>> {
    let candidates = list_rollout_candidates_payload(state, profile_id, archived).await?;
    if candidates.is_empty() {
        return Ok(None);
    }

    let snapshot = read_session_summary_ui_snapshot(state, profile_id).await?;
    let candidate_priority = |candidate: &Value| {
        let Some(session_id) = candidate.get("id").and_then(Value::as_str) else {
            return 0;
        };
        if snapshot
            .session_meta_by_thread_id
            .get(session_id)
            .and_then(|value| value.get("pinned"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return 2;
        }
        if snapshot.active_thread_ids.contains(session_id)
            || snapshot
                .runtime_status_by_thread_id
                .get(session_id)
                .and_then(|value| normalized_thread_status(Some(value)))
                .as_deref()
                .is_some_and(|status| session_sort_priority(Some(status)) > 0)
        {
            return 1;
        }
        0
    };
    let candidate_updated_at = |candidate: &Value| {
        normalize_session_timestamp(
            candidate
                .get("indexedUpdatedAt")
                .and_then(Value::as_i64)
                .or_else(|| candidate.get("updatedAt").and_then(Value::as_i64))
                .unwrap_or_default(),
        )
    };
    let mut candidates = candidates;
    candidates.sort_by(|left, right| {
        let priority_difference = candidate_priority(right).cmp(&candidate_priority(left));
        if priority_difference != std::cmp::Ordering::Equal {
            return priority_difference;
        }
        let updated_difference = candidate_updated_at(right).cmp(&candidate_updated_at(left));
        if updated_difference != std::cmp::Ordering::Equal {
            return updated_difference;
        }
        right
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(left.get("id").and_then(Value::as_str).unwrap_or_default())
    });
    let start = cursor
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let window_size = limit.clamp(1, 200) as usize;
    let mut page = Vec::new();
    let mut matched_count = 0usize;
    let matched_state_ids = match needle {
        Some(needle) => {
            search_state_thread_ids_payload(state, profile_id, archived, needle).await?
        }
        None => None,
    };
    let chunk_size = window_size.saturating_mul(2).clamp(32, 128);
    let mut candidate_index = 0usize;
    let mut has_more = false;

    while candidate_index < candidates.len() && !has_more {
        let end = candidate_index
            .saturating_add(chunk_size)
            .min(candidates.len());
        let candidate_chunk = &candidates[candidate_index..end];
        candidate_index = end;

        let hydrated_candidates = candidate_chunk
            .iter()
            .filter(|candidate| {
                candidate_matches_session_filter_snapshot(candidate, &snapshot, filter)
            })
            .filter(|candidate| {
                let Some(needle) = needle else {
                    return true;
                };
                let indexed_match = candidate_matches_indexed_query(candidate, needle);
                let state_match = matched_state_ids.as_ref().is_some_and(|matched_ids| {
                    candidate
                        .get("id")
                        .and_then(Value::as_str)
                        .is_some_and(|session_id| matched_ids.contains(session_id))
                });
                indexed_match || state_match || include_full_text
            })
            .cloned()
            .collect::<Vec<_>>();
        if hydrated_candidates.is_empty() {
            continue;
        }

        let hydrated_threads = hydrate_rollout_candidates_to_threads_payload(
            state,
            profile_id,
            archived,
            &hydrated_candidates,
        )
        .await?;
        for (candidate, thread) in hydrated_candidates.iter().zip(hydrated_threads.into_iter()) {
            if thread_is_subagent(&thread) {
                continue;
            }
            let summary =
                build_session_summary_from_thread_payload(&thread, &snapshot, None, None)?;
            if !session_summary_matches_filter(&summary, filter) {
                continue;
            }
            if let Some(needle) = needle {
                let indexed_match = candidate_matches_indexed_query(candidate, needle);
                let state_match = matched_state_ids.as_ref().is_some_and(|matched_ids| {
                    summary
                        .get("id")
                        .and_then(Value::as_str)
                        .is_some_and(|session_id| matched_ids.contains(session_id))
                });
                let summary_match =
                    indexed_match || state_match || session_summary_matches_query(&summary, needle);
                if !summary_match
                    && (!include_full_text
                        || !rollout_candidate_contains_query_payload(candidate, needle).await?)
                {
                    continue;
                }
            }

            if matched_count < start {
                matched_count += 1;
                continue;
            }
            if page.len() < window_size {
                page.push(summary);
                matched_count += 1;
                continue;
            }
            matched_count += 1;
            has_more = true;
            break;
        }
    }

    let next_cursor = has_more.then(|| start.saturating_add(window_size).to_string());
    let (session_ids, summary_versions, state_hash) =
        session_summary_page_state(&page, next_cursor.as_deref());

    Ok(Some(json!({
        "sessions": page,
        "nextCursor": next_cursor,
        "sessionIds": session_ids,
        "summaryVersions": summary_versions,
        "stateHash": state_hash
    })))
}

pub(crate) async fn list_sessions_payload(
    state: &AppState,
    profile_id: &str,
    archived: bool,
    cursor: Option<&str>,
    limit: u64,
    filter: &SessionFilterCriteria,
) -> ApiResult<Value> {
    if let Some(payload) = scan_rollout_sessions_with_query_payload(
        state, profile_id, archived, cursor, limit, filter, None, false,
    )
    .await?
    {
        return Ok(payload);
    }

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
    if let Some(payload) = scan_rollout_sessions_with_query_payload(
        state,
        profile_id,
        archived,
        cursor,
        limit,
        filter,
        Some(&needle),
        include_full_text,
    )
    .await?
    {
        return Ok(payload);
    }

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
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    state.session_app_server_assignments.lock().await.insert(
        runtime_session_key(&resolved_profile_id, &session_id),
        resolved_profile_id,
    );

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
        None,
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
    status_override: Option<&str>,
) -> ApiResult<Value> {
    let thread = match read_local_thread_metadata_payload(state, profile_id, session_id).await? {
        Some(thread) => thread,
        None => read_thread_metadata_payload(state, profile_id, session_id).await?,
    };
    let snapshot = read_session_summary_ui_snapshot(state, profile_id).await?;
    let summary = build_session_summary_from_thread_payload(
        &thread,
        &snapshot,
        preferences_override,
        status_override,
    )?;
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
    status_override: Option<&str>,
) {
    invalidate_session_listing_cache(state, profile_id, Some(session_id)).await;
    let summary = build_session_summary_payload(
        state,
        profile_id,
        session_id,
        preferences_override,
        status_override,
    )
    .await;
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
