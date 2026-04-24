use super::*;

pub(crate) fn normalize_session_title_source(prompt: &str) -> String {
    prompt.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn is_placeholder_thread_name(name: Option<&str>) -> bool {
    name.map(str::trim)
        .is_none_or(|value| value.is_empty() || value == "New thread")
}

pub(crate) fn infer_session_display_title(prompt: &str) -> Option<String> {
    let trimmed = prompt.trim();
    let without_attachments =
        if let Some(rest) = trimmed.strip_prefix(&format!("{ATTACHMENT_PREAMBLE_START}\n")) {
            if let Some((_, tail)) = rest.split_once(&format!("\n{ATTACHMENT_PREAMBLE_END}")) {
                tail.trim_start_matches('\n').trim()
            } else {
                trimmed
            }
        } else {
            trimmed
        };
    let normalized = normalize_session_title_source(without_attachments);
    if normalized.is_empty() {
        return None;
    }
    let mut title_source = normalized.as_str();
    while let Some((token, remainder)) = title_source.split_once(char::is_whitespace) {
        let is_command_like = token
            .strip_prefix('$')
            .or_else(|| token.strip_prefix('/'))
            .is_some_and(|value| {
                !value.is_empty()
                    && value.chars().all(|character| {
                        character.is_ascii_alphanumeric() || character == '_' || character == '-'
                    })
            });
        if !is_command_like {
            break;
        }
        title_source = remainder.trim_start();
    }
    let candidate = title_source
        .chars()
        .take(60)
        .collect::<String>()
        .trim()
        .trim_end_matches(['.', '?', '!'])
        .trim()
        .to_string();
    if candidate.is_empty() {
        None
    } else if normalized.chars().count() > 60 {
        Some(format!("{candidate}..."))
    } else {
        Some(candidate)
    }
}

pub(crate) fn display_thread_name(name: Option<&str>, preview: Option<&str>) -> Option<String> {
    if !is_placeholder_thread_name(name) {
        name.map(str::trim).map(str::to_string)
    } else {
        infer_session_display_title(preview.unwrap_or_default())
    }
}

#[derive(Clone, Default)]
pub(crate) struct SessionFilterCriteria {
    pub(crate) pinned_only: bool,
    pub(crate) running_only: bool,
    pub(crate) queued_only: bool,
    pub(crate) highlight: Option<String>,
    pub(crate) tags: Vec<String>,
}

#[derive(Clone, Default)]
pub(crate) struct SessionSummaryUiSnapshot {
    pub(crate) session_meta_by_thread_id: serde_json::Map<String, Value>,
    pub(crate) preferences_by_thread_id: serde_json::Map<String, Value>,
    pub(crate) highlights_by_thread_id: serde_json::Map<String, Value>,
    pub(crate) runtime_status_by_thread_id: serde_json::Map<String, Value>,
    pub(crate) queue_counts_by_thread_id: HashMap<String, usize>,
    pub(crate) active_thread_ids: HashSet<String>,
}

pub(crate) fn session_filter_from_value(filter: Option<&Value>) -> SessionFilterCriteria {
    let mut tags = filter
        .and_then(|value| value.get("tags"))
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
        .unwrap_or_default();
    tags.sort();
    tags.dedup();

    SessionFilterCriteria {
        pinned_only: filter
            .and_then(|value| value.get("pinnedOnly"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        running_only: filter
            .and_then(|value| value.get("runningOnly"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        queued_only: filter
            .and_then(|value| value.get("queuedOnly"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        highlight: filter
            .and_then(|value| value.get("highlight"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| *value == "attention" || *value == "completed")
            .map(str::to_string),
        tags,
    }
}

pub(crate) fn session_filter_from_query(query: Option<&str>) -> SessionFilterCriteria {
    let mut tags = query_param_values(query, "filterTag")
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    tags.sort();
    tags.dedup();

    SessionFilterCriteria {
        pinned_only: query_param_value(query, "filterPinned").as_deref() == Some("true"),
        running_only: query_param_value(query, "filterRunning").as_deref() == Some("true"),
        queued_only: query_param_value(query, "filterQueued").as_deref() == Some("true"),
        highlight: query_param_value(query, "filterHighlight")
            .map(|value| value.trim().to_string())
            .filter(|value| value == "attention" || value == "completed"),
        tags,
    }
}

pub(crate) fn session_sort_priority(status: Option<&str>) -> i32 {
    match status.unwrap_or_default() {
        "running" | "active" => 1,
        _ => 0,
    }
}

pub(crate) fn normalize_session_timestamp(value: i64) -> i64 {
    if value <= 0 {
        return 0;
    }
    if value >= 1_000_000_000_000 {
        value
    } else {
        value.saturating_mul(1000)
    }
}

pub(crate) fn session_summary_matches_filter(
    summary: &Value,
    filter: &SessionFilterCriteria,
) -> bool {
    if filter.pinned_only
        && !summary
            .get("pinned")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        return false;
    }
    if filter.running_only
        && session_sort_priority(summary.get("status").and_then(Value::as_str)) == 0
    {
        return false;
    }
    if filter.queued_only
        && summary
            .get("queueCount")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            == 0
    {
        return false;
    }
    if let Some(highlight) = &filter.highlight {
        if summary
            .get("highlight")
            .and_then(|value| value.get("kind"))
            .and_then(Value::as_str)
            != Some(highlight.as_str())
        {
            return false;
        }
    }
    if filter.tags.is_empty() {
        return true;
    }

    let session_tags = summary
        .get("tags")
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

    filter
        .tags
        .iter()
        .all(|tag| session_tags.contains(tag.as_str()))
}

pub(crate) fn session_summary_matches_query(summary: &Value, needle: &str) -> bool {
    let haystack = format!(
        "{}\n{}",
        summary
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        summary
            .get("preview")
            .and_then(Value::as_str)
            .unwrap_or_default()
    )
    .to_lowercase();
    haystack.contains(needle)
}

pub(crate) fn sort_session_summaries(summaries: &mut [Value]) {
    summaries.sort_by(|left, right| {
        let pinned_difference = right
            .get("pinned")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            .cmp(&left.get("pinned").and_then(Value::as_bool).unwrap_or(false));
        if pinned_difference != std::cmp::Ordering::Equal {
            return pinned_difference;
        }

        let priority_difference =
            session_sort_priority(right.get("status").and_then(Value::as_str)).cmp(
                &session_sort_priority(left.get("status").and_then(Value::as_str)),
            );
        if priority_difference != std::cmp::Ordering::Equal {
            return priority_difference;
        }

        let updated_difference = right
            .get("updatedAt")
            .and_then(Value::as_i64)
            .map(normalize_session_timestamp)
            .unwrap_or(0)
            .cmp(
                &left
                    .get("updatedAt")
                    .and_then(Value::as_i64)
                    .map(normalize_session_timestamp)
                    .unwrap_or(0),
            );
        if updated_difference != std::cmp::Ordering::Equal {
            return updated_difference;
        }

        right
            .get("createdAt")
            .and_then(Value::as_i64)
            .map(normalize_session_timestamp)
            .unwrap_or(0)
            .cmp(
                &left
                    .get("createdAt")
                    .and_then(Value::as_i64)
                    .map(normalize_session_timestamp)
                    .unwrap_or(0),
            )
    });
}

pub(crate) fn session_summary_page(
    mut summaries: Vec<Value>,
    cursor: Option<&str>,
    limit: u64,
) -> Value {
    let window_size = limit.clamp(1, 200) as usize;
    let start = cursor
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let end = start.saturating_add(window_size).min(summaries.len());
    let next_cursor = (end < summaries.len()).then(|| end.to_string());
    let page = if start < summaries.len() {
        summaries.drain(start..end).collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let (session_ids, summary_versions, state_hash) =
        session_summary_page_state(&page, next_cursor.as_deref());

    json!({
        "sessions": page,
        "nextCursor": next_cursor,
        "sessionIds": session_ids,
        "summaryVersions": summary_versions,
        "stateHash": state_hash
    })
}

pub(crate) fn session_summary_page_state(
    sessions: &[Value],
    next_cursor: Option<&str>,
) -> (Vec<String>, serde_json::Map<String, Value>, String) {
    let mut session_ids = Vec::new();
    let mut summary_versions = serde_json::Map::new();

    for summary in sessions {
        let Some(session_id) = summary.get("id").and_then(Value::as_str) else {
            continue;
        };
        session_ids.push(session_id.to_string());
        summary_versions.insert(
            session_id.to_string(),
            Value::String(payload_cache_version(summary)),
        );
    }

    let state_hash = session_summary_list_state_hash(
        &session_ids,
        next_cursor,
        &summary_versions
            .iter()
            .filter_map(|(session_id, version)| {
                version
                    .as_str()
                    .map(|version| (session_id.clone(), version.to_string()))
            })
            .collect::<HashMap<_, _>>(),
    );

    (session_ids, summary_versions, state_hash)
}

pub(crate) fn session_summary_versions_from_value(
    value: Option<&Value>,
) -> Option<HashMap<String, String>> {
    value.and_then(Value::as_object).map(|versions| {
        versions
            .iter()
            .filter_map(|(session_id, version)| {
                version
                    .as_str()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|version| (session_id.clone(), version.to_string()))
            })
            .collect::<HashMap<_, _>>()
    })
}

pub(crate) fn session_summary_list_state_hash(
    session_ids: &[String],
    next_cursor: Option<&str>,
    summary_versions: &HashMap<String, String>,
) -> String {
    let mut source = format!("cursor={}\n", next_cursor.unwrap_or_default());
    for session_id in session_ids {
        source.push_str(session_id);
        source.push('\t');
        source.push_str(
            summary_versions
                .get(session_id)
                .map(String::as_str)
                .unwrap_or_default(),
        );
        source.push('\n');
    }
    fnv1a32_hex(source.as_bytes())
}

pub(crate) fn cacheable_session_list_response(
    payload: Value,
    known_version: Option<&str>,
    known_summary_versions: Option<HashMap<String, String>>,
    known_state_hash: Option<&str>,
) -> Value {
    let version = payload_cache_version(&payload);
    if known_version
        .map(str::trim)
        .is_some_and(|candidate| !candidate.is_empty() && candidate == version)
    {
        return json!({
            "cacheVersion": version,
            "notModified": true
        });
    }

    if known_version.is_some()
        && known_state_hash
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
        && known_summary_versions.is_some()
    {
        let known_versions = known_summary_versions.unwrap_or_default();
        let current_versions =
            session_summary_versions_from_value(payload.get("summaryVersions")).unwrap_or_default();
        let sessions = payload
            .get("sessions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let session_ids = payload
            .get("sessionIds")
            .and_then(Value::as_array)
            .map(|ids| {
                ids.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let current_session_ids = session_ids.iter().cloned().collect::<HashSet<_>>();
        let upserts = sessions
            .into_iter()
            .filter(|summary| {
                let Some(session_id) = summary.get("id").and_then(Value::as_str) else {
                    return false;
                };
                current_versions.get(session_id) != known_versions.get(session_id)
            })
            .collect::<Vec<_>>();
        let removes = known_versions
            .keys()
            .filter(|session_id| !current_session_ids.contains(*session_id))
            .cloned()
            .collect::<Vec<_>>();

        return json!({
            "cacheVersion": version,
            "notModified": false,
            "patch": {
                "baseCacheVersion": known_version.unwrap_or_default(),
                "baseStateHash": known_state_hash.unwrap_or_default(),
                "finalCacheVersion": version,
                "finalStateHash": payload.get("stateHash").cloned().unwrap_or(Value::Null),
                "sessionIds": session_ids,
                "summaryVersions": current_versions,
                "upserts": upserts,
                "removes": removes,
                "nextCursor": payload.get("nextCursor").cloned().unwrap_or(Value::Null)
            }
        });
    }

    let mut next_payload = payload;
    if let Some(payload_object) = next_payload.as_object_mut() {
        payload_object.insert("cacheVersion".to_string(), Value::String(version));
        payload_object.insert("notModified".to_string(), Value::Bool(false));
    }
    next_payload
}

pub(crate) async fn read_session_summary_ui_snapshot(
    state: &AppState,
    profile_id: &str,
) -> ApiResult<SessionSummaryUiSnapshot> {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    let mut active_thread_ids = state
        .active_turns
        .lock()
        .await
        .keys()
        .filter_map(|key| {
            key.strip_prefix(&format!(
                "profile::{resolved_profile_id}::session-runtime::"
            ))
            .map(str::to_string)
        })
        .collect::<HashSet<_>>();
    active_thread_ids.extend(
        state
            .pending_turn_starts
            .lock()
            .await
            .iter()
            .filter_map(|key| {
                key.strip_prefix(&format!(
                    "profile::{resolved_profile_id}::session-runtime::"
                ))
                .map(str::to_string)
            }),
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
        })
    })
    .await
}

pub(crate) fn value_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Value::Object(object) => {
            for key in ["text", "title", "value", "name", "status", "state"] {
                if let Some(text) = object.get(key).and_then(value_text) {
                    return Some(text);
                }
            }
            None
        }
        _ => None,
    }
}

pub(crate) fn normalized_thread_status(value: Option<&Value>) -> Option<String> {
    let Some(value) = value else {
        return None;
    };
    match value {
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Value::Object(object) => object
            .get("type")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| value_text(value)),
        _ => None,
    }
}

pub(crate) fn is_live_thread_status(status: &str) -> bool {
    matches!(status, "running" | "active")
}
