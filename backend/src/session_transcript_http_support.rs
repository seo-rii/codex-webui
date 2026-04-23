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
