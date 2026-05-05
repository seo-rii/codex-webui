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
            Some("experimentalFeature/enablement/set") => {
                let enablement = payload
                    .get("params")
                    .and_then(|params| params.get("enablement"))
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                print_message(&json!({
                    "id": id,
                    "result": {
                        "enablement": enablement
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
                let thread = json!({
                    "id": thread_id,
                    "name": "New thread",
                    "preview": "",
                    "cwd": cwd,
                    "archived": false,
                    "createdAt": timestamp_counter,
                    "updatedAt": timestamp_counter,
                    "status": "idle",
                    "isSubagent": false,
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
                    json!({
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
                    })
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
                let turn = json!({
                    "id": turn_id,
                    "status": "inProgress",
                    "error": Value::Null,
                    "startedAt": timestamp_counter,
                    "completedAt": Value::Null,
                    "durationMs": Value::Null,
                    "items": [
                        {
                            "id": format!("{turn_id}:user:0"),
                            "type": "userMessage",
                            "text": text.clone()
                        }
                    ]
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
                    thread_object
                        .insert("status".to_string(), Value::String("running".to_string()));
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
