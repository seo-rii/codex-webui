use super::*;

pub(crate) async fn handle_http(
    State(state): State<AppState>,
    jar: CookieJar,
    request: Request,
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

            if route_path.starts_with("/api/auth/") {
                return handle_auth_http(state, jar, method, route_path, headers, request)
                    .await
                    .into_response();
            }

            if route_path == "/api/account" || route_path.starts_with("/api/account/") {
                let Some(auth) = auth_context(&state.config, &jar) else {
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
                let Some(auth) = auth_context(&state.config, &jar) else {
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
                let Some(auth) = auth_context(&state.config, &jar) else {
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
                let Some(auth) = auth_context(&state.config, &jar) else {
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
                let Some(auth) = auth_context(&state.config, &jar) else {
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
                    request,
                    &route_path,
                    cors_origin.as_deref(),
                    requested_headers.as_deref(),
                )
                .await;
            }

            if route_path == "/api/events/stream" {
                let Some(auth) = auth_context(&state.config, &jar) else {
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
                let mut response = handle_events_stream_http(state, request, auth).await;
                if let Some(origin_value) = cors_origin {
                    apply_cors_headers(
                        response.headers_mut(),
                        &origin_value,
                        requested_headers.as_deref(),
                    );
                }
                return response;
            }

            if route_path.starts_with("/api/") {
                if auth_context(&state.config, &jar).is_none() {
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
