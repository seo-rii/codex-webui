use std::{
    collections::BTreeMap,
    env, fs,
    io::{self, BufRead, Write},
    path::Path,
};

use serde_json::{Value, json};

fn append_line(path: &Path, line: &str) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{line}");
    }
}

fn print_message(payload: &Value) {
    println!(
        "{}",
        serde_json::to_string(payload).expect("payload should serialize")
    );
    io::stdout().flush().expect("stdout should flush");
}

fn thread_cursor_value(cursor: Option<&Value>) -> usize {
    cursor
        .and_then(Value::as_str)
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0)
}

fn main() {
    let start_log_path = env::var("FAKE_CODEX_START_LOG").ok();
    if let Some(path) = start_log_path.as_deref() {
        append_line(Path::new(path), "started");
    }

    let stdin = io::stdin();
    let mut server_request_id = 0_u64;
    let mut thread_counter = 0_u64;
    let mut timestamp_counter = 0_i64;
    let mut turn_counter = 0_u64;
    let mut threads = BTreeMap::<String, Value>::new();
    let args = env::args().skip(1).collect::<Vec<_>>();
    let mut goals_enabled = args
        .windows(2)
        .any(|window| window[0] == "--enable" && window[1] == "goals");

    for line in stdin.lock().lines() {
        let Ok(line) = line else {
            break;
        };
        if line.trim().is_empty() {
            continue;
        }

        let payload: Value = serde_json::from_str(&line).expect("input should be valid json");
        let id = payload.get("id").cloned();
        let method = payload.get("method").and_then(Value::as_str);

        match method {
            Some("initialize") => {
                print_message(&json!({
                    "id": id,
                    "result": {
                        "serverInfo": {
                            "name": "fake-codex",
                            "title": "Fake Codex App Server",
                            "version": "0.1.0"
                        }
                    }
                }));
            }
            Some("initialized") => {
                print_message(&json!({
                    "method": "fake/ready",
                    "params": {
                        "codexHome": env::var("CODEX_HOME").unwrap_or_default()
                    }
                }));
            }
            Some("experimentalFeature/list") => {
                print_message(&json!({
                    "id": id,
                    "result": {
                        "features": [
                            {
                                "key": "plugins",
                                "enabled": true,
                                "defaultEnabled": true,
                                "stage": "stable"
                            },
                            {
                                "key": "tool_suggest",
                                "enabled": true,
                                "defaultEnabled": true,
                                "stage": "beta"
                            }
                        ],
                        "nextCursor": null
                    }
                }));
            }
            Some("experimentalFeature/enablement/set") => {
                let enablement = payload
                    .get("params")
                    .and_then(|params| params.get("enablement"))
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                print_message(&json!({
                    "id": id,
                    "error": {
                        "code": -32602,
                        "message": format!(
                            "unsupported feature enablement `goals`: currently supported features are apps, memories, plugins, remote_control, tool_search, tool_suggest, tool_call_mcp_elicitation; requested {}",
                            enablement
                        )
                    }
                }));
            }
            Some("plugin/list") => {
                print_message(&json!({
                    "id": id,
                    "result": {
                        "marketplaces": [
                            {
                                "name": "openai-bundled",
                                "path": null,
                                "interface": {
                                    "displayName": "OpenAI bundled"
                                },
                                "plugins": [
                                    {
                                        "id": "computer-use@openai-bundled",
                                        "name": "computer-use",
                                        "shareContext": null,
                                        "source": {
                                            "type": "remote"
                                        },
                                        "installed": false,
                                        "enabled": true,
                                        "installPolicy": "AVAILABLE",
                                        "authPolicy": "ON_USE",
                                        "availability": "AVAILABLE",
                                        "interface": {
                                            "displayName": "Computer Use",
                                            "shortDescription": "Control a browser or desktop through Codex tools.",
                                            "developerName": "OpenAI",
                                            "category": "automation",
                                            "capabilities": ["mcp", "computer"]
                                        },
                                        "keywords": ["computer", "browser", "desktop"]
                                    }
                                ]
                            }
                        ],
                        "marketplaceLoadErrors": [],
                        "featuredPluginIds": ["computer-use@openai-bundled"]
                    }
                }));
            }
            Some("plugin/read") => {
                let params = payload.get("params").cloned().unwrap_or_else(|| json!({}));
                let plugin_name = params
                    .get("pluginName")
                    .and_then(Value::as_str)
                    .unwrap_or("computer-use");
                let marketplace_name = params
                    .get("remoteMarketplaceName")
                    .and_then(Value::as_str)
                    .unwrap_or("openai-bundled");
                print_message(&json!({
                    "id": id,
                    "result": {
                        "plugin": {
                            "marketplaceName": marketplace_name,
                            "marketplacePath": params.get("marketplacePath").cloned().unwrap_or(Value::Null),
                            "summary": {
                                "id": format!("{plugin_name}@{marketplace_name}"),
                                "name": plugin_name,
                                "shareContext": null,
                                "source": {
                                    "type": "remote"
                                },
                                "installed": false,
                                "enabled": true,
                                "installPolicy": "AVAILABLE",
                                "authPolicy": "ON_USE",
                                "availability": "AVAILABLE",
                                "interface": {
                                    "displayName": "Computer Use",
                                    "shortDescription": "Control a browser or desktop through Codex tools.",
                                    "developerName": "OpenAI",
                                    "category": "automation",
                                    "capabilities": ["mcp", "computer"]
                                },
                                "keywords": ["computer"]
                            },
                            "description": "Bundled computer-use plugin.",
                            "skills": [],
                            "hooks": [],
                            "apps": [],
                            "mcpServers": ["computer-use"]
                        }
                    }
                }));
            }
            Some("plugin/install") => {
                print_message(&json!({
                    "id": id,
                    "result": {
                        "authPolicy": "ON_USE",
                        "appsNeedingAuth": []
                    }
                }));
            }
            Some("plugin/uninstall") => {
                print_message(&json!({
                    "id": id,
                    "result": {}
                }));
            }
            Some("marketplace/add" | "marketplace/remove" | "marketplace/upgrade") => {
                print_message(&json!({
                    "id": id,
                    "result": {}
                }));
            }
            Some("skills/list") => {
                print_message(&json!({
                    "id": id,
                    "result": {
                        "skills": [
                            {
                                "name": "imagegen",
                                "description": "Generate and edit images.",
                                "path": "skills/.system/imagegen/SKILL.md",
                                "source": "system"
                            }
                        ],
                        "nextCursor": null
                    }
                }));
            }
            Some("hooks/list") => {
                print_message(&json!({
                    "id": id,
                    "result": {
                        "hooks": [],
                        "nextCursor": null
                    }
                }));
            }
            Some("mcpServerStatus/list") => {
                print_message(&json!({
                    "id": id,
                    "result": {
                        "data": [
                            {
                                "name": "computer-use",
                                "authStatus": "oAuth",
                                "tools": {
                                    "computer.screenshot": {
                                        "name": "computer.screenshot",
                                        "title": "Screenshot",
                                        "description": "Capture the current computer frame.",
                                        "inputSchema": {}
                                    }
                                },
                                "resources": [
                                    {
                                        "name": "session-log",
                                        "uri": "mcp://computer-use/session-log",
                                        "mimeType": "text/plain"
                                    }
                                ],
                                "resourceTemplates": []
                            }
                        ],
                        "nextCursor": null
                    }
                }));
            }
            Some("config/mcpServer/reload") => {
                print_message(&json!({
                    "id": id,
                    "result": {}
                }));
            }
            Some("mcpServer/oauth/login") => {
                print_message(&json!({
                    "id": id,
                    "result": {
                        "authorizationUrl": "https://example.com/oauth/authorize"
                    }
                }));
            }
            Some("app/list") => {
                print_message(&json!({
                    "id": id,
                    "result": {
                        "data": [
                            {
                                "id": "computer-use",
                                "name": "Computer Use",
                                "description": "Control a browser or desktop through Codex tools.",
                                "logoUrl": null,
                                "logoUrlDark": null,
                                "distributionChannel": "plugin",
                                "branding": null,
                                "appMetadata": null,
                                "labels": null,
                                "installUrl": null,
                                "isAccessible": true,
                                "isEnabled": true,
                                "pluginDisplayNames": ["Computer Use"]
                            }
                        ],
                        "nextCursor": null
                    }
                }));
            }
            Some("thread/realtime/listVoices") => {
                print_message(&json!({
                    "id": id,
                    "result": {
                        "voices": ["alloy", "verse"]
                    }
                }));
            }
            Some("thread/realtime/start") => {
                let params = payload.get("params").cloned().unwrap_or_else(|| json!({}));
                let thread_id = params
                    .get("threadId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                print_message(&json!({
                    "method": "thread/realtime/started",
                    "params": {
                        "threadId": thread_id,
                        "realtimeSessionId": "rt-test"
                    }
                }));
                if params
                    .get("transport")
                    .and_then(|transport| transport.get("type"))
                    .and_then(Value::as_str)
                    == Some("webrtc")
                {
                    print_message(&json!({
                        "method": "thread/realtime/sdp",
                        "params": {
                            "threadId": thread_id,
                            "sdp": "v=0\r\n"
                        }
                    }));
                }
                print_message(&json!({
                    "id": id,
                    "result": {}
                }));
            }
            Some(
                "thread/realtime/appendText"
                | "thread/realtime/appendAudio"
                | "thread/realtime/stop",
            ) => {
                print_message(&json!({
                    "id": id,
                    "result": {}
                }));
            }
            Some("config/batchWrite") => {
                if let Some(edits) = payload
                    .get("params")
                    .and_then(|params| params.get("edits"))
                    .and_then(Value::as_array)
                {
                    for edit in edits {
                        if edit.get("keyPath").and_then(Value::as_str) == Some("features.goals") {
                            goals_enabled = edit
                                .get("value")
                                .and_then(Value::as_bool)
                                .unwrap_or(goals_enabled);
                        }
                    }
                }
                print_message(&json!({
                    "id": id,
                    "result": {
                        "status": "ok"
                    }
                }));
            }
            Some("debug/setGoalsEnabled") => {
                goals_enabled = payload
                    .get("params")
                    .and_then(|params| params.get("enabled"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                print_message(&json!({
                    "id": id,
                    "result": {
                        "ok": true,
                        "enabled": goals_enabled
                    }
                }));
            }
            Some("echo") => {
                print_message(&json!({
                    "id": id,
                    "result": payload.get("params").cloned().unwrap_or_else(|| json!({}))
                }));
            }
            Some("emitNotification") => {
                print_message(&json!({
                    "method": "fake/custom",
                    "params": payload.get("params").cloned().unwrap_or_else(|| json!({}))
                }));
                print_message(&json!({
                    "id": id,
                    "result": {
                        "ok": true
                    }
                }));
            }
            Some("thread/start") => {
                thread_counter += 1;
                timestamp_counter += 1;
                let thread_id = format!("thread-{thread_counter}");
                let params = payload.get("params").cloned().unwrap_or_else(|| json!({}));
                let cwd = params
                    .get("cwd")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let model = params.get("model").cloned().unwrap_or(Value::Null);
                let is_subagent =
                    params.get("threadSource").and_then(Value::as_str) == Some("subagent");
                let thread = json!({
                    "id": thread_id,
                    "name": "New thread",
                    "preview": "",
                    "cwd": cwd,
                    "ephemeral": params.get("ephemeral").and_then(Value::as_bool).unwrap_or(false),
                    "threadSource": params.get("threadSource").cloned().unwrap_or(Value::Null),
                    "archived": false,
                    "createdAt": timestamp_counter,
                    "updatedAt": timestamp_counter,
                    "status": "idle",
                    "isSubagent": is_subagent,
                    "agentNickname": Value::Null,
                    "agentRole": Value::Null,
                    "model": model,
                    "turns": []
                });
                threads.insert(thread_id.clone(), thread.clone());
                print_message(&json!({
                    "id": id,
                    "result": {
                        "thread": thread
                    }
                }));
            }
            Some("thread/name/set") => {
                let thread_id = payload
                    .get("params")
                    .and_then(|params| params.get("threadId"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let name = payload
                    .get("params")
                    .and_then(|params| params.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                timestamp_counter += 1;
                if let Some(thread) = threads.get_mut(&thread_id).and_then(Value::as_object_mut) {
                    thread.insert("name".to_string(), Value::String(name.clone()));
                    thread.insert("updatedAt".to_string(), Value::from(timestamp_counter));
                }
                print_message(&json!({
                    "id": id,
                    "result": {
                        "ok": true
                    }
                }));
            }
            Some("thread/goal/get") => {
                if !goals_enabled {
                    print_message(&json!({
                        "id": id,
                        "error": {
                            "code": -32000,
                            "message": "goals feature is disabled"
                        }
                    }));
                    continue;
                }
                let thread_id = payload
                    .get("params")
                    .and_then(|params| params.get("threadId"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let goal = threads
                    .get(thread_id)
                    .and_then(|thread| thread.get("goal"))
                    .cloned()
                    .unwrap_or(Value::Null);
                print_message(&json!({
                    "id": id,
                    "result": {
                        "goal": goal
                    }
                }));
            }
            Some("thread/goal/set") => {
                if !goals_enabled {
                    print_message(&json!({
                        "id": id,
                        "error": {
                            "code": -32000,
                            "message": "goals feature is disabled"
                        }
                    }));
                    continue;
                }
                let params = payload.get("params").cloned().unwrap_or_else(|| json!({}));
                let thread_id = params
                    .get("threadId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                timestamp_counter += 1;
                let Some(thread_object) =
                    threads.get_mut(&thread_id).and_then(Value::as_object_mut)
                else {
                    print_message(&json!({
                        "id": id,
                        "error": {
                            "code": -32000,
                            "message": "thread not found"
                        }
                    }));
                    continue;
                };
                let existing_goal = thread_object.get("goal").cloned().unwrap_or(Value::Null);
                let objective = params
                    .get("objective")
                    .and_then(Value::as_str)
                    .or_else(|| existing_goal.get("objective").and_then(Value::as_str))
                    .unwrap_or_default()
                    .to_string();
                let status = params
                    .get("status")
                    .and_then(Value::as_str)
                    .or_else(|| existing_goal.get("status").and_then(Value::as_str))
                    .unwrap_or("active")
                    .to_string();
                let goal = json!({
                    "threadId": thread_id,
                    "objective": objective,
                    "status": status,
                    "tokenBudget": params
                        .get("tokenBudget")
                        .cloned()
                        .or_else(|| existing_goal.get("tokenBudget").cloned())
                        .unwrap_or(Value::Null),
                    "tokensUsed": existing_goal
                        .get("tokensUsed")
                        .and_then(Value::as_i64)
                        .unwrap_or(0),
                    "timeUsedSeconds": existing_goal
                        .get("timeUsedSeconds")
                        .and_then(Value::as_i64)
                        .unwrap_or(0),
                    "createdAt": existing_goal
                        .get("createdAt")
                        .and_then(Value::as_i64)
                        .unwrap_or(timestamp_counter),
                    "updatedAt": timestamp_counter
                });
                thread_object.insert("goal".to_string(), goal.clone());
                thread_object.insert("updatedAt".to_string(), Value::from(timestamp_counter));
                print_message(&json!({
                    "method": "thread/goal/updated",
                    "params": {
                        "threadId": thread_id,
                        "turnId": Value::Null,
                        "goal": goal
                    }
                }));
                print_message(&json!({
                    "id": id,
                    "result": {
                        "goal": goal
                    }
                }));
            }
            Some("thread/goal/clear") => {
                if !goals_enabled {
                    print_message(&json!({
                        "id": id,
                        "error": {
                            "code": -32000,
                            "message": "goals feature is disabled"
                        }
                    }));
                    continue;
                }
                let thread_id = payload
                    .get("params")
                    .and_then(|params| params.get("threadId"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let cleared = if let Some(thread_object) =
                    threads.get_mut(&thread_id).and_then(Value::as_object_mut)
                {
                    thread_object.remove("goal").is_some()
                } else {
                    false
                };
                if cleared {
                    print_message(&json!({
                        "method": "thread/goal/cleared",
                        "params": {
                            "threadId": thread_id
                        }
                    }));
                }
                print_message(&json!({
                    "id": id,
                    "result": {
                        "cleared": cleared
                    }
                }));
            }
            Some("thread/memoryMode/set") => {
                let thread_id = payload
                    .get("params")
                    .and_then(|params| params.get("threadId"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let mode = payload
                    .get("params")
                    .and_then(|params| params.get("mode"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if !matches!(mode.as_str(), "enabled" | "disabled") {
                    print_message(&json!({
                        "id": id,
                        "error": {
                            "code": -32602,
                            "message": "invalid memory mode"
                        }
                    }));
                    continue;
                }
                let Some(thread_object) =
                    threads.get_mut(&thread_id).and_then(Value::as_object_mut)
                else {
                    print_message(&json!({
                        "id": id,
                        "error": {
                            "code": -32000,
                            "message": "thread not found"
                        }
                    }));
                    continue;
                };
                thread_object.insert("memoryMode".to_string(), Value::String(mode));
                print_message(&json!({
                    "id": id,
                    "result": {}
                }));
            }
            Some("memory/reset") => {
                if let Ok(codex_home) = env::var("CODEX_HOME") {
                    let memory_root = Path::new(&codex_home).join("memories");
                    let _ = fs::remove_dir_all(&memory_root);
                    let _ = fs::create_dir_all(&memory_root);
                }
                print_message(&json!({
                    "id": id,
                    "result": {}
                }));
            }
            Some("thread/seed") => {
                let thread = payload
                    .get("params")
                    .and_then(|params| params.get("thread"))
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                if let Some(thread_id) = thread.get("id").and_then(Value::as_str) {
                    threads.insert(thread_id.to_string(), thread);
                }
                print_message(&json!({
                    "id": id,
                    "result": {
                        "ok": true
                    }
                }));
            }
            Some("thread/read") => {
                let thread_id = payload
                    .get("params")
                    .and_then(|params| params.get("threadId"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let thread = threads.get(thread_id).cloned().unwrap_or_else(|| {
                    let thread = json!({
                        "id": thread_id,
                        "name": "New thread",
                        "preview": "",
                        "cwd": "",
                        "archived": false,
                        "createdAt": 0,
                        "updatedAt": 0,
                        "status": "idle",
                        "isSubagent": false,
                        "agentNickname": Value::Null,
                        "agentRole": Value::Null,
                        "turns": []
                    });
                    if !thread_id.is_empty() {
                        threads.insert(thread_id.to_string(), thread.clone());
                    }
                    thread
                });
                let read_error = thread.get("readError").cloned().unwrap_or(Value::Null);
                if let Some(message) = read_error
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    print_message(&json!({
                        "id": id,
                        "error": {
                            "code": -32000,
                            "message": message
                        }
                    }));
                } else if let Some(message) = read_error
                    .as_str()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    print_message(&json!({
                        "id": id,
                        "error": {
                            "code": -32000,
                            "message": message
                        }
                    }));
                } else {
                    print_message(&json!({
                        "id": id,
                        "result": {
                            "thread": thread
                        }
                    }));
                }
            }
            Some("thread/list") => {
                let params = payload.get("params").cloned().unwrap_or_else(|| json!({}));
                let archived = params
                    .get("archived")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let limit = params
                    .get("limit")
                    .and_then(Value::as_u64)
                    .map(|value| value.clamp(1, 200) as usize)
                    .unwrap_or(20);
                let start = thread_cursor_value(params.get("cursor"));
                let mut data = threads
                    .values()
                    .filter(|thread| {
                        thread
                            .get("archived")
                            .and_then(Value::as_bool)
                            .unwrap_or(false)
                            == archived
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                data.sort_by(|left, right| {
                    right
                        .get("updatedAt")
                        .and_then(Value::as_i64)
                        .unwrap_or(0)
                        .cmp(&left.get("updatedAt").and_then(Value::as_i64).unwrap_or(0))
                });
                let end = start.saturating_add(limit).min(data.len());
                let next_cursor = (end < data.len()).then(|| end.to_string());
                let page = if start < data.len() {
                    data.drain(start..end).collect::<Vec<_>>()
                } else {
                    Vec::new()
                };
                print_message(&json!({
                    "id": id,
                    "result": {
                        "data": page,
                        "nextCursor": next_cursor
                    }
                }));
            }
            Some("thread/archive") | Some("thread/unarchive") => {
                let thread_id = payload
                    .get("params")
                    .and_then(|params| params.get("threadId"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if let Some(thread) = threads.get_mut(&thread_id).and_then(Value::as_object_mut) {
                    timestamp_counter += 1;
                    thread.insert(
                        "archived".to_string(),
                        Value::Bool(method == Some("thread/archive")),
                    );
                    thread.insert("updatedAt".to_string(), Value::from(timestamp_counter));
                }
                print_message(&json!({
                    "id": id,
                    "result": {
                        "ok": true
                    }
                }));
            }
            Some("thread/resume") => {
                let thread_id = payload
                    .get("params")
                    .and_then(|params| params.get("threadId"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if let Some(thread) = threads.get_mut(&thread_id).and_then(Value::as_object_mut) {
                    let resume_count = thread
                        .get("resumeCount")
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                        + 1;
                    thread.insert("status".to_string(), Value::String("idle".to_string()));
                    thread.insert("resumeCount".to_string(), Value::from(resume_count));
                }
                print_message(&json!({
                    "id": id,
                    "result": {
                        "ok": true
                    }
                }));
            }
            Some("thread/fork") => {
                let source_thread_id = payload
                    .get("params")
                    .and_then(|params| params.get("threadId"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let Some(source_thread) = threads.get(&source_thread_id).cloned() else {
                    print_message(&json!({
                        "id": id,
                        "error": {
                            "code": -32000,
                            "message": "thread not found"
                        }
                    }));
                    continue;
                };

                thread_counter += 1;
                timestamp_counter += 1;
                let forked_thread_id = format!("fork-{thread_counter}");
                let mut forked_thread = source_thread;
                if let Some(thread_object) = forked_thread.as_object_mut() {
                    thread_object.insert("id".to_string(), Value::String(forked_thread_id.clone()));
                    thread_object.insert("createdAt".to_string(), Value::from(timestamp_counter));
                    thread_object.insert("updatedAt".to_string(), Value::from(timestamp_counter));
                    thread_object.insert(
                        "forkedFrom".to_string(),
                        Value::String(source_thread_id.clone()),
                    );
                }
                threads.insert(forked_thread_id.clone(), forked_thread.clone());
                print_message(&json!({
                    "id": id,
                    "result": {
                        "thread": forked_thread
                    }
                }));
            }
            Some("thread/rollback") => {
                let thread_id = payload
                    .get("params")
                    .and_then(|params| params.get("threadId"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let num_turns = payload
                    .get("params")
                    .and_then(|params| params.get("numTurns"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize;
                let Some(thread_object) =
                    threads.get_mut(&thread_id).and_then(Value::as_object_mut)
                else {
                    print_message(&json!({
                        "id": id,
                        "error": {
                            "code": -32000,
                            "message": "thread not found"
                        }
                    }));
                    continue;
                };
                if let Some(turns) = thread_object.get_mut("turns").and_then(Value::as_array_mut) {
                    let remaining = turns.len().saturating_sub(num_turns);
                    turns.truncate(remaining);
                }
                let rollback_count = thread_object
                    .get("rollbackCount")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    + num_turns as u64;
                thread_object.insert("rollbackCount".to_string(), Value::from(rollback_count));
                thread_object.insert("updatedAt".to_string(), Value::from(timestamp_counter));
                print_message(&json!({
                    "id": id,
                    "result": {
                        "thread": Value::Object(thread_object.clone())
                    }
                }));
            }
            Some("review/start") => {
                let params = payload.get("params").cloned().unwrap_or_else(|| json!({}));
                let thread_id = params
                    .get("threadId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let delivery = params
                    .get("delivery")
                    .and_then(Value::as_str)
                    .unwrap_or("inline");
                timestamp_counter += 1;
                let review_thread_id = if delivery == "detached" {
                    thread_counter += 1;
                    let review_thread_id = format!("review-{thread_counter}");
                    let cwd = threads
                        .get(&thread_id)
                        .and_then(Value::as_object)
                        .and_then(|thread| thread.get("cwd"))
                        .cloned()
                        .unwrap_or_else(|| Value::String(String::new()));
                    threads.insert(
                        review_thread_id.clone(),
                        json!({
                            "id": review_thread_id.clone(),
                            "name": "Review",
                            "preview": "Review current changes",
                            "cwd": cwd,
                            "archived": false,
                            "createdAt": timestamp_counter,
                            "updatedAt": timestamp_counter,
                            "status": "running",
                            "isSubagent": false,
                            "agentNickname": Value::Null,
                            "agentRole": Value::Null,
                            "turns": []
                        }),
                    );
                    review_thread_id
                } else {
                    thread_id.clone()
                };
                let turn_id = format!("review-turn-{timestamp_counter}");
                if let Some(thread_object) = threads
                    .get_mut(&review_thread_id)
                    .and_then(Value::as_object_mut)
                {
                    let turn = json!({
                        "id": turn_id,
                        "status": "inProgress",
                        "error": Value::Null,
                        "startedAt": timestamp_counter,
                        "completedAt": Value::Null,
                        "durationMs": Value::Null,
                        "items": [
                            {
                                "id": format!("{turn_id}:review:0"),
                                "type": "enteredReviewMode",
                                "review": "Review current changes"
                            }
                        ]
                    });
                    thread_object
                        .entry("turns".to_string())
                        .or_insert_with(|| Value::Array(Vec::new()));
                    if let Some(turns) =
                        thread_object.get_mut("turns").and_then(Value::as_array_mut)
                    {
                        turns.push(turn);
                    }
                    thread_object
                        .insert("status".to_string(), Value::String("running".to_string()));
                    thread_object.insert("updatedAt".to_string(), Value::from(timestamp_counter));
                    thread_object.insert("lastReviewStart".to_string(), params.clone());
                }
                if review_thread_id != thread_id {
                    if let Some(thread_object) =
                        threads.get_mut(&thread_id).and_then(Value::as_object_mut)
                    {
                        thread_object.insert("lastReviewStart".to_string(), params.clone());
                    }
                }
                print_message(&json!({
                    "id": id,
                    "result": {
                        "turn": {
                            "id": turn_id,
                            "status": "inProgress",
                            "itemsView": "notLoaded",
                            "items": []
                        },
                        "reviewThreadId": review_thread_id
                    }
                }));
            }
            Some("turn/start") => {
                let params = payload.get("params").cloned().unwrap_or_else(|| json!({}));
                let thread_id = params
                    .get("threadId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                turn_counter += 1;
                timestamp_counter += 1;
                let turn_id = format!("turn-{turn_counter}");
                let thread = threads.entry(thread_id.clone()).or_insert_with(|| {
                    json!({
                        "id": thread_id,
                        "name": "New thread",
                        "preview": "",
                        "cwd": params.get("cwd").and_then(Value::as_str).unwrap_or_default(),
                        "archived": false,
                        "createdAt": timestamp_counter,
                        "updatedAt": timestamp_counter,
                        "status": "idle",
                        "isSubagent": false,
                        "agentNickname": Value::Null,
                        "agentRole": Value::Null,
                        "turns": []
                    })
                });

                let input = params
                    .get("input")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let text = input
                    .iter()
                    .find(|item| item.get("type").and_then(Value::as_str) == Some("text"))
                    .and_then(|item| item.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let client_user_message_id = params
                    .get("clientUserMessageId")
                    .and_then(Value::as_str)
                    .or_else(|| {
                        params
                            .get("responsesapiClientMetadata")
                            .and_then(|metadata| metadata.get("clientUserMessageId"))
                            .and_then(Value::as_str)
                    })
                    .map(str::to_string);
                let is_ephemeral = thread
                    .get("ephemeral")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let mut user_item = json!({
                    "id": format!("{turn_id}:user:0"),
                    "type": "userMessage",
                    "text": text.clone()
                });
                if let (Some(user_object), Some(client_user_message_id)) =
                    (user_item.as_object_mut(), client_user_message_id.as_ref())
                {
                    user_object.insert(
                        "clientId".to_string(),
                        Value::String(client_user_message_id.clone()),
                    );
                    user_object.insert(
                        "clientUserMessageId".to_string(),
                        Value::String(client_user_message_id.clone()),
                    );
                }
                let mut items = vec![user_item];
                if is_ephemeral {
                    let agent_text = if text.contains("Translate the following Codex answer") {
                        "번역된 응답입니다.".to_string()
                    } else {
                        let language = if text
                            .chars()
                            .any(|ch| ('\u{ac00}'..='\u{d7af}').contains(&ch))
                        {
                            "Korean"
                        } else {
                            "English"
                        };
                        let english = if language == "Korean" {
                            "Summarize it."
                        } else {
                            text.as_str()
                        };
                        json!({
                            "english": english,
                            "language": language
                        })
                        .to_string()
                    };
                    items.push(json!({
                        "id": format!("{turn_id}:agent:0"),
                        "type": "agentMessage",
                        "text": agent_text
                    }));
                }
                let turn = json!({
                    "id": turn_id,
                    "status": if is_ephemeral { "completed" } else { "inProgress" },
                    "error": Value::Null,
                    "startedAt": timestamp_counter,
                    "completedAt": if is_ephemeral { Value::from(timestamp_counter) } else { Value::Null },
                    "durationMs": if is_ephemeral { Value::from(0) } else { Value::Null },
                    "items": items
                });

                if let Some(thread_object) = thread.as_object_mut() {
                    let turns = thread_object
                        .entry("turns".to_string())
                        .or_insert_with(|| Value::Array(Vec::new()));
                    if let Some(turns) = turns.as_array_mut() {
                        turns.push(turn);
                    }
                    thread_object.insert(
                        "preview".to_string(),
                        Value::String(text.trim().to_string()),
                    );
                    thread_object.insert(
                        "status".to_string(),
                        Value::String(if is_ephemeral { "idle" } else { "running" }.to_string()),
                    );
                    thread_object.insert("updatedAt".to_string(), Value::from(timestamp_counter));
                    thread_object.insert("lastTurnStart".to_string(), params);
                }

                print_message(&json!({
                    "id": id,
                    "result": {
                        "turn": {
                            "id": turn_id
                        }
                    }
                }));
            }
            Some("turn/steer") => {
                let params = payload.get("params").cloned().unwrap_or_else(|| json!({}));
                let thread_id = params
                    .get("threadId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if let Some(thread_object) =
                    threads.get_mut(&thread_id).and_then(Value::as_object_mut)
                {
                    thread_object.insert("lastTurnSteer".to_string(), params.clone());
                }
                print_message(&json!({
                    "id": id,
                    "result": {
                        "turnId": params.get("expectedTurnId").cloned().unwrap_or(Value::Null)
                    }
                }));
            }
            Some("turn/interrupt") => {
                print_message(&json!({
                    "id": id,
                    "result": {
                        "interrupted": true
                    }
                }));
            }
            Some("thread/inject_items") => {
                let params = payload.get("params").cloned().unwrap_or_else(|| json!({}));
                let thread_id = params
                    .get("threadId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if let Some(thread_object) =
                    threads.get_mut(&thread_id).and_then(Value::as_object_mut)
                {
                    thread_object.insert("lastInjectedItems".to_string(), params.clone());
                    thread_object.insert("updatedAt".to_string(), Value::from(timestamp_counter));
                }
                print_message(&json!({
                    "id": id,
                    "result": {}
                }));
            }
            Some("mcpServer/tool/call") => {
                let params = payload.get("params").cloned().unwrap_or_else(|| json!({}));
                let thread_id = params
                    .get("threadId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if let Some(thread_object) =
                    threads.get_mut(&thread_id).and_then(Value::as_object_mut)
                {
                    thread_object.insert("lastMcpToolCall".to_string(), params.clone());
                    thread_object.insert("updatedAt".to_string(), Value::from(timestamp_counter));
                }
                print_message(&json!({
                    "id": id,
                    "result": {
                        "content": [
                            {
                                "type": "text",
                                "text": "computer input accepted"
                            }
                        ],
                        "structuredContent": {
                            "ok": true,
                            "params": params
                        },
                        "isError": false
                    }
                }));
            }
            Some("account/read") => {
                print_message(&json!({
                    "id": id,
                    "result": {
                        "account": {
                            "type": "chatgpt",
                            "email": "demo@example.com",
                            "planType": "plus"
                        },
                        "requiresOpenaiAuth": false
                    }
                }));
            }
            Some("model/list") => {
                print_message(&json!({
                    "id": id,
                    "result": {
                        "data": [
                            {
                                "id": "gpt-5",
                                "displayName": "GPT-5",
                                "description": "Default coding model",
                                "defaultReasoningEffort": "medium",
                                "supportedReasoningEfforts": ["low", "medium", "high"],
                                "additionalSpeedTiers": ["fast", "flex"],
                                "inputModalities": ["text", "image"],
                                "isDefault": true
                            }
                        ]
                    }
                }));
            }
            Some("collaborationMode/list") => {
                print_message(&json!({
                    "id": id,
                    "result": {
                        "data": [
                            {
                                "name": "Default",
                                "mode": "default",
                                "model": Value::Null,
                                "reasoning_effort": Value::Null
                            },
                            {
                                "name": "Plan",
                                "mode": "plan",
                                "model": Value::Null,
                                "reasoning_effort": "high"
                            }
                        ]
                    }
                }));
            }
            Some("account/login/start") => {
                let login_type = payload
                    .get("params")
                    .and_then(|params| params.get("type"))
                    .and_then(Value::as_str)
                    .unwrap_or("chatgpt");
                let result = match login_type {
                    "apiKey" => json!({
                        "type": "apiKey"
                    }),
                    "chatgptDeviceCode" => json!({
                        "type": "chatgptDeviceCode",
                        "loginId": "login-device-1",
                        "verificationUrl": "https://example.com/device",
                        "userCode": "ABCD-EFGH"
                    }),
                    _ => json!({
                        "type": "chatgpt",
                        "loginId": "login-chatgpt-1",
                        "authUrl": "https://example.com/auth"
                    }),
                };
                print_message(&json!({
                    "method": "account/login/completed",
                    "params": {
                        "loginId": result.get("loginId").cloned().unwrap_or(Value::Null),
                        "success": true,
                        "error": Value::Null
                    }
                }));
                print_message(&json!({
                    "method": "account/updated",
                    "params": {
                        "type": if login_type == "apiKey" { "apiKey" } else { "chatgpt" }
                    }
                }));
                print_message(&json!({
                    "method": "account/rateLimits/updated",
                    "params": {
                        "source": "fake"
                    }
                }));
                print_message(&json!({
                    "id": id,
                    "result": result
                }));
            }
            Some("account/login/cancel") => {
                print_message(&json!({
                    "id": id,
                    "result": {
                        "status": "canceled",
                        "loginId": payload
                            .get("params")
                            .and_then(|params| params.get("loginId"))
                            .cloned()
                            .unwrap_or(Value::Null)
                    }
                }));
            }
            Some("account/logout") => {
                print_message(&json!({
                    "method": "account/updated",
                    "params": {
                        "type": Value::Null
                    }
                }));
                print_message(&json!({
                    "id": id,
                    "result": {
                        "ok": true
                    }
                }));
            }
            Some("askQuestion") => {
                server_request_id += 1;
                print_message(&json!({
                    "id": format!("srv-{server_request_id}"),
                    "method": "input/request",
                    "params": {
                        "question": "Continue?"
                    }
                }));
                print_message(&json!({
                    "id": id,
                    "result": {
                        "ok": true
                    }
                }));
            }
            Some(other) => {
                print_message(&json!({
                    "id": id,
                    "error": {
                        "code": -32000,
                        "message": format!("unknown method: {other}")
                    }
                }));
            }
            None => {
                if payload.get("result").is_some() || payload.get("error").is_some() {
                    print_message(&json!({
                        "method": "fake/serverRequestResolved",
                        "params": {
                            "id": payload.get("id").cloned().unwrap_or(Value::Null),
                            "result": payload.get("result").cloned().unwrap_or(Value::Null),
                            "error": payload.get("error").cloned().unwrap_or(Value::Null)
                        }
                    }));
                }
            }
        }
    }
}
