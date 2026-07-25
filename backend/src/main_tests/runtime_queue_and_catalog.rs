use super::*;

async fn mark_test_session_active(state: &AppState, session_id: &str) {
    state.active_turns.lock().await.insert(
        runtime_session_key("default", session_id),
        "turn-1".to_string(),
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn archive_and_unarchive_use_native_thread_state() {
    let sandbox = unique_test_dir("native-archive");
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
                    "id": "thread-archive-native",
                    "name": "Archive native",
                    "preview": "archive me",
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

    let archived = archive_session_payload(&state, "default", "thread-archive-native")
        .await
        .unwrap();
    assert_eq!(archived.get("ok").and_then(Value::as_bool), Some(true));
    let archived_thread = client
        .request(
            "thread/read",
            json!({ "threadId": "thread-archive-native" }),
        )
        .await
        .unwrap();
    assert_eq!(
        archived_thread
            .get("thread")
            .and_then(|thread| thread.get("archived"))
            .and_then(Value::as_bool),
        Some(true)
    );
    let filter = session_filter_from_value(None);
    let archived_list = list_sessions_payload(&state, "default", true, None, 20, &filter)
        .await
        .unwrap();
    assert_eq!(
        archived_list
            .get("sessions")
            .and_then(Value::as_array)
            .and_then(|sessions| sessions.first())
            .and_then(|session| session.get("id"))
            .and_then(Value::as_str),
        Some("thread-archive-native")
    );

    let unarchived = unarchive_session_payload(&state, "default", "thread-archive-native")
        .await
        .unwrap();
    assert_eq!(unarchived.get("ok").and_then(Value::as_bool), Some(true));
    assert_eq!(
        unarchived
            .get("session")
            .and_then(|session| session.get("archived"))
            .and_then(Value::as_bool),
        Some(false)
    );
    let active_list = list_sessions_payload(&state, "default", false, None, 20, &filter)
        .await
        .unwrap();
    assert_eq!(
        active_list
            .get("sessions")
            .and_then(Value::as_array)
            .and_then(|sessions| sessions.first())
            .and_then(|session| session.get("id"))
            .and_then(Value::as_str),
        Some("thread-archive-native")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn memory_status_and_controls_use_native_memory_methods() {
    let sandbox = unique_test_dir("memory-controls");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(codex_home.join("memories").join("rollout_summaries")).unwrap();
    fs::write(
        config_toml_path(&codex_home),
        r#"[memories]
generate_memories = false
use_memories = true
max_rollouts_per_startup = 7
extract_model = "gpt-5-mini"
"#,
    )
    .unwrap();
    fs::write(
        codex_home.join("memories").join("MEMORY.md"),
        "remember this\n",
    )
    .unwrap();
    fs::write(
        codex_home
            .join("memories")
            .join("rollout_summaries")
            .join("thread.md"),
        "summary\n",
    )
    .unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let client = app_server_client(&state, "default").await.unwrap();
    client
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": "thread-memory-native",
                    "name": "Memory native",
                    "preview": "memory mode",
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

    let status = memory_status_payload(&state, "default", Some("thread-memory-native"))
        .await
        .unwrap();
    assert_eq!(
        status
            .get("settings")
            .and_then(|settings| settings.get("generateMemories"))
            .and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        status
            .get("settings")
            .and_then(|settings| settings.get("maxRolloutsPerStartup"))
            .and_then(Value::as_i64),
        Some(7)
    );
    assert_eq!(
        status
            .get("storage")
            .and_then(|storage| storage.get("fileCount"))
            .and_then(Value::as_u64),
        Some(2)
    );

    let set_mode =
        set_session_memory_mode_payload(&state, "default", "thread-memory-native", "disabled")
            .await
            .unwrap();
    assert_eq!(
        set_mode.get("memoryMode").and_then(Value::as_str),
        Some("disabled")
    );
    let thread = client
        .request("thread/read", json!({ "threadId": "thread-memory-native" }))
        .await
        .unwrap();
    assert_eq!(
        thread
            .get("thread")
            .and_then(|thread| thread.get("memoryMode"))
            .and_then(Value::as_str),
        Some("disabled")
    );

    let reset = reset_memory_payload(&state, "default").await.unwrap();
    assert_eq!(reset.get("ok").and_then(Value::as_bool), Some(true));
    assert_eq!(
        reset
            .get("memory")
            .and_then(|memory| memory.get("storage"))
            .and_then(|storage| storage.get("fileCount"))
            .and_then(Value::as_u64),
        Some(0)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolve_server_request_payload_uses_pending_request_store() {
    let sandbox = unique_test_dir("approval-rust");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    mark_test_session_active(&state, "thread-1").await;
    handle_profile_server_request(
        &state,
        "default",
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
async fn resolve_server_request_payload_uses_original_app_server_client_key() {
    let sandbox = unique_test_dir("approval-original-client-key");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let session_id = "thread-dedicated-approval";
    let runtime_key = runtime_session_key("default", session_id);
    let client_key = format!("default::session::{session_id}");
    state
        .session_app_server_assignments
        .lock()
        .await
        .insert(runtime_key.clone(), client_key.clone());
    mark_test_session_active(&state, session_id).await;
    let _dedicated_client = app_server_client_for_session(&state, "default", session_id)
        .await
        .unwrap();

    handle_profile_server_request(
        &state,
        "default",
        &client_key,
        &backend::codex_app_server::AppServerRequest {
            id: json!("srv-dedicated"),
            method: "input/request".to_string(),
            params: json!({
                "threadId": session_id,
                "question": "Continue dedicated turn?"
            }),
        },
    )
    .await;
    assert_eq!(
        state
            .pending_server_requests
            .lock()
            .await
            .get(&runtime_key)
            .and_then(|entries| entries.get("srv-dedicated"))
            .map(|entry| entry.client_key.as_str()),
        Some(client_key.as_str()),
        "pending server requests must remember the app-server client that emitted them"
    );
    state
        .session_app_server_assignments
        .lock()
        .await
        .remove(&runtime_key);

    resolve_server_request_payload(
        &state,
        "default",
        session_id,
        "srv-dedicated",
        json!({ "answer": "yes" }),
    )
    .await
    .unwrap();

    assert!(
        state
            .pending_server_requests
            .lock()
            .await
            .get(&runtime_key)
            .is_none()
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_request_is_kept_when_it_arrives_before_turn_started_cache() {
    let sandbox = unique_test_dir("approval-before-turn-started-cache");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let session_id = "thread-early-approval";
    let runtime_key = runtime_session_key("default", session_id);
    let client_key = format!("default::session::{session_id}");
    state
        .session_app_server_assignments
        .lock()
        .await
        .insert(runtime_key.clone(), client_key.clone());

    handle_profile_server_request(
        &state,
        "default",
        &client_key,
        &backend::codex_app_server::AppServerRequest {
            id: json!("srv-early"),
            method: "input/request".to_string(),
            params: json!({
                "threadId": session_id,
                "question": "Continue before turn-started?"
            }),
        },
    )
    .await;

    assert!(
        state
            .pending_server_requests
            .lock()
            .await
            .get(&runtime_key)
            .and_then(|entries| entries.get("srv-early"))
            .is_some(),
        "server request should not be rejected before turn/started updates local caches"
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_server_request_does_not_reemit_attention() {
    let sandbox = unique_test_dir("approval-duplicate-request");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let session_id = "thread-duplicate-approval";
    let runtime_key = runtime_session_key("default", session_id);
    state
        .session_app_server_assignments
        .lock()
        .await
        .insert(runtime_key.clone(), "default".to_string());

    let request = backend::codex_app_server::AppServerRequest {
        id: json!("srv-duplicate"),
        method: "input/request".to_string(),
        params: json!({
            "threadId": session_id,
            "question": "Continue once?"
        }),
    };
    handle_profile_server_request(&state, "default", "default", &request).await;
    handle_profile_server_request(&state, "default", "default", &request).await;

    let pending_count = state
        .pending_server_requests
        .lock()
        .await
        .get(&runtime_key)
        .map(HashMap::len)
        .unwrap_or(0);
    assert_eq!(pending_count, 1);

    let attention_count = with_ui_state_read(&state, "default", |ui_state| {
        Ok(ui_state["notifications"]["items"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .filter(|notification| {
                notification.get("type").and_then(Value::as_str) == Some("sessionAttention")
                    && notification.get("sessionId").and_then(Value::as_str) == Some(session_id)
                    && notification
                        .get("payload")
                        .and_then(|payload| payload.get("requestId"))
                        .and_then(Value::as_str)
                        == Some("srv-duplicate")
            })
            .count())
    })
    .await
    .unwrap();
    assert_eq!(attention_count, 1);

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn computer_input_resolves_pending_computer_tool_request() {
    let sandbox = unique_test_dir("computer-input-pending");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    mark_test_session_active(&state, "thread-computer-input").await;
    handle_profile_server_request(
        &state,
        "default",
        "default",
        &backend::codex_app_server::AppServerRequest {
            id: json!("computer-call-1"),
            method: "item/tool/call".to_string(),
            params: json!({
                "threadId": "thread-computer-input",
                "turnId": "turn-1",
                "callId": "call-1",
                "namespace": "computer",
                "tool": "click",
                "arguments": {}
            }),
        },
    )
    .await;

    let payload = send_computer_input_payload(
        &state,
        "default",
        "thread-computer-input",
        json!({
            "type": "click",
            "x": 0.5,
            "y": 0.25,
            "button": "left",
            "coordinateSpace": "normalized"
        }),
    )
    .await
    .unwrap();

    assert_eq!(
        payload.get("routed").and_then(Value::as_str),
        Some("pendingDynamicTool")
    );
    assert!(
        state
            .pending_server_requests
            .lock()
            .await
            .get(&runtime_session_key("default", "thread-computer-input"))
            .is_none()
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_server_request_without_runtime_activity_is_ignored() {
    let sandbox = unique_test_dir("approval-stale-runtime");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    handle_profile_server_request(
        &state,
        "default",
        "default",
        &backend::codex_app_server::AppServerRequest {
            id: json!("stale-approval"),
            method: "input/request".to_string(),
            params: json!({
                "threadId": "thread-stale",
                "question": "Continue?"
            }),
        },
    )
    .await;

    let pending = state.pending_server_requests.lock().await;
    assert!(
        pending
            .get(&runtime_session_key("default", "thread-stale"))
            .is_none()
    );
    drop(pending);

    let ui_state = with_ui_state_read(&state, "default", |ui_state| Ok(ui_state.clone()))
        .await
        .unwrap();
    assert!(
        ui_state["highlightsByThreadId"]
            .get("thread-stale")
            .is_none()
    );
    assert!(
        ui_state["notifications"]["items"]
            .as_array()
            .is_some_and(Vec::is_empty)
    );

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
    mark_test_session_active(&state, "thread-dynamic").await;
    handle_profile_server_request(
        &state,
        "default",
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
    assert!(
        payload
            .get("hostResources")
            .and_then(Value::as_object)
            .is_some(),
        "runtime status should expose host resource diagnostics"
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

    let mcp = execute_ws_method(
        &state,
        &out_tx,
        &subscriptions,
        &auth,
        "codex/mcp/status/list",
        json!({ "detail": "toolsAndAuthOnly", "limit": 20 }),
    )
    .await
    .unwrap();
    assert!(
        mcp.get("data")
            .and_then(Value::as_array)
            .is_some_and(|servers| servers
                .iter()
                .any(|server| server.get("name").and_then(Value::as_str) == Some("computer-use")))
    );

    let refresh = execute_ws_method(
        &state,
        &out_tx,
        &subscriptions,
        &auth,
        "codex/mcp/refresh",
        json!({}),
    )
    .await
    .unwrap();
    assert!(refresh.as_object().is_some_and(|object| object.is_empty()));

    let oauth = execute_ws_method(
        &state,
        &out_tx,
        &subscriptions,
        &auth,
        "codex/mcp/oauth/login",
        json!({ "name": "computer-use" }),
    )
    .await
    .unwrap();
    assert_eq!(
        oauth.get("authorizationUrl").and_then(Value::as_str),
        Some("https://example.com/oauth/authorize")
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
async fn codex_apps_list_routes_by_thread_profile_when_request_profile_is_stale() {
    let sandbox = unique_test_dir("codex-apps-list-profile-routing");
    let workspace = sandbox.join("workspace");
    let default_codex_home = sandbox.join("codex-default");
    let second_codex_home = sandbox.join("codex-second");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&default_codex_home).unwrap();
    fs::create_dir_all(&second_codex_home).unwrap();

    let mut state = test_state_with_fake_app_server(
        workspace.clone(),
        vec![workspace.clone()],
        default_codex_home.clone(),
    );
    let mut config = (*state.config).clone();
    config.config_file_path = Some(sandbox.join("isolated-codex-webui.yml"));
    config.profiles.insert(
        "second".to_string(),
        RuntimeProfile {
            label: "Second".to_string(),
            codex_home: second_codex_home.clone(),
            data_dir: sandbox.join(".data").join("profiles").join("second"),
        },
    );
    state.config = Arc::new(config);

    let session_id = "019f0000-0000-7000-8000-000000000305";
    let rollout_dir = second_codex_home.join("sessions").join("2026/06/24");
    fs::create_dir_all(&rollout_dir).unwrap();
    fs::write(
        rollout_dir.join(format!(
            "rollout-2026-06-24T00-03-03-{session_id}.jsonl"
        )),
        format!(
            "{{\"timestamp\":\"2026-04-24T01:00:00.000Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"{session_id}\",\"timestamp\":\"2026-04-24T01:00:00.000Z\",\"cwd\":\"{}\",\"originator\":\"codex_webui\",\"cli_version\":\"0.121.0\",\"source\":\"vscode\"}}}}\n{{\"timestamp\":\"2026-04-24T01:00:01.000Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"Second profile apps\",\"kind\":\"plain\"}}}}\n",
            workspace.display()
        ),
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(8));

    let default_client = app_server_client(&state, "default").await.unwrap();
    let second_client = app_server_client(&state, "second").await.unwrap();
    let (out_tx, _out_rx) = mpsc::channel(8);
    let subscriptions = Arc::new(Mutex::new(HashMap::new()));
    let auth = AuthContext {
        role: UserRole::Owner,
        profile_id: "default".to_string(),
    };

    execute_ws_method(
        &state,
        &out_tx,
        &subscriptions,
        &auth,
        "codex/apps/list",
        json!({
            "threadId": session_id,
            "profileId": "default"
        }),
    )
    .await
    .unwrap();

    let default_count = default_client
        .request("debug/requestCount", json!({ "target": "app/list" }))
        .await
        .unwrap();
    let second_count = second_client
        .request("debug/requestCount", json!({ "target": "app/list" }))
        .await
        .unwrap();
    assert_eq!(default_count.get("count").and_then(Value::as_u64), Some(0));
    assert_eq!(second_count.get("count").and_then(Value::as_u64), Some(1));

    let _ = fs::remove_dir_all(sandbox);
}

#[test]
fn quota_normalization_uses_server_window_durations_and_additional_limits() {
    let payload: UsageResponseShape = serde_json::from_value(json!({
        "email": "quota@example.com",
        "plan_type": "pro",
        "rate_limit": {
            "primary_window": {
                "used_percent": 38,
                "limit_window_seconds": 2_592_000,
                "reset_at": 1_800_000_000
            }
        },
        "additional_rate_limits": [{
            "limit_name": "Code review",
            "metered_feature": "review",
            "rate_limit": {
                "primary_window": {
                    "used_percent": 20,
                    "limit_window_seconds": 86_400,
                    "reset_at": 1_800_086_400
                }
            }
        }]
    }))
    .unwrap();

    let normalized = normalize_quota_payload(payload);
    assert_eq!(normalized["available"].as_bool(), Some(true));
    assert_eq!(normalized["limits"].as_array().map(Vec::len), Some(2));
    assert_eq!(normalized["windows"][0]["label"].as_str(), Some("Monthly"));
    assert_eq!(
        normalized["windows"][0]["windowDurationMinutes"].as_i64(),
        Some(43_200)
    );
    assert_eq!(
        normalized["windows"][0]["resetAt"].as_u64(),
        Some(1_800_000_000_000)
    );
    assert!(normalized["fiveHour"].is_null());
    assert!(normalized["weekly"].is_null());
    assert_eq!(
        normalized["limits"][1]["windows"][0]["label"].as_str(),
        Some("Daily")
    );
}

#[test]
fn quota_normalization_finds_legacy_aliases_by_duration_not_window_position() {
    let payload: UsageResponseShape = serde_json::from_value(json!({
        "rate_limit": {
            "primary_window": {
                "used_percent": 10,
                "limit_window_seconds": 604_800,
                "reset_at": 1_800_000_000
            },
            "secondary_window": {
                "used_percent": 25,
                "limit_window_seconds": 18_000,
                "reset_at": 1_799_000_000
            }
        }
    }))
    .unwrap();

    let normalized = normalize_quota_payload(payload);
    assert_eq!(normalized["weekly"]["kind"].as_str(), Some("primary"));
    assert_eq!(normalized["weekly"]["label"].as_str(), Some("Weekly"));
    assert_eq!(normalized["fiveHour"]["kind"].as_str(), Some("secondary"));
    assert_eq!(normalized["fiveHour"]["label"].as_str(), Some("5h"));
}

#[tokio::test]
async fn rate_limit_notification_invalidates_quota_before_client_refresh() {
    let sandbox = unique_test_dir("quota-notification-cache-invalidation");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    state.quota_cache.lock().await.insert(
        "default".to_string(),
        CachedQuota {
            created_at: Instant::now(),
            payload: json!({ "available": true, "windows": [] }),
        },
    );

    handle_profile_runtime_notification(
        &state,
        "default",
        &AppServerNotification {
            method: "account/rateLimits/updated".to_string(),
            params: json!({
                "rateLimits": {
                    "primary": { "usedPercent": 50 }
                }
            }),
        },
    )
    .await;

    assert!(!state.quota_cache.lock().await.contains_key("default"));
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
async fn profile_account_list_returns_quota_for_each_configured_profile() {
    let sandbox = unique_test_dir("profile-account-list");
    let workspace = sandbox.join("workspace");
    let default_codex_home = sandbox.join("codex-default");
    let work_codex_home = sandbox.join("codex-work");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&default_codex_home).unwrap();
    fs::create_dir_all(&work_codex_home).unwrap();
    fs::write(
        default_codex_home.join("auth.json"),
        r#"{"auth_mode":"chatgpt","tokens":{"access_token":"default-token","account_id":"default-account"}}"#,
    )
    .unwrap();
    fs::write(
        work_codex_home.join("auth.json"),
        r#"{"auth_mode":"chatgpt","tokens":{"access_token":"work-token","account_id":"work-account"}}"#,
    )
    .unwrap();

    let mut state = test_state(
        workspace.clone(),
        vec![workspace.clone()],
        default_codex_home,
    );
    let mut config = (*state.config).clone();
    config.profiles.insert(
        "work".to_string(),
        RuntimeProfile {
            label: "Work".to_string(),
            codex_home: work_codex_home,
            data_dir: sandbox.join("data-work"),
        },
    );
    state.config = Arc::new(config);
    state.quota_cache.lock().await.insert(
        "default".to_string(),
        CachedQuota {
            created_at: Instant::now(),
            payload: json!({
                "available": true,
                "source": "backend-api",
                "fetchedAt": now_unix_ms(),
                "account": "default@example.com",
                "plan": "plus",
                "fiveHour": { "remainingPercent": 75 },
                "weekly": { "remainingPercent": 60 },
                "error": Value::Null
            }),
        },
    );
    state.quota_cache.lock().await.insert(
        "work".to_string(),
        CachedQuota {
            created_at: Instant::now(),
            payload: json!({
                "available": true,
                "source": "backend-api",
                "fetchedAt": now_unix_ms(),
                "account": "work@example.com",
                "plan": "team",
                "fiveHour": { "remainingPercent": 90 },
                "weekly": { "remainingPercent": 80 },
                "error": Value::Null
            }),
        },
    );

    let payload = codex_profile_accounts_payload(&state, "work", false)
        .await
        .unwrap();
    let profiles = payload
        .get("profiles")
        .and_then(Value::as_array)
        .expect("profile summaries should be returned");
    assert_eq!(profiles.len(), 2);
    assert_eq!(
        profiles[0].get("profileId").and_then(Value::as_str),
        Some("work")
    );
    assert_eq!(
        profiles[0]
            .get("account")
            .and_then(|account| account.get("email"))
            .and_then(Value::as_str),
        Some("work@example.com")
    );
    assert_eq!(
        profiles[0]
            .get("quota")
            .and_then(|quota| quota.get("weekly"))
            .and_then(|weekly| weekly.get("remainingPercent"))
            .and_then(Value::as_u64),
        Some(80)
    );
    assert_eq!(
        profiles[1]
            .get("account")
            .and_then(|account| account.get("type"))
            .and_then(Value::as_str),
        Some("chatgpt")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn codex_reset_tickets_payload_normalizes_app_server_rate_limit_tickets() {
    let sandbox = unique_test_dir("codex-reset-tickets");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let client = app_server_client(&state, "default").await.unwrap();
    client
        .request(
            "debug/setRateLimitsResponse",
            json!({
                "response": {
                    "rateLimits": {
                        "limitId": "codex",
                        "limitName": "Codex"
                    },
                    "rateLimitsByLimitId": {
                        "codex": {
                            "limitName": "Codex",
                            "resetTickets": [
                                {
                                    "ticketId": "ticket-1",
                                    "label": "Reset five hour",
                                    "limitId": "codex",
                                    "expiresAt": "2026-06-14T00:00:00Z"
                                },
                                {
                                    "ticketId": "ticket-never",
                                    "label": "Permanent reset",
                                    "expiresAt": Value::Null
                                },
                                {
                                    "ticketId": "ticket-legacy",
                                    "label": "Legacy reset"
                                }
                            ]
                        }
                    }
                }
            }),
        )
        .await
        .unwrap();

    let payload = codex_reset_tickets_payload(&state, "default", true)
        .await
        .unwrap();
    assert_eq!(
        payload.get("available").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        payload.get("supported").and_then(Value::as_bool),
        Some(true)
    );
    let tickets = payload
        .get("tickets")
        .and_then(Value::as_array)
        .expect("reset tickets should be normalized");
    let ticket = tickets
        .iter()
        .find(|ticket| ticket.get("id").and_then(Value::as_str) == Some("ticket-1"))
        .expect("expiring reset ticket should be present");
    assert_eq!(ticket.get("id").and_then(Value::as_str), Some("ticket-1"));
    assert_eq!(ticket.get("limitId").and_then(Value::as_str), Some("codex"));
    assert_eq!(ticket.get("available").and_then(Value::as_bool), Some(true));
    assert_eq!(
        ticket.get("expiresAt").and_then(Value::as_str),
        Some("2026-06-14T00:00:00Z")
    );
    assert_eq!(
        ticket.get("expirationStatus").and_then(Value::as_str),
        Some("expires")
    );
    let never_expires = tickets
        .iter()
        .find(|ticket| ticket.get("id").and_then(Value::as_str) == Some("ticket-never"))
        .expect("non-expiring reset ticket should be present");
    assert_eq!(never_expires["expiresAt"], Value::Null);
    assert_eq!(never_expires["expirationStatus"].as_str(), Some("never"));
    let expiration_unknown = tickets
        .iter()
        .find(|ticket| ticket.get("id").and_then(Value::as_str) == Some("ticket-legacy"))
        .expect("legacy reset ticket should be present");
    assert_eq!(expiration_unknown["expiresAt"], Value::Null);
    assert_eq!(
        expiration_unknown["expirationStatus"].as_str(),
        Some("unknown")
    );
    assert!(payload.get("rateLimitsByLimitId").is_some());

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn codex_reset_tickets_payload_supports_latest_reset_credit_summary() {
    let sandbox = unique_test_dir("codex-reset-credit-summary");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let client = app_server_client(&state, "default").await.unwrap();
    client
        .request(
            "debug/setRateLimitsResponse",
            json!({
                "response": {
                    "rateLimits": {
                        "limitId": "codex",
                        "limitName": "Codex"
                    },
                    "rateLimitsByLimitId": {
                        "codex": {
                            "limitName": "Codex"
                        }
                    },
                    "rateLimitResetCredits": {
                        "availableCount": 1,
                        "credits": [{
                            "id": "credit-latest-1",
                            "resetType": "codex",
                            "status": "available",
                            "grantedAt": 1_800_000_000,
                            "expiresAt": 1_800_086_400,
                            "title": "Event reset credit",
                            "description": "One temporary reset"
                        }]
                    }
                }
            }),
        )
        .await
        .unwrap();

    let payload = codex_reset_tickets_payload(&state, "default", true)
        .await
        .unwrap();
    assert_eq!(
        payload.get("supported").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        payload.get("availableCount").and_then(Value::as_i64),
        Some(1)
    );
    let tickets = payload
        .get("tickets")
        .and_then(Value::as_array)
        .expect("latest reset credits should materialize usable entries");
    assert_eq!(tickets.len(), 1);
    assert_eq!(
        tickets
            .first()
            .and_then(|ticket| ticket.get("id"))
            .and_then(Value::as_str),
        Some("credit-latest-1")
    );
    assert_eq!(tickets[0]["status"].as_str(), Some("available"));
    assert_eq!(tickets[0]["resetType"].as_str(), Some("codex"));
    assert_eq!(tickets[0]["createdAt"].as_u64(), Some(1_800_000_000_000));
    assert_eq!(tickets[0]["expiresAt"].as_u64(), Some(1_800_086_400_000));
    assert_eq!(tickets[0]["expirationStatus"].as_str(), Some("expires"));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn codex_reset_tickets_payload_is_quiet_when_protocol_is_unsupported() {
    let sandbox = unique_test_dir("codex-reset-tickets-unsupported-quiet");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let payload = codex_reset_tickets_payload(&state, "default", false)
        .await
        .unwrap();
    assert_eq!(
        payload.get("available").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        payload.get("supported").and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        payload.get("message"),
        Some(&Value::Null),
        "unsupported protocol should not produce a user-facing warning"
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn codex_reset_ticket_use_reports_unsupported_without_codex_rpc() {
    let sandbox = unique_test_dir("codex-reset-ticket-use-unsupported");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let client = app_server_client(&state, "default").await.unwrap();
    for method in [
        "account/rateLimitResetCredit/consume",
        "account/rateLimits/resetTicket/use",
        "account/resetTickets/use",
        "account/resetTicket/use",
        "account/rateLimitResetTicket/use",
    ] {
        client
            .request(
                "debug/setError",
                json!({
                    "method": method,
                    "message": "unknown method"
                }),
            )
            .await
            .unwrap();
    }
    let error = use_codex_reset_ticket_payload(
        &state,
        "default",
        json!({
            "ticketId": "ticket-1",
            "limitId": "codex"
        }),
    )
    .await
    .expect_err("fake app-server does not expose reset-ticket use RPCs");
    assert!(
        error
            .to_string()
            .contains("reset-ticket use is not exposed"),
        "{error}"
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn codex_reset_ticket_use_consumes_latest_reset_credit() {
    let sandbox = unique_test_dir("codex-reset-credit-use");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let payload = use_codex_reset_ticket_payload(
        &state,
        "default",
        json!({
            "ticketId": "rate-limit-reset-credit-1",
            "idempotencyKey": "reset-attempt-1"
        }),
    )
    .await
    .unwrap();
    assert_eq!(payload.get("ok").and_then(Value::as_bool), Some(true));
    assert_eq!(
        payload.get("method").and_then(Value::as_str),
        Some("account/rateLimitResetCredit/consume")
    );
    assert_eq!(
        payload.get("outcome").and_then(Value::as_str),
        Some("reset")
    );
    assert_eq!(
        payload.get("idempotencyKey").and_then(Value::as_str),
        Some("reset-attempt-1")
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
    let thread_summary = payload
        .get("sessions")
        .and_then(Value::as_array)
        .and_then(|sessions| {
            sessions
                .iter()
                .find(|session| session.get("id").and_then(Value::as_str) == Some("thread-1"))
        })
        .cloned()
        .expect("expected seeded session");
    assert_eq!(
        thread_summary.get("status").and_then(Value::as_str),
        Some("running")
    );

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
                "updatedAt": now_unix_ms().saturating_sub(70_000)
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
    let thread_summary = payload
        .get("sessions")
        .and_then(Value::as_array)
        .and_then(|sessions| {
            sessions
                .iter()
                .find(|session| session.get("id").and_then(Value::as_str) == Some("thread-1"))
        })
        .cloned()
        .expect("expected seeded session");
    assert_eq!(
        thread_summary.get("status").and_then(Value::as_str),
        Some("running")
    );

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
                "updatedAt": now_unix_ms().saturating_sub(61_000)
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
async fn session_list_keeps_recent_running_status_without_reconcile() {
    let sandbox = unique_test_dir("session-list-recent-running-no-reconcile");
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
                    "id": "thread-recent-running",
                    "name": "Recent running",
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
                            "id": "turn-recent",
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
        runtime_session_key("default", "thread-recent-running"),
        "turn-recent".to_string(),
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
            "thread-recent-running".to_string(),
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
        Some("thread-recent-running")
    );
    assert_eq!(first.get("status").and_then(Value::as_str), Some("running"));
    assert_eq!(
        state
            .active_turns
            .lock()
            .await
            .get(&runtime_session_key("default", "thread-recent-running"))
            .cloned(),
        Some("turn-recent".to_string())
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
async fn runtime_item_aliases_update_volatile_turn_without_persisting_status() {
    let sandbox = unique_test_dir("runtime-notification-alias-running");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    handle_profile_runtime_notification(
        &state,
        "default",
        &AppServerNotification {
            method: "item/started".to_string(),
            params: json!({
                "sessionId": "thread-alias",
                "turn_id": "turn-alias",
                "itemId": "item-alias",
                "item": {
                    "id": "item-alias",
                    "type": "reasoning"
                }
            }),
        },
    )
    .await;

    let runtime_key = runtime_session_key("default", "thread-alias");
    assert_eq!(
        state.active_turns.lock().await.get(&runtime_key).cloned(),
        Some("turn-alias".to_string())
    );
    let runtime_status = with_ui_state_read(&state, "default", |ui_state| {
        Ok(ui_state["runtimeStatusByThreadId"]["thread-alias"].clone())
    })
    .await
    .unwrap();
    assert!(runtime_status.is_null());

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_thread_status_overrides_stale_cached_runtime_activity() {
    let sandbox = unique_test_dir("terminal-status-authoritative");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let session_id = "thread-terminal-authoritative";
    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": session_id,
                    "name": "Terminal status authoritative",
                    "preview": "",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 20,
                    "status": "completed",
                    "isSubagent": false,
                    "agentNickname": Value::Null,
                    "agentRole": Value::Null,
                    "turns": []
                }
            }),
        )
        .await
        .unwrap();

    let runtime_key = runtime_session_key("default", session_id);
    state
        .active_turns
        .lock()
        .await
        .insert(runtime_key.clone(), "turn-stale".to_string());
    state
        .pending_turn_starts
        .lock()
        .await
        .insert(runtime_key.clone());
    with_ui_state_write(&state, "default", |ui_state| {
        ui_state["runtimeStatusByThreadId"][session_id] = json!({
            "status": "running",
            "updatedAt": now_unix_ms()
        });
        Ok(())
    })
    .await
    .unwrap();

    let relay = ensure_stream_relay(&state, "default", session_id)
        .await
        .expect("relay should initialize");
    let mut receiver = relay.subscribe();

    handle_profile_runtime_notification(
        &state,
        "default",
        &AppServerNotification {
            method: "thread/status/changed".to_string(),
            params: json!({
                "threadId": session_id,
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
    assert!(state.active_turns.lock().await.get(&runtime_key).is_none());
    assert!(
        !state
            .pending_turn_starts
            .lock()
            .await
            .contains(&runtime_key)
    );

    let ui_state = with_ui_state_read(&state, "default", |ui_state| Ok(ui_state.clone()))
        .await
        .unwrap();
    assert_eq!(
        ui_state["runtimeStatusByThreadId"][session_id]["status"].as_str(),
        Some("completed")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_status_does_not_override_app_server_active_turn() {
    let sandbox = unique_test_dir("terminal-status-keeps-live-turn");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let session_id = "thread-terminal-keeps-live";
    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": session_id,
                    "name": "Terminal status should not stop active turn",
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
                            "id": "turn-live",
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

    let runtime_key = runtime_session_key("default", session_id);
    state
        .active_turns
        .lock()
        .await
        .insert(runtime_key.clone(), "turn-live".to_string());
    set_runtime_session_status(&state, "default", session_id, "running").await;

    handle_profile_runtime_notification(
        &state,
        "default",
        &AppServerNotification {
            method: "thread/status/changed".to_string(),
            params: json!({
                "threadId": session_id,
                "status": { "type": "completed" }
            }),
        },
    )
    .await;

    assert_eq!(
        state.active_turns.lock().await.get(&runtime_key).cloned(),
        Some("turn-live".to_string())
    );
    let ui_state = with_ui_state_read(&state, "default", |ui_state| Ok(ui_state.clone()))
        .await
        .unwrap();
    assert_eq!(
        ui_state["runtimeStatusByThreadId"][session_id]["status"].as_str(),
        Some("running")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_status_timeout_preserves_cached_active_turn() {
    let sandbox = unique_test_dir("terminal-status-timeout-keeps-live-turn");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let session_id = "thread-terminal-timeout-keeps-live";
    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": session_id,
                    "name": "A slow read must not stop a live turn",
                    "preview": "",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 20,
                    "status": "running",
                    "isSubagent": false,
                    "readDelayMs": 1_500,
                    "turns": [{
                        "id": "turn-live",
                        "status": "inProgress",
                        "items": []
                    }]
                }
            }),
        )
        .await
        .unwrap();

    let runtime_key = runtime_session_key("default", session_id);
    state
        .active_turns
        .lock()
        .await
        .insert(runtime_key.clone(), "turn-live".to_string());
    set_runtime_session_status(&state, "default", session_id, "running").await;

    handle_profile_runtime_notification(
        &state,
        "default",
        &AppServerNotification {
            method: "thread/status/changed".to_string(),
            params: json!({
                "threadId": session_id,
                "status": "failed"
            }),
        },
    )
    .await;

    assert_eq!(
        state.active_turns.lock().await.get(&runtime_key).cloned(),
        Some("turn-live".to_string())
    );
    let status = with_ui_state_read(&state, "default", |ui_state| {
        Ok(ui_state["runtimeStatusByThreadId"][session_id]["status"].clone())
    })
    .await
    .unwrap();
    assert_eq!(status.as_str(), Some("running"));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_status_checks_app_server_when_runtime_status_is_fresh_running() {
    let sandbox = unique_test_dir("terminal-status-checks-fresh-runtime");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let session_id = "thread-terminal-fresh-runtime";
    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": session_id,
                    "name": "Fresh runtime status should force app-server check",
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
                            "id": "turn-live-without-cached-id",
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

    let runtime_key = runtime_session_key("default", session_id);
    set_runtime_session_status(&state, "default", session_id, "running").await;

    handle_profile_runtime_notification(
        &state,
        "default",
        &AppServerNotification {
            method: "thread/status/changed".to_string(),
            params: json!({
                "threadId": session_id,
                "status": { "type": "completed" }
            }),
        },
    )
    .await;

    assert_eq!(
        state.active_turns.lock().await.get(&runtime_key).cloned(),
        Some("turn-live-without-cached-id".to_string())
    );
    let ui_state = with_ui_state_read(&state, "default", |ui_state| Ok(ui_state.clone()))
        .await
        .unwrap();
    assert_eq!(
        ui_state["runtimeStatusByThreadId"][session_id]["status"].as_str(),
        Some("running")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_status_is_not_overridden_by_unversioned_delta() {
    let sandbox = unique_test_dir("terminal-status-ignores-unversioned-delta");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let session_id = "thread-terminal-recent-live";
    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": session_id,
                    "name": "Recent live event beats stale terminal status",
                    "preview": "",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 20,
                    "status": "completed",
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
            method: "item/agentMessage/delta".to_string(),
            params: json!({
                "threadId": session_id,
                "itemId": "item-live-no-turn-id",
                "delta": "still streaming"
            }),
        },
    )
    .await;

    handle_profile_runtime_notification(
        &state,
        "default",
        &AppServerNotification {
            method: "thread/status/changed".to_string(),
            params: json!({
                "threadId": session_id,
                "status": { "type": "completed" }
            }),
        },
    )
    .await;

    let runtime_key = runtime_session_key("default", session_id);
    assert!(state.active_turns.lock().await.get(&runtime_key).is_none());
    let ui_state = with_ui_state_read(&state, "default", |ui_state| Ok(ui_state.clone()))
        .await
        .unwrap();
    assert_eq!(
        ui_state["runtimeStatusByThreadId"][session_id]["status"].as_str(),
        Some("completed")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turn_completed_clears_stale_different_cached_active_turn() {
    let sandbox = unique_test_dir("turn-completed-clears-stale-active");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let session_id = "thread-completed-stale-active";
    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": session_id,
                    "name": "Completed turn with stale cache",
                    "preview": "",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 20,
                    "status": "completed",
                    "isSubagent": false,
                    "agentNickname": Value::Null,
                    "agentRole": Value::Null,
                    "turns": []
                }
            }),
        )
        .await
        .unwrap();

    let runtime_key = runtime_session_key("default", session_id);
    state
        .active_turns
        .lock()
        .await
        .insert(runtime_key.clone(), "turn-stale".to_string());
    set_runtime_session_status(&state, "default", session_id, "running").await;

    handle_profile_runtime_notification(
        &state,
        "default",
        &AppServerNotification {
            method: "turn/completed".to_string(),
            params: json!({
                "threadId": session_id,
                "turnId": "turn-actual"
            }),
        },
    )
    .await;

    assert!(state.active_turns.lock().await.get(&runtime_key).is_none());
    let ui_state = with_ui_state_read(&state, "default", |ui_state| Ok(ui_state.clone()))
        .await
        .unwrap();
    assert_eq!(
        ui_state["runtimeStatusByThreadId"][session_id]["status"].as_str(),
        Some("completed")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_app_server_cleanup_is_scoped_to_requested_session() {
    let sandbox = unique_test_dir("missing-app-server-session-scoped");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    let session_a = "thread-missing-app-server-a";
    let session_b = "thread-missing-app-server-b";
    let runtime_key_a = runtime_session_key("default", session_a);
    let runtime_key_b = runtime_session_key("default", session_b);
    state
        .active_turns
        .lock()
        .await
        .insert(runtime_key_a.clone(), "turn-a".to_string());
    state
        .active_turns
        .lock()
        .await
        .insert(runtime_key_b.clone(), "turn-b".to_string());
    {
        let mut assignments = state.session_app_server_assignments.lock().await;
        assignments.insert(runtime_key_a.clone(), "default::session::a".to_string());
        assignments.insert(runtime_key_b.clone(), "default::session::b".to_string());
    }
    with_ui_state_write(&state, "default", |ui_state| {
        ui_state["runtimeStatusByThreadId"][session_a] = json!({
            "status": "running",
            "updatedAt": now_unix_ms().saturating_sub(60_000)
        });
        ui_state["runtimeStatusByThreadId"][session_b] = json!({
            "status": "running",
            "updatedAt": now_unix_ms().saturating_sub(60_000)
        });
        Ok(())
    })
    .await
    .unwrap();

    let cleared = clear_stale_session_runtime_activity_if_app_server_missing(
        &state,
        "default",
        session_a,
        0,
        "codex app-server is not running",
    )
    .await;

    assert!(cleared);
    assert!(
        state
            .active_turns
            .lock()
            .await
            .get(&runtime_key_a)
            .is_none()
    );
    assert_eq!(
        state.active_turns.lock().await.get(&runtime_key_b).cloned(),
        Some("turn-b".to_string())
    );
    let ui_state = with_ui_state_read(&state, "default", |ui_state| Ok(ui_state.clone()))
        .await
        .unwrap();
    assert_eq!(
        ui_state["runtimeStatusByThreadId"][session_a]["status"].as_str(),
        Some("failed")
    );
    assert_eq!(
        ui_state["runtimeStatusByThreadId"][session_b]["status"].as_str(),
        Some("running")
    );
    let assignments = state.session_app_server_assignments.lock().await;
    assert!(!assignments.contains_key(&runtime_key_a));
    assert_eq!(
        assignments.get(&runtime_key_b).map(String::as_str),
        Some("default::session::b")
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
    let client = app_server_client(&state, "default").await.unwrap();
    let argv = client.request("debug/argv", json!({})).await.unwrap();
    let args = argv
        .get("args")
        .and_then(Value::as_array)
        .expect("fake app-server should expose argv");
    assert!(args.windows(2).any(|window| {
        window[0].as_str() == Some("--enable") && window[1].as_str() == Some("goals")
    }));

    let count = client
        .request(
            "debug/requestCount",
            json!({
                "target": "config/batchWrite"
            }),
        )
        .await
        .unwrap();

    assert_eq!(count.get("count").and_then(Value::as_u64), Some(0));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unassigned_goal_session_gets_dedicated_app_server_client() {
    let sandbox = unique_test_dir("goal-dedicated-client");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let _default_client = app_server_client(&state, "default").await.unwrap();
    let _goal_client =
        app_server_client_for_goal_session(&state, "default", "thread-goal-dedicated")
            .await
            .unwrap();

    assert_eq!(state.app_servers.client_count().await, 2);
    assert_eq!(
        state
            .session_app_server_assignments
            .lock()
            .await
            .get(&runtime_session_key("default", "thread-goal-dedicated"))
            .cloned(),
        Some("default::goal::thread-goal-dedicated".to_string())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assigned_session_goal_reuses_existing_app_server_client() {
    let sandbox = unique_test_dir("goal-existing-client");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let _turn_client =
        app_server_client_for_session_turn(&state, "default", "thread-goal-existing")
            .await
            .unwrap();
    let _goal_client =
        app_server_client_for_goal_session(&state, "default", "thread-goal-existing")
            .await
            .unwrap();

    assert_eq!(state.app_servers.client_count().await, 1);
    assert_eq!(
        state
            .session_app_server_assignments
            .lock()
            .await
            .get(&runtime_session_key("default", "thread-goal-existing"))
            .cloned(),
        Some("default".to_string())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn active_goal_session_turn_uses_goal_app_server_after_restart_assignment_loss() {
    let sandbox = unique_test_dir("goal-turn-dedicated-after-restart");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    cache_session_goal_payload(
        &state,
        "default",
        "thread-goal-resume",
        &json!({
            "threadId": "thread-goal-resume",
            "objective": "continue after gateway restart",
            "status": "active"
        }),
    )
    .await;

    let _turn_client = app_server_client_for_session_turn(&state, "default", "thread-goal-resume")
        .await
        .unwrap();

    assert_eq!(state.app_servers.client_count().await, 1);
    assert_eq!(
        state
            .session_app_server_assignments
            .lock()
            .await
            .get(&runtime_session_key("default", "thread-goal-resume"))
            .cloned(),
        Some("default::goal::thread-goal-resume".to_string())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn active_goal_session_send_resumes_thread_before_turn_start_after_restart_assignment_loss() {
    let sandbox = unique_test_dir("goal-send-resumes-after-restart");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let session_id = "thread-goal-resume-send";
    cache_session_goal_payload(
        &state,
        "default",
        session_id,
        &json!({
            "threadId": session_id,
            "objective": "continue after gateway restart",
            "status": "active"
        }),
    )
    .await;

    send_turn_payload(
        &state,
        "default",
        session_id,
        "Continue the active goal.",
        None,
        None,
        json!({
            "cwd": workspace.display().to_string(),
            "model": "gpt-5"
        }),
        Some("client-goal-resume-send"),
    )
    .await
    .unwrap();

    let client_key = format!("default::goal::{session_id}");
    assert_eq!(
        state
            .session_app_server_assignments
            .lock()
            .await
            .get(&runtime_session_key("default", session_id))
            .cloned(),
        Some(client_key.clone())
    );
    let goal_client = app_server_client_by_key(&state, "default", &client_key)
        .await
        .unwrap();
    let request_log = goal_client
        .request("debug/requestLog", json!({}))
        .await
        .unwrap();
    let methods = request_log
        .get("methods")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let resume_pos = methods
        .iter()
        .position(|method| method.as_str() == Some("thread/resume"))
        .expect("goal send should resume the thread before turn/start");
    let turn_start_pos = methods
        .iter()
        .position(|method| method.as_str() == Some("turn/start"))
        .expect("goal send should start a turn after resume");
    assert!(
        resume_pos < turn_start_pos,
        "thread/resume must precede turn/start for goal resume send: {request_log}"
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn usage_limited_goal_session_turn_uses_goal_app_server_after_restart_assignment_loss() {
    let sandbox = unique_test_dir("goal-turn-usage-limited-dedicated-after-restart");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    cache_session_goal_payload(
        &state,
        "default",
        "thread-goal-usage-limited-resume",
        &json!({
            "threadId": "thread-goal-usage-limited-resume",
            "objective": "resume after usage reset",
            "status": "usageLimited"
        }),
    )
    .await;

    let _turn_client =
        app_server_client_for_session_turn(&state, "default", "thread-goal-usage-limited-resume")
            .await
            .unwrap();

    assert_eq!(state.app_servers.client_count().await, 1);
    assert_eq!(
        state
            .session_app_server_assignments
            .lock()
            .await
            .get(&runtime_session_key(
                "default",
                "thread-goal-usage-limited-resume"
            ))
            .cloned(),
        Some("default::goal::thread-goal-usage-limited-resume".to_string())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn budget_limited_goal_session_turn_uses_goal_app_server_after_restart_assignment_loss() {
    let sandbox = unique_test_dir("goal-turn-budget-limited-dedicated-after-restart");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    cache_session_goal_payload(
        &state,
        "default",
        "thread-goal-budget-limited-resume",
        &json!({
            "threadId": "thread-goal-budget-limited-resume",
            "objective": "wrap up after budget limit",
            "status": "budgetLimited"
        }),
    )
    .await;

    let _turn_client =
        app_server_client_for_session_turn(&state, "default", "thread-goal-budget-limited-resume")
            .await
            .unwrap();

    assert_eq!(state.app_servers.client_count().await, 1);
    assert_eq!(
        state
            .session_app_server_assignments
            .lock()
            .await
            .get(&runtime_session_key(
                "default",
                "thread-goal-budget-limited-resume"
            ))
            .cloned(),
        Some("default::goal::thread-goal-budget-limited-resume".to_string())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn active_goal_fetch_uses_goal_app_server_after_restart_assignment_loss() {
    let sandbox = unique_test_dir("goal-fetch-dedicated-after-restart");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let goal_client =
        app_server_client_for_goal_session(&state, "default", "thread-goal-fetch-resume")
            .await
            .unwrap();
    goal_client
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": "thread-goal-fetch-resume",
                    "name": "Goal thread",
                    "preview": "resume goal",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 2,
                    "status": "idle",
                    "turns": [],
                    "goal": {
                        "threadId": "thread-goal-fetch-resume",
                        "objective": "continue after restart",
                        "status": "active",
                        "createdAt": 1,
                        "updatedAt": 2
                    }
                }
            }),
        )
        .await
        .unwrap();
    cache_session_goal_payload(
        &state,
        "default",
        "thread-goal-fetch-resume",
        &json!({
            "threadId": "thread-goal-fetch-resume",
            "objective": "continue after restart",
            "status": "active"
        }),
    )
    .await;
    state
        .session_app_server_assignments
        .lock()
        .await
        .remove(&runtime_session_key("default", "thread-goal-fetch-resume"));

    let _default_client = app_server_client(&state, "default").await.unwrap();
    let goal = fetch_session_goal_payload(&state, "default", "thread-goal-fetch-resume")
        .await
        .unwrap();

    assert_eq!(
        goal.get("objective").and_then(Value::as_str),
        Some("continue after restart")
    );
    assert_eq!(goal.get("status").and_then(Value::as_str), Some("active"));
    assert_eq!(
        state
            .session_app_server_assignments
            .lock()
            .await
            .get(&runtime_session_key("default", "thread-goal-fetch-resume"))
            .cloned(),
        Some("default::goal::thread-goal-fetch-resume".to_string())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_detail_uses_cached_goal_without_allocating_goal_app_server() {
    let sandbox = unique_test_dir("goal-detail-cache-without-dedicated-client");
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
                    "id": "thread-goal-detail-cache",
                    "name": "Cached goal detail",
                    "preview": "inspect cached goal",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 2,
                    "status": "idle",
                    "turns": []
                }
            }),
        )
        .await
        .unwrap();
    cache_session_goal_payload(
        &state,
        "default",
        "thread-goal-detail-cache",
        &json!({
            "threadId": "thread-goal-detail-cache",
            "objective": "keep detail reads lightweight",
            "status": "active",
            "tokenBudget": 5000
        }),
    )
    .await;

    let detail = session_detail_payload(&state, "default", "thread-goal-detail-cache", 20)
        .await
        .unwrap();

    assert_eq!(
        detail
            .get("goal")
            .and_then(|goal| goal.get("objective"))
            .and_then(Value::as_str),
        Some("keep detail reads lightweight")
    );
    assert!(
        state
            .session_app_server_assignments
            .lock()
            .await
            .get(&runtime_session_key("default", "thread-goal-detail-cache"))
            .is_none(),
        "session detail should not allocate a dedicated goal app-server"
    );
    assert_eq!(state.app_servers.client_count().await, 1);

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn goal_fetch_preserves_cached_goal_when_native_goal_is_missing() {
    let sandbox = unique_test_dir("goal-fetch-preserve-cache");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let goal_client = app_server_client_for_goal_session(&state, "default", "thread-goal-moved")
        .await
        .unwrap();
    goal_client
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": "thread-goal-moved",
                    "name": "Moved goal thread",
                    "preview": "resume moved goal",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 2,
                    "status": "idle",
                    "turns": []
                }
            }),
        )
        .await
        .unwrap();
    cache_session_goal_payload(
        &state,
        "default",
        "thread-goal-moved",
        &json!({
            "threadId": "thread-goal-moved",
            "objective": "continue moved goal",
            "status": "active",
            "tokenBudget": 5000,
            "tokensUsed": 120
        }),
    )
    .await;

    let goal = fetch_session_goal_payload(&state, "default", "thread-goal-moved")
        .await
        .unwrap();

    assert_eq!(
        goal.get("objective").and_then(Value::as_str),
        Some("continue moved goal")
    );
    assert_eq!(goal.get("status").and_then(Value::as_str), Some("active"));
    assert_eq!(
        cached_session_goal_or_null_payload(&state, "default", "thread-goal-moved")
            .await
            .get("objective")
            .and_then(Value::as_str),
        Some("continue moved goal")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn usage_limited_goal_fetch_uses_goal_app_server_after_restart_assignment_loss() {
    let sandbox = unique_test_dir("goal-fetch-usage-limited-dedicated-after-restart");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let goal_client = app_server_client_for_goal_session(
        &state,
        "default",
        "thread-goal-fetch-usage-limited-resume",
    )
    .await
    .unwrap();
    goal_client
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": "thread-goal-fetch-usage-limited-resume",
                    "name": "Usage-limited goal thread",
                    "preview": "resume usage-limited goal",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 2,
                    "status": "idle",
                    "turns": [],
                    "goal": {
                        "threadId": "thread-goal-fetch-usage-limited-resume",
                        "objective": "resume after usage reset",
                        "status": "usageLimited",
                        "createdAt": 1,
                        "updatedAt": 2
                    }
                }
            }),
        )
        .await
        .unwrap();
    cache_session_goal_payload(
        &state,
        "default",
        "thread-goal-fetch-usage-limited-resume",
        &json!({
            "threadId": "thread-goal-fetch-usage-limited-resume",
            "objective": "resume after usage reset",
            "status": "usageLimited"
        }),
    )
    .await;
    state
        .session_app_server_assignments
        .lock()
        .await
        .remove(&runtime_session_key(
            "default",
            "thread-goal-fetch-usage-limited-resume",
        ));

    let _default_client = app_server_client(&state, "default").await.unwrap();
    let goal =
        fetch_session_goal_payload(&state, "default", "thread-goal-fetch-usage-limited-resume")
            .await
            .unwrap();

    assert_eq!(
        goal.get("objective").and_then(Value::as_str),
        Some("resume after usage reset")
    );
    assert_eq!(
        goal.get("status").and_then(Value::as_str),
        Some("usageLimited")
    );
    assert_eq!(
        state
            .session_app_server_assignments
            .lock()
            .await
            .get(&runtime_session_key(
                "default",
                "thread-goal-fetch-usage-limited-resume"
            ))
            .cloned(),
        Some("default::goal::thread-goal-fetch-usage-limited-resume".to_string())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn budget_limited_goal_fetch_uses_goal_app_server_after_restart_assignment_loss() {
    let sandbox = unique_test_dir("goal-fetch-budget-limited-dedicated-after-restart");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let goal_client = app_server_client_for_goal_session(
        &state,
        "default",
        "thread-goal-fetch-budget-limited-resume",
    )
    .await
    .unwrap();
    goal_client
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": "thread-goal-fetch-budget-limited-resume",
                    "name": "Budget-limited goal thread",
                    "preview": "wrap up budget-limited goal",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 2,
                    "status": "idle",
                    "turns": [],
                    "goal": {
                        "threadId": "thread-goal-fetch-budget-limited-resume",
                        "objective": "wrap up after budget limit",
                        "status": "budgetLimited",
                        "createdAt": 1,
                        "updatedAt": 2
                    }
                }
            }),
        )
        .await
        .unwrap();
    cache_session_goal_payload(
        &state,
        "default",
        "thread-goal-fetch-budget-limited-resume",
        &json!({
            "threadId": "thread-goal-fetch-budget-limited-resume",
            "objective": "wrap up after budget limit",
            "status": "budgetLimited"
        }),
    )
    .await;
    state
        .session_app_server_assignments
        .lock()
        .await
        .remove(&runtime_session_key(
            "default",
            "thread-goal-fetch-budget-limited-resume",
        ));

    let _default_client = app_server_client(&state, "default").await.unwrap();
    let goal =
        fetch_session_goal_payload(&state, "default", "thread-goal-fetch-budget-limited-resume")
            .await
            .unwrap();

    assert_eq!(
        goal.get("objective").and_then(Value::as_str),
        Some("wrap up after budget limit")
    );
    assert_eq!(
        goal.get("status").and_then(Value::as_str),
        Some("budgetLimited")
    );
    assert_eq!(
        state
            .session_app_server_assignments
            .lock()
            .await
            .get(&runtime_session_key(
                "default",
                "thread-goal-fetch-budget-limited-resume"
            ))
            .cloned(),
        Some("default::goal::thread-goal-fetch-budget-limited-resume".to_string())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn created_session_goal_reuses_created_app_server_client() {
    let sandbox = unique_test_dir("goal-created-client");
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
        Some("Goal created session"),
    )
    .await
    .unwrap();
    let session_id = created.get("id").and_then(Value::as_str).unwrap();
    assert_eq!(
        state
            .session_app_server_assignments
            .lock()
            .await
            .get(&runtime_session_key("default", session_id))
            .cloned(),
        Some("default".to_string())
    );

    let goal = set_session_goal_payload(
        &state,
        "default",
        session_id,
        json!({
            "objective": "keep the created session on its existing runtime"
        }),
    )
    .await
    .unwrap();

    assert_eq!(state.app_servers.client_count().await, 1);
    assert_eq!(
        goal.get("goal")
            .and_then(|goal| goal.get("objective"))
            .and_then(Value::as_str),
        Some("keep the created session on its existing runtime")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn regular_session_turns_share_profile_app_server_by_default() {
    let sandbox = unique_test_dir("session-app-server-default");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let _first = app_server_client_for_session_turn(&state, "default", "thread-a")
        .await
        .unwrap();
    let _second = app_server_client_for_session_turn(&state, "default", "thread-b")
        .await
        .unwrap();

    assert_eq!(state.app_servers.client_count().await, 1);
    assert_eq!(
        state
            .session_app_server_assignments
            .lock()
            .await
            .get(&runtime_session_key("default", "thread-a"))
            .cloned(),
        Some("default".to_string())
    );
    assert_eq!(
        state
            .session_app_server_assignments
            .lock()
            .await
            .get(&runtime_session_key("default", "thread-b"))
            .cloned(),
        Some("default".to_string())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_regular_session_turns_use_dedicated_app_servers() {
    let sandbox = unique_test_dir("session-app-server-concurrent");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let _first = app_server_client_for_session_turn(&state, "default", "thread-a")
        .await
        .unwrap();
    mark_test_session_active(&state, "thread-a").await;
    let _second = app_server_client_for_session_turn(&state, "default", "thread-b")
        .await
        .unwrap();

    assert_eq!(state.app_servers.client_count().await, 2);
    assert_eq!(
        state
            .session_app_server_assignments
            .lock()
            .await
            .get(&runtime_session_key("default", "thread-a"))
            .cloned(),
        Some("default".to_string())
    );
    assert_eq!(
        state
            .session_app_server_assignments
            .lock()
            .await
            .get(&runtime_session_key("default", "thread-b"))
            .cloned(),
        Some("default::session::thread-b".to_string())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn per_session_app_server_option_allocates_regular_sessions_separately() {
    let sandbox = unique_test_dir("session-app-server-opt-in");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let mut state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    Arc::make_mut(&mut state.config).per_session_app_servers = true;
    let _first = app_server_client_for_session_turn(&state, "default", "thread-a")
        .await
        .unwrap();
    let _second = app_server_client_for_session_turn(&state, "default", "thread-b")
        .await
        .unwrap();

    assert_eq!(state.app_servers.client_count().await, 2);
    assert_eq!(
        state
            .session_app_server_assignments
            .lock()
            .await
            .get(&runtime_session_key("default", "thread-a"))
            .cloned(),
        Some("default::session::thread-a".to_string())
    );
    assert_eq!(
        state
            .session_app_server_assignments
            .lock()
            .await
            .get(&runtime_session_key("default", "thread-b"))
            .cloned(),
        Some("default::session::thread-b".to_string())
    );
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
    assert_eq!(
        cached_session_goal_or_null_payload(&state, "default", "thread-goal-1")
            .await
            .get("status")
            .and_then(Value::as_str),
        Some("paused")
    );
    let goal_client_key = state
        .session_app_server_assignments
        .lock()
        .await
        .get(&runtime_session_key("default", "thread-goal-1"))
        .cloned()
        .unwrap();
    let goal_client = app_server_client_by_key(&state, "default", &goal_client_key)
        .await
        .unwrap();
    let resume_count = goal_client
        .request("debug/requestCount", json!({ "target": "thread/resume" }))
        .await
        .unwrap();
    assert_eq!(
        resume_count.get("count").and_then(Value::as_u64),
        Some(1),
        "pausing an active goal must not start an extra continuation turn"
    );
    let blocked = set_session_goal_payload(
        &state,
        "default",
        "thread-goal-1",
        json!({
            "status": "blocked"
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        blocked
            .get("goal")
            .and_then(|goal| goal.get("status"))
            .and_then(Value::as_str),
        Some("blocked")
    );
    let usage_limited = set_session_goal_payload(
        &state,
        "default",
        "thread-goal-1",
        json!({
            "status": "usage_limited"
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        usage_limited
            .get("goal")
            .and_then(|goal| goal.get("status"))
            .and_then(Value::as_str),
        Some("usageLimited")
    );
    let budget_limited = set_session_goal_payload(
        &state,
        "default",
        "thread-goal-1",
        json!({
            "status": "budget-limited"
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        budget_limited
            .get("goal")
            .and_then(|goal| goal.get("status"))
            .and_then(Value::as_str),
        Some("budgetLimited")
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
    assert!(
        cached_session_goal_or_null_payload(&state, "default", "thread-goal-1")
            .await
            .is_null()
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_session_goal_payload_resumes_thread_before_goal_update() {
    let sandbox = unique_test_dir("thread-goal-resume-before-set");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let session_id = "thread-goal-set-resume";
    set_session_goal_payload(
        &state,
        "default",
        session_id,
        json!({
            "objective": "resume before native goal update"
        }),
    )
    .await
    .unwrap();

    let client_key = format!("default::goal::{session_id}");
    assert_eq!(
        state
            .session_app_server_assignments
            .lock()
            .await
            .get(&runtime_session_key("default", session_id))
            .cloned(),
        Some(client_key.clone())
    );
    let goal_client = app_server_client_by_key(&state, "default", &client_key)
        .await
        .unwrap();
    let request_log = goal_client
        .request("debug/requestLog", json!({}))
        .await
        .unwrap();
    let methods = request_log
        .get("methods")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let resume_pos = methods
        .iter()
        .position(|method| method.as_str() == Some("thread/resume"))
        .expect("goal set should resume the thread before updating goal state");
    let goal_set_pos = methods
        .iter()
        .position(|method| method.as_str() == Some("thread/goal/set"))
        .expect("goal set should proxy thread/goal/set after resume");
    assert!(
        resume_pos < goal_set_pos,
        "thread/resume must precede thread/goal/set: {request_log}"
    );
    assert!(
        !methods
            .iter()
            .any(|method| method.as_str() == Some("thread/read")),
        "goal setup should not rely on thread/read to materialize native thread state: {request_log}"
    );

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
    let _assigned_client =
        app_server_client_for_session_turn(&state, "default", "thread-goal-disabled")
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
    assert_eq!(count.get("count").and_then(Value::as_u64), Some(1));

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
    assert_eq!(
        event
            .get("params")
            .and_then(|params| params.get("threadId"))
            .and_then(Value::as_str),
        Some("thread-goal-2")
    );

    let cleared = map_app_server_session_notification(&AppServerNotification {
        method: "thread/goal/cleared".to_string(),
        params: json!({
            "thread_id": "thread-goal-2"
        }),
    })
    .unwrap();
    assert_eq!(
        cleared
            .get("params")
            .and_then(|params| params.get("threadId"))
            .and_then(Value::as_str),
        Some("thread-goal-2")
    );
    assert!(
        cleared
            .get("params")
            .and_then(|params| params.get("goal"))
            .is_some_and(Value::is_null)
    );
}

#[test]
fn silent_turn_completion_notifications_wait_for_history_backfill() {
    let event = map_app_server_session_notification(&AppServerNotification {
        method: "turn/completed".to_string(),
        params: json!({
            "turnId": "turn-silent",
            "turn": {
                "id": "turn-silent",
                "status": "completed",
                "items": [
                    {
                        "id": "turn-silent:user",
                        "type": "userMessage",
                        "text": "continue the previous work"
                    }
                ]
            }
        }),
    })
    .unwrap();
    let turn = event
        .get("params")
        .and_then(|params| params.get("turn"))
        .expect("mapped turn");

    assert_eq!(
        turn.get("status").and_then(Value::as_str),
        Some("completed")
    );
    assert!(turn.get("error").is_none_or(Value::is_null));
    assert_eq!(
        turn.get("items").and_then(Value::as_array).map(Vec::len),
        Some(1)
    );
}

#[test]
fn error_only_turn_completion_notifications_keep_visible_error_message() {
    let event = map_app_server_session_notification(&AppServerNotification {
        method: "turn/completed".to_string(),
        params: json!({
            "turnId": "turn-context-full",
            "turn": {
                "id": "turn-context-full",
                "status": "failed",
                "error": {
                    "codexErrorInfo": "contextWindowExceeded",
                    "message": "Codex ran out of room in the model's context window."
                },
                "items": [
                    {
                        "id": "turn-context-full:user",
                        "type": "userMessage",
                        "text": "계속 작업해"
                    }
                ]
            }
        }),
    })
    .unwrap();
    let turn = event
        .get("params")
        .and_then(|params| params.get("turn"))
        .expect("mapped turn");

    assert_eq!(turn.get("status").and_then(Value::as_str), Some("failed"));
    assert_eq!(
        turn.get("error")
            .and_then(|error| error.get("codexErrorInfo"))
            .and_then(Value::as_str),
        Some("contextWindowExceeded")
    );
    assert!(
        turn.get("items")
            .and_then(Value::as_array)
            .is_some_and(|items| items.iter().any(|item| {
                item.get("type").and_then(Value::as_str) == Some("agentMessage")
                    && item
                        .get("text")
                        .and_then(Value::as_str)
                        .is_some_and(|text| text.contains("context window"))
            }))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_goal_notifications_update_cached_goal_snapshot() {
    let sandbox = unique_test_dir("thread-goal-notification-cache");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    handle_profile_runtime_notification(
        &state,
        "default",
        &AppServerNotification {
            method: "thread/goal/updated".to_string(),
            params: json!({
                "threadId": "thread-goal-cache",
                "goal": {
                    "threadId": "thread-goal-cache",
                    "objective": "finish cached goal",
                    "status": "complete",
                    "tokensUsed": 42,
                    "timeUsedSeconds": 12,
                    "createdAt": 1,
                    "updatedAt": 2
                }
            }),
        },
    )
    .await;
    let cached = cached_session_goal_or_null_payload(&state, "default", "thread-goal-cache").await;
    assert_eq!(
        cached.get("status").and_then(Value::as_str),
        Some("complete")
    );

    handle_profile_runtime_notification(
        &state,
        "default",
        &AppServerNotification {
            method: "thread/goal/cleared".to_string(),
            params: json!({
                "threadId": "thread-goal-cache"
            }),
        },
    )
    .await;
    assert!(
        cached_session_goal_or_null_payload(&state, "default", "thread-goal-cache")
            .await
            .is_null()
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[test]
fn computer_frame_notifications_are_mapped_from_dynamic_tool_images() {
    let event = map_app_server_computer_frame_notification(&AppServerNotification {
        method: "item/completed".to_string(),
        params: json!({
            "threadId": "thread-computer",
            "turnId": "turn-1",
            "itemId": "item-computer-1",
            "item": {
                "id": "item-computer-1",
                "type": "dynamicToolCall",
                "namespace": "computer",
                "tool": "screenshot",
                "contentItems": [
                    {
                        "type": "inputImage",
                        "imageUrl": "data:image/avif;base64,AAAA"
                    }
                ]
            }
        }),
    })
    .expect("computer frame event should be mapped");

    assert_eq!(
        event.get("method").and_then(Value::as_str),
        Some("codex-webui/computerFrame")
    );
    assert_eq!(
        event
            .get("params")
            .and_then(|params| params.get("imageUrl"))
            .and_then(Value::as_str),
        Some("data:image/avif;base64,AAAA")
    );
    assert_eq!(
        event
            .get("params")
            .and_then(|params| params.get("mimeType"))
            .and_then(Value::as_str),
        Some("image/avif")
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
    state.active_turns.lock().await.insert(
        runtime_session_key("default", "thread-1"),
        "turn-queue-helper-test".to_string(),
    );

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
        Some("client-queue-first"),
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
        Some("client-queue-first"),
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
    assert_eq!(
        duplicate_first
            .get("items")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|item| item.get("clientUserMessageId"))
            .and_then(Value::as_str),
        Some("client-queue-first")
    );
    let repeated_first = enqueue_session_queue_payload(
        &state,
        "default",
        "thread-1",
        "first",
        Some("request-first-repeat"),
        None,
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
    let second = enqueue_session_queue_payload(
        &state, "default", "thread-1", "second", None, None, None, None,
    )
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
async fn dispatch_queue_item_returns_current_queue_when_dispatch_is_busy() {
    let sandbox = unique_test_dir("queue-dispatch-busy");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    with_ui_state_write(&state, "default", |ui_state| {
        ui_state["queuesByThreadId"]["thread-1"] = json!({
            "items": [
                {
                    "id": "queue-1",
                    "prompt": "follow up without surfacing dispatch contention",
                    "attachmentIds": [],
                    "attachmentNames": [],
                    "createdAt": 15
                }
            ],
            "resumePending": false,
            "updatedAt": 20
        });
        Ok(())
    })
    .await
    .unwrap();

    let state_for_guard = state.clone();
    let guard = tokio::spawn(async move {
        with_queue_dispatch_guard(&state_for_guard, "default", "thread-1", async {
            tokio::time::sleep(Duration::from_millis(150)).await;
        })
        .await;
    });
    tokio::time::sleep(Duration::from_millis(20)).await;

    let queue = dispatch_session_queue_item_payload(
        &state, "default", "thread-1", "queue-1", "message", None,
    )
    .await
    .unwrap();

    assert_eq!(
        queue.get("items").and_then(Value::as_array).map(Vec::len),
        Some(1)
    );
    assert_eq!(
        queue
            .get("dispatchAlreadyInProgress")
            .and_then(Value::as_bool),
        Some(true)
    );
    guard.await.unwrap();

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn queue_mutations_reject_only_the_item_claimed_for_dispatch() {
    let sandbox = unique_test_dir("queue-item-dispatch-claim");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    with_ui_state_write(&state, "default", |ui_state| {
        ui_state["queuesByThreadId"]["thread-1"] = json!({
            "items": [
                { "id": "queue-dispatching", "prompt": "first", "createdAt": 1 },
                { "id": "queue-second", "prompt": "second", "createdAt": 2 },
                { "id": "queue-third", "prompt": "third", "createdAt": 3 },
                { "id": "queue-fourth", "prompt": "fourth", "createdAt": 4 }
            ],
            "resumePending": false,
            "updatedAt": 5
        });
        Ok(())
    })
    .await
    .unwrap();

    let claimed =
        claim_session_queue_item_for_dispatch(&state, "default", "thread-1", "queue-dispatching")
            .await
            .unwrap();
    assert_eq!(
        claimed.get("status").and_then(Value::as_str),
        Some("dispatching")
    );
    assert!(
        claimed
            .get("dispatchingAt")
            .and_then(Value::as_u64)
            .is_some()
    );

    let update_error = update_session_queue_item_payload(
        &state,
        "default",
        "thread-1",
        "queue-dispatching",
        Some("stale edit"),
        None,
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(update_error.status, StatusCode::CONFLICT);
    assert_eq!(update_error.message, "QUEUE_ALREADY_DISPATCHING");

    let remove_error =
        remove_session_queue_item_payload(&state, "default", "thread-1", "queue-dispatching")
            .await
            .unwrap_err();
    assert_eq!(remove_error.status, StatusCode::CONFLICT);
    assert_eq!(remove_error.message, "QUEUE_ALREADY_DISPATCHING");

    let move_dispatching_error = reorder_session_queue_payload(
        &state,
        "default",
        "thread-1",
        &[
            "queue-second".to_string(),
            "queue-dispatching".to_string(),
            "queue-third".to_string(),
            "queue-fourth".to_string(),
        ],
    )
    .await
    .unwrap_err();
    assert_eq!(move_dispatching_error.status, StatusCode::CONFLICT);
    assert_eq!(move_dispatching_error.message, "QUEUE_ALREADY_DISPATCHING");

    let reordered = reorder_session_queue_payload(
        &state,
        "default",
        "thread-1",
        &[
            "queue-dispatching".to_string(),
            "queue-fourth".to_string(),
            "queue-second".to_string(),
            "queue-third".to_string(),
        ],
    )
    .await
    .unwrap();
    let reordered_ids = reordered
        .get("items")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .filter_map(|item| item.get("id").and_then(Value::as_str))
        .collect::<Vec<_>>();
    assert_eq!(
        reordered_ids,
        vec![
            "queue-dispatching",
            "queue-fourth",
            "queue-second",
            "queue-third"
        ]
    );

    let updated = update_session_queue_item_payload(
        &state,
        "default",
        "thread-1",
        "queue-second",
        Some("second updated while first dispatches"),
        None,
        None,
    )
    .await
    .unwrap();
    assert!(
        updated
            .get("items")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .any(|item| {
                item.get("id").and_then(Value::as_str) == Some("queue-second")
                    && item.get("prompt").and_then(Value::as_str)
                        == Some("second updated while first dispatches")
            })
    );

    let removed = remove_session_queue_item_payload(&state, "default", "thread-1", "queue-third")
        .await
        .unwrap();
    assert_eq!(
        removed.get("items").and_then(Value::as_array).map(Vec::len),
        Some(3)
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_queue_dispatch_resumes_remaining_items_after_turn_completion() {
    let sandbox = unique_test_dir("queue-manual-dispatch-resumes");
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
        Some("Queue manual dispatch"),
    )
    .await
    .unwrap();
    let session_id = created.get("id").and_then(Value::as_str).unwrap();
    with_ui_state_write(&state, "default", |ui_state| {
        ui_state["queuesByThreadId"][session_id] = json!({
            "items": [
                {
                    "id": "queue-1",
                    "prompt": "first manual follow-up",
                    "attachmentIds": [],
                    "attachmentNames": [],
                    "createdAt": 15
                },
                {
                    "id": "queue-2",
                    "prompt": "second automatic follow-up",
                    "attachmentIds": [],
                    "attachmentNames": [],
                    "createdAt": 16
                }
            ],
            "resumePending": false,
            "updatedAt": 20
        });
        Ok(())
    })
    .await
    .unwrap();

    let queue = dispatch_session_queue_item_payload(
        &state, "default", session_id, "queue-1", "message", None,
    )
    .await
    .unwrap();
    assert_eq!(
        queue.get("items").and_then(Value::as_array).map(Vec::len),
        Some(1)
    );
    let active_turn_id = state
        .active_turns
        .lock()
        .await
        .get(&runtime_session_key("default", session_id))
        .cloned()
        .expect("manual dispatch should mark the first turn active");
    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": session_id,
                    "name": "Queue manual dispatch",
                    "preview": "first manual follow-up",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 2,
                    "status": "completed",
                    "isSubagent": false,
                    "agentNickname": Value::Null,
                    "agentRole": Value::Null,
                    "turns": [
                        {
                            "id": active_turn_id,
                            "status": "completed",
                            "items": []
                        }
                    ]
                }
            }),
        )
        .await
        .unwrap();
    handle_profile_runtime_notification(
        &state,
        "default",
        &AppServerNotification {
            method: "turn/completed".to_string(),
            params: json!({
                "threadId": session_id,
                "turnId": active_turn_id,
                "turn": {
                    "id": active_turn_id,
                    "status": "completed",
                    "items": []
                }
            }),
        },
    )
    .await;

    for _ in 0..80 {
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

    let queue = get_session_queue_payload(&state, "default", session_id)
        .await
        .unwrap();
    assert_eq!(
        queue.get("items").and_then(Value::as_array).map(Vec::len),
        Some(0)
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queue_drain_recovers_orphaned_dispatch_claim_without_manual_resume() {
    let sandbox = unique_test_dir("queue-recovers-orphaned-dispatch-claim");
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
        Some("Recover orphaned queue dispatch"),
    )
    .await
    .unwrap();
    let session_id = created.get("id").and_then(Value::as_str).unwrap();
    with_ui_state_write(&state, "default", |ui_state| {
        ui_state["queuesByThreadId"][session_id] = json!({
            "items": [
                {
                    "id": "queue-orphaned",
                    "prompt": "Continue without requiring Resume queue.",
                    "attachmentIds": [],
                    "attachmentNames": [],
                    "status": "dispatching",
                    "dispatchingAt": 10,
                    "createdAt": 10
                },
                {
                    "id": "queue-follow-up",
                    "prompt": "Send the next item automatically.",
                    "attachmentIds": [],
                    "attachmentNames": [],
                    "createdAt": 11
                }
            ],
            "resumePending": true,
            "updatedAt": 20
        });
        Ok(())
    })
    .await
    .unwrap();

    maybe_drain_queue(&state, "default", session_id).await;

    let active_turn_id = state
        .active_turns
        .lock()
        .await
        .get(&runtime_session_key("default", session_id))
        .cloned()
        .expect("the recovered first queue item should start a turn");
    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": session_id,
                    "name": "Recover orphaned queue dispatch",
                    "preview": "Continue without requiring Resume queue.",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 2,
                    "status": "completed",
                    "isSubagent": false,
                    "agentNickname": Value::Null,
                    "agentRole": Value::Null,
                    "turns": [{
                        "id": active_turn_id,
                        "status": "completed",
                        "items": []
                    }]
                }
            }),
        )
        .await
        .unwrap();

    for _ in 0..80 {
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

    let queue = get_session_queue_payload(&state, "default", session_id)
        .await
        .unwrap();
    assert_eq!(
        queue.get("items").and_then(Value::as_array).map(Vec::len),
        Some(0)
    );
    assert_eq!(
        queue.get("resumeRequired").and_then(Value::as_bool),
        Some(false)
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
        Some("Send the next item automatically.")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ws_queue_dispatch_uses_requested_session_profile() {
    let sandbox = unique_test_dir("queue-dispatch-profile-routing");
    let workspace = sandbox.join("workspace");
    let default_codex_home = sandbox.join("codex-default");
    let second_codex_home = sandbox.join("codex-second");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&default_codex_home).unwrap();
    fs::create_dir_all(&second_codex_home).unwrap();

    let mut state = test_state_with_fake_app_server(
        workspace.clone(),
        vec![workspace.clone()],
        default_codex_home,
    );
    let mut config = (*state.config).clone();
    config.config_file_path = Some(sandbox.join("isolated-codex-webui.yml"));
    config.profiles.insert(
        "second".to_string(),
        RuntimeProfile {
            label: "Second".to_string(),
            codex_home: second_codex_home,
            data_dir: sandbox.join(".data").join("profiles").join("second"),
        },
    );
    state.config = Arc::new(config);

    let session_id = "thread-second-profile-queue";
    app_server_client(&state, "second")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": session_id,
                    "name": "Second profile queue",
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
    with_ui_state_write(&state, "second", |ui_state| {
        ui_state["queuesByThreadId"][session_id] = json!({
            "items": [
                {
                    "id": "queue-second",
                    "prompt": "Dispatch in the second profile",
                    "attachmentIds": [],
                    "attachmentNames": [],
                    "createdAt": 15
                }
            ],
            "resumePending": false,
            "updatedAt": 20
        });
        Ok(())
    })
    .await
    .unwrap();

    let (out_tx, _out_rx) = mpsc::channel(8);
    let subscriptions: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let auth = AuthContext {
        profile_id: "default".to_string(),
        role: UserRole::Admin,
    };
    let queue = execute_ws_method(
        &state,
        &out_tx,
        &subscriptions,
        &auth,
        "session/queue/dispatch",
        json!({
            "sessionId": session_id,
            "profileId": "second",
            "queueId": "queue-second",
            "mode": "message"
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        queue.get("items").and_then(Value::as_array).map(Vec::len),
        Some(0)
    );
    let thread = read_thread_payload(&state, "second", session_id, true)
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
        Some("Dispatch in the second profile")
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

    for _ in 0..80 {
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
async fn send_turn_payload_is_idempotent_for_client_user_message_id() {
    let sandbox = unique_test_dir("send-turn-client-id-idempotent");
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
        Some("Idempotent send"),
    )
    .await
    .unwrap();
    let session_id = created.get("id").and_then(Value::as_str).unwrap();
    let preferences = json!({
        "cwd": workspace.display().to_string(),
        "model": "gpt-5.4"
    });

    let first = send_turn_payload(
        &state,
        "default",
        session_id,
        "Run this once.",
        None,
        None,
        preferences.clone(),
        Some("client-send-once"),
    )
    .await
    .unwrap();
    assert_ne!(first.get("duplicate").and_then(Value::as_bool), Some(true));

    let second = send_turn_payload(
        &state,
        "default",
        session_id,
        "Run this once.",
        None,
        None,
        preferences,
        Some("client-send-once"),
    )
    .await
    .unwrap();
    assert_eq!(second.get("duplicate").and_then(Value::as_bool), Some(true));

    let thread = read_thread_payload(&state, "default", session_id, true)
        .await
        .unwrap();
    let turns = thread
        .get("turns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        turns.len(),
        1,
        "same client id must not create a second turn"
    );
    assert_eq!(
        turns
            .first()
            .and_then(|turn| turn.get("items"))
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|item| item.get("clientUserMessageId"))
            .and_then(Value::as_str),
        Some("client-send-once")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_turn_payload_does_not_read_long_thread_before_first_send() {
    let sandbox = unique_test_dir("turn-send-no-preflight-thread-read");
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
            "model": "gpt-5"
        }),
        None,
        Some("Slow history should not block send"),
    )
    .await
    .unwrap();
    let session_id = created
        .get("id")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();
    let client = app_server_client(&state, "default").await.unwrap();
    client
        .request(
            "debug/setDelay",
            json!({
                "method": "thread/read",
                "delayMs": 4000
            }),
        )
        .await
        .unwrap();
    let baseline_read_count = client
        .request("debug/requestCount", json!({ "target": "thread/read" }))
        .await
        .unwrap()
        .get("count")
        .and_then(Value::as_u64)
        .unwrap_or_default();

    let started_at = Instant::now();
    let payload = send_turn_payload(
        &state,
        "default",
        &session_id,
        "Send immediately even when full history reads are slow.",
        None,
        None,
        json!({
            "cwd": workspace.display().to_string(),
            "model": "gpt-5"
        }),
        Some("client-no-preflight-read"),
    )
    .await
    .unwrap();

    assert_eq!(
        payload.get("turnId").and_then(Value::as_str),
        Some("turn-1")
    );
    assert!(
        started_at.elapsed() < Duration::from_millis(2500),
        "turn/send waited for a slow thread/read preflight"
    );
    let request_count = client
        .request("debug/requestCount", json!({ "target": "thread/read" }))
        .await
        .unwrap();
    let request_log = client.request("debug/requestLog", json!({})).await.unwrap();
    assert_eq!(
        request_count.get("count").and_then(Value::as_u64),
        Some(baseline_read_count),
        "unexpected thread/read request log: {request_log}"
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_turn_payload_resumes_thread_before_starting_turn() {
    let sandbox = unique_test_dir("turn-send-resumes-before-start");
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
            "model": "gpt-5"
        }),
        None,
        Some("Resume before send"),
    )
    .await
    .unwrap();
    let session_id = created.get("id").and_then(Value::as_str).unwrap();
    let client = app_server_client(&state, "default").await.unwrap();
    let baseline_log = client.request("debug/requestLog", json!({})).await.unwrap();
    let baseline_len = baseline_log
        .get("methods")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();

    send_turn_payload(
        &state,
        "default",
        session_id,
        "Continue after app-server resume.",
        None,
        None,
        json!({
            "cwd": workspace.display().to_string(),
            "model": "gpt-5"
        }),
        Some("client-resume-before-start"),
    )
    .await
    .unwrap();

    let request_log = client.request("debug/requestLog", json!({})).await.unwrap();
    let methods = request_log
        .get("methods")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let methods_after_send = &methods[baseline_len..];
    let resume_pos = methods_after_send
        .iter()
        .position(|method| method.as_str() == Some("thread/resume"))
        .expect("send should resume the thread before starting a turn");
    let turn_start_pos = methods_after_send
        .iter()
        .position(|method| method.as_str() == Some("turn/start"))
        .expect("send should start a turn after resuming");
    assert!(
        resume_pos < turn_start_pos,
        "thread/resume must precede turn/start in request log: {request_log}"
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queue_dispatch_resumes_thread_before_starting_turn() {
    let sandbox = unique_test_dir("queue-dispatch-resumes-before-start");
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
            "model": "gpt-5"
        }),
        None,
        Some("Queue resume before send"),
    )
    .await
    .unwrap();
    let session_id = created.get("id").and_then(Value::as_str).unwrap();
    with_ui_state_write(&state, "default", |ui_state| {
        ui_state["queuesByThreadId"][session_id] = json!({
            "items": [
                {
                    "id": "queue-resume",
                    "prompt": "Dispatch through the queue after app-server resume.",
                    "attachmentIds": [],
                    "attachmentNames": [],
                    "createdAt": 15
                }
            ],
            "resumePending": false,
            "updatedAt": 20
        });
        Ok(())
    })
    .await
    .unwrap();
    let client = app_server_client(&state, "default").await.unwrap();
    let baseline_log = client.request("debug/requestLog", json!({})).await.unwrap();
    let baseline_len = baseline_log
        .get("methods")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();

    dispatch_session_queue_item_payload(
        &state,
        "default",
        session_id,
        "queue-resume",
        "message",
        None,
    )
    .await
    .unwrap();

    let request_log = client.request("debug/requestLog", json!({})).await.unwrap();
    let methods = request_log
        .get("methods")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let methods_after_dispatch = &methods[baseline_len..];
    let resume_pos = methods_after_dispatch
        .iter()
        .position(|method| method.as_str() == Some("thread/resume"))
        .expect("queue dispatch should resume the thread before starting a turn");
    let turn_start_pos = methods_after_dispatch
        .iter()
        .position(|method| method.as_str() == Some("turn/start"))
        .expect("queue dispatch should start a turn after resuming");
    assert!(
        resume_pos < turn_start_pos,
        "thread/resume must precede turn/start in queue request log: {request_log}"
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
        None,
    )
    .await
    .unwrap();

    assert!(
        started.elapsed() < Duration::from_millis(500),
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
async fn queue_drain_recovers_after_repeated_activity_probe_timeouts() {
    let sandbox = unique_test_dir("queue-recovers-after-activity-timeouts");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let session_id = "thread-activity-probe-timeout";
    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": session_id,
                    "name": "Activity probe timeout",
                    "preview": "",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 1,
                    "status": "running",
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
    with_ui_state_write(&state, "default", |ui_state| {
        ui_state["queuesByThreadId"][session_id] = json!({
            "items": [
                {
                    "id": "queue-1",
                    "prompt": "recover after repeated activity probe timeouts",
                    "attachmentIds": [],
                    "attachmentNames": [],
                    "createdAt": 15
                }
            ],
            "resumePending": false,
            "updatedAt": 20
        });
        ui_state["runtimeStatusByThreadId"][session_id] = json!({
            "status": "running",
            "updatedAt": now_unix_ms().saturating_sub(120_000)
        });
        Ok(())
    })
    .await
    .unwrap();
    state.active_turns.lock().await.insert(
        runtime_session_key("default", session_id),
        "stale-turn".to_string(),
    );

    maybe_drain_queue(&state, "default", session_id).await;
    tokio::time::sleep(Duration::from_millis(3000)).await;

    let queue = get_session_queue_payload(&state, "default", session_id)
        .await
        .unwrap();
    assert_eq!(
        queue.get("items").and_then(Value::as_array).map(Vec::len),
        Some(0),
        "queue did not drain: {queue}"
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
        Some("recover after repeated activity probe timeouts")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queue_drain_waits_when_app_server_reports_running_without_turn_id() {
    let sandbox = unique_test_dir("queue-running-without-turn-id");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let session_id = "thread-running-without-turn-id";
    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": session_id,
                    "name": "Running without turn id",
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
    with_ui_state_write(&state, "default", |ui_state| {
        ui_state["queuesByThreadId"][session_id] = json!({
            "items": [
                {
                    "id": "queue-1",
                    "prompt": "wait for the running turn",
                    "attachmentIds": [],
                    "attachmentNames": [],
                    "createdAt": 15
                }
            ],
            "resumePending": false,
            "updatedAt": 20
        });
        Ok(())
    })
    .await
    .unwrap();

    maybe_drain_queue(&state, "default", session_id).await;
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
                    "status": "completed",
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
        Some("client-queued-dispatch"),
        None,
        None,
    )
    .await
    .unwrap();

    for _ in 0..80 {
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
    assert_eq!(
        thread
            .get("lastTurnStart")
            .and_then(|value| value.get("clientUserMessageId"))
            .and_then(Value::as_str),
        Some("client-queued-dispatch")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queue_drain_clears_stale_pending_start_without_app_server_and_dispatches() {
    let sandbox = unique_test_dir("queue-stale-pending-no-app-server");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let session_id = "thread-stale-pending-no-app-server";
    let runtime_key = runtime_session_key("default", session_id);
    state
        .pending_turn_starts
        .lock()
        .await
        .insert(runtime_key.clone());
    with_ui_state_write(&state, "default", |ui_state| {
        ui_state["runtimeStatusByThreadId"][session_id] = json!({
            "status": "running",
            "updatedAt": now_unix_ms().saturating_sub(60_000)
        });
        ui_state["highlightsByThreadId"][session_id] = json!({
            "kind": "attention",
            "at": 1,
            "reason": "failed"
        });
        ui_state["queuesByThreadId"][session_id] = json!({
            "items": [
                {
                    "id": "queue-1",
                    "prompt": "Run after the crashed app-server state is cleared.",
                    "attachmentIds": [],
                    "attachmentNames": [],
                    "createdAt": 15
                }
            ],
            "resumePending": false,
            "updatedAt": 20
        });
        Ok(())
    })
    .await
    .unwrap();

    maybe_drain_queue(&state, "default", session_id).await;

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
    assert!(
        state
            .pending_turn_starts
            .lock()
            .await
            .get(&runtime_key)
            .is_none()
    );
    let ui_state = with_ui_state_read(&state, "default", |ui_state| Ok(ui_state.clone()))
        .await
        .unwrap();
    assert!(ui_state["highlightsByThreadId"].get(session_id).is_none());

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
        Some("Run after the crashed app-server state is cleared.")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queue_drain_ignores_orphaned_local_active_tail_without_cached_runtime_activity() {
    let sandbox = unique_test_dir("queue-orphaned-local-active-tail");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let session_id = "thread-orphaned-local-active-tail";
    let rollout_dir = codex_home
        .join("sessions")
        .join("2026")
        .join("04")
        .join("24");
    fs::create_dir_all(&rollout_dir).unwrap();
    fs::write(
        rollout_dir.join(format!("rollout-2026-04-24T01-12-00-{session_id}.jsonl")),
        format!(
            "{}\n{}\n{}\n",
            json!({
                "timestamp": "2026-04-24T01:12:00.000Z",
                "type": "session_meta",
                "payload": {
                    "id": session_id,
                    "timestamp": "2026-04-24T01:12:00.000Z",
                    "cwd": workspace.display().to_string()
                }
            }),
            json!({
                "timestamp": "2026-04-24T01:12:01.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "task_started",
                    "turn_id": "turn-orphaned"
                }
            }),
            json!({
                "timestamp": "2026-04-24T01:12:02.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "user_message",
                    "message": "this turn was interrupted before completion"
                }
            })
        ),
    )
    .unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": session_id,
                    "name": "Orphaned local active tail",
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
    enqueue_session_queue_payload(
        &state,
        "default",
        session_id,
        "Dispatch after ignoring the stale local active tail.",
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    maybe_drain_queue(&state, "default", session_id).await;

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
        Some("Dispatch after ignoring the stale local active tail.")
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queue_drain_persists_dispatch_failure_on_item() {
    let sandbox = unique_test_dir("queue-dispatch-failure-state");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let session_id = "thread-queue-dispatch-fails";
    let client = app_server_client(&state, "default").await.unwrap();
    client
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": session_id,
                    "name": "Queue dispatch failure",
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
    client
        .request(
            "debug/setError",
            json!({
                "method": "turn/start",
                "message": "queued turn rejected"
            }),
        )
        .await
        .unwrap();

    with_ui_state_write(&state, "default", |ui_state| {
        ui_state["queuesByThreadId"][session_id] = json!({
            "items": [
                {
                    "id": "queue-failed",
                    "prompt": "This queued item should be marked failed.",
                    "attachmentIds": [],
                    "attachmentNames": [],
                    "createdAt": 15
                }
            ],
            "resumePending": false,
            "updatedAt": 20
        });
        Ok(())
    })
    .await
    .unwrap();

    maybe_drain_queue(&state, "default", session_id).await;

    let queue = get_session_queue_payload(&state, "default", session_id)
        .await
        .unwrap();
    assert_eq!(
        queue.get("resumeRequired").and_then(Value::as_bool),
        Some(true)
    );
    let item = queue
        .get("items")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .expect("failed queue item should remain visible");
    assert_eq!(item.get("id").and_then(Value::as_str), Some("queue-failed"));
    assert_eq!(item.get("status").and_then(Value::as_str), Some("failed"));
    assert!(item.get("dispatchingAt").is_none());
    assert!(item.get("failedAt").and_then(Value::as_u64).is_some());
    assert!(
        item.get("error")
            .and_then(Value::as_str)
            .is_some_and(|message| message.contains("queued turn rejected"))
    );

    let thread = read_thread_payload(&state, "default", session_id, true)
        .await
        .unwrap();
    assert_eq!(
        thread.get("turns").and_then(Value::as_array).map(Vec::len),
        Some(0)
    );

    let ui_state = with_ui_state_read(&state, "default", |ui_state| Ok(ui_state.clone()))
        .await
        .unwrap();
    let notifications = ui_state["notifications"]["items"]
        .as_array()
        .expect("notifications should be available");
    assert!(notifications.iter().any(|notification| {
        notification.get("type").and_then(Value::as_str) == Some("queueDispatchFailed")
            && notification.get("sessionId").and_then(Value::as_str) == Some(session_id)
            && notification
                .get("payload")
                .and_then(|payload| payload.get("queueId"))
                .and_then(Value::as_str)
                == Some("queue-failed")
    }));

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

#[test]
fn codex_thread_statuses_map_to_stable_webui_states() {
    assert_eq!(runtime_status_from_codex_thread_status("active"), "running");
    assert_eq!(runtime_status_from_codex_thread_status("idle"), "completed");
    assert_eq!(
        runtime_status_from_codex_thread_status("systemError"),
        "failed"
    );
    assert_eq!(
        runtime_status_from_codex_thread_status("notLoaded"),
        "stopped"
    );
}

#[test]
fn empty_completed_notification_waits_for_authoritative_history() {
    let event = map_app_server_session_notification(&AppServerNotification {
        method: "turn/completed".to_string(),
        params: json!({
            "threadId": "thread-empty-completion",
            "turnId": "turn-empty-completion",
            "turn": {
                "id": "turn-empty-completion",
                "status": "completed",
                "items": []
            }
        }),
    })
    .unwrap();
    let turn = &event["params"]["turn"];
    assert_eq!(turn["status"].as_str(), Some("completed"));
    assert!(turn.get("error").is_none_or(Value::is_null));
    assert!(turn["items"].as_array().is_some_and(Vec::is_empty));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idle_reconciliation_completes_instead_of_failing_session() {
    let sandbox = unique_test_dir("idle-runtime-reconcile");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let session_id = "thread-idle-runtime-reconcile";
    let client = app_server_client(&state, "default").await.unwrap();
    client
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": session_id,
                    "name": "Idle is complete",
                    "preview": "",
                    "cwd": workspace,
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 2,
                    "status": "idle",
                    "isSubagent": false,
                    "turns": []
                }
            }),
        )
        .await
        .unwrap();
    let runtime_key = runtime_session_key("default", session_id);
    state
        .active_turns
        .lock()
        .await
        .insert(runtime_key, "turn-stale".to_string());
    set_runtime_session_status(&state, "default", session_id, "running").await;

    let reconciled = reconcile_lost_runtime_activity_for_profile(&state, "default").await;
    assert_eq!(reconciled, vec![session_id.to_string()]);
    let status = with_ui_state_read(&state, "default", |ui_state| {
        Ok(ui_state["runtimeStatusByThreadId"][session_id]["status"].clone())
    })
    .await
    .unwrap();
    assert_eq!(status.as_str(), Some("completed"));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_delta_recovers_runtime_without_cancelling_scheduled_shutdown() {
    let sandbox = unique_test_dir("non-live-runtime-events");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    let session_id = "thread-non-live-events";
    set_runtime_session_status(&state, "default", session_id, "completed").await;
    set_session_highlight(
        &state,
        "default",
        session_id,
        Some(json!({ "kind": "attention", "reason": "approval", "at": 1 })),
    )
    .await;
    handle_profile_runtime_notification(
        &state,
        "default",
        &AppServerNotification {
            method: "item/agentMessage/delta".to_string(),
            params: json!({
                "threadId": session_id,
                "turnId": "turn-1",
                "itemId": "item-1",
                "delta": "first"
            }),
        },
    )
    .await;
    with_ui_state_write(&state, "default", |ui_state| {
        ui_state["global"]["scheduledShutdown"] = json!({
            "scheduledFor": now_unix_ms() + 60_000,
            "reason": "test"
        });
        Ok(())
    })
    .await
    .unwrap();
    let before = with_ui_state_read(&state, "default", |ui_state| {
        Ok(json!({
            "runtime": ui_state["runtimeStatusByThreadId"][session_id].clone(),
            "scheduledShutdown": ui_state["global"]["scheduledShutdown"].clone()
        }))
    })
    .await
    .unwrap();

    for notification in [
        AppServerNotification {
            method: "item/agentMessage/delta".to_string(),
            params: json!({
                "threadId": session_id,
                "turnId": "turn-1",
                "itemId": "item-1",
                "delta": "second"
            }),
        },
        AppServerNotification {
            method: "thread/tokenUsage/updated".to_string(),
            params: json!({ "threadId": session_id, "tokenUsage": { "totalTokens": 10 } }),
        },
        AppServerNotification {
            method: "thread/goal/updated".to_string(),
            params: json!({
                "threadId": session_id,
                "goal": { "threadId": session_id, "objective": "stay terminal", "status": "complete" }
            }),
        },
    ] {
        handle_profile_runtime_notification(&state, "default", &notification).await;
    }

    let after = with_ui_state_read(&state, "default", |ui_state| {
        Ok(json!({
            "runtime": ui_state["runtimeStatusByThreadId"][session_id].clone(),
            "highlight": ui_state["highlightsByThreadId"][session_id].clone(),
            "scheduledShutdown": ui_state["global"]["scheduledShutdown"].clone()
        }))
    })
    .await
    .unwrap();
    assert_eq!(after["runtime"]["status"].as_str(), Some("running"));
    assert_eq!(after["scheduledShutdown"], before["scheduledShutdown"]);
    assert_eq!(after["highlight"]["reason"].as_str(), Some("approval"));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delayed_running_write_cannot_resurrect_completed_compaction() {
    let sandbox = unique_test_dir("compact-completion-race");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    let session_id = "thread-compact-completion-race";

    set_runtime_session_status(&state, "default", session_id, "starting").await;
    set_runtime_session_status(&state, "default", session_id, "completed").await;
    set_runtime_session_status(&state, "default", session_id, "running").await;

    let status = with_ui_state_read(&state, "default", |ui_state| {
        Ok(ui_state["runtimeStatusByThreadId"][session_id]["status"].clone())
    })
    .await
    .unwrap();
    assert_eq!(status.as_str(), Some("completed"));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolved_notification_tombstones_late_server_request() {
    let sandbox = unique_test_dir("resolved-before-request");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let session_id = "thread-resolved-before-request";
    mark_test_session_active(&state, session_id).await;

    handle_profile_runtime_notification(
        &state,
        "default",
        &AppServerNotification {
            method: "serverRequest/resolved".to_string(),
            params: json!({ "threadId": session_id, "requestId": "request-race" }),
        },
    )
    .await;
    handle_profile_server_request(
        &state,
        "default",
        "default",
        &backend::codex_app_server::AppServerRequest {
            id: json!("request-race"),
            method: "item/tool/requestUserInput".to_string(),
            params: json!({ "threadId": session_id, "turnId": "turn-1", "questions": [] }),
        },
    )
    .await;

    assert!(
        state
            .pending_server_requests
            .lock()
            .await
            .get(&runtime_session_key("default", session_id))
            .is_none()
    );
    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_input_auto_resolution_clears_pending_request() {
    let sandbox = unique_test_dir("request-auto-resolution");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let session_id = "thread-auto-resolution";
    mark_test_session_active(&state, session_id).await;
    handle_profile_server_request(
        &state,
        "default",
        "default",
        &backend::codex_app_server::AppServerRequest {
            id: json!("auto-resolve-request"),
            method: "item/tool/requestUserInput".to_string(),
            params: json!({
                "threadId": session_id,
                "turnId": "turn-1",
                "questions": [],
                "autoResolutionMs": 10
            }),
        },
    )
    .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        state
            .pending_server_requests
            .lock()
            .await
            .get(&runtime_session_key("default", session_id))
            .is_none()
    );
    let _ = fs::remove_dir_all(sandbox);
}

#[test]
fn manual_permission_decisions_use_native_response_shape() {
    let pending = PendingServerRequestEntry {
        raw_id: json!("permissions-1"),
        client_key: "default".to_string(),
        method: "item/permissions/requestApproval".to_string(),
        params: json!({
            "permissions": {
                "fileSystem": { "entries": [] },
                "network": { "enabled": true }
            }
        }),
        created_at: "2026-07-10T00:00:00Z".to_string(),
        created_at_ms: 1,
    };
    let accepted =
        normalize_server_request_response(&pending, json!({ "decision": "acceptForSession" }));
    assert_eq!(accepted["scope"].as_str(), Some("session"));
    assert_eq!(accepted["permissions"], pending.params["permissions"]);
    let declined = normalize_server_request_response(&pending, json!({ "decision": "decline" }));
    assert_eq!(declined, json!({ "permissions": {}, "scope": "turn" }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn profile_maintenance_monitor_is_registered_once_per_profile() {
    let sandbox = unique_test_dir("single-profile-maintenance-monitor");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    let (notifications_a, receiver_a) = broadcast::channel(4);
    let (requests_a, request_receiver_a) = broadcast::channel(4);
    let (notifications_b, receiver_b) = broadcast::channel(4);
    let (requests_b, request_receiver_b) = broadcast::channel(4);

    register_runtime_profile_monitor(
        &state,
        "default",
        "client-a",
        receiver_a,
        request_receiver_a,
    );
    register_runtime_profile_monitor(
        &state,
        "default",
        "client-b",
        receiver_b,
        request_receiver_b,
    );
    let maintenance_count = state
        .runtime_profile_monitors
        .lock()
        .unwrap()
        .keys()
        .filter(|key| key.ends_with("::__profile_maintenance"))
        .count();
    assert_eq!(maintenance_count, 1);

    drop((notifications_a, requests_a, notifications_b, requests_b));
    for (_, handle) in state.runtime_profile_monitors.lock().unwrap().drain() {
        handle.abort();
    }
    let _ = fs::remove_dir_all(sandbox);
}

#[test]
fn string_subagent_sources_are_hidden_from_main_session_list() {
    for source in ["review", "compact", "memory_consolidation", "thread_spawn"] {
        assert!(thread_source_marks_subagent(&json!({ "subagent": source })));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_source_state_db_does_not_override_materialized_target_profile() {
    let sandbox = unique_test_dir("stale-profile-state-db");
    let workspace = sandbox.join("workspace");
    let source_codex_home = sandbox.join("source-codex-home");
    let target_codex_home = sandbox.join("target-codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&source_codex_home).unwrap();
    fs::create_dir_all(target_codex_home.join("archived_sessions")).unwrap();
    let session_id = "019f4b17-0000-7000-8000-000000000001";

    let connection = rusqlite::Connection::open(source_codex_home.join("state_5.sqlite")).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL, created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL, source TEXT NOT NULL, model_provider TEXT NOT NULL,
                cwd TEXT NOT NULL, title TEXT NOT NULL, sandbox_policy TEXT NOT NULL,
                approval_mode TEXT NOT NULL, archived INTEGER NOT NULL DEFAULT 0,
                first_user_message TEXT NOT NULL DEFAULT '', agent_nickname TEXT, agent_role TEXT,
                created_at_ms INTEGER, updated_at_ms INTEGER
            );
            CREATE TABLE thread_spawn_edges (
                parent_thread_id TEXT NOT NULL, child_thread_id TEXT NOT NULL PRIMARY KEY,
                status TEXT NOT NULL
            );",
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO threads (
                id, rollout_path, created_at, updated_at, source, model_provider, cwd, title,
                sandbox_policy, approval_mode, archived, first_user_message, created_at_ms,
                updated_at_ms
            ) VALUES (?1, ?2, 1, 1, 'vscode', 'openai', ?3, 'stale source', '{}',
                'never', 0, 'stale', 1000, 1000)",
            rusqlite::params![
                session_id,
                source_codex_home
                    .join("missing.jsonl")
                    .display()
                    .to_string(),
                workspace.display().to_string()
            ],
        )
        .unwrap();
    drop(connection);
    let target_rollout = target_codex_home
        .join("archived_sessions")
        .join(format!("rollout-2026-07-10T00-00-00-{session_id}.jsonl"));
    fs::write(
        target_rollout,
        format!(
            "{}\n",
            json!({
                "timestamp": "2026-07-10T00:00:00Z",
                "type": "session_meta",
                "payload": {
                    "id": session_id,
                    "timestamp": "2026-07-10T00:00:00Z",
                    "cwd": workspace,
                    "originator": "codex_webui",
                    "cli_version": "0.144.1",
                    "source": "vscode"
                }
            })
        ),
    )
    .unwrap();

    let mut state = test_state(
        workspace.clone(),
        vec![workspace.clone()],
        source_codex_home,
    );
    Arc::make_mut(&mut state.config).profiles.insert(
        "target".to_string(),
        RuntimeProfile {
            label: "Target".to_string(),
            codex_home: target_codex_home,
            data_dir: sandbox.join("target-data"),
        },
    );
    assert_eq!(
        resolve_session_profile_id(&state, "default", session_id).await,
        "target"
    );

    let _ = fs::remove_dir_all(sandbox);
}
