use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_session_payload_uses_app_server_and_persists_preferences() {
    let sandbox = unique_test_dir("session-create-rust");
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
            "model": "gpt-5.4",
            "approvalPolicy": "on-request",
            "sandboxMode": "workspace-write"
        }),
        None,
        Some("Review docs"),
    )
    .await
    .unwrap();

    assert_eq!(
        created.get("name").and_then(Value::as_str),
        Some("Review docs")
    );
    let session_id = created
        .get("id")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();

    let stored_preferences = with_ui_state_read(&state, "default", |ui_state| {
        Ok(ui_state
            .get("preferencesByThreadId")
            .and_then(Value::as_object)
            .and_then(|entries| entries.get(&session_id))
            .cloned()
            .unwrap_or(Value::Null))
    })
    .await
    .unwrap();
    assert_eq!(
        stored_preferences.get("model").and_then(Value::as_str),
        Some("gpt-5.4")
    );

    let thread = app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/read",
            json!({
                "threadId": session_id,
                "includeTurns": false
            }),
        )
        .await
        .unwrap();
    assert_eq!(
        thread
            .get("thread")
            .and_then(|value| value.get("name"))
            .and_then(Value::as_str),
        Some("Review docs")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rust_session_list_and_search_use_app_server_threads() {
    let sandbox = unique_test_dir("session-list-rust");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let first = create_session_payload(
        &state,
        "default",
        json!({ "cwd": workspace.display().to_string() }),
        None,
        Some("Build Docs"),
    )
    .await
    .unwrap();
    let second = create_session_payload(
        &state,
        "default",
        json!({ "cwd": workspace.display().to_string() }),
        None,
        Some("Fix Queue"),
    )
    .await
    .unwrap();
    let first_id = first.get("id").and_then(Value::as_str).unwrap().to_string();
    let second_id = second
        .get("id")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();

    with_ui_state_write(&state, "default", |ui_state| {
        let Some(session_meta) = ui_state
            .get_mut("sessionMetaByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "session meta state is missing",
            ));
        };
        session_meta.insert(
            first_id.clone(),
            json!({
                "pinned": true,
                "tags": ["docs"]
            }),
        );
        let Some(queues) = ui_state
            .get_mut("queuesByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "queue state is missing",
            ));
        };
        queues.insert(
            second_id.clone(),
            json!({
                "items": [
                    {
                        "id": "queue-1",
                        "prompt": "follow up"
                    }
                ],
                "resumePending": false,
                "updatedAt": 10
            }),
        );
        Ok(())
    })
    .await
    .unwrap();

    let pinned_only = list_sessions_payload(
        &state,
        "default",
        false,
        None,
        20,
        &SessionFilterCriteria {
            pinned_only: true,
            ..SessionFilterCriteria::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        pinned_only
            .get("sessions")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        pinned_only
            .get("sessions")
            .and_then(Value::as_array)
            .and_then(|entries| entries.first())
            .and_then(|entry| entry.get("id"))
            .and_then(Value::as_str),
        Some(first_id.as_str())
    );

    let queued_only = list_sessions_payload(
        &state,
        "default",
        false,
        None,
        20,
        &SessionFilterCriteria {
            queued_only: true,
            ..SessionFilterCriteria::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        queued_only
            .get("sessions")
            .and_then(Value::as_array)
            .and_then(|entries| entries.first())
            .and_then(|entry| entry.get("id"))
            .and_then(Value::as_str),
        Some(second_id.as_str())
    );

    let matched = search_sessions_payload(
        &state,
        "default",
        "queue",
        "summary",
        false,
        None,
        20,
        &SessionFilterCriteria::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        matched
            .get("sessions")
            .and_then(Value::as_array)
            .and_then(|entries| entries.first())
            .and_then(|entry| entry.get("name"))
            .and_then(Value::as_str),
        Some("Fix Queue")
    );

    let uppercase = search_sessions_payload(
        &state,
        "default",
        "BUILD",
        "summary",
        false,
        None,
        20,
        &SessionFilterCriteria::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        uppercase
            .get("sessions")
            .and_then(Value::as_array)
            .and_then(|entries| entries.first())
            .and_then(|entry| entry.get("name"))
            .and_then(Value::as_str),
        Some("Build Docs")
    );

    app_server_client(&state, "default")
            .await
            .unwrap()
            .request(
                "thread/seed",
                json!({
                    "thread": {
                        "id": "thread-full",
                        "name": "Research notes",
                        "preview": "Unrelated summary",
                        "cwd": workspace.display().to_string(),
                        "archived": false,
                        "createdAt": 3,
                        "updatedAt": 4,
                        "status": "idle",
                        "isSubagent": false,
                        "agentNickname": Value::Null,
                        "agentRole": Value::Null,
                        "turns": [
                            {
                                "id": "turn-1",
                                "status": "completed",
                                "error": Value::Null,
                                "startedAt": 30,
                                "completedAt": 40,
                                "durationMs": 10,
                                "items": [
                                    {
                                        "id": "item-1",
                                        "type": "assistantMessage",
                                        "text": "The websocket duplicate send race originates from optimistic queue replay."
                                    }
                                ]
                            }
                        ]
                    }
                }),
            )
            .await
            .unwrap();

    let full_text = search_sessions_payload(
        &state,
        "default",
        "optimistic queue replay",
        "full",
        false,
        None,
        20,
        &SessionFilterCriteria::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        full_text
            .get("sessions")
            .and_then(Value::as_array)
            .and_then(|entries| entries.first())
            .and_then(|entry| entry.get("id"))
            .and_then(Value::as_str),
        Some("thread-full")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ws_session_cache_validation_returns_not_modified_for_matching_versions() {
    let sandbox = unique_test_dir("session-cache-validation-rust");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let created = create_session_payload(
        &state,
        "default",
        json!({ "cwd": workspace.display().to_string() }),
        None,
        Some("Cached thread"),
    )
    .await
    .unwrap();
    let session_id = created
        .get("id")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();

    let (out_tx, _out_rx) = mpsc::unbounded_channel();
    let subscriptions: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let auth = AuthContext {
        role: UserRole::Admin,
        profile_id: "default".to_string(),
    };

    let list_payload = execute_ws_method(
        &state,
        &out_tx,
        &subscriptions,
        &auth,
        "sessions/list",
        json!({
            "archived": false,
            "limit": 20
        }),
    )
    .await
    .unwrap();
    let list_version = list_payload
        .get("cacheVersion")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();
    let list_not_modified = execute_ws_method(
        &state,
        &out_tx,
        &subscriptions,
        &auth,
        "sessions/list",
        json!({
            "archived": false,
            "limit": 20,
            "knownVersion": list_version
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        list_not_modified
            .get("notModified")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert!(list_not_modified.get("sessions").is_none());

    let detail_payload = execute_ws_method(
        &state,
        &out_tx,
        &subscriptions,
        &auth,
        "session/get",
        json!({
            "sessionId": session_id,
            "limit": 20
        }),
    )
    .await
    .unwrap();
    let detail_version = detail_payload
        .get("cacheVersion")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();
    let detail_not_modified = execute_ws_method(
        &state,
        &out_tx,
        &subscriptions,
        &auth,
        "session/get",
        json!({
            "sessionId": session_id,
            "limit": 20,
            "knownVersion": detail_version
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        detail_not_modified
            .get("notModified")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert!(detail_not_modified.get("thread").is_none());

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abort_turn_payload_uses_known_active_turn() {
    let sandbox = unique_test_dir("abort-turn-rust");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    state.active_turns.lock().await.insert(
        runtime_session_key("default", "thread-1"),
        "turn-123".to_string(),
    );

    let payload = abort_turn_payload(&state, "default", "thread-1")
        .await
        .unwrap();
    assert_eq!(
        payload.get("interrupted").and_then(Value::as_bool),
        Some(true)
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_detail_and_turn_search_payloads_use_rust_thread_reads() {
    let sandbox = unique_test_dir("session-detail-rust");
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
                        "name": "Investigate bug",
                        "preview": "Investigate websocket bug",
                        "cwd": workspace.display().to_string(),
                        "archived": false,
                        "createdAt": 1,
                        "updatedAt": 2,
                        "status": "running",
                        "isSubagent": false,
                        "turns": [
                            {
                                "id": "turn-1",
                                "status": "completed",
                                "error": Value::Null,
                                "startedAt": 10,
                                "completedAt": 20,
                                "durationMs": 10,
                                "items": [
                                    {
                                        "id": "item-1",
                                        "type": "userMessage",
                                        "text": "Find the websocket bug"
                                    },
                                    {
                                        "id": "item-2",
                                        "type": "agentMessage",
                                        "text": "Investigating the websocket bug now"
                                    }
                                ]
                            },
                            {
                                "id": "turn-2",
                                "status": "inProgress",
                                "error": Value::Null,
                                "startedAt": 30,
                                "completedAt": Value::Null,
                                "durationMs": Value::Null,
                                "items": [
                                    {
                                        "id": "item-3",
                                        "type": "reasoning",
                                        "text": "Need to inspect websocket state handling"
                                    }
                                ]
                            }
                        ],
                        "tokenUsage": {
                            "total": { "totalTokens": 12, "inputTokens": 6, "cachedInputTokens": 0, "outputTokens": 6, "reasoningOutputTokens": 2 },
                            "last": { "totalTokens": 7, "inputTokens": 3, "cachedInputTokens": 0, "outputTokens": 4, "reasoningOutputTokens": 1 },
                            "modelContextWindow": 1000
                        }
                    }
                }),
            )
            .await
            .unwrap();

    let detail = session_detail_payload(&state, "default", "thread-1", 1)
        .await
        .unwrap();
    assert_eq!(
        detail
            .get("thread")
            .and_then(|value| value.get("turns"))
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        detail.get("activeTurnId").and_then(Value::as_str),
        Some("turn-2")
    );
    assert_eq!(
        detail
            .get("hydration")
            .and_then(|value| value.get("remainingTurns"))
            .and_then(Value::as_u64),
        Some(1)
    );

    let older = session_older_turns_payload(&state, "default", "thread-1", "turn-2", 5)
        .await
        .unwrap();
    assert_eq!(
        older.get("turns").and_then(Value::as_array).map(Vec::len),
        Some(1)
    );

    let turn = session_turn_payload(&state, "default", "thread-1", "turn-1")
        .await
        .unwrap();
    assert_eq!(
        turn.get("turn")
            .and_then(|value| value.get("id"))
            .and_then(Value::as_str),
        Some("turn-1")
    );

    let item = session_item_detail_payload(&state, "default", "thread-1", "turn-1", "item-2")
        .await
        .unwrap();
    assert_eq!(
        item.get("item")
            .and_then(|value| value.get("detailState"))
            .and_then(Value::as_str),
        Some("loaded")
    );

    let search = search_session_turns_payload(&state, "default", "thread-1", "websocket", None, 20)
        .await
        .unwrap();
    assert_eq!(
        search
            .get("matches")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(3)
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_detail_payload_clears_completed_highlight_on_open() {
    let sandbox = unique_test_dir("session-detail-highlight-clear");
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
                    "name": "Investigate bug",
                    "preview": "Investigate websocket bug",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 2,
                    "status": "completed",
                    "isSubagent": false,
                    "turns": [
                        {
                            "id": "turn-1",
                            "status": "completed",
                            "error": Value::Null,
                            "startedAt": 10,
                            "completedAt": 20,
                            "durationMs": 10,
                            "items": [
                                {
                                    "id": "item-1",
                                    "type": "agentMessage",
                                    "text": "Done"
                                }
                            ]
                        }
                    ]
                }
            }),
        )
        .await
        .unwrap();

    with_ui_state_write(&state, "default", |ui_state| {
        let highlights = ui_state
            .get_mut("highlightsByThreadId")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                api_error(StatusCode::INTERNAL_SERVER_ERROR, "missing highlight state")
            })?;
        highlights.insert(
            "thread-1".to_string(),
            json!({
                "kind": "completed",
                "at": now_unix_ms()
            }),
        );
        Ok(())
    })
    .await
    .unwrap();

    session_detail_payload(&state, "default", "thread-1", 20)
        .await
        .unwrap();

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
async fn send_turn_payload_uses_app_server_and_updates_session_state() {
    let sandbox = unique_test_dir("turn-send-rust");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let runtime_profile = resolve_runtime_profile(&state.config, "default");
    let uploads_dir = runtime_profile.data_dir.join("uploads").join("thread-1");
    fs::create_dir_all(&uploads_dir).unwrap();

    let text_attachment_path = workspace.join("notes.md");
    let image_attachment_path = workspace.join("diagram.png");
    fs::write(&text_attachment_path, "attachment").unwrap();
    fs::write(&image_attachment_path, "png").unwrap();
    fs::write(
        uploads_dir.join("att-file.json"),
        serde_json::to_vec(&json!({
            "id": "att-file",
            "originalName": "notes.md",
            "path": text_attachment_path.display().to_string(),
            "mimeType": "text/markdown",
            "size": 10,
            "kind": "file",
            "createdAt": "2026-04-20T00:00:00Z"
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        uploads_dir.join("att-image.json"),
        serde_json::to_vec(&json!({
            "id": "att-image",
            "originalName": "diagram.png",
            "path": image_attachment_path.display().to_string(),
            "mimeType": "image/png",
            "size": 12,
            "kind": "image",
            "createdAt": "2026-04-20T00:00:01Z"
        }))
        .unwrap(),
    )
    .unwrap();

    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": "thread-1",
                    "name": "New thread",
                    "preview": "",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 1,
                    "status": "notLoaded",
                    "isSubagent": false,
                    "agentNickname": Value::Null,
                    "agentRole": Value::Null,
                    "turns": []
                }
            }),
        )
        .await
        .unwrap();

    save_session_draft_payload(&state, "default", "thread-1", "Draft to clear", "message")
        .await
        .unwrap();

    let prompt = "Inspect the duplicated websocket send behaviour and capture the root cause before patching it.";
    let selected_skills = json!([
        {
            "id": "skill-1",
            "name": "imagegen",
            "path": "/skills/imagegen/SKILL.md"
        }
    ]);
    let payload = send_turn_payload(
        &state,
        "default",
        "thread-1",
        prompt,
        Some(&json!(["att-file", "att-image"])),
        Some(&selected_skills),
        json!({
            "cwd": workspace.display().to_string(),
            "model": "gpt-5",
            "approvalPolicy": "on-request",
            "sandboxMode": "workspace-write",
            "speed": "fast",
            "effort": "high",
            "networkAccess": true
        }),
    )
    .await
    .unwrap();

    assert_eq!(payload.get("ok").and_then(Value::as_bool), Some(true));
    assert_eq!(
        payload.get("turnId").and_then(Value::as_str),
        Some("turn-1")
    );

    let thread = read_thread_payload(&state, "default", "thread-1", true)
        .await
        .unwrap();
    assert_eq!(
        thread.get("status").and_then(Value::as_str),
        Some("running")
    );
    assert_eq!(thread.get("resumeCount").and_then(Value::as_u64), Some(1));
    assert_eq!(
        thread.get("name").and_then(Value::as_str),
        infer_persisted_session_title(prompt).as_deref()
    );
    assert_eq!(
        thread.get("turns").and_then(Value::as_array).map(Vec::len),
        Some(1)
    );

    let last_turn_start = thread.get("lastTurnStart").cloned().unwrap_or(Value::Null);
    assert_eq!(
        last_turn_start.get("serviceTier").and_then(Value::as_str),
        Some("fast")
    );
    let input = last_turn_start
        .get("input")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(input.len(), 3);
    let first_text = input
        .first()
        .and_then(|value| value.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(first_text.contains("$imagegen"));
    assert!(first_text.contains(ATTACHMENT_PREAMBLE_START));
    assert!(first_text.contains(ATTACHMENT_PREAMBLE_END));
    assert!(first_text.contains(text_attachment_path.to_str().unwrap()));
    assert!(first_text.contains(prompt.trim()));
    assert_eq!(
        input
            .get(2)
            .and_then(|value| value.get("path"))
            .and_then(Value::as_str),
        Some(image_attachment_path.to_str().unwrap())
    );
    assert_eq!(
        input
            .get(1)
            .and_then(|value| value.get("type"))
            .and_then(Value::as_str),
        Some("skill")
    );
    assert_eq!(
        input
            .get(1)
            .and_then(|value| value.get("name"))
            .and_then(Value::as_str),
        Some("imagegen")
    );

    let stored_preferences = with_ui_state_read(&state, "default", |ui_state| {
        Ok(ui_state
            .get("preferencesByThreadId")
            .and_then(Value::as_object)
            .and_then(|entries| entries.get("thread-1"))
            .cloned()
            .unwrap_or(Value::Null))
    })
    .await
    .unwrap();
    assert_eq!(
        stored_preferences.get("model").and_then(Value::as_str),
        Some("gpt-5")
    );

    let draft = get_session_draft_payload(&state, "default", "thread-1")
        .await
        .unwrap();
    assert_eq!(draft.get("draft").and_then(Value::as_str), Some(""));

    let runtime_key = runtime_session_key(
        resolve_runtime_profile_entry(&state.config, "default").0,
        "thread-1",
    );
    assert_eq!(
        state.active_turns.lock().await.get(&runtime_key).cloned(),
        Some("turn-1".to_string())
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steer_turn_payload_uses_active_turn_from_thread_reads() {
    let sandbox = unique_test_dir("turn-steer-rust");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let runtime_profile = resolve_runtime_profile(&state.config, "default");
    let uploads_dir = runtime_profile.data_dir.join("uploads").join("thread-1");
    fs::create_dir_all(&uploads_dir).unwrap();

    let text_attachment_path = workspace.join("handoff.md");
    fs::write(&text_attachment_path, "handoff").unwrap();
    fs::write(
        uploads_dir.join("att-file.json"),
        serde_json::to_vec(&json!({
            "id": "att-file",
            "originalName": "handoff.md",
            "path": text_attachment_path.display().to_string(),
            "mimeType": "text/markdown",
            "size": 7,
            "kind": "file",
            "createdAt": "2026-04-20T00:00:00Z"
        }))
        .unwrap(),
    )
    .unwrap();

    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": "thread-1",
                    "name": "Investigate queue",
                    "preview": "Investigate queue",
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
                            "status": "completed",
                            "error": Value::Null,
                            "startedAt": 10,
                            "completedAt": 20,
                            "durationMs": 10,
                            "items": []
                        },
                        {
                            "id": "turn-2",
                            "status": "inProgress",
                            "error": Value::Null,
                            "startedAt": 30,
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

    save_session_draft_payload(&state, "default", "thread-1", "Steer draft", "steer")
        .await
        .unwrap();

    let payload = steer_turn_payload(
        &state,
        "default",
        "thread-1",
        "Focus on the queue deduplication race first.",
        Some(&json!(["att-file"])),
        None,
    )
    .await
    .unwrap();

    assert_eq!(payload.get("ok").and_then(Value::as_bool), Some(true));
    assert_eq!(
        payload.get("turnId").and_then(Value::as_str),
        Some("turn-2")
    );

    let thread = read_thread_payload(&state, "default", "thread-1", true)
        .await
        .unwrap();
    let last_turn_steer = thread.get("lastTurnSteer").cloned().unwrap_or(Value::Null);
    assert_eq!(
        last_turn_steer
            .get("expectedTurnId")
            .and_then(Value::as_str),
        Some("turn-2")
    );
    let first_text = last_turn_steer
        .get("input")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|value| value.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(first_text.contains(ATTACHMENT_PREAMBLE_START));
    assert!(first_text.contains(text_attachment_path.to_str().unwrap()));
    assert!(first_text.contains("Focus on the queue deduplication race first."));

    let runtime_key = runtime_session_key(
        resolve_runtime_profile_entry(&state.config, "default").0,
        "thread-1",
    );
    assert_eq!(
        state.active_turns.lock().await.get(&runtime_key).cloned(),
        Some("turn-2".to_string())
    );

    let draft = get_session_draft_payload(&state, "default", "thread-1")
        .await
        .unwrap();
    assert_eq!(draft.get("draft").and_then(Value::as_str), Some(""));

    let _ = fs::remove_dir_all(sandbox);
}
