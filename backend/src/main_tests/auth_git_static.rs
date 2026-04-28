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
    let (first_tx, _first_rx) = mpsc::unbounded_channel();
    let (second_tx, _second_rx) = mpsc::unbounded_channel();

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
    assert!(websocket_origin_allowed(
        &state.config,
        &same_origin_headers
    ));

    let mut forwarded_https_headers = same_origin_headers.clone();
    forwarded_https_headers.insert(
        header::ORIGIN,
        HeaderValue::from_static("https://127.0.0.1:4173"),
    );
    forwarded_https_headers.insert("x-forwarded-proto", HeaderValue::from_static("https, http"));
    assert!(!request_is_secure(&state.config, &forwarded_https_headers));
    assert!(!websocket_origin_allowed(
        &state.config,
        &forwarded_https_headers
    ));

    let mut trusted_proxy_config = (*state.config).clone();
    trusted_proxy_config.trust_proxy_headers = true;
    assert!(request_is_secure(
        &trusted_proxy_config,
        &forwarded_https_headers
    ));
    assert!(websocket_origin_allowed(
        &trusted_proxy_config,
        &forwarded_https_headers
    ));

    let mut rejected_headers = same_origin_headers.clone();
    rejected_headers.insert(
        header::ORIGIN,
        HeaderValue::from_static("https://attacker.example"),
    );
    assert!(!websocket_origin_allowed(&state.config, &rejected_headers));

    let mut config = (*state.config).clone();
    config.cors_allowed_origins = vec!["https://attacker.example".to_string()];
    state.config = Arc::new(config);
    assert!(websocket_origin_allowed(&state.config, &rejected_headers));

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
    let response = handle_http(State(state), CookieJar::new(), same_origin).await;
    assert_ne!(response.status(), StatusCode::FORBIDDEN);

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
    state.config = Arc::new(config);
    fs::create_dir_all(&state.config.data_dir).unwrap();

    let health_request = Request::builder()
        .method(Method::GET)
        .uri("/healthz")
        .header("x-codex-webui-instance-token", "probe-token")
        .body(Body::empty())
        .unwrap();
    let health_response = handle_http(State(state.clone()), CookieJar::new(), health_request).await;
    assert_eq!(health_response.status(), StatusCode::OK);
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
    let metrics_response = handle_http(State(state), jar, metrics_request).await;
    assert_eq!(metrics_response.status(), StatusCode::OK);
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

    state.config = Arc::new(config);
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
            .all(|cookie| cookie.contains("Path=/absproxy/4173"))
    );

    let _ = fs::remove_dir_all(sandbox);
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
    )
    .await;

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
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
        "<html><body>/__CODEX_WEBUI_BASE__/index</body></html>",
    )
    .unwrap();
    fs::write(
        static_dir.join("200.html"),
        "<html><body>/__CODEX_WEBUI_BASE__/fallback</body></html>",
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
