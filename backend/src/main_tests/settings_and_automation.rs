use super::*;

#[tokio::test]
async fn writes_and_reads_editable_files_inside_profile_home() {
    let sandbox = unique_test_dir("editor");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state(workspace.clone(), vec![workspace], codex_home.clone());
    let config_path = codex_home.join("config.toml");
    let saved: EditableFilePayload = serde_json::from_value(
        write_editable_file_payload(
            &state,
            "default",
            config_path.to_str().unwrap(),
            "model = 'gpt-5.4'\n",
        )
        .await
        .expect("save should succeed"),
    )
    .expect("payload should deserialize");
    let loaded: EditableFilePayload = serde_json::from_value(
        read_editable_file_payload(&state, "default", config_path.to_str().unwrap())
            .await
            .expect("read should succeed"),
    )
    .expect("payload should deserialize");

    assert_eq!(saved.path, config_path.display().to_string());
    assert_eq!(saved.language, "ini");
    assert_eq!(loaded.content, "model = 'gpt-5.4'\n");
    assert!(loaded.writable);

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn rejects_editable_files_outside_allowed_roots() {
    let sandbox = unique_test_dir("editor-outside");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    let outside = sandbox.join("outside").join("secret.txt");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    let error = resolve_editable_file_path(&state, "default", outside.to_str().unwrap())
        .await
        .expect_err("outside paths must be rejected");

    assert_eq!(error.status, StatusCode::FORBIDDEN);
    assert_eq!(error.message, "This file is outside editable roots.");

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn rejects_sensitive_editable_files_inside_profile_home() {
    let sandbox = unique_test_dir("editor-sensitive");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state(workspace.clone(), vec![workspace], codex_home.clone());
    let auth_path = codex_home.join("auth.json");
    fs::write(&auth_path, "{}").unwrap();
    let error = read_editable_file_payload(&state, "default", auth_path.to_str().unwrap())
        .await
        .expect_err("auth files must be blocked");

    assert_eq!(error.status, StatusCode::FORBIDDEN);
    assert_eq!(
        error.message,
        "This file is blocked by the sensitive file policy."
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn rejects_oversized_editable_file_previews() {
    let sandbox = unique_test_dir("editor-large-preview");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state(workspace.clone(), vec![workspace.clone()], codex_home);
    let large_path = workspace.join("large.txt");
    fs::write(
        &large_path,
        vec![b'a'; TEXT_FILE_PREVIEW_LIMIT_BYTES as usize + 1],
    )
    .unwrap();
    let error = read_editable_file_payload(&state, "default", large_path.to_str().unwrap())
        .await
        .expect_err("large files should not be loaded into editor preview");

    assert_eq!(error.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(error.message, "The selected file is too large to preview.");

    let _ = fs::remove_dir_all(sandbox);
}

#[cfg(unix)]
#[tokio::test]
async fn command_runner_rejects_oversized_output() {
    let error = run_command_with_timeout(
        "sh",
        vec![
            "-c".to_string(),
            format!("yes x | head -c {}", CHILD_OUTPUT_LIMIT_BYTES + 1),
        ],
        Duration::from_secs(5),
    )
    .await
    .expect_err("oversized command output should be rejected");

    assert!(format!("{error:#}").contains("output limit"));
}

#[cfg(unix)]
#[tokio::test]
async fn rejects_editable_file_writes_through_symlinked_parent() {
    let sandbox = unique_test_dir("editor-symlink-parent");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    let outside = sandbox.join("outside");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, workspace.join("link-out")).unwrap();

    let state = test_state(workspace.clone(), vec![workspace.clone()], codex_home);
    let error = write_editable_file_payload(
        &state,
        "default",
        workspace
            .join("link-out")
            .join("secret.txt")
            .to_str()
            .unwrap(),
        "secret\n",
    )
    .await
    .expect_err("symlinked parent writes must be rejected");

    assert_eq!(error.status, StatusCode::FORBIDDEN);
    assert_eq!(
        error.message,
        "Refusing to write through a symlinked parent directory."
    );
    assert!(!outside.join("secret.txt").exists());

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn notification_helpers_update_ui_state_and_counts() {
    let sandbox = unique_test_dir("notifications");
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
                "items": [
                    {
                        "id": "n1",
                        "type": "sessionCompleted",
                        "createdAt": 20,
                        "readAt": Value::Null,
                        "sessionId": "s1",
                        "sessionName": "One",
                        "payload": {}
                    },
                    {
                        "id": "n2",
                        "type": "sessionAttention",
                        "createdAt": 10,
                        "readAt": Value::Null,
                        "sessionId": "s2",
                        "sessionName": "Two",
                        "payload": {}
                    }
                ],
                "settings": {
                    "enabledEventTypes": ["sessionCompleted"],
                    "slackWebhookUrl": "",
                    "webhookUrl": Value::Null
                }
            },
            "sessionMetaByThreadId": {},
            "savedSessionFilters": [],
            "promptPresets": [],
            "automations": [],
            "automationRuns": [],
            "preferencesByThreadId": {},
            "draftsByThreadId": {},
            "queuesByThreadId": {},
            "highlightsByThreadId": {}
        }))
        .unwrap(),
    )
    .unwrap();

    let listed = get_notifications_payload(&state, "default", 80)
        .await
        .unwrap();
    assert_eq!(listed.get("unreadCount").and_then(Value::as_u64), Some(2));

    let marked = mark_notifications_read_payload(&state, "default", Some(vec!["n1".to_string()]))
        .await
        .unwrap();
    assert_eq!(marked.get("unreadCount").and_then(Value::as_u64), Some(1));

    let settings = update_notification_settings_payload(
        &state,
        "default",
        json!({
            "enabledEventTypes": ["sessionAttention", "invalid"],
            "slackWebhookUrl": " https://hooks.slack.test/one ",
            "webhookUrl": ""
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        settings
            .get("settings")
            .and_then(|value| value.get("enabledEventTypes"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        vec![json!("sessionAttention")]
    );
    assert_eq!(
        settings
            .get("settings")
            .and_then(|value| value.get("slackWebhookUrl"))
            .and_then(Value::as_str),
        Some("https://hooks.slack.test/one")
    );
    assert!(
        settings
            .get("settings")
            .and_then(|value| value.get("webhookUrl"))
            .is_some_and(Value::is_null)
    );

    let cleared = clear_notifications_payload(&state, "default")
        .await
        .unwrap();
    assert_eq!(cleared.get("unreadCount").and_then(Value::as_u64), Some(0));
    assert_eq!(
        cleared
            .get("notifications")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn terminal_cleanup_removes_stale_sessions() {
    let sandbox = unique_test_dir("terminal-cleanup");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state(workspace.clone(), vec![workspace.clone()], codex_home);
    let (relay, _) = broadcast::channel(1);
    let terminal_id = "stale-terminal".to_string();
    state.terminals.lock().await.insert(
        terminal_id.clone(),
        Arc::new(TerminalSession {
            summary: Mutex::new(TerminalSummaryState {
                id: terminal_id.clone(),
                title: "Old".to_string(),
                cwd: workspace.display().to_string(),
                created_at: 1,
                last_activity_at: now_unix_ms()
                    .saturating_sub(TERMINAL_EXITED_TTL_MS)
                    .saturating_sub(1),
                status: "exited".to_string(),
                exit_code: Some(0),
            }),
            buffer: Mutex::new(String::new()),
            stdin: Mutex::new(None),
            relay,
            pid: None,
        }),
    );

    cleanup_terminal_sessions(state.clone()).await;
    assert!(!state.terminals.lock().await.contains_key(&terminal_id));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn terminal_create_rejects_session_limit_before_spawning() {
    let sandbox = unique_test_dir("terminal-limit");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state(workspace.clone(), vec![workspace.clone()], codex_home);
    for index in 0..MAX_TERMINAL_SESSIONS {
        let (relay, _) = broadcast::channel(1);
        let terminal_id = format!("terminal-{index}");
        state.terminals.lock().await.insert(
            terminal_id.clone(),
            Arc::new(TerminalSession {
                summary: Mutex::new(TerminalSummaryState {
                    id: terminal_id,
                    title: "Running".to_string(),
                    cwd: workspace.display().to_string(),
                    created_at: now_unix_ms(),
                    last_activity_at: now_unix_ms(),
                    status: "running".to_string(),
                    exit_code: None,
                }),
                buffer: Mutex::new(String::new()),
                stdin: Mutex::new(None),
                relay,
                pid: None,
            }),
        );
    }

    let error = create_terminal(state, Some(workspace.display().to_string()), None)
        .await
        .expect_err("terminal limit should reject before process spawn");
    assert!(error.to_string().contains("terminal session limit reached"));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn theme_settings_round_trip_through_rust_store() {
    let sandbox = unique_test_dir("theme-settings");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    let expected = json!({
        "light": {
            "bg": "#fffef7",
            "sidebar": "#f8f1de"
        },
        "dark": {
            "bg": "#181713",
            "sidebar": "#12110d"
        }
    });

    write_stored_theme_settings(&state.config, "default", &expected)
        .await
        .expect("theme settings should save");
    let restored = read_stored_theme_settings(&state.config, "default")
        .await
        .expect("theme settings should load");

    assert_eq!(restored, Some(expected));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn ui_state_writes_leave_no_atomic_temp_files() {
    let sandbox = unique_test_dir("ui-state-atomic");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    with_ui_state_write(&state, "default", |ui_state| {
        ui_state["sessionMetaByThreadId"]["thread-1"]["title"] = json!("Atomic");
        Ok(())
    })
    .await
    .expect("ui state should save");

    let ui_state_path = profile_ui_state_path(&state.config, "default");
    let raw = fs::read_to_string(&ui_state_path).expect("ui state file should exist");
    let parsed: Value = serde_json::from_str(&raw).expect("ui state should be valid JSON");
    assert_eq!(
        parsed
            .get("sessionMetaByThreadId")
            .and_then(|value| value.get("thread-1"))
            .and_then(|value| value.get("title"))
            .and_then(Value::as_str),
        Some("Atomic")
    );

    let mut entries = fs::read_dir(ui_state_path.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .collect::<Vec<_>>();
    entries.sort();
    assert!(
        !entries
            .iter()
            .any(|name| name.starts_with(".codex-webui-state-"))
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn arming_shutdown_while_idle_waits_for_future_activity() {
    let sandbox = unique_test_dir("shutdown-idle-arming");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let mut state = test_state(workspace.clone(), vec![workspace], codex_home);
    let mut config = (*state.config).clone();
    config.system_shutdown_enabled = true;
    state.config = Arc::new(config);

    let updated = update_config_payload(
        &state,
        "default",
        json!({
            "systemShutdown": {
                "armed": true
            }
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        updated
            .get("systemShutdown")
            .and_then(|value| value.get("armed"))
            .and_then(Value::as_bool),
        Some(true)
    );
    assert!(
        updated
            .get("startup")
            .and_then(|value| value.get("scheduledShutdown"))
            .is_some_and(Value::is_null)
    );

    let idle_state = with_ui_state_read(&state, "default", |ui_state| {
        Ok((
            ui_state
                .get("global")
                .and_then(|value| value.get("shutdownAfterQueueCompletes"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            ui_state
                .get("global")
                .and_then(|value| value.get("shutdownAfterQueueCompletesPrimed"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            ui_state
                .get("global")
                .and_then(|value| value.get("scheduledShutdown"))
                .cloned()
                .unwrap_or(Value::Null),
        ))
    })
    .await
    .unwrap();
    assert_eq!(idle_state.0, true);
    assert_eq!(idle_state.1, false);
    assert!(idle_state.2.is_null());

    maybe_schedule_global_shutdown(&state, "default", None).await;

    let after_schedule_attempt = with_ui_state_read(&state, "default", |ui_state| {
        Ok(ui_state
            .get("global")
            .and_then(|value| value.get("scheduledShutdown"))
            .cloned()
            .unwrap_or(Value::Null))
    })
    .await
    .unwrap();
    assert!(after_schedule_attempt.is_null());

    cancel_scheduled_shutdown_for_activity(&state, "default").await;

    let primed_state = with_ui_state_read(&state, "default", |ui_state| {
        Ok(ui_state
            .get("global")
            .and_then(|value| value.get("shutdownAfterQueueCompletesPrimed"))
            .and_then(Value::as_bool)
            .unwrap_or(false))
    })
    .await
    .unwrap();
    assert!(primed_state);

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn restore_runtime_profile_state_does_not_schedule_shutdown_without_new_work() {
    let sandbox = unique_test_dir("shutdown-restore-idle");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let mut state = test_state(workspace.clone(), vec![workspace], codex_home);
    let mut config = (*state.config).clone();
    config.system_shutdown_enabled = true;
    config.system_shutdown_command_override = Some("/bin/true".to_string());
    state.config = Arc::new(config);

    with_ui_state_write(&state, "default", |ui_state| {
        let Some(global) = ui_state.get_mut("global").and_then(Value::as_object_mut) else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "global state is missing",
            ));
        };
        global.insert("shutdownAfterQueueCompletes".to_string(), json!(true));
        global.insert("shutdownAfterQueueCompletesPrimed".to_string(), json!(true));
        global.insert("scheduledShutdown".to_string(), Value::Null);
        Ok(())
    })
    .await
    .unwrap();

    restore_persisted_shutdown_state(&state, "default")
        .await
        .unwrap();

    let restored_state = with_ui_state_read(&state, "default", |ui_state| {
        Ok((
            ui_state
                .get("global")
                .and_then(|value| value.get("shutdownAfterQueueCompletes"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            ui_state
                .get("global")
                .and_then(|value| value.get("shutdownAfterQueueCompletesPrimed"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            ui_state
                .get("global")
                .and_then(|value| value.get("scheduledShutdown"))
                .cloned()
                .unwrap_or(Value::Null),
        ))
    })
    .await
    .unwrap();

    assert!(restored_state.0);
    assert!(!restored_state.1);
    assert!(restored_state.2.is_null());

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn syncs_codex_toml_with_preferences_for_plan_mode() {
    let sandbox = unique_test_dir("sync-codex-toml");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&codex_home).unwrap();

    sync_codex_toml_with_preferences(
        &codex_home,
        &json!({
            "model": "gpt-5.4",
            "modelContextWindow": 100000000,
            "approvalPolicy": "on-request",
            "sandboxMode": "workspace-write",
            "speed": "fast",
            "mode": "plan",
            "effort": "high",
            "networkAccess": true
        }),
    )
    .await
    .expect("config.toml should sync");

    let raw = fs::read_to_string(config_toml_path(&codex_home)).unwrap();
    assert!(raw.contains("model = \"gpt-5.4\""));
    assert!(raw.contains("model_context_window = 100000000"));
    assert!(raw.contains("approval_policy = \"on-request\""));
    assert!(raw.contains("sandbox_mode = \"workspace-write\""));
    assert!(raw.contains("service_tier = \"fast\""));
    assert!(raw.contains("plan_mode_reasoning_effort = \"high\""));
    assert!(raw.contains("[sandbox_workspace_write]"));
    assert!(raw.contains("network_access = true"));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn session_preferences_defaults_include_model_context_window_from_codex_toml() {
    let sandbox = unique_test_dir("defaults-model-context-window");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    fs::write(
        config_toml_path(&codex_home),
        format!("{CONFIG_SCHEMA_HEADER}\nmodel_context_window = 100000000\n"),
    )
    .unwrap();

    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    let defaults = session_preferences_defaults_payload(&state, "default").await;

    assert_eq!(
        defaults.get("modelContextWindow").and_then(Value::as_i64),
        Some(100000000)
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn updates_session_organization_and_known_tags() {
    let sandbox = unique_test_dir("session-organization");
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
            "sessionMetaByThreadId": {
                "session-1": {
                    "pinned": false,
                    "tags": ["alpha"]
                }
            },
            "savedSessionFilters": [],
            "promptPresets": [],
            "automations": [],
            "automationRuns": [],
            "preferencesByThreadId": {},
            "draftsByThreadId": {},
            "queuesByThreadId": {},
            "highlightsByThreadId": {}
        }))
        .unwrap(),
    )
    .unwrap();

    let payload = update_session_organization_payload(
        &state,
        "default",
        "session-1",
        json!({
            "pinned": true,
            "tags": ["beta", "alpha", "beta", " "]
        }),
    )
    .await
    .expect("session organization should update");

    assert_eq!(
        payload.get("meta"),
        Some(&json!({
            "pinned": true,
            "tags": ["alpha", "beta"]
        }))
    );
    assert_eq!(
        payload
            .get("knownTags")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        vec![json!("alpha"), json!("beta")]
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn saves_filters_and_prompt_presets_with_normalization() {
    let sandbox = unique_test_dir("ui-state-saves");
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
            "sessionMetaByThreadId": {
                "thread-1": {
                    "pinned": true,
                    "tags": ["alpha", "beta"]
                }
            },
            "savedSessionFilters": [],
            "promptPresets": [],
            "automations": [],
            "automationRuns": [],
            "preferencesByThreadId": {},
            "draftsByThreadId": {},
            "queuesByThreadId": {},
            "highlightsByThreadId": {}
        }))
        .unwrap(),
    )
    .unwrap();

    let filters = save_session_filter_payload(
        &state,
        "default",
        json!({
            "id": "filter-1",
            "name": "  Important  ",
            "pinnedOnly": true,
            "runningOnly": false,
            "queuedOnly": true,
            "highlight": "completed",
            "tags": ["beta", "alpha", "beta", ""]
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        filters
            .get("savedFilters")
            .and_then(Value::as_array)
            .and_then(|entries| entries.first())
            .and_then(|entry| entry.get("name"))
            .and_then(Value::as_str),
        Some("Important")
    );
    assert_eq!(
        filters
            .get("savedFilters")
            .and_then(Value::as_array)
            .and_then(|entries| entries.first())
            .and_then(|entry| entry.get("tags"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        vec![json!("alpha"), json!("beta")]
    );
    assert_eq!(
        filters
            .get("knownTags")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        vec![json!("alpha"), json!("beta")]
    );

    let presets = save_prompt_preset_payload(
        &state,
        "default",
        json!({
            "id": "preset-1",
            "name": "  Draft reply  ",
            "prompt": "Use the existing repo style.",
            "createdAt": 5
        }),
    )
    .await
    .unwrap();
    let first_preset = presets
        .get("promptPresets")
        .and_then(Value::as_array)
        .and_then(|entries| entries.first())
        .cloned()
        .unwrap();
    assert_eq!(
        first_preset.get("name").and_then(Value::as_str),
        Some("Draft reply")
    );
    assert_eq!(
        first_preset.get("createdAt").and_then(Value::as_i64),
        Some(5)
    );
    assert!(
        first_preset
            .get("updatedAt")
            .and_then(Value::as_i64)
            .is_some_and(|value| value >= 5)
    );

    let deleted_filters = delete_session_filter_payload(&state, "default", "filter-1")
        .await
        .unwrap();
    assert_eq!(
        deleted_filters
            .get("savedFilters")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );

    let deleted_presets = delete_prompt_preset_payload(&state, "default", "preset-1")
        .await
        .unwrap();
    assert_eq!(
        deleted_presets
            .get("promptPresets")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn saves_and_deletes_automations_with_normalization() {
    let sandbox = unique_test_dir("automation-saves");
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
            "queuesByThreadId": {},
            "highlightsByThreadId": {}
        }))
        .unwrap(),
    )
    .unwrap();

    let saved = save_automation_payload(
        &state,
        "default",
        json!({
            "id": "auto-1",
            "name": "  Morning Review  ",
            "prompt": "Check the repo state.",
            "enabled": true,
            "scheduleMode": "interval",
            "intervalMinutes": 5,
            "target": "local",
            "repoPath": "",
            "cwd": " /tmp/review ",
            "model": "gpt-5.4",
            "effort": "high",
            "speed": "fast",
            "mode": "plan"
        }),
    )
    .await
    .unwrap();

    let first_automation = saved
        .get("automations")
        .and_then(Value::as_array)
        .and_then(|entries| entries.first())
        .cloned()
        .unwrap();
    assert_eq!(
        first_automation.get("name").and_then(Value::as_str),
        Some("Morning Review")
    );
    assert_eq!(
        first_automation.get("scheduleMode").and_then(Value::as_str),
        Some("interval")
    );
    assert_eq!(
        first_automation
            .get("intervalMinutes")
            .and_then(Value::as_i64),
        Some(5)
    );
    assert_eq!(
        first_automation.get("cwd").and_then(Value::as_str),
        Some("/tmp/review")
    );
    assert!(
        first_automation
            .get("nextRunAt")
            .and_then(Value::as_i64)
            .is_some()
    );

    let deleted = delete_automation_payload(&state, "default", "auto-1")
        .await
        .unwrap();
    assert_eq!(
        deleted
            .get("automations")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );

    let _ = fs::remove_dir_all(sandbox);
}
