use super::*;

fn summarize_command_payload(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::Array(entries)) => {
            let summary = entries
                .iter()
                .filter_map(value_text)
                .collect::<Vec<_>>()
                .join(" ");
            let trimmed = summary.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        Some(Value::String(command)) => {
            let trimmed = command.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        Some(other) => value_text(other),
        None => None,
    }
}

fn summarize_tool_invocation_payload(value: Option<&Value>) -> Option<String> {
    let record = value.and_then(Value::as_object)?;
    let tool_name = ["toolName", "name", "tool", "method", "displayName"]
        .iter()
        .find_map(|key| record.get(*key).and_then(value_text));
    let server_name = ["serverName", "server"]
        .iter()
        .find_map(|key| record.get(*key).and_then(value_text));

    match (server_name, tool_name) {
        (Some(server_name), Some(tool_name)) => Some(format!("{server_name} · {tool_name}")),
        (Some(server_name), None) => Some(server_name),
        (None, Some(tool_name)) => Some(tool_name),
        (None, None) => None,
    }
}

fn remove_deferred_item_detail_fields(normalized: &mut serde_json::Map<String, Value>) {
    for key in [
        "aggregatedOutput",
        "output",
        "stdout",
        "stderr",
        "logs",
        "result",
        "response",
        "raw",
        "diff",
        "original",
        "modified",
        "content",
        "patch",
    ] {
        normalized.remove(key);
    }
}

pub(crate) fn prepare_session_deferred_item_payload(
    item: &Value,
    turn_id: &str,
    item_index: usize,
) -> Value {
    let mut normalized = normalize_session_item_payload(item, turn_id, item_index)
        .as_object()
        .cloned()
        .unwrap_or_default();
    let item_type = normalized
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();

    match item_type.as_str() {
        "contextCompaction" => {
            normalized.insert("title".to_string(), json!("Context compression"));
            normalized.insert("detailState".to_string(), json!("inline"));
            normalized.insert(
                "detailPreview".to_string(),
                json!("Compressing conversation context"),
            );
        }
        "commandExecution" => {
            remove_deferred_item_detail_fields(&mut normalized);
            normalized.insert("title".to_string(), json!("Command"));
            normalized.insert("detailState".to_string(), json!("deferred"));
            normalized.insert(
                "detailPreview".to_string(),
                summarize_command_payload(normalized.get("command"))
                    .map(Value::String)
                    .unwrap_or(Value::Null),
            );
            if !normalized.contains_key("parsed_cmd") && normalized.contains_key("parsedCmd") {
                if let Some(parsed_cmd) = normalized.get("parsedCmd").cloned() {
                    normalized.insert("parsed_cmd".to_string(), parsed_cmd);
                }
            }
        }
        "fileChange" => {
            remove_deferred_item_detail_fields(&mut normalized);
            let changes = normalized
                .get("changes")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let first_path = changes
                .iter()
                .find_map(|entry: &Value| entry.get("path").and_then(value_text));
            normalized.insert("title".to_string(), json!("Files changed"));
            normalized.insert("detailState".to_string(), json!("deferred"));
            normalized.insert("changeCount".to_string(), json!(changes.len()));
            normalized.insert(
                "firstChangePath".to_string(),
                first_path.clone().map(Value::String).unwrap_or(Value::Null),
            );
            normalized.insert(
                "detailPreview".to_string(),
                first_path.map(Value::String).unwrap_or_else(|| {
                    if changes.is_empty() {
                        Value::Null
                    } else {
                        Value::String(format!("{} files", changes.len()))
                    }
                }),
            );
            normalized.insert(
                "changes".to_string(),
                Value::Array(
                    changes
                        .into_iter()
                        .map(|entry: Value| {
                            json!({
                                "path": entry.get("path").and_then(value_text).unwrap_or_else(|| "Code edit".to_string()),
                                "kind": entry.get("kind").and_then(value_text).unwrap_or_else(|| "update".to_string())
                            })
                        })
                        .collect(),
                ),
            );
        }
        "imageGeneration" => {
            normalized.insert("title".to_string(), json!("Generated image"));
            normalized.insert("detailState".to_string(), json!("inline"));
            normalized.insert(
                "detailPreview".to_string(),
                value_text(normalized.get("revised_prompt").unwrap_or(&Value::Null))
                    .or_else(|| value_text(normalized.get("status").unwrap_or(&Value::Null)))
                    .map(Value::String)
                    .unwrap_or(Value::Null),
            );
        }
        "webSearch" => {
            let detail_preview = value_text(normalized.get("query").unwrap_or(&Value::Null))
                .or_else(|| summarize_tool_invocation_payload(normalized.get("action")));
            remove_deferred_item_detail_fields(&mut normalized);
            normalized.remove("action");
            normalized.insert("title".to_string(), json!("Web search"));
            normalized.insert("detailState".to_string(), json!("deferred"));
            normalized.insert(
                "detailPreview".to_string(),
                detail_preview.map(Value::String).unwrap_or(Value::Null),
            );
        }
        "mcpToolCall" | "dynamicToolCall" => {
            let detail_preview = summarize_tool_invocation_payload(normalized.get("invocation"))
                .or_else(|| value_text(normalized.get("tool").unwrap_or(&Value::Null)));
            remove_deferred_item_detail_fields(&mut normalized);
            normalized.remove("action");
            normalized.remove("invocation");
            normalized.insert(
                "title".to_string(),
                Value::String(if item_type == "mcpToolCall" {
                    "MCP call".to_string()
                } else {
                    "Tool call".to_string()
                }),
            );
            normalized.insert("detailState".to_string(), json!("deferred"));
            normalized.insert(
                "detailPreview".to_string(),
                detail_preview.map(Value::String).unwrap_or(Value::Null),
            );
        }
        _ => {
            normalized
                .entry("detailState".to_string())
                .or_insert_with(|| json!("inline"));
        }
    }

    Value::Object(normalized)
}

fn prepare_session_stream_item_payload(item: &Value, turn_id: &str) -> Value {
    prepare_session_deferred_item_payload(item, turn_id, 0)
}

pub(crate) fn map_app_server_session_notification(
    notification: &AppServerNotification,
) -> Option<Value> {
    let params = notification.params.as_object().cloned().unwrap_or_default();
    let mut mapped = params.clone();

    match notification.method.as_str() {
        "turn/started" | "turn/completed" => {
            let fallback_turn_id = mapped
                .get("turnId")
                .and_then(Value::as_str)
                .unwrap_or("turn-0")
                .to_string();
            let fallback_status = if notification.method == "turn/started" {
                "inProgress"
            } else {
                "completed"
            };
            let turn = mapped.get("turn").cloned().unwrap_or_else(|| {
                json!({
                    "id": fallback_turn_id,
                    "status": fallback_status,
                    "items": []
                })
            });
            mapped.insert("turn".to_string(), normalize_session_turn_payload(&turn, 0));
        }
        "item/started" | "item/completed" => {
            let turn_id = mapped
                .get("turnId")
                .and_then(Value::as_str)
                .unwrap_or("turn-0");
            let mut item = mapped.get("item").cloned().unwrap_or_else(
                || json!({ "id": mapped.get("itemId").cloned().unwrap_or(Value::Null) }),
            );
            if let Some(item_object) = item.as_object_mut() {
                let needs_id = item_object
                    .get("id")
                    .and_then(Value::as_str)
                    .is_none_or(|value| value.trim().is_empty());
                if needs_id {
                    if let Some(item_id) = mapped.get("itemId").cloned() {
                        item_object.insert("id".to_string(), item_id);
                    }
                }
            }
            mapped.insert(
                "item".to_string(),
                prepare_session_stream_item_payload(&item, turn_id),
            );
        }
        "thread/name/updated" => {
            let thread_id = mapped
                .get("threadId")
                .or_else(|| mapped.get("thread_id"))
                .and_then(value_text)
                .or_else(|| {
                    mapped
                        .get("thread")
                        .and_then(|thread| thread.get("id"))
                        .and_then(value_text)
                });
            let thread_name = ["threadName", "thread_name", "name", "title"]
                .iter()
                .find_map(|key| mapped.get(*key).and_then(value_text))
                .or_else(|| {
                    mapped.get("thread").and_then(|thread| {
                        ["threadName", "thread_name", "name", "title"]
                            .iter()
                            .find_map(|key| thread.get(*key).and_then(value_text))
                    })
                });
            if let Some(thread_id) = thread_id {
                mapped.insert("threadId".to_string(), Value::String(thread_id));
            }
            mapped.insert(
                "threadName".to_string(),
                thread_name.map(Value::String).unwrap_or(Value::Null),
            );
        }
        "thread/status/changed" => {
            mapped.insert(
                "status".to_string(),
                Value::String(
                    normalized_thread_status(mapped.get("status"))
                        .unwrap_or_else(|| "unknown".to_string()),
                ),
            );
        }
        "thread/tokenUsage/updated" => {
            mapped.insert(
                "tokenUsage".to_string(),
                normalize_token_usage_payload(mapped.get("tokenUsage")),
            );
        }
        "thread/goal/updated" => {
            let thread_id = mapped
                .get("threadId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            mapped.insert(
                "goal".to_string(),
                mapped
                    .get("goal")
                    .map(|goal| normalize_thread_goal_payload(goal, &thread_id))
                    .unwrap_or(Value::Null),
            );
            mapped
                .entry("turnId".to_string())
                .or_insert_with(|| Value::Null);
        }
        "thread/goal/cleared" => {
            mapped.insert("goal".to_string(), Value::Null);
        }
        "item/commandExecution/outputDelta" => {
            let delta = mapped
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            mapped.insert("delta".to_string(), Value::String(delta.clone()));
            mapped.insert("deltaLength".to_string(), json!(delta.chars().count()));
        }
        _ => {}
    }

    Some(json!({
        "kind": "notification",
        "method": notification.method,
        "params": Value::Object(mapped)
    }))
}

pub(crate) fn map_app_server_global_notification(
    notification: &AppServerNotification,
) -> Option<Value> {
    match notification.method.as_str() {
        "account/updated" => Some(json!({
            "kind": "notification",
            "method": "codex-webui/accountUpdated",
            "params": notification.params
        })),
        "account/login/completed" => Some(json!({
            "kind": "notification",
            "method": "codex-webui/accountLoginCompleted",
            "params": {
                "loginId": notification
                    .params
                    .get("loginId")
                    .cloned()
                    .unwrap_or(Value::Null),
                "success": notification
                    .params
                    .get("success")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                "error": notification
                    .params
                    .get("error")
                    .cloned()
                    .unwrap_or(Value::Null)
            }
        })),
        "account/rateLimits/updated" => Some(json!({
            "kind": "notification",
            "method": "codex-webui/accountRateLimitsUpdated",
            "params": notification.params
        })),
        _ => None,
    }
}
