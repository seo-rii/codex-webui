use std::{
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

fn main() {
    let start_log_path = env::var("FAKE_CODEX_START_LOG").ok();
    if let Some(path) = start_log_path.as_deref() {
        append_line(Path::new(path), "started");
    }

    let stdin = io::stdin();
    let mut server_request_id = 0_u64;

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
