use super::*;
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc};

fn unique_test_dir(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("codex-webui-{label}-{}", Uuid::new_v4()))
}

#[test]
fn default_gateway_runtime_stack_is_low_memory_friendly() {
    assert_eq!(DEFAULT_SERVER_THREAD_STACK_BYTES, 16 * 1024 * 1024);
    assert_eq!(
        runtime_thread_stack_bytes_from_env(
            "CODEX_WEBUI_TEST_MISSING_THREAD_STACK_BYTES",
            DEFAULT_SERVER_THREAD_STACK_BYTES,
        ),
        16 * 1024 * 1024
    );
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
            require_owner_role: false,
            require_origin_header: false,
            trust_proxy_headers: false,
            trusted_proxy_cidrs: Vec::new(),
            webhook_allowed_hosts: Vec::new(),
            instance_token: None,
            app_server_handoff_enabled: false,
            per_session_app_servers: false,
            force_yolo: false,
            restart_command: None,
            config_file_path: None,
        }),
        app_servers: AppServerManager::new(AppServerClientConfig::default()),
        http: reqwest::Client::new(),
        login_attempts: Arc::new(Mutex::new(HashMap::new())),
        response_cache: Arc::new(Mutex::new(HashMap::new())),
        session_thread_cache: Arc::new(Mutex::new(HashMap::new())),
        session_thread_cache_locks: Arc::new(Mutex::new(HashMap::new())),
        session_search_text_cache: Arc::new(Mutex::new(HashMap::new())),
        static_asset_cache: Arc::new(Mutex::new(HashMap::new())),
        catalog_cache: Arc::new(Mutex::new(HashMap::new())),
        git_repository_cache: Arc::new(Mutex::new(None)),
        pinned_git_repositories: Arc::new(Mutex::new(HashMap::new())),
        git_operation_locks: Arc::new(Mutex::new(HashMap::new())),
        inflight_requests: Arc::new(Mutex::new(HashMap::new())),
        profile_request_slots: Arc::new(Mutex::new(HashMap::new())),
        quota_cache: Arc::new(Mutex::new(HashMap::new())),
        quota_refreshes: Arc::new(Mutex::new(HashSet::new())),
        attachment_storage_usage_cache: Arc::new(Mutex::new(HashMap::new())),
        attachment_storage_locks: Arc::new(Mutex::new(HashMap::new())),
        relays: Arc::new(Mutex::new(HashMap::new())),
        session_event_epoch: Arc::from(Uuid::new_v4().to_string()),
        session_event_sequences: Arc::new(Mutex::new(HashMap::new())),
        terminals: Arc::new(Mutex::new(HashMap::new())),
        session_summary_update_tasks: Arc::new(Mutex::new(HashMap::new())),
        runtime_config_update_tasks: Arc::new(Mutex::new(HashMap::new())),
        ui_state_locks: Arc::new(Mutex::new(HashMap::new())),
        ui_state_cache: Arc::new(Mutex::new(HashMap::new())),
        ui_state_persistence: Arc::new(Mutex::new(HashMap::new())),
        automation_timers: Arc::new(Mutex::new(HashMap::new())),
        queue_dispatching: Arc::new(Mutex::new(HashSet::new())),
        queue_drain_retries: Arc::new(Mutex::new(HashMap::new())),
        session_operation_locks: Arc::new(Mutex::new(HashMap::new())),
        session_app_server_assignments: Arc::new(Mutex::new(HashMap::new())),
        active_turns: Arc::new(Mutex::new(HashMap::new())),
        pending_turn_starts: Arc::new(Mutex::new(HashSet::new())),
        recent_client_user_messages: Arc::new(Mutex::new(HashMap::new())),
        pending_server_requests: Arc::new(Mutex::new(HashMap::new())),
        account_login_flows: Arc::new(Mutex::new(HashMap::new())),
        shutdown_timers: Arc::new(Mutex::new(HashMap::new())),
        runtime_profile_monitors: Arc::new(std::sync::Mutex::new(HashMap::new())),
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
import os
import shutil
import sys
import time

threads = {}
thread_counter = 0
timestamp_counter = 0
turn_counter = 0
request_counts = {}
request_log = []
method_delays = {}
method_errors = {}
rate_limits_response = {
    "rateLimits": {
        "limitId": "codex",
        "limitName": "Codex",
        "primary": None,
        "secondary": None,
        "credits": None,
        "individualLimit": None,
        "planType": None,
        "rateLimitReachedType": None
    },
    "rateLimitsByLimitId": {}
}
args = sys.argv[1:]
goals_enabled = any(args[index:index + 2] == ["--enable", "goals"] for index in range(len(args)))

for raw_line in sys.stdin:
    line = raw_line.strip()
    if not line:
        continue

    payload = json.loads(line)
    request_id = payload.get("id")
    method = payload.get("method")
    params = payload.get("params") or {}
    request_counts[method] = int(request_counts.get(method, 0) or 0) + 1
    request_log.append(method)

    def write(message):
        sys.stdout.write(json.dumps(message) + "\n")
        sys.stdout.flush()

    if method == "debug/setDelay":
        target = str(params.get("method") or "")
        delay_ms = max(0, int(params.get("delayMs", 0) or 0))
        if target:
            method_delays[target] = delay_ms
        write({
            "id": request_id,
            "result": {
                "ok": True
            }
        })
        continue
    if method == "debug/setError":
        target = str(params.get("method") or "")
        message = str(params.get("message") or "debug forced error")
        if target:
            method_errors[target] = message
        write({
            "id": request_id,
            "result": {
                "ok": True
            }
        })
        continue
    if method == "debug/setGoalsEnabled":
        goals_enabled = bool(params.get("enabled"))
        write({
            "id": request_id,
            "result": {
                "ok": True,
                "enabled": goals_enabled
            }
        })
        continue
    if method == "debug/setRateLimitsResponse":
        rate_limits_response = params.get("response") or {}
        write({
            "id": request_id,
            "result": {
                "ok": True
            }
        })
        continue
    if method == "debug/argv":
        write({
            "id": request_id,
            "result": {
                "args": args
            }
        })
        continue

    delay_ms = int(method_delays.get(method, 0) or 0)
    if delay_ms > 0:
        time.sleep(delay_ms / 1000)
    error_message = method_errors.get(method)
    if error_message:
        write({
            "id": request_id,
            "error": {
                "code": -32000,
                "message": str(error_message)
            }
        })
        continue

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
    elif method == "experimentalFeature/list":
        write({
            "id": request_id,
            "result": {
                "features": [
                    {
                        "key": "plugins",
                        "enabled": True,
                        "defaultEnabled": True,
                        "stage": "stable"
                    },
                    {
                        "key": "tool_suggest",
                        "enabled": True,
                        "defaultEnabled": True,
                        "stage": "beta"
                    }
                ],
                "nextCursor": None
            }
        })
    elif method == "experimentalFeature/enablement/set":
        write({
            "id": request_id,
            "error": {
                "code": -32602,
                "message": "unsupported feature enablement `goals`: currently supported features are apps, memories, plugins, remote_control, tool_search, tool_suggest, tool_call_mcp_elicitation"
            }
        })
    elif method == "plugin/list":
        write({
            "id": request_id,
            "result": {
                "marketplaces": [
                    {
                        "name": "openai-bundled",
                        "path": None,
                        "interface": {
                            "displayName": "OpenAI bundled"
                        },
                        "plugins": [
                            {
                                "id": "computer-use@openai-bundled",
                                "name": "computer-use",
                                "shareContext": None,
                                "source": {
                                    "type": "remote"
                                },
                                "installed": False,
                                "enabled": True,
                                "installPolicy": "AVAILABLE",
                                "authPolicy": "ON_USE",
                                "availability": "AVAILABLE",
                                "interface": {
                                    "displayName": "Computer Use",
                                    "shortDescription": "Control a browser or desktop through Codex tools.",
                                    "developerName": "OpenAI",
                                    "category": "automation",
                                    "capabilities": ["mcp", "computer"],
                                    "defaultPrompt": ["Use the computer-use tools when a task needs UI interaction."]
                                },
                                "keywords": ["computer", "browser", "desktop"]
                            }
                        ]
                    }
                ],
                "marketplaceLoadErrors": [],
                "featuredPluginIds": ["computer-use@openai-bundled"]
            }
        })
    elif method == "plugin/read":
        plugin_name = params.get("pluginName") or params.get("plugin_name") or "computer-use"
        marketplace_name = params.get("remoteMarketplaceName") or "openai-bundled"
        write({
            "id": request_id,
            "result": {
                "plugin": {
                    "marketplaceName": marketplace_name,
                    "marketplacePath": params.get("marketplacePath"),
                    "summary": {
                        "id": f"{plugin_name}@{marketplace_name}",
                        "name": plugin_name,
                        "shareContext": None,
                        "source": {
                            "type": "remote"
                        },
                        "installed": False,
                        "enabled": True,
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
        })
    elif method == "plugin/install":
        write({
            "id": request_id,
            "result": {
                "authPolicy": "ON_USE",
                "appsNeedingAuth": []
            }
        })
    elif method == "plugin/uninstall":
        write({
            "id": request_id,
            "result": {}
        })
    elif method in ("marketplace/add", "marketplace/remove", "marketplace/upgrade"):
        write({
            "id": request_id,
            "result": {}
        })
    elif method == "skills/list":
        write({
            "id": request_id,
            "result": {
                "skills": [
                    {
                        "name": "imagegen",
                        "description": "Generate and edit images.",
                        "path": "skills/.system/imagegen/SKILL.md",
                        "source": "system"
                    }
                ],
                "nextCursor": None
            }
        })
    elif method == "hooks/list":
        write({
            "id": request_id,
            "result": {
                "hooks": [],
                "nextCursor": None
            }
        })
    elif method == "mcpServerStatus/list":
        write({
            "id": request_id,
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
                "nextCursor": None
            }
        })
    elif method == "config/mcpServer/reload":
        write({
            "id": request_id,
            "result": {}
        })
    elif method == "mcpServer/oauth/login":
        write({
            "id": request_id,
            "result": {
                "authorizationUrl": "https://example.com/oauth/authorize"
            }
        })
    elif method == "app/list":
        write({
            "id": request_id,
            "result": {
                "data": [
                    {
                        "id": "computer-use",
                        "name": "Computer Use",
                        "description": "Control a browser or desktop through Codex tools.",
                        "logoUrl": None,
                        "logoUrlDark": None,
                        "distributionChannel": "plugin",
                        "branding": None,
                        "appMetadata": None,
                        "labels": None,
                        "installUrl": None,
                        "isAccessible": True,
                        "isEnabled": True,
                        "pluginDisplayNames": ["Computer Use"]
                    }
                ],
                "nextCursor": None
            }
        })
    elif method == "thread/realtime/listVoices":
        write({
            "id": request_id,
            "result": {
                "voices": ["alloy", "verse"]
            }
        })
    elif method == "thread/realtime/start":
        thread_id = params.get("threadId", "")
        write({
            "method": "thread/realtime/started",
            "params": {
                "threadId": thread_id,
                "realtimeSessionId": "rt-test"
            }
        })
        if isinstance(params.get("transport"), dict) and params["transport"].get("type") == "webrtc":
            write({
                "method": "thread/realtime/sdp",
                "params": {
                    "threadId": thread_id,
                    "sdp": "v=0\\r\\n"
                }
            })
        write({
            "id": request_id,
            "result": {}
        })
    elif method in ("thread/realtime/appendText", "thread/realtime/appendAudio", "thread/realtime/stop"):
        write({
            "id": request_id,
            "result": {}
        })
    elif method == "config/batchWrite":
        for edit in params.get("edits") or []:
            if edit.get("keyPath") == "features.goals":
                goals_enabled = bool(edit.get("value"))
        write({
            "id": request_id,
            "result": {
                "status": "ok"
            }
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
            "ephemeral": bool(params.get("ephemeral", False)),
            "developerInstructions": params.get("developerInstructions"),
            "threadSource": params.get("threadSource"),
            "archived": False,
            "createdAt": timestamp_counter,
            "updatedAt": timestamp_counter,
            "status": "idle",
            "isSubagent": params.get("threadSource") == "subagent",
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
        write({
            "method": "thread/name/updated",
            "params": {
                "threadId": thread_id,
                "threadName": params.get("name", "")
            }
        })
    elif method == "thread/goal/get":
        if not goals_enabled:
            write({
                "id": request_id,
                "error": {
                    "code": -32000,
                    "message": "goals feature is disabled"
                }
            })
            continue
        thread_id = params.get("threadId", "")
        thread = threads.get(thread_id) or {}
        write({
            "id": request_id,
            "result": {
                "goal": thread.get("goal")
            }
        })
    elif method == "thread/goal/set":
        if not goals_enabled:
            write({
                "id": request_id,
                "error": {
                    "code": -32000,
                    "message": "goals feature is disabled"
                }
            })
            continue
        thread_id = params.get("threadId", "")
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
        timestamp_counter += 1
        existing_goal = thread.get("goal") or {}
        goal = {
            "threadId": thread_id,
            "objective": params.get("objective") or existing_goal.get("objective", ""),
            "status": params.get("status") or existing_goal.get("status", "active"),
            "tokenBudget": params["tokenBudget"] if "tokenBudget" in params else existing_goal.get("tokenBudget"),
            "tokensUsed": int(existing_goal.get("tokensUsed", 0) or 0),
            "timeUsedSeconds": int(existing_goal.get("timeUsedSeconds", 0) or 0),
            "createdAt": int(existing_goal.get("createdAt", timestamp_counter) or timestamp_counter),
            "updatedAt": timestamp_counter
        }
        thread["goal"] = goal
        thread["updatedAt"] = timestamp_counter
        write({
            "method": "thread/goal/updated",
            "params": {
                "threadId": thread_id,
                "turnId": None,
                "goal": goal
            }
        })
        write({
            "id": request_id,
            "result": {
                "goal": goal
            }
        })
    elif method == "thread/goal/clear":
        if not goals_enabled:
            write({
                "id": request_id,
                "error": {
                    "code": -32000,
                    "message": "goals feature is disabled"
                }
            })
            continue
        thread_id = params.get("threadId", "")
        thread = threads.get(thread_id)
        cleared = False
        if isinstance(thread, dict):
            cleared = thread.pop("goal", None) is not None
        if cleared:
            write({
                "method": "thread/goal/cleared",
                "params": {
                    "threadId": thread_id
                }
            })
        write({
            "id": request_id,
            "result": {
                "cleared": cleared
            }
        })
    elif method == "thread/memoryMode/set":
        thread_id = params.get("threadId", "")
        mode = params.get("mode", "")
        if mode not in ("enabled", "disabled"):
            write({
                "id": request_id,
                "error": {
                    "code": -32602,
                    "message": "invalid memory mode"
                }
            })
            continue
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
        thread["memoryMode"] = mode
        write({
            "id": request_id,
            "result": {}
        })
    elif method == "memory/reset":
        memory_root = os.path.join(os.environ.get("CODEX_HOME", ""), "memories")
        if memory_root.strip():
            shutil.rmtree(memory_root, ignore_errors=True)
            os.makedirs(memory_root, exist_ok=True)
        write({
            "id": request_id,
            "result": {}
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
        thread = threads.get(thread_id)
        if not isinstance(thread, dict):
            thread = {
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
            }
            if thread_id:
                threads[thread_id] = thread
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
    elif method == "thread/loaded/list":
        limit = max(1, min(int(params.get("limit", 200) or 200), 200))
        cursor = str(params.get("cursor") or "").strip()
        start = int(cursor) if cursor.isdigit() else 0
        data = list(threads.keys())
        end = min(start + limit, len(data))
        next_cursor = str(end) if end < len(data) else None
        write({
            "id": request_id,
            "result": {
                "data": data[start:end] if start < len(data) else [],
                "nextCursor": next_cursor
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
    elif method in ("thread/archive", "thread/unarchive"):
        thread_id = params.get("threadId", "")
        if thread_id in threads:
            timestamp_counter += 1
            threads[thread_id]["archived"] = method == "thread/archive"
            threads[thread_id]["updatedAt"] = timestamp_counter
        write({
            "id": request_id,
            "result": {
                "ok": True
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
    elif method == "debug/requestLog":
        write({
            "id": request_id,
            "result": {
                "methods": list(request_log)
            }
        })
    elif method == "thread/resume":
        thread_id = params.get("threadId", "")
        if thread_id and thread_id not in threads:
            timestamp_counter += 1
            threads[thread_id] = {
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
    elif method == "review/start":
        thread_id = params.get("threadId", "")
        target = params.get("target") or {}
        delivery = str(params.get("delivery") or "inline")
        timestamp_counter += 1
        review_thread_id = thread_id
        if delivery == "detached":
            thread_counter += 1
            review_thread_id = f"review-{thread_counter}"
            source_thread = threads.get(thread_id) if isinstance(threads.get(thread_id), dict) else {}
            threads[review_thread_id] = {
                "id": review_thread_id,
                "name": "Review",
                "preview": "Review current changes",
                "cwd": source_thread.get("cwd", ""),
                "archived": False,
                "createdAt": timestamp_counter,
                "updatedAt": timestamp_counter,
                "status": "running",
                "isSubagent": False,
                "agentNickname": None,
                "agentRole": None,
                "turns": []
            }
        thread = threads.get(review_thread_id)
        if not isinstance(thread, dict):
            thread = {
                "id": review_thread_id,
                "name": "Review",
                "preview": "Review current changes",
                "cwd": "",
                "archived": False,
                "createdAt": timestamp_counter,
                "updatedAt": timestamp_counter,
                "status": "running",
                "isSubagent": False,
                "agentNickname": None,
                "agentRole": None,
                "turns": []
            }
        turn_id = f"review-turn-{timestamp_counter}"
        target_label = target.get("type", "custom") if isinstance(target, dict) else "custom"
        turn = {
            "id": turn_id,
            "status": "inProgress",
            "error": None,
            "startedAt": timestamp_counter,
            "completedAt": None,
            "durationMs": None,
            "items": [
                {
                    "id": f"{turn_id}:review:0",
                    "type": "enteredReviewMode",
                    "review": f"Review target: {target_label}"
                }
            ]
        }
        thread["turns"] = list(thread.get("turns") or []) + [turn]
        thread["status"] = "running"
        thread["updatedAt"] = timestamp_counter
        thread["lastReviewStart"] = params
        threads[review_thread_id] = thread
        if review_thread_id != thread_id and isinstance(threads.get(thread_id), dict):
            threads[thread_id]["lastReviewStart"] = params
        write({
            "id": request_id,
            "result": {
                "turn": {
                    "id": turn_id,
                    "status": "inProgress",
                    "itemsView": "notLoaded",
                    "items": turn["items"]
                },
                "reviewThreadId": review_thread_id
            }
        })
    elif method == "turn/start":
        sandbox_policy = params.get("sandboxPolicy")
        if isinstance(sandbox_policy, dict):
            if sandbox_policy.get("type") == "readOnly":
                access = sandbox_policy.get("access")
                if isinstance(access, dict) and access.get("type") == "restricted":
                    write({
                        "id": request_id,
                        "error": {
                            "code": -32602,
                            "message": "Invalid request: readOnly.access is no longer supported; use permissionProfile for restricted reads"
                        }
                    })
                    continue
            if sandbox_policy.get("type") == "workspaceWrite":
                read_only_access = sandbox_policy.get("readOnlyAccess")
                if isinstance(read_only_access, dict) and read_only_access.get("type") == "restricted":
                    write({
                        "id": request_id,
                        "error": {
                            "code": -32602,
                            "message": "Invalid request: workspaceWrite.readOnlyAccess is no longer supported; use permissionProfile for restricted reads"
                        }
                    })
                    continue
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
        if bool(thread.get("isSubagent", False)):
            write({
                "id": request_id,
                "error": {
                    "code": -32602,
                    "message": "direct app-server input is not allowed for multi-agent v2 sub-agents"
                }
            })
            continue
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
        client_user_message_id = params.get("clientUserMessageId")
        if not isinstance(client_user_message_id, str) or not client_user_message_id.strip():
            metadata = params.get("responsesapiClientMetadata")
            if isinstance(metadata, dict):
                client_user_message_id = metadata.get("clientUserMessageId")
        if not isinstance(client_user_message_id, str) or not client_user_message_id.strip():
            client_user_message_id = None
        is_ephemeral = bool(thread.get("ephemeral", False))
        agent_items = []
        if is_ephemeral:
            if "Generate one concise title for this coding conversation" in text_value:
                agent_text = "Repair generated session titles"
            elif "Translate the following Codex answer" in text_value:
                agent_text = "번역된 응답입니다."
            else:
                language = "Korean" if any("\uac00" <= ch <= "\ud7af" for ch in text_value) else "English"
                english = "Summarize it." if language == "Korean" else text_value
                agent_text = json.dumps({
                    "english": english,
                    "language": language
                })
            agent_items.append({
                "id": f"{turn_id}:agent:0",
                "type": "agentMessage",
                "text": agent_text
            })
        user_item = {
            "id": f"{turn_id}:user:0",
            "type": "userMessage",
            "text": text_value
        }
        if client_user_message_id:
            user_item["clientId"] = client_user_message_id
            user_item["clientUserMessageId"] = client_user_message_id
        turn = {
            "id": turn_id,
            "status": "completed" if is_ephemeral else "inProgress",
            "error": None,
            "startedAt": timestamp_counter,
            "completedAt": timestamp_counter if is_ephemeral else None,
            "durationMs": 0 if is_ephemeral else None,
            "items": [user_item] + agent_items
        }
        thread["turns"] = list(thread.get("turns") or []) + [turn]
        thread["preview"] = text_value.strip()
        thread["status"] = "idle" if is_ephemeral else "running"
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
        if is_ephemeral:
            write({
                "method": "turn/completed",
                "params": {
                    "threadId": thread_id,
                    "turn": turn
                }
            })
    elif method == "thread/compact/start":
        thread_id = params.get("threadId", "")
        turn_counter += 1
        timestamp_counter += 1
        turn_id = f"turn-{turn_counter}"
        thread = threads.get(thread_id) or {
            "id": thread_id,
            "name": "New thread",
            "preview": "",
            "cwd": "",
            "archived": False,
            "createdAt": timestamp_counter,
            "updatedAt": timestamp_counter,
            "status": "idle",
            "isSubagent": False,
            "agentNickname": None,
            "agentRole": None,
            "turns": []
        }
        turn = {
            "id": turn_id,
            "status": "inProgress",
            "error": None,
            "startedAt": timestamp_counter,
            "completedAt": None,
            "durationMs": None,
            "items": [
                {
                    "id": f"{turn_id}:compact:0",
                    "type": "contextCompression",
                    "status": "inProgress"
                }
            ]
        }
        thread["turns"] = list(thread.get("turns") or []) + [turn]
        thread["status"] = "running"
        thread["updatedAt"] = timestamp_counter
        thread["lastCompactStart"] = params
        threads[thread_id] = thread
        write({
            "id": request_id,
            "result": {}
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
    elif method == "account/rateLimits/read":
        write({
            "id": request_id,
            "result": rate_limits_response
        })
    elif method == "account/rateLimitResetCredit/consume":
        if not params.get("idempotencyKey"):
            write({
                "id": request_id,
                "error": {
                    "code": -32602,
                    "message": "idempotencyKey must not be empty"
                }
            })
            continue
        write({
            "id": request_id,
            "result": {
                "outcome": "reset"
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
mod residual_safety;
mod runtime_queue_and_catalog;
mod session_flow;
mod settings_and_automation;
