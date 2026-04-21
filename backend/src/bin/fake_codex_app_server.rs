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
                print_message(&json!({
                    "id": id,
                    "result": {
                        "thread": thread
                    }
                }));
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
