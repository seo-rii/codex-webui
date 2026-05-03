use super::*;
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc};

fn unique_test_dir(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("codex-webui-{label}-{}", Uuid::new_v4()))
}

fn init_test_git_repo(repo_path: &Path) {
    fs::create_dir_all(repo_path).unwrap();
    let commands = [
        vec!["init".to_string(), repo_path.display().to_string()],
        vec![
            "-C".to_string(),
            repo_path.display().to_string(),
            "config".to_string(),
            "user.name".to_string(),
            "Codex WebUI".to_string(),
        ],
        vec![
            "-C".to_string(),
            repo_path.display().to_string(),
            "config".to_string(),
            "user.email".to_string(),
            "codex-webui@example.com".to_string(),
        ],
    ];
    for command in commands {
        let output = std::process::Command::new("git")
            .args(command)
            .output()
            .unwrap();
        assert!(output.status.success(), "git setup command failed");
    }

    fs::write(repo_path.join("README.md"), "init\n").unwrap();
    let add = std::process::Command::new("git")
        .args(["-C", repo_path.to_str().unwrap(), "add", "README.md"])
        .output()
        .unwrap();
    assert!(add.status.success(), "git add failed");
    let commit = std::process::Command::new("git")
        .args(["-C", repo_path.to_str().unwrap(), "commit", "-m", "init"])
        .output()
        .unwrap();
    assert!(commit.status.success(), "git commit failed");
}

fn test_state(project_root: PathBuf, allowed_roots: Vec<PathBuf>, codex_home: PathBuf) -> AppState {
    let profile_id = "default".to_string();
    let profile_data_dir = project_root
        .join(".data")
        .join("profiles")
        .join(&profile_id);
    let mut profiles = HashMap::new();
    profiles.insert(
        profile_id.clone(),
        RuntimeProfile {
            label: "Default".to_string(),
            codex_home,
            data_dir: profile_data_dir,
        },
    );

    AppState {
        config: Arc::new(Config {
            project_root: project_root.clone(),
            allowed_roots,
            default_profile_id: profile_id,
            profiles,
            data_dir: project_root.join(".data"),
            base_path: String::new(),
            static_dir: project_root.join("static"),
            public_host: "127.0.0.1".to_string(),
            public_port: 4173,
            codex_bin: "codex".to_string(),
            max_upload_bytes: 20 * 1024 * 1024,
            max_attachment_storage_bytes: 2 * 1024 * 1024 * 1024,
            git_discovery_depth: 1,
            system_shutdown_enabled: false,
            system_shutdown_delay_seconds: 30,
            system_shutdown_command_override: None,
            password: None,
            password_hash: None,
            owner_password: None,
            owner_password_hash: None,
            viewer_password: None,
            viewer_password_hash: None,
            hcaptcha_site_key: None,
            hcaptcha_secret_key: None,
            session_secret: Some("test-session-secret-for-cookie-signing".to_string()),
            cookie_same_site: SameSiteMode::Strict,
            cookie_secure_mode: CookieSecureMode::Auto,
            cors_allowed_origins: Vec::new(),
            require_origin_header: false,
            trust_proxy_headers: false,
            trusted_proxy_cidrs: Vec::new(),
            webhook_allowed_hosts: Vec::new(),
            instance_token: None,
            app_server_handoff_enabled: false,
            restart_command: None,
        }),
        app_servers: AppServerManager::new(AppServerClientConfig::default()),
        http: reqwest::Client::new(),
        login_attempts: Arc::new(Mutex::new(HashMap::new())),
        response_cache: Arc::new(Mutex::new(HashMap::new())),
        session_thread_cache: Arc::new(Mutex::new(HashMap::new())),
        session_search_text_cache: Arc::new(Mutex::new(HashMap::new())),
        static_asset_cache: Arc::new(Mutex::new(HashMap::new())),
        catalog_cache: Arc::new(Mutex::new(HashMap::new())),
        git_repository_cache: Arc::new(Mutex::new(None)),
        pinned_git_repositories: Arc::new(Mutex::new(HashMap::new())),
        git_operation_locks: Arc::new(Mutex::new(HashMap::new())),
        inflight_requests: Arc::new(Mutex::new(HashMap::new())),
        profile_request_slots: Arc::new(Mutex::new(HashMap::new())),
        quota_cache: Arc::new(Mutex::new(HashMap::new())),
        relays: Arc::new(Mutex::new(HashMap::new())),
        terminals: Arc::new(Mutex::new(HashMap::new())),
        ui_state_locks: Arc::new(Mutex::new(HashMap::new())),
        ui_state_cache: Arc::new(Mutex::new(HashMap::new())),
        automation_timers: Arc::new(Mutex::new(HashMap::new())),
        queue_dispatching: Arc::new(Mutex::new(HashSet::new())),
        queue_drain_retries: Arc::new(Mutex::new(HashMap::new())),
        active_turns: Arc::new(Mutex::new(HashMap::new())),
        pending_turn_starts: Arc::new(Mutex::new(HashSet::new())),
        pending_server_requests: Arc::new(Mutex::new(HashMap::new())),
        shutdown_timers: Arc::new(Mutex::new(HashMap::new())),
        preserve_app_servers_on_shutdown: Arc::new(AtomicBool::new(false)),
        shutdown_notify: Arc::new(Notify::new()),
        restart_plan: Arc::new(Mutex::new(None)),
    }
}

fn test_state_with_static_dir_and_base_path(
    project_root: PathBuf,
    allowed_roots: Vec<PathBuf>,
    codex_home: PathBuf,
    static_dir: PathBuf,
    base_path: &str,
) -> AppState {
    let mut state = test_state(project_root, allowed_roots, codex_home);
    let mut config = (*state.config).clone();
    config.static_dir = static_dir;
    config.base_path = base_path.to_string();
    state.config = Arc::new(config);
    state
}

fn test_state_with_fake_app_server(
    project_root: PathBuf,
    allowed_roots: Vec<PathBuf>,
    codex_home: PathBuf,
) -> AppState {
    let mut state = test_state(project_root, allowed_roots, codex_home);
    let fake_server_path = state.config.project_root.join("fake-codex-test.py");
    fs::write(
            &fake_server_path,
            r#"#!/usr/bin/env python3
import json
import sys
import time

threads = {}
thread_counter = 0
timestamp_counter = 0
turn_counter = 0
request_counts = {}

for raw_line in sys.stdin:
    line = raw_line.strip()
    if not line:
        continue

    payload = json.loads(line)
    request_id = payload.get("id")
    method = payload.get("method")
    params = payload.get("params") or {}
    request_counts[method] = int(request_counts.get(method, 0) or 0) + 1

    def write(message):
        sys.stdout.write(json.dumps(message) + "\n")
        sys.stdout.flush()

    if method == "initialize":
        write({
            "id": request_id,
            "result": {
                "serverInfo": {
                    "name": "fake-codex",
                    "title": "Fake Codex App Server",
                    "version": "0.1.0"
                }
            }
        })
    elif method == "initialized":
        write({
            "method": "fake/ready",
            "params": {}
        })
    elif method == "thread/start":
        thread_counter += 1
        timestamp_counter += 1
        thread_id = f"thread-{thread_counter}"
        thread = {
            "id": thread_id,
            "name": "New thread",
            "preview": "",
            "cwd": params.get("cwd", ""),
            "archived": False,
            "createdAt": timestamp_counter,
            "updatedAt": timestamp_counter,
            "status": "idle",
            "isSubagent": False,
            "agentNickname": None,
            "agentRole": None,
            "turns": []
        }
        threads[thread_id] = thread
        write({
            "id": request_id,
            "result": {
                "thread": thread
            }
        })
    elif method == "thread/name/set":
        thread_id = params.get("threadId", "")
        timestamp_counter += 1
        if thread_id in threads:
            threads[thread_id]["name"] = params.get("name", "")
            threads[thread_id]["updatedAt"] = timestamp_counter
        write({
            "id": request_id,
            "result": {
                "ok": True
            }
        })
    elif method == "thread/seed":
        thread = params.get("thread") or {}
        thread_id = thread.get("id")
        if isinstance(thread_id, str) and thread_id:
            threads[thread_id] = thread
        write({
            "id": request_id,
            "result": {
                "ok": True
            }
        })
    elif method == "thread/read":
        thread_id = params.get("threadId", "")
        thread = threads.get(thread_id, {
            "id": thread_id,
            "name": "New thread",
            "preview": "",
            "cwd": "",
            "archived": False,
            "createdAt": 0,
            "updatedAt": 0,
            "status": "idle",
            "isSubagent": False,
            "agentNickname": None,
            "agentRole": None,
            "turns": []
        })
        read_error = thread.get("readError")
        read_delay_ms = int(thread.get("readDelayMs", 0) or 0)
        if read_delay_ms > 0:
            time.sleep(read_delay_ms / 1000)
        if isinstance(read_error, dict) and str(read_error.get("message", "")).strip():
            write({
                "id": request_id,
                "error": {
                    "code": -32000,
                    "message": str(read_error.get("message"))
                }
            })
        elif isinstance(read_error, str) and read_error.strip():
            write({
                "id": request_id,
                "error": {
                    "code": -32000,
                    "message": read_error.strip()
                }
            })
        else:
            write({
                "id": request_id,
                "result": {
                    "thread": thread
                }
            })
    elif method == "thread/list":
        archived = bool(params.get("archived", False))
        limit = max(1, min(int(params.get("limit", 20) or 20), 200))
        cursor = str(params.get("cursor") or "").strip()
        start = int(cursor) if cursor.isdigit() else 0
        data = [thread for thread in threads.values() if bool(thread.get("archived", False)) == archived]
        data.sort(key=lambda thread: int(thread.get("updatedAt", 0)), reverse=True)
        end = min(start + limit, len(data))
        next_cursor = str(end) if end < len(data) else None
        write({
            "id": request_id,
            "result": {
                "data": data[start:end] if start < len(data) else [],
                "nextCursor": next_cursor
            }
        })
    elif method == "debug/requestCount":
        target = str(params.get("target") or "")
        write({
            "id": request_id,
            "result": {
                "count": int(request_counts.get(target, 0) or 0)
            }
        })
    elif method == "thread/resume":
        thread_id = params.get("threadId", "")
        if thread_id in threads:
            threads[thread_id]["status"] = "idle"
            threads[thread_id]["resumeCount"] = int(threads[thread_id].get("resumeCount", 0) or 0) + 1
        write({
            "id": request_id,
            "result": {
                "ok": True
            }
        })
    elif method == "thread/fork":
        source_thread_id = params.get("threadId", "")
        source_thread = threads.get(source_thread_id)
        if not isinstance(source_thread, dict):
            write({
                "id": request_id,
                "error": {
                    "code": -32000,
                    "message": "thread not found"
                }
            })
            continue
        thread_counter += 1
        timestamp_counter += 1
        forked_thread = json.loads(json.dumps(source_thread))
        forked_thread["id"] = f"fork-{thread_counter}"
        forked_thread["createdAt"] = timestamp_counter
        forked_thread["updatedAt"] = timestamp_counter
        forked_thread["forkedFrom"] = source_thread_id
        threads[forked_thread["id"]] = forked_thread
        write({
            "id": request_id,
            "result": {
                "thread": forked_thread
            }
        })
    elif method == "thread/rollback":
        thread_id = params.get("threadId", "")
        num_turns = max(0, int(params.get("numTurns", 0) or 0))
        thread = threads.get(thread_id)
        if not isinstance(thread, dict):
            write({
                "id": request_id,
                "error": {
                    "code": -32000,
                    "message": "thread not found"
                }
            })
            continue
        turns = list(thread.get("turns") or [])
        if num_turns > 0:
            turns = turns[:-num_turns] if num_turns < len(turns) else []
        thread["turns"] = turns
        thread["rollbackCount"] = int(thread.get("rollbackCount", 0) or 0) + num_turns
        thread["updatedAt"] = timestamp_counter
        threads[thread_id] = thread
        write({
            "id": request_id,
            "result": {
                "thread": thread
            }
        })
    elif method == "turn/start":
        thread_id = params.get("threadId", "")
        turn_counter += 1
        timestamp_counter += 1
        turn_id = f"turn-{turn_counter}"
        thread = threads.get(thread_id) or {
            "id": thread_id,
            "name": "New thread",
            "preview": "",
            "cwd": params.get("cwd", ""),
            "archived": False,
            "createdAt": timestamp_counter,
            "updatedAt": timestamp_counter,
            "status": "idle",
            "isSubagent": False,
            "agentNickname": None,
            "agentRole": None,
            "turns": []
        }
        input_items = params.get("input") or []
        text_item = next(
            (
                item for item in input_items
                if isinstance(item, dict) and item.get("type") == "text"
            ),
            {}
        )
        text_value = text_item.get("text") if isinstance(text_item, dict) else ""
        if not isinstance(text_value, str):
            text_value = ""
        turn = {
            "id": turn_id,
            "status": "inProgress",
            "error": None,
            "startedAt": timestamp_counter,
            "completedAt": None,
            "durationMs": None,
            "items": [
                {
                    "id": f"{turn_id}:user:0",
                    "type": "userMessage",
                    "text": text_value
                }
            ]
        }
        thread["turns"] = list(thread.get("turns") or []) + [turn]
        thread["preview"] = text_value.strip()
        thread["status"] = "running"
        thread["updatedAt"] = timestamp_counter
        thread["lastTurnStart"] = params
        threads[thread_id] = thread
        write({
            "id": request_id,
            "result": {
                "turn": {
                    "id": turn_id
                }
            }
        })
    elif method == "turn/steer":
        thread_id = params.get("threadId", "")
        expected_turn_id = params.get("expectedTurnId", "")
        if thread_id in threads:
            threads[thread_id]["lastTurnSteer"] = params
        write({
            "id": request_id,
            "result": {
                "turnId": expected_turn_id
            }
        })
    elif method == "turn/interrupt":
        write({
            "id": request_id,
            "result": {
                "interrupted": True
            }
        })
    elif method == "account/read":
        write({
            "id": request_id,
            "result": {
                "account": {
                    "type": "chatgpt",
                    "email": "demo@example.com",
                    "planType": "plus"
                },
                "requiresOpenaiAuth": False
            }
        })
    elif method == "model/list":
        write({
            "id": request_id,
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
                        "isDefault": True
                    }
                ]
            }
        })
    elif method == "collaborationMode/list":
        write({
            "id": request_id,
            "result": {
                "data": [
                    {
                        "name": "Default",
                        "mode": "default",
                        "model": None,
                        "reasoning_effort": None
                    },
                    {
                        "name": "Plan",
                        "mode": "plan",
                        "model": None,
                        "reasoning_effort": "high"
                    }
                ]
            }
        })
    else:
        write({
            "id": request_id,
            "error": {
                "code": -32000,
                "message": f"unknown method: {method}"
            }
        })
"#,
        )
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&fake_server_path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&fake_server_path, permissions).unwrap();
    }
    state.app_servers = AppServerManager::new(AppServerClientConfig {
        codex_bin: fake_server_path.display().to_string(),
        ..AppServerClientConfig::default()
    });
    state
}

mod attachments_and_recovery;
mod auth_git_static;
mod runtime_queue_and_catalog;
mod session_flow;
mod settings_and_automation;
