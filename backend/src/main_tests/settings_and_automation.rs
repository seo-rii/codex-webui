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
async fn codex_home_editor_access_is_limited_to_config_toml() {
    let sandbox = unique_test_dir("editor-codex-home-allowlist");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state(workspace.clone(), vec![workspace], codex_home.clone());
    let allowed_config = codex_home.join("config.toml");
    write_editable_file_payload(
        &state,
        "default",
        allowed_config.to_str().unwrap(),
        "model = 'gpt-5.4'\n",
    )
    .await
    .expect("config.toml should remain editable");

    let other_file = codex_home.join("notes.md");
    fs::write(&other_file, "# private\n").unwrap();
    let error = read_editable_file_payload(&state, "default", other_file.to_str().unwrap())
        .await
        .expect_err("non-config files in CODEX_HOME must be blocked");

    assert_eq!(error.status, StatusCode::FORBIDDEN);
    assert_eq!(
        error.message,
        "Only config.toml is editable inside CODEX_HOME."
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

#[tokio::test]
async fn rejects_binary_editable_file_previews() {
    let sandbox = unique_test_dir("editor-binary-preview");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state(workspace.clone(), vec![workspace.clone()], codex_home);
    let binary_path = workspace.join("image.bin");
    fs::write(&binary_path, [0, 1, 2, 3, 4]).unwrap();
    let error = read_editable_file_payload(&state, "default", binary_path.to_str().unwrap())
        .await
        .expect_err("binary files should not be loaded into editor preview");

    assert_eq!(error.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(
        error.message,
        "The selected file appears to be binary and cannot be previewed."
    );

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
async fn command_runner_timeout_kills_spawned_process_group() {
    let sandbox = unique_test_dir("command-timeout-process-group");
    fs::create_dir_all(&sandbox).unwrap();
    let pid_path = sandbox.join("child.pid");
    let escaped_pid_path = pid_path.to_string_lossy().replace('\'', "'\\''");
    let script = format!("sleep 30 & echo $! > '{escaped_pid_path}'; wait");

    let error = run_command_with_timeout(
        "sh",
        vec!["-c".to_string(), script],
        Duration::from_millis(150),
    )
    .await
    .expect_err("command should time out");

    assert!(format!("{error:#}").contains("timed out"));
    let child_pid = fs::read_to_string(&pid_path)
        .expect("script should write child pid")
        .trim()
        .to_string();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let probe = run_command_with_timeout(
        "kill",
        vec!["-0".to_string(), child_pid],
        Duration::from_secs(2),
    )
    .await
    .expect("probe should run");
    assert!(!probe.status.success());

    let _ = fs::remove_dir_all(sandbox);
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
async fn notification_settings_reject_local_webhook_urls() {
    let sandbox = unique_test_dir("notifications-webhook-policy");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    let error = update_notification_settings_payload(
        &state,
        "default",
        json!({
            "slackWebhookUrl": "https://127.0.0.1/hook",
            "webhookUrl": "http://example.com/hook"
        }),
    )
    .await
    .expect_err("private and non-https webhook URLs should be rejected");

    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        error.message,
        "slackWebhookUrl cannot target a private or local address."
    );
    assert_eq!(
        validate_notification_webhook_url_str("https://printer.local/hook", "webhookUrl")
            .expect_err("local mdns hosts should be rejected")
            .message,
        "webhookUrl cannot target a local address."
    );
    assert!(
        validate_notification_webhook_url_str("https://example.com/hook", "webhookUrl").is_ok()
    );
    assert!(notification_webhook_ip_is_private_or_local(
        "10.0.0.5".parse().unwrap()
    ));
    assert!(notification_webhook_ip_is_private_or_local(
        "fd00::1".parse().unwrap()
    ));
    assert!(!notification_webhook_ip_is_private_or_local(
        "93.184.216.34".parse().unwrap()
    ));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn notification_settings_enforce_webhook_host_allowlist() {
    let sandbox = unique_test_dir("notifications-webhook-allowlist");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let mut state = test_state(workspace.clone(), vec![workspace], codex_home);
    let mut config = (*state.config).clone();
    config.webhook_allowed_hosts = vec![
        "hooks.example.com".to_string(),
        "*.trusted.example".to_string(),
    ];
    state.config = Arc::new(config);

    let error = update_notification_settings_payload(
        &state,
        "default",
        json!({
            "webhookUrl": "https://evil.example.com/hook"
        }),
    )
    .await
    .expect_err("unlisted webhook host should be rejected");
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.message, "webhookUrl host is not allowed.");

    let settings = update_notification_settings_payload(
        &state,
        "default",
        json!({
            "webhookUrl": "https://hooks.example.com/hook",
            "slackWebhookUrl": "https://team.trusted.example/hook"
        }),
    )
    .await
    .expect("listed and wildcard webhook hosts should be accepted");
    assert_eq!(
        settings
            .get("settings")
            .and_then(|value| value.get("webhookUrl"))
            .and_then(Value::as_str),
        Some("https://hooks.example.com/hook")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notification_webhook_resolution_skips_dns_pinning_for_literal_public_ip() {
    let sandbox = unique_test_dir("notifications-webhook-literal-ip");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let state = test_state(workspace.clone(), vec![workspace], codex_home);

    let pinned = resolve_notification_webhook_public_addrs(
        &state.config,
        "https://93.184.216.34/hook",
        "webhookUrl",
    )
    .await
    .expect("public literal IP should pass URL policy without DNS lookup");

    assert!(pinned.is_none());

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn notification_webhook_failures_are_persisted_in_profile_state() {
    let sandbox = unique_test_dir("notification-webhook-failure-history");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    let notification = json!({
        "id": "notification-1",
        "type": "sessionCompleted"
    });
    record_notification_webhook_failure(
        &state,
        "default",
        &notification,
        "webhookUrl",
        "failed with token sk-secret and /home/example/path",
    )
    .await;

    let payload = get_notifications_payload(&state, "default", 20)
        .await
        .expect("notifications payload should load");
    let failure = payload
        .get("webhookFailures")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .expect("webhook failure should be persisted");
    assert_eq!(
        failure.get("notificationId").and_then(Value::as_str),
        Some("notification-1")
    );
    let error = failure
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(!error.contains("sk-secret"));
    assert!(!error.contains("/home/example/path"));

    let _ = fs::remove_dir_all(sandbox);
}

#[test]
fn notification_webhook_deliveries_build_generic_and_slack_payloads() {
    let notification = json!({
        "id": "n1",
        "type": "sessionCompleted",
        "createdAt": 10,
        "readAt": Value::Null,
        "sessionId": "thread-1",
        "sessionName": "Build thread",
        "payload": {
            "status": "completed"
        }
    });
    let deliveries = notification_webhook_deliveries(
        &notification,
        &json!({
            "webhookUrl": "https://example.com/generic",
            "slackWebhookUrl": "https://hooks.slack.test/one"
        }),
    );

    assert_eq!(deliveries.len(), 2);
    assert_eq!(deliveries[0].0, "https://example.com/generic");
    assert_eq!(
        deliveries[0].1.get("event").and_then(Value::as_str),
        Some("sessionCompleted")
    );
    assert_eq!(deliveries[1].0, "https://hooks.slack.test/one");
    assert!(
        deliveries[1]
            .1
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(|text| text.contains("Build thread"))
    );
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
async fn terminal_cleanup_loop_removes_stale_sessions_without_api_polling() {
    let sandbox = unique_test_dir("terminal-cleanup-loop");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state(workspace.clone(), vec![workspace.clone()], codex_home);
    let (relay, _) = broadcast::channel(1);
    let terminal_id = "stale-loop-terminal".to_string();
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

    let cleanup_task = spawn_terminal_cleanup_loop(state.clone(), Duration::from_millis(10));
    for _ in 0..20 {
        if !state.terminals.lock().await.contains_key(&terminal_id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    cleanup_task.abort();
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
async fn session_relay_pruning_removes_idle_stream_relay() {
    let sandbox = unique_test_dir("session-relay-prune");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    let relay = ensure_stream_relay(&state, "default", "thread-1")
        .await
        .expect("relay should be created");
    let receiver = relay.subscribe();
    prune_unused_session_relay(&state, "default", "thread-1").await;
    assert!(
        state
            .relays
            .lock()
            .await
            .contains_key(&session_relay_key("default", "thread-1"))
    );

    drop(receiver);
    prune_unused_session_relay(&state, "default", "thread-1").await;
    assert!(
        !state
            .relays
            .lock()
            .await
            .contains_key(&session_relay_key("default", "thread-1"))
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn session_subscription_drops_slow_outbound_client() {
    let sandbox = unique_test_dir("session-relay-slow-client");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    let (out_tx, _out_rx) = mpsc::channel(1);
    let subscriptions = Arc::new(Mutex::new(HashMap::new()));
    subscribe_session(
        state.clone(),
        out_tx,
        subscriptions.clone(),
        "default".to_string(),
        "thread-1".to_string(),
        UserRole::Admin,
    )
    .await
    .expect("subscription should start");

    let relay = {
        state
            .relays
            .lock()
            .await
            .get(&session_relay_key("default", "thread-1"))
            .cloned()
            .expect("relay should exist while subscribed")
    };
    relay
        .send(json!({ "kind": "delta" }))
        .expect("relay event should publish to subscription");

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if !state
                .relays
                .lock()
                .await
                .contains_key(&session_relay_key("default", "thread-1"))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("slow outbound client should be dropped and relay pruned");
    assert!(
        subscriptions
            .lock()
            .await
            .contains_key(&session_relay_key("default", "thread-1")),
        "subscription registry cleanup is handled by websocket disconnect paths"
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn viewer_session_subscription_redacts_queue_payloads() {
    let sandbox = unique_test_dir("viewer-session-queue-redaction");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    with_ui_state_write(&state, "default", |ui_state| {
        let queues = ui_state
            .get_mut("queuesByThreadId")
            .and_then(Value::as_object_mut)
            .expect("queue state should exist");
        queues.insert(
            "thread-queue".to_string(),
            json!({
                "items": [
                    {
                        "id": "queue-1",
                        "prompt": "secret queued prompt",
                        "skills": [{ "name": "secret-skill" }],
                        "createdAt": 1
                    }
                ],
                "resumePending": true,
                "updatedAt": 2
            }),
        );
        Ok(())
    })
    .await
    .expect("queue fixture should save");

    let (out_tx, mut out_rx) = mpsc::channel(8);
    let subscriptions = Arc::new(Mutex::new(HashMap::new()));
    subscribe_session(
        state.clone(),
        out_tx,
        subscriptions,
        "default".to_string(),
        "thread-queue".to_string(),
        UserRole::Viewer,
    )
    .await
    .expect("viewer subscription should start");

    let envelope = tokio::time::timeout(Duration::from_secs(1), out_rx.recv())
        .await
        .expect("redacted queue event should arrive")
        .expect("redacted queue event should be readable");
    let ServerEnvelope::Event { event, .. } = envelope else {
        panic!("expected session event");
    };
    let queue = event
        .get("params")
        .and_then(|params| params.get("queue"))
        .expect("queue payload should exist");
    assert_eq!(queue.get("itemCount").and_then(Value::as_u64), Some(1));
    assert!(queue.get("items").is_none());
    assert!(queue.to_string().contains("secret queued prompt") == false);
    assert!(queue.to_string().contains("secret-skill") == false);

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn viewer_global_subscription_filters_sensitive_events() {
    let sandbox = unique_test_dir("viewer-global-redaction");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let (out_tx, mut out_rx) = mpsc::channel(8);
    let subscriptions = Arc::new(Mutex::new(HashMap::new()));
    subscribe_global(
        state.clone(),
        out_tx,
        subscriptions,
        "default".to_string(),
        UserRole::Viewer,
    )
    .await
    .expect("viewer global subscription should start");

    emit_profile_config_updated(
        &state,
        "default",
        json!({
            "promptPresets": [{ "name": "secret preset", "prompt": "do secret work" }]
        }),
    )
    .await;
    emit_profile_global_notification(
        &state,
        "default",
        json!({
            "kind": "notification",
            "method": "codex-webui/accountUpdated",
            "params": { "email": "secret@example.com" }
        }),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        out_rx.try_recv().is_err(),
        "viewer should not receive config/account events"
    );

    emit_profile_global_notification(
        &state,
        "default",
        json!({
            "kind": "notification",
            "method": "codex-webui/notificationAdded",
            "params": {
                "notification": {
                    "id": "notice-1",
                    "type": "sessionCompleted",
                    "createdAt": 3,
                    "readAt": Value::Null,
                    "sessionId": "thread-1",
                    "payload": { "secret": "hidden" }
                },
                "unreadCount": 1
            }
        }),
    )
    .await;

    let envelope = tokio::time::timeout(Duration::from_secs(1), out_rx.recv())
        .await
        .expect("redacted notification should arrive")
        .expect("redacted notification should be readable");
    let ServerEnvelope::GlobalEvent { event } = envelope else {
        panic!("expected global event");
    };
    assert_eq!(
        event.get("method").and_then(Value::as_str),
        Some("codex-webui/notificationAdded")
    );
    assert!(!event.to_string().contains("hidden"));

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
async fn ui_state_read_adds_current_schema_version() {
    let sandbox = unique_test_dir("ui-state-schema-version");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    let ui_state_path = profile_ui_state_path(&state.config, "default");
    fs::create_dir_all(ui_state_path.parent().unwrap()).unwrap();
    fs::write(&ui_state_path, "{}").unwrap();

    let loaded = with_ui_state_read(&state, "default", |ui_state| Ok(ui_state.clone()))
        .await
        .expect("ui state should load");

    assert_eq!(loaded.get("schemaVersion").and_then(Value::as_u64), Some(1));
    assert!(loaded.get("queuesByThreadId").is_some_and(Value::is_object));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn ui_state_writes_preserve_previous_snapshot() {
    let sandbox = unique_test_dir("ui-state-backup");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    with_ui_state_write(&state, "default", |ui_state| {
        ui_state["sessionMetaByThreadId"]["thread-1"]["title"] = json!("Before");
        Ok(())
    })
    .await
    .expect("first ui-state write should save");
    with_ui_state_write(&state, "default", |ui_state| {
        ui_state["sessionMetaByThreadId"]["thread-1"]["title"] = json!("After");
        Ok(())
    })
    .await
    .expect("second ui-state write should save");

    let ui_state_path = profile_ui_state_path(&state.config, "default");
    let backup_path = ui_state_path.with_extension("json.bak");
    let raw = fs::read_to_string(&backup_path).expect("ui-state backup should exist");
    let parsed: Value = serde_json::from_str(&raw).expect("ui-state backup should be valid JSON");
    assert_eq!(
        parsed
            .get("sessionMetaByThreadId")
            .and_then(|value| value.get("thread-1"))
            .and_then(|value| value.get("title"))
            .and_then(Value::as_str),
        Some("Before")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn corrupt_ui_state_recovers_from_previous_snapshot() {
    let sandbox = unique_test_dir("ui-state-backup-restore");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    with_ui_state_write(&state, "default", |ui_state| {
        ui_state["sessionMetaByThreadId"]["thread-1"]["title"] = json!("Recover me");
        Ok(())
    })
    .await
    .expect("first ui-state write should save");
    with_ui_state_write(&state, "default", |ui_state| {
        ui_state["sessionMetaByThreadId"]["thread-1"]["title"] = json!("Current");
        Ok(())
    })
    .await
    .expect("second ui-state write should create backup");

    let ui_state_path = profile_ui_state_path(&state.config, "default");
    fs::write(&ui_state_path, b"{broken json").expect("test should corrupt active ui-state");
    state.ui_state_cache.lock().await.clear();

    let restored = with_ui_state_read(&state, "default", |ui_state| Ok(ui_state.clone()))
        .await
        .expect("ui-state should recover from backup");
    assert_eq!(
        restored
            .get("sessionMetaByThreadId")
            .and_then(|value| value.get("thread-1"))
            .and_then(|value| value.get("title"))
            .and_then(Value::as_str),
        Some("Recover me")
    );
    let active_raw =
        fs::read_to_string(&ui_state_path).expect("active ui-state should be restored");
    let active_state: Value =
        serde_json::from_str(&active_raw).expect("active ui-state should be valid JSON");
    let recovery_events = active_state
        .get("global")
        .and_then(|value| value.get("dataRecoveryEvents"))
        .and_then(Value::as_array)
        .expect("ui-state recovery should be recorded");
    let recovery_event = recovery_events
        .first()
        .expect("at least one recovery event should be recorded");
    assert_eq!(
        recovery_event.get("kind").and_then(Value::as_str),
        Some("uiState")
    );
    assert_eq!(
        recovery_event
            .get("restoredFromBackup")
            .and_then(Value::as_bool),
        Some(true)
    );

    let config_payload = get_config_payload(&state, "default")
        .await
        .expect("config payload should surface recovery events");
    let surfaced_events = config_payload
        .get("startup")
        .and_then(|value| value.get("dataRecoveryEvents"))
        .and_then(Value::as_array)
        .expect("config startup payload should include recovery events");
    assert_eq!(surfaced_events.len(), 1);

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn ui_state_cache_is_bounded_across_profiles() {
    let sandbox = unique_test_dir("ui-state-cache-cap");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let mut state = test_state(workspace.clone(), vec![workspace], codex_home);
    let base_profile = state
        .config
        .profiles
        .get("default")
        .cloned()
        .expect("default profile should exist");
    let mut config = (*state.config).clone();
    let mut profiles = std::collections::HashMap::new();
    for index in 0..(UI_STATE_CACHE_MAX_ENTRIES + 8) {
        let profile_id = format!("profile-{index}");
        let mut profile = base_profile.clone();
        profile.data_dir = sandbox.join("profiles").join(&profile_id);
        profiles.insert(profile_id, profile);
    }
    config.default_profile_id = "profile-0".to_string();
    config.profiles = profiles;
    state.config = Arc::new(config);

    for index in 0..(UI_STATE_CACHE_MAX_ENTRIES + 8) {
        let profile_id = format!("profile-{index}");
        with_ui_state_write(&state, &profile_id, |ui_state| {
            ui_state["sessionMetaByThreadId"]["thread"]["title"] = json!(profile_id);
            Ok(())
        })
        .await
        .expect("profile ui-state should save");
    }

    let cache = state.ui_state_cache.lock().await;
    assert!(cache.len() <= UI_STATE_CACHE_MAX_ENTRIES);
    assert!(cache.contains_key(&format!("profile-{}", UI_STATE_CACHE_MAX_ENTRIES + 7)));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn config_payload_uses_fallbacks_when_codex_metadata_is_slow() {
    let sandbox = unique_test_dir("config-metadata-timeout");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let client = app_server_client(&state, "default").await.unwrap();
    for method in ["model/list", "collaborationMode/list", "account/read"] {
        client
            .request(
                "debug/setDelay",
                json!({
                    "method": method,
                    "delayMs": 3_000
                }),
            )
            .await
            .unwrap();
    }
    client
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": "thread-paused-slow",
                    "name": "Paused slow queue",
                    "preview": "",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 1,
                    "status": "idle",
                    "readDelayMs": 3_000,
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
        ui_state["queuesByThreadId"]["thread-paused-slow"] = json!({
            "resumePending": true,
            "updatedAt": now_unix_ms(),
            "items": [
                {
                    "id": "queue-item-1",
                    "prompt": "continue after restart"
                }
            ]
        });
        Ok(())
    })
    .await
    .unwrap();

    let started_at = Instant::now();
    let payload = get_config_payload(&state, "default")
        .await
        .expect("config payload should fall back when Codex metadata is slow");

    assert!(
        started_at.elapsed() < Duration::from_secs(2),
        "config payload should not wait for slow Codex metadata"
    );
    assert_eq!(
        payload
            .get("models")
            .and_then(Value::as_array)
            .and_then(|models| models.first())
            .and_then(|model| model.get("id"))
            .and_then(Value::as_str),
        Some("gpt-5")
    );
    assert_eq!(
        payload
            .get("collaborationModes")
            .and_then(Value::as_array)
            .and_then(|modes| modes.first())
            .and_then(|mode| mode.get("mode"))
            .and_then(Value::as_str),
        Some("default")
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
async fn forced_shutdown_requires_explicit_confirmation_phrase() {
    let sandbox = unique_test_dir("shutdown-force-confirmation");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let mut state = test_state(workspace.clone(), vec![workspace], codex_home);
    let mut config = (*state.config).clone();
    config.system_shutdown_enabled = true;
    config.system_shutdown_command_override = Some("/bin/true".to_string());
    state.config = Arc::new(config);

    let error = force_scheduled_shutdown_payload(&state, "default", &json!({}))
        .await
        .expect_err("forced shutdown should require a confirmation phrase");
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert!(error.message.contains("requires confirmation"));

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
async fn scheduled_shutdown_records_blocked_reason_for_pending_queue() {
    let sandbox = unique_test_dir("shutdown-blocked-reason");
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
        ui_state["queuesByThreadId"]["thread-1"]["items"] = json!([{ "id": "queue-1" }]);
        Ok(())
    })
    .await
    .unwrap();

    maybe_schedule_global_shutdown(&state, "default", None).await;

    let blocked_state = with_ui_state_read(&state, "default", |ui_state| {
        Ok((
            ui_state
                .get("global")
                .and_then(|value| value.get("scheduledShutdown"))
                .cloned()
                .unwrap_or(Value::Null),
            ui_state
                .get("global")
                .and_then(|value| value.get("scheduledShutdownBlockedReason"))
                .and_then(Value::as_str)
                .map(str::to_string),
        ))
    })
    .await
    .unwrap();

    assert!(blocked_state.0.is_null());
    assert_eq!(blocked_state.1.as_deref(), Some("queuedWork"));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn scheduled_shutdown_records_blocked_reason_when_runtime_status_check_fails() {
    let sandbox = unique_test_dir("shutdown-active-check-failure");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let mut state = test_state(workspace.clone(), vec![workspace], codex_home);
    let missing_codex = sandbox.join("missing-codex-bin");
    let mut config = (*state.config).clone();
    config.system_shutdown_enabled = true;
    config.system_shutdown_command_override = Some("/bin/true".to_string());
    config.codex_bin = missing_codex.display().to_string();
    state.config = Arc::new(config);
    state.app_servers = AppServerManager::new(AppServerClientConfig {
        codex_bin: missing_codex.display().to_string(),
        ..AppServerClientConfig::default()
    });

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

    maybe_schedule_global_shutdown(&state, "default", None).await;

    let blocked_state = with_ui_state_read(&state, "default", |ui_state| {
        Ok((
            ui_state
                .get("global")
                .and_then(|value| value.get("scheduledShutdown"))
                .cloned()
                .unwrap_or(Value::Null),
            ui_state
                .get("global")
                .and_then(|value| value.get("scheduledShutdownBlockedReason"))
                .and_then(Value::as_str)
                .map(str::to_string),
        ))
    })
    .await
    .unwrap();

    assert!(blocked_state.0.is_null());
    assert_eq!(blocked_state.1.as_deref(), Some("activeWork"));

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
    let temp_files = fs::read_dir(&codex_home)
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with(".codex-webui-state-config.toml-"))
        .collect::<Vec<_>>();
    assert!(temp_files.is_empty());

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

#[tokio::test]
async fn automation_run_rejects_duplicate_active_run() {
    let sandbox = unique_test_dir("automation-duplicate-run");
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
            "automations": [{
                "id": "auto-1",
                "name": "Daily task",
                "prompt": "Check status.",
                "enabled": false,
                "scheduleMode": "manual",
                "intervalMinutes": Value::Null,
                "target": "local",
                "repoPath": Value::Null,
                "cwd": Value::Null,
                "model": Value::Null,
                "effort": Value::Null,
                "speed": Value::Null,
                "mode": Value::Null,
                "createdAt": 1,
                "updatedAt": 1,
                "lastRunAt": Value::Null,
                "nextRunAt": Value::Null
            }],
            "automationRuns": [{
                "id": "run-1",
                "automationId": "auto-1",
                "automationName": "Daily task",
                "status": "started",
                "trigger": "manual",
                "sessionId": "thread-auto",
                "repoPath": Value::Null,
                "cwd": Value::Null,
                "worktreePath": Value::Null,
                "startedAt": 1,
                "completedAt": Value::Null,
                "error": Value::Null
            }],
            "preferencesByThreadId": {},
            "draftsByThreadId": {},
            "queuesByThreadId": {},
            "highlightsByThreadId": {}
        }))
        .unwrap(),
    )
    .unwrap();

    let error = run_automation_payload(&state, "default", "auto-1", "manual")
        .await
        .expect_err("duplicate active automation runs should be rejected before app-server work");

    assert_eq!(error.status, StatusCode::CONFLICT);
    assert_eq!(error.message, "Automation is already running.");

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn runtime_completion_marks_started_automation_run_completed() {
    let sandbox = unique_test_dir("automation-completion");
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
            "automationRuns": [{
                "id": "run-1",
                "automationId": "auto-1",
                "automationName": "Daily task",
                "status": "started",
                "trigger": "manual",
                "sessionId": "thread-auto",
                "repoPath": Value::Null,
                "cwd": Value::Null,
                "worktreePath": Value::Null,
                "startedAt": 1,
                "completedAt": Value::Null,
                "error": Value::Null
            }],
            "preferencesByThreadId": {},
            "draftsByThreadId": {},
            "queuesByThreadId": {},
            "highlightsByThreadId": {}
        }))
        .unwrap(),
    )
    .unwrap();

    handle_profile_runtime_notification(
        &state,
        "default",
        &AppServerNotification {
            method: "turn/completed".to_string(),
            params: json!({
                "threadId": "thread-auto",
                "turnId": "turn-auto"
            }),
        },
    )
    .await;

    let runs = with_ui_state_read(&state, "default", |ui_state| {
        Ok(recent_automation_runs_from_ui_state(ui_state, 10))
    })
    .await
    .unwrap();
    let run = runs.first().expect("run should remain available");
    assert_eq!(run.get("status").and_then(Value::as_str), Some("completed"));
    assert!(run.get("completedAt").and_then(Value::as_i64).is_some());

    let _ = fs::remove_dir_all(sandbox);
}
