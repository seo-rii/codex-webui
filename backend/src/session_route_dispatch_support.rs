use super::*;

fn apply_route_cors(
    mut response: Response,
    cors_origin: Option<&str>,
    requested_headers: Option<&str>,
) -> Response {
    if let Some(origin_value) = cors_origin {
        apply_cors_headers(response.headers_mut(), origin_value, requested_headers);
    }
    response
}

fn unauthorized_route_response(
    cors_origin: Option<&str>,
    requested_headers: Option<&str>,
) -> Response {
    apply_route_cors(
        json_error(StatusCode::UNAUTHORIZED, "Authentication required."),
        cors_origin,
        requested_headers,
    )
}

fn not_found_route_response(
    cors_origin: Option<&str>,
    requested_headers: Option<&str>,
) -> Response {
    apply_route_cors(
        json_error(StatusCode::NOT_FOUND, "Not found."),
        cors_origin,
        requested_headers,
    )
}

fn invalid_session_id_route_response(
    cors_origin: Option<&str>,
    requested_headers: Option<&str>,
) -> Response {
    apply_route_cors(
        json_error(StatusCode::BAD_REQUEST, "Invalid session id."),
        cors_origin,
        requested_headers,
    )
}

fn session_auth_or_response(
    state: &AppState,
    jar: &CookieJar,
    cors_origin: Option<&str>,
    requested_headers: Option<&str>,
) -> std::result::Result<AuthContext, Response> {
    auth_context(&state.config, jar)
        .ok_or_else(|| unauthorized_route_response(cors_origin, requested_headers))
}

fn session_id_from_suffix(route_path: &str, suffix: &str) -> String {
    route_path
        .strip_prefix("/api/sessions/")
        .and_then(|value| value.strip_suffix(suffix))
        .unwrap_or_default()
        .trim_end_matches('/')
        .to_string()
}

pub(crate) async fn handle_session_route_http(
    state: AppState,
    jar: &CookieJar,
    request: Request,
    route_path: &str,
    cors_origin: Option<&str>,
    requested_headers: Option<&str>,
) -> Response {
    macro_rules! checked_session_id {
        ($value:expr) => {
            match validate_session_id(&$value) {
                Ok(session_id) => session_id,
                Err(_) => {
                    return invalid_session_id_route_response(cors_origin, requested_headers);
                }
            }
        };
    }

    if route_path == "/api/sessions" {
        let auth = match session_auth_or_response(&state, jar, cors_origin, requested_headers) {
            Ok(auth) => auth,
            Err(response) => return response,
        };
        return apply_route_cors(
            handle_sessions_api_http(state, request, auth).await,
            cors_origin,
            requested_headers,
        );
    }

    if route_path.starts_with("/api/sessions/") && route_path.ends_with("/organization") {
        let auth = match session_auth_or_response(&state, jar, cors_origin, requested_headers) {
            Ok(auth) => auth,
            Err(response) => return response,
        };
        let session_id = checked_session_id!(session_id_from_suffix(route_path, "/organization"));
        return apply_route_cors(
            handle_session_organization_api_http(state, &session_id, request, auth).await,
            cors_origin,
            requested_headers,
        );
    }

    if route_path.starts_with("/api/sessions/") && route_path.ends_with("/name") {
        let auth = match session_auth_or_response(&state, jar, cors_origin, requested_headers) {
            Ok(auth) => auth,
            Err(response) => return response,
        };
        let session_id = checked_session_id!(session_id_from_suffix(route_path, "/name"));
        return apply_route_cors(
            handle_session_name_api_http(state, &session_id, request, auth).await,
            cors_origin,
            requested_headers,
        );
    }

    if route_path.starts_with("/api/sessions/") && route_path.ends_with("/archive") {
        let auth = match session_auth_or_response(&state, jar, cors_origin, requested_headers) {
            Ok(auth) => auth,
            Err(response) => return response,
        };
        let session_id = checked_session_id!(session_id_from_suffix(route_path, "/archive"));
        return apply_route_cors(
            handle_session_archive_api_http(state, &session_id, request, auth, true).await,
            cors_origin,
            requested_headers,
        );
    }

    if route_path.starts_with("/api/sessions/") && route_path.ends_with("/unarchive") {
        let auth = match session_auth_or_response(&state, jar, cors_origin, requested_headers) {
            Ok(auth) => auth,
            Err(response) => return response,
        };
        let session_id = checked_session_id!(session_id_from_suffix(route_path, "/unarchive"));
        return apply_route_cors(
            handle_session_archive_api_http(state, &session_id, request, auth, false).await,
            cors_origin,
            requested_headers,
        );
    }

    if route_path.starts_with("/api/sessions/") && route_path.ends_with("/fork") {
        let auth = match session_auth_or_response(&state, jar, cors_origin, requested_headers) {
            Ok(auth) => auth,
            Err(response) => return response,
        };
        let session_id = checked_session_id!(session_id_from_suffix(route_path, "/fork"));
        return apply_route_cors(
            handle_session_fork_api_http(state, &session_id, request, auth).await,
            cors_origin,
            requested_headers,
        );
    }

    if let Some(session_id) = route_path
        .strip_prefix("/api/sessions/")
        .filter(|value| !value.is_empty() && !value.contains('/'))
        .map(str::to_string)
    {
        let auth = match session_auth_or_response(&state, jar, cors_origin, requested_headers) {
            Ok(auth) => auth,
            Err(response) => return response,
        };
        let session_id = checked_session_id!(session_id);
        return apply_route_cors(
            handle_session_api_http(state, &session_id, request, auth).await,
            cors_origin,
            requested_headers,
        );
    }

    if route_path.starts_with("/api/sessions/") && route_path.ends_with("/draft") {
        let auth = match session_auth_or_response(&state, jar, cors_origin, requested_headers) {
            Ok(auth) => auth,
            Err(response) => return response,
        };
        let session_id = checked_session_id!(session_id_from_suffix(route_path, "/draft"));
        return apply_route_cors(
            handle_session_draft_api_http(state, request, auth, &session_id).await,
            cors_origin,
            requested_headers,
        );
    }

    if route_path.starts_with("/api/sessions/") && route_path.ends_with("/messages") {
        let auth = match session_auth_or_response(&state, jar, cors_origin, requested_headers) {
            Ok(auth) => auth,
            Err(response) => return response,
        };
        let session_id = checked_session_id!(session_id_from_suffix(route_path, "/messages"));
        return apply_route_cors(
            handle_session_messages_api_http(state, request, auth, &session_id).await,
            cors_origin,
            requested_headers,
        );
    }

    if route_path.starts_with("/api/sessions/") && route_path.ends_with("/steer") {
        let auth = match session_auth_or_response(&state, jar, cors_origin, requested_headers) {
            Ok(auth) => auth,
            Err(response) => return response,
        };
        let session_id = checked_session_id!(session_id_from_suffix(route_path, "/steer"));
        return apply_route_cors(
            handle_session_steer_api_http(state, request, auth, &session_id).await,
            cors_origin,
            requested_headers,
        );
    }

    if route_path.starts_with("/api/sessions/") && route_path.contains("/queue") {
        let auth = match session_auth_or_response(&state, jar, cors_origin, requested_headers) {
            Ok(auth) => auth,
            Err(response) => return response,
        };
        let session_id = route_path
            .strip_prefix("/api/sessions/")
            .and_then(|suffix| suffix.split("/queue").next())
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string();
        let session_id = checked_session_id!(session_id);
        return apply_route_cors(
            handle_session_queue_api_http(state, request, auth, &session_id, route_path).await,
            cors_origin,
            requested_headers,
        );
    }

    if route_path.starts_with("/api/sessions/") && route_path.ends_with("/search") {
        let auth = match session_auth_or_response(&state, jar, cors_origin, requested_headers) {
            Ok(auth) => auth,
            Err(response) => return response,
        };
        let session_id = checked_session_id!(session_id_from_suffix(route_path, "/search"));
        return apply_route_cors(
            handle_session_search_api_http(state, request, auth, &session_id).await,
            cors_origin,
            requested_headers,
        );
    }

    if route_path.starts_with("/api/sessions/") && route_path.ends_with("/turns") {
        let auth = match session_auth_or_response(&state, jar, cors_origin, requested_headers) {
            Ok(auth) => auth,
            Err(response) => return response,
        };
        let session_id = checked_session_id!(session_id_from_suffix(route_path, "/turns"));
        return apply_route_cors(
            handle_session_turns_api_http(state, request, auth, &session_id).await,
            cors_origin,
            requested_headers,
        );
    }

    if route_path.starts_with("/api/sessions/")
        && route_path.contains("/turns/")
        && route_path.contains("/items/")
    {
        let auth = match session_auth_or_response(&state, jar, cors_origin, requested_headers) {
            Ok(auth) => auth,
            Err(response) => return response,
        };
        let mut segments = route_path
            .trim_start_matches("/api/sessions/")
            .split("/turns/");
        let session_id = segments
            .next()
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string();
        let session_id = checked_session_id!(session_id);
        let rest = segments.next().unwrap_or_default();
        let mut turn_segments = rest.split("/items/");
        let turn_id = turn_segments
            .next()
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string();
        let item_id = turn_segments
            .next()
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string();
        return apply_route_cors(
            handle_session_item_detail_api_http(
                state,
                request,
                auth,
                &session_id,
                &turn_id,
                &item_id,
            )
            .await,
            cors_origin,
            requested_headers,
        );
    }

    if route_path.starts_with("/api/sessions/")
        && route_path.trim_end_matches('/').ends_with("/attachments")
        && !route_path.contains("/attachments/")
    {
        let auth = match session_auth_or_response(&state, jar, cors_origin, requested_headers) {
            Ok(auth) => auth,
            Err(response) => return response,
        };
        let session_id = route_path
            .trim_end_matches('/')
            .strip_prefix("/api/sessions/")
            .and_then(|suffix| suffix.strip_suffix("/attachments"))
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string();
        let session_id = checked_session_id!(session_id);
        return apply_route_cors(
            handle_session_attachments_api_http(state, request, auth, &session_id).await,
            cors_origin,
            requested_headers,
        );
    }

    if route_path.starts_with("/api/sessions/") && route_path.contains("/attachments/") {
        let auth = match session_auth_or_response(&state, jar, cors_origin, requested_headers) {
            Ok(auth) => auth,
            Err(response) => return response,
        };
        let mut segments = route_path
            .trim_start_matches("/api/sessions/")
            .split("/attachments/");
        let session_id = segments
            .next()
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string();
        let session_id = checked_session_id!(session_id);
        let attachment_id = segments
            .next()
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string();
        return apply_route_cors(
            handle_session_attachment_api_http(state, request, auth, &session_id, &attachment_id)
                .await,
            cors_origin,
            requested_headers,
        );
    }

    if route_path.starts_with("/api/sessions/") && route_path.contains("/turns/") {
        let auth = match session_auth_or_response(&state, jar, cors_origin, requested_headers) {
            Ok(auth) => auth,
            Err(response) => return response,
        };
        let mut segments = route_path
            .trim_start_matches("/api/sessions/")
            .split("/turns/");
        let session_id = segments
            .next()
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string();
        let session_id = checked_session_id!(session_id);
        let turn_id = segments
            .next()
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string();
        return apply_route_cors(
            handle_session_turn_api_http(state, request, auth, &session_id, &turn_id).await,
            cors_origin,
            requested_headers,
        );
    }

    if route_path.starts_with("/api/sessions/") && route_path.ends_with("/abort") {
        let auth = match session_auth_or_response(&state, jar, cors_origin, requested_headers) {
            Ok(auth) => auth,
            Err(response) => return response,
        };
        let session_id = checked_session_id!(session_id_from_suffix(route_path, "/abort"));
        return apply_route_cors(
            handle_session_abort_api_http(state, request, auth, &session_id).await,
            cors_origin,
            requested_headers,
        );
    }

    if route_path.starts_with("/api/sessions/") && route_path.ends_with("/approval") {
        let auth = match session_auth_or_response(&state, jar, cors_origin, requested_headers) {
            Ok(auth) => auth,
            Err(response) => return response,
        };
        let session_id = checked_session_id!(session_id_from_suffix(route_path, "/approval"));
        return apply_route_cors(
            handle_session_approval_api_http(state, request, auth, &session_id).await,
            cors_origin,
            requested_headers,
        );
    }

    if auth_context(&state.config, jar).is_none() {
        unauthorized_route_response(cors_origin, requested_headers)
    } else {
        not_found_route_response(cors_origin, requested_headers)
    }
}
