use super::*;

pub(crate) async fn handle_http(
    State(state): State<AppState>,
    jar: CookieJar,
    request: Request,
) -> Response {
    let peer_addr = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|connect_info| connect_info.0);
    let mut response = handle_http_inner(state, jar, request, peer_addr).await;
    apply_security_headers(response.headers_mut());
    response
}

async fn handle_http_inner(
    state: AppState,
    jar: CookieJar,
    request: Request,
    peer_addr: Option<SocketAddr>,
) -> Response {
    let method = request.method().clone();
    let uri = request.uri().clone();
    let headers = request.headers().clone();
    let path = uri.path().to_string();

    match normalize_request_path(&state.config.base_path, &path) {
        NormalizedPath::Redirect(target) => Redirect::temporary(&target).into_response(),
        NormalizedPath::OutsideBase => (StatusCode::NOT_FOUND, "Not found").into_response(),
        NormalizedPath::Route(route_path) => {
            let origin = extract_origin(&headers);
            let cors_origin = allowed_cors_origin(&state.config, &origin);
            let route_requires_admin = |route_path: &str| {
                matches!(
                    route_path,
                    "/api/account"
                        | "/api/account/login"
                        | "/api/account/login/cancel"
                        | "/api/account/logout"
                        | "/api/config"
                        | "/api/directories"
                        | "/api/editor"
                        | "/api/catalog"
                        | "/api/git/repositories"
                ) || route_path.starts_with("/api/git/")
            };
            let requested_headers = headers
                .get("access-control-request-headers")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);

            if route_path.starts_with("/api/")
                && method == Method::OPTIONS
                && headers.contains_key("access-control-request-method")
            {
                if let Some(origin_value) = cors_origin {
                    let mut response = Response::new(Body::empty());
                    *response.status_mut() = StatusCode::NO_CONTENT;
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                    return response;
                }
                return (StatusCode::FORBIDDEN, "CORS origin is not allowed.").into_response();
            }

            if route_path.starts_with("/api/")
                && matches!(
                    method,
                    Method::POST | Method::PUT | Method::PATCH | Method::DELETE
                )
                && !request_origin_allowed(&state.config, &headers, peer_addr)
            {
                return (StatusCode::FORBIDDEN, "Request origin is not allowed.").into_response();
            }

            if route_path.starts_with("/api/")
                && matches!(
                    method,
                    Method::POST | Method::PUT | Method::PATCH | Method::DELETE
                )
                && route_path != "/api/auth/login"
                && route_path != "/api/auth/logout"
                && jar.get(AUTH_COOKIE).is_some()
                && auth_context_from_headers(&state.config, &jar, &headers).is_some()
                && !verify_csrf_token(&state.config, &jar, &headers)
            {
                let mut response =
                    json_error(StatusCode::FORBIDDEN, "CSRF token is missing or invalid.");
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            if route_path == "/healthz" {
                let instance_token_matched = headers
                    .get("x-codex-webui-instance-token")
                    .and_then(|value| value.to_str().ok())
                    .zip(state.config.instance_token.as_deref())
                    .is_some_and(|(provided, expected)| {
                        !expected.is_empty() && provided.trim() == expected
                    });
                let mut payload = json!({
                    "status": "ok",
                    "instanceTokenMatched": instance_token_matched
                });
                if instance_token_matched {
                    payload["version"] = json!(env!("CARGO_PKG_VERSION"));
                    payload["buildVersion"] = json!(
                        option_env!("CODEX_WEBUI_BUILD_VERSION")
                            .unwrap_or(env!("CARGO_PKG_VERSION"))
                    );
                    payload["buildCommit"] =
                        json!(option_env!("CODEX_WEBUI_BUILD_COMMIT").unwrap_or("unknown"));
                }
                return Json(payload).into_response();
            }

            if route_path == "/readyz" {
                let data_dir_exists = tokio_fs::metadata(&state.config.data_dir)
                    .await
                    .map(|metadata| metadata.is_dir())
                    .unwrap_or(false);
                let probe_path = state
                    .config
                    .data_dir
                    .join(format!(".readyz-{}", Uuid::new_v4()));
                let data_dir_writable = if data_dir_exists {
                    match tokio_fs::write(&probe_path, b"ok").await {
                        Ok(_) => {
                            let _ = tokio_fs::remove_file(&probe_path).await;
                            true
                        }
                        Err(_) => false,
                    }
                } else {
                    false
                };
                let has_profile = !state.config.profiles.is_empty()
                    && state
                        .config
                        .profiles
                        .contains_key(&state.config.default_profile_id);
                let has_allowed_roots = !state.config.allowed_roots.is_empty();
                let ready =
                    data_dir_exists && data_dir_writable && has_profile && has_allowed_roots;
                let mut response = Json(json!({
                    "status": if ready { "ready" } else { "degraded" },
                    "checks": {
                        "dataDirExists": data_dir_exists,
                        "dataDirWritable": data_dir_writable,
                        "defaultProfileConfigured": has_profile,
                        "allowedRootsConfigured": has_allowed_roots
                    }
                }))
                .into_response();
                if !ready {
                    *response.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
                }
                return response;
            }

            if route_path == "/metrics" {
                let Some(auth) = auth_context_from_headers(&state.config, &jar, &headers) else {
                    return json_error(StatusCode::UNAUTHORIZED, "Authentication required.");
                };
                if !role_has_admin_access(auth.role) {
                    return json_error(
                        StatusCode::FORBIDDEN,
                        "This action requires an admin role.",
                    );
                }

                let (response_cache_entries, response_cache_bytes) = {
                    let cache = state.response_cache.lock().await;
                    (
                        cache.len(),
                        cache
                            .values()
                            .map(|entry| entry.response_bytes)
                            .sum::<usize>(),
                    )
                };
                let session_thread_cache_entries = state.session_thread_cache.lock().await.len();
                let (session_search_cache_entries, session_search_cache_bytes) = {
                    let cache = state.session_search_text_cache.lock().await;
                    (
                        cache.len(),
                        cache.values().map(|entry| entry.text_bytes).sum::<usize>(),
                    )
                };
                let (static_asset_cache_entries, static_asset_cache_bytes) = {
                    let cache = state.static_asset_cache.lock().await;
                    (
                        cache.len(),
                        cache.values().map(|asset| asset.bytes.len()).sum::<usize>(),
                    )
                };
                let catalog_cache_entries = state.catalog_cache.lock().await.len();
                let quota_cache_entries = state.quota_cache.lock().await.len();
                let relay_count = state.relays.lock().await.len();
                let terminal_count = state.terminals.lock().await.len();
                let active_turn_count = state.active_turns.lock().await.len();
                let pending_turn_start_count = state.pending_turn_starts.lock().await.len();
                let pending_server_request_count = state
                    .pending_server_requests
                    .lock()
                    .await
                    .values()
                    .map(HashMap::len)
                    .sum::<usize>();
                let app_server_client_count = state.app_servers.client_count().await;

                let metrics = format!(
                    "# TYPE codex_webui_profiles gauge\n\
codex_webui_profiles {}\n\
# TYPE codex_webui_allowed_roots gauge\n\
codex_webui_allowed_roots {}\n\
# TYPE codex_webui_app_server_clients gauge\n\
codex_webui_app_server_clients {app_server_client_count}\n\
# TYPE codex_webui_response_cache_entries gauge\n\
codex_webui_response_cache_entries {response_cache_entries}\n\
# TYPE codex_webui_response_cache_bytes gauge\n\
codex_webui_response_cache_bytes {response_cache_bytes}\n\
# TYPE codex_webui_session_thread_cache_entries gauge\n\
codex_webui_session_thread_cache_entries {session_thread_cache_entries}\n\
# TYPE codex_webui_session_search_cache_entries gauge\n\
codex_webui_session_search_cache_entries {session_search_cache_entries}\n\
# TYPE codex_webui_session_search_cache_bytes gauge\n\
codex_webui_session_search_cache_bytes {session_search_cache_bytes}\n\
# TYPE codex_webui_static_asset_cache_entries gauge\n\
codex_webui_static_asset_cache_entries {static_asset_cache_entries}\n\
# TYPE codex_webui_static_asset_cache_bytes gauge\n\
codex_webui_static_asset_cache_bytes {static_asset_cache_bytes}\n\
# TYPE codex_webui_catalog_cache_entries gauge\n\
codex_webui_catalog_cache_entries {catalog_cache_entries}\n\
# TYPE codex_webui_quota_cache_entries gauge\n\
codex_webui_quota_cache_entries {quota_cache_entries}\n\
# TYPE codex_webui_relays gauge\n\
codex_webui_relays {relay_count}\n\
# TYPE codex_webui_terminals gauge\n\
codex_webui_terminals {terminal_count}\n\
# TYPE codex_webui_active_turns gauge\n\
codex_webui_active_turns {active_turn_count}\n\
# TYPE codex_webui_pending_turn_starts gauge\n\
codex_webui_pending_turn_starts {pending_turn_start_count}\n\
# TYPE codex_webui_pending_server_requests gauge\n\
codex_webui_pending_server_requests {pending_server_request_count}\n",
                    state.config.profiles.len(),
                    state.config.allowed_roots.len()
                );
                let mut response = Response::new(Body::from(metrics));
                response.headers_mut().insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
                );
                return response;
            }

            if route_path == "/api/admin/restart-handoff/prepare" {
                if method != Method::POST {
                    return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
                }
                let token_matches = headers
                    .get("x-codex-webui-instance-token")
                    .and_then(|value| value.to_str().ok())
                    .zip(state.config.instance_token.as_deref())
                    .is_some_and(|(provided, expected)| {
                        !expected.is_empty() && provided.trim() == expected
                    });
                let auth_allowed = auth_context_from_headers(&state.config, &jar, &headers)
                    .is_some_and(|auth| role_has_owner_access(&state.config, auth.role));
                if !token_matches && !auth_allowed {
                    return json_error(
                        StatusCode::FORBIDDEN,
                        "Instance token or owner role is required.",
                    );
                }
                state
                    .preserve_app_servers_on_shutdown
                    .store(true, Ordering::SeqCst);
                let mut response = Json(json!({
                    "ok": true,
                    "appServerClients": state.app_servers.client_count().await
                }))
                .into_response();
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            if route_path == "/api/admin/restart" {
                if method != Method::POST {
                    return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
                }
                let Some(auth) = auth_context_from_headers(&state.config, &jar, &headers) else {
                    return json_error(StatusCode::UNAUTHORIZED, "Authentication required.");
                };
                if !role_has_owner_access(&state.config, auth.role) {
                    return json_error(
                        StatusCode::FORBIDDEN,
                        "This action requires the owner role.",
                    );
                }
                let mut response = match prepare_gateway_restart_payload(&state).await {
                    Ok(payload) => Json(payload).into_response(),
                    Err(error) => json_error(error.status, &error.message),
                };
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            if route_path.starts_with("/api/auth/") {
                return handle_auth_http(
                    state, jar, method, route_path, headers, request, peer_addr,
                )
                .await
                .into_response();
            }

            if route_path == "/api/account" || route_path.starts_with("/api/account/") {
                let Some(auth) = auth_context_from_headers(&state.config, &jar, &headers) else {
                    let mut response =
                        json_error(StatusCode::UNAUTHORIZED, "Authentication required.");
                    if let Some(origin_value) = cors_origin {
                        apply_cors_headers(
                            response.headers_mut(),
                            &origin_value,
                            requested_headers.as_deref(),
                        );
                    }
                    return response;
                };
                if route_requires_admin(&route_path) && !role_has_admin_access(auth.role) {
                    let mut response =
                        json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
                    if let Some(origin_value) = cors_origin {
                        apply_cors_headers(
                            response.headers_mut(),
                            &origin_value,
                            requested_headers.as_deref(),
                        );
                    }
                    return response;
                }

                let mut response =
                    handle_account_api_http(state, method, route_path, request, auth).await;
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            if matches!(
                route_path.as_str(),
                "/api/config"
                    | "/api/directories"
                    | "/api/editor"
                    | "/api/catalog"
                    | "/api/notifications"
                    | "/api/notifications/settings"
                    | "/api/session-filters"
                    | "/api/prompt-presets"
            ) {
                let Some(auth) = auth_context_from_headers(&state.config, &jar, &headers) else {
                    let mut response =
                        json_error(StatusCode::UNAUTHORIZED, "Authentication required.");
                    if let Some(origin_value) = cors_origin {
                        apply_cors_headers(
                            response.headers_mut(),
                            &origin_value,
                            requested_headers.as_deref(),
                        );
                    }
                    return response;
                };
                if route_requires_admin(&route_path) && !role_has_admin_access(auth.role) {
                    let mut response =
                        json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
                    if let Some(origin_value) = cors_origin {
                        apply_cors_headers(
                            response.headers_mut(),
                            &origin_value,
                            requested_headers.as_deref(),
                        );
                    }
                    return response;
                }

                let mut response = match route_path.as_str() {
                    "/api/config" => handle_config_api_http(state, request, auth).await,
                    "/api/directories" => handle_directories_api_http(state, request).await,
                    "/api/editor" => handle_editor_api_http(state, request, auth).await,
                    "/api/catalog" => handle_catalog_api_http(state, request, auth).await,
                    "/api/notifications" => {
                        handle_notifications_api_http(state, request, auth).await
                    }
                    "/api/notifications/settings" => {
                        handle_notification_settings_api_http(state, request, auth).await
                    }
                    "/api/session-filters" => {
                        handle_session_filters_api_http(state, request, auth).await
                    }
                    "/api/prompt-presets" => {
                        handle_prompt_presets_api_http(state, request, auth).await
                    }
                    _ => unreachable!(),
                };
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            if route_path == "/api/git/repositories" || route_path.starts_with("/api/git/") {
                let Some(auth) = auth_context_from_headers(&state.config, &jar, &headers) else {
                    let mut response =
                        json_error(StatusCode::UNAUTHORIZED, "Authentication required.");
                    if let Some(origin_value) = cors_origin {
                        apply_cors_headers(
                            response.headers_mut(),
                            &origin_value,
                            requested_headers.as_deref(),
                        );
                    }
                    return response;
                };
                if route_requires_admin(&route_path) && !role_has_admin_access(auth.role) {
                    let mut response =
                        json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
                    if let Some(origin_value) = cors_origin {
                        apply_cors_headers(
                            response.headers_mut(),
                            &origin_value,
                            requested_headers.as_deref(),
                        );
                    }
                    return response;
                }

                let mut response = handle_git_api_http(state, request, auth, &route_path).await;
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            if route_path == "/api/automations" || route_path.starts_with("/api/automations/") {
                let Some(auth) = auth_context_from_headers(&state.config, &jar, &headers) else {
                    let mut response =
                        json_error(StatusCode::UNAUTHORIZED, "Authentication required.");
                    if let Some(origin_value) = cors_origin {
                        apply_cors_headers(
                            response.headers_mut(),
                            &origin_value,
                            requested_headers.as_deref(),
                        );
                    }
                    return response;
                };

                let mut response =
                    handle_automations_api_http(state, request, auth, &route_path).await;
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            if route_path == "/api/arena" {
                let Some(auth) = auth_context_from_headers(&state.config, &jar, &headers) else {
                    let mut response =
                        json_error(StatusCode::UNAUTHORIZED, "Authentication required.");
                    if let Some(origin_value) = cors_origin {
                        apply_cors_headers(
                            response.headers_mut(),
                            &origin_value,
                            requested_headers.as_deref(),
                        );
                    }
                    return response;
                };

                let mut response = handle_arena_api_http(state, request, auth).await;
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            if route_path == "/api/sessions" || route_path.starts_with("/api/sessions/") {
                return handle_session_route_http(
                    state,
                    &jar,
                    &headers,
                    request,
                    &route_path,
                    cors_origin.as_deref(),
                    requested_headers.as_deref(),
                )
                .await;
            }

            if route_path.starts_with("/api/") {
                if auth_context_from_headers(&state.config, &jar, &headers).is_none() {
                    let mut response =
                        json_error(StatusCode::UNAUTHORIZED, "Authentication required.");
                    if let Some(origin_value) = cors_origin {
                        apply_cors_headers(
                            response.headers_mut(),
                            &origin_value,
                            requested_headers.as_deref(),
                        );
                    }
                    return response;
                }

                let mut response = json_error(StatusCode::NOT_FOUND, "Not found.");
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            serve_static_asset(state, &route_path).await
        }
    }
}
