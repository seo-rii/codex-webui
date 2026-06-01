use super::*;

pub(crate) async fn handle_session_queue_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    session_id: &str,
    route_path: &str,
) -> Response {
    let queue_prefix = format!("/api/sessions/{session_id}/queue");
    let suffix = route_path.strip_prefix(&queue_prefix).unwrap_or_default();
    let requires_admin = request.method() != Method::GET;
    if requires_admin && !role_has_admin_access(auth.role) {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    let result = if suffix.is_empty() {
        match request.method() {
            &Method::GET => {
                get_session_queue_payload_for_role(&state, &auth.profile_id, session_id, auth.role)
                    .await
            }
            &Method::POST => {
                match read_json_body(request, LARGE_JSON_BODY_LIMIT, "queue request body").await {
                    Ok(payload) => {
                        enqueue_session_queue_payload(
                            &state,
                            &auth.profile_id,
                            session_id,
                            payload
                                .get("prompt")
                                .and_then(Value::as_str)
                                .unwrap_or_default(),
                            payload.get("clientRequestId").and_then(Value::as_str),
                            payload.get("clientUserMessageId").and_then(Value::as_str),
                            payload.get("skills"),
                            payload.get("attachmentIds"),
                        )
                        .await
                    }
                    Err(error) => Err(error),
                }
            }
            _ => Err(api_error(
                StatusCode::METHOD_NOT_ALLOWED,
                "Method not allowed.",
            )),
        }
    } else if suffix == "/resume" {
        if request.method() != Method::POST {
            Err(api_error(
                StatusCode::METHOD_NOT_ALLOWED,
                "Method not allowed.",
            ))
        } else {
            resume_session_queue_payload(&state, &auth.profile_id, session_id).await
        }
    } else if suffix == "/reorder" {
        if request.method() != Method::POST {
            Err(api_error(
                StatusCode::METHOD_NOT_ALLOWED,
                "Method not allowed.",
            ))
        } else {
            match read_json_body(request, SMALL_JSON_BODY_LIMIT, "queue reorder request body").await
            {
                Ok(payload) => {
                    let queue_ids = string_array_from_value(payload.get("queueIds"));
                    reorder_session_queue_payload(&state, &auth.profile_id, session_id, &queue_ids)
                        .await
                }
                Err(error) => Err(error),
            }
        }
    } else {
        let queue_id = suffix.trim_start_matches('/');
        if queue_id.is_empty() || queue_id.contains('/') {
            Err(api_error(StatusCode::NOT_FOUND, "Not found."))
        } else {
            match request.method() {
                &Method::DELETE => {
                    remove_session_queue_item_payload(
                        &state,
                        &auth.profile_id,
                        session_id,
                        queue_id,
                    )
                    .await
                }
                &Method::PATCH => {
                    match read_json_body(
                        request,
                        LARGE_JSON_BODY_LIMIT,
                        "queue update request body",
                    )
                    .await
                    {
                        Ok(payload) => {
                            update_session_queue_item_payload(
                                &state,
                                &auth.profile_id,
                                session_id,
                                queue_id,
                                payload.get("prompt").and_then(Value::as_str),
                                payload.get("skills"),
                                payload.get("attachmentIds"),
                            )
                            .await
                        }
                        Err(error) => Err(error),
                    }
                }
                &Method::POST => {
                    match read_json_body(
                        request,
                        SMALL_JSON_BODY_LIMIT,
                        "queue dispatch request body",
                    )
                    .await
                    {
                        Ok(payload) => {
                            dispatch_session_queue_item_payload(
                                &state,
                                &auth.profile_id,
                                session_id,
                                queue_id,
                                payload
                                    .get("mode")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default(),
                                payload
                                    .get("activeTurnId")
                                    .or_else(|| payload.get("expectedTurnId"))
                                    .and_then(Value::as_str),
                            )
                            .await
                        }
                        Err(error) => Err(error),
                    }
                }
                _ => Err(api_error(
                    StatusCode::METHOD_NOT_ALLOWED,
                    "Method not allowed.",
                )),
            }
        }
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

pub(crate) async fn handle_session_abort_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    session_id: &str,
) -> Response {
    if request.method() != Method::POST {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }
    if !role_has_admin_access(auth.role) {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    match abort_turn_payload(&state, &auth.profile_id, session_id).await {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

pub(crate) async fn handle_session_approval_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    session_id: &str,
) -> Response {
    if request.method() != Method::POST {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }
    if !role_has_admin_access(auth.role) {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    let result = match read_json_body(request, SMALL_JSON_BODY_LIMIT, "approval request body").await
    {
        Ok(payload) => {
            let request_id = payload
                .get("requestId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            resolve_server_request_payload(
                &state,
                &auth.profile_id,
                session_id,
                &request_id,
                payload.get("result").cloned().unwrap_or(Value::Null),
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
