use super::*;

pub(crate) async fn handle_events_stream_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
) -> Response {
    if request.method() != Method::GET {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }

    match ensure_global_relay(&state, &auth.profile_id).await {
        Ok(relay) => sse_response(relay.subscribe(), json!({ "scope": "global" })),
        Err(error) => json_error(StatusCode::BAD_GATEWAY, &error.to_string()),
    }
}

pub(crate) async fn handle_session_stream_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    session_id: &str,
) -> Response {
    if request.method() != Method::GET {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }

    match ensure_stream_relay(&state, &auth.profile_id, session_id).await {
        Ok(relay) => sse_response(relay.subscribe(), json!({ "threadId": session_id })),
        Err(error) => json_error(StatusCode::BAD_GATEWAY, &error.to_string()),
    }
}

fn encode_sse_event(event: &str, payload: &Value) -> Bytes {
    let body = serde_json::to_string(payload).unwrap_or_else(|_| "null".to_string());
    Bytes::from(format!("event: {event}\ndata: {body}\n\n"))
}

fn sse_response(receiver: broadcast::Receiver<Value>, ready_payload: Value) -> Response {
    struct SseState {
        ready: Option<Bytes>,
        receiver: broadcast::Receiver<Value>,
        keepalive: Pin<Box<tokio::time::Sleep>>,
    }

    let stream = futures_util::stream::unfold(
        SseState {
            ready: Some(encode_sse_event("ready", &ready_payload)),
            receiver,
            keepalive: Box::pin(tokio::time::sleep(Duration::from_secs(15))),
        },
        |mut state| async move {
            if let Some(ready) = state.ready.take() {
                return Some((Ok::<Bytes, Infallible>(ready), state));
            }

            loop {
                tokio::select! {
                    _ = &mut state.keepalive => {
                        state.keepalive.as_mut().reset(tokio::time::Instant::now() + Duration::from_secs(15));
                        return Some((Ok(Bytes::from_static(b": ping\n\n")), state));
                    }
                    result = state.receiver.recv() => {
                        match result {
                            Ok(event) => {
                                state.keepalive.as_mut().reset(tokio::time::Instant::now() + Duration::from_secs(15));
                                return Some((Ok(encode_sse_event("message", &event)), state));
                            }
                            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                                warn!("sse relay lagged: skipped {skipped} messages");
                            }
                            Err(broadcast::error::RecvError::Closed) => return None,
                        }
                    }
                }
            }
        },
    );

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CACHE_CONTROL, "no-cache, no-transform")
        .header(header::CONNECTION, "keep-alive")
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header("x-accel-buffering", "no")
        .body(Body::from_stream(stream))
        .unwrap_or_else(|error| json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()))
}
