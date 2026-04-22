use super::*;

#[derive(Debug, Deserialize)]
struct LoginPayload {
    password: Option<String>,
    #[serde(alias = "hcaptchaToken", alias = "hcaptcha_token")]
    hcaptcha_token: Option<String>,
}

pub(crate) async fn handle_ws(
    State(state): State<AppState>,
    jar: CookieJar,
    ws: WebSocketUpgrade,
) -> Response {
    let Some(auth) = auth_context(&state.config, &jar) else {
        return (StatusCode::UNAUTHORIZED, "Authentication required.").into_response();
    };

    ws.on_upgrade(move |socket| websocket_session(socket, state, auth))
        .into_response()
}

pub(crate) async fn handle_auth_http(
    state: AppState,
    jar: CookieJar,
    method: Method,
    route_path: String,
    headers: HeaderMap,
    request: Request,
) -> Response {
    let origin = extract_origin(&headers);
    let cors_origin = allowed_cors_origin(&state.config, &origin);
    let requested_headers = headers
        .get("access-control-request-headers")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    if method == Method::OPTIONS {
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

    let result = match (method, route_path.as_str()) {
        (Method::POST, "/api/auth/login") => auth_login(state.clone(), jar, headers, request).await,
        (Method::POST, "/api/auth/logout") => Ok(auth_logout(jar)),
        (Method::POST, "/api/auth/profile") => {
            let Some(auth) = auth_context(&state.config, &jar) else {
                return json_error(StatusCode::UNAUTHORIZED, "Authentication required.");
            };
            select_profile(state.config.clone(), jar, headers, request, auth).await
        }
        (Method::GET, "/api/auth/session") => {
            let auth = auth_context(&state.config, &jar);
            let active_profile_id = auth
                .as_ref()
                .map(|context| context.profile_id.as_str())
                .unwrap_or(&state.config.default_profile_id);
            Ok((
                jar,
                Json(json!({
                    "authenticated": auth.is_some(),
                    "activeProfileId": active_profile_id,
                    "role": auth.map(|context| match context.role {
                        UserRole::Admin => "admin",
                        UserRole::Viewer => "viewer",
                    }),
                    "hcaptcha": {
                        "enabled": state.config.hcaptcha_enabled(),
                        "siteKey": state.config.hcaptcha_site_key(),
                    }
                })),
            )
                .into_response())
        }
        _ => Ok((StatusCode::NOT_FOUND, "Not found").into_response()),
    };

    let mut response = match result {
        Ok(response) => response,
        Err(error_message) => json_error(StatusCode::UNAUTHORIZED, &error_message),
    };

    if let Some(origin_value) = cors_origin {
        apply_cors_headers(
            response.headers_mut(),
            &origin_value,
            requested_headers.as_deref(),
        );
    }

    response
}

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

async fn auth_login(
    state: AppState,
    jar: CookieJar,
    headers: HeaderMap,
    request: Request,
) -> std::result::Result<Response, String> {
    let secure_request = request_is_secure(&headers);
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .map_err(|_| "Invalid request body.".to_string())?;
    let payload: LoginPayload = serde_json::from_slice(&body).unwrap_or(LoginPayload {
        password: None,
        hcaptcha_token: None,
    });
    let password = payload.password.unwrap_or_default();
    let identifier = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown")
        .to_string();

    if !check_rate_limit(&state, &identifier).await {
        return Ok(json_error(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many login attempts. Try again later.",
        ));
    }

    if state.config.hcaptcha_enabled() {
        let Some(hcaptcha_secret_key) = state.config.hcaptcha_secret_key() else {
            return Ok(json_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "hCaptcha is not fully configured.",
            ));
        };
        let Some(hcaptcha_token) = payload
            .hcaptcha_token
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(json_error(
                StatusCode::BAD_REQUEST,
                "Complete the hCaptcha challenge before signing in.",
            ));
        };

        let mut verification_payload = vec![
            ("secret", hcaptcha_secret_key.to_string()),
            ("response", hcaptcha_token.to_string()),
        ];
        if identifier != "unknown" {
            verification_payload.push(("remoteip", identifier.clone()));
        }

        let verification_response = state
            .http
            .post("https://api.hcaptcha.com/siteverify")
            .form(&verification_payload)
            .send()
            .await
            .map_err(|error| {
                tracing::warn!("failed to verify hcaptcha: {error}");
                "Failed to verify hCaptcha."
            })?;

        if !verification_response.status().is_success() {
            tracing::warn!(
                status = %verification_response.status(),
                "hcaptcha verification request returned a non-success status"
            );
            return Ok(json_error(
                StatusCode::BAD_GATEWAY,
                "Failed to verify hCaptcha.",
            ));
        }

        let verification_result: Value = verification_response.json().await.map_err(|error| {
            tracing::warn!("failed to parse hcaptcha verification response: {error}");
            "Failed to verify hCaptcha."
        })?;
        let verification_ok = verification_result
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        if !verification_ok {
            record_login_failure(&state, &identifier).await;
            let _ = append_audit_log(
                &state.config,
                AuditLogEntry {
                    id: Uuid::new_v4().to_string(),
                    at: now_unix_ms(),
                    role: "anonymous".to_string(),
                    method: "auth/login".to_string(),
                    target: None,
                    ok: false,
                    error: Some("Failed hCaptcha verification.".to_string()),
                },
            )
            .await;
            return Ok(json_error(
                StatusCode::UNAUTHORIZED,
                "Complete the hCaptcha challenge before signing in.",
            ));
        }
    }

    let Some(role) =
        authenticate_role(&state.config, &password).map_err(|error| error.to_string())?
    else {
        record_login_failure(&state, &identifier).await;
        let _ = append_audit_log(
            &state.config,
            AuditLogEntry {
                id: Uuid::new_v4().to_string(),
                at: now_unix_ms(),
                role: "anonymous".to_string(),
                method: "auth/login".to_string(),
                target: None,
                ok: false,
                error: Some("Invalid password.".to_string()),
            },
        )
        .await;
        return Ok(json_error(StatusCode::UNAUTHORIZED, "Invalid password."));
    };

    clear_login_failures(&state, &identifier).await;
    let next_jar = issue_auth_cookie(&state.config, jar, secure_request, role)
        .map_err(|error| error.to_string())?;
    let _ = append_audit_log(
        &state.config,
        AuditLogEntry {
            id: Uuid::new_v4().to_string(),
            at: now_unix_ms(),
            role: match role {
                UserRole::Admin => "admin".to_string(),
                UserRole::Viewer => "viewer".to_string(),
            },
            method: "auth/login".to_string(),
            target: None,
            ok: true,
            error: None,
        },
    )
    .await;
    Ok((
        next_jar,
        Json(json!({
            "ok": true,
            "role": match role {
                UserRole::Admin => "admin",
                UserRole::Viewer => "viewer",
            }
        })),
    )
        .into_response())
}

fn auth_logout(jar: CookieJar) -> Response {
    let mut cookie = Cookie::new(AUTH_COOKIE, "");
    cookie.set_path("/");
    cookie.set_max_age(CookieDuration::seconds(0));
    let mut profile_cookie = Cookie::new(PROFILE_COOKIE, "");
    profile_cookie.set_path("/");
    profile_cookie.set_max_age(CookieDuration::seconds(0));
    (
        jar.remove(cookie).remove(profile_cookie),
        Json(json!({ "ok": true })),
    )
        .into_response()
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

async fn websocket_session(socket: WebSocket, state: AppState, auth: AuthContext) {
    let (mut sender, mut receiver) = socket.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<ServerEnvelope>();
    let connection_id = Uuid::new_v4().to_string();
    let subscriptions: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let writer = tokio::spawn(async move {
        while let Some(message) = out_rx.recv().await {
            let text = match serde_json::to_string(&message) {
                Ok(text) => text,
                Err(error) => {
                    error!("failed to serialize websocket message: {error:#}");
                    continue;
                }
            };

            if sender.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    let _ = out_tx.send(ServerEnvelope::Ready {
        connection_id: connection_id.clone(),
    });

    while let Some(Ok(message)) = receiver.next().await {
        match message {
            Message::Text(text) => {
                let payload = match serde_json::from_str::<ClientEnvelope>(&text) {
                    Ok(payload) => payload,
                    Err(error) => {
                        let _ = out_tx.send(ServerEnvelope::Response {
                            id: Uuid::new_v4().to_string(),
                            ok: false,
                            result: None,
                            error: Some(format!("Invalid websocket payload: {error}")),
                        });
                        continue;
                    }
                };

                let state = state.clone();
                let out_tx = out_tx.clone();
                let subscriptions = Arc::clone(&subscriptions);
                let auth = auth.clone();
                tokio::spawn(async move {
                    ACTIVE_PROFILE_ID
                        .scope(auth.profile_id.clone(), async move {
                            if let Err(error) =
                                handle_ws_message(&state, &out_tx, &subscriptions, &auth, payload)
                                    .await
                            {
                                error!("websocket request failed: {error:#}");
                            }
                        })
                        .await;
                });
            }
            Message::Ping(payload) => {
                let _ = out_tx.send(ServerEnvelope::Pong {
                    nonce: Some(URL_SAFE_NO_PAD.encode(payload)),
                });
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    let mut handles = subscriptions.lock().await;
    for (_, handle) in handles.drain() {
        handle.abort();
    }
    writer.abort();
}

async fn handle_ws_message(
    state: &AppState,
    out_tx: &mpsc::UnboundedSender<ServerEnvelope>,
    subscriptions: &Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    auth: &AuthContext,
    payload: ClientEnvelope,
) -> Result<()> {
    match payload {
        ClientEnvelope::Ping { nonce } => {
            let _ = out_tx.send(ServerEnvelope::Pong { nonce });
        }
        ClientEnvelope::Request { id, method, params } => {
            let request_key = request_cache_key(&auth.profile_id, &id);

            if let Some(cached) = cached_response(state, &request_key).await {
                let _ = out_tx.send(cached);
                return Ok(());
            }

            if !register_inflight_request(state, &request_key, out_tx).await {
                return Ok(());
            }

            let audit_target = summarize_audit_target(&params);
            let message = match execute_ws_method(
                state,
                out_tx,
                subscriptions,
                auth,
                &method,
                params,
            )
            .await
            {
                Ok(result) => ServerEnvelope::Response {
                    id: id.clone(),
                    ok: true,
                    result: Some(result),
                    error: None,
                },
                Err(error) => ServerEnvelope::Response {
                    id: id.clone(),
                    ok: false,
                    result: None,
                    error: Some(error.to_string()),
                },
            };
            if should_audit_ws_method(&method) {
                let log_config = state.config.clone();
                let role = auth.role;
                let method_name = method.clone();
                let target = audit_target;
                let error = match &message {
                    ServerEnvelope::Response { error, .. } => error.clone(),
                    _ => None,
                };
                let ok = matches!(&message, ServerEnvelope::Response { ok: true, .. });
                tokio::spawn(async move {
                    let _ = append_audit_log(
                        &log_config,
                        AuditLogEntry {
                            id: Uuid::new_v4().to_string(),
                            at: now_unix_ms(),
                            role: match role {
                                UserRole::Admin => "admin".to_string(),
                                UserRole::Viewer => "viewer".to_string(),
                            },
                            method: method_name,
                            target,
                            ok,
                            error,
                        },
                    )
                    .await;
                });
            }

            cache_response(state, &request_key, message.clone()).await;
            resolve_inflight_request(state, &request_key, message).await;
        }
    }

    Ok(())
}

fn should_audit_ws_method(method: &str) -> bool {
    !matches!(
        method,
        "config/get"
            | "runtime/status"
            | "runtime/checkUpdate"
            | "runtime/quota"
            | "catalog/get"
            | "directories/browse"
            | "editor/file/get"
            | "sessions/list"
            | "sessions/search"
            | "session/get"
            | "session/draft/get"
            | "session/queue/get"
            | "session/olderTurns/get"
            | "session/turn/get"
            | "session/itemDetail/get"
            | "notifications/list"
            | "account/get"
            | "arena/list"
            | "git/repositories/list"
            | "git/status"
            | "git/github/pulls"
            | "git/github/pull"
            | "git/commit/diff"
            | "git/file/get"
            | "git/file/resolve"
            | "git/worktrees/list"
            | "terminal/list"
            | "terminal/read"
            | "session/subscribe"
            | "session/unsubscribe"
            | "events/subscribe"
            | "events/unsubscribe"
            | "terminal/subscribe"
            | "terminal/unsubscribe"
    )
}

fn summarize_audit_target(params: &Value) -> Option<String> {
    for key in [
        "sessionId",
        "threadId",
        "terminalId",
        "queueId",
        "turnId",
        "presetId",
        "filterId",
        "repoPath",
        "filePath",
    ] {
        if let Some(value) = params
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            return Some(value.trim().to_string());
        }
    }
    None
}
