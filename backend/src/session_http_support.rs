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
