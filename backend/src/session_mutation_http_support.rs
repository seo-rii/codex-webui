use super::*;

pub(crate) async fn handle_session_fork_api_http(
    state: AppState,
    session_id: &str,
    request: Request,
    auth: AuthContext,
) -> Response {
    if request.method() != Method::POST {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }
    if auth.role != UserRole::Admin {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    let result = match read_json_body(request, SMALL_JSON_BODY_LIMIT, "session fork body").await {
        Ok(payload) => {
            fork_session_payload(
                &state,
                &auth.profile_id,
                session_id,
                payload
                    .get("mode")
                    .and_then(Value::as_str)
                    .unwrap_or("fork"),
                payload.get("turnId").and_then(Value::as_str),
                payload.get("messageText").and_then(Value::as_str),
            )
            .await
        }
        Err(error) => Err(error),
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

pub(crate) async fn handle_session_organization_api_http(
    state: AppState,
    session_id: &str,
    request: Request,
    auth: AuthContext,
) -> Response {
    if request.method() != Method::PATCH {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }
    if auth.role != UserRole::Admin {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    let result = match read_json_body(request, SMALL_JSON_BODY_LIMIT, "session organization body")
        .await
    {
        Ok(payload) => {
            update_session_organization_payload(&state, &auth.profile_id, session_id, payload).await
        }
        Err(error) => Err(error),
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

pub(crate) async fn handle_session_name_api_http(
    state: AppState,
    session_id: &str,
    request: Request,
    auth: AuthContext,
) -> Response {
    if request.method() != Method::POST {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }
    if auth.role != UserRole::Admin {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    let result = match read_json_body(request, SMALL_JSON_BODY_LIMIT, "session name body").await {
        Ok(payload) => {
            rename_session_payload(
                &state,
                &auth.profile_id,
                session_id,
                payload
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
            .await
        }
        Err(error) => Err(error),
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

pub(crate) async fn handle_session_archive_api_http(
    state: AppState,
    session_id: &str,
    request: Request,
    auth: AuthContext,
    archived: bool,
) -> Response {
    if request.method() != Method::POST {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }
    if auth.role != UserRole::Admin {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    let result = if archived {
        archive_session_payload(&state, &auth.profile_id, session_id).await
    } else {
        unarchive_session_payload(&state, &auth.profile_id, session_id).await
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

pub(crate) async fn handle_session_draft_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    session_id: &str,
) -> Response {
    let result = match request.method() {
        &Method::GET => get_session_draft_payload(&state, &auth.profile_id, session_id).await,
        &Method::PATCH => {
            if auth.role != UserRole::Admin {
                return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
            }
            match read_json_body(request, SMALL_JSON_BODY_LIMIT, "draft request body").await {
                Ok(payload) => {
                    save_session_draft_payload(
                        &state,
                        &auth.profile_id,
                        session_id,
                        payload
                            .get("draft")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                        payload
                            .get("intent")
                            .and_then(Value::as_str)
                            .unwrap_or("message"),
                    )
                    .await
                }
                Err(error) => Err(error),
            }
        }
        &Method::DELETE => {
            if auth.role != UserRole::Admin {
                return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
            }
            clear_session_draft_payload(&state, &auth.profile_id, session_id).await
        }
        _ => return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed."),
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

pub(crate) async fn handle_session_messages_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    session_id: &str,
) -> Response {
    if request.method() != Method::POST {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }
    if auth.role != UserRole::Admin {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    let result = match read_json_body(request, LARGE_JSON_BODY_LIMIT, "session message body").await
    {
        Ok(payload) => {
            send_turn_payload(
                &state,
                &auth.profile_id,
                session_id,
                payload
                    .get("prompt")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                payload.get("attachmentIds"),
                payload.get("skills"),
                payload
                    .get("preferences")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
            )
            .await
        }
        Err(error) => Err(error),
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

pub(crate) async fn handle_session_steer_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    session_id: &str,
) -> Response {
    if request.method() != Method::POST {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }
    if auth.role != UserRole::Admin {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    let result = match read_json_body(request, LARGE_JSON_BODY_LIMIT, "session steer body").await {
        Ok(payload) => {
            steer_turn_payload(
                &state,
                &auth.profile_id,
                session_id,
                payload
                    .get("prompt")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                payload.get("attachmentIds"),
                payload.get("skills"),
            )
            .await
        }
        Err(error) => Err(error),
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}
