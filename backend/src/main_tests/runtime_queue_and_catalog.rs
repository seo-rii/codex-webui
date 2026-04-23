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
    let second = enqueue_session_queue_payload(&state, "default", "thread-1", "second", None, None)
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
        &[second_id.clone(), first_id.clone()],
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
        Some(1)
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
