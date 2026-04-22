use super::*;

pub(crate) fn normalize_session_title_source(prompt: &str) -> String {
    prompt.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn is_placeholder_thread_name(name: Option<&str>) -> bool {
    name.map(str::trim)
        .is_none_or(|value| value.is_empty() || value == "New thread")
}

pub(crate) fn infer_session_display_title(prompt: &str) -> Option<String> {
    let normalized = normalize_session_title_source(prompt);
    if normalized.is_empty() {
        return None;
    }
    let candidate = normalized
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

pub(crate) fn infer_persisted_session_title(prompt: &str) -> Option<String> {
    let normalized = normalize_session_title_source(prompt);
    let title = infer_session_display_title(prompt)?;
    (title != normalized).then_some(title)
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
    pub(crate) queue_counts_by_thread_id: HashMap<String, usize>,
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

fn session_sort_priority(status: Option<&str>) -> i32 {
    match status.unwrap_or_default() {
        "running" | "active" => 1,
        _ => 0,
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
            .unwrap_or(0)
            .cmp(&left.get("updatedAt").and_then(Value::as_i64).unwrap_or(0));
        if updated_difference != std::cmp::Ordering::Equal {
            return updated_difference;
        }

        right
            .get("createdAt")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .cmp(&left.get("createdAt").and_then(Value::as_i64).unwrap_or(0))
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

    json!({
        "sessions": page,
        "nextCursor": next_cursor
    })
}

pub(crate) async fn read_session_summary_ui_snapshot(
    state: &AppState,
    profile_id: &str,
) -> ApiResult<SessionSummaryUiSnapshot> {
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
            queue_counts_by_thread_id,
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
