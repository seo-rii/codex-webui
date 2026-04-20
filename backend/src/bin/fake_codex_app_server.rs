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
