use super::*;

#[tokio::test]
async fn goal_cache_does_not_regress_to_an_older_notification() {
    let sandbox = unique_test_dir("goal-cache-ordering");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    let session_id = "thread-goal-ordering";

    cache_session_goal_payload(
        &state,
        "default",
        session_id,
        &json!({
            "threadId": session_id,
            "objective": "new objective",
            "status": "active",
            "updatedAt": 200
        }),
    )
    .await;
    cache_session_goal_payload(
        &state,
        "default",
        session_id,
        &json!({
            "threadId": session_id,
            "objective": "stale objective",
            "status": "paused",
            "updatedAt": 100
        }),
    )
    .await;

    let cached = cached_session_goal_or_null_payload(&state, "default", session_id).await;
    assert_eq!(
        cached.get("objective").and_then(Value::as_str),
        Some("new objective")
    );
    assert_eq!(cached.get("status").and_then(Value::as_str), Some("active"));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn session_operation_locks_are_scoped_by_profile_and_session() {
    let sandbox = unique_test_dir("session-operation-locks");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    let state = test_state(workspace.clone(), vec![workspace], codex_home);

    let first = session_operation_lock(&state, "default", "thread-1").await;
    let same = session_operation_lock(&state, "default", "thread-1").await;
    let other = session_operation_lock(&state, "other", "thread-1").await;

    assert!(Arc::ptr_eq(&first, &same));
    assert!(!Arc::ptr_eq(&first, &other));

    let _ = fs::remove_dir_all(sandbox);
}

#[test]
fn error_redaction_removes_every_repeated_credential() {
    let redacted = redact_user_facing_error(
        "Bearer first-token then Bearer second-token access_token=one access_token=two",
    );
    assert!(!redacted.contains("first-token"));
    assert!(!redacted.contains("second-token"));
    assert!(!redacted.contains("=one"));
    assert!(!redacted.contains("=two"));
    assert_eq!(redacted.matches("[redacted]").count(), 4);
}

#[tokio::test]
async fn websocket_request_profile_overrides_the_connection_default_without_reconnect() {
    let sandbox = unique_test_dir("ws-request-profile-override");
    let workspace = sandbox.join("workspace");
    let default_home = sandbox.join("codex-default");
    let second_home = sandbox.join("codex-second");
    fs::create_dir_all(&workspace).unwrap();
    let mut state = test_state(workspace.clone(), vec![workspace], default_home);
    let mut config = (*state.config).clone();
    config.profiles.insert(
        "second".to_string(),
        RuntimeProfile {
            label: "Second".to_string(),
            codex_home: second_home,
            data_dir: sandbox.join("data-second"),
        },
    );
    state.config = Arc::new(config);
    let (out_tx, _out_rx) = mpsc::channel(8);
    let subscriptions = Arc::new(Mutex::new(HashMap::new()));
    let auth = AuthContext {
        role: UserRole::Admin,
        profile_id: "default".to_string(),
    };

    let payload = execute_ws_method(
        &state,
        &out_tx,
        &subscriptions,
        &auth,
        "config/get",
        json!({ "requestProfileId": "second" }),
    )
    .await
    .unwrap();

    assert!(
        payload
            .get("profiles")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .any(|profile| {
                profile.get("id").and_then(Value::as_str) == Some("second")
                    && profile.get("active").and_then(Value::as_bool) == Some(true)
            })
    );
    let _ = fs::remove_dir_all(sandbox);
}
