use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolve_server_request_payload_uses_pending_request_store() {
    let sandbox = unique_test_dir("approval-rust");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    handle_profile_server_request(
        &state,
        "default",
        &backend::codex_app_server::AppServerRequest {
            id: json!("srv-1"),
            method: "input/request".to_string(),
            params: json!({
                "threadId": "thread-1",
                "question": "Continue?"
            }),
        },
    )
    .await;

    let pending_before = state
        .pending_server_requests
        .lock()
        .await
        .get(&runtime_session_key("default", "thread-1"))
        .and_then(|entries| entries.get("srv-1"))
        .cloned();
    assert!(pending_before.is_some());

    let highlighted = with_ui_state_read(&state, "default", |ui_state| {
        Ok(ui_state
            .get("highlightsByThreadId")
            .and_then(Value::as_object)
            .and_then(|entries| entries.get("thread-1"))
            .cloned()
            .unwrap_or(Value::Null))
    })
    .await
    .unwrap();
    assert_eq!(
        highlighted.get("kind").and_then(Value::as_str),
        Some("attention")
    );

    let payload = resolve_server_request_payload(
        &state,
        "default",
        "thread-1",
        "srv-1",
        json!({ "answer": "yes" }),
    )
    .await
    .unwrap();
    assert_eq!(payload.get("ok").and_then(Value::as_bool), Some(true));

    let pending_after = state
        .pending_server_requests
        .lock()
        .await
        .get(&runtime_session_key("default", "thread-1"))
        .cloned();
    assert!(pending_after.is_none());

    let highlight_after = with_ui_state_read(&state, "default", |ui_state| {
        Ok(ui_state
            .get("highlightsByThreadId")
            .and_then(Value::as_object)
            .and_then(|entries| entries.get("thread-1"))
            .cloned()
            .unwrap_or(Value::Null))
    })
    .await
    .unwrap();
    assert!(highlight_after.is_null());

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolve_server_request_payload_returns_not_found_without_pending_request() {
    let sandbox = unique_test_dir("approval-missing");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);

    let error = resolve_server_request_payload(
        &state,
        "default",
        "thread-1",
        "missing-request",
        json!({ "answer": "yes" }),
    )
    .await
    .expect_err("missing request should fail");

    assert_eq!(error.status, StatusCode::NOT_FOUND);
    assert_eq!(error.message, "SERVER_REQUEST_NOT_FOUND");

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dynamic_tool_call_requests_can_be_resolved_with_content_items() {
    let sandbox = unique_test_dir("dynamic-tool-call-request");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    handle_profile_server_request(
        &state,
        "default",
        &backend::codex_app_server::AppServerRequest {
            id: json!("dynamic-1"),
            method: "item/tool/call".to_string(),
            params: json!({
                "threadId": "thread-dynamic",
                "turnId": "turn-1",
                "callId": "call-1",
                "namespace": "computer",
                "tool": "screenshot",
                "arguments": {}
            }),
        },
    )
    .await;

    let pending = state
        .pending_server_requests
        .lock()
        .await
        .get(&runtime_session_key("default", "thread-dynamic"))
        .and_then(|entries| entries.get("dynamic-1"))
        .cloned()
        .expect("dynamic tool request should be stored");
    assert_eq!(pending.method, "item/tool/call");
    assert_eq!(
        pending.params.get("namespace").and_then(Value::as_str),
        Some("computer")
    );

    let payload = resolve_server_request_payload(
        &state,
        "default",
        "thread-dynamic",
        "dynamic-1",
        json!({
            "contentItems": [
                {
                    "type": "inputText",
                    "text": "manual tool output"
                }
            ],
            "success": true
        }),
    )
    .await
    .unwrap();
    assert_eq!(payload.get("ok").and_then(Value::as_bool), Some(true));
    assert!(
        state
            .pending_server_requests
            .lock()
            .await
            .get(&runtime_session_key("default", "thread-dynamic"))
            .is_none()
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_status_includes_webui_build_metadata() {
    let sandbox = unique_test_dir("runtime-build-metadata");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let payload = codex_runtime_status(&state, false).await.unwrap();
    let build_version = payload
        .get("webuiBuildVersion")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let commit_short = payload
        .get("webuiBuildCommitShort")
        .and_then(Value::as_str)
        .unwrap_or_default();

    assert_eq!(
        payload.get("webuiVersion").and_then(Value::as_str),
        Some(env!("CARGO_PKG_VERSION"))
    );
    assert!(!build_version.is_empty());
    assert!(!commit_short.is_empty());
    assert!(build_version.contains(commit_short));
    assert!(
        payload
            .get("webuiBuildDirty")
            .and_then(Value::as_bool)
            .is_some()
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catalog_merges_app_server_computer_use_plugin() {
    let sandbox = unique_test_dir("catalog-computer-use-plugin");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let payload = get_catalog_payload(&state, "default").await.unwrap();
    let plugins = payload
        .get("plugins")
        .and_then(Value::as_array)
        .expect("catalog should include plugins array");
    let computer_use = plugins
        .iter()
        .find(|plugin| {
            plugin.get("mentionPath").and_then(Value::as_str)
                == Some("plugin://computer-use@openai-bundled")
        })
        .expect("computer-use plugin should be merged from app-server");

    assert_eq!(
        computer_use.get("displayName").and_then(Value::as_str),
        Some("Computer Use")
    );
    assert_eq!(
        computer_use.get("marketplaceName").and_then(Value::as_str),
        Some("openai-bundled")
    );
    assert_eq!(
        computer_use.get("installed").and_then(Value::as_bool),
        Some(false)
    );
    assert!(
        computer_use
            .get("capabilities")
            .and_then(Value::as_array)
            .is_some_and(|capabilities| capabilities
                .iter()
                .any(|value| value.as_str() == Some("computer")))
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[test]
fn turn_input_payload_encodes_codex_plugin_mentions() {
    let selected_plugins = vec![json!({
        "name": "Computer Use",
        "path": "plugin://computer-use@openai-bundled"
    })];

    let (input, additional_roots) =
        build_turn_input_payload("open a browser", &[], &selected_plugins);

    assert!(additional_roots.is_empty());
    assert_eq!(input.len(), 2);
    assert_eq!(
        input[0].get("text").and_then(Value::as_str),
        Some("@computer-use\n\nopen a browser")
    );
    assert_eq!(
        input[1].get("type").and_then(Value::as_str),
        Some("mention")
    );
    assert_eq!(
        input[1].get("path").and_then(Value::as_str),
        Some("plugin://computer-use@openai-bundled")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn codex_protocol_proxy_methods_forward_to_app_server() {
    let sandbox = unique_test_dir("codex-protocol-proxy");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let (out_tx, _out_rx) = mpsc::channel(8);
    let subscriptions = Arc::new(Mutex::new(HashMap::new()));
    let auth = AuthContext {
        role: UserRole::Owner,
        profile_id: "default".to_string(),
    };

    let features = execute_ws_method(
        &state,
        &out_tx,
        &subscriptions,
        &auth,
        "codex/features/list",
        json!({}),
    )
    .await
    .unwrap();
    assert!(
        features
            .get("features")
            .and_then(Value::as_array)
            .is_some_and(|features| features
                .iter()
                .any(|feature| { feature.get("key").and_then(Value::as_str) == Some("plugins") }))
    );

    let install = execute_ws_method(
        &state,
        &out_tx,
        &subscriptions,
        &auth,
        "codex/plugins/install",
        json!({
            "marketplacePath": Value::Null,
            "remoteMarketplaceName": "openai-bundled",
            "pluginName": "computer-use"
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        install.get("authPolicy").and_then(Value::as_str),
        Some("ON_USE")
    );

    let voices = execute_ws_method(
        &state,
        &out_tx,
        &subscriptions,
        &auth,
        "codex/realtime/listVoices",
        json!({}),
    )
    .await
    .unwrap();
    assert!(
        voices
            .get("voices")
            .and_then(Value::as_array)
            .is_some_and(|voices| voices.iter().any(|voice| voice.as_str() == Some("alloy")))
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn codex_apps_list_proxy_forwards_to_app_server() {
    let sandbox = unique_test_dir("codex-apps-list-proxy");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let (out_tx, _out_rx) = mpsc::channel(8);
    let subscriptions = Arc::new(Mutex::new(HashMap::new()));
    let auth = AuthContext {
        role: UserRole::Owner,
        profile_id: "default".to_string(),
    };

    let apps = execute_ws_method(
        &state,
        &out_tx,
        &subscriptions,
        &auth,
        "codex/apps/list",
        json!({}),
    )
    .await
    .unwrap();
    assert!(
        apps.get("data")
            .and_then(Value::as_array)
            .is_some_and(|apps| apps
                .iter()
                .any(|app| app.get("id").and_then(Value::as_str) == Some("computer-use")))
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_quota_status_reuses_recent_cache_for_forced_refresh() {
    let sandbox = unique_test_dir("runtime-quota-cache");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let cached_payload = json!({
        "available": true,
        "source": "cached",
        "fetchedAt": now_unix_ms(),
        "account": {
            "email": "cached@example.com"
        },
        "plan": {
            "type": "pro"
        },
        "fiveHour": {
            "remainingPercent": 90
        },
        "weekly": {
            "remainingPercent": 80
        },
        "error": Value::Null
    });
    state.quota_cache.lock().await.insert(
        "default".to_string(),
        CachedQuota {
            created_at: Instant::now(),
            payload: cached_payload.clone(),
        },
    );

    let payload = codex_quota_status(&state, true, "default").await.unwrap();
    assert_eq!(payload, cached_payload);

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_quota_status_returns_cached_payload_while_refresh_in_flight() {
    let sandbox = unique_test_dir("runtime-quota-inflight-cache");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let cached_payload = json!({
        "available": true,
        "source": "cached",
        "fetchedAt": now_unix_ms(),
        "account": {
            "email": "cached@example.com"
        },
        "plan": {
            "type": "pro"
        },
        "fiveHour": {
            "remainingPercent": 90
        },
        "weekly": {
            "remainingPercent": 80
        },
        "error": Value::Null
    });
    state.quota_cache.lock().await.insert(
        "default".to_string(),
        CachedQuota {
            created_at: Instant::now() - QUOTA_CACHE_TTL - Duration::from_secs(1),
            payload: cached_payload,
        },
    );
    state
        .quota_refreshes
        .lock()
        .await
        .insert("default".to_string());

    let payload = codex_quota_status(&state, true, "default").await.unwrap();
    assert_eq!(
        payload.get("source").and_then(Value::as_str),
        Some("cached")
    );
    assert_eq!(
        payload.get("refreshing").and_then(Value::as_bool),
        Some(true)
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_list_shows_running_when_active_turn_exists_but_thread_metadata_is_idle() {
    let sandbox = unique_test_dir("session-list-running-status-sync");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": "thread-1",
                    "name": "Status sync",
                    "preview": "",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 1,
                    "status": "idle",
                    "isSubagent": false,
                    "agentNickname": Value::Null,
                    "agentRole": Value::Null,
                    "turns": []
                }
            }),
        )
        .await
        .unwrap();

    handle_profile_runtime_notification(
        &state,
        "default",
        &AppServerNotification {
            method: "turn/started".to_string(),
            params: json!({
                "threadId": "thread-1",
                "turnId": "turn-1"
            }),
        },
    )
    .await;

    let payload = list_sessions_payload(
        &state,
        "default",
        false,
        None,
        20,
        &SessionFilterCriteria::default(),
    )
    .await
    .unwrap();
    let first = payload
        .get("sessions")
        .and_then(Value::as_array)
        .and_then(|sessions| sessions.first())
        .cloned()
        .expect("expected seeded session");
    assert_eq!(first.get("id").and_then(Value::as_str), Some("thread-1"));
    assert_eq!(first.get("status").and_then(Value::as_str), Some("running"));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_list_shows_completed_when_runtime_completion_beats_stale_thread_metadata() {
    let sandbox = unique_test_dir("session-list-completed-status-sync");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": "thread-1",
                    "name": "Status sync",
                    "preview": "",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 2,
                    "status": "running",
                    "isSubagent": false,
                    "agentNickname": Value::Null,
                    "agentRole": Value::Null,
                    "turns": []
                }
            }),
        )
        .await
        .unwrap();
    state.active_turns.lock().await.insert(
        runtime_session_key("default", "thread-1"),
        "turn-1".to_string(),
    );

    handle_profile_runtime_notification(
        &state,
        "default",
        &AppServerNotification {
            method: "turn/completed".to_string(),
            params: json!({
                "threadId": "thread-1",
                "turnId": "turn-1",
                "turn": {
                    "id": "turn-1",
                    "status": "completed",
                    "items": []
                }
            }),
        },
    )
    .await;

    let payload = list_sessions_payload(
        &state,
        "default",
        false,
        None,
        20,
        &SessionFilterCriteria::default(),
    )
    .await
    .unwrap();
    let first = payload
        .get("sessions")
        .and_then(Value::as_array)
        .and_then(|sessions| sessions.first())
        .cloned()
        .expect("expected seeded session");
    assert_eq!(first.get("id").and_then(Value::as_str), Some("thread-1"));
    assert_eq!(
        first.get("status").and_then(Value::as_str),
        Some("completed")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_list_reconciles_stale_running_status_without_opening_thread() {
    let sandbox = unique_test_dir("session-list-stale-running-reconcile");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": "thread-1",
                    "name": "Status sync",
                    "preview": "",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 20,
                    "status": "completed",
                    "isSubagent": false,
                    "agentNickname": Value::Null,
                    "agentRole": Value::Null,
                    "turns": [
                        {
                            "id": "turn-1",
                            "status": "completed",
                            "error": Value::Null,
                            "startedAt": 10,
                            "completedAt": 20,
                            "durationMs": 10,
                            "items": []
                        }
                    ]
                }
            }),
        )
        .await
        .unwrap();
    state.active_turns.lock().await.insert(
        runtime_session_key("default", "thread-1"),
        "turn-1".to_string(),
    );
    state
        .pending_turn_starts
        .lock()
        .await
        .insert(runtime_session_key("default", "thread-1"));
    with_ui_state_write(&state, "default", |ui_state| {
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
            "thread-1".to_string(),
            json!({
                "status": "running",
                "updatedAt": now_unix_ms().saturating_sub(10_000)
            }),
        );
        Ok(())
    })
    .await
    .unwrap();

    let payload = list_sessions_payload(
        &state,
        "default",
        false,
        None,
        20,
        &SessionFilterCriteria::default(),
    )
    .await
    .unwrap();
    let first = payload
        .get("sessions")
        .and_then(Value::as_array)
        .and_then(|sessions| sessions.first())
        .cloned()
        .expect("expected seeded session");
    assert_eq!(first.get("id").and_then(Value::as_str), Some("thread-1"));
    assert_eq!(
        first.get("status").and_then(Value::as_str),
        Some("completed")
    );
    assert!(
        state
            .active_turns
            .lock()
            .await
            .get(&runtime_session_key("default", "thread-1"))
            .is_none()
    );
    assert!(
        !state
            .pending_turn_starts
            .lock()
            .await
            .contains(&runtime_session_key("default", "thread-1"))
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn older_turn_completion_does_not_override_newer_active_turn() {
    let sandbox = unique_test_dir("session-list-active-turn-wins");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": "thread-1",
                    "name": "Status sync",
                    "preview": "",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 2,
                    "status": "running",
                    "isSubagent": false,
                    "agentNickname": Value::Null,
                    "agentRole": Value::Null,
                    "turns": [
                        {
                            "id": "turn-1",
                            "status": "inProgress",
                            "error": Value::Null,
                            "startedAt": 10,
                            "completedAt": Value::Null,
                            "durationMs": Value::Null,
                            "items": []
                        },
                        {
                            "id": "turn-2",
                            "status": "inProgress",
                            "error": Value::Null,
                            "startedAt": 20,
                            "completedAt": Value::Null,
                            "durationMs": Value::Null,
                            "items": []
                        }
                    ]
                }
            }),
        )
        .await
        .unwrap();
    state.active_turns.lock().await.insert(
        runtime_session_key("default", "thread-1"),
        "turn-2".to_string(),
    );

    handle_profile_runtime_notification(
        &state,
        "default",
        &AppServerNotification {
            method: "turn/completed".to_string(),
            params: json!({
                "threadId": "thread-1",
                "turnId": "turn-1",
                "turn": {
                    "id": "turn-1",
                    "status": "completed",
                    "items": []
                }
            }),
        },
    )
    .await;

    assert_eq!(
        state
            .active_turns
            .lock()
            .await
            .get(&runtime_session_key("default", "thread-1"))
            .cloned(),
        Some("turn-2".to_string())
    );

    let payload = list_sessions_payload(
        &state,
        "default",
        false,
        None,
        20,
        &SessionFilterCriteria::default(),
    )
    .await
    .unwrap();
    let first = payload
        .get("sessions")
        .and_then(Value::as_array)
        .and_then(|sessions| sessions.first())
        .cloned()
        .expect("expected seeded session");
    assert_eq!(first.get("id").and_then(Value::as_str), Some("thread-1"));
    assert_eq!(first.get("status").and_then(Value::as_str), Some("running"));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_list_clears_stale_cached_active_turn_when_thread_completed() {
    let sandbox = unique_test_dir("session-list-stale-active-completed");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": "thread-stale-active-completed",
                    "name": "Stale active completed",
                    "preview": "",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 20,
                    "status": "completed",
                    "isSubagent": false,
                    "agentNickname": Value::Null,
                    "agentRole": Value::Null,
                    "turns": [
                        {
                            "id": "turn-1",
                            "status": "completed",
                            "error": Value::Null,
                            "startedAt": 10,
                            "completedAt": 20,
                            "durationMs": 10,
                            "items": []
                        }
                    ]
                }
            }),
        )
        .await
        .unwrap();
    state.active_turns.lock().await.insert(
        runtime_session_key("default", "thread-stale-active-completed"),
        "turn-1".to_string(),
    );
    with_ui_state_write(&state, "default", |ui_state| {
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
            "thread-stale-active-completed".to_string(),
            json!({
                "status": "running",
                "updatedAt": now_unix_ms().saturating_sub(10_000)
            }),
        );
        Ok(())
    })
    .await
    .unwrap();

    let payload = list_sessions_payload(
        &state,
        "default",
        false,
        None,
        20,
        &SessionFilterCriteria::default(),
    )
    .await
    .unwrap();
    let first = payload
        .get("sessions")
        .and_then(Value::as_array)
        .and_then(|sessions| sessions.first())
        .cloned()
        .expect("expected seeded session");
    assert_eq!(
        first.get("id").and_then(Value::as_str),
        Some("thread-stale-active-completed")
    );
    assert_eq!(
        first.get("status").and_then(Value::as_str),
        Some("completed")
    );
    assert!(
        !state
            .active_turns
            .lock()
            .await
            .contains_key(&runtime_session_key(
                "default",
                "thread-stale-active-completed"
            ))
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_list_keeps_stale_cached_active_turn_when_thread_still_active() {
    let sandbox = unique_test_dir("session-list-stale-active-running");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": "thread-stale-active-running",
                    "name": "Stale active running",
                    "preview": "",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 20,
                    "status": "running",
                    "isSubagent": false,
                    "agentNickname": Value::Null,
                    "agentRole": Value::Null,
                    "turns": [
                        {
                            "id": "turn-1",
                            "status": "inProgress",
                            "error": Value::Null,
                            "startedAt": 10,
                            "completedAt": Value::Null,
                            "durationMs": Value::Null,
                            "items": []
                        }
                    ]
                }
            }),
        )
        .await
        .unwrap();
    state.active_turns.lock().await.insert(
        runtime_session_key("default", "thread-stale-active-running"),
        "turn-1".to_string(),
    );
    with_ui_state_write(&state, "default", |ui_state| {
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
            "thread-stale-active-running".to_string(),
            json!({
                "status": "running",
                "updatedAt": now_unix_ms().saturating_sub(10_000)
            }),
        );
        Ok(())
    })
    .await
    .unwrap();

    let payload = list_sessions_payload(
        &state,
        "default",
        false,
        None,
        20,
        &SessionFilterCriteria::default(),
    )
    .await
    .unwrap();
    let first = payload
        .get("sessions")
        .and_then(Value::as_array)
        .and_then(|sessions| sessions.first())
        .cloned()
        .expect("expected seeded session");
    assert_eq!(
        first.get("id").and_then(Value::as_str),
        Some("thread-stale-active-running")
    );
    assert_eq!(first.get("status").and_then(Value::as_str), Some("running"));
    assert!(
        state
            .active_turns
            .lock()
            .await
            .contains_key(&runtime_session_key(
                "default",
                "thread-stale-active-running"
            ))
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_list_prefers_newer_thread_status_over_older_runtime_completion_override() {
    let sandbox = unique_test_dir("session-list-newer-thread-status");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": "thread-1",
                    "name": "Status sync",
                    "preview": "",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 50,
                    "status": "running",
                    "isSubagent": false,
                    "agentNickname": Value::Null,
                    "agentRole": Value::Null,
                    "turns": []
                }
            }),
        )
        .await
        .unwrap();
    with_ui_state_write(&state, "default", |ui_state| {
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
            "thread-1".to_string(),
            json!({
                "status": "completed",
                "updatedAt": 10
            }),
        );
        Ok(())
    })
    .await
    .unwrap();

    let payload = list_sessions_payload(
        &state,
        "default",
        false,
        None,
        20,
        &SessionFilterCriteria::default(),
    )
    .await
    .unwrap();
    let first = payload
        .get("sessions")
        .and_then(Value::as_array)
        .and_then(|sessions| sessions.first())
        .cloned()
        .expect("expected seeded session");
    assert_eq!(first.get("id").and_then(Value::as_str), Some("thread-1"));
    assert_eq!(first.get("status").and_then(Value::as_str), Some("running"));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_list_does_not_block_on_stale_runtime_status_reconciliation() {
    let sandbox = unique_test_dir("session-list-stale-reconcile-timeout");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": "thread-slow-stale",
                    "name": "Slow stale status",
                    "preview": "",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 10,
                    "status": "running",
                    "readDelayMs": 2_000,
                    "isSubagent": false,
                    "agentNickname": Value::Null,
                    "agentRole": Value::Null,
                    "turns": []
                }
            }),
        )
        .await
        .unwrap();
    with_ui_state_write(&state, "default", |ui_state| {
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
            "thread-slow-stale".to_string(),
            json!({
                "status": "running",
                "updatedAt": now_unix_ms().saturating_sub(60_000)
            }),
        );
        Ok(())
    })
    .await
    .unwrap();

    let started_at = Instant::now();
    let payload = list_sessions_payload(
        &state,
        "default",
        false,
        None,
        20,
        &SessionFilterCriteria::default(),
    )
    .await
    .unwrap();

    assert!(
        started_at.elapsed() < Duration::from_secs(1),
        "session list should not wait for slow stale status reconciliation"
    );
    let first = payload
        .get("sessions")
        .and_then(Value::as_array)
        .and_then(|sessions| sessions.first())
        .cloned()
        .expect("expected seeded session");
    assert_eq!(
        first.get("id").and_then(Value::as_str),
        Some("thread-slow-stale")
    );
    assert_eq!(
        first.get("status").and_then(Value::as_str),
        Some("completed")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_notifications_emit_session_stream_events_from_rust_relay() {
    let sandbox = unique_test_dir("session-stream-rust");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let relay = ensure_stream_relay(&state, "default", "thread-1")
        .await
        .expect("relay should initialize");
    let mut receiver = relay.subscribe();

    handle_profile_runtime_notification(
        &state,
        "default",
        &AppServerNotification {
            method: "item/started".to_string(),
            params: json!({
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "item-1",
                "item": {
                    "id": "item-1",
                    "type": "commandExecution",
                    "command": ["sed", "-n", "1,20p", "src/main.rs"]
                }
            }),
        },
    )
    .await;

    let item_started = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
        .await
        .expect("item event should arrive")
        .expect("item event should be readable");
    assert_eq!(
        item_started.get("method").and_then(Value::as_str),
        Some("item/started")
    );
    assert_eq!(
        item_started
            .get("params")
            .and_then(|value| value.get("item"))
            .and_then(|value| value.get("title"))
            .and_then(Value::as_str),
        Some("Command")
    );

    handle_profile_runtime_notification(
        &state,
        "default",
        &AppServerNotification {
            method: "item/commandExecution/outputDelta".to_string(),
            params: json!({
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "item-1",
                "delta": "hello"
            }),
        },
    )
    .await;

    let command_delta = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
        .await
        .expect("command delta should arrive")
        .expect("command delta should be readable");
    assert_eq!(
        command_delta
            .get("params")
            .and_then(|value| value.get("deltaLength"))
            .and_then(Value::as_u64),
        Some(5)
    );

    handle_profile_runtime_notification(
        &state,
        "default",
        &AppServerNotification {
            method: "thread/status/changed".to_string(),
            params: json!({
                "threadId": "thread-1",
                "status": { "type": "completed" }
            }),
        },
    )
    .await;

    let status_changed = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
        .await
        .expect("status event should arrive")
        .expect("status event should be readable");
    assert_eq!(
        status_changed
            .get("params")
            .and_then(|value| value.get("status"))
            .and_then(Value::as_str),
        Some("completed")
    );

    handle_profile_runtime_notification(
        &state,
        "default",
        &AppServerNotification {
            method: "thread/tokenUsage/updated".to_string(),
            params: json!({
                "threadId": "thread-1",
                "tokenUsage": {
                    "total": {
                        "totalTokens": 15,
                        "inputTokens": 7,
                        "cachedInputTokens": 1,
                        "outputTokens": 8,
                        "reasoningOutputTokens": 2
                    },
                    "last": {
                        "totalTokens": 10,
                        "inputTokens": 4,
                        "cachedInputTokens": 1,
                        "outputTokens": 6,
                        "reasoningOutputTokens": 1
                    },
                    "modelContextWindow": 2000
                }
            }),
        },
    )
    .await;

    let token_usage = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
        .await
        .expect("token usage event should arrive")
        .expect("token usage event should be readable");
    assert_eq!(
        token_usage
            .get("params")
            .and_then(|value| value.get("tokenUsage"))
            .and_then(|value| value.get("total"))
            .and_then(|value| value.get("totalTokens"))
            .and_then(Value::as_u64),
        Some(15)
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn app_server_start_enables_goals_by_default() {
    let sandbox = unique_test_dir("app-server-goals-default");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let count = app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "debug/requestCount",
            json!({
                "target": "config/batchWrite"
            }),
        )
        .await
        .unwrap();

    assert_eq!(count.get("count").and_then(Value::as_u64), Some(1));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_goal_payload_round_trips_through_app_server() {
    let sandbox = unique_test_dir("thread-goal");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": "thread-goal-1",
                    "name": "Goal test",
                    "preview": "",
                    "cwd": workspace,
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 1,
                    "status": "idle",
                    "isSubagent": false,
                    "turns": []
                }
            }),
        )
        .await
        .unwrap();

    let set = set_session_goal_payload(
        &state,
        "default",
        "thread-goal-1",
        json!({
            "objective": "ship upstream goal parity",
            "tokenBudget": 12000
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        set.get("goal")
            .and_then(|goal| goal.get("objective"))
            .and_then(Value::as_str),
        Some("ship upstream goal parity")
    );
    assert_eq!(
        set.get("goal")
            .and_then(|goal| goal.get("status"))
            .and_then(Value::as_str),
        Some("active")
    );

    let paused = set_session_goal_payload(
        &state,
        "default",
        "thread-goal-1",
        json!({
            "status": "paused"
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        paused
            .get("goal")
            .and_then(|goal| goal.get("status"))
            .and_then(Value::as_str),
        Some("paused")
    );

    let detail = session_detail_payload(&state, "default", "thread-goal-1", 20)
        .await
        .unwrap();
    assert_eq!(
        detail
            .get("goal")
            .and_then(|goal| goal.get("objective"))
            .and_then(Value::as_str),
        Some("ship upstream goal parity")
    );

    let cleared = clear_session_goal_payload(&state, "default", "thread-goal-1")
        .await
        .unwrap();
    assert_eq!(cleared.get("cleared").and_then(Value::as_bool), Some(true));
    let get = get_session_goal_payload(&state, "default", "thread-goal-1")
        .await
        .unwrap();
    assert!(get.get("goal").is_some_and(Value::is_null));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_goal_request_reloads_config_when_goals_are_disabled() {
    let sandbox = unique_test_dir("thread-goal-reenable");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let client = app_server_client(&state, "default").await.unwrap();
    client
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": "thread-goal-disabled",
                    "name": "Goal disabled retry",
                    "preview": "",
                    "cwd": workspace,
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 1,
                    "status": "idle",
                    "isSubagent": false,
                    "turns": []
                }
            }),
        )
        .await
        .unwrap();
    client
        .request("debug/setGoalsEnabled", json!({ "enabled": false }))
        .await
        .unwrap();

    let set = set_session_goal_payload(
        &state,
        "default",
        "thread-goal-disabled",
        json!({
            "objective": "retry goal enablement"
        }),
    )
    .await
    .unwrap();

    assert_eq!(
        set.get("goal")
            .and_then(|goal| goal.get("objective"))
            .and_then(Value::as_str),
        Some("retry goal enablement")
    );
    let count = client
        .request(
            "debug/requestCount",
            json!({
                "target": "config/batchWrite"
            }),
        )
        .await
        .unwrap();
    assert_eq!(count.get("count").and_then(Value::as_u64), Some(2));

    let _ = fs::remove_dir_all(sandbox);
}

#[test]
fn thread_goal_notifications_are_mapped_for_session_streams() {
    let event = map_app_server_session_notification(&AppServerNotification {
        method: "thread/goal/updated".to_string(),
        params: json!({
            "threadId": "thread-goal-2",
            "turnId": "turn-1",
            "goal": {
                "threadId": "thread-goal-2",
                "objective": "finish compatibility work",
                "status": "budget_limited",
                "token_budget": 2000,
                "tokens_used": 1500,
                "time_used_seconds": 90,
                "created_at": 10,
                "updated_at": 20
            }
        }),
    })
    .unwrap();
    assert_eq!(
        event
            .get("params")
            .and_then(|params| params.get("goal"))
            .and_then(|goal| goal.get("status"))
            .and_then(Value::as_str),
        Some("budgetLimited")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_notifications_emit_global_events_without_internal_sse() {
    let sandbox = unique_test_dir("global-stream-rust");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let relay = ensure_global_relay(&state, "default")
        .await
        .expect("global relay should initialize");
    let mut receiver = relay.subscribe();

    handle_profile_runtime_notification(
        &state,
        "default",
        &AppServerNotification {
            method: "turn/completed".to_string(),
            params: json!({
                "threadId": "thread-1",
                "turnId": "turn-1",
                "turn": {
                    "id": "turn-1",
                    "status": "completed",
                    "items": []
                }
            }),
        },
    )
    .await;

    let mut saw_completion_attention = false;
    for _ in 0..6 {
        let event = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("completion event should arrive")
            .expect("completion event should be readable");
        if event.get("method").and_then(Value::as_str) == Some("codex-webui/sessionAttention")
            && event
                .get("params")
                .and_then(|value| value.get("reason"))
                .and_then(Value::as_str)
                == Some("completed")
        {
            saw_completion_attention = true;
            break;
        }
    }
    assert!(saw_completion_attention);

    handle_profile_runtime_notification(
        &state,
        "default",
        &AppServerNotification {
            method: "thread/archived".to_string(),
            params: json!({
                "threadId": "thread-1"
            }),
        },
    )
    .await;

    let mut saw_invalidation = false;
    for _ in 0..6 {
        let event = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("invalidation event should arrive")
            .expect("invalidation event should be readable");
        if event.get("method").and_then(Value::as_str)
            == Some("codex-webui/sessionListsInvalidated")
        {
            saw_invalidation = true;
            break;
        }
    }
    assert!(saw_invalidation);

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completed_runtime_notification_skips_highlight_when_session_is_open() {
    let sandbox = unique_test_dir("completion-highlight-open-session");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let relay = ensure_stream_relay(&state, "default", "thread-1")
        .await
        .expect("session relay should initialize");
    let _receiver = relay.subscribe();

    handle_profile_runtime_notification(
        &state,
        "default",
        &AppServerNotification {
            method: "turn/completed".to_string(),
            params: json!({
                "threadId": "thread-1",
                "turnId": "turn-1",
                "turn": {
                    "id": "turn-1",
                    "status": "completed",
                    "items": []
                }
            }),
        },
    )
    .await;

    let highlight = with_ui_state_read(&state, "default", |ui_state| {
        Ok(ui_state
            .get("highlightsByThreadId")
            .and_then(Value::as_object)
            .and_then(|entries| entries.get("thread-1"))
            .cloned()
            .unwrap_or(Value::Null))
    })
    .await
    .unwrap();
    assert!(highlight.is_null());

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn arena_list_falls_back_to_stored_runs_when_sessions_cannot_be_loaded() {
    let sandbox = unique_test_dir("arena-list");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    let arena_path = arena_store_path(&state.config, "default");
    fs::create_dir_all(arena_path.parent().unwrap()).unwrap();
    fs::write(
        &arena_path,
        serde_json::to_vec_pretty(&ArenaStoreState {
            runs: vec![ArenaRunRecord {
                id: "arena-1".to_string(),
                prompt: "compare models".to_string(),
                cwd: "/tmp/project".to_string(),
                status: "running".to_string(),
                created_at: 100,
                updated_at: 110,
                contestants: vec![ArenaContestantRecord {
                    id: "contestant-1".to_string(),
                    session_id: "session-1".to_string(),
                    model: "gpt-5.4".to_string(),
                    label: "Primary".to_string(),
                    status: "running".to_string(),
                    response: None,
                    created_at: 100,
                    updated_at: 110,
                }],
            }],
        })
        .unwrap(),
    )
    .unwrap();

    let payload = list_arena_runs_payload(&state, "default").await.unwrap();
    let first_run = payload
        .get("runs")
        .and_then(Value::as_array)
        .and_then(|runs| runs.first())
        .cloned()
        .unwrap();
    assert_eq!(first_run.get("id").and_then(Value::as_str), Some("arena-1"));
    assert_eq!(
        first_run.get("status").and_then(Value::as_str),
        Some("running")
    );
    assert_eq!(
        first_run
            .get("contestants")
            .and_then(Value::as_array)
            .and_then(|contestants| contestants.first())
            .and_then(|contestant| contestant.get("sessionId"))
            .and_then(Value::as_str),
        Some("session-1")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn saves_drafts_and_reads_queue_payloads() {
    let sandbox = unique_test_dir("draft-queue");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    let ui_state_path = profile_ui_state_path(&state.config, "default");
    fs::create_dir_all(ui_state_path.parent().unwrap()).unwrap();
    fs::write(
        &ui_state_path,
        serde_json::to_vec_pretty(&json!({
            "global": {
                "shutdownAfterQueueCompletes": false,
                "scheduledShutdown": Value::Null
            },
            "notifications": {
                "items": [],
                "settings": default_notification_settings_value()
            },
            "sessionMetaByThreadId": {},
            "savedSessionFilters": [],
            "promptPresets": [],
            "automations": [],
            "automationRuns": [],
            "preferencesByThreadId": {},
            "draftsByThreadId": {},
            "queuesByThreadId": {
                "thread-1": {
                    "items": [
                        {
                            "id": "queue-1",
                            "prompt": "follow up",
                            "attachmentIds": ["att-1"],
                            "attachmentNames": ["notes.txt"],
                            "createdAt": 15
                        }
                    ],
                    "resumePending": true,
                    "updatedAt": 20
                }
            },
            "highlightsByThreadId": {}
        }))
        .unwrap(),
    )
    .unwrap();

    let saved = save_session_draft_payload(&state, "default", "thread-1", "Draft message", "queue")
        .await
        .unwrap();
    assert_eq!(
        saved.get("draft").and_then(Value::as_str),
        Some("Draft message")
    );
    assert_eq!(saved.get("intent").and_then(Value::as_str), Some("queue"));

    let cleared = clear_session_draft_payload(&state, "default", "thread-1")
        .await
        .unwrap();
    assert_eq!(cleared.get("draft").and_then(Value::as_str), Some(""));
    assert!(cleared.get("intent").is_some_and(Value::is_null));

    let queue = get_session_queue_payload(&state, "default", "thread-1")
        .await
        .unwrap();
    assert_eq!(
        queue.get("resumeRequired").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        queue
            .get("items")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|item| item.get("attachmentNames"))
            .and_then(Value::as_array)
            .and_then(|names| names.first())
            .and_then(Value::as_str),
        Some("notes.txt")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn queue_write_helpers_mutate_queue_state() {
    let sandbox = unique_test_dir("queue-write-helpers");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state(workspace.clone(), vec![workspace], codex_home);

    let queue_skills = json!([
        {
            "id": "skill-queue",
            "name": "openai-docs",
            "path": "/skills/openai-docs/SKILL.md"
        }
    ]);
    let first = enqueue_session_queue_payload(
        &state,
        "default",
        "thread-1",
        "first",
        Some("request-first"),
        Some(&queue_skills),
        None,
    )
    .await
    .unwrap();
    let first_id = first
        .get("enqueueItemId")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();
    let duplicate_first = enqueue_session_queue_payload(
        &state,
        "default",
        "thread-1",
        "first",
        Some("request-first"),
        Some(&queue_skills),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        duplicate_first.get("enqueueItemId").and_then(Value::as_str),
        Some(first_id.as_str())
    );
    assert_eq!(
        duplicate_first
            .get("items")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );
    let repeated_first = enqueue_session_queue_payload(
        &state,
        "default",
        "thread-1",
        "first",
        Some("request-first-repeat"),
        Some(&queue_skills),
        None,
    )
    .await
    .unwrap();
    let repeated_first_id = repeated_first
        .get("enqueueItemId")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();
    assert_ne!(Some(repeated_first_id.as_str()), Some(first_id.as_str()));
    assert_eq!(
        repeated_first
            .get("items")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(2)
    );
    let second =
        enqueue_session_queue_payload(&state, "default", "thread-1", "second", None, None, None)
            .await
            .unwrap();
    let second_id = second
        .get("enqueueItemId")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();

    let reordered = reorder_session_queue_payload(
        &state,
        "default",
        "thread-1",
        &[
            second_id.clone(),
            repeated_first_id.clone(),
            first_id.clone(),
        ],
    )
    .await
    .unwrap();
    assert_eq!(
        reordered
            .get("items")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|item| item.get("id"))
            .and_then(Value::as_str),
        Some(second_id.as_str())
    );

    let empty_attachments = json!([]);
    let updated = update_session_queue_item_payload(
        &state,
        "default",
        "thread-1",
        &first_id,
        Some("first updated"),
        Some(&queue_skills),
        Some(&empty_attachments),
    )
    .await
    .unwrap();
    let updated_item = updated
        .get("items")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.get("id").and_then(Value::as_str) == Some(first_id.as_str()))
        })
        .cloned()
        .unwrap();
    assert_eq!(
        updated_item.get("prompt").and_then(Value::as_str),
        Some("first updated")
    );
    assert_eq!(
        updated_item
            .get("skills")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );

    let removed = remove_session_queue_item_payload(&state, "default", "thread-1", &second_id)
        .await
        .unwrap();
    assert_eq!(
        removed.get("items").and_then(Value::as_array).map(Vec::len),
        Some(2)
    );
    let removed_dispatched_first =
        remove_session_queue_item_after_dispatch(&state, "default", "thread-1", &first_id)
            .await
            .unwrap();
    assert_eq!(
        removed_dispatched_first
            .get("items")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|item| item.get("id"))
            .and_then(Value::as_str),
        Some(repeated_first_id.as_str())
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enqueue_session_queue_payload_auto_dispatches_when_session_is_idle() {
    let sandbox = unique_test_dir("queue-auto-dispatch");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let created = create_session_payload(
        &state,
        "default",
        json!({
            "cwd": workspace.display().to_string(),
            "model": "gpt-5.4"
        }),
        None,
        Some("Queue auto dispatch"),
    )
    .await
    .unwrap();
    let session_id = created
        .get("id")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();

    let queued = enqueue_session_queue_payload(
        &state,
        "default",
        &session_id,
        "Continue the work after the browser disconnects.",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(
        queued.get("enqueueAccepted").and_then(Value::as_bool),
        Some(true)
    );
    assert!(
        queued
            .get("enqueueItemId")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
    );

    for _ in 0..20 {
        let queue = get_session_queue_payload(&state, "default", &session_id)
            .await
            .unwrap();
        if queue
            .get("items")
            .and_then(Value::as_array)
            .is_none_or(|items| items.is_empty())
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let queue_after = get_session_queue_payload(&state, "default", &session_id)
        .await
        .unwrap();
    assert_eq!(
        queue_after
            .get("items")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );

    let thread = read_thread_payload(&state, "default", &session_id, true)
        .await
        .unwrap();
    assert_eq!(
        thread.get("status").and_then(Value::as_str),
        Some("running")
    );
    let last_turn_start = thread.get("lastTurnStart").cloned().unwrap_or(Value::Null);
    let input = last_turn_start
        .get("input")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        input
            .first()
            .and_then(|value| value.get("text"))
            .and_then(Value::as_str),
        Some("Continue the work after the browser disconnects.")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enqueue_session_queue_payload_returns_before_queue_drain_reads_thread() {
    let sandbox = unique_test_dir("queue-nonblocking-enqueue");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let session_id = "thread-slow-read";
    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": session_id,
                    "name": "Slow queue drain",
                    "preview": "",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 1,
                    "status": "idle",
                    "isSubagent": false,
                    "agentNickname": Value::Null,
                    "agentRole": Value::Null,
                    "readDelayMs": 900,
                    "turns": []
                }
            }),
        )
        .await
        .unwrap();

    let started = Instant::now();
    let queue = enqueue_session_queue_payload(
        &state,
        "default",
        session_id,
        "Queue should not wait for a slow thread read.",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    assert!(
        started.elapsed() < Duration::from_millis(250),
        "enqueue waited for queue drain: {:?}",
        started.elapsed()
    );
    assert_eq!(
        queue.get("items").and_then(Value::as_array).map(Vec::len),
        Some(1)
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queue_drain_waits_while_turn_start_is_pending() {
    let sandbox = unique_test_dir("queue-pending-turn-start");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let session_id = "thread-pending-start";
    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": session_id,
                    "name": "Pending turn start",
                    "preview": "",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 1,
                    "status": "idle",
                    "isSubagent": false,
                    "agentNickname": Value::Null,
                    "agentRole": Value::Null,
                    "turns": []
                }
            }),
        )
        .await
        .unwrap();
    state
        .pending_turn_starts
        .lock()
        .await
        .insert(runtime_session_key("default", session_id));

    enqueue_session_queue_payload(
        &state,
        "default",
        session_id,
        "Do not dispatch until the in-flight turn start resolves.",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(350)).await;

    let queue = get_session_queue_payload(&state, "default", session_id)
        .await
        .unwrap();
    assert_eq!(
        queue.get("items").and_then(Value::as_array).map(Vec::len),
        Some(1)
    );

    state
        .pending_turn_starts
        .lock()
        .await
        .remove(&runtime_session_key("default", session_id));
    maybe_drain_queue(&state, "default", session_id).await;
    let drained = get_session_queue_payload(&state, "default", session_id)
        .await
        .unwrap();
    assert_eq!(
        drained.get("items").and_then(Value::as_array).map(Vec::len),
        Some(0)
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queue_drain_waits_while_cached_active_turn_is_recent() {
    let sandbox = unique_test_dir("queue-recent-active-turn");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let session_id = "thread-recent-active";
    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": session_id,
                    "name": "Recent active turn",
                    "preview": "",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 1,
                    "status": "running",
                    "isSubagent": false,
                    "agentNickname": Value::Null,
                    "agentRole": Value::Null,
                    "turns": []
                }
            }),
        )
        .await
        .unwrap();
    state.active_turns.lock().await.insert(
        runtime_session_key("default", session_id),
        "turn-still-settling".to_string(),
    );
    with_ui_state_write(&state, "default", |ui_state| {
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
                "status": "running",
                "updatedAt": now_unix_ms()
            }),
        );
        Ok(())
    })
    .await
    .unwrap();

    enqueue_session_queue_payload(
        &state,
        "default",
        session_id,
        "Do not dispatch while the active turn may still be settling.",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(350)).await;

    let queue = get_session_queue_payload(&state, "default", session_id)
        .await
        .unwrap();
    assert_eq!(
        queue.get("items").and_then(Value::as_array).map(Vec::len),
        Some(1)
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queue_drain_clears_stale_cached_active_turn_and_dispatches() {
    let sandbox = unique_test_dir("queue-stale-active-turn");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let session_id = "thread-stale-active";
    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": session_id,
                    "name": "Stale active turn",
                    "preview": "",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 10,
                    "status": "running",
                    "isSubagent": false,
                    "agentNickname": Value::Null,
                    "agentRole": Value::Null,
                    "turns": []
                }
            }),
        )
        .await
        .unwrap();
    state.active_turns.lock().await.insert(
        runtime_session_key("default", session_id),
        "turn-missed-completion".to_string(),
    );
    with_ui_state_write(&state, "default", |ui_state| {
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
                "status": "running",
                "updatedAt": now_unix_ms().saturating_sub(6_000)
            }),
        );
        Ok(())
    })
    .await
    .unwrap();

    enqueue_session_queue_payload(
        &state,
        "default",
        session_id,
        "Dispatch after stale active cache is reconciled.",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    for _ in 0..20 {
        let queue = get_session_queue_payload(&state, "default", session_id)
            .await
            .unwrap();
        if queue
            .get("items")
            .and_then(Value::as_array)
            .is_none_or(|items| items.is_empty())
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let queue_after = get_session_queue_payload(&state, "default", session_id)
        .await
        .unwrap();
    assert_eq!(
        queue_after
            .get("items")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );
    let thread = read_thread_payload(&state, "default", session_id, true)
        .await
        .unwrap();
    assert_eq!(
        thread
            .get("lastTurnStart")
            .and_then(|value| value.get("input"))
            .and_then(Value::as_array)
            .and_then(|input| input.first())
            .and_then(|value| value.get("text"))
            .and_then(Value::as_str),
        Some("Dispatch after stale active cache is reconciled.")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_message_retries_after_transient_thread_read_error() {
    let sandbox = unique_test_dir("queue-auto-dispatch-retry");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let session_id = "thread-transient-read-error";
    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": session_id,
                    "name": "Retry queue dispatch",
                    "preview": "",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 1,
                    "status": "idle",
                    "isSubagent": false,
                    "agentNickname": Value::Null,
                    "agentRole": Value::Null,
                    "readError": "rollout is settling",
                    "turns": []
                }
            }),
        )
        .await
        .unwrap();

    enqueue_session_queue_payload(
        &state,
        "default",
        session_id,
        "Dispatch after the transient read failure clears.",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let queued_before_retry = get_session_queue_payload(&state, "default", session_id)
        .await
        .unwrap();
    assert_eq!(
        queued_before_retry
            .get("items")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );

    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": session_id,
                    "name": "Retry queue dispatch",
                    "preview": "",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 2,
                    "status": "idle",
                    "isSubagent": false,
                    "agentNickname": Value::Null,
                    "agentRole": Value::Null,
                    "turns": []
                }
            }),
        )
        .await
        .unwrap();

    for _ in 0..20 {
        let queue = get_session_queue_payload(&state, "default", session_id)
            .await
            .unwrap();
        if queue
            .get("items")
            .and_then(Value::as_array)
            .is_none_or(|items| items.is_empty())
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let queue_after_retry = get_session_queue_payload(&state, "default", session_id)
        .await
        .unwrap();
    assert_eq!(
        queue_after_retry
            .get("items")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );

    let thread = read_thread_payload(&state, "default", session_id, true)
        .await
        .unwrap();
    assert_eq!(
        thread.get("turns").and_then(Value::as_array).map(Vec::len),
        Some(1)
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn marks_resume_pending_queues_and_lists_paused_entries() {
    let sandbox = unique_test_dir("queue-resume-pending");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    let ui_state_path = profile_ui_state_path(&state.config, "default");
    fs::create_dir_all(ui_state_path.parent().unwrap()).unwrap();
    fs::write(
        &ui_state_path,
        serde_json::to_vec_pretty(&json!({
            "global": {
                "shutdownAfterQueueCompletes": false,
                "scheduledShutdown": Value::Null
            },
            "notifications": {
                "items": [],
                "settings": default_notification_settings_value()
            },
            "sessionMetaByThreadId": {},
            "savedSessionFilters": [],
            "promptPresets": [],
            "automations": [],
            "automationRuns": [],
            "preferencesByThreadId": {
                "thread-1": {
                    "cwd": "/tmp/project"
                }
            },
            "draftsByThreadId": {},
            "queuesByThreadId": {
                "thread-1": {
                    "items": [
                        {
                            "id": "queue-1",
                            "prompt": "follow up",
                            "attachmentIds": [],
                            "attachmentNames": [],
                            "createdAt": 15
                        }
                    ],
                    "resumePending": false,
                    "updatedAt": 20
                }
            },
            "highlightsByThreadId": {}
        }))
        .unwrap(),
    )
    .unwrap();

    assert!(
        mark_queues_pending_resume_payload(&state, "default")
            .await
            .unwrap()
    );
    let paused = list_resume_pending_queues_payload(&state, "default")
        .await
        .unwrap();
    let first = paused
        .as_array()
        .and_then(|items| items.first())
        .cloned()
        .unwrap();
    assert_eq!(
        first.get("sessionId").and_then(Value::as_str),
        Some("thread-1")
    );
    assert_eq!(first.get("pendingCount").and_then(Value::as_u64), Some(1));
    assert_eq!(
        first.get("cwd").and_then(Value::as_str),
        Some("/tmp/project")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[test]
fn catalog_builder_discovers_plugins_and_skills() {
    let sandbox = unique_test_dir("catalog");
    let codex_home = sandbox.join(".codex");
    let local_skill_dir = codex_home.join("skills").join("my-skill");
    let system_skill_dir = codex_home.join("skills").join(".system").join("sys-skill");
    let plugin_base = codex_home.join("plugins").join("sample-plugin");
    let plugin_skill_dir = plugin_base.join("skills").join("plugin-skill");

    fs::create_dir_all(&local_skill_dir).unwrap();
    fs::create_dir_all(&system_skill_dir).unwrap();
    fs::create_dir_all(plugin_base.join(".codex-plugin")).unwrap();
    fs::create_dir_all(&plugin_skill_dir).unwrap();

    fs::write(
        local_skill_dir.join("SKILL.md"),
        "---\nname: Local Skill\ndescription: Local description\n---\nbody\n",
    )
    .unwrap();
    fs::write(
        system_skill_dir.join("SKILL.md"),
        "---\nname: System Skill\ndescription: System description\n---\nbody\n",
    )
    .unwrap();
    fs::write(
        plugin_skill_dir.join("SKILL.md"),
        "---\nname: Plugin Skill\ndescription: Plugin description\n---\nbody\n",
    )
    .unwrap();
    fs::write(
        plugin_base.join(".codex-plugin").join("plugin.json"),
        serde_json::to_vec_pretty(&json!({
            "name": "sample-plugin",
            "description": "Plugin description",
            "version": "1.2.3",
            "skills": "skills",
            "interface": {
                "displayName": "Sample Plugin",
                "developerName": "Codex Web UI",
                "category": "tools"
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let payload = build_catalog_payload_for_codex_home(&codex_home);
    let skills = payload
        .get("skills")
        .and_then(Value::as_array)
        .cloned()
        .unwrap();
    let plugins = payload
        .get("plugins")
        .and_then(Value::as_array)
        .cloned()
        .unwrap();

    assert!(skills.iter().any(|entry| {
        entry.get("name").and_then(Value::as_str) == Some("Local Skill")
            && entry.get("source").and_then(Value::as_str) == Some("local")
    }));
    assert!(skills.iter().any(|entry| {
        entry.get("name").and_then(Value::as_str) == Some("System Skill")
            && entry.get("source").and_then(Value::as_str) == Some("system")
    }));
    assert!(skills.iter().any(|entry| {
        entry.get("name").and_then(Value::as_str) == Some("Plugin Skill")
            && entry.get("pluginName").and_then(Value::as_str) == Some("sample-plugin")
    }));
    assert!(plugins.iter().any(|entry| {
        entry.get("displayName").and_then(Value::as_str) == Some("Sample Plugin")
            && entry
                .get("skills")
                .and_then(Value::as_array)
                .is_some_and(|skills| skills.contains(&json!("plugin-skill")))
    }));

    let _ = fs::remove_dir_all(sandbox);
}
