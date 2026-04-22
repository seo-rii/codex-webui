use super::*;

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
