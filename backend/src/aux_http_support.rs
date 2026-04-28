use super::*;

pub(crate) async fn handle_editor_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
) -> Response {
    let method = request.method().clone();
    let result = match method {
        Method::GET => {
            let file_path =
                query_param_value(request.uri().query(), "filePath").unwrap_or_default();
            read_editable_file_payload(&state, &auth.profile_id, &file_path).await
        }
        Method::PUT => {
            if !role_has_admin_access(auth.role) {
                return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
            }

            match read_json_body(request, LARGE_JSON_BODY_LIMIT, "editor request body").await {
                Ok(payload) => {
                    let file_path = payload
                        .get("filePath")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let content = payload
                        .get("content")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    write_editable_file_payload(&state, &auth.profile_id, file_path, content).await
                }
                Err(error) => Err(error),
            }
        }
        _ => return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed."),
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

pub(crate) async fn handle_catalog_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
) -> Response {
    if request.method() != Method::GET {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }

    match get_catalog_payload(&state, &auth.profile_id).await {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

pub(crate) async fn handle_notifications_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
) -> Response {
    let result = match request.method() {
        &Method::GET => {
            let limit = query_param_value(request.uri().query(), "limit")
                .and_then(|value| value.parse::<usize>().ok())
                .map(|value| value.clamp(1, 200))
                .unwrap_or(DEFAULT_NOTIFICATION_LIMIT);
            get_notifications_payload(&state, &auth.profile_id, limit).await
        }
        &Method::PATCH => {
            if !role_has_admin_access(auth.role) {
                return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
            }
            match read_json_body(request, SMALL_JSON_BODY_LIMIT, "notifications request body").await
            {
                Ok(payload) => {
                    let ids = payload.get("ids").and_then(Value::as_array).map(|values| {
                        values
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    });
                    mark_notifications_read_payload(&state, &auth.profile_id, ids).await
                }
                Err(error) => Err(error),
            }
        }
        &Method::DELETE => {
            if !role_has_admin_access(auth.role) {
                return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
            }
            clear_notifications_payload(&state, &auth.profile_id).await
        }
        _ => return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed."),
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

pub(crate) async fn handle_notification_settings_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
) -> Response {
    if request.method() != Method::PATCH {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }
    if !role_has_admin_access(auth.role) {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    let result = match read_json_body(
        request,
        SMALL_JSON_BODY_LIMIT,
        "notification settings request body",
    )
    .await
    {
        Ok(payload) => {
            update_notification_settings_payload(&state, &auth.profile_id, payload).await
        }
        Err(error) => Err(error),
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

pub(crate) async fn handle_session_filters_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
) -> Response {
    if !role_has_admin_access(auth.role) {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    let result = match request.method() {
        &Method::POST => {
            match read_json_body(
                request,
                SMALL_JSON_BODY_LIMIT,
                "session filters request body",
            )
            .await
            {
                Ok(payload) => {
                    save_session_filter_payload(
                        &state,
                        &auth.profile_id,
                        payload.get("filter").cloned().unwrap_or_else(|| json!({})),
                    )
                    .await
                }
                Err(error) => Err(error),
            }
        }
        &Method::DELETE => {
            let filter_id =
                query_param_value(request.uri().query(), "filterId").unwrap_or_default();
            delete_session_filter_payload(&state, &auth.profile_id, &filter_id).await
        }
        _ => return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed."),
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

pub(crate) async fn handle_prompt_presets_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
) -> Response {
    if !role_has_admin_access(auth.role) {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    let result = match request.method() {
        &Method::POST => {
            match read_json_body(
                request,
                SMALL_JSON_BODY_LIMIT,
                "prompt presets request body",
            )
            .await
            {
                Ok(payload) => {
                    save_prompt_preset_payload(
                        &state,
                        &auth.profile_id,
                        payload.get("preset").cloned().unwrap_or_else(|| json!({})),
                    )
                    .await
                }
                Err(error) => Err(error),
            }
        }
        &Method::DELETE => {
            let preset_id =
                query_param_value(request.uri().query(), "presetId").unwrap_or_default();
            delete_prompt_preset_payload(&state, &auth.profile_id, &preset_id).await
        }
        _ => return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed."),
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

pub(crate) async fn handle_automations_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    route_path: &str,
) -> Response {
    if !role_has_admin_access(auth.role) {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    let result = if route_path == "/api/automations" {
        match request.method() {
            &Method::POST => {
                match read_json_body(request, SMALL_JSON_BODY_LIMIT, "automations request body")
                    .await
                {
                    Ok(payload) => {
                        save_automation_payload(
                            &state,
                            &auth.profile_id,
                            payload
                                .get("automation")
                                .cloned()
                                .unwrap_or_else(|| json!({})),
                        )
                        .await
                    }
                    Err(error) => Err(error),
                }
            }
            &Method::DELETE => {
                let automation_id =
                    query_param_value(request.uri().query(), "automationId").unwrap_or_default();
                delete_automation_payload(&state, &auth.profile_id, &automation_id).await
            }
            _ => return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed."),
        }
    } else if request.method() == Method::POST && route_path.ends_with("/run") {
        let automation_id = route_path
            .strip_prefix("/api/automations/")
            .and_then(|suffix| suffix.strip_suffix("/run"))
            .unwrap_or_default()
            .trim()
            .to_string();
        match read_json_body(
            request,
            SMALL_JSON_BODY_LIMIT,
            "automation run request body",
        )
        .await
        {
            Ok(payload) => {
                let trigger = if payload.get("trigger").and_then(Value::as_str) == Some("schedule")
                {
                    "schedule"
                } else {
                    "manual"
                };
                run_automation_payload(&state, &auth.profile_id, &automation_id, trigger).await
            }
            Err(error) => Err(error),
        }
    } else {
        return json_error(StatusCode::NOT_FOUND, "Not found.");
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

pub(crate) async fn handle_arena_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
) -> Response {
    let result = match request.method() {
        &Method::GET => list_arena_runs_payload(&state, &auth.profile_id).await,
        &Method::POST => {
            if !role_has_admin_access(auth.role) {
                return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
            }
            match read_json_body(request, LARGE_JSON_BODY_LIMIT, "arena request body").await {
                Ok(payload) => {
                    start_arena_run_payload(
                        &state,
                        &auth.profile_id,
                        payload
                            .get("prompt")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                        payload.get("contestants").unwrap_or(&Value::Null),
                        payload.get("preferences").unwrap_or(&Value::Null),
                    )
                    .await
                }
                Err(error) => Err(error),
            }
        }
        _ => return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed."),
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}
