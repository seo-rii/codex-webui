use super::*;

const INLINE_IMAGE_GENERATION_RESULT_MAX_CHARS: usize = 256 * 1024;

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
        "results",
        "sources",
        "citations",
        "searchResults",
        "sourceResults",
        "citationResults",
        "arguments",
        "input",
        "params",
        "request",
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
            normalized.remove("action");
            normalized.remove("invocation");
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
            normalized.insert(
                "detailPreview".to_string(),
                value_text(normalized.get("revised_prompt").unwrap_or(&Value::Null))
                    .or_else(|| value_text(normalized.get("status").unwrap_or(&Value::Null)))
                    .map(Value::String)
                    .unwrap_or(Value::Null),
            );
            let large_result = normalized
                .get("result")
                .and_then(Value::as_str)
                .is_some_and(|result| result.len() > INLINE_IMAGE_GENERATION_RESULT_MAX_CHARS);
            if large_result {
                normalized.insert("result".to_string(), Value::Null);
                normalized.insert("resultOmitted".to_string(), Value::Bool(true));
                normalized.insert("detailState".to_string(), json!("deferred"));
            } else {
                normalized.insert("detailState".to_string(), json!("inline"));
            }
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

const COMPUTER_FRAME_IMAGE_URL_MAX_BYTES: usize = 4 * 1024 * 1024;

fn value_string_from_keys(
    record: &serde_json::Map<String, Value>,
    keys: &[&str],
) -> Option<String> {
    keys.iter()
        .find_map(|key| record.get(*key).and_then(value_text))
}

fn infer_computer_frame_mime_type(image_url: &str) -> Option<String> {
    let trimmed = image_url.trim();
    if let Some(rest) = trimmed.strip_prefix("data:") {
        return rest
            .split_once(';')
            .map(|(mime_type, _)| mime_type.trim().to_string())
            .filter(|mime_type| mime_type.starts_with("image/"));
    }
    let without_query = trimmed.split(['?', '#']).next().unwrap_or(trimmed);
    match without_query
        .rsplit('.')
        .next()
        .map(str::to_ascii_lowercase)
    {
        Some(ext) if ext == "avif" => Some("image/avif".to_string()),
        Some(ext) if ext == "webp" => Some("image/webp".to_string()),
        Some(ext) if ext == "jpg" || ext == "jpeg" => Some("image/jpeg".to_string()),
        Some(ext) if ext == "png" => Some("image/png".to_string()),
        Some(ext) if ext == "gif" => Some("image/gif".to_string()),
        _ => None,
    }
}

fn extract_computer_frame_image_url(value: &Value, depth: usize) -> Option<String> {
    if depth > 8 {
        return None;
    }
    match value {
        Value::Array(entries) => entries
            .iter()
            .find_map(|entry| extract_computer_frame_image_url(entry, depth + 1)),
        Value::Object(record) => {
            let item_type = value_string_from_keys(record, &["type", "kind"])
                .unwrap_or_default()
                .to_ascii_lowercase();
            let image_url = value_string_from_keys(
                record,
                &["imageUrl", "image_url", "dataUrl", "data_url", "url"],
            );
            if item_type.contains("image") {
                if let Some(image_url) = image_url {
                    let trimmed = image_url.trim();
                    if !trimmed.is_empty() && trimmed.len() <= COMPUTER_FRAME_IMAGE_URL_MAX_BYTES {
                        return Some(trimmed.to_string());
                    }
                }
            }
            for key in [
                "contentItems",
                "content_items",
                "content",
                "result",
                "output",
                "response",
                "items",
            ] {
                if let Some(found) = record
                    .get(key)
                    .and_then(|entry| extract_computer_frame_image_url(entry, depth + 1))
                {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

fn computer_frame_tool_label(item: &Value) -> Option<String> {
    let record = item.as_object()?;
    let invocation = record.get("invocation").and_then(Value::as_object);
    let namespace =
        value_string_from_keys(record, &["namespace", "serverName", "server"]).or_else(|| {
            invocation.and_then(|value| {
                value_string_from_keys(value, &["namespace", "serverName", "server"])
            })
        });
    let tool = value_string_from_keys(record, &["tool", "toolName", "name", "displayName"])
        .or_else(|| {
            invocation.and_then(|value| {
                value_string_from_keys(value, &["tool", "toolName", "name", "displayName"])
            })
        });
    match (namespace, tool) {
        (Some(namespace), Some(tool)) => Some(format!("{namespace} · {tool}")),
        (Some(namespace), None) => Some(namespace),
        (None, Some(tool)) => Some(tool),
        (None, None) => None,
    }
}

fn looks_like_computer_tool(item: &Value) -> bool {
    let Some(record) = item.as_object() else {
        return false;
    };
    if record
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|value| value == "dynamicToolCall")
        && record
            .get("namespace")
            .and_then(Value::as_str)
            .is_some_and(|value| value.eq_ignore_ascii_case("computer"))
    {
        return true;
    }
    let label = computer_frame_tool_label(item)
        .unwrap_or_default()
        .to_ascii_lowercase();
    ["computer", "screenshot", "browser", "desktop", "remote"]
        .iter()
        .any(|needle| label.contains(needle))
}

pub(crate) fn map_app_server_computer_frame_notification(
    notification: &AppServerNotification,
) -> Option<Value> {
    if !matches!(
        notification.method.as_str(),
        "item/started" | "item/completed" | "rawResponseItem/added" | "rawResponseItem/completed"
    ) {
        return None;
    }
    let params = notification.params.as_object()?;
    let item = params
        .get("item")
        .or_else(|| params.get("rawResponseItem"))
        .or_else(|| params.get("responseItem"))?;
    if !looks_like_computer_tool(item) {
        return None;
    }
    let image_url = extract_computer_frame_image_url(item, 0)?;
    let thread_id = value_string_from_keys(
        params,
        &["threadId", "thread_id", "sessionId", "session_id"],
    )
    .unwrap_or_default();
    let turn_id = value_string_from_keys(params, &["turnId", "turn_id"]);
    let item_id = value_string_from_keys(params, &["itemId", "item_id"])
        .or_else(|| item.get("id").and_then(value_text));
    let mime_type = infer_computer_frame_mime_type(&image_url);
    let tool = computer_frame_tool_label(item);
    Some(json!({
        "kind": "notification",
        "method": "codex-webui/computerFrame",
        "params": {
            "threadId": thread_id,
            "turnId": turn_id,
            "itemId": item_id,
            "imageUrl": image_url,
            "mimeType": mime_type,
            "tool": tool,
            "transport": "websocket",
            "frameMode": "snapshot",
            "fpsHint": 1,
            "updatedAt": now_unix_ms()
        }
    }))
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
                .or_else(|| mapped.get("turn_id"))
                .and_then(Value::as_str)
                .unwrap_or("turn-0")
                .to_string();
            let fallback_status = if notification.method == "turn/started" {
                "inProgress"
            } else {
                "completed"
            };
            let mut turn = mapped.get("turn").cloned().unwrap_or_else(|| {
                json!({
                    "id": fallback_turn_id,
                    "status": fallback_status,
                    "items": []
                })
            });
            if let Some(turn_object) = turn.as_object_mut() {
                let missing_turn_id = turn_object
                    .get("id")
                    .and_then(Value::as_str)
                    .is_none_or(|value| value.trim().is_empty());
                if missing_turn_id {
                    turn_object.insert("id".to_string(), Value::String(fallback_turn_id));
                }
            }
            let mut normalized_turn = normalize_session_turn_payload(&turn, 0);
            if notification.method == "turn/completed" {
                mark_turn_without_agent_output_failed(&mut normalized_turn, "live");
            }
            mapped.insert("turn".to_string(), normalized_turn);
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
                .or_else(|| mapped.get("thread_id"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            mapped.insert("threadId".to_string(), Value::String(thread_id.clone()));
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
            if let Some(thread_id) = mapped
                .get("threadId")
                .or_else(|| mapped.get("thread_id"))
                .and_then(Value::as_str)
                .map(str::to_string)
            {
                mapped.insert("threadId".to_string(), Value::String(thread_id));
            }
            mapped.insert("goal".to_string(), Value::Null);
        }
        "item/commandExecution/outputDelta" => {
            let delta = mapped
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if delta.is_empty() {
                return None;
            }
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
        "remoteControl/status/changed" => Some(json!({
            "kind": "notification",
            "method": "codex-webui/remoteControlStatusChanged",
            "params": notification.params
        })),
        "app/list/updated" => Some(json!({
            "kind": "notification",
            "method": "codex-webui/appListUpdated",
            "params": notification.params
        })),
        _ => None,
    }
}
