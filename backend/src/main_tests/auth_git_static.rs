use super::*;

#[test]
fn detects_invalid_refresh_token_errors() {
    assert!(is_invalid_refresh_token_error_message(
        "Auth(TokenRefreshFailed(\"Server returned error response: invalid_grant: Invalid refresh token\"))"
    ));
    assert!(!is_invalid_refresh_token_error_message(
        "some other runtime failure"
    ));
}

#[test]
fn maps_account_login_completed_notifications() {
    let mapped = map_app_server_global_notification(&AppServerNotification {
        method: "account/login/completed".to_string(),
        params: json!({
            "loginId": "login-1",
            "success": true
        }),
    })
    .expect("notification should map");

    assert_eq!(
        mapped,
        json!({
            "kind": "notification",
            "method": "codex-webui/accountLoginCompleted",
            "params": {
                "loginId": "login-1",
                "success": true,
                "error": Value::Null
            }
        })
    );
}

#[tokio::test]
async fn account_login_uses_browser_base_url_for_oauth_callback() {
    let sandbox = unique_test_dir("account-login-browser-callback");
    let codex_home = sandbox.join("codex-home");
    let mut state = test_state(sandbox.clone(), vec![sandbox.clone()], codex_home);
    let mut config = (*state.config).clone();
    config.base_path = "/absproxy/4173".to_string();
    state.config = Arc::new(config);

    let response = start_account_login(
        &state,
        "default",
        &json!({
            "type": "chatgpt",
            "browserBaseUrl": "https://dev.seorii.io/absproxy/4173/"
        }),
    )
    .await
    .expect("browser login should start");

    assert_eq!(
        response.get("type").and_then(Value::as_str),
        Some("chatgpt")
    );
    let auth_url = response
        .get("authUrl")
        .and_then(Value::as_str)
        .expect("auth url should be returned");
    assert!(!auth_url.contains("localhost"));
    let parsed = reqwest::Url::parse(auth_url).expect("auth url should parse");
    assert_eq!(
        query_param_value(parsed.query(), "redirect_uri").as_deref(),
        Some("https://dev.seorii.io/absproxy/4173/api/account/oauth/callback")
    );

    let login_id = response
        .get("loginId")
        .and_then(Value::as_str)
        .expect("login id should be returned");
    let flows = state.account_login_flows.lock().await;
    let flow = flows
        .get(login_id)
        .expect("pending login flow should be stored");
    assert_eq!(
        flow.redirect_uri,
        "https://dev.seorii.io/absproxy/4173/api/account/oauth/callback"
    );
    assert_eq!(flow.return_url, "https://dev.seorii.io/absproxy/4173");
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminate_process_hard_kills_term_ignoring_process_group() {
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg("trap '' TERM; sleep 30")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command.process_group(0);
    let mut child = command.spawn().expect("test process should start");
    let pid = child.id().expect("test process should have a pid");

    terminate_process(pid)
        .await
        .expect("terminal process should terminate");
    let status = tokio::time::timeout(Duration::from_secs(3), child.wait())
        .await
        .expect("process should exit after hard kill")
        .expect("wait should succeed");
    assert!(!status.success());
}

#[cfg(target_os = "linux")]
#[test]
fn terminal_process_identity_parses_proc_stat_and_matches_current_process() {
    let parsed = parse_terminal_process_identity(
        123,
        "123 (script worker) S 1 123 123 0 -1 4194304 1 2 3 4 5 6 7 8 20 0 1 0 987654321 0",
    )
    .expect("proc stat identity should parse");
    assert_eq!(parsed.pid, 123);
    assert_eq!(parsed.process_group_id, 123);
    assert_eq!(parsed.start_time_ticks, 987654321);

    let current_pid = std::process::id();
    let current = read_terminal_process_identity(current_pid)
        .expect("current linux process identity should be readable");
    assert_eq!(current.pid, current_pid);
    let wrong_start_time = TerminalProcessIdentity {
        start_time_ticks: current.start_time_ticks.saturating_add(1),
        ..current
    };
    assert!(terminal_process_identity_matches(current_pid, current));
    assert!(!terminal_process_identity_matches(
        current_pid,
        wrong_start_time
    ));
}

#[test]
fn user_facing_error_redaction_hides_paths_and_tokens() {
    let home = env::var("HOME").unwrap_or_else(|_| "/home/example".to_string());
    let redacted = redact_user_facing_error(&format!(
        "failed at {home}/.codex/auth.json with Bearer sk-secret access_token=abc123 password:super-secret \"refresh_token\":\"refresh-secret\""
    ));

    assert!(!redacted.contains("sk-secret"));
    assert!(!redacted.contains("abc123"));
    assert!(!redacted.contains("super-secret"));
    assert!(!redacted.contains("refresh-secret"));
    if home != "/" {
        assert!(!redacted.contains(&home));
    }
    assert!(redacted.contains("[redacted]"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_password_authenticates_as_owner_role() {
    let sandbox = unique_test_dir("owner-password-role");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let mut config = (*test_state(workspace.clone(), vec![workspace], codex_home).config).clone();
    config.owner_password = Some("owner-secret".to_string());
    config.password = Some("admin-secret".to_string());

    assert_eq!(
        authenticate_role(&config, "owner-secret").unwrap(),
        Some(UserRole::Owner)
    );
    assert_eq!(
        authenticate_role(&config, "admin-secret").unwrap(),
        Some(UserRole::Admin)
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn revoked_auth_cookie_is_rejected_server_side() {
    let sandbox = unique_test_dir("auth-cookie-revocation");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let mut state = test_state(workspace.clone(), vec![workspace], codex_home);
    let mut config = (*state.config).clone();
    config.owner_password = Some("owner-secret".to_string());
    state.config = Arc::new(config);
    let jar = issue_auth_cookie(&state.config, CookieJar::new(), false, UserRole::Admin).unwrap();

    assert_eq!(
        auth_context(&state.config, &jar).map(|auth| auth.role),
        Some(UserRole::Admin)
    );
    assert!(revoke_auth_cookie(&state.config, &jar));
    assert!(auth_context(&state.config, &jar).is_none());
    assert!(
        state
            .config
            .data_dir
            .join("auth-revocations.jsonl")
            .exists()
    );

    let fresh_jar =
        issue_auth_cookie(&state.config, CookieJar::new(), false, UserRole::Admin).unwrap();
    assert_eq!(
        auth_context(&state.config, &fresh_jar).map(|auth| auth.role),
        Some(UserRole::Admin)
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[test]
fn persisted_auth_revocation_file_is_loaded_on_first_auth_check() {
    let sandbox = unique_test_dir("auth-cookie-revocation-reload");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let mut state = test_state(workspace.clone(), vec![workspace], codex_home);
    let mut config = (*state.config).clone();
    config.owner_password = Some("owner-secret".to_string());
    state.config = Arc::new(config);
    let jar = issue_auth_cookie(&state.config, CookieJar::new(), false, UserRole::Admin).unwrap();
    let token = jar.get(AUTH_COOKIE).unwrap().value();
    let token_parts = token.split('.').collect::<Vec<_>>();
    let expires = token_parts[1].parse::<u128>().unwrap();
    let nonce = token_parts[3];
    fs::create_dir_all(&state.config.data_dir).unwrap();
    fs::write(
        state.config.data_dir.join("auth-revocations.jsonl"),
        format!(
            "{}\n",
            serde_json::to_string(&json!({
                "nonce": nonce,
                "expires": expires,
                "revokedAt": now_millis()
            }))
            .unwrap()
        ),
    )
    .unwrap();

    assert!(auth_context(&state.config, &jar).is_none());

    let _ = fs::remove_dir_all(sandbox);
}

#[test]
fn public_host_rejects_plaintext_password_env_values() {
    let sandbox = unique_test_dir("public-host-plaintext-passwords");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let mut config = (*test_state(workspace.clone(), vec![workspace], codex_home).config).clone();
    config.public_host = "0.0.0.0".to_string();
    config.password = Some("admin-secret".to_string());
    config.owner_password = Some("owner-secret".to_string());
    config.viewer_password = Some("viewer-secret".to_string());

    let error = validate_plaintext_password_policy(&config)
        .expect_err("public host should reject plaintext secrets");
    let message = error.to_string();
    assert!(message.contains("CODEX_WEBUI_PASSWORD"));
    assert!(message.contains("CODEX_WEBUI_OWNER_PASSWORD"));
    assert!(message.contains("CODEX_WEBUI_VIEWER_PASSWORD"));

    config.password = None;
    config.owner_password = None;
    config.viewer_password = None;
    config.password_hash = Some("scrypt$v1$salt$key".to_string());
    config.owner_password_hash = Some("scrypt$v1$salt$key".to_string());
    config.viewer_password_hash = Some("scrypt$v1$salt$key".to_string());
    validate_plaintext_password_policy(&config)
        .expect("public host should allow hash-only secrets");

    config.public_host = "127.0.0.1".to_string();
    config.password = Some("admin-secret".to_string());
    validate_plaintext_password_policy(&config)
        .expect("loopback host should keep local plaintext setup support");

    let _ = fs::remove_dir_all(sandbox);
}

#[test]
fn auth_cookie_round_trips_owner_role() {
    let sandbox = unique_test_dir("owner-cookie-role");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let config = (*test_state(workspace.clone(), vec![workspace], codex_home).config).clone();
    let jar = issue_auth_cookie(&config, CookieJar::new(), false, UserRole::Owner).unwrap();
    let auth = auth_context(&config, &jar).expect("owner cookie should authenticate");

    assert_eq!(auth.role, UserRole::Owner);

    let _ = fs::remove_dir_all(sandbox);
}

#[test]
fn duplicate_auth_cookie_headers_prefer_owner_role() {
    let sandbox = unique_test_dir("duplicate-auth-cookie-owner-role");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let config = (*test_state(workspace.clone(), vec![workspace], codex_home).config).clone();
    let admin_jar = issue_auth_cookie(&config, CookieJar::new(), false, UserRole::Admin).unwrap();
    let owner_jar = issue_auth_cookie(&config, CookieJar::new(), false, UserRole::Owner).unwrap();
    let admin_token = admin_jar
        .get(AUTH_COOKIE)
        .expect("admin auth cookie should be issued")
        .value()
        .to_string();
    let owner_token = owner_jar
        .get(AUTH_COOKIE)
        .expect("owner auth cookie should be issued")
        .value()
        .to_string();
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        HeaderValue::from_str(&format!(
            "{AUTH_COOKIE}={admin_token}; {AUTH_COOKIE}={owner_token}"
        ))
        .unwrap(),
    );

    let auth = auth_context_from_headers(&config, &admin_jar, &headers)
        .expect("duplicate auth cookies should authenticate");

    assert_eq!(auth.role, UserRole::Owner);

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn logout_revokes_duplicate_auth_cookie_headers() {
    let sandbox = unique_test_dir("logout-revokes-duplicate-auth-cookies");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let mut state = test_state(workspace.clone(), vec![workspace], codex_home);
    let mut config = (*state.config).clone();
    config.owner_password = Some("owner-secret".to_string());
    state.config = Arc::new(config);
    let admin_jar =
        issue_auth_cookie(&state.config, CookieJar::new(), false, UserRole::Admin).unwrap();
    let owner_jar =
        issue_auth_cookie(&state.config, CookieJar::new(), false, UserRole::Owner).unwrap();
    let admin_token = admin_jar
        .get(AUTH_COOKIE)
        .expect("admin auth cookie should be issued")
        .value()
        .to_string();
    let owner_token = owner_jar
        .get(AUTH_COOKIE)
        .expect("owner auth cookie should be issued")
        .value()
        .to_string();
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        HeaderValue::from_str(&format!(
            "{AUTH_COOKIE}={admin_token}; {AUTH_COOKIE}={owner_token}"
        ))
        .unwrap(),
    );
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/auth/logout")
        .body(Body::empty())
        .unwrap();

    let response = handle_auth_http(
        state.clone(),
        admin_jar.clone(),
        Method::POST,
        "/api/auth/logout".to_string(),
        headers.clone(),
        request,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert!(auth_context_from_headers(&state.config, &admin_jar, &headers).is_none());

    let _ = fs::remove_dir_all(sandbox);
}

#[test]
fn system_shutdown_enabled_requires_owner_for_owner_only_actions() {
    let sandbox = unique_test_dir("shutdown-owner-required");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    let mut config = (*state.config).clone();

    assert!(role_has_owner_access(&config, UserRole::Admin));
    config.system_shutdown_enabled = true;
    assert!(!role_has_owner_access(&config, UserRole::Admin));
    assert!(role_has_owner_access(&config, UserRole::Owner));
    assert!(
        authorize_ws_method(&config, UserRole::Admin, "terminal/create", &json!({}))
            .expect_err("shutdown-enabled deployments should require owner preflight")
            .to_string()
            .contains("OWNER_REQUIRED")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_config_blocks_admin_from_owner_only_websocket_methods() {
    let sandbox = unique_test_dir("owner-only-ws-method");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let mut state = test_state(workspace.clone(), vec![workspace], codex_home);
    let mut config = (*state.config).clone();
    config.owner_password = Some("owner-secret".to_string());
    state.config = Arc::new(config);
    let (out_tx, _out_rx) = mpsc::channel(8);
    let subscriptions = Arc::new(Mutex::new(HashMap::new()));
    let auth = AuthContext {
        profile_id: "default".to_string(),
        role: UserRole::Admin,
    };

    let error = execute_ws_method(
        &state,
        &out_tx,
        &subscriptions,
        &auth,
        "runtime/install",
        json!({}),
    )
    .await
    .expect_err("admin should not run owner-only method when owner role is configured");

    assert!(error.to_string().contains("OWNER_REQUIRED"));
    assert!(is_ws_method_allowed(UserRole::Owner, "terminal/create"));
    assert!(ws_method_requires_owner("terminal/create", &json!({})));
    assert!(ws_method_requires_owner(
        "session/savePreferences",
        &json!({ "preferences": { "autoApproveMode": "session" } })
    ));
    assert!(ws_method_requires_owner(
        "turn/send",
        &json!({ "preferences": { "approvalPolicy": "never" } })
    ));
    assert!(ws_method_requires_owner(
        "arena/start",
        &json!({ "preferences": { "sandboxMode": "danger-full-access" } })
    ));
    assert!(ws_method_requires_owner(
        "git/worktrees/remove",
        &json!({ "force": true })
    ));
    assert!(ws_method_requires_owner(
        "system/shutdown/force",
        &json!({})
    ));
    assert!(!ws_method_requires_owner(
        "git/worktrees/remove",
        &json!({ "force": false })
    ));
    assert!(
        authorize_ws_method(
            &state.config,
            UserRole::Admin,
            "system/shutdown/force",
            &json!({})
        )
        .expect_err("admin should not force shutdown when owner role is configured")
        .to_string()
        .contains("OWNER_REQUIRED")
    );
    assert!(
        authorize_ws_method(
            &state.config,
            UserRole::Admin,
            "runtime/install",
            &json!({})
        )
        .expect_err("admin should fail owner preflight")
        .to_string()
        .contains("OWNER_REQUIRED")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_profile_request_slots_are_bounded() {
    let sandbox = unique_test_dir("ws-profile-slots");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let state = test_state(workspace.clone(), vec![workspace], codex_home);

    let mut permits = Vec::new();
    for _ in 0..WS_MAX_PROFILE_CONCURRENT_REQUESTS {
        permits.push(
            try_acquire_profile_ws_request_slot(&state, "default")
                .await
                .expect("slot should be available within profile limit"),
        );
    }
    assert!(
        try_acquire_profile_ws_request_slot(&state, "default")
            .await
            .is_none()
    );

    permits.pop();
    assert!(
        try_acquire_profile_ws_request_slot(&state, "default")
            .await
            .is_some()
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[test]
fn external_or_configured_owner_modes_require_owner_role_for_owner_actions() {
    let sandbox = unique_test_dir("owner-secure-mode");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    let mut config = (*state.config).clone();

    assert!(role_has_owner_access(&config, UserRole::Admin));

    config.public_host = "0.0.0.0".to_string();
    assert!(!role_has_owner_access(&config, UserRole::Admin));
    assert!(role_has_owner_access(&config, UserRole::Owner));

    config.public_host = "127.0.0.1".to_string();
    config.system_shutdown_enabled = true;
    assert!(!role_has_owner_access(&config, UserRole::Admin));

    config.system_shutdown_enabled = false;
    config.require_owner_role = true;
    assert!(!role_has_owner_access(&config, UserRole::Admin));
    assert!(role_has_owner_access(&config, UserRole::Owner));
    config.require_owner_role = false;
    config.owner_password = Some("owner-secret".to_string());
    assert!(!role_has_owner_access(&config, UserRole::Admin));
    assert!(role_has_owner_access(&config, UserRole::Owner));

    let _ = fs::remove_dir_all(sandbox);
}

#[test]
fn session_summary_normalizes_live_thread_without_runtime_evidence_to_completed() {
    let mut snapshot = SessionSummaryUiSnapshot {
        loaded_thread_ids_available: true,
        ..SessionSummaryUiSnapshot::default()
    };
    let thread = json!({
        "id": "thread-live",
        "name": "Live on disk",
        "preview": "Live on disk",
        "status": "active",
        "archived": false,
        "createdAt": 1,
        "updatedAt": 2
    });
    let summary = build_session_summary_from_thread_payload(&thread, &snapshot, None, None)
        .expect("summary should be built");
    assert_eq!(
        summary.get("status").and_then(Value::as_str),
        Some("completed")
    );

    snapshot.loaded_thread_ids.insert("thread-live".to_string());
    let loaded_without_activity =
        build_session_summary_from_thread_payload(&thread, &snapshot, None, None)
            .expect("loaded summary should be built");
    assert_eq!(
        loaded_without_activity
            .get("status")
            .and_then(Value::as_str),
        Some("completed")
    );

    snapshot.active_thread_ids.insert("thread-live".to_string());
    let loaded_summary = build_session_summary_from_thread_payload(&thread, &snapshot, None, None)
        .expect("loaded summary should be built");
    assert_eq!(
        loaded_summary.get("status").and_then(Value::as_str),
        Some("running")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_loopback_without_auth_config_defaults_to_admin() {
    let sandbox = unique_test_dir("local-authless-admin");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let state = test_state(workspace.clone(), vec![workspace], codex_home);

    let auth = auth_context(&state.config, &CookieJar::new())
        .expect("local loopback without auth config should default to admin");
    assert_eq!(auth.role, UserRole::Admin);
    assert_eq!(
        authenticate_role(&state.config, "").unwrap(),
        Some(UserRole::Admin)
    );

    let request = Request::builder()
        .method(Method::GET)
        .uri("/api/config")
        .body(Body::empty())
        .unwrap();
    let response = handle_http(State(state), CookieJar::new(), request).await;
    assert_ne!(response.status(), StatusCode::UNAUTHORIZED);
    assert_ne!(response.status(), StatusCode::FORBIDDEN);

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_host_without_auth_config_still_requires_authentication() {
    let sandbox = unique_test_dir("external-auth-required");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let mut state = test_state(workspace.clone(), vec![workspace], codex_home);
    let mut config = (*state.config).clone();
    config.public_host = "0.0.0.0".to_string();
    state.config = Arc::new(config);

    assert!(auth_context(&state.config, &CookieJar::new()).is_none());
    assert!(
        authenticate_role(&state.config, "")
            .unwrap_err()
            .to_string()
            .contains("Set CODEX_WEBUI_PASSWORD_HASH")
    );

    let request = Request::builder()
        .method(Method::GET)
        .uri("/api/config")
        .body(Body::empty())
        .unwrap();
    let response = handle_http(State(state), CookieJar::new(), request).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_response_cache_is_partitioned_by_role_method_and_params() {
    let sandbox = unique_test_dir("ws-cache-partition");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    let request_id = "client-controlled-id";
    let method = "config/set";
    let params = json!({ "value": "admin-only" });
    let params_hash = request_params_hash(&params);
    let admin_key = request_cache_key("default", request_id, UserRole::Admin);
    cache_response(
        &state,
        &admin_key,
        method,
        &params_hash,
        ServerEnvelope::Response {
            id: request_id.to_string(),
            ok: true,
            result: Some(json!({ "secret": true })),
            error: None,
        },
    )
    .await;

    let viewer_key = request_cache_key("default", request_id, UserRole::Viewer);
    assert!(matches!(
        cached_response(&state, &viewer_key, method, &params_hash).await,
        CachedResponseLookup::Miss
    ));
    assert!(matches!(
        cached_response(&state, &admin_key, "runtime/status", &params_hash).await,
        CachedResponseLookup::Conflict
    ));

    let different_params_hash = request_params_hash(&json!({ "value": "different" }));
    assert!(matches!(
        cached_response(&state, &admin_key, method, &different_params_hash).await,
        CachedResponseLookup::Conflict
    ));
    assert!(matches!(
        cached_response(&state, &admin_key, method, &params_hash).await,
        CachedResponseLookup::Hit(_)
    ));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_inflight_requests_reject_id_reuse_with_different_payloads() {
    let sandbox = unique_test_dir("ws-inflight-idempotency");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    let request_key = request_cache_key("default", "client-id", UserRole::Admin);
    let params_hash = request_params_hash(&json!({ "value": 1 }));
    let (first_tx, _first_rx) = mpsc::channel(8);
    let (second_tx, _second_rx) = mpsc::channel(8);

    assert!(matches!(
        register_inflight_request(&state, &request_key, "session/get", &params_hash, &first_tx)
            .await,
        InflightRequestRegistration::Started
    ));
    assert!(matches!(
        register_inflight_request(
            &state,
            &request_key,
            "session/get",
            &params_hash,
            &second_tx
        )
        .await,
        InflightRequestRegistration::Joined
    ));

    let different_params_hash = request_params_hash(&json!({ "value": 2 }));
    assert!(matches!(
        register_inflight_request(
            &state,
            &request_key,
            "session/get",
            &different_params_hash,
            &second_tx
        )
        .await,
        InflightRequestRegistration::Conflict
    ));
    assert!(matches!(
        register_inflight_request(
            &state,
            &request_key,
            "session/list",
            &params_hash,
            &second_tx
        )
        .await,
        InflightRequestRegistration::Conflict
    ));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_inflight_requests_limit_join_waiters() {
    let sandbox = unique_test_dir("ws-inflight-waiter-cap");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    let request_key = request_cache_key("default", "client-id", UserRole::Admin);
    let params_hash = request_params_hash(&json!({ "value": 1 }));
    let (first_tx, _first_rx) = mpsc::channel(8);

    assert!(matches!(
        register_inflight_request(&state, &request_key, "session/get", &params_hash, &first_tx)
            .await,
        InflightRequestRegistration::Started
    ));

    for _ in 1..INFLIGHT_REQUEST_MAX_WAITERS {
        let (tx, _rx) = mpsc::channel(8);
        assert!(matches!(
            register_inflight_request(&state, &request_key, "session/get", &params_hash, &tx).await,
            InflightRequestRegistration::Joined
        ));
    }

    let (overflow_tx, _overflow_rx) = mpsc::channel(8);
    assert!(matches!(
        register_inflight_request(
            &state,
            &request_key,
            "session/get",
            &params_hash,
            &overflow_tx
        )
        .await,
        InflightRequestRegistration::Full
    ));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_inflight_resolution_drops_saturated_waiters_without_blocking() {
    let sandbox = unique_test_dir("ws-inflight-saturated-waiter");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    let request_key = request_cache_key("default", "client-id", UserRole::Admin);
    let params_hash = request_params_hash(&json!({ "value": 1 }));
    let (tx, _rx) = mpsc::channel(1);
    tx.try_send(ServerEnvelope::Pong { nonce: None })
        .expect("test channel should fill");

    assert!(matches!(
        register_inflight_request(&state, &request_key, "session/get", &params_hash, &tx).await,
        InflightRequestRegistration::Started
    ));

    tokio::time::timeout(
        Duration::from_millis(100),
        resolve_inflight_request(
            &state,
            &request_key,
            ServerEnvelope::Response {
                id: "client-id".to_string(),
                ok: true,
                result: Some(json!({ "ok": true })),
                error: None,
            },
        ),
    )
    .await
    .expect("saturated websocket waiter should not block inflight resolution");

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_response_cache_prunes_oldest_entries_at_cap() {
    let sandbox = unique_test_dir("ws-cache-cap");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    let now = Instant::now();

    {
        let mut cache = state.response_cache.lock().await;
        for index in 0..=RESPONSE_CACHE_MAX_ENTRIES {
            cache.insert(
                format!("key-{index}"),
                CachedResponse {
                    created_at: now
                        - Duration::from_millis(
                            (RESPONSE_CACHE_MAX_ENTRIES.saturating_sub(index)) as u64,
                        ),
                    method: "runtime/status".to_string(),
                    params_hash: request_params_hash(&json!({ "index": index })),
                    response_bytes: 64,
                    message: ServerEnvelope::Response {
                        id: index.to_string(),
                        ok: true,
                        result: Some(json!({ "index": index })),
                        error: None,
                    },
                },
            );
        }
    }

    cache_response(
        &state,
        "new-key",
        "runtime/status",
        &request_params_hash(&json!({ "new": true })),
        ServerEnvelope::Response {
            id: "new-key".to_string(),
            ok: true,
            result: Some(json!({ "new": true })),
            error: None,
        },
    )
    .await;

    let cache = state.response_cache.lock().await;
    assert!(cache.len() <= RESPONSE_CACHE_MAX_ENTRIES);
    assert!(!cache.contains_key("key-0"));
    assert!(cache.contains_key("new-key"));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_response_cache_skips_oversized_entries() {
    let sandbox = unique_test_dir("ws-cache-entry-size");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    let params_hash = request_params_hash(&json!({}));
    let request_key = request_cache_key("default", "large-response", UserRole::Admin);

    cache_response(
        &state,
        &request_key,
        "session/get",
        &params_hash,
        ServerEnvelope::Response {
            id: "large-response".to_string(),
            ok: true,
            result: Some(json!({ "payload": "x".repeat(RESPONSE_CACHE_MAX_ENTRY_BYTES + 1) })),
            error: None,
        },
    )
    .await;

    assert!(matches!(
        cached_response(&state, &request_key, "session/get", &params_hash).await,
        CachedResponseLookup::Miss
    ));

    let _ = fs::remove_dir_all(sandbox);
}

#[test]
fn websocket_origin_allows_same_origin_and_configured_cors_only() {
    let sandbox = unique_test_dir("ws-origin-policy");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let mut state = test_state(workspace.clone(), vec![workspace], codex_home);

    let mut same_origin_headers = HeaderMap::new();
    same_origin_headers.insert(header::HOST, HeaderValue::from_static("127.0.0.1:4173"));
    same_origin_headers.insert(
        header::ORIGIN,
        HeaderValue::from_static("http://127.0.0.1:4173"),
    );
    let loopback_peer: SocketAddr = "127.0.0.1:51234".parse().unwrap();
    let remote_peer: SocketAddr = "203.0.113.10:51234".parse().unwrap();
    assert!(websocket_origin_allowed(
        &state.config,
        &same_origin_headers,
        Some(loopback_peer)
    ));

    let mut forwarded_https_headers = same_origin_headers.clone();
    forwarded_https_headers.insert(
        header::ORIGIN,
        HeaderValue::from_static("https://127.0.0.1:4173"),
    );
    forwarded_https_headers.insert("x-forwarded-proto", HeaderValue::from_static("https, http"));
    assert!(!request_is_secure(
        &state.config,
        &forwarded_https_headers,
        Some(loopback_peer)
    ));
    assert!(!websocket_origin_allowed(
        &state.config,
        &forwarded_https_headers,
        Some(loopback_peer)
    ));

    let mut trusted_proxy_config = (*state.config).clone();
    trusted_proxy_config.trust_proxy_headers = true;
    assert!(request_is_secure(
        &trusted_proxy_config,
        &forwarded_https_headers,
        Some(loopback_peer)
    ));
    assert!(websocket_origin_allowed(
        &trusted_proxy_config,
        &forwarded_https_headers,
        Some(loopback_peer)
    ));
    assert!(!request_is_secure(
        &trusted_proxy_config,
        &forwarded_https_headers,
        Some(remote_peer)
    ));

    trusted_proxy_config.trusted_proxy_cidrs = vec![TrustedProxyNet {
        addr: "203.0.113.0".parse().unwrap(),
        prefix: 24,
    }];
    assert!(request_is_secure(
        &trusted_proxy_config,
        &forwarded_https_headers,
        Some(remote_peer)
    ));

    let mut rejected_headers = same_origin_headers.clone();
    rejected_headers.insert(
        header::ORIGIN,
        HeaderValue::from_static("https://attacker.example"),
    );
    assert!(!websocket_origin_allowed(
        &state.config,
        &rejected_headers,
        Some(loopback_peer)
    ));

    let mut config = (*state.config).clone();
    config.cors_allowed_origins = vec!["https://attacker.example".to_string()];
    state.config = Arc::new(config);
    assert!(websocket_origin_allowed(
        &state.config,
        &rejected_headers,
        Some(loopback_peer)
    ));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsafe_http_api_mutations_reject_cross_origin_requests() {
    let sandbox = unique_test_dir("http-origin-policy");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let state = test_state(workspace.clone(), vec![workspace], codex_home);

    let rejected = Request::builder()
        .method(Method::POST)
        .uri("/api/auth/login")
        .header(header::HOST, "127.0.0.1:4173")
        .header(header::ORIGIN, "https://attacker.example")
        .body(Body::from("{}"))
        .unwrap();
    let response = handle_http(State(state.clone()), CookieJar::new(), rejected).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let same_origin = Request::builder()
        .method(Method::POST)
        .uri("/api/auth/login")
        .header(header::HOST, "127.0.0.1:4173")
        .header(header::ORIGIN, "http://127.0.0.1:4173")
        .body(Body::from("{}"))
        .unwrap();
    let response = handle_http(State(state.clone()), CookieJar::new(), same_origin).await;
    assert_ne!(response.status(), StatusCode::FORBIDDEN);

    let no_origin_loopback = Request::builder()
        .method(Method::POST)
        .uri("/api/auth/login")
        .header(header::HOST, "127.0.0.1:4173")
        .body(Body::from("{}"))
        .unwrap();
    let response = handle_http(State(state.clone()), CookieJar::new(), no_origin_loopback).await;
    assert_ne!(response.status(), StatusCode::FORBIDDEN);

    let mut strict_state = state.clone();
    let mut strict_config = (*strict_state.config).clone();
    strict_config.require_origin_header = true;
    strict_state.config = Arc::new(strict_config);
    let no_origin_strict = Request::builder()
        .method(Method::POST)
        .uri("/api/auth/login")
        .header(header::HOST, "127.0.0.1:4173")
        .body(Body::from("{}"))
        .unwrap();
    let response = handle_http(State(strict_state), CookieJar::new(), no_origin_strict).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let mut external_state = state.clone();
    let mut external_config = (*external_state.config).clone();
    external_config.public_host = "0.0.0.0".to_string();
    external_state.config = Arc::new(external_config);
    let no_origin_external = Request::builder()
        .method(Method::POST)
        .uri("/api/auth/login")
        .header(header::HOST, "example.com")
        .body(Body::from("{}"))
        .unwrap();
    let response = handle_http(State(external_state), CookieJar::new(), no_origin_external).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn health_readiness_and_metrics_endpoints_report_gateway_state() {
    let sandbox = unique_test_dir("ops-probes");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let mut state = test_state(workspace.clone(), vec![workspace], codex_home);
    let mut config = (*state.config).clone();
    config.instance_token = Some("probe-token".to_string());
    config.password = Some("admin-secret".to_string());
    config.restart_command = Some("true".to_string());
    config.app_server_handoff_enabled = true;
    state.config = Arc::new(config);
    fs::create_dir_all(&state.config.data_dir).unwrap();

    let public_health_request = Request::builder()
        .method(Method::GET)
        .uri("/healthz")
        .body(Body::empty())
        .unwrap();
    let public_health_response = handle_http(
        State(state.clone()),
        CookieJar::new(),
        public_health_request,
    )
    .await;
    assert_eq!(public_health_response.status(), StatusCode::OK);
    let public_health_body = to_bytes(public_health_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let public_health_payload: Value = serde_json::from_slice(&public_health_body).unwrap();
    assert_eq!(
        public_health_payload.get("status").and_then(Value::as_str),
        Some("ok")
    );
    assert!(
        public_health_payload.get("buildCommit").is_none(),
        "public health response must not expose build metadata"
    );

    let health_request = Request::builder()
        .method(Method::GET)
        .uri("/healthz")
        .header("x-codex-webui-instance-token", "probe-token")
        .body(Body::empty())
        .unwrap();
    let health_response = handle_http(State(state.clone()), CookieJar::new(), health_request).await;
    assert_eq!(health_response.status(), StatusCode::OK);
    assert_eq!(
        health_response
            .headers()
            .get(header::HeaderName::from_static("x-frame-options")),
        Some(&HeaderValue::from_static("DENY"))
    );
    let csp = health_response
        .headers()
        .get(header::HeaderName::from_static("content-security-policy"))
        .and_then(|value| value.to_str().ok())
        .expect("CSP header should be present");
    assert!(csp.contains("frame-ancestors 'none'"));
    assert!(csp.contains("script-src 'self'"));
    assert!(!csp.contains("script-src 'self' 'unsafe-inline'"));
    assert!(csp.contains("connect-src 'self' https://hcaptcha.com https://*.hcaptcha.com;"));
    assert!(csp.contains("img-src 'self' data: blob:;"));
    assert!(!csp.contains("ws:"));
    assert!(!csp.contains("wss:"));
    assert!(!csp.contains("img-src 'self' data: blob: http: https:"));
    let health_body = to_bytes(health_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let health_payload: Value = serde_json::from_slice(&health_body).unwrap();
    assert_eq!(
        health_payload.get("status").and_then(Value::as_str),
        Some("ok")
    );
    assert_eq!(
        health_payload
            .get("instanceTokenMatched")
            .and_then(Value::as_bool),
        Some(true)
    );

    let ready_request = Request::builder()
        .method(Method::GET)
        .uri("/readyz")
        .body(Body::empty())
        .unwrap();
    let ready_response = handle_http(State(state.clone()), CookieJar::new(), ready_request).await;
    assert_eq!(ready_response.status(), StatusCode::OK);

    let metrics_request = Request::builder()
        .method(Method::GET)
        .uri("/metrics")
        .body(Body::empty())
        .unwrap();
    let metrics_response =
        handle_http(State(state.clone()), CookieJar::new(), metrics_request).await;
    assert_eq!(metrics_response.status(), StatusCode::UNAUTHORIZED);

    let jar = issue_auth_cookie(&state.config, CookieJar::new(), false, UserRole::Admin).unwrap();
    let metrics_request = Request::builder()
        .method(Method::GET)
        .uri("/metrics")
        .body(Body::empty())
        .unwrap();
    let metrics_response = handle_http(State(state.clone()), jar, metrics_request).await;
    assert_eq!(metrics_response.status(), StatusCode::OK);
    assert_eq!(
        metrics_response
            .headers()
            .get(header::HeaderName::from_static("x-content-type-options")),
        Some(&HeaderValue::from_static("nosniff"))
    );
    assert_eq!(
        metrics_response.headers().get(header::CONTENT_TYPE),
        Some(&HeaderValue::from_static(
            "text/plain; version=0.0.4; charset=utf-8"
        ))
    );
    let metrics_body = to_bytes(metrics_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let metrics_text = String::from_utf8(metrics_body.to_vec()).unwrap();
    assert!(metrics_text.contains("codex_webui_profiles 1"));
    assert!(metrics_text.contains("codex_webui_response_cache_entries 0"));

    let denied_handoff_request = Request::builder()
        .method(Method::POST)
        .uri("/api/admin/restart-handoff/prepare")
        .header("x-codex-webui-instance-token", "wrong-token")
        .body(Body::empty())
        .unwrap();
    let denied_handoff_response = handle_http(
        State(state.clone()),
        CookieJar::new(),
        denied_handoff_request,
    )
    .await;
    assert_eq!(denied_handoff_response.status(), StatusCode::FORBIDDEN);
    assert!(
        !state
            .preserve_app_servers_on_shutdown
            .load(Ordering::SeqCst)
    );

    let owner_jar =
        issue_auth_cookie(&state.config, CookieJar::new(), false, UserRole::Owner).unwrap();
    let owner_handoff_without_csrf = Request::builder()
        .method(Method::POST)
        .uri("/api/admin/restart-handoff/prepare")
        .body(Body::empty())
        .unwrap();
    let owner_handoff_without_csrf_response = handle_http(
        State(state.clone()),
        owner_jar.clone(),
        owner_handoff_without_csrf,
    )
    .await;
    assert_eq!(
        owner_handoff_without_csrf_response.status(),
        StatusCode::FORBIDDEN
    );
    assert!(
        !state
            .preserve_app_servers_on_shutdown
            .load(Ordering::SeqCst)
    );

    let owner_jar = issue_csrf_cookie(&state.config, owner_jar, false).unwrap();
    let csrf_token = owner_jar
        .get(CSRF_COOKIE)
        .expect("csrf cookie should be issued")
        .value()
        .to_string();
    let owner_handoff_request = Request::builder()
        .method(Method::POST)
        .uri("/api/admin/restart-handoff/prepare")
        .header(CSRF_HEADER, csrf_token)
        .body(Body::empty())
        .unwrap();
    let owner_handoff_response =
        handle_http(State(state.clone()), owner_jar, owner_handoff_request).await;
    assert_eq!(owner_handoff_response.status(), StatusCode::OK);
    assert!(
        state
            .preserve_app_servers_on_shutdown
            .load(Ordering::SeqCst)
    );

    state
        .preserve_app_servers_on_shutdown
        .store(false, Ordering::SeqCst);
    let handoff_request = Request::builder()
        .method(Method::POST)
        .uri("/api/admin/restart-handoff/prepare")
        .header("x-codex-webui-instance-token", "probe-token")
        .body(Body::empty())
        .unwrap();
    let handoff_response =
        handle_http(State(state.clone()), CookieJar::new(), handoff_request).await;
    assert_eq!(handoff_response.status(), StatusCode::OK);
    assert!(
        state
            .preserve_app_servers_on_shutdown
            .load(Ordering::SeqCst)
    );

    state
        .preserve_app_servers_on_shutdown
        .store(false, Ordering::SeqCst);
    *state.restart_plan.lock().await = None;
    let restart_jar =
        issue_auth_cookie(&state.config, CookieJar::new(), false, UserRole::Owner).unwrap();
    let restart_jar = issue_csrf_cookie(&state.config, restart_jar, false).unwrap();
    let restart_csrf_token = restart_jar
        .get(CSRF_COOKIE)
        .expect("csrf cookie should be issued")
        .value()
        .to_string();
    let restart_request = Request::builder()
        .method(Method::POST)
        .uri("/api/admin/restart")
        .header(CSRF_HEADER, restart_csrf_token)
        .body(Body::empty())
        .unwrap();
    let restart_response = handle_http(State(state.clone()), restart_jar, restart_request).await;
    assert_eq!(restart_response.status(), StatusCode::OK);
    assert!(
        state
            .preserve_app_servers_on_shutdown
            .load(Ordering::SeqCst)
    );
    assert!(
        state.restart_plan.lock().await.is_none(),
        "replacement gateway should be spawned before shutdown begins instead of waiting for graceful connections to drain"
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_handoff_refuses_to_drop_active_app_server_when_disabled() {
    let sandbox = unique_test_dir("restart-handoff-disabled");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let mut state = test_state(workspace.clone(), vec![workspace], codex_home.clone());
    let mut config = (*state.config).clone();
    config.instance_token = Some("probe-token".to_string());
    config.password = Some("admin-secret".to_string());
    config.restart_command = Some("true".to_string());
    config.app_server_handoff_enabled = false;
    state.config = Arc::new(config);
    state
        .app_servers
        .get_or_create(AppServerProfile {
            id: "default".to_string(),
            codex_home,
        })
        .await;

    let prepare_request = Request::builder()
        .method(Method::POST)
        .uri("/api/admin/restart-handoff/prepare")
        .header("x-codex-webui-instance-token", "probe-token")
        .body(Body::empty())
        .unwrap();
    let prepare_response =
        handle_http(State(state.clone()), CookieJar::new(), prepare_request).await;
    assert_eq!(prepare_response.status(), StatusCode::CONFLICT);
    assert!(
        !state
            .preserve_app_servers_on_shutdown
            .load(Ordering::SeqCst)
    );

    let owner_jar =
        issue_auth_cookie(&state.config, CookieJar::new(), false, UserRole::Owner).unwrap();
    let owner_jar = issue_csrf_cookie(&state.config, owner_jar, false).unwrap();
    let csrf_token = owner_jar
        .get(CSRF_COOKIE)
        .expect("csrf cookie should be issued")
        .value()
        .to_string();
    let restart_request = Request::builder()
        .method(Method::POST)
        .uri("/api/admin/restart")
        .header(CSRF_HEADER, csrf_token)
        .body(Body::empty())
        .unwrap();
    let restart_response = handle_http(State(state.clone()), owner_jar, restart_request).await;
    assert_eq!(restart_response.status(), StatusCode::CONFLICT);
    assert!(
        !state
            .preserve_app_servers_on_shutdown
            .load(Ordering::SeqCst)
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[test]
fn viewer_websocket_permissions_are_session_observation_only() {
    for method in [
        "sessions/list",
        "sessions/search",
        "session/get",
        "session/olderTurns/get",
        "session/turn/get",
        "session/itemDetail/get",
        "notifications/list",
        "session/subscribe",
        "session/unsubscribe",
    ] {
        assert!(
            is_ws_method_allowed(UserRole::Viewer, method),
            "{method} should remain visible to viewers"
        );
    }

    for method in [
        "config/get",
        "account/get",
        "audit/list",
        "catalog/get",
        "directories/browse",
        "editor/file/get",
        "git/repositories/list",
        "git/status",
        "git/file/get",
        "terminal/list",
        "terminal/read",
        "session/draft/get",
        "session/queue/get",
    ] {
        assert!(
            !is_ws_method_allowed(UserRole::Viewer, method),
            "{method} should require admin"
        );
    }
}

#[test]
fn connection_local_websocket_methods_bypass_request_replay() {
    for method in [
        "session/subscribe",
        "session/unsubscribe",
        "terminal/subscribe",
        "terminal/unsubscribe",
        "events/subscribe",
        "events/unsubscribe",
    ] {
        assert!(
            !ws_method_uses_request_replay(method),
            "{method} must install connection-local side effects on every request"
        );
    }

    for method in ["session/get", "sessions/list", "turn/send", "git/status"] {
        assert!(
            ws_method_uses_request_replay(method),
            "{method} should keep normal websocket idempotency replay"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn viewer_http_routes_match_websocket_authorization_policy() {
    let sandbox = unique_test_dir("viewer-http-policy");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let mut state = test_state(workspace.clone(), vec![workspace], codex_home);
    let mut config = (*state.config).clone();
    config.password = Some("admin-secret".to_string());
    state.config = Arc::new(config);
    let jar = issue_auth_cookie(&state.config, CookieJar::new(), false, UserRole::Viewer).unwrap();

    for (method, uri) in [
        (Method::GET, "/api/config"),
        (Method::GET, "/api/editor?filePath=README.md"),
        (Method::GET, "/api/directories"),
        (Method::GET, "/api/catalog"),
        (Method::GET, "/api/git/status?repoPath=/tmp/repo"),
        (Method::GET, "/api/account"),
        (Method::POST, "/api/account/logout"),
    ] {
        let request = Request::builder()
            .method(method.clone())
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        let response = handle_http(State(state.clone()), jar.clone(), request).await;
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "{method} {uri} should require admin access"
        );
    }

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn viewer_notification_list_payloads_are_redacted() {
    let sandbox = unique_test_dir("viewer-notification-redaction");
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
                "items": [{
                    "id": "notice-1",
                    "type": "sessionCompleted",
                    "createdAt": 20,
                    "readAt": Value::Null,
                    "sessionId": "thread-1",
                    "sessionName": "Secret session name",
                    "payload": { "secret": "hidden notification payload" }
                }],
                "webhookFailures": [{
                    "id": "failure-1",
                    "error": "hidden webhook failure"
                }],
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

    let (out_tx, _out_rx) = mpsc::channel(8);
    let subscriptions = Arc::new(Mutex::new(HashMap::new()));
    let auth = AuthContext {
        profile_id: "default".to_string(),
        role: UserRole::Viewer,
    };
    let ws_payload = execute_ws_method(
        &state,
        &out_tx,
        &subscriptions,
        &auth,
        "notifications/list",
        json!({ "limit": 20 }),
    )
    .await
    .expect("viewer notification list should load");
    assert!(
        !ws_payload
            .to_string()
            .contains("hidden notification payload")
    );
    assert!(!ws_payload.to_string().contains("Secret session name"));
    assert!(ws_payload.get("webhookFailures").is_none());

    let jar = issue_auth_cookie(&state.config, CookieJar::new(), false, UserRole::Viewer).unwrap();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/api/notifications?limit=20")
        .body(Body::empty())
        .unwrap();
    let response = handle_http(State(state.clone()), jar, request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let http_payload: Value = serde_json::from_slice(&body).unwrap();
    assert!(
        !http_payload
            .to_string()
            .contains("hidden notification payload")
    );
    assert!(!http_payload.to_string().contains("hidden webhook failure"));
    assert!(!http_payload.to_string().contains("Secret session name"));
    assert!(http_payload.get("webhookFailures").is_none());

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn viewer_session_queue_and_draft_http_payloads_are_redacted() {
    let sandbox = unique_test_dir("viewer-session-http-redaction");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let state = test_state(workspace.clone(), vec![workspace], codex_home);

    with_ui_state_write(&state, "default", |ui_state| {
        ui_state["queuesByThreadId"]["thread-1"] = json!({
            "items": [{
                "id": "queue-1",
                "prompt": "secret queued prompt",
                "skills": [{
                    "id": "secret-skill",
                    "name": "Secret Skill"
                }],
                "attachmentIds": ["secret-attachment"]
            }],
            "resumePending": true,
            "updatedAt": 42
        });
        ui_state["draftsByThreadId"]["thread-1"] = json!({
            "draft": "secret unsent draft",
            "intent": "queue",
            "updatedAt": 43
        });
        Ok(())
    })
    .await
    .unwrap();

    let viewer_jar =
        issue_auth_cookie(&state.config, CookieJar::new(), false, UserRole::Viewer).unwrap();
    for uri in [
        "/api/sessions/thread-1/queue",
        "/api/sessions/thread-1/draft",
    ] {
        let request = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        let response = handle_http(State(state.clone()), viewer_jar.clone(), request).await;
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let payload: Value = serde_json::from_slice(&body).unwrap();
        let payload_text = payload.to_string();
        assert!(
            !payload_text.contains("secret queued prompt"),
            "{uri} leaked queued prompt: {payload_text}"
        );
        assert!(
            !payload_text.contains("secret unsent draft"),
            "{uri} leaked draft: {payload_text}"
        );
        assert!(
            !payload_text.contains("secret-skill"),
            "{uri} leaked selected skill: {payload_text}"
        );
        assert!(
            !payload_text.contains("secret-attachment"),
            "{uri} leaked attachment id: {payload_text}"
        );
    }

    let admin_jar =
        issue_auth_cookie(&state.config, CookieJar::new(), false, UserRole::Admin).unwrap();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/api/sessions/thread-1/queue")
        .body(Body::empty())
        .unwrap();
    let response = handle_http(State(state.clone()), admin_jar.clone(), request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert!(payload.to_string().contains("secret queued prompt"));

    let request = Request::builder()
        .method(Method::GET)
        .uri("/api/sessions/thread-1/draft")
        .body(Body::empty())
        .unwrap();
    let response = handle_http(State(state.clone()), admin_jar, request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert!(payload.to_string().contains("secret unsent draft"));

    let _ = fs::remove_dir_all(sandbox);
}

#[test]
fn auth_token_signing_requires_explicit_session_secret() {
    let sandbox = unique_test_dir("auth-token-secret");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    let mut config = (*state.config).clone();

    config.session_secret = None;
    assert!(
        sign(&config, "payload")
            .unwrap_err()
            .to_string()
            .contains("CODEX_WEBUI_SESSION_SECRET")
    );

    config.session_secret = Some("too-short".to_string());
    assert!(
        sign(&config, "payload")
            .unwrap_err()
            .to_string()
            .contains("at least 32 bytes")
    );

    config.session_secret = Some("test-session-secret-for-cookie-signing".to_string());
    assert!(sign(&config, "payload").is_ok());

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_cookies_are_scoped_to_base_path() {
    let sandbox = unique_test_dir("auth-cookie-path");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let mut state = test_state(workspace.clone(), vec![workspace], codex_home);
    let mut config = (*state.config).clone();
    config.base_path = "/absproxy/4173".to_string();
    config.owner_password = Some("owner-secret".to_string());

    let jar = issue_auth_cookie(&config, CookieJar::new(), false, UserRole::Admin).unwrap();
    assert_eq!(
        jar.get(AUTH_COOKIE).and_then(|cookie| cookie.path()),
        Some("/absproxy/4173")
    );
    let jar = issue_profile_cookie(&config, jar, false, "default").unwrap();
    assert_eq!(
        jar.get(PROFILE_COOKIE).and_then(|cookie| cookie.path()),
        Some("/absproxy/4173")
    );
    let jar = issue_csrf_cookie(&config, jar, false).unwrap();
    assert_eq!(
        jar.get(CSRF_COOKIE).and_then(|cookie| cookie.path()),
        Some("/absproxy/4173")
    );
    assert!(
        clear_csrf_cookie(&config, jar.clone())
            .get(CSRF_COOKIE)
            .is_none()
    );

    state.config = Arc::new(config);
    let login_request = Request::builder()
        .method(Method::POST)
        .uri("/api/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "password": "owner-secret" }).to_string(),
        ))
        .unwrap();
    let login_response = handle_auth_http(
        state.clone(),
        CookieJar::new(),
        Method::POST,
        "/api/auth/login".to_string(),
        HeaderMap::new(),
        login_request,
        None,
    )
    .await;
    assert_eq!(login_response.status(), StatusCode::OK);
    let login_set_cookies = login_response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().unwrap_or_default().to_string())
        .collect::<Vec<_>>();
    assert!(login_set_cookies.iter().any(|cookie| {
        cookie.contains("codex_webui_auth=") && cookie.contains("Path=/absproxy/4173")
    }));
    assert!(
        login_set_cookies
            .iter()
            .any(|cookie| { cookie.contains("codex_webui_auth=") && cookie.contains("Path=/;") })
    );
    assert!(login_set_cookies.iter().any(|cookie| {
        cookie.contains("codex_webui_auth=") && cookie.contains("Path=/absproxy;")
    }));

    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/auth/logout")
        .body(Body::empty())
        .unwrap();
    let response = handle_auth_http(
        state,
        jar,
        Method::POST,
        "/api/auth/logout".to_string(),
        HeaderMap::new(),
        request,
        None,
    )
    .await;
    let set_cookies = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().unwrap_or_default().to_string())
        .collect::<Vec<_>>();
    assert!(
        set_cookies
            .iter()
            .any(|cookie| cookie.contains("Path=/absproxy/4173"))
    );
    assert!(
        set_cookies
            .iter()
            .any(|cookie| cookie.contains("Path=/absproxy;"))
    );
    assert!(
        set_cookies
            .iter()
            .any(|cookie| cookie.contains("codex_webui_auth=") && cookie.contains("Path=/;"))
    );
    assert!(
        set_cookies
            .iter()
            .any(|cookie| cookie.contains("codex_webui_profile=") && cookie.contains("Path=/;"))
    );
    assert!(
        set_cookies
            .iter()
            .any(|cookie| cookie.contains("codex_webui_csrf=") && cookie.contains("Path=/;"))
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authenticated_http_mutations_require_csrf_token() {
    let sandbox = unique_test_dir("http-csrf-token");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    let jar = issue_auth_cookie(&state.config, CookieJar::new(), false, UserRole::Admin).unwrap();

    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/auth/profile")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "profileId": "default" }).to_string()))
        .unwrap();
    let response = handle_http(State(state.clone()), jar.clone(), request).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let csrf_jar = issue_csrf_cookie(&state.config, jar, false).unwrap();
    let csrf_token = csrf_jar
        .get(CSRF_COOKIE)
        .expect("csrf cookie should be issued")
        .value()
        .to_string();
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/auth/profile")
        .header(header::CONTENT_TYPE, "application/json")
        .header(CSRF_HEADER, csrf_token)
        .body(Body::from(json!({ "profileId": "default" }).to_string()))
        .unwrap();
    let response = handle_http(State(state.clone()), csrf_jar, request).await;
    assert_eq!(response.status(), StatusCode::OK);

    let login_request = Request::builder()
        .method(Method::POST)
        .uri("/api/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "password": "" }).to_string()))
        .unwrap();
    let login_response = handle_http(State(state.clone()), CookieJar::new(), login_request).await;
    assert_ne!(login_response.status(), StatusCode::FORBIDDEN);

    let logout_jar =
        issue_auth_cookie(&state.config, CookieJar::new(), false, UserRole::Admin).unwrap();
    let logout_request = Request::builder()
        .method(Method::POST)
        .uri("/api/auth/logout")
        .body(Body::empty())
        .unwrap();
    let logout_response = handle_http(State(state.clone()), logout_jar, logout_request).await;
    assert_eq!(logout_response.status(), StatusCode::OK);

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn json_http_body_reader_rejects_malformed_non_empty_payloads() {
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/test")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{not valid json"))
        .unwrap();
    let error = read_json_body(request, SMALL_JSON_BODY_LIMIT, "test body")
        .await
        .expect_err("malformed JSON should be rejected");
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.message, "test body must be valid JSON.");

    let empty_request = Request::builder()
        .method(Method::POST)
        .uri("/api/test")
        .body(Body::from(" \n\t "))
        .unwrap();
    let empty_payload = read_json_body(empty_request, SMALL_JSON_BODY_LIMIT, "test body")
        .await
        .expect("empty JSON body should preserve compatibility");
    assert_eq!(empty_payload, json!({}));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_login_rejects_oversized_json_body() {
    let sandbox = unique_test_dir("auth-login-body-limit");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/auth/login")
        .header(
            header::CONTENT_LENGTH,
            (SMALL_JSON_BODY_LIMIT + 1).to_string(),
        )
        .body(Body::from(vec![b'a'; SMALL_JSON_BODY_LIMIT + 1]))
        .unwrap();

    let response = handle_auth_http(
        state,
        CookieJar::new(),
        Method::POST,
        "/api/auth/login".to_string(),
        HeaderMap::new(),
        request,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_login_rate_limit_uses_peer_address_when_proxy_headers_are_untrusted() {
    let sandbox = unique_test_dir("auth-login-peer-rate-limit");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let mut state = test_state(workspace.clone(), vec![workspace], codex_home);
    let mut config = (*state.config).clone();
    config.password = Some("admin-secret".to_string());
    state.config = Arc::new(config);
    let peer_addr: SocketAddr = "203.0.113.10:44123".parse().unwrap();
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/auth/login")
        .extension(ConnectInfo(peer_addr))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "password": "wrong" }).to_string()))
        .unwrap();

    let response = handle_http(State(state.clone()), CookieJar::new(), request).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let attempts = state.login_attempts.lock().await;
    assert!(attempts.contains_key("203.0.113.10"));
    assert!(!attempts.contains_key("unknown"));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_rate_limit_identifier_store_is_bounded() {
    let sandbox = unique_test_dir("auth-login-rate-limit-cap");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    let now = now_millis();

    {
        let mut attempts = state.login_attempts.lock().await;
        for index in 0..LOGIN_RATE_LIMIT_MAX_IDENTIFIERS {
            attempts.insert(
                format!("198.51.100.{index}"),
                vec![now.saturating_sub((LOGIN_RATE_LIMIT_MAX_IDENTIFIERS - index) as u128)],
            );
        }
    }

    record_login_failure(&state, "203.0.113.42").await;

    let attempts = state.login_attempts.lock().await;
    assert!(attempts.len() <= LOGIN_RATE_LIMIT_MAX_IDENTIFIERS);
    assert!(attempts.contains_key("203.0.113.42"));
    assert!(!attempts.contains_key("198.51.100.0"));

    let _ = fs::remove_dir_all(sandbox);
}

#[test]
fn maps_session_item_notifications_for_stream_clients() {
    let mapped = map_app_server_session_notification(&AppServerNotification {
        method: "item/started".to_string(),
        params: json!({
            "threadId": "thread-1",
            "turnId": "turn-1",
            "itemId": "item-1",
            "item": {
                "id": "item-1",
                "type": "commandExecution",
                "command": ["sed", "-n", "1,20p", "src/main.rs"],
                "cwd": "/tmp/project"
            }
        }),
    })
    .expect("notification should map");

    assert_eq!(
        mapped,
        json!({
            "kind": "notification",
            "method": "item/started",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "item-1",
                "item": {
                    "id": "item-1",
                    "type": "commandExecution",
                    "command": ["sed", "-n", "1,20p", "src/main.rs"],
                    "cwd": "/tmp/project",
                    "title": "Command",
                    "detailState": "deferred",
                    "detailPreview": "sed -n 1,20p src/main.rs"
                }
            }
        })
    );
}

#[test]
fn maps_session_item_notifications_use_item_id_when_payload_omits_item_id() {
    let mapped = map_app_server_session_notification(&AppServerNotification {
        method: "item/started".to_string(),
        params: json!({
            "threadId": "thread-1",
            "turnId": "turn-1",
            "itemId": "item-42",
            "item": {
                "type": "commandExecution",
                "command": ["rg", "-n", "queue", "src/routes/+page.svelte"],
                "cwd": "/tmp/project"
            }
        }),
    })
    .expect("notification should map");

    assert_eq!(
        mapped,
        json!({
            "kind": "notification",
            "method": "item/started",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "item-42",
                "item": {
                    "id": "item-42",
                    "type": "commandExecution",
                    "command": ["rg", "-n", "queue", "src/routes/+page.svelte"],
                    "cwd": "/tmp/project",
                    "title": "Command",
                    "detailState": "deferred",
                    "detailPreview": "rg -n queue src/routes/+page.svelte"
                }
            }
        })
    );
}

#[test]
fn maps_thread_name_updates_from_snake_case_payloads() {
    let mapped = map_app_server_session_notification(&AppServerNotification {
        method: "thread/name/updated".to_string(),
        params: json!({
            "thread_id": "thread-1",
            "thread_name": "Generated session title"
        }),
    })
    .expect("notification should map");

    assert_eq!(
        mapped,
        json!({
            "kind": "notification",
            "method": "thread/name/updated",
            "params": {
                "threadId": "thread-1",
                "thread_id": "thread-1",
                "thread_name": "Generated session title",
                "threadName": "Generated session title"
            }
        })
    );
}

#[tokio::test]
async fn lists_only_directories_within_allowed_root() {
    let sandbox = unique_test_dir("directories");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(workspace.join("alpha")).unwrap();
    fs::create_dir_all(workspace.join("beta")).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    fs::write(workspace.join("notes.txt"), "ignore").unwrap();

    let state = test_state(workspace.clone(), vec![workspace.clone()], codex_home);
    let payload: DirectoryPayload = serde_json::from_value(
        list_directories_payload(&state, Some(workspace.to_str().unwrap()))
            .await
            .expect("directory payload should load"),
    )
    .expect("payload should deserialize");

    assert_eq!(payload.allowed_roots.len(), 1);
    assert_eq!(payload.current_path, Some(workspace.display().to_string()));
    assert_eq!(payload.parent_path, None);
    assert_eq!(
        payload
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha", "beta"]
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_worktree_payloads_use_rust_git_helpers() {
    let sandbox = unique_test_dir("git-worktrees");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    let repo = workspace.join("repo");
    let worktree = workspace.join(".codex-webui-worktrees").join("feature");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    init_test_git_repo(&repo);

    let state = test_state(workspace.clone(), vec![workspace.clone()], codex_home);

    let created = create_git_worktree_payload(
        &state,
        repo.to_str().unwrap(),
        worktree.to_str().unwrap(),
        Some("feature/test"),
        true,
        false,
    )
    .await
    .unwrap();
    assert_eq!(
        created.get("repoPath").and_then(Value::as_str),
        Some(repo.to_str().unwrap())
    );
    assert!(
        created
            .get("worktrees")
            .and_then(Value::as_array)
            .is_some_and(|worktrees| {
                worktrees.iter().any(|entry| {
                    entry.get("path").and_then(Value::as_str) == Some(worktree.to_str().unwrap())
                        && entry.get("branch").and_then(Value::as_str) == Some("feature/test")
                })
            })
    );

    let listed = list_git_worktrees_payload(&state, repo.to_str().unwrap())
        .await
        .unwrap();
    assert!(
        listed
            .get("worktrees")
            .and_then(Value::as_array)
            .is_some_and(|worktrees| worktrees.len() >= 2)
    );
    let repositories = list_git_repositories_payload(&state, false).await.unwrap();
    assert!(
        repositories
            .get("repositories")
            .and_then(Value::as_array)
            .is_some_and(|repositories| repositories.iter().any(|entry| {
                entry.get("path").and_then(Value::as_str) == Some(worktree.to_str().unwrap())
            }))
    );

    let removed = remove_git_worktree_payload(
        &state,
        repo.to_str().unwrap(),
        worktree.to_str().unwrap(),
        false,
    )
    .await
    .unwrap();
    assert!(
        removed
            .get("worktrees")
            .and_then(Value::as_array)
            .is_some_and(|worktrees| {
                !worktrees.iter().any(|entry| {
                    entry.get("path").and_then(Value::as_str) == Some(worktree.to_str().unwrap())
                })
            })
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_worktree_remove_rejects_dirty_force_remove() {
    let sandbox = unique_test_dir("git-worktree-dirty-remove");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    let repo = workspace.join("repo");
    let worktree = workspace.join(".codex-webui-worktrees").join("dirty");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    init_test_git_repo(&repo);

    let state = test_state(workspace.clone(), vec![workspace.clone()], codex_home);
    create_git_worktree_payload(
        &state,
        repo.to_str().unwrap(),
        worktree.to_str().unwrap(),
        Some("feature/dirty"),
        true,
        false,
    )
    .await
    .unwrap();
    fs::write(worktree.join("dirty.txt"), "dirty\n").unwrap();

    let error = remove_git_worktree_payload(
        &state,
        repo.to_str().unwrap(),
        worktree.to_str().unwrap(),
        true,
    )
    .await
    .expect_err("dirty worktree force-removal should be rejected");
    assert_eq!(error.status, StatusCode::CONFLICT);
    assert_eq!(
        error.message,
        "Refusing to force-remove a worktree with uncommitted changes."
    );
    assert!(worktree.exists());

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audit_log_list_clamps_limit_and_reads_latest_entries() {
    let sandbox = unique_test_dir("audit-log-tail");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let state = test_state(workspace.clone(), vec![workspace], codex_home);

    for index in 0..(MAX_AUDIT_LOG_LIMIT + 20) {
        append_audit_log(
            &state.config,
            AuditLogEntry {
                id: format!("entry-{index}"),
                at: index as u64,
                role: "admin".to_string(),
                method: "test/method".to_string(),
                target: None,
                ok: true,
                error: None,
            },
        )
        .await
        .unwrap();
    }

    let payload = list_audit_log(&state.config, usize::MAX).await.unwrap();
    let entries = payload
        .get("entries")
        .and_then(Value::as_array)
        .expect("audit entries should be present");
    assert_eq!(entries.len(), MAX_AUDIT_LOG_LIMIT);
    assert_eq!(
        entries
            .first()
            .and_then(|entry| entry.get("id"))
            .and_then(Value::as_str),
        Some("entry-519")
    );
    assert_eq!(
        entries
            .last()
            .and_then(|entry| entry.get("id"))
            .and_then(Value::as_str),
        Some("entry-20")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_read_payloads_use_rust_helpers() {
    let sandbox = unique_test_dir("git-read");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    let repo = workspace.join("repo");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    init_test_git_repo(&repo);
    fs::write(repo.join("README.md"), "changed\n").unwrap();
    fs::write(repo.join("notes.txt"), "todo\n").unwrap();

    let state = test_state(workspace.clone(), vec![workspace.clone()], codex_home);

    let repositories = list_git_repositories_payload(&state, false).await.unwrap();
    assert!(
        repositories
            .get("repositories")
            .and_then(Value::as_array)
            .is_some_and(|repositories| repositories.iter().any(|entry| {
                entry.get("path").and_then(Value::as_str) == Some(repo.to_str().unwrap())
            }))
    );

    let status = get_git_status_payload(&state, repo.to_str().unwrap())
        .await
        .unwrap();
    assert_eq!(
        status
            .get("repo")
            .and_then(|value| value.get("path"))
            .and_then(Value::as_str),
        Some(repo.to_str().unwrap())
    );
    assert_eq!(status.get("clean").and_then(Value::as_bool), Some(false));
    assert!(
        status
            .get("files")
            .and_then(Value::as_array)
            .is_some_and(|files| files.iter().any(|entry| {
                entry.get("path").and_then(Value::as_str) == Some("README.md")
                    && entry.get("unstagedLabel").and_then(Value::as_str) == Some("modified")
            }))
    );
    assert!(
        status
            .get("files")
            .and_then(Value::as_array)
            .is_some_and(|files| files.iter().any(|entry| {
                entry.get("path").and_then(Value::as_str) == Some("notes.txt")
                    && entry.get("isUntracked").and_then(Value::as_bool) == Some(true)
            }))
    );

    let file_payload = get_git_file_payload(&state, repo.to_str().unwrap(), "README.md")
        .await
        .unwrap();
    assert_eq!(
        file_payload.get("originalContent").and_then(Value::as_str),
        Some("init\n")
    );
    assert_eq!(
        file_payload.get("modifiedContent").and_then(Value::as_str),
        Some("changed\n")
    );

    let diff_payload = get_git_commit_diff_payload(&state, repo.to_str().unwrap(), "HEAD")
        .await
        .unwrap();
    assert!(
        diff_payload
            .get("diff")
            .and_then(Value::as_str)
            .is_some_and(|diff| diff.contains("README.md"))
    );

    let resolved = resolve_git_file_from_absolute_path_payload(
        &state,
        repo.join("README.md").to_str().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(
        resolved.get("repoPath").and_then(Value::as_str),
        Some(repo.to_str().unwrap())
    );
    assert_eq!(
        resolved.get("filePath").and_then(Value::as_str),
        Some("README.md")
    );

    fs::write(repo.join(".env"), "TOKEN=secret\n").unwrap();
    let error = get_git_file_payload(&state, repo.to_str().unwrap(), ".env")
        .await
        .expect_err("sensitive git files must be blocked");
    assert_eq!(error.status, StatusCode::FORBIDDEN);

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_commit_diff_rejects_too_many_changed_files() {
    let sandbox = unique_test_dir("git-diff-file-limit");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    let repo = workspace.join("repo");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    init_test_git_repo(&repo);
    let bulk_dir = repo.join("bulk");
    fs::create_dir_all(&bulk_dir).unwrap();
    for index in 0..=GIT_DIFF_PREVIEW_MAX_FILES {
        fs::write(
            bulk_dir.join(format!("file-{index}.txt")),
            format!("{index}\n"),
        )
        .unwrap();
    }
    let add = std::process::Command::new("git")
        .args(["-C", repo.to_str().unwrap(), "add", "bulk"])
        .output()
        .unwrap();
    assert!(add.status.success(), "git add bulk failed");
    let commit = std::process::Command::new("git")
        .args(["-C", repo.to_str().unwrap(), "commit", "-m", "bulk update"])
        .output()
        .unwrap();
    assert!(commit.status.success(), "git commit bulk failed");

    let state = test_state(workspace.clone(), vec![workspace.clone()], codex_home);
    let error = get_git_commit_diff_payload(&state, repo.to_str().unwrap(), "HEAD")
        .await
        .expect_err("large changed-file count should be rejected");

    assert_eq!(error.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(error.message.contains("file preview limit"));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_write_payloads_use_rust_helpers() {
    let sandbox = unique_test_dir("git-write");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    let repo = workspace.join("repo");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    init_test_git_repo(&repo);

    let state = test_state(workspace.clone(), vec![workspace.clone()], codex_home);

    let saved = save_git_file_payload(&state, repo.to_str().unwrap(), "src/new.txt", "hello\n")
        .await
        .unwrap();
    assert_eq!(
        saved.get("modifiedContent").and_then(Value::as_str),
        Some("hello\n")
    );

    let staged = stage_git_changes_payload(&state, repo.to_str().unwrap(), Some("src/new.txt"))
        .await
        .unwrap();
    assert!(
        staged
            .get("files")
            .and_then(Value::as_array)
            .is_some_and(|files| files.iter().any(|entry| {
                entry.get("path").and_then(Value::as_str) == Some("src/new.txt")
                    && entry.get("hasStagedChanges").and_then(Value::as_bool) == Some(true)
            }))
    );

    let unstaged = unstage_git_changes_payload(&state, repo.to_str().unwrap(), Some("src/new.txt"))
        .await
        .unwrap();
    assert!(
        unstaged
            .get("files")
            .and_then(Value::as_array)
            .is_some_and(|files| files
                .iter()
                .any(|entry| { entry.get("isUntracked").and_then(Value::as_bool) == Some(true) }))
    );

    stage_git_changes_payload(&state, repo.to_str().unwrap(), None)
        .await
        .unwrap();
    let committed = commit_git_changes_payload(&state, repo.to_str().unwrap(), "add new file")
        .await
        .unwrap();
    assert_eq!(committed.get("clean").and_then(Value::as_bool), Some(true));
    assert_eq!(
        committed
            .get("commits")
            .and_then(Value::as_array)
            .and_then(|commits| commits.first())
            .and_then(|commit| commit.get("subject"))
            .and_then(Value::as_str),
        Some("add new file")
    );

    let switched =
        checkout_git_branch_payload(&state, repo.to_str().unwrap(), "feature/test", true)
            .await
            .unwrap();
    assert_eq!(
        switched.get("branch").and_then(Value::as_str),
        Some("feature/test")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_repo_root_must_remain_inside_allowed_roots() {
    let sandbox = unique_test_dir("git-root-escape");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    let repo = workspace.join("repo");
    let allowed_subdir = repo.join("src");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    init_test_git_repo(&repo);
    fs::create_dir_all(&allowed_subdir).unwrap();

    let state = test_state(repo.clone(), vec![allowed_subdir.clone()], codex_home);
    let error = resolve_git_repo_root(&state, allowed_subdir.to_str().unwrap())
        .await
        .expect_err("repo root outside allowed root should be rejected");

    assert_eq!(error.status, StatusCode::FORBIDDEN);
    assert_eq!(
        error.message,
        "The selected Git repository root is outside the allowed roots."
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_operation_locks_prune_idle_entries() {
    let sandbox = unique_test_dir("git-operation-lock-prune");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let state = test_state(workspace.clone(), vec![workspace], codex_home);

    for index in 0..(GIT_OPERATION_LOCK_MAX_ENTRIES + 10) {
        let _ = git_operation_lock(&state, &format!("/tmp/repo-{index}")).await;
    }

    let locks = state.git_operation_locks.lock().await;
    assert!(locks.len() <= 1);

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catalog_cache_prunes_old_entries_at_cap() {
    let sandbox = unique_test_dir("catalog-cache-cap");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let mut state = test_state(workspace.clone(), vec![workspace], codex_home);
    let mut config = (*state.config).clone();
    for index in 0..(CATALOG_CACHE_MAX_ENTRIES + 10) {
        let profile_codex_home = sandbox.join(format!("codex-home-{index}"));
        fs::create_dir_all(&profile_codex_home).unwrap();
        config.profiles.insert(
            format!("profile-{index}"),
            RuntimeProfile {
                label: format!("Profile {index}"),
                codex_home: profile_codex_home,
                data_dir: sandbox.join(format!("data-{index}")),
            },
        );
    }
    state.config = Arc::new(config);

    for index in 0..(CATALOG_CACHE_MAX_ENTRIES + 10) {
        get_catalog_payload(&state, &format!("profile-{index}"))
            .await
            .unwrap();
    }

    assert!(state.catalog_cache.lock().await.len() <= CATALOG_CACHE_MAX_ENTRIES);

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pending_server_requests_are_capped_per_session() {
    let sandbox = unique_test_dir("pending-server-request-cap");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    let runtime_key = runtime_session_key("default", "thread-1");

    {
        let mut pending = state.pending_server_requests.lock().await;
        let entries = pending.entry(runtime_key.clone()).or_default();
        for index in 0..PENDING_SERVER_REQUEST_MAX_PER_SESSION {
            entries.insert(
                format!("request-{index}"),
                PendingServerRequestEntry {
                    raw_id: json!(format!("request-{index}")),
                    method: "item/commandExecution/requestApproval".to_string(),
                    params: json!({ "threadId": "thread-1" }),
                    created_at: index.to_string(),
                    created_at_ms: index as u64,
                },
            );
        }
    }

    state
        .active_turns
        .lock()
        .await
        .insert(runtime_key.clone(), "turn-1".to_string());
    handle_profile_server_request(
        &state,
        "default",
        "default",
        &backend::codex_app_server::AppServerRequest {
            id: json!("request-new"),
            method: "item/commandExecution/requestApproval".to_string(),
            params: json!({ "threadId": "thread-1" }),
        },
    )
    .await;

    let pending = state.pending_server_requests.lock().await;
    let entries = pending
        .get(&runtime_key)
        .expect("pending request bucket should exist");
    assert!(entries.len() <= PENDING_SERVER_REQUEST_MAX_PER_SESSION);
    assert!(entries.contains_key("request-new"));
    assert!(!entries.contains_key("request-0"));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_stage_and_unstage_treat_file_path_as_literal_pathspec() {
    let sandbox = unique_test_dir("git-literal-pathspec");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    let repo = workspace.join("repo");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    init_test_git_repo(&repo);
    fs::write(repo.join(":(glob)**"), "literal\n").unwrap();
    fs::write(repo.join("other.txt"), "other\n").unwrap();

    let state = test_state(workspace.clone(), vec![workspace.clone()], codex_home);
    let staged = stage_git_changes_payload(&state, repo.to_str().unwrap(), Some(":(glob)**"))
        .await
        .unwrap();
    let files = staged
        .get("files")
        .and_then(Value::as_array)
        .expect("git status files should be returned");
    assert!(files.iter().any(|entry| {
        entry.get("path").and_then(Value::as_str) == Some(":(glob)**")
            && entry.get("hasStagedChanges").and_then(Value::as_bool) == Some(true)
    }));
    assert!(files.iter().any(|entry| {
        entry.get("path").and_then(Value::as_str) == Some("other.txt")
            && entry.get("hasStagedChanges").and_then(Value::as_bool) != Some(true)
    }));

    let unstaged = unstage_git_changes_payload(&state, repo.to_str().unwrap(), Some(":(glob)**"))
        .await
        .unwrap();
    let files = unstaged
        .get("files")
        .and_then(Value::as_array)
        .expect("git status files should be returned");
    assert!(files.iter().any(|entry| {
        entry.get("path").and_then(Value::as_str) == Some(":(glob)**")
            && entry.get("isUntracked").and_then(Value::as_bool) == Some(true)
    }));
    assert!(files.iter().any(|entry| {
        entry.get("path").and_then(Value::as_str) == Some("other.txt")
            && entry.get("hasStagedChanges").and_then(Value::as_bool) != Some(true)
    }));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_mutations_share_repo_operation_lock() {
    let sandbox = unique_test_dir("git-write-lock");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    let repo = workspace.join("repo");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    init_test_git_repo(&repo);
    fs::write(repo.join("queued.txt"), "queued\n").unwrap();

    let state = test_state(workspace.clone(), vec![workspace.clone()], codex_home);
    let repo_root = resolve_git_repo_root(&state, repo.to_str().unwrap())
        .await
        .unwrap();
    let repo_lock = git_operation_lock(&state, &repo_root).await;
    let guard = repo_lock.lock().await;
    let state_for_task = state.clone();
    let repo_for_task = repo_root.clone();
    let handle = tokio::spawn(async move {
        stage_git_changes_payload(&state_for_task, &repo_for_task, Some("queued.txt")).await
    });

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!handle.is_finished());
    drop(guard);

    let staged = handle.await.unwrap().unwrap();
    assert!(
        staged
            .get("files")
            .and_then(Value::as_array)
            .is_some_and(|files| files.iter().any(|entry| {
                entry.get("path").and_then(Value::as_str) == Some("queued.txt")
                    && entry.get("hasStagedChanges").and_then(Value::as_bool) == Some(true)
            }))
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn destructive_git_mutations_reject_active_codex_work() {
    let sandbox = unique_test_dir("git-busy-active-turn");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    let repo = workspace.join("repo");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    init_test_git_repo(&repo);

    let state = test_state(workspace.clone(), vec![workspace.clone()], codex_home);
    with_ui_state_write(&state, "default", |ui_state| {
        let Some(preferences_by_thread_id) = ui_state
            .get_mut("preferencesByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "preferences state is missing",
            ));
        };
        preferences_by_thread_id.insert(
            "thread-1".to_string(),
            json!({
                "cwd": repo.display().to_string(),
                "gitRepoPath": repo.display().to_string()
            }),
        );
        Ok(())
    })
    .await
    .unwrap();
    state.active_turns.lock().await.insert(
        runtime_session_key("default", "thread-1"),
        "turn-1".to_string(),
    );

    let error = checkout_git_branch_payload(&state, repo.to_str().unwrap(), "busy-test", true)
        .await
        .expect_err("branch switching should reject active Codex work");

    assert_eq!(error.status, StatusCode::CONFLICT);
    assert_eq!(
        error.message,
        "Refusing to mutate this repository while a Codex turn is active."
    );

    let error = save_git_file_payload(&state, repo.to_str().unwrap(), "busy.txt", "busy\n")
        .await
        .expect_err("file save should reject active Codex work");
    assert_eq!(error.status, StatusCode::CONFLICT);

    let error = stage_git_changes_payload(&state, repo.to_str().unwrap(), Some("busy.txt"))
        .await
        .expect_err("stage should reject active Codex work");
    assert_eq!(error.status, StatusCode::CONFLICT);

    let error = unstage_git_changes_payload(&state, repo.to_str().unwrap(), Some("busy.txt"))
        .await
        .expect_err("unstage should reject active Codex work");
    assert_eq!(error.status, StatusCode::CONFLICT);

    let error = commit_git_changes_payload(&state, repo.to_str().unwrap(), "busy commit")
        .await
        .expect_err("commit should reject active Codex work");
    assert_eq!(error.status, StatusCode::CONFLICT);

    let error = fetch_git_repository_payload(&state, repo.to_str().unwrap())
        .await
        .expect_err("fetch should reject active Codex work");
    assert_eq!(error.status, StatusCode::CONFLICT);

    let worktree = workspace.join("busy-worktree");
    let error = create_git_worktree_payload(
        &state,
        repo.to_str().unwrap(),
        worktree.to_str().unwrap(),
        Some("busy-worktree"),
        true,
        false,
    )
    .await
    .expect_err("worktree create should reject active Codex work");
    assert_eq!(error.status, StatusCode::CONFLICT);

    let error = checkout_github_pull_request_payload(&state, repo.to_str().unwrap(), 1)
        .await
        .expect_err("GitHub PR checkout should reject active Codex work before gh runs");
    assert_eq!(error.status, StatusCode::CONFLICT);

    let _ = fs::remove_dir_all(sandbox);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_file_save_rejects_symlinked_parent_escape() {
    let sandbox = unique_test_dir("git-write-symlink-parent");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    let repo = workspace.join("repo");
    let outside = sandbox.join("outside");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    fs::create_dir_all(&outside).unwrap();
    init_test_git_repo(&repo);
    std::os::unix::fs::symlink(&outside, repo.join("link-out")).unwrap();

    let state = test_state(workspace.clone(), vec![workspace.clone()], codex_home);
    let error = save_git_file_payload(
        &state,
        repo.to_str().unwrap(),
        "link-out/secret.txt",
        "secret\n",
    )
    .await
    .expect_err("git file save must not write through symlinked parents");

    assert_eq!(error.status, StatusCode::FORBIDDEN);
    assert_eq!(
        error.message,
        "Refusing to write through a symlinked parent directory."
    );
    assert!(!outside.join("secret.txt").exists());

    let _ = fs::remove_dir_all(sandbox);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_worktree_create_rejects_symlinked_parent_escape() {
    let sandbox = unique_test_dir("git-worktree-symlink-parent");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    let repo = workspace.join("repo");
    let outside = sandbox.join("outside");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    fs::create_dir_all(&outside).unwrap();
    init_test_git_repo(&repo);
    std::os::unix::fs::symlink(&outside, workspace.join("link-out")).unwrap();

    let state = test_state(workspace.clone(), vec![workspace.clone()], codex_home);
    let target = workspace.join("link-out").join("new-worktree");
    let error = create_git_worktree_payload(
        &state,
        repo.to_str().unwrap(),
        target.to_str().unwrap(),
        Some("escape-worktree"),
        true,
        false,
    )
    .await
    .expect_err("worktree create must not follow a symlinked parent");

    assert_eq!(error.status, StatusCode::FORBIDDEN);
    assert_eq!(
        error.message,
        "Refusing to create a worktree through a symlinked parent directory."
    );
    assert!(!outside.join("new-worktree").exists());

    let _ = fs::remove_dir_all(sandbox);
}

#[test]
fn github_pull_request_file_pagination_reports_truncation() {
    assert!(!github_pull_request_files_truncated(2, 2, false));
    assert!(github_pull_request_files_truncated(101, 100, false));
    assert!(github_pull_request_files_truncated(100, 100, true));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_http_handlers_use_rust_routes() {
    let sandbox = unique_test_dir("git-http");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    let repo = workspace.join("repo");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    init_test_git_repo(&repo);

    let state = test_state(workspace.clone(), vec![workspace.clone()], codex_home);
    let auth = AuthContext {
        role: UserRole::Admin,
        profile_id: "default".to_string(),
    };

    let repositories_request = Request::builder()
        .method(Method::GET)
        .uri("/api/git/repositories")
        .body(Body::empty())
        .unwrap();
    let repositories_response = handle_git_api_http(
        state.clone(),
        repositories_request,
        auth.clone(),
        "/api/git/repositories",
    )
    .await;
    assert_eq!(repositories_response.status(), StatusCode::OK);
    let repositories_body = to_bytes(repositories_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let repositories_payload: Value = serde_json::from_slice(&repositories_body).unwrap();
    assert_eq!(
        repositories_payload
            .get("repositories")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );

    fs::create_dir_all(repo.join("src")).unwrap();
    fs::write(repo.join("src").join("queue.rs"), "pub fn run() {}\n").unwrap();
    let stage_request = Request::builder()
        .method(Method::POST)
        .uri("/api/git/stage")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "repoPath": repo.display().to_string(),
                "filePath": "src/queue.rs"
            })
            .to_string(),
        ))
        .unwrap();
    let stage_response =
        handle_git_api_http(state.clone(), stage_request, auth, "/api/git/stage").await;
    assert_eq!(stage_response.status(), StatusCode::OK);
    let stage_body = to_bytes(stage_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let stage_payload: Value = serde_json::from_slice(&stage_body).unwrap();
    assert_eq!(
        stage_payload
            .get("files")
            .and_then(Value::as_array)
            .map(|files| files.iter().any(|entry| {
                entry.get("path").and_then(Value::as_str) == Some("src/queue.rs")
                    && entry
                        .get("hasStagedChanges")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
            })),
        Some(true)
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn static_asset_handler_rewrites_base_path_and_uses_spa_fallbacks() {
    let sandbox = unique_test_dir("static-assets");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    let static_dir = workspace.join("static");
    fs::create_dir_all(static_dir.join("_app").join("immutable")).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    fs::write(
        static_dir.join("index.html"),
        "<html><body>/__CODEX_WEBUI_BASE__/index<script>window.__boot = true;</script></body></html>",
    )
    .unwrap();
    fs::write(
        static_dir.join("200.html"),
        "<html><body>/__CODEX_WEBUI_BASE__/fallback<script>window.__fallback = true;</script></body></html>",
    )
    .unwrap();
    fs::write(
        static_dir.join("_app").join("immutable").join("app.js"),
        "window.__BASE__ = '/__CODEX_WEBUI_BASE__';",
    )
    .unwrap();

    let state = test_state_with_static_dir_and_base_path(
        workspace.clone(),
        vec![workspace.clone()],
        codex_home,
        static_dir,
        "/absproxy/4173",
    );

    let root_response = serve_static_asset(state.clone(), "/").await;
    assert_eq!(root_response.status(), StatusCode::OK);
    assert_eq!(
        root_response.headers().get(header::CACHE_CONTROL),
        Some(&HeaderValue::from_static(
            "no-store, max-age=0, must-revalidate"
        ))
    );
    let content_security_policy = root_response
        .headers()
        .get(header::HeaderName::from_static("content-security-policy"))
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    assert!(content_security_policy.contains("object-src 'none'"));
    assert!(content_security_policy.contains("base-uri 'none'"));
    assert!(content_security_policy.contains("frame-ancestors 'none'"));
    assert!(content_security_policy.contains("'sha256-"));
    assert!(!content_security_policy.contains("script-src 'self' 'unsafe-inline'"));
    assert!(content_security_policy.contains("img-src 'self' data: blob:;"));
    assert!(
        content_security_policy
            .contains("connect-src 'self' https://hcaptcha.com https://*.hcaptcha.com;")
    );
    assert!(!content_security_policy.contains("ws:"));
    assert!(!content_security_policy.contains("wss:"));
    assert!(!content_security_policy.contains("img-src 'self' data: blob: http: https:"));
    assert_eq!(
        root_response
            .headers()
            .get(header::HeaderName::from_static("x-content-type-options")),
        Some(&HeaderValue::from_static("nosniff"))
    );
    assert_eq!(
        root_response
            .headers()
            .get(header::HeaderName::from_static("referrer-policy")),
        Some(&HeaderValue::from_static("same-origin"))
    );
    let root_body = to_bytes(root_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let root_text = String::from_utf8(root_body.to_vec()).unwrap();
    assert!(root_text.contains("/absproxy/4173/index"));
    assert!(!root_text.contains(STATIC_BASE_PLACEHOLDER));

    let session_response = serve_static_asset(state.clone(), "/sessions/thread-1").await;
    assert_eq!(session_response.status(), StatusCode::OK);
    assert_eq!(
        session_response.headers().get(header::CACHE_CONTROL),
        Some(&HeaderValue::from_static(
            "no-store, max-age=0, must-revalidate"
        ))
    );
    let session_body = to_bytes(session_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let session_text = String::from_utf8(session_body.to_vec()).unwrap();
    assert!(session_text.contains("/absproxy/4173/fallback"));
    assert!(!session_text.contains(STATIC_BASE_PLACEHOLDER));

    let asset_response = serve_static_asset(state, "/_app/immutable/app.js").await;
    assert_eq!(asset_response.status(), StatusCode::OK);
    assert_eq!(
        asset_response.headers().get(header::CACHE_CONTROL),
        Some(&HeaderValue::from_static(
            "public, max-age=31536000, immutable"
        ))
    );
    let asset_body = to_bytes(asset_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let asset_text = String::from_utf8(asset_body.to_vec()).unwrap();
    assert!(asset_text.contains("/absproxy/4173"));
    assert!(!asset_text.contains(STATIC_BASE_PLACEHOLDER));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn static_asset_handler_never_memocaches_mutable_versioned_shell_files() {
    let sandbox = unique_test_dir("static-assets-version-cache");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    let static_dir = workspace.join("static");
    fs::create_dir_all(static_dir.join("_app")).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    fs::write(static_dir.join("index.html"), "<html>v1</html>").unwrap();
    fs::write(
        static_dir.join("service-worker.js"),
        "const VERSION = 'v1';",
    )
    .unwrap();
    fs::write(static_dir.join("manifest.webmanifest"), "{\"name\":\"v1\"}").unwrap();
    fs::write(
        static_dir.join("_app").join("version.json"),
        "{\"version\":\"v1\"}",
    )
    .unwrap();

    let state = test_state_with_static_dir_and_base_path(
        workspace.clone(),
        vec![workspace.clone()],
        codex_home,
        static_dir.clone(),
        "",
    );

    let first_version_response = serve_static_asset(state.clone(), "/_app/version.json").await;
    assert_eq!(first_version_response.status(), StatusCode::OK);
    assert_eq!(
        first_version_response.headers().get(header::CACHE_CONTROL),
        Some(&HeaderValue::from_static(
            "no-store, max-age=0, must-revalidate"
        ))
    );
    let first_version_body = to_bytes(first_version_response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(
        String::from_utf8(first_version_body.to_vec())
            .unwrap()
            .contains("\"v1\"")
    );

    fs::write(
        static_dir.join("_app").join("version.json"),
        "{\"version\":\"v2\"}",
    )
    .unwrap();
    let second_version_response = serve_static_asset(state.clone(), "/_app/version.json").await;
    let second_version_body = to_bytes(second_version_response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(
        String::from_utf8(second_version_body.to_vec())
            .unwrap()
            .contains("\"v2\"")
    );

    let service_worker_response = serve_static_asset(state.clone(), "/service-worker.js").await;
    assert_eq!(
        service_worker_response.headers().get(header::CACHE_CONTROL),
        Some(&HeaderValue::from_static(
            "no-store, max-age=0, must-revalidate"
        ))
    );

    let manifest_response = serve_static_asset(state, "/manifest.webmanifest").await;
    assert_eq!(
        manifest_response.headers().get(header::CACHE_CONTROL),
        Some(&HeaderValue::from_static(
            "no-cache, max-age=0, must-revalidate"
        ))
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn static_asset_handler_rejects_invalid_and_missing_paths() {
    let sandbox = unique_test_dir("static-assets-404");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    let static_dir = workspace.join("static");
    fs::create_dir_all(&static_dir).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    fs::write(static_dir.join("index.html"), "<html>ok</html>").unwrap();
    fs::write(static_dir.join("200.html"), "<html>fallback</html>").unwrap();

    let state = test_state_with_static_dir_and_base_path(
        workspace.clone(),
        vec![workspace.clone()],
        codex_home,
        static_dir,
        "",
    );

    let invalid_response = serve_static_asset(state.clone(), "/../../secret.txt").await;
    assert_eq!(invalid_response.status(), StatusCode::NOT_FOUND);

    let missing_asset_response = serve_static_asset(state, "/missing.css").await;
    assert_eq!(missing_asset_response.status(), StatusCode::NOT_FOUND);

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn static_asset_cache_prunes_to_entry_budget() {
    let sandbox = unique_test_dir("static-assets-cache-budget");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    let static_dir = workspace.join("static");
    let immutable_dir = static_dir.join("_app").join("immutable");
    fs::create_dir_all(&immutable_dir).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    fs::write(static_dir.join("index.html"), "<html></html>").unwrap();
    fs::write(static_dir.join("200.html"), "<html></html>").unwrap();

    let state = test_state_with_static_dir_and_base_path(
        workspace.clone(),
        vec![workspace.clone()],
        codex_home,
        static_dir,
        "",
    );

    for index in 0..=STATIC_ASSET_CACHE_MAX_ENTRIES {
        let file_name = format!("asset-{index}.js");
        fs::write(
            immutable_dir.join(&file_name),
            format!("console.log({index});"),
        )
        .unwrap();
        let response =
            serve_static_asset(state.clone(), &format!("/_app/immutable/{file_name}")).await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    let cache = state.static_asset_cache.lock().await;
    assert!(cache.len() <= STATIC_ASSET_CACHE_MAX_ENTRIES);
    assert!(!cache.contains_key("_app/immutable/asset-0.js"));
    assert!(cache.contains_key(&format!(
        "_app/immutable/asset-{}.js",
        STATIC_ASSET_CACHE_MAX_ENTRIES
    )));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_fetch_and_pull_payloads_use_rust_helpers() {
    let sandbox = unique_test_dir("git-fetch-pull");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    let seed = workspace.join("seed");
    let remote = workspace.join("remote.git");
    let local = workspace.join("local");
    let updater = workspace.join("updater");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    init_test_git_repo(&seed);

    let current_branch = {
        let output = std::process::Command::new("git")
            .args(["-C", seed.to_str().unwrap(), "branch", "--show-current"])
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };

    let clone_bare = std::process::Command::new("git")
        .args([
            "clone",
            "--bare",
            seed.to_str().unwrap(),
            remote.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(clone_bare.status.success(), "git clone --bare failed");

    let clone_local = std::process::Command::new("git")
        .args(["clone", remote.to_str().unwrap(), local.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(clone_local.status.success(), "git clone local failed");

    let clone_updater = std::process::Command::new("git")
        .args(["clone", remote.to_str().unwrap(), updater.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(clone_updater.status.success(), "git clone updater failed");
    for args in [
        vec![
            "-C",
            updater.to_str().unwrap(),
            "config",
            "user.name",
            "Codex WebUI",
        ],
        vec![
            "-C",
            updater.to_str().unwrap(),
            "config",
            "user.email",
            "codex-webui@example.com",
        ],
    ] {
        let output = std::process::Command::new("git")
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "git updater config failed");
    }

    fs::write(updater.join("README.md"), "remote update\n").unwrap();
    let add = std::process::Command::new("git")
        .args(["-C", updater.to_str().unwrap(), "add", "README.md"])
        .output()
        .unwrap();
    assert!(add.status.success(), "git add updater failed");
    let commit = std::process::Command::new("git")
        .args([
            "-C",
            updater.to_str().unwrap(),
            "commit",
            "-m",
            "remote update",
        ])
        .output()
        .unwrap();
    assert!(commit.status.success(), "git commit updater failed");
    let push = std::process::Command::new("git")
        .args([
            "-C",
            updater.to_str().unwrap(),
            "push",
            "origin",
            &current_branch,
        ])
        .output()
        .unwrap();
    assert!(push.status.success(), "git push updater failed");

    let state = test_state(workspace.clone(), vec![workspace.clone()], codex_home);

    let fetched = fetch_git_repository_payload(&state, local.to_str().unwrap())
        .await
        .unwrap();
    assert!(
        fetched
            .get("behind")
            .and_then(Value::as_u64)
            .is_some_and(|value| value >= 1)
    );

    let pulled = pull_git_repository_payload(&state, local.to_str().unwrap())
        .await
        .unwrap();
    assert_eq!(pulled.get("clean").and_then(Value::as_bool), Some(true));
    assert_eq!(
        fs::read_to_string(local.join("README.md")).unwrap(),
        "remote update\n"
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[test]
fn parses_github_remote_urls() {
    let ssh =
        parse_github_remote_payload("origin", "git@github.com:openai/codex-webui.git").unwrap();
    assert_eq!(ssh.get("host").and_then(Value::as_str), Some("github.com"));
    assert_eq!(ssh.get("owner").and_then(Value::as_str), Some("openai"));
    assert_eq!(ssh.get("name").and_then(Value::as_str), Some("codex-webui"));

    let https =
        parse_github_remote_payload("upstream", "https://github.com/openai/codex-webui.git")
            .unwrap();
    assert_eq!(
        https.get("remoteName").and_then(Value::as_str),
        Some("upstream")
    );
    assert_eq!(
        https.get("url").and_then(Value::as_str),
        Some("https://github.com/openai/codex-webui")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolves_github_repository_payload_from_git_remote() {
    let sandbox = unique_test_dir("github-remote");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    let repo = workspace.join("repo");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    init_test_git_repo(&repo);

    let remote = std::process::Command::new("git")
        .args([
            "-C",
            repo.to_str().unwrap(),
            "remote",
            "add",
            "origin",
            "git@github.com:openai/codex-webui.git",
        ])
        .output()
        .unwrap();
    assert!(remote.status.success(), "git remote add failed");

    let state = test_state(workspace.clone(), vec![workspace.clone()], codex_home);
    let repository = resolve_github_repository_payload(&state, repo.to_str().unwrap())
        .await
        .unwrap();
    assert_eq!(
        repository.get("owner").and_then(Value::as_str),
        Some("openai")
    );
    assert_eq!(
        repository.get("name").and_then(Value::as_str),
        Some("codex-webui")
    );

    let _ = fs::remove_dir_all(sandbox);
}
