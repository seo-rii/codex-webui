use super::*;

pub(crate) async fn handle_sessions_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
) -> Response {
    let query = request.uri().query().map(str::to_string);
    let result = match request.method() {
        &Method::GET => {
            let archived =
                query_param_value(query.as_deref(), "archived").as_deref() == Some("true");
            let cursor = query_param_value(query.as_deref(), "cursor");
            let limit = query_param_value(query.as_deref(), "limit")
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(20);
            let search_query = query_param_value(query.as_deref(), "query").unwrap_or_default();
            let scope = query_param_value(query.as_deref(), "scope")
                .unwrap_or_else(|| "summary".to_string());
            let filter = session_filter_from_query(query.as_deref());

            if search_query.trim().is_empty() {
                list_sessions_payload(
                    &state,
                    &auth.profile_id,
                    archived,
                    cursor.as_deref(),
                    limit,
                    &filter,
                )
                .await
            } else {
                search_sessions_payload(
                    &state,
                    &auth.profile_id,
                    &search_query,
                    &scope,
                    archived,
                    cursor.as_deref(),
                    limit,
                    &filter,
                )
                .await
            }
        }
        &Method::POST => {
            if auth.role != UserRole::Admin {
                return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
            }

            let body = to_bytes(request.into_body(), usize::MAX)
                .await
                .context("failed to read session create body");
            match body {
                Ok(body) => {
                    let payload: Value =
                        serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
                    create_session_payload(
                        &state,
                        &auth.profile_id,
                        payload
                            .get("preferences")
                            .cloned()
                            .unwrap_or_else(|| json!({})),
                        payload.get("selectedSkills"),
                        payload.get("name").and_then(Value::as_str),
                    )
                    .await
                }
                Err(_) => Err(api_error(
                    StatusCode::BAD_REQUEST,
                    "Failed to read session create body.",
                )),
            }
        }
        _ => return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed."),
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

pub(crate) async fn handle_session_api_http(
    state: AppState,
    session_id: &str,
    request: Request,
    auth: AuthContext,
) -> Response {
    let result = match request.method() {
        &Method::GET => {
            let limit = query_param_value(request.uri().query(), "limit")
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(20);
            session_detail_payload(&state, &auth.profile_id, session_id, limit).await
        }
        &Method::PATCH => {
            if auth.role != UserRole::Admin {
                return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
            }

            let body = to_bytes(request.into_body(), usize::MAX)
                .await
                .context("failed to read session update body");
            match body {
                Ok(body) => {
                    let payload: Value =
                        serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
                    save_session_preferences_payload(
                        &state,
                        &auth.profile_id,
                        session_id,
                        payload
                            .get("preferences")
                            .cloned()
                            .unwrap_or_else(|| json!({})),
                    )
                    .await
                }
                Err(_) => Err(api_error(
                    StatusCode::BAD_REQUEST,
                    "Failed to read session update body.",
                )),
            }
        }
        _ => return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed."),
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

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

    let result = match to_bytes(request.into_body(), usize::MAX)
        .await
        .context("failed to read session fork body")
    {
        Ok(body) => {
            let payload: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
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
        Err(_) => Err(api_error(
            StatusCode::BAD_REQUEST,
            "Failed to read session fork body.",
        )),
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

    let result = match to_bytes(request.into_body(), usize::MAX)
        .await
        .context("failed to read session organization body")
    {
        Ok(body) => {
            let payload: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
            update_session_organization_payload(&state, &auth.profile_id, session_id, payload).await
        }
        Err(_) => Err(api_error(
            StatusCode::BAD_REQUEST,
            "Failed to read session organization body.",
        )),
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

    let result = match to_bytes(request.into_body(), usize::MAX)
        .await
        .context("failed to read session name body")
    {
        Ok(body) => {
            let payload: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
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
        Err(_) => Err(api_error(
            StatusCode::BAD_REQUEST,
            "Failed to read session name body.",
        )),
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
            let body = to_bytes(request.into_body(), usize::MAX)
                .await
                .context("failed to read draft request body");
            match body {
                Ok(body) => {
                    let payload: Value =
                        serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
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
                Err(_) => Err(api_error(
                    StatusCode::BAD_REQUEST,
                    "Failed to read draft request body.",
                )),
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

    let result = match to_bytes(request.into_body(), usize::MAX)
        .await
        .context("failed to read session message body")
    {
        Ok(body) => {
            let payload: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
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
        Err(_) => Err(api_error(
            StatusCode::BAD_REQUEST,
            "Failed to read session message body.",
        )),
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

    let result = match to_bytes(request.into_body(), usize::MAX)
        .await
        .context("failed to read session steer body")
    {
        Ok(body) => {
            let payload: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
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
        Err(_) => Err(api_error(
            StatusCode::BAD_REQUEST,
            "Failed to read session steer body.",
        )),
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

pub(crate) async fn handle_session_attachments_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    session_id: &str,
) -> Response {
    let method = request.method().clone();
    match method {
        Method::GET => {
            match list_session_attachments_payload(&state, &auth.profile_id, session_id).await {
                Ok(attachments) => Json(json!({ "attachments": attachments })).into_response(),
                Err(error) => json_error(error.status, &error.message),
            }
        }
        Method::POST => {
            if auth.role != UserRole::Admin {
                return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
            }

            let multipart = match Multipart::from_request(request, &()).await {
                Ok(multipart) => multipart,
                Err(_) => {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        "Failed to read attachment upload body.",
                    );
                }
            };
            let mut multipart = multipart;
            let mut uploads = Vec::new();

            loop {
                let field = match multipart.next_field().await {
                    Ok(Some(field)) => field,
                    Ok(None) => break,
                    Err(_) => {
                        return json_error(
                            StatusCode::BAD_REQUEST,
                            "Failed to read attachment upload body.",
                        );
                    }
                };

                if field.name() != Some("files") {
                    continue;
                }

                let file_name = field
                    .file_name()
                    .map(str::to_string)
                    .unwrap_or_else(|| "attachment".to_string());
                let mime_type = field.content_type().map(str::to_string);
                let bytes = match field.bytes().await {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        return json_error(
                            StatusCode::BAD_REQUEST,
                            "Failed to read attachment upload body.",
                        );
                    }
                };
                if bytes.is_empty() {
                    continue;
                }

                uploads.push(AttachmentUploadPayload {
                    name: file_name,
                    mime_type,
                    bytes: bytes.to_vec(),
                });
            }

            match save_uploaded_attachment_records(&state, &auth.profile_id, session_id, uploads)
                .await
            {
                Ok(stored) => {
                    if let Err(error) =
                        emit_attachments_updated(&state, &auth.profile_id, session_id).await
                    {
                        return json_error(error.status, &error.message);
                    }
                    let mut response = Json(json!({
                        "attachments": stored
                            .iter()
                            .map(attachment_payload_from_record)
                            .collect::<Vec<_>>()
                    }))
                    .into_response();
                    *response.status_mut() = StatusCode::CREATED;
                    response
                }
                Err(error) => json_error(error.status, &error.message),
            }
        }
        _ => json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed."),
    }
}

pub(crate) async fn handle_session_attachment_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    session_id: &str,
    attachment_id: &str,
) -> Response {
    if request.method() != Method::DELETE {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }
    if auth.role != UserRole::Admin {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    match delete_attachment_payload(&state, &auth.profile_id, session_id, attachment_id).await {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

pub(crate) async fn handle_session_search_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    session_id: &str,
) -> Response {
    if request.method() != Method::GET {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }

    let query = query_param_value(request.uri().query(), "query").unwrap_or_default();
    let cursor = query_param_value(request.uri().query(), "cursor");
    let limit = query_param_value(request.uri().query(), "limit")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(20);
    match search_session_turns_payload(
        &state,
        &auth.profile_id,
        session_id,
        &query,
        cursor.as_deref(),
        limit,
    )
    .await
    {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

pub(crate) async fn handle_session_turns_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    session_id: &str,
) -> Response {
    if request.method() != Method::GET {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }

    let before_turn_id =
        query_param_value(request.uri().query(), "beforeTurnId").unwrap_or_default();
    let limit = query_param_value(request.uri().query(), "limit")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(20);
    match session_older_turns_payload(&state, &auth.profile_id, session_id, &before_turn_id, limit)
        .await
    {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

pub(crate) async fn handle_session_turn_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    session_id: &str,
    turn_id: &str,
) -> Response {
    if request.method() != Method::GET {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }

    match session_turn_payload(&state, &auth.profile_id, session_id, turn_id).await {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

pub(crate) async fn handle_session_item_detail_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    session_id: &str,
    turn_id: &str,
    item_id: &str,
) -> Response {
    if request.method() != Method::GET {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }

    match session_item_detail_payload(&state, &auth.profile_id, session_id, turn_id, item_id).await
    {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

pub(crate) async fn handle_session_recovery_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    session_id: &str,
) -> Response {
    let app_error_response = |status: StatusCode, code: &str, message: &str| {
        let mut response = Json(json!({
            "code": code,
            "message": message,
            "status": status.as_u16()
        }))
        .into_response();
        *response.status_mut() = status;
        response
    };

    if request.method() != Method::POST {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }
    if auth.role != UserRole::Admin {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    let thread = match read_thread_payload(&state, &auth.profile_id, session_id, false).await {
        Ok(thread) => thread,
        Err(error) => return json_error(error.status, &error.message),
    };
    let Some(rollout_path) = resolve_rollout_path(&state, &auth.profile_id, session_id, &thread)
    else {
        return app_error_response(
            StatusCode::NOT_FOUND,
            "SESSION_ROLLOUT_NOT_FOUND",
            "No persisted rollout file was found for this session.",
        );
    };
    let rollout_buffer = match tokio_fs::read(&rollout_path).await {
        Ok(buffer) => buffer,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return app_error_response(
                StatusCode::NOT_FOUND,
                "SESSION_ROLLOUT_NOT_FOUND",
                "No persisted rollout file was found for this session.",
            );
        }
        Err(error) => {
            return json_error(StatusCode::BAD_GATEWAY, &error.to_string());
        }
    };
    let plan = inspect_rollout_recovery_content(&rollout_buffer);
    if !plan.info.available
        || plan.info.recoverable_lines == 0
        || plan.recovered_content.trim().is_empty()
    {
        return app_error_response(
            StatusCode::CONFLICT,
            "SESSION_ROLLOUT_NOT_RECOVERABLE",
            "This session history could not be recovered automatically.",
        );
    }

    let backup_path = PathBuf::from(format!("{}.bak-{}", rollout_path.display(), now_unix_ms()));
    if let Err(error) = tokio_fs::copy(&rollout_path, &backup_path).await {
        return json_error(StatusCode::BAD_GATEWAY, &error.to_string());
    }
    if let Err(error) = tokio_fs::write(&rollout_path, plan.recovered_content.as_bytes()).await {
        return json_error(StatusCode::BAD_GATEWAY, &error.to_string());
    }

    append_runtime_error_log(
        &state.config,
        "rust-gateway",
        "recovered corrupted rollout",
        json!({
            "threadId": session_id,
            "rolloutPath": rollout_path.display().to_string(),
            "backupPath": backup_path.display().to_string(),
            "recovery": plan.info
        }),
    );

    Json(json!({
        "ok": true,
        "sessionId": session_id,
        "backupPath": backup_path.display().to_string(),
        "recoveredAt": now_unix_ms(),
        "totalLines": plan.info.total_lines,
        "recoveredLines": plan.info.recoverable_lines,
        "skippedLines": plan.info.skipped_lines
    }))
    .into_response()
}
