use super::*;

const SESSION_DETAIL_ROLLOUT_TAIL_INITIAL_BYTES: u64 = 8 * 1024 * 1024;
const SESSION_DETAIL_ROLLOUT_TAIL_MAX_BYTES: u64 = 8 * 1024 * 1024;
const SESSION_COMPLETION_ROLLOUT_TAIL_INITIAL_BYTES: u64 = 512 * 1024;
const SESSION_DETAIL_GOAL_TIMEOUT_MS: u64 = 150;
const SESSION_DETAIL_ACTIVE_RECONCILE_TIMEOUT_MS: u64 = 1_000;
const SESSION_DETAIL_INLINE_IMAGE_RESULT_MAX_CHARS: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SessionTurnDetailMode {
    Collapsed,
    Expanded,
}

pub(crate) struct LocalRolloutTurnWindow {
    pub(crate) turns: Vec<Value>,
    pub(crate) loaded_start: usize,
    pub(crate) known_total_turns: Option<usize>,
    pub(crate) truncated: bool,
    pub(crate) trailing_incomplete: bool,
    pub(crate) changed_during_read: bool,
    pub(crate) file_size: u64,
    pub(crate) token_usage: Value,
    pub(crate) recovery: Option<RolloutRecoveryInfoPayload>,
    pub(crate) modified_at_ms: Option<u64>,
}

pub(crate) struct LocalRolloutRuntimeEvidence {
    pub(crate) status: String,
    pub(crate) modified_at_ms: Option<u64>,
}

pub(crate) struct RolloutRecord {
    pub(crate) value: Value,
    pub(crate) absolute_offset: u64,
}

pub(crate) struct RolloutRecordWindow {
    pub(crate) records: Vec<RolloutRecord>,
    pub(crate) truncated: bool,
    pub(crate) corruption_detected: bool,
    pub(crate) trailing_incomplete: bool,
    pub(crate) changed_during_read: bool,
    pub(crate) file_size: u64,
    pub(crate) modified_at_ms: Option<u64>,
}

async fn clear_completed_session_highlight_on_open(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) {
    let has_completed_highlight = with_ui_state_read(state, profile_id, |ui_state| {
        Ok(ui_state
            .get("highlightsByThreadId")
            .and_then(Value::as_object)
            .and_then(|entries| entries.get(session_id))
            .and_then(|highlight| highlight.get("kind"))
            .and_then(Value::as_str)
            == Some("completed"))
    })
    .await
    .unwrap_or(false);

    if has_completed_highlight {
        set_session_highlight(state, profile_id, session_id, None).await;
    }
}

async fn session_pending_requests_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> Vec<Value> {
    let runtime_key = runtime_session_key(
        &resolve_runtime_profile_entry(&state.config, profile_id).0,
        session_id,
    );
    let mut requests = state
        .pending_server_requests
        .lock()
        .await
        .get(&runtime_key)
        .map(|entries| {
            entries
                .iter()
                .map(|(request_id, pending)| {
                    json!({
                        "id": request_id,
                        "method": pending.method,
                        "params": pending.params,
                        "createdAt": pending.created_at
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    requests.sort_by(|left, right| {
        right
            .get("createdAt")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(
                left.get("createdAt")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
    });
    requests
}

pub(crate) fn summarize_session_turn_for_detail_payload(
    turn: &Value,
    turn_index: usize,
    detail_mode: SessionTurnDetailMode,
) -> Value {
    let mut summarized = turn.as_object().cloned().unwrap_or_default();
    let turn_id = summarized
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("turn-{turn_index}"));
    let turn_status = value_text(summarized.get("status").unwrap_or(&Value::Null))
        .unwrap_or_else(|| "unknown".to_string());
    let compact_non_running_turn = turn_status != "inProgress";
    let mut hidden_item_count = 0usize;
    let items = summarized
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .enumerate()
        .filter_map(|(item_index, item)| {
            let mut normalized = normalize_session_item_payload(item, &turn_id, item_index);
            let item_type = normalized
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            if is_internal_session_item_type(&item_type) {
                return None;
            }
            if item_type == "imageGeneration" {
                if let Some(result_len) = normalized
                    .get("result")
                    .and_then(Value::as_str)
                    .map(str::len)
                {
                    if result_len > SESSION_DETAIL_INLINE_IMAGE_RESULT_MAX_CHARS {
                        if let Some(item_object) = normalized.as_object_mut() {
                            item_object.insert("result".to_string(), Value::Null);
                            item_object.insert("resultOmitted".to_string(), Value::Bool(true));
                            item_object.insert(
                                "detailState".to_string(),
                                Value::String("deferred".to_string()),
                            );
                            item_object.insert(
                                "detailPreview".to_string(),
                                Value::String(
                                    "Generated image payload is available on demand.".to_string(),
                                ),
                            );
                        }
                    }
                }
            }
            if detail_mode == SessionTurnDetailMode::Collapsed
                && compact_non_running_turn
                && !matches!(
                    item_type.as_str(),
                    "userMessage" | "agentMessage" | "plan" | "contextCompaction"
                )
            {
                hidden_item_count = hidden_item_count.saturating_add(1);
                return None;
            }
            Some(match item_type.as_str() {
                "commandExecution" | "fileChange" | "mcpToolCall" | "dynamicToolCall"
                | "webSearch" => prepare_session_deferred_item_payload(item, &turn_id, item_index),
                _ => normalized,
            })
        })
        .collect::<Vec<_>>();
    summarized.insert("id".to_string(), Value::String(turn_id));
    summarized.insert("items".to_string(), Value::Array(items));
    summarized.insert("status".to_string(), Value::String(turn_status));
    summarized
        .entry("error".to_string())
        .or_insert_with(|| Value::Null);
    summarized
        .entry("startedAt".to_string())
        .or_insert_with(|| Value::Null);
    summarized
        .entry("completedAt".to_string())
        .or_insert_with(|| Value::Null);
    summarized
        .entry("durationMs".to_string())
        .or_insert_with(|| Value::Null);
    summarized.insert(
        "detailState".to_string(),
        Value::String(
            if hidden_item_count > 0 {
                "summary"
            } else {
                "full"
            }
            .to_string(),
        ),
    );
    summarized.insert(
        "hiddenItemCount".to_string(),
        Value::from(hidden_item_count),
    );
    Value::Object(summarized)
}

fn summarize_completed_turn_tail_payload(turn: &Value, turn_index: usize) -> Value {
    let mut summarized = summarize_session_turn_for_detail_payload(
        turn,
        turn_index,
        SessionTurnDetailMode::Collapsed,
    );
    let Some(turn_object) = summarized.as_object_mut() else {
        return summarized;
    };
    let turn_id = turn_object
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("turn")
        .to_string();
    let items = turn_object
        .remove("items")
        .and_then(|items| items.as_array().cloned())
        .unwrap_or_default();
    let final_agent_index = items
        .iter()
        .rposition(|item| item.get("type").and_then(Value::as_str) == Some("agentMessage"));
    let mut additionally_hidden = 0usize;
    let visible_items = items
        .into_iter()
        .enumerate()
        .filter_map(|(index, mut item)| {
            let item_type = item.get("type").and_then(Value::as_str);
            if item_type == Some("userMessage")
                || (item_type == Some("agentMessage") && final_agent_index == Some(index))
            {
                if item_type == Some("agentMessage") {
                    let text = item.get("text").and_then(Value::as_str).unwrap_or_default();
                    let content_version = payload_cache_version(&json!({
                        "turnId": turn_id,
                        "type": "agentMessage",
                        "text": text,
                    }));
                    if let Some(item_object) = item.as_object_mut() {
                        item_object.insert(
                            "completionLineage".to_string(),
                            Value::String(format!("{turn_id}:final-agent:{content_version}")),
                        );
                    }
                }
                return Some(item);
            }
            additionally_hidden = additionally_hidden.saturating_add(1);
            None
        })
        .collect::<Vec<_>>();
    let hidden_item_count = turn_object
        .get("hiddenItemCount")
        .and_then(Value::as_u64)
        .unwrap_or_default()
        .saturating_add(additionally_hidden as u64);
    turn_object.insert("items".to_string(), Value::Array(visible_items));
    turn_object.insert(
        "hiddenItemCount".to_string(),
        Value::from(hidden_item_count),
    );
    turn_object.insert(
        "detailState".to_string(),
        Value::String(
            if hidden_item_count > 0 {
                "summary"
            } else {
                "full"
            }
            .to_string(),
        ),
    );
    summarized
}

fn summarize_turn_for_rollback_target(turn: &Value) -> String {
    let items = turn
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let text = items
        .iter()
        .find_map(|item| {
            if item.get("type").and_then(Value::as_str) != Some("userMessage") {
                return None;
            }
            item.get("text")
                .or_else(|| item.get("message"))
                .and_then(value_text)
        })
        .or_else(|| {
            items.iter().find_map(|item| {
                if item.get("type").and_then(Value::as_str) != Some("agentMessage") {
                    return None;
                }
                item.get("text")
                    .or_else(|| item.get("message"))
                    .and_then(value_text)
            })
        })
        .or_else(|| turn.get("id").and_then(value_text))
        .unwrap_or_else(|| "Rollback target".to_string())
        .replace(['\r', '\n', '\t'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if text.chars().count() > 120 {
        format!(
            "{}...",
            text.chars().take(120).collect::<String>().trim_end()
        )
    } else {
        text
    }
}

fn rollback_targets_from_turns(
    turns: &[Value],
    loaded_start: usize,
    known_total_turns: Option<usize>,
    truncated_before: bool,
) -> Value {
    let targets = turns
        .iter()
        .enumerate()
        .filter_map(|(index, turn)| {
            let later_turns = turns.len().saturating_sub(index + 1);
            if later_turns == 0 {
                return None;
            }
            let items = turn
                .get("items")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if !items
                .iter()
                .any(|item| item.get("type").and_then(Value::as_str) == Some("userMessage"))
            {
                return None;
            }
            Some(json!({
                "turnId": turn.get("id").cloned().unwrap_or(Value::Null),
                "turnIndex": loaded_start.saturating_add(index),
                "numTurns": later_turns,
                "preview": summarize_turn_for_rollback_target(turn),
                "startedAt": turn.get("startedAt").cloned().unwrap_or(Value::Null),
                "completedAt": turn.get("completedAt").cloned().unwrap_or(Value::Null)
            }))
        })
        .collect::<Vec<_>>();
    json!({
        "targets": targets,
        "loadedStart": loaded_start,
        "loadedTurns": turns.len(),
        "totalTurns": known_total_turns.map(Value::from).unwrap_or(Value::Null),
        "truncatedBefore": truncated_before
    })
}

fn parse_rollout_timestamp_ms(record: &Value) -> Option<i64> {
    record
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|value| {
            time::OffsetDateTime::parse(
                value.trim(),
                &time::format_description::well_known::Rfc3339,
            )
            .ok()
        })
        .map(|timestamp| timestamp.unix_timestamp_nanos() / 1_000_000)
        .and_then(|value| i64::try_from(value).ok())
}

fn new_rollout_turn(turn_id: String, status: &str, started_at: Option<i64>) -> Value {
    json!({
        "id": turn_id,
        "status": status,
        "error": Value::Null,
        "startedAt": started_at.map(Value::from).unwrap_or(Value::Null),
        "completedAt": Value::Null,
        "durationMs": Value::Null,
        "items": []
    })
}

fn find_rollout_turn_index(turns: &[Value], turn_id: &str) -> Option<usize> {
    turns
        .iter()
        .position(|turn| turn.get("id").and_then(Value::as_str) == Some(turn_id))
}

fn ensure_rollout_turn_index(
    turns: &mut Vec<Value>,
    current_index: &mut Option<usize>,
    turn_id: Option<&str>,
    timestamp_ms: Option<i64>,
) -> usize {
    if let Some(turn_id) = turn_id.map(str::trim).filter(|value| !value.is_empty()) {
        if let Some(index) = find_rollout_turn_index(turns, turn_id) {
            *current_index = Some(index);
            return index;
        }
        turns.push(new_rollout_turn(
            turn_id.to_string(),
            "inProgress",
            timestamp_ms,
        ));
        let index = turns.len().saturating_sub(1);
        *current_index = Some(index);
        return index;
    }

    if let Some(index) = *current_index {
        return index;
    }
    let turn_id = format!("rollout-turn-{}", turns.len());
    turns.push(new_rollout_turn(turn_id, "completed", timestamp_ms));
    let index = turns.len().saturating_sub(1);
    *current_index = Some(index);
    index
}

fn push_rollout_item(turn: &mut Value, item: Value) {
    if let Some(items) = turn.get_mut("items").and_then(Value::as_array_mut) {
        let item_id = item.get("id").and_then(Value::as_str).unwrap_or_default();
        if !item_id.is_empty()
            && items
                .iter()
                .any(|candidate| candidate.get("id").and_then(Value::as_str) == Some(item_id))
        {
            return;
        }
        items.push(item);
    }
}

fn update_rollout_item(turn: &mut Value, item_id: &str, update: impl FnOnce(&mut Value)) -> bool {
    let Some(items) = turn.get_mut("items").and_then(Value::as_array_mut) else {
        return false;
    };
    let Some(item) = items
        .iter_mut()
        .find(|item| item.get("id").and_then(Value::as_str) == Some(item_id))
    else {
        return false;
    };
    update(item);
    true
}

fn parse_function_call_arguments(payload: &Value) -> Value {
    payload
        .get("arguments")
        .or_else(|| payload.get("input"))
        .and_then(Value::as_str)
        .and_then(|arguments| serde_json::from_str::<Value>(arguments).ok())
        .or_else(|| payload.get("arguments").cloned())
        .or_else(|| payload.get("input").cloned())
        .unwrap_or_else(|| json!({}))
}

fn copy_rollout_item_metadata(payload: &Value, item: &mut serde_json::Map<String, Value>) {
    for (target_key, source_keys) in [
        ("annotations", ["annotations", "annotations"]),
        (
            "internalChatMessageMetadataPassthrough",
            [
                "internalChatMessageMetadataPassthrough",
                "internal_chat_message_metadata_passthrough",
            ],
        ),
        ("metadata", ["metadata", "metadata"]),
        ("_meta", ["_meta", "_meta"]),
    ] {
        if let Some(value) = source_keys
            .iter()
            .find_map(|source_key| payload.get(*source_key))
            .filter(|value| !value.is_null())
        {
            if let (Some(existing), Some(incoming)) = (
                item.get_mut(target_key).and_then(Value::as_object_mut),
                value.as_object(),
            ) {
                for (key, value) in incoming {
                    existing.insert(key.clone(), value.clone());
                }
            } else {
                item.insert(target_key.to_string(), value.clone());
            }
        }
    }
}

fn command_item_from_rollout_function_call(payload: &Value) -> Value {
    let call_id = payload
        .get("call_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let name = payload
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("tool");
    let arguments = parse_function_call_arguments(payload);
    let mut item = if matches!(name, "exec" | "exec_command" | "write_stdin") {
        json!({
            "id": call_id,
            "type": "commandExecution",
            "command": arguments
                .get("cmd")
                .or_else(|| arguments.get("chars"))
                .or_else(|| arguments.get("command"))
                .cloned()
                .unwrap_or_else(|| payload.get("input").cloned().unwrap_or_else(|| Value::String(name.to_string()))),
            "cwd": arguments.get("workdir").cloned().unwrap_or(Value::Null),
            "exitCode": Value::Null,
            "status": payload.get("status").cloned().unwrap_or_else(|| json!("running")),
            "arguments": arguments.clone(),
            "input": payload.get("input").cloned().unwrap_or(Value::Null),
            "invocation": {
                "tool": name,
                "arguments": arguments
            }
        })
    } else {
        json!({
            "id": call_id,
            "type": "dynamicToolCall",
            "tool": name,
            "status": payload.get("status").cloned().unwrap_or_else(|| json!("running")),
            "arguments": arguments.clone(),
            "input": payload.get("input").cloned().unwrap_or(Value::Null),
            "invocation": {
                "tool": name,
                "arguments": arguments
            }
        })
    };
    if let Some(item_object) = item.as_object_mut() {
        copy_rollout_item_metadata(payload, item_object);
    }
    item
}

fn mcp_tool_call_item_from_rollout_payload(payload: &Value, status: &str) -> Value {
    let invocation = payload
        .get("invocation")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let raw_result = payload.get("result").cloned().unwrap_or(Value::Null);
    let failed = raw_result.get("Err").is_some() || payload.get("error").is_some();
    let result = raw_result.get("Ok").cloned().unwrap_or_else(|| {
        (!failed)
            .then_some(raw_result.clone())
            .unwrap_or(Value::Null)
    });
    let error = raw_result
        .get("Err")
        .cloned()
        .or_else(|| payload.get("error").cloned())
        .unwrap_or(Value::Null);
    let mut item = json!({
        "id": payload
            .get("call_id")
            .or_else(|| payload.get("id"))
            .and_then(Value::as_str)
            .unwrap_or("mcp-tool"),
        "type": "mcpToolCall",
        "server": invocation.get("server").cloned().unwrap_or(Value::Null),
        "tool": invocation.get("tool").cloned().unwrap_or(Value::Null),
        "arguments": invocation.get("arguments").cloned().unwrap_or_else(|| json!({})),
        "invocation": invocation,
        "status": if failed { "failed" } else { status },
        "result": result,
        "error": error,
        "duration": payload.get("duration").cloned().unwrap_or(Value::Null)
    });
    if let Some(item_object) = item.as_object_mut() {
        copy_rollout_item_metadata(payload, item_object);
    }
    item
}

fn context_compaction_item_from_rollout_payload(
    payload: &Value,
    fallback_id: String,
    status: &str,
) -> Value {
    json!({
        "id": payload
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or(fallback_id),
        "type": "contextCompaction",
        "status": status
    })
}

fn file_change_item_from_patch_apply_payload(payload: &Value) -> Value {
    let changes = payload
        .get("changes")
        .and_then(Value::as_object)
        .map(|entries| {
            entries
                .iter()
                .map(|(path, change)| {
                    json!({
                        "path": path,
                        "kind": change
                            .get("type")
                            .and_then(Value::as_str)
                            .unwrap_or("update"),
                        "diff": change.get("unified_diff").cloned().unwrap_or(Value::Null)
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    json!({
        "id": payload.get("call_id").and_then(Value::as_str).unwrap_or("patch"),
        "type": "fileChange",
        "status": if payload.get("success").and_then(Value::as_bool).unwrap_or(false) {
            "completed"
        } else {
            "failed"
        },
        "changes": changes
    })
}

fn web_search_item_from_rollout_payload(
    payload: &Value,
    fallback_id: String,
    status: &str,
) -> Value {
    let mut item = json!({
        "id": payload
            .get("call_id")
            .or_else(|| payload.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or(fallback_id),
        "type": "webSearch",
        "status": status,
        "query": payload.get("query").cloned().unwrap_or(Value::Null),
        "action": payload.get("action").cloned().unwrap_or(Value::Null)
    });
    if let Some(object) = item.as_object_mut() {
        for (target_key, source_keys) in [
            ("summary", ["summary", "result_summary", "resultSummary"]),
            ("results", ["results", "search_results", "searchResults"]),
            ("sources", ["sources", "source_results", "sourceResults"]),
            (
                "citations",
                ["citations", "citation_results", "citationResults"],
            ),
        ] {
            if let Some(value) = source_keys
                .iter()
                .filter_map(|source_key| payload.get(*source_key))
                .find(|value| !value.is_null())
            {
                object.insert(target_key.to_string(), value.clone());
            }
        }
    }
    item
}

fn image_generation_item_from_rollout_payload(
    payload: &Value,
    fallback_id: String,
    status: &str,
) -> Value {
    json!({
        "id": payload
            .get("call_id")
            .or_else(|| payload.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or(fallback_id),
        "type": "imageGeneration",
        "status": payload
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or(status),
        "revisedPrompt": payload
            .get("revised_prompt")
            .or_else(|| payload.get("revisedPrompt"))
            .cloned()
            .unwrap_or(Value::Null),
        "result": payload.get("result").cloned().unwrap_or(Value::Null),
        "savedPath": payload
            .get("saved_path")
            .or_else(|| payload.get("savedPath"))
            .cloned()
            .unwrap_or(Value::Null)
    })
}

fn review_mode_item_from_rollout_payload(
    payload: &Value,
    fallback_id: String,
    item_type: &str,
) -> Value {
    let review_output = payload
        .get("review_output")
        .or_else(|| payload.get("reviewOutput"))
        .cloned()
        .unwrap_or(Value::Null);
    let review = if item_type == "enteredReviewMode" {
        payload
            .get("user_facing_hint")
            .or_else(|| payload.get("userFacingHint"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("Review requested.")
    } else {
        review_output
            .get("overall_explanation")
            .or_else(|| review_output.get("overallExplanation"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("Review mode completed.")
    };
    json!({
        "id": fallback_id,
        "type": item_type,
        "review": review,
        "target": payload.get("target").cloned().unwrap_or(Value::Null),
        "reviewOutput": review_output
    })
}

fn rollout_tail_records(
    path: &Path,
    target_turns: usize,
    max_bytes: u64,
) -> Result<RolloutRecordWindow, String> {
    rollout_tail_records_with_filter(
        path,
        target_turns,
        SESSION_DETAIL_ROLLOUT_TAIL_INITIAL_BYTES,
        max_bytes,
        None,
        rollout_tail_line_may_affect_detail,
        false,
    )
}

fn rollout_completion_tail_records(
    path: &Path,
    expected_turn_id: Option<&str>,
    max_bytes: u64,
) -> Result<RolloutRecordWindow, String> {
    rollout_tail_records_with_filter(
        path,
        1,
        SESSION_COMPLETION_ROLLOUT_TAIL_INITIAL_BYTES,
        max_bytes,
        expected_turn_id,
        rollout_tail_line_may_affect_completion,
        true,
    )
}

fn rollout_tail_records_with_filter(
    path: &Path,
    target_turns: usize,
    initial_bytes: u64,
    max_bytes: u64,
    expected_turn_id: Option<&str>,
    line_filter: fn(&str) -> bool,
    stop_at_target_turn_count: bool,
) -> Result<RolloutRecordWindow, String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("failed to stat rollout file: {error}"))?;
    let file_len = metadata.len();
    let modified_at_ms = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|duration| u64::try_from(duration.as_millis()).ok());
    let mut window = initial_bytes.min(file_len.max(1));
    let target_turns = target_turns.max(1);

    loop {
        let start = file_len.saturating_sub(window);
        let mut file = fs::File::open(path)
            .map_err(|error| format!("failed to open rollout file: {error}"))?;
        let starts_at_line_boundary = if start == 0 {
            true
        } else {
            std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(start.saturating_sub(1)))
                .map_err(|error| format!("failed to seek rollout file: {error}"))?;
            let mut previous = [0_u8; 1];
            std::io::Read::read_exact(&mut file, &mut previous)
                .map_err(|error| format!("failed to inspect rollout boundary: {error}"))?;
            previous[0] == b'\n'
        };
        std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(start))
            .map_err(|error| format!("failed to seek rollout file: {error}"))?;
        let mut buffer = Vec::with_capacity(window.min(usize::MAX as u64) as usize);
        std::io::Read::read_to_end(&mut file, &mut buffer)
            .map_err(|error| format!("failed to read rollout file: {error}"))?;
        let mut cursor = 0_usize;
        if !starts_at_line_boundary {
            cursor = buffer
                .iter()
                .position(|byte| *byte == b'\n')
                .map(|index| index.saturating_add(1))
                .unwrap_or(buffer.len());
        }
        let mut records = Vec::new();
        let mut corruption_detected = false;
        let mut trailing_incomplete = false;
        while cursor < buffer.len() {
            let line_start = cursor;
            let next_newline = buffer[cursor..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map(|relative| cursor.saturating_add(relative));
            let trailing_without_newline = next_newline.is_none();
            let line_end = next_newline.unwrap_or(buffer.len());
            cursor = if trailing_without_newline {
                buffer.len()
            } else {
                line_end.saturating_add(1)
            };
            let mut line = &buffer[line_start..line_end];
            if line.last() == Some(&b'\r') {
                line = &line[..line.len().saturating_sub(1)];
            }
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let Ok(text) = std::str::from_utf8(line) else {
                if trailing_without_newline {
                    trailing_incomplete = true;
                } else {
                    corruption_detected = true;
                }
                continue;
            };
            let trimmed = text.trim();
            // A filtered record can still be the line Codex is currently appending.
            // Validate it before deciding whether it belongs in the compact view.
            let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
                // Codex may currently be appending this final record. Ignore an
                // incomplete trailing line, but accept a complete JSON value even
                // when the process exited before writing its final newline.
                if trailing_without_newline {
                    trailing_incomplete = true;
                } else {
                    corruption_detected = true;
                }
                continue;
            };
            let line_is_relevant = line_filter(trimmed);
            if line_is_relevant {
                records.push(RolloutRecord {
                    value,
                    absolute_offset: start.saturating_add(line_start as u64),
                });
            }
        }
        let started_turn_count = records
            .iter()
            .filter(|record| {
                record.value.get("type").and_then(Value::as_str) == Some("event_msg")
                    && record
                        .value
                        .get("payload")
                        .and_then(|payload| payload.get("type"))
                        .and_then(Value::as_str)
                        == Some("task_started")
            })
            .count();
        let completed_turn_count = records
            .iter()
            .filter(|record| {
                record.value.get("type").and_then(Value::as_str) == Some("event_msg")
                    && record
                        .value
                        .get("payload")
                        .and_then(|payload| payload.get("type"))
                        .and_then(Value::as_str)
                        == Some("task_complete")
            })
            .count();
        let expected_turn_seen = expected_turn_id.is_some_and(|expected_turn_id| {
            records.iter().any(|record| {
                record
                    .value
                    .get("payload")
                    .and_then(|payload| payload.get("turn_id"))
                    .and_then(Value::as_str)
                    == Some(expected_turn_id)
            })
        });
        let enough_turn_boundaries = if stop_at_target_turn_count {
            started_turn_count >= target_turns || completed_turn_count >= target_turns
        } else {
            started_turn_count > target_turns
        };
        if start == 0
            || expected_turn_seen
            || (expected_turn_id.is_none() && enough_turn_boundaries)
            || window >= max_bytes.min(file_len)
        {
            let metadata_after = fs::metadata(path)
                .map_err(|error| format!("failed to restat rollout file: {error}"))?;
            let modified_at_ms_after = metadata_after
                .modified()
                .ok()
                .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|duration| u64::try_from(duration.as_millis()).ok());
            return Ok(RolloutRecordWindow {
                records,
                truncated: start > 0,
                corruption_detected,
                trailing_incomplete,
                changed_during_read: metadata_after.len() != file_len
                    || modified_at_ms_after != modified_at_ms,
                file_size: file_len,
                modified_at_ms,
            });
        }
        window = window.saturating_mul(2).min(max_bytes).min(file_len);
    }
}

fn rollout_tail_line_may_affect_detail(line: &str) -> bool {
    line.contains(r#""type":"event_msg""#)
        || line.contains(r#""type": "event_msg""#)
        || line.contains(r#""type":"function_call""#)
        || line.contains(r#""type": "function_call""#)
        || line.contains(r#""type":"custom_tool_call""#)
        || line.contains(r#""type": "custom_tool_call""#)
        || line.contains(r#""type":"function_call_output""#)
        || line.contains(r#""type": "function_call_output""#)
        || line.contains(r#""type":"custom_tool_call_output""#)
        || line.contains(r#""type": "custom_tool_call_output""#)
        || line.contains(r#""type":"mcp_tool_call_""#)
        || line.contains(r#""type": "mcp_tool_call_""#)
        || line.contains(r#""type":"reasoning""#)
        || line.contains(r#""type": "reasoning""#)
        || line.contains(r#""type":"context_compaction""#)
        || line.contains(r#""type": "context_compaction""#)
        || line.contains(r#""type":"contextCompaction""#)
        || line.contains(r#""type": "contextCompaction""#)
        || line.contains(r#""type":"web_search_"#)
        || line.contains(r#""type": "web_search_"#)
        || line.contains(r#""type":"image_generation_"#)
        || line.contains(r#""type": "image_generation_"#)
        || line.contains(r#""type":"view_image_tool_call""#)
        || line.contains(r#""type": "view_image_tool_call""#)
        || line.contains(r#""type":"entered_review_mode""#)
        || line.contains(r#""type": "entered_review_mode""#)
        || line.contains(r#""type":"exited_review_mode""#)
        || line.contains(r#""type": "exited_review_mode""#)
        || line.contains(r#""type":"thread_rolled_back""#)
        || line.contains(r#""type": "thread_rolled_back""#)
}

fn rollout_tail_line_may_affect_completion(line: &str) -> bool {
    (line.contains(r#""type":"event_msg""#) || line.contains(r#""type": "event_msg""#))
        && [
            "task_started",
            "user_message",
            "agent_message",
            "task_complete",
            r#""type":"error""#,
            r#""type": "error""#,
        ]
        .iter()
        .any(|marker| line.contains(marker))
}

pub(crate) fn build_turn_window_from_rollout_records(
    record_window: RolloutRecordWindow,
    limit: usize,
) -> LocalRolloutTurnWindow {
    let RolloutRecordWindow {
        records,
        truncated,
        corruption_detected: _,
        trailing_incomplete,
        changed_during_read,
        file_size,
        modified_at_ms,
    } = record_window;
    let mut turns = Vec::new();
    let mut current_index = None;
    let mut call_turn_indices: HashMap<String, usize> = HashMap::new();
    let mut token_usage = Value::Null;

    for record in &records {
        let record_identity = record.absolute_offset;
        let record = &record.value;
        let timestamp_ms = parse_rollout_timestamp_ms(record);
        let record_type = record
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let payload = record.get("payload").unwrap_or(&Value::Null);
        let payload_type = payload
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();

        match (record_type, payload_type) {
            ("event_msg", "task_started") => {
                let turn_id = payload.get("turn_id").and_then(Value::as_str);
                let index = ensure_rollout_turn_index(
                    &mut turns,
                    &mut current_index,
                    turn_id,
                    timestamp_ms,
                );
                if let Some(turn) = turns.get_mut(index).and_then(Value::as_object_mut) {
                    turn.insert("status".to_string(), json!("inProgress"));
                    turn.insert(
                        "startedAt".to_string(),
                        timestamp_ms.map(Value::from).unwrap_or(Value::Null),
                    );
                }
            }
            ("event_msg", "task_complete") => {
                let turn_id = payload.get("turn_id").and_then(Value::as_str);
                let previous_current_index = current_index;
                let index = ensure_rollout_turn_index(
                    &mut turns,
                    &mut current_index,
                    turn_id,
                    timestamp_ms,
                );
                // Completion for an older turn may arrive after a newer turn has
                // started. Updating that turn must not redirect following unscoped
                // records back to the completed turn.
                current_index = previous_current_index;
                if let Some(turn) = turns.get_mut(index).and_then(Value::as_object_mut) {
                    turn.insert(
                        "completionRecordOffset".to_string(),
                        Value::from(record_identity),
                    );
                    if turn.get("status").and_then(Value::as_str) != Some("failed") {
                        turn.insert("status".to_string(), json!("completed"));
                    }
                    turn.insert(
                        "completedAt".to_string(),
                        payload
                            .get("completed_at")
                            .and_then(Value::as_i64)
                            .or(timestamp_ms)
                            .map(Value::from)
                            .unwrap_or(Value::Null),
                    );
                    turn.insert(
                        "durationMs".to_string(),
                        payload
                            .get("duration_ms")
                            .cloned()
                            .unwrap_or_else(|| Value::Null),
                    );
                    if let Some(last_agent_message) = payload
                        .get("last_agent_message")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                    {
                        let has_same_agent_message = turn
                            .get("items")
                            .and_then(Value::as_array)
                            .is_some_and(|items| {
                                items.iter().any(|item| {
                                    item.get("type").and_then(Value::as_str) == Some("agentMessage")
                                        && item.get("text").and_then(Value::as_str)
                                            == Some(last_agent_message)
                                })
                            });
                        if !has_same_agent_message {
                            let turn_item_prefix = turn
                                .get("id")
                                .and_then(Value::as_str)
                                .unwrap_or("turn")
                                .to_string();
                            if let Some(items) = turn.get_mut("items").and_then(Value::as_array_mut)
                            {
                                items.push(json!({
                                    "id": format!("{turn_item_prefix}:agent-complete:{record_identity}"),
                                    "type": "agentMessage",
                                    "text": last_agent_message,
                                    "phase": "final_answer"
                                }));
                            }
                        }
                    }
                    mark_turn_without_agent_output_failed(
                        &mut turns[index],
                        record_identity.to_string(),
                    );
                }
            }
            ("event_msg", "error") => {
                let error_info = payload
                    .get("codex_error_info")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let affects_turn_status = !matches!(
                    error_info,
                    "thread_rollback_failed" | "active_turn_not_steerable"
                );
                if !affects_turn_status {
                    continue;
                }
                let Some(index) = current_index else {
                    continue;
                };
                if let Some(turn_object) = turns.get_mut(index).and_then(Value::as_object_mut) {
                    turn_object.insert("status".to_string(), json!("failed"));
                    turn_object.insert(
                        "completedAt".to_string(),
                        timestamp_ms.map(Value::from).unwrap_or(Value::Null),
                    );
                    turn_object.insert(
                        "error".to_string(),
                        json!({
                            "message": payload
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("Session failed"),
                            "codexErrorInfo": payload
                                .get("codex_error_info")
                                .cloned()
                                .unwrap_or(Value::Null)
                        }),
                    );
                    if let Some(items) = turn_object.get_mut("items").and_then(Value::as_array_mut)
                    {
                        for item in items {
                            let item_status = item.get("status").and_then(Value::as_str);
                            if matches!(item_status, Some("running" | "inProgress") | None) {
                                if let Some(item_object) = item.as_object_mut() {
                                    item_object.insert("status".to_string(), json!("failed"));
                                }
                            }
                        }
                    }
                }
            }
            ("event_msg", "user_message") => {
                let index =
                    ensure_rollout_turn_index(&mut turns, &mut current_index, None, timestamp_ms);
                if let Some(message) = payload
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    let turn_item_prefix = turns[index]
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("turn")
                        .to_string();
                    push_rollout_item(
                        &mut turns[index],
                        json!({
                            "id": format!("{turn_item_prefix}:user:{record_identity}"),
                            "type": "userMessage",
                            "text": message
                        }),
                    );
                }
            }
            ("event_msg", "item_started") => {
                let item = payload.get("item").unwrap_or(&Value::Null);
                let item_type = item.get("type").and_then(Value::as_str);
                if !matches!(item_type, Some("context_compaction" | "contextCompaction")) {
                    continue;
                }
                let turn_id = payload.get("turn_id").and_then(Value::as_str);
                let index = ensure_rollout_turn_index(
                    &mut turns,
                    &mut current_index,
                    turn_id,
                    timestamp_ms,
                );
                let turn_item_prefix = turns[index]
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("turn")
                    .to_string();
                push_rollout_item(
                    &mut turns[index],
                    context_compaction_item_from_rollout_payload(
                        item,
                        format!("{turn_item_prefix}:context-compaction:{record_identity}"),
                        "running",
                    ),
                );
            }
            ("event_msg", "item_completed") => {
                let item = payload.get("item").unwrap_or(&Value::Null);
                let item_type = item.get("type").and_then(Value::as_str);
                if !matches!(item_type, Some("context_compaction" | "contextCompaction")) {
                    continue;
                }
                let turn_id = payload.get("turn_id").and_then(Value::as_str);
                let index = ensure_rollout_turn_index(
                    &mut turns,
                    &mut current_index,
                    turn_id,
                    timestamp_ms,
                );
                let turn_item_prefix = turns[index]
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("turn")
                    .to_string();
                let compaction_id = item
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        format!("{turn_item_prefix}:context-compaction:{record_identity}")
                    });
                let updated = update_rollout_item(&mut turns[index], &compaction_id, |existing| {
                    if let Some(object) = existing.as_object_mut() {
                        object.insert("status".to_string(), json!("completed"));
                    }
                });
                if !updated {
                    push_rollout_item(
                        &mut turns[index],
                        context_compaction_item_from_rollout_payload(
                            item,
                            compaction_id,
                            "completed",
                        ),
                    );
                }
            }
            ("event_msg", "agent_message") => {
                let index =
                    ensure_rollout_turn_index(&mut turns, &mut current_index, None, timestamp_ms);
                if let Some(message) = payload
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    let turn_item_prefix = turns[index]
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("turn")
                        .to_string();
                    push_rollout_item(
                        &mut turns[index],
                        json!({
                            "id": format!("{turn_item_prefix}:agent:{record_identity}"),
                            "type": "agentMessage",
                            "text": message,
                            "phase": payload.get("phase").cloned().unwrap_or(Value::Null),
                            "memoryCitation": payload
                                .get("memory_citation")
                                .or_else(|| payload.get("memoryCitation"))
                                .cloned()
                                .unwrap_or(Value::Null)
                        }),
                    );
                }
            }
            ("event_msg", "token_count") => {
                token_usage = normalize_token_usage_payload(payload.get("info"));
            }
            ("event_msg", "thread_rolled_back") => {
                let rollback_turns = payload
                    .get("num_turns")
                    .and_then(Value::as_u64)
                    .and_then(|value| usize::try_from(value).ok())
                    .unwrap_or(0);
                if rollback_turns >= turns.len() {
                    turns.clear();
                    current_index = None;
                } else if rollback_turns > 0 {
                    let next_len = turns.len().saturating_sub(rollback_turns);
                    turns.truncate(next_len);
                    current_index = next_len.checked_sub(1);
                }
                call_turn_indices.retain(|_, index| *index < turns.len());
            }
            ("event_msg", "web_search_begin") => {
                let index =
                    ensure_rollout_turn_index(&mut turns, &mut current_index, None, timestamp_ms);
                let turn_item_prefix = turns[index]
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("turn")
                    .to_string();
                push_rollout_item(
                    &mut turns[index],
                    web_search_item_from_rollout_payload(
                        payload,
                        format!("{turn_item_prefix}:web-search:{record_identity}"),
                        "running",
                    ),
                );
            }
            ("event_msg", "web_search_end") => {
                let index =
                    ensure_rollout_turn_index(&mut turns, &mut current_index, None, timestamp_ms);
                let turn_item_prefix = turns[index]
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("turn")
                    .to_string();
                let item = web_search_item_from_rollout_payload(
                    payload,
                    format!("{turn_item_prefix}:web-search:{record_identity}"),
                    "completed",
                );
                let item_id = item
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if !item_id.is_empty() {
                    let updated = update_rollout_item(&mut turns[index], &item_id, |existing| {
                        *existing = item.clone();
                    });
                    if !updated {
                        push_rollout_item(&mut turns[index], item);
                    }
                }
            }
            ("event_msg", "image_generation_begin") => {
                let index =
                    ensure_rollout_turn_index(&mut turns, &mut current_index, None, timestamp_ms);
                let turn_item_prefix = turns[index]
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("turn")
                    .to_string();
                push_rollout_item(
                    &mut turns[index],
                    image_generation_item_from_rollout_payload(
                        payload,
                        format!("{turn_item_prefix}:image-generation:{record_identity}"),
                        "running",
                    ),
                );
            }
            ("event_msg", "image_generation_end") => {
                let index =
                    ensure_rollout_turn_index(&mut turns, &mut current_index, None, timestamp_ms);
                let turn_item_prefix = turns[index]
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("turn")
                    .to_string();
                let item = image_generation_item_from_rollout_payload(
                    payload,
                    format!("{turn_item_prefix}:image-generation:{record_identity}"),
                    "completed",
                );
                let item_id = item
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if !item_id.is_empty() {
                    let updated = update_rollout_item(&mut turns[index], &item_id, |existing| {
                        *existing = item.clone();
                    });
                    if !updated {
                        push_rollout_item(&mut turns[index], item);
                    }
                }
            }
            ("event_msg", "view_image_tool_call") => {
                let index =
                    ensure_rollout_turn_index(&mut turns, &mut current_index, None, timestamp_ms);
                let turn_item_prefix = turns[index]
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("turn")
                    .to_string();
                push_rollout_item(
                    &mut turns[index],
                    json!({
                        "id": payload
                            .get("call_id")
                            .or_else(|| payload.get("id"))
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .unwrap_or_else(|| format!("{turn_item_prefix}:image-view:{record_identity}")),
                        "type": "imageView",
                        "path": payload.get("path").cloned().unwrap_or(Value::Null),
                        "status": "completed"
                    }),
                );
            }
            ("event_msg", "entered_review_mode") | ("event_msg", "exited_review_mode") => {
                let index =
                    ensure_rollout_turn_index(&mut turns, &mut current_index, None, timestamp_ms);
                let turn_item_prefix = turns[index]
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("turn")
                    .to_string();
                let item_type = if payload_type == "entered_review_mode" {
                    "enteredReviewMode"
                } else {
                    "exitedReviewMode"
                };
                push_rollout_item(
                    &mut turns[index],
                    review_mode_item_from_rollout_payload(
                        payload,
                        format!("{turn_item_prefix}:{payload_type}:{record_identity}"),
                        item_type,
                    ),
                );
            }
            ("event_msg", "exec_command_end") => {
                let turn_id = payload.get("turn_id").and_then(Value::as_str);
                let index = ensure_rollout_turn_index(
                    &mut turns,
                    &mut current_index,
                    turn_id,
                    timestamp_ms,
                );
                let call_id = payload
                    .get("call_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if !call_id.is_empty() {
                    let updated = update_rollout_item(&mut turns[index], &call_id, |item| {
                        if let Some(object) = item.as_object_mut() {
                            object.insert(
                                "type".to_string(),
                                Value::String("commandExecution".to_string()),
                            );
                            object.insert(
                                "command".to_string(),
                                payload.get("command").cloned().unwrap_or(Value::Null),
                            );
                            object.insert(
                                "cwd".to_string(),
                                payload.get("cwd").cloned().unwrap_or(Value::Null),
                            );
                            object.insert(
                                "parsed_cmd".to_string(),
                                payload.get("parsed_cmd").cloned().unwrap_or(Value::Null),
                            );
                            object.insert(
                                "exitCode".to_string(),
                                payload.get("exit_code").cloned().unwrap_or(Value::Null),
                            );
                            object.insert(
                                "status".to_string(),
                                payload
                                    .get("status")
                                    .cloned()
                                    .unwrap_or_else(|| json!("completed")),
                            );
                        }
                    });
                    if !updated {
                        push_rollout_item(
                            &mut turns[index],
                            json!({
                                "id": call_id,
                                "type": "commandExecution",
                                "command": payload.get("command").cloned().unwrap_or(Value::Null),
                                "cwd": payload.get("cwd").cloned().unwrap_or(Value::Null),
                                "parsed_cmd": payload.get("parsed_cmd").cloned().unwrap_or(Value::Null),
                                "exitCode": payload.get("exit_code").cloned().unwrap_or(Value::Null),
                                "status": payload.get("status").cloned().unwrap_or_else(|| json!("completed"))
                            }),
                        );
                    }
                }
            }
            ("event_msg", "patch_apply_end") => {
                let turn_id = payload.get("turn_id").and_then(Value::as_str);
                let index = ensure_rollout_turn_index(
                    &mut turns,
                    &mut current_index,
                    turn_id,
                    timestamp_ms,
                );
                push_rollout_item(
                    &mut turns[index],
                    file_change_item_from_patch_apply_payload(payload),
                );
            }
            ("event_msg", "mcp_tool_call_begin") | ("event_msg", "mcp_tool_call_end") => {
                let turn_id = payload.get("turn_id").and_then(Value::as_str);
                let index = ensure_rollout_turn_index(
                    &mut turns,
                    &mut current_index,
                    turn_id,
                    timestamp_ms,
                );
                let status = if payload_type == "mcp_tool_call_begin" {
                    "inProgress"
                } else {
                    "completed"
                };
                let item = mcp_tool_call_item_from_rollout_payload(payload, status);
                let item_id = item
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if item_id.is_empty() {
                    continue;
                }
                let updated = update_rollout_item(&mut turns[index], &item_id, |existing| {
                    if let (Some(existing), Some(incoming)) =
                        (existing.as_object_mut(), item.as_object())
                    {
                        for (key, value) in incoming {
                            if !value.is_null() {
                                existing.insert(key.clone(), value.clone());
                            }
                        }
                    }
                });
                if !updated {
                    push_rollout_item(&mut turns[index], item);
                }
            }
            ("response_item", "function_call") | ("response_item", "custom_tool_call") => {
                let index =
                    ensure_rollout_turn_index(&mut turns, &mut current_index, None, timestamp_ms);
                let item = command_item_from_rollout_function_call(payload);
                if let Some(call_id) = item.get("id").and_then(Value::as_str) {
                    call_turn_indices.insert(call_id.to_string(), index);
                }
                push_rollout_item(&mut turns[index], item);
            }
            ("response_item", "function_call_output")
            | ("response_item", "custom_tool_call_output") => {
                let Some(call_id) = payload.get("call_id").and_then(Value::as_str) else {
                    continue;
                };
                if let Some(index) = call_turn_indices.get(call_id).copied() {
                    update_rollout_item(&mut turns[index], call_id, |item| {
                        if let Some(object) = item.as_object_mut() {
                            object.insert("status".to_string(), json!("completed"));
                            if let Some(result) = payload
                                .get("output")
                                .or_else(|| payload.get("result"))
                                .filter(|value| !value.is_null())
                            {
                                object.insert("result".to_string(), result.clone());
                            }
                            copy_rollout_item_metadata(payload, object);
                        }
                    });
                }
            }
            ("response_item", "reasoning") => {
                let summary = payload
                    .get("summary")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|entry| value_text(&entry))
                    .collect::<Vec<_>>();
                let text = payload
                    .get("content")
                    .and_then(value_text)
                    .unwrap_or_default();
                if summary.is_empty() && text.trim().is_empty() {
                    continue;
                }
                let index =
                    ensure_rollout_turn_index(&mut turns, &mut current_index, None, timestamp_ms);
                let turn_item_prefix = turns[index]
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("turn")
                    .to_string();
                push_rollout_item(
                    &mut turns[index],
                    json!({
                        "id": format!("{turn_item_prefix}:reasoning:{record_identity}"),
                        "type": "reasoning",
                        "text": text,
                        "summary": summary
                    }),
                );
            }
            ("response_item", "context_compaction") | ("response_item", "contextCompaction") => {
                let index =
                    ensure_rollout_turn_index(&mut turns, &mut current_index, None, timestamp_ms);
                let turn_item_prefix = turns[index]
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("turn")
                    .to_string();
                push_rollout_item(
                    &mut turns[index],
                    context_compaction_item_from_rollout_payload(
                        payload,
                        format!("{turn_item_prefix}:context-compaction:{record_identity}"),
                        if payload.get("encrypted_content").is_some() {
                            "completed"
                        } else {
                            "running"
                        },
                    ),
                );
            }
            ("response_item", "message") => {}
            _ => {}
        }
    }

    turns.retain(|turn| {
        turn.get("items")
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty())
    });
    if turns.len() > 1 {
        let last_index = turns.len().saturating_sub(1);
        for (index, turn) in turns.iter_mut().enumerate() {
            if index >= last_index {
                continue;
            }
            let is_in_progress = turn.get("status").and_then(Value::as_str) == Some("inProgress");
            if is_in_progress {
                if let Some(turn_object) = turn.as_object_mut() {
                    turn_object.insert("status".to_string(), json!("completed"));
                }
            }
        }
    }
    let known_total_turns = (!truncated).then_some(turns.len());
    let window_size = limit.max(1);
    let loaded_start = turns.len().saturating_sub(window_size);
    let turns = turns[loaded_start..].to_vec();
    LocalRolloutTurnWindow {
        turns,
        loaded_start,
        known_total_turns,
        truncated,
        trailing_incomplete,
        changed_during_read,
        file_size,
        token_usage,
        recovery: None,
        modified_at_ms,
    }
}

pub(crate) async fn read_local_rollout_turn_window(
    rollout_path: PathBuf,
    limit: usize,
) -> ApiResult<LocalRolloutTurnWindow> {
    read_local_rollout_turn_window_with_max(
        rollout_path,
        limit,
        SESSION_DETAIL_ROLLOUT_TAIL_MAX_BYTES,
    )
    .await
}

async fn read_local_rollout_completion_turn(
    rollout_path: PathBuf,
    expected_turn_id: Option<String>,
) -> ApiResult<LocalRolloutTurnWindow> {
    tokio::task::spawn_blocking(move || {
        let record_window = rollout_completion_tail_records(
            &rollout_path,
            expected_turn_id.as_deref(),
            SESSION_DETAIL_ROLLOUT_TAIL_MAX_BYTES,
        )?;
        let limit = if expected_turn_id.is_some() {
            usize::MAX
        } else {
            1
        };
        Ok::<_, String>(build_turn_window_from_rollout_records(record_window, limit))
    })
    .await
    .map_err(|error| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to load the local session completion tail: {error}"),
        )
    })?
    .map_err(|error| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to load the local session completion tail: {error}"),
        )
    })
}

pub(crate) async fn read_local_rollout_runtime_evidence(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    expected_turn_id: &str,
) -> ApiResult<Option<LocalRolloutRuntimeEvidence>> {
    let Some(rollout_path) = find_rollout_path_by_session_id(state, profile_id, session_id).await?
    else {
        return Ok(None);
    };
    let expected_turn_id = expected_turn_id.to_string();
    tokio::task::spawn_blocking(move || {
        let record_window = rollout_completion_tail_records(
            &rollout_path,
            Some(&expected_turn_id),
            SESSION_DETAIL_ROLLOUT_TAIL_MAX_BYTES,
        )?;
        if record_window.trailing_incomplete || record_window.changed_during_read {
            return Ok(None);
        }
        let mut status = None;
        for record in &record_window.records {
            let payload = record.value.get("payload").unwrap_or(&Value::Null);
            if payload.get("turn_id").and_then(Value::as_str) != Some(&expected_turn_id) {
                continue;
            }
            status = match payload.get("type").and_then(Value::as_str) {
                Some("task_started") => Some("running"),
                Some("task_complete") => Some("completed"),
                Some("error") => Some("failed"),
                _ => status,
            };
        }
        Ok::<_, String>(status.map(|status| LocalRolloutRuntimeEvidence {
            status: status.to_string(),
            modified_at_ms: record_window.modified_at_ms,
        }))
    })
    .await
    .map_err(|error| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to inspect the local runtime tail: {error}"),
        )
    })?
    .map_err(|error| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to inspect the local runtime tail: {error}"),
        )
    })
}

async fn read_local_rollout_turn_window_with_max(
    rollout_path: PathBuf,
    limit: usize,
    max_bytes: u64,
) -> ApiResult<LocalRolloutTurnWindow> {
    tokio::task::spawn_blocking(move || {
        let record_window = rollout_tail_records(&rollout_path, limit, max_bytes)?;
        let corruption_detected = record_window.corruption_detected;
        let mut turn_window = build_turn_window_from_rollout_records(record_window, limit);
        if corruption_detected {
            let recovery = inspect_rollout_recovery_file(&rollout_path)
                .map_err(|error| format!("failed to inspect corrupted rollout file: {error}"))?;
            if recovery.available {
                turn_window.recovery = Some(recovery);
            }
        }
        Ok::<_, String>(turn_window)
    })
    .await
    .map_err(|error| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to load the local session rollout: {error}"),
        )
    })?
    .map_err(|error| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to load the local session rollout: {error}"),
        )
    })
}

pub(crate) async fn read_local_rollout_full_window(
    rollout_path: PathBuf,
) -> ApiResult<LocalRolloutTurnWindow> {
    read_local_rollout_turn_window_with_max(rollout_path, usize::MAX, u64::MAX).await
}

async fn find_rollout_path_by_session_id(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<Option<PathBuf>> {
    if let Some(path) = find_rollout_path_by_session_id_direct(state, profile_id, session_id) {
        return Ok(Some(path));
    }

    for archived in [false, true] {
        let candidates = list_rollout_candidates_payload(state, profile_id, archived).await?;
        for candidate in candidates {
            if candidate.get("id").and_then(Value::as_str) != Some(session_id) {
                continue;
            }
            if let Some(path) = candidate
                .get("path")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                return Ok(Some(PathBuf::from(path)));
            }
        }
    }
    Ok(None)
}

fn find_rollout_path_by_session_id_direct(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> Option<PathBuf> {
    let profile = resolve_runtime_profile(&state.config, profile_id);
    for date in uuid_v7_session_date_candidates(session_id) {
        let day_directory = profile
            .codex_home
            .join("sessions")
            .join(date.year().to_string())
            .join(format!("{:02}", u8::from(date.month())))
            .join(format!("{:02}", date.day()));
        if let Some(path) = find_rollout_path_in_directory(&day_directory, session_id) {
            return Some(path);
        }
    }
    find_rollout_path_in_directory(&profile.codex_home.join("archived_sessions"), session_id)
}

fn uuid_v7_session_date_candidates(session_id: &str) -> Vec<time::Date> {
    let hex = session_id
        .chars()
        .filter(|character| *character != '-')
        .collect::<String>();
    let Ok(timestamp_ms) = u64::from_str_radix(hex.get(..12).unwrap_or_default(), 16) else {
        return Vec::new();
    };
    let Ok(timestamp_seconds) = i64::try_from(timestamp_ms / 1000) else {
        return Vec::new();
    };
    let Ok(base) = time::OffsetDateTime::from_unix_timestamp(timestamp_seconds) else {
        return Vec::new();
    };
    let mut dates = Vec::with_capacity(3);
    for offset in [0_i64, -1, 1] {
        let Some(candidate) = base.checked_add(time::Duration::days(offset)) else {
            continue;
        };
        let date = candidate.date();
        if !dates.contains(&date) {
            dates.push(date);
        }
    }
    dates
}

fn find_rollout_path_in_directory(directory: &Path, session_id: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(directory).ok()?;
    let suffix = format!("{session_id}.jsonl");
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.ends_with(&suffix))
        {
            return Some(entry.path());
        }
    }
    None
}

async fn read_local_session_detail_source(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    limit: u64,
) -> ApiResult<Option<(Value, LocalRolloutTurnWindow)>> {
    if let Some(rollout_path) =
        find_rollout_path_by_session_id(state, profile_id, session_id).await?
    {
        let thread = read_local_thread_metadata_payload(state, profile_id, session_id)
            .await?
            .unwrap_or_else(|| {
                json!({
                    "id": session_id,
                    "name": Value::Null,
                    "preview": "",
                    "cwd": Value::Null,
                    "status": "completed",
                    "createdAt": 0,
                    "updatedAt": 0,
                    "archived": false,
                    "turns": []
                })
            });
        let turns =
            read_local_rollout_turn_window(rollout_path, limit.clamp(1, 200) as usize).await?;
        return Ok(Some((thread, turns)));
    }

    let Some(thread) = read_local_thread_metadata_payload(state, profile_id, session_id).await?
    else {
        return Ok(None);
    };
    let Some(rollout_path) = resolve_rollout_path(state, profile_id, session_id, &thread) else {
        return Ok(None);
    };
    let turns = read_local_rollout_turn_window(rollout_path, limit.clamp(1, 200) as usize).await?;
    Ok(Some((thread, turns)))
}

async fn read_local_session_completion_source(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    expected_turn_id: Option<&str>,
) -> ApiResult<Option<(Value, LocalRolloutTurnWindow)>> {
    if let Some(rollout_path) =
        find_rollout_path_by_session_id(state, profile_id, session_id).await?
    {
        let thread = read_local_thread_metadata_payload(state, profile_id, session_id)
            .await?
            .unwrap_or_else(|| {
                json!({
                    "id": session_id,
                    "name": Value::Null,
                    "preview": "",
                    "cwd": Value::Null,
                    "status": "completed",
                    "createdAt": 0,
                    "updatedAt": 0,
                    "archived": false,
                    "turns": []
                })
            });
        let turn =
            read_local_rollout_completion_turn(rollout_path, expected_turn_id.map(str::to_string))
                .await?;
        return Ok(Some((thread, turn)));
    }

    let Some(thread) = read_local_thread_metadata_payload(state, profile_id, session_id).await?
    else {
        return Ok(None);
    };
    let Some(rollout_path) = resolve_rollout_path(state, profile_id, session_id, &thread) else {
        return Ok(None);
    };
    let turn =
        read_local_rollout_completion_turn(rollout_path, expected_turn_id.map(str::to_string))
            .await?;
    Ok(Some((thread, turn)))
}

pub(crate) async fn local_session_diagnostics_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    limit: u64,
) -> ApiResult<Option<Value>> {
    let Some((thread, turn_window)) =
        read_local_session_detail_source(state, profile_id, session_id, limit).await?
    else {
        return Ok(None);
    };
    let visible_turns = turn_window
        .turns
        .iter()
        .enumerate()
        .map(|(visible_index, turn)| {
            summarize_session_turn_for_detail_payload(
                turn,
                turn_window.loaded_start + visible_index,
                SessionTurnDetailMode::Collapsed,
            )
        })
        .collect::<Vec<_>>();
    let remaining_turns = turn_window
        .loaded_start
        .saturating_add(usize::from(turn_window.truncated));
    Ok(Some(json!({
        "thread": {
            "id": thread.get("id").cloned().unwrap_or_else(|| json!(session_id)),
            "preview": thread.get("preview").cloned().unwrap_or_else(|| json!("")),
            "name": thread.get("name").cloned().unwrap_or(Value::Null),
            "cwd": thread.get("cwd").cloned().unwrap_or(Value::Null),
            "status": thread.get("status").cloned().unwrap_or_else(|| json!("unknown")),
            "createdAt": thread.get("createdAt").cloned().unwrap_or_else(|| json!(0)),
            "updatedAt": thread.get("updatedAt").cloned().unwrap_or_else(|| json!(0)),
            "isSubagent": thread.get("isSubagent").cloned().unwrap_or_else(|| json!(false)),
            "agentNickname": thread.get("agentNickname").cloned().unwrap_or(Value::Null),
            "agentRole": thread.get("agentRole").cloned().unwrap_or(Value::Null),
            "turns": visible_turns
        },
        "hydration": {
            "state": if remaining_turns > 0 { "idle" } else { "complete" },
            "loadedTurns": turn_window.turns.len(),
            "totalTurns": turn_window
                .known_total_turns
                .unwrap_or_else(|| remaining_turns.saturating_add(turn_window.turns.len())),
            "remainingTurns": remaining_turns,
            "truncated": turn_window.truncated
        }
    })))
}

pub(crate) async fn local_session_has_active_turn_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<Option<bool>> {
    let Some((_, turn_window)) =
        read_local_session_detail_source(state, profile_id, session_id, 20).await?
    else {
        return Ok(None);
    };

    Ok(Some(
        active_turn_id_from_turns(&turn_window.turns).is_some(),
    ))
}

pub(crate) async fn session_detail_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    limit: u64,
) -> ApiResult<Value> {
    let (
        thread,
        mut visible_turns,
        total_turns,
        start,
        mut hydration_state,
        hydration_message,
        hydration_recovery,
    ) = match read_local_session_detail_source(state, profile_id, session_id, limit).await? {
        Some((mut thread, mut turn_window)) => {
            let metadata_status = normalized_thread_status(thread.get("status"));
            let metadata_updated_at = thread
                .get("updatedAt")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let terminal_metadata_is_newer_than_rollout = metadata_status
                .as_deref()
                .is_some_and(|status| !is_live_thread_status(status) && status != "notLoaded")
                && turn_window
                    .modified_at_ms
                    .is_some_and(|modified_at| metadata_updated_at > modified_at);
            if terminal_metadata_is_newer_than_rollout {
                if let Ok(Some(response)) = tokio::time::timeout(
                    Duration::from_millis(SESSION_DETAIL_ACTIVE_RECONCILE_TIMEOUT_MS),
                    async {
                        let Ok(client) =
                            app_server_client_for_session(state, profile_id, session_id).await
                        else {
                            return None;
                        };
                        client
                            .request_with_timeout(
                                "thread/read",
                                json!({
                                    "threadId": session_id,
                                    "includeTurns": true
                                }),
                                Duration::from_millis(SESSION_DETAIL_ACTIVE_RECONCILE_TIMEOUT_MS),
                                false,
                            )
                            .await
                            .ok()
                    },
                )
                .await
                {
                    if let Some(authoritative_thread) = response.get("thread").cloned() {
                        let authoritative_turns = authoritative_thread
                            .get("turns")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        if !authoritative_turns.is_empty() {
                            let window_size = limit.clamp(1, 200) as usize;
                            let loaded_start =
                                authoritative_turns.len().saturating_sub(window_size);
                            turn_window.turns = authoritative_turns[loaded_start..].to_vec();
                            turn_window.loaded_start = loaded_start;
                            turn_window.known_total_turns = Some(authoritative_turns.len());
                            turn_window.truncated = false;
                            if let Some(token_usage) =
                                authoritative_thread.get("tokenUsage").cloned()
                            {
                                turn_window.token_usage = token_usage;
                            }
                            thread = authoritative_thread;
                        }
                    }
                }
            }
            if !turn_window.token_usage.is_null() {
                if let Some(thread_object) = thread.as_object_mut() {
                    thread_object
                        .entry("tokenUsage".to_string())
                        .or_insert(turn_window.token_usage);
                }
            }
            let visible_turns = turn_window
                .turns
                .iter()
                .enumerate()
                .map(|(visible_index, turn)| {
                    summarize_session_turn_for_detail_payload(
                        turn,
                        turn_window.loaded_start + visible_index,
                        SessionTurnDetailMode::Collapsed,
                    )
                })
                .collect::<Vec<_>>();
            let start = turn_window
                .loaded_start
                .saturating_add(usize::from(turn_window.truncated));
            let total_turns = turn_window
                .known_total_turns
                .unwrap_or_else(|| start.saturating_add(visible_turns.len()));
            let recovery = turn_window
                .recovery
                .as_ref()
                .map(|recovery| json!(recovery))
                .unwrap_or_else(|| {
                    json!({
                        "available": false,
                        "issue": Value::Null,
                        "totalLines": Value::Null,
                        "recoverableLines": Value::Null,
                        "skippedLines": Value::Null
                    })
                });
            let has_recovery = turn_window.recovery.is_some();
            (
                thread,
                visible_turns,
                total_turns,
                start,
                if has_recovery {
                    "error"
                } else if start > 0 {
                    "idle"
                } else {
                    "complete"
                },
                has_recovery
                    .then(|| Value::String("The rollout contains recoverable corruption.".into()))
                    .unwrap_or(Value::Null),
                recovery,
            )
        }
        None => match read_thread_payload(state, profile_id, session_id, true).await {
            Ok(thread) => {
                let turns = thread
                    .get("turns")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let total_turns = turns.len();
                let window_size = limit.clamp(1, 200) as usize;
                let start = total_turns.saturating_sub(window_size);
                let visible_turns = turns[start..]
                    .iter()
                    .enumerate()
                    .map(|(visible_index, turn)| {
                        summarize_session_turn_for_detail_payload(
                            turn,
                            start + visible_index,
                            SessionTurnDetailMode::Collapsed,
                        )
                    })
                    .collect::<Vec<_>>();
                (
                    thread,
                    visible_turns,
                    total_turns,
                    start,
                    "complete",
                    Value::Null,
                    json!({
                        "available": false,
                        "issue": Value::Null,
                        "totalLines": Value::Null,
                        "recoverableLines": Value::Null,
                        "skippedLines": Value::Null
                    }),
                )
            }
            Err(error) => {
                if is_unmaterialized_thread_error_message(&error.message) {
                    let thread = read_thread_metadata_payload(state, profile_id, session_id)
                        .await
                        .unwrap_or_else(|_| {
                            json!({
                                "id": session_id,
                                "name": Value::Null,
                                "preview": "",
                                "cwd": Value::Null,
                                "status": "notLoaded",
                                "createdAt": 0,
                                "updatedAt": 0,
                                "archived": false,
                                "turns": []
                            })
                        });
                    (
                        thread,
                        Vec::new(),
                        0,
                        0,
                        "idle",
                        Value::Null,
                        json!({
                            "available": false,
                            "issue": "thread_not_loaded",
                            "totalLines": Value::Null,
                            "recoverableLines": Value::Null,
                            "skippedLines": Value::Null
                        }),
                    )
                } else {
                    let thread = read_thread_metadata_payload(state, profile_id, session_id)
                        .await
                        .map_err(|_| error.clone())?;
                    let rollout_path = resolve_rollout_path(state, profile_id, session_id, &thread)
                        .ok_or_else(|| error.clone())?;
                    let recovery_info = tokio::task::spawn_blocking(move || {
                        inspect_rollout_recovery_file(&rollout_path)
                    })
                    .await
                    .map_err(|_| error.clone())?
                    .map_err(|_| error.clone())?;
                    if !recovery_info.available || recovery_info.recoverable_lines == 0 {
                        return Err(error);
                    }

                    let turns = thread
                        .get("turns")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    let total_turns = turns.len();
                    let window_size = limit.clamp(1, 200) as usize;
                    let start = total_turns.saturating_sub(window_size);
                    let visible_turns = turns[start..]
                        .iter()
                        .enumerate()
                        .map(|(visible_index, turn)| {
                            summarize_session_turn_for_detail_payload(
                                turn,
                                start + visible_index,
                                SessionTurnDetailMode::Collapsed,
                            )
                        })
                        .collect::<Vec<_>>();
                    (
                        thread,
                        visible_turns,
                        total_turns,
                        start,
                        "error",
                        Value::String(error.message),
                        json!(recovery_info),
                    )
                }
            }
        },
    };
    let turns = visible_turns.clone();
    let runtime_key = runtime_session_key(
        &resolve_runtime_profile_entry(&state.config, profile_id).0,
        session_id,
    );
    let raw_active_turn_id_from_payload = active_turn_id_from_turns(&turns);
    let runtime_status_entry = with_ui_state_read(state, profile_id, |ui_state| {
        Ok(ui_state
            .get("runtimeStatusByThreadId")
            .and_then(Value::as_object)
            .and_then(|statuses| statuses.get(session_id))
            .cloned())
    })
    .await?;
    let runtime_status_text = runtime_status_entry
        .as_ref()
        .and_then(|status| normalized_thread_status(Some(status)));
    let runtime_status_updated_at = runtime_status_entry
        .as_ref()
        .and_then(|status| status.get("updatedAt"))
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let stale_starting_runtime_status = runtime_status_text.as_deref() == Some("starting")
        && now_unix_ms().saturating_sub(runtime_status_updated_at)
            >= SESSION_DETAIL_ACTIVE_RECONCILE_TIMEOUT_MS;
    let mut terminal_runtime_status = runtime_status_text
        .clone()
        .filter(|status| !is_live_thread_status(status) && status != "starting");

    if (raw_active_turn_id_from_payload.is_some() || stale_starting_runtime_status)
        && terminal_runtime_status.is_none()
    {
        let app_server_thread =
            match app_server_client_for_session(state, profile_id, session_id).await {
                Ok(client) => client
                    .request_with_timeout(
                        "thread/read",
                        json!({
                            "threadId": session_id,
                            "includeTurns": true
                        }),
                        Duration::from_millis(SESSION_DETAIL_ACTIVE_RECONCILE_TIMEOUT_MS),
                        false,
                    )
                    .await
                    .ok()
                    .and_then(|response| response.get("thread").cloned()),
                Err(_) => None,
            };
        let app_server_status = app_server_thread
            .as_ref()
            .and_then(|thread| normalized_thread_status(thread.get("status")));
        let app_server_active_turn_id = app_server_thread
            .as_ref()
            .and_then(|thread| thread.get("turns"))
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .and_then(active_turn_id_from_turns);
        if app_server_active_turn_id.is_none() && app_server_status.is_some() {
            let status = match app_server_status.as_deref() {
                Some("failed" | "error") => "failed",
                Some("cancelled" | "canceled" | "aborted") => "cancelled",
                _ => "completed",
            };
            let reason = if status == "completed" {
                "codex app-server did not report an active turn"
            } else if app_server_status
                .as_deref()
                .is_some_and(is_live_thread_status)
            {
                "codex app-server did not report an active turn"
            } else {
                "codex app-server no longer has an active turn"
            };
            state.active_turns.lock().await.remove(&runtime_key);
            state.pending_turn_starts.lock().await.remove(&runtime_key);
            with_ui_state_write(state, profile_id, |ui_state| {
                let Some(runtime_status_by_thread_id) = ui_state
                    .get_mut("runtimeStatusByThreadId")
                    .and_then(Value::as_object_mut)
                else {
                    return Err(api_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "runtime status state is missing",
                    ));
                };
                runtime_status_by_thread_id.insert(
                    session_id.to_string(),
                    json!({
                        "status": status,
                        "updatedAt": now_unix_ms(),
                        "reason": reason
                    }),
                );
                Ok(())
            })
            .await?;
            terminal_runtime_status = Some(status.to_string());
            emit_session_summary_updated(state, profile_id, session_id, None, Some(status)).await;
        }
    }

    let stale_active_turn_after_exit =
        raw_active_turn_id_from_payload.is_some() && terminal_runtime_status.is_some();
    if stale_active_turn_after_exit {
        let settled_turn_status = match terminal_runtime_status.as_deref() {
            Some("failed" | "error") => "failed",
            Some("cancelled" | "canceled" | "aborted") => "cancelled",
            _ => "completed",
        };
        for turn in &mut visible_turns {
            if turn.get("status").and_then(Value::as_str) == Some("inProgress") {
                if let Some(turn_object) = turn.as_object_mut() {
                    turn_object.insert(
                        "status".to_string(),
                        Value::String(settled_turn_status.to_string()),
                    );
                    turn_object
                        .entry("completedAt".to_string())
                        .or_insert_with(|| json!(now_unix_ms()));
                }
            }
        }
    }
    if let Err(error) = apply_language_bridge_translations_to_turns(
        state,
        profile_id,
        session_id,
        &mut visible_turns,
    )
    .await
    {
        tracing::warn!(
            profile_id,
            session_id,
            error = %error.message,
            "failed to apply language bridge translations to session detail"
        );
    }
    let active_turn_id_from_payload = if stale_active_turn_after_exit {
        None
    } else {
        raw_active_turn_id_from_payload
    };
    let cached_active_turn_id = if terminal_runtime_status.is_some() {
        state.active_turns.lock().await.remove(&runtime_key);
        state.pending_turn_starts.lock().await.remove(&runtime_key);
        None
    } else {
        state.active_turns.lock().await.get(&runtime_key).cloned()
    };
    let active_turn_id = active_turn_id_from_payload
        .clone()
        .or_else(|| cached_active_turn_id.clone());
    if let Some(turn_id) = active_turn_id_from_payload {
        state
            .active_turns
            .lock()
            .await
            .insert(runtime_key.clone(), turn_id);
    } else if active_turn_id.is_none() {
        state.active_turns.lock().await.remove(&runtime_key);
    }
    let detail_thread_status = if active_turn_id.is_some() {
        json!("running")
    } else if let Some(status) = terminal_runtime_status {
        json!(status)
    } else {
        thread
            .get("status")
            .cloned()
            .unwrap_or_else(|| json!("unknown"))
    };
    if active_turn_id.is_some() && hydration_state == "complete" {
        hydration_state = "idle";
    }
    let preferences = with_ui_state_read(state, profile_id, |ui_state| {
        Ok(ui_state
            .get("preferencesByThreadId")
            .and_then(Value::as_object)
            .and_then(|entries| entries.get(session_id))
            .cloned()
            .unwrap_or_else(|| {
                json!({
                    "cwd": thread.get("cwd").cloned().unwrap_or(Value::Null)
                })
            }))
    })
    .await
    .unwrap_or_else(|error| {
        tracing::warn!(
            profile_id,
            session_id,
            error = %error.message,
            "failed to read session preferences for detail"
        );
        json!({
            "cwd": thread.get("cwd").cloned().unwrap_or(Value::Null)
        })
    });
    let selected_skills = with_ui_state_read(state, profile_id, |ui_state| {
        Ok(Value::Array(session_selected_skills_from_ui_state(
            ui_state, session_id,
        )))
    })
    .await
    .unwrap_or_else(|error| {
        tracing::warn!(
            profile_id,
            session_id,
            error = %error.message,
            "failed to read selected skills for session detail"
        );
        json!([])
    });
    let cached_goal = cached_session_goal_or_null_payload(state, profile_id, session_id).await;
    let goal = if cached_goal.is_null() {
        tokio::time::timeout(
            Duration::from_millis(SESSION_DETAIL_GOAL_TIMEOUT_MS),
            fetch_session_goal_payload(state, profile_id, session_id),
        )
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or(Value::Null)
    } else {
        cached_goal
    };
    clear_completed_session_highlight_on_open(state, profile_id, session_id).await;
    let (detail_profile_id, detail_profile) =
        resolve_runtime_profile_entry(&state.config, profile_id);

    let attachments = list_session_attachments_payload(state, profile_id, session_id)
        .await
        .unwrap_or_else(|error| {
            tracing::warn!(
                profile_id,
                session_id,
                error = %error.message,
                "failed to read session attachments for detail"
            );
            Vec::new()
        });
    let queue = get_session_queue_payload(state, profile_id, session_id)
        .await
        .unwrap_or_else(|error| {
            tracing::warn!(
                profile_id,
                session_id,
                error = %error.message,
                "failed to read session queue for detail"
            );
            json!({
                "sessionId": session_id,
                "items": [],
                "resumeRequired": false,
                "updatedAt": Value::Null
            })
        });

    let payload = json!({
        "profileId": detail_profile_id,
        "profileLabel": detail_profile.label,
        "profileCodexHome": detail_profile.codex_home.display().to_string(),
        "accountEmail": Value::Null,
        "accountType": Value::Null,
        "thread": {
            "id": thread.get("id").cloned().unwrap_or_else(|| json!(session_id)),
            "preview": thread.get("preview").cloned().unwrap_or_else(|| json!("")),
            "name": thread.get("name").cloned().unwrap_or(Value::Null),
            "cwd": thread.get("cwd").cloned().unwrap_or(Value::Null),
            "status": detail_thread_status,
            "createdAt": thread.get("createdAt").cloned().unwrap_or_else(|| json!(0)),
            "updatedAt": thread.get("updatedAt").cloned().unwrap_or_else(|| json!(0)),
            "isSubagent": thread.get("isSubagent").cloned().unwrap_or_else(|| json!(false)),
            "agentNickname": thread.get("agentNickname").cloned().unwrap_or(Value::Null),
            "agentRole": thread.get("agentRole").cloned().unwrap_or(Value::Null),
            "turns": visible_turns
        },
        "preferences": preferences,
        "selectedSkills": selected_skills,
        "goal": goal,
        "attachments": attachments,
        "queue": queue,
        "pendingRequests": session_pending_requests_payload(state, profile_id, session_id).await,
        "activeTurnId": active_turn_id,
        "tokenUsage": thread.get("tokenUsage").cloned().unwrap_or(Value::Null),
        "hydration": {
            "state": hydration_state,
            "loadedTurns": total_turns.saturating_sub(start),
            "totalTurns": total_turns,
            "remainingTurns": start,
            "message": hydration_message,
            "recovery": hydration_recovery
        }
    });
    Ok(augment_session_detail_payload(payload))
}

fn augment_session_detail_payload(mut payload: Value) -> Value {
    let turns = payload
        .get("thread")
        .and_then(|thread| thread.get("turns"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let (turn_ids, turn_versions) = session_detail_turn_state(&turns);
    let metadata_version = payload_cache_version(&session_detail_metadata_payload(&payload));
    let state_hash = session_detail_state_hash(&metadata_version, &turn_ids, &turn_versions);
    let cache_version = session_detail_cache_version(&metadata_version, &turn_ids, &turn_versions);

    if let Some(payload_object) = payload.as_object_mut() {
        payload_object.insert("cacheVersion".to_string(), Value::String(cache_version));
        payload_object.insert("turnIds".to_string(), json!(turn_ids));
        payload_object.insert("turnVersions".to_string(), json!(turn_versions));
        payload_object.insert(
            "metadataVersion".to_string(),
            Value::String(metadata_version),
        );
        payload_object.insert("stateHash".to_string(), Value::String(state_hash));
    }
    payload
}

fn session_detail_turn_state(turns: &[Value]) -> (Vec<String>, HashMap<String, String>) {
    let mut turn_ids = Vec::new();
    let mut turn_versions = HashMap::new();

    for turn in turns {
        let Some(turn_id) = turn.get("id").and_then(Value::as_str) else {
            continue;
        };
        turn_ids.push(turn_id.to_string());
        turn_versions.insert(turn_id.to_string(), payload_cache_version(turn));
    }

    (turn_ids, turn_versions)
}

fn session_detail_metadata_payload(payload: &Value) -> Value {
    let Some(payload_object) = payload.as_object() else {
        return payload.clone();
    };
    let mut metadata_object = serde_json::Map::with_capacity(payload_object.len());
    for (key, value) in payload_object {
        if matches!(
            key.as_str(),
            "cacheVersion"
                | "notModified"
                | "turnIds"
                | "turnVersions"
                | "metadataVersion"
                | "stateHash"
        ) {
            continue;
        }
        if key == "thread" {
            let mut thread = value.as_object().cloned().unwrap_or_default();
            thread.insert("turns".to_string(), json!([]));
            metadata_object.insert(key.clone(), Value::Object(thread));
        } else {
            metadata_object.insert(key.clone(), value.clone());
        }
    }
    Value::Object(metadata_object)
}

pub(crate) fn session_detail_turn_versions_from_value(
    value: Option<&Value>,
) -> Option<HashMap<String, String>> {
    value.and_then(Value::as_object).map(|versions| {
        versions
            .iter()
            .filter_map(|(turn_id, version)| {
                version
                    .as_str()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|version| (turn_id.clone(), version.to_string()))
            })
            .collect::<HashMap<_, _>>()
    })
}

fn session_detail_state_source(
    metadata_version: &str,
    turn_ids: &[String],
    turn_versions: &HashMap<String, String>,
) -> String {
    let mut source = format!("metadata={metadata_version}\n");
    for turn_id in turn_ids {
        source.push_str(turn_id);
        source.push('\t');
        source.push_str(
            turn_versions
                .get(turn_id)
                .map(String::as_str)
                .unwrap_or_default(),
        );
        source.push('\n');
    }
    source
}

pub(crate) fn session_detail_state_hash(
    metadata_version: &str,
    turn_ids: &[String],
    turn_versions: &HashMap<String, String>,
) -> String {
    fnv1a32_hex(session_detail_state_source(metadata_version, turn_ids, turn_versions).as_bytes())
}

fn session_detail_cache_version(
    metadata_version: &str,
    turn_ids: &[String],
    turn_versions: &HashMap<String, String>,
) -> String {
    payload_cache_version(&Value::String(session_detail_state_source(
        metadata_version,
        turn_ids,
        turn_versions,
    )))
}

pub(crate) fn cacheable_session_detail_response(
    payload: Value,
    known_version: Option<&str>,
    known_turn_versions: Option<HashMap<String, String>>,
    known_state_hash: Option<&str>,
) -> Value {
    let version = payload
        .get("cacheVersion")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| payload_cache_version(&payload));
    if known_version
        .map(str::trim)
        .is_some_and(|known| known == version)
        && known_state_hash
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .zip(
                payload
                    .get("stateHash")
                    .and_then(Value::as_str)
                    .map(str::trim),
            )
            .is_some_and(|(known, current)| known == current)
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
        && known_turn_versions.is_some()
    {
        let mut patch_payload = payload;
        let known_versions = known_turn_versions.unwrap_or_default();
        let current_versions =
            session_detail_turn_versions_from_value(patch_payload.get("turnVersions"))
                .unwrap_or_default();
        let turns = patch_payload
            .get_mut("thread")
            .and_then(|thread| thread.get_mut("turns"))
            .and_then(Value::as_array_mut)
            .map(std::mem::take)
            .unwrap_or_default();
        let turn_ids = patch_payload
            .get("turnIds")
            .and_then(Value::as_array)
            .map(|ids| {
                ids.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let current_turn_ids = turn_ids.iter().cloned().collect::<HashSet<_>>();
        let turn_upserts = turns
            .into_iter()
            .filter(|turn| {
                let Some(turn_id) = turn.get("id").and_then(Value::as_str) else {
                    return false;
                };
                current_versions.get(turn_id) != known_versions.get(turn_id)
            })
            .collect::<Vec<_>>();
        let turn_removes = known_versions
            .keys()
            .filter(|turn_id| !current_turn_ids.contains(*turn_id))
            .cloned()
            .collect::<Vec<_>>();
        let thread = patch_payload
            .get("thread")
            .cloned()
            .unwrap_or_else(|| json!({}));

        return json!({
            "cacheVersion": version,
            "notModified": false,
            "patch": {
                "baseCacheVersion": known_version.unwrap_or_default(),
                "baseStateHash": known_state_hash.unwrap_or_default(),
                "finalCacheVersion": version,
                "finalStateHash": patch_payload.get("stateHash").cloned().unwrap_or(Value::Null),
                "metadataVersion": patch_payload.get("metadataVersion").cloned().unwrap_or(Value::Null),
                "turnIds": turn_ids,
                "turnVersions": current_versions,
                "turnUpserts": turn_upserts,
                "turnRemoves": turn_removes,
                "thread": thread,
                "preferences": patch_payload.get("preferences").cloned().unwrap_or(Value::Null),
                "selectedSkills": patch_payload.get("selectedSkills").cloned().unwrap_or_else(|| json!([])),
                "goal": patch_payload.get("goal").cloned().unwrap_or(Value::Null),
                "attachments": patch_payload.get("attachments").cloned().unwrap_or_else(|| json!([])),
                "queue": patch_payload.get("queue").cloned().unwrap_or(Value::Null),
                "pendingRequests": patch_payload.get("pendingRequests").cloned().unwrap_or_else(|| json!([])),
                "activeTurnId": patch_payload.get("activeTurnId").cloned().unwrap_or(Value::Null),
                "tokenUsage": patch_payload.get("tokenUsage").cloned().unwrap_or(Value::Null),
                "hydration": patch_payload.get("hydration").cloned().unwrap_or(Value::Null)
            }
        });
    }

    if known_version
        .map(str::trim)
        .is_some_and(|candidate| !candidate.is_empty() && candidate == version)
    {
        return json!({
            "cacheVersion": version,
            "notModified": true
        });
    }

    let mut next_payload = payload;
    if let Some(payload_object) = next_payload.as_object_mut() {
        payload_object.insert("cacheVersion".to_string(), Value::String(version));
        payload_object.insert("notModified".to_string(), Value::Bool(false));
    }
    next_payload
}

pub(crate) async fn session_older_turns_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    before_turn_id: &str,
    limit: u64,
) -> ApiResult<Value> {
    if let Some(rollout_path) =
        find_rollout_path_by_session_id(state, profile_id, session_id).await?
    {
        // Older-page requests are explicit and infrequent. Parse the complete
        // rollout here so pagination can cross both the 200-turn and tail-byte
        // boundaries without pretending that missing history does not exist.
        let turn_window = read_local_rollout_full_window(rollout_path).await?;
        let turns = turn_window.turns;
        let Some(before_index) = turns
            .iter()
            .position(|turn| turn.get("id").and_then(Value::as_str) == Some(before_turn_id))
        else {
            return Ok(json!({
                "turns": [],
                "loadedTurns": turn_window.loaded_start,
                "totalTurns": turn_window.loaded_start.saturating_add(turns.len()),
                "remainingTurns": usize::from(turn_window.truncated)
            }));
        };
        let window_size = limit.clamp(1, 200) as usize;
        let start = before_index.saturating_sub(window_size);
        let visible_turns = turns[start..before_index]
            .iter()
            .enumerate()
            .map(|(visible_index, turn)| {
                summarize_session_turn_for_detail_payload(
                    turn,
                    turn_window.loaded_start + start + visible_index,
                    SessionTurnDetailMode::Collapsed,
                )
            })
            .collect::<Vec<_>>();
        let remaining_turns = turn_window.loaded_start.saturating_add(start);
        return Ok(json!({
            "turns": visible_turns,
            "loadedTurns": turn_window.loaded_start.saturating_add(before_index),
            "totalTurns": turn_window.known_total_turns.unwrap_or_else(|| turn_window.loaded_start.saturating_add(turns.len())),
            "remainingTurns": remaining_turns
        }));
    }

    let thread = read_thread_payload(state, profile_id, session_id, true).await?;
    let turns = thread
        .get("turns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let Some(before_index) = turns
        .iter()
        .position(|turn| turn.get("id").and_then(Value::as_str) == Some(before_turn_id))
    else {
        return Ok(json!({
            "turns": [],
            "loadedTurns": turns.len(),
            "totalTurns": turns.len(),
            "remainingTurns": 0
        }));
    };
    let window_size = limit.clamp(1, 200) as usize;
    let start = before_index.saturating_sub(window_size);
    let visible_turns = turns[start..before_index]
        .iter()
        .enumerate()
        .map(|(visible_index, turn)| {
            summarize_session_turn_for_detail_payload(
                turn,
                start + visible_index,
                SessionTurnDetailMode::Collapsed,
            )
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "turns": visible_turns,
        "loadedTurns": before_index,
        "totalTurns": turns.len(),
        "remainingTurns": start
    }))
}

pub(crate) async fn session_rollback_targets_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<Value> {
    if let Some(rollout_path) =
        find_rollout_path_by_session_id(state, profile_id, session_id).await?
    {
        let turn_window = read_local_rollout_full_window(rollout_path).await?;
        return Ok(rollback_targets_from_turns(
            &turn_window.turns,
            turn_window.loaded_start,
            turn_window.known_total_turns,
            turn_window.truncated,
        ));
    }

    let thread = read_thread_payload(state, profile_id, session_id, true).await?;
    let turns = thread
        .get("turns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(rollback_targets_from_turns(
        &turns,
        0,
        Some(turns.len()),
        false,
    ))
}

async fn session_raw_turn_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    turn_id: &str,
) -> ApiResult<Value> {
    if let Some(rollout_path) =
        find_rollout_path_by_session_id(state, profile_id, session_id).await?
    {
        let turn_window = read_local_rollout_full_window(rollout_path).await?;
        if let Some(turn) = turn_window
            .turns
            .iter()
            .find(|turn| turn.get("id").and_then(Value::as_str) == Some(turn_id))
            .cloned()
        {
            return Ok(turn);
        }
    }

    let thread = read_thread_payload(state, profile_id, session_id, true).await?;
    thread
        .get("turns")
        .and_then(Value::as_array)
        .and_then(|turns| {
            turns
                .iter()
                .find(|turn| turn.get("id").and_then(Value::as_str) == Some(turn_id))
        })
        .cloned()
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Turn not found."))
}

pub(crate) async fn session_turn_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    turn_id: &str,
) -> ApiResult<Value> {
    let turn = session_raw_turn_payload(state, profile_id, session_id, turn_id).await?;
    Ok(json!({
        "turn": summarize_session_turn_for_detail_payload(
            &turn,
            0,
            SessionTurnDetailMode::Expanded,
        )
    }))
}

pub(crate) async fn session_latest_completed_turn_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    expected_turn_id: Option<&str>,
    known_completion_version: Option<&str>,
) -> ApiResult<Value> {
    let expected_turn_id = expected_turn_id
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let Some((thread, turn_window)) =
        read_local_session_completion_source(state, profile_id, session_id, expected_turn_id)
            .await?
    else {
        return Ok(json!({
            "sessionId": session_id,
            "profileId": profile_id,
            "targetTurnId": expected_turn_id,
            "threadStatus": "unknown",
            "threadUpdatedAt": Value::Null,
            "turn": Value::Null,
            "turnId": Value::Null,
            "turnPosition": Value::Null,
            "completionVersion": Value::Null,
            "settled": false,
            "expectedTurnReady": false,
            "sourceStable": false,
            "notModified": false,
            "retryAfterMs": 750,
            "rolloutRevision": Value::Null
        }));
    };

    let raw_turn_index = expected_turn_id
        .and_then(|expected_turn_id| {
            turn_window
                .turns
                .iter()
                .position(|turn| turn.get("id").and_then(Value::as_str) == Some(expected_turn_id))
        })
        .or_else(|| {
            expected_turn_id
                .is_none()
                .then(|| turn_window.turns.len().checked_sub(1))
                .flatten()
        });
    let raw_turn = raw_turn_index
        .and_then(|index| turn_window.turns.get(index))
        .cloned();
    let turn_id = raw_turn
        .as_ref()
        .and_then(|turn| turn.get("id"))
        .and_then(Value::as_str);
    let turn_status = raw_turn
        .as_ref()
        .and_then(|turn| turn.get("status"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let turn_is_terminal = !matches!(
        turn_status,
        "" | "inProgress" | "running" | "active" | "pending" | "starting"
    );
    let turn_has_terminal_content = raw_turn.as_ref().is_some_and(|turn| {
        session_turn_has_visible_agent_output(turn)
            || turn.get("error").is_some_and(|error| !error.is_null())
    });
    let completion_record_observed = raw_turn
        .as_ref()
        .and_then(|turn| turn.get("completionRecordOffset"))
        .and_then(Value::as_u64)
        .is_some();
    let expected_turn_matches = expected_turn_id.is_none_or(|expected| turn_id == Some(expected));
    let source_stable = !turn_window.trailing_incomplete && !turn_window.changed_during_read;
    let thread_updated_at = thread.get("updatedAt").and_then(Value::as_u64);
    let turn_completed_at = raw_turn
        .as_ref()
        .and_then(|turn| turn.get("completedAt"))
        .and_then(Value::as_u64);
    let settled = expected_turn_matches
        && completion_record_observed
        && turn_is_terminal
        && turn_has_terminal_content
        && source_stable;
    let summarized_turn = settled.then(|| {
        summarize_completed_turn_tail_payload(
            raw_turn
                .as_ref()
                .expect("a settled completion must have a turn"),
            turn_window
                .loaded_start
                .saturating_add(raw_turn_index.unwrap_or_default()),
        )
    });
    let completion_version = summarized_turn.as_ref().map(payload_cache_version);
    let not_modified = settled
        && completion_version.as_deref().is_some_and(|version| {
            known_completion_version
                .map(str::trim)
                .is_some_and(|known| !known.is_empty() && known == version)
        });
    let metadata_status = normalized_thread_status(thread.get("status"));
    let completion_targets_current_tail =
        raw_turn_index.is_some_and(|index| index.saturating_add(1) == turn_window.turns.len());
    let resolved_thread_status = if settled
        && completion_targets_current_tail
        && metadata_status.as_deref().is_none_or(|status| {
            is_live_thread_status(status) || matches!(status, "notLoaded" | "unknown")
        }) {
        turn_status
    } else {
        metadata_status.as_deref().unwrap_or(if turn_is_terminal {
            "completed"
        } else {
            "running"
        })
    };
    let resolved_updated_at = thread_updated_at.into_iter().chain(turn_completed_at).max();

    Ok(json!({
        "sessionId": session_id,
        "profileId": profile_id,
        "targetTurnId": expected_turn_id,
        "threadStatus": resolved_thread_status,
        "threadUpdatedAt": resolved_updated_at,
        "turn": if not_modified { Value::Null } else { summarized_turn.unwrap_or(Value::Null) },
        "turnId": turn_id,
        "turnPosition": (!turn_window.truncated)
            .then(|| raw_turn_index.map(|index| turn_window.loaded_start.saturating_add(index)))
            .flatten(),
        "completionVersion": completion_version,
        "settled": settled,
        "expectedTurnReady": settled && expected_turn_matches,
        "sourceStable": source_stable,
        "notModified": not_modified,
        "retryAfterMs": if turn_window.changed_during_read || turn_window.trailing_incomplete {
            300
        } else {
            750
        },
        "rolloutRevision": {
            "size": turn_window.file_size,
            "modifiedAt": turn_window.modified_at_ms
        }
    }))
}

pub(crate) async fn session_item_detail_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    turn_id: &str,
    item_id: &str,
) -> ApiResult<Value> {
    let turn = session_raw_turn_payload(state, profile_id, session_id, turn_id).await?;
    let mut item = turn
        .get("items")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .enumerate()
                .map(|(item_index, item)| normalize_session_item_payload(item, turn_id, item_index))
                .find(|item| item.get("id").and_then(Value::as_str) == Some(item_id))
        })
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Transcript item detail not found."))?;
    if let Some(item_object) = item.as_object_mut() {
        item_object.insert(
            "detailState".to_string(),
            Value::String("loaded".to_string()),
        );
    }
    Ok(json!({ "item": item }))
}

pub(crate) async fn search_session_turns_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    query: &str,
    cursor: Option<&str>,
    limit: u64,
) -> ApiResult<Value> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Ok(json!({
            "matches": [],
            "nextCursor": Value::Null,
            "totalMatches": 0
        }));
    }

    let thread = read_thread_payload(state, profile_id, session_id, true).await?;
    let turns = thread
        .get("turns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut matches = Vec::new();

    for (turn_index, turn) in turns.iter().enumerate() {
        let started_at = turn.get("startedAt").and_then(Value::as_i64);
        for item in turn
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let serialized = serde_json::to_string(&item).unwrap_or_default();
            let normalized = serialized.replace("\\n", " ").replace('\n', " ");
            let lowered = normalized.to_lowercase();
            let Some(match_index) = lowered.find(&needle) else {
                continue;
            };
            let normalized_chars = normalized.chars().collect::<Vec<_>>();
            let match_char_index = lowered[..match_index].chars().count();
            let snippet_start = match_char_index.saturating_sub(54);
            let snippet_end =
                (match_char_index + needle.chars().count() + 54).min(normalized_chars.len());
            matches.push(json!({
                "turnId": turn.get("id").cloned().unwrap_or(Value::Null),
                "turnIndex": turn_index,
                "itemId": item.get("id").cloned().unwrap_or(Value::Null),
                "itemType": item.get("type").cloned().unwrap_or(Value::Null),
                "preview": format!(
                    "{}{}{}",
                    if snippet_start > 0 { "..." } else { "" },
                    normalized_chars[snippet_start..snippet_end]
                        .iter()
                        .collect::<String>()
                        .trim(),
                    if snippet_end < normalized_chars.len() { "..." } else { "" }
                ),
                "startedAt": started_at,
                "requiresFullTurn": false,
                "requiresItemDetail": false
            }));
        }
    }

    let window_size = limit.clamp(1, 200) as usize;
    let start = cursor
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let end = start.saturating_add(window_size).min(matches.len());
    Ok(json!({
        "matches": if start < matches.len() { matches[start..end].to_vec() } else { Vec::<Value>::new() },
        "nextCursor": (end < matches.len()).then(|| end.to_string()),
        "totalMatches": matches.len()
    }))
}
