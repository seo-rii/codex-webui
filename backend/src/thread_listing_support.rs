use super::*;

const SESSION_THREAD_CACHE_TTL: Duration = Duration::from_secs(3);
const SESSION_SEARCH_TEXT_CACHE_TTL: Duration = Duration::from_secs(45);

#[cfg(test)]
pub(crate) static ROLLOUT_LISTING_HYDRATIONS_BY_PROJECT: std::sync::LazyLock<
    std::sync::Mutex<HashMap<String, usize>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

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
    let meta_name = meta
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "New thread");
    let display_name = if let Some(meta_name) = meta_name {
        if !thread_name_is_preview_fallback(thread_name, &preview) {
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
    let has_runtime_live_evidence = runtime_status.as_deref().is_some_and(is_live_thread_status)
        && runtime_status_updated_at > 0
        && runtime_status_updated_at >= thread_updated_at;
    let has_thread_live_evidence = thread_status.as_deref().is_some_and(is_live_thread_status)
        && runtime_status.is_some()
        && runtime_status_updated_at > 0
        && thread_updated_at >= runtime_status_updated_at;
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
    if !has_status_override && snapshot.active_thread_ids.contains(session_id) {
        status = "running".to_string();
    }
    if !has_status_override
        && is_live_thread_status(&status)
        && !has_direct_live_evidence
        && !has_runtime_live_evidence
        && !has_thread_live_evidence
    {
        status = "completed".to_string();
    }
    if snapshot.loaded_thread_ids_available
        && is_live_thread_status(&status)
        && !snapshot.active_thread_ids.contains(session_id)
        && !has_runtime_live_evidence
        && !snapshot.loaded_thread_ids.contains(session_id)
    {
        status = "completed".to_string();
    }

    Ok(json!({
        "id": session_id,
        "profileId": snapshot.profile_id.clone(),
        "profileLabel": snapshot.profile_label.clone(),
        "profileCodexHome": snapshot.profile_codex_home.clone(),
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
    let mut seen_cursors = HashSet::new();
    let mut page_count = 0usize;

    loop {
        let (batch, next_cursor) =
            list_app_server_thread_batch(state, profile_id, archived, cursor.as_deref(), 200)
                .await?;
        page_count += 1;
        threads.extend(batch);
        cursor = match bounded_pagination_cursor(
            &mut seen_cursors,
            next_cursor.as_deref(),
            page_count,
            32,
        ) {
            Ok(cursor) => cursor,
            Err(reason) => {
                warn!(
                    profile_id,
                    archived, reason, "stopped cyclic app-server thread pagination"
                );
                None
            }
        };
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

async fn read_session_listing_ui_snapshot(
    state: &AppState,
    profile_id: &str,
) -> ApiResult<SessionSummaryUiSnapshot> {
    let (resolved_profile_id_ref, resolved_profile) =
        resolve_runtime_profile_entry(&state.config, profile_id);
    let resolved_profile_id = resolved_profile_id_ref.to_string();
    let profile_label = resolved_profile.label.clone();
    let profile_codex_home = resolved_profile.codex_home.display().to_string();
    let runtime_key_prefix = format!("profile::{resolved_profile_id}::session-runtime::");
    let mut active_thread_ids = state
        .active_turns
        .lock()
        .await
        .keys()
        .filter_map(|key| key.strip_prefix(&runtime_key_prefix).map(str::to_string))
        .collect::<HashSet<_>>();
    active_thread_ids.extend(
        state
            .pending_turn_starts
            .lock()
            .await
            .iter()
            .filter_map(|key| key.strip_prefix(&runtime_key_prefix).map(str::to_string)),
    );

    with_ui_state_read(state, profile_id, |ui_state| {
        let queue_counts_by_thread_id = ui_state
            .get("queuesByThreadId")
            .and_then(Value::as_object)
            .map(|queues| {
                queues
                    .iter()
                    .map(|(thread_id, queue)| {
                        (
                            thread_id.clone(),
                            queue
                                .get("items")
                                .and_then(Value::as_array)
                                .map(Vec::len)
                                .unwrap_or(0),
                        )
                    })
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default();

        Ok(SessionSummaryUiSnapshot {
            profile_id: resolved_profile_id.clone(),
            profile_label: profile_label.clone(),
            profile_codex_home: profile_codex_home.clone(),
            session_meta_by_thread_id: ui_state
                .get("sessionMetaByThreadId")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
            preferences_by_thread_id: ui_state
                .get("preferencesByThreadId")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
            highlights_by_thread_id: ui_state
                .get("highlightsByThreadId")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
            runtime_status_by_thread_id: ui_state
                .get("runtimeStatusByThreadId")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
            queue_counts_by_thread_id,
            active_thread_ids,
            loaded_thread_ids: HashSet::new(),
            loaded_thread_ids_available: false,
        })
    })
    .await
}

fn session_listing_profile_ids(
    state: &AppState,
    active_profile_id: &str,
    filter: &SessionFilterCriteria,
) -> Vec<String> {
    let (default_profile_id, profiles) = runtime_profiles_snapshot(&state.config);
    let mut profile_ids = if filter.profile_ids.is_empty() {
        profiles.keys().cloned().collect::<Vec<_>>()
    } else {
        filter
            .profile_ids
            .iter()
            .filter(|profile_id| profiles.contains_key(*profile_id))
            .cloned()
            .collect::<Vec<_>>()
    };

    if profile_ids.is_empty() {
        profile_ids.push(resolve_runtime_profile_entry(&state.config, active_profile_id).0);
    }

    profile_ids.sort_by(|left, right| {
        let left_active = left == active_profile_id || left == &default_profile_id;
        let right_active = right == active_profile_id || right == &default_profile_id;
        right_active.cmp(&left_active).then_with(|| left.cmp(right))
    });
    profile_ids.dedup();
    profile_ids
}

async fn collect_rollout_session_summaries_payload(
    state: &AppState,
    profile_id: &str,
    archived: bool,
    filter: &SessionFilterCriteria,
    needle: Option<&str>,
    include_full_text: bool,
) -> ApiResult<Option<Vec<Value>>> {
    let mut candidates = list_rollout_candidates_payload(state, profile_id, archived).await?;
    if candidates.is_empty() {
        return Ok(None);
    }

    let snapshot = read_session_listing_ui_snapshot(state, profile_id).await?;
    candidates.sort_by(|left, right| {
        let updated_difference = normalize_session_timestamp(candidate_effective_updated_at(right))
            .cmp(&normalize_session_timestamp(
                candidate_effective_updated_at(left),
            ));
        if updated_difference != std::cmp::Ordering::Equal {
            return updated_difference;
        }
        right
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(left.get("id").and_then(Value::as_str).unwrap_or_default())
    });

    let matched_state_ids = match needle {
        Some(needle) => {
            search_state_thread_ids_payload(state, profile_id, archived, needle).await?
        }
        None => None,
    };
    let mut summaries = Vec::new();
    for candidate_chunk in candidates.chunks(128) {
        let hydrated_candidates = candidate_chunk
            .iter()
            .filter(|candidate| {
                candidate_matches_session_filter_snapshot(candidate, &snapshot, filter)
            })
            .cloned()
            .collect::<Vec<_>>();
        if hydrated_candidates.is_empty() {
            continue;
        }

        #[cfg(test)]
        {
            let project_key = state.config.project_root.display().to_string();
            let mut hydration_counts = ROLLOUT_LISTING_HYDRATIONS_BY_PROJECT
                .lock()
                .expect("rollout listing hydration counter should not be poisoned");
            *hydration_counts.entry(project_key).or_default() += hydrated_candidates.len();
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
            summaries.push(summary);
        }
    }
    sort_session_summaries(&mut summaries);
    Ok(Some(summaries))
}

async fn collect_session_summaries_with_query_payload(
    state: &AppState,
    profile_id: &str,
    archived: bool,
    filter: &SessionFilterCriteria,
    needle: Option<&str>,
    include_full_text: bool,
) -> ApiResult<Vec<Value>> {
    if let Some(mut summaries) = collect_rollout_session_summaries_payload(
        state,
        profile_id,
        archived,
        filter,
        needle,
        include_full_text,
    )
    .await?
    {
        if needle.is_none() && !archived {
            merge_missing_runtime_session_summaries(state, profile_id, filter, &mut summaries)
                .await?;
        }
        return Ok(summaries);
    }

    let mut sessions = Vec::new();
    if needle.is_none() && !archived {
        merge_missing_runtime_session_summaries(state, profile_id, filter, &mut sessions).await?;
    }
    let Some(needle) = needle else {
        return Ok(sessions);
    };
    let mut matched = Vec::new();
    for summary in sessions {
        if session_summary_matches_query(&summary, needle) {
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
        if full_text.contains(needle) {
            matched.push(summary);
        }
    }
    Ok(matched)
}

async fn merge_missing_runtime_session_summaries(
    state: &AppState,
    profile_id: &str,
    filter: &SessionFilterCriteria,
    summaries: &mut Vec<Value>,
) -> ApiResult<()> {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    let existing = summaries
        .iter()
        .filter_map(|summary| {
            summary
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect::<HashSet<_>>();
    let mut candidate_statuses = HashMap::<String, String>::new();
    let runtime_key_prefix = format!("profile::{resolved_profile_id}::session-runtime::");

    for runtime_key in state.active_turns.lock().await.keys() {
        if let Some(session_id) = runtime_key.strip_prefix(&runtime_key_prefix) {
            candidate_statuses.insert(session_id.to_string(), "running".to_string());
        }
    }

    for runtime_key in state.pending_turn_starts.lock().await.iter() {
        if let Some(session_id) = runtime_key.strip_prefix(&runtime_key_prefix) {
            candidate_statuses
                .entry(session_id.to_string())
                .or_insert_with(|| "starting".to_string());
        }
    }

    for (session_id, status) in candidate_statuses {
        if existing.contains(&session_id) {
            continue;
        }
        let summary = build_lightweight_session_summary_payload(
            state,
            profile_id,
            &session_id,
            None,
            &status,
        )
        .await;
        if session_summary_matches_filter(&summary, filter) {
            summaries.push(summary);
        }
    }
    sort_session_summaries(summaries);
    Ok(())
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
    window_size: usize,
    filter: &SessionFilterCriteria,
    needle: Option<&str>,
    include_full_text: bool,
) -> ApiResult<Option<Value>> {
    let candidates = list_rollout_candidates_payload(state, profile_id, archived).await?;
    if candidates.is_empty() {
        return Ok(None);
    }

    let snapshot = read_session_listing_ui_snapshot(state, profile_id).await?;
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
    let candidate_updated_at =
        |candidate: &Value| normalize_session_timestamp(candidate_effective_updated_at(candidate));
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
    let window_size = window_size.max(1);
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
            .cloned()
            .collect::<Vec<_>>();
        if hydrated_candidates.is_empty() {
            continue;
        }

        #[cfg(test)]
        {
            let project_key = state.config.project_root.display().to_string();
            let mut hydration_counts = ROLLOUT_LISTING_HYDRATIONS_BY_PROJECT
                .lock()
                .expect("rollout listing hydration counter should not be poisoned");
            *hydration_counts.entry(project_key).or_default() += hydrated_candidates.len();
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
    let listing_profile_ids = session_listing_profile_ids(state, profile_id, filter);
    if listing_profile_ids.len() != 1 {
        let window_size = limit.clamp(1, 200) as usize;
        let start = cursor
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or(0);
        let profile_window_size = start.saturating_add(window_size).saturating_add(1);
        let mut sessions = Vec::new();

        for listing_profile_id in listing_profile_ids {
            let mut profile_sessions = if let Some(payload) =
                scan_rollout_sessions_with_query_payload(
                    state,
                    &listing_profile_id,
                    archived,
                    None,
                    profile_window_size,
                    filter,
                    None,
                    false,
                )
                .await?
            {
                payload
                    .get("sessions")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
            } else {
                collect_session_summaries_with_query_payload(
                    state,
                    &listing_profile_id,
                    archived,
                    filter,
                    None,
                    false,
                )
                .await?
            };

            if !archived {
                merge_missing_runtime_session_summaries(
                    state,
                    &listing_profile_id,
                    filter,
                    &mut profile_sessions,
                )
                .await?;
            }
            sort_session_summaries(&mut profile_sessions);
            profile_sessions.truncate(profile_window_size);
            sessions.extend(profile_sessions);
        }

        sessions.sort_by(|left, right| {
            if left.get("id").and_then(Value::as_str) == right.get("id").and_then(Value::as_str) {
                let left_profile = left
                    .get("profileId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let right_profile = right
                    .get("profileId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                return left_profile.cmp(right_profile);
            }
            std::cmp::Ordering::Equal
        });
        sort_session_summaries(&mut sessions);
        return Ok(session_summary_page(sessions, cursor, limit));
    }
    let profile_id = listing_profile_ids
        .first()
        .map(String::as_str)
        .unwrap_or(profile_id);

    if let Some(payload) = scan_rollout_sessions_with_query_payload(
        state,
        profile_id,
        archived,
        cursor,
        limit.clamp(1, 200) as usize,
        filter,
        None,
        false,
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
    let listing_profile_ids = session_listing_profile_ids(state, profile_id, filter);
    if listing_profile_ids.len() != 1 {
        let window_size = limit.clamp(1, 200) as usize;
        let start = cursor
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or(0);
        let profile_window_size = start.saturating_add(window_size).saturating_add(1);
        let mut sessions = Vec::new();

        for listing_profile_id in listing_profile_ids {
            let mut profile_sessions = if let Some(payload) =
                scan_rollout_sessions_with_query_payload(
                    state,
                    &listing_profile_id,
                    archived,
                    None,
                    profile_window_size,
                    filter,
                    Some(&needle),
                    include_full_text,
                )
                .await?
            {
                payload
                    .get("sessions")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
            } else {
                collect_session_summaries_with_query_payload(
                    state,
                    &listing_profile_id,
                    archived,
                    filter,
                    Some(&needle),
                    include_full_text,
                )
                .await?
            };
            sort_session_summaries(&mut profile_sessions);
            profile_sessions.truncate(profile_window_size);
            sessions.extend(profile_sessions);
        }

        sessions.sort_by(|left, right| {
            if left.get("id").and_then(Value::as_str) == right.get("id").and_then(Value::as_str) {
                let left_profile = left
                    .get("profileId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let right_profile = right
                    .get("profileId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                return left_profile.cmp(right_profile);
            }
            std::cmp::Ordering::Equal
        });
        sort_session_summaries(&mut sessions);
        return Ok(session_summary_page(sessions, cursor, limit));
    }
    let profile_id = listing_profile_ids
        .first()
        .map(String::as_str)
        .unwrap_or(profile_id);

    if let Some(payload) = scan_rollout_sessions_with_query_payload(
        state,
        profile_id,
        archived,
        cursor,
        limit.clamp(1, 200) as usize,
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
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    let task_key = format!("{resolved_profile_id}:{session_id}");
    let assignment_fence = capture_session_assignment_fence(state, profile_id, session_id).await;
    let state_for_task = state.clone();
    let profile_id = profile_id.to_string();
    let session_id = session_id.to_string();
    let status_override = status_override.map(str::to_string);
    let handle = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(120)).await;
        if !session_assignment_fence_is_current(&state_for_task, &assignment_fence).await {
            return;
        }
        let summary = if status_override
            .as_deref()
            .is_some_and(|status| status == "starting" || is_live_thread_status(status))
        {
            Ok(build_lightweight_session_summary_payload(
                &state_for_task,
                &profile_id,
                &session_id,
                preferences_override,
                status_override.as_deref().unwrap_or("running"),
            )
            .await)
        } else {
            build_session_summary_payload(
                &state_for_task,
                &profile_id,
                &session_id,
                preferences_override,
                status_override.as_deref(),
            )
            .await
        };
        if let Ok(summary) = summary {
            let session_lock =
                session_operation_lock(&state_for_task, &resolved_profile_id, &session_id).await;
            let _session_guard = session_lock.lock().await;
            if !session_assignment_fence_is_current(&state_for_task, &assignment_fence).await {
                return;
            }
            emit_profile_global_notification(
                &state_for_task,
                &profile_id,
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
    });
    let mut tasks = state.session_summary_update_tasks.lock().await;
    if let Some(existing) = tasks.insert(task_key, handle) {
        existing.abort();
    }
}

async fn build_lightweight_session_summary_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    preferences_override: Option<Value>,
    status: &str,
) -> Value {
    let fallback_summary = || {
        let (resolved_profile_id, profile) =
            resolve_runtime_profile_entry(&state.config, profile_id);
        json!({
            "id": session_id,
            "profileId": resolved_profile_id,
            "profileLabel": profile.label,
            "profileCodexHome": profile.codex_home.display().to_string(),
            "name": Value::Null,
            "preview": "",
            "queueCount": 0,
            "highlight": Value::Null,
            "pinned": false,
            "archived": false,
            "cwd": Value::Null,
            "model": Value::Null,
            "tags": [],
            "folderIds": [],
            "isSubagent": false,
            "agentNickname": Value::Null,
            "agentRole": Value::Null,
            "accountEmail": Value::Null,
            "accountType": Value::Null,
            "status": status,
            "createdAt": 0,
            "updatedAt": now_unix_ms()
        })
    };
    match read_local_thread_metadata_payload(state, profile_id, session_id).await {
        Ok(Some(thread)) => match read_session_listing_ui_snapshot(state, profile_id).await {
            Ok(snapshot) => build_session_summary_from_thread_payload(
                &thread,
                &snapshot,
                preferences_override,
                Some(status),
            )
            .unwrap_or_else(|_| fallback_summary()),
            Err(_) => fallback_summary(),
        },
        _ => fallback_summary(),
    }
}

pub(crate) async fn emit_session_status_summary_updated(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    preferences_override: Option<Value>,
    status: &str,
) {
    invalidate_session_listing_cache(state, profile_id, Some(session_id)).await;
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    let task_key = format!("{resolved_profile_id}:{session_id}");
    if let Some(existing) = state
        .session_summary_update_tasks
        .lock()
        .await
        .remove(&task_key)
    {
        existing.abort();
    }
    let summary = build_lightweight_session_summary_payload(
        state,
        profile_id,
        session_id,
        preferences_override,
        status,
    )
    .await;
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
