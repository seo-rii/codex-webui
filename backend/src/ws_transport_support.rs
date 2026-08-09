use super::*;

tokio::task_local! {
    static ACTIVE_PROFILE_ID: String;
}

const SLOW_WS_LOG_INTERVAL: Duration = Duration::from_secs(10);
const WS_SOCKET_SEND_TIMEOUT: Duration = Duration::from_secs(10);

struct SlowWebSocketLogState {
    next_log_at: Instant,
    suppressed_count: u64,
    max_elapsed_ms: u128,
    max_response_bytes: usize,
}

static SLOW_WS_LOG_STATE: std::sync::OnceLock<std::sync::Mutex<SlowWebSocketLogState>> =
    std::sync::OnceLock::new();

fn rough_json_value_size(value: &Value) -> usize {
    match value {
        Value::Null => 4,
        Value::Bool(true) => 4,
        Value::Bool(false) => 5,
        Value::Number(number) => number.to_string().len(),
        Value::String(text) => text.len().saturating_add(2),
        Value::Array(items) => items
            .iter()
            .map(rough_json_value_size)
            .sum::<usize>()
            .saturating_add(items.len().saturating_add(1)),
        Value::Object(entries) => entries
            .iter()
            .map(|(key, item)| {
                key.len()
                    .saturating_add(rough_json_value_size(item))
                    .saturating_add(4)
            })
            .sum::<usize>()
            .saturating_add(entries.len().saturating_add(1)),
    }
}

fn rough_ws_response_size(message: &ServerEnvelope) -> usize {
    match message {
        ServerEnvelope::Response {
            id, result, error, ..
        } => 48usize
            .saturating_add(id.len())
            .saturating_add(
                result
                    .as_ref()
                    .map(rough_json_value_size)
                    .unwrap_or_default(),
            )
            .saturating_add(error.as_ref().map(String::len).unwrap_or_default()),
        ServerEnvelope::Event {
            session_id,
            profile_id,
            event,
        } => 48usize
            .saturating_add(session_id.len())
            .saturating_add(profile_id.len())
            .saturating_add(rough_json_value_size(event)),
        ServerEnvelope::TerminalEvent { terminal_id, event } => 56usize
            .saturating_add(terminal_id.len())
            .saturating_add(rough_json_value_size(event)),
        ServerEnvelope::GlobalEvent { event } => {
            32usize.saturating_add(rough_json_value_size(event))
        }
        ServerEnvelope::Ready { connection_id } => 40usize.saturating_add(connection_id.len()),
        ServerEnvelope::ResyncRequired { reason } => 40usize.saturating_add(reason.len()),
        ServerEnvelope::Pong { nonce } => {
            24usize.saturating_add(nonce.as_ref().map(String::len).unwrap_or_default())
        }
    }
}

pub(crate) async fn handle_ws(
    State(state): State<AppState>,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    jar: CookieJar,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !websocket_origin_allowed(&state.config, &headers, Some(peer_addr)) {
        let mut response =
            (StatusCode::FORBIDDEN, "WebSocket origin is not allowed.").into_response();
        apply_security_headers(response.headers_mut());
        return response;
    }

    let Some(auth) = auth_context_from_headers(&state.config, &jar, &headers) else {
        let mut response = (StatusCode::UNAUTHORIZED, "Authentication required.").into_response();
        apply_security_headers(response.headers_mut());
        return response;
    };
    let auth_token = strongest_auth_token(&state.config, &jar, &headers);

    let mut response = ws
        .max_message_size(WS_MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| websocket_session(socket, state, auth, auth_token))
        .into_response();
    apply_security_headers(response.headers_mut());
    response
}

async fn websocket_session(
    socket: WebSocket,
    state: AppState,
    auth: AuthContext,
    auth_token: Option<String>,
) {
    let (mut sender, mut receiver) = socket.split();
    let (out_tx, mut out_rx, mut invalidation_rx) =
        WsOutbound::new(WS_OUTBOUND_QUEUE_CAPACITY);
    let connection_id = Uuid::new_v4().to_string();
    let subscriptions: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let request_slots = Arc::new(tokio::sync::Semaphore::new(WS_MAX_CONCURRENT_REQUESTS));

    let writer_out_tx = out_tx.clone();
    let mut writer_invalidation_rx = out_tx.subscribe_invalidation();
    let writer = tokio::spawn(async move {
        loop {
            let message = tokio::select! {
                biased;
                changed = writer_invalidation_rx.changed() => {
                    if changed.is_ok() {
                        warn!(
                            reason = ?writer_invalidation_rx.borrow_and_update().clone(),
                            "closing invalidated websocket writer"
                        );
                    }
                    break;
                }
                message = out_rx.recv() => {
                    let Some(message) = message else {
                        break;
                    };
                    message
                }
            };
            let text = match serde_json::to_string(&message) {
                Ok(text) => text,
                Err(error) => {
                    error!("failed to serialize websocket message: {error:#}");
                    continue;
                }
            };

            match tokio::time::timeout(
                WS_SOCKET_SEND_TIMEOUT,
                sender.send(Message::Text(text.into())),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    writer_out_tx.invalidate(format!("websocket socket send failed: {error}"));
                    break;
                }
                Err(_) => {
                    warn!("closing websocket connection after stalled socket send");
                    writer_out_tx.invalidate("websocket socket send stalled".to_string());
                    break;
                }
            }
        }
    });

    let _ = queue_ws_envelope(
        &out_tx,
        ServerEnvelope::Ready {
            connection_id: connection_id.clone(),
        },
        "ready",
    );

    loop {
        let message = tokio::select! {
            biased;
            changed = invalidation_rx.changed() => {
                if changed.is_ok() {
                    warn!(
                        reason = ?invalidation_rx.borrow_and_update().clone(),
                        "closing invalidated websocket connection"
                    );
                }
                break;
            }
            message = receiver.next() => message,
        };
        let Some(Ok(message)) = message else {
            break;
        };
        if auth_token
            .as_deref()
            .is_some_and(|token| valid_auth_cookie_role(&state.config, token) != Some(auth.role))
        {
            warn!(
                role = user_role_label(auth.role),
                "closing websocket connection after its authentication session expired or was revoked"
            );
            break;
        }
        match message {
            Message::Text(text) => {
                let payload = match serde_json::from_str::<ClientEnvelope>(&text) {
                    Ok(payload) => payload,
                    Err(error) => {
                        let _ = queue_ws_envelope(
                            &out_tx,
                            ServerEnvelope::Response {
                                id: Uuid::new_v4().to_string(),
                                ok: false,
                                result: None,
                                error: Some(format!("Invalid websocket payload: {error}")),
                            },
                            "invalid-payload",
                        );
                        continue;
                    }
                };

                let request_permit = match &payload {
                    ClientEnvelope::Request { id, .. } => match tokio::time::timeout(
                        WS_REQUEST_SLOT_WAIT,
                        Arc::clone(&request_slots).acquire_owned(),
                    )
                    .await
                    {
                        Ok(Ok(permit)) => Some(permit),
                        Ok(Err(_)) => {
                            let _ = queue_ws_envelope(
                                &out_tx,
                                ServerEnvelope::Response {
                                    id: id.clone(),
                                    ok: false,
                                    result: None,
                                    error: Some("WebSocket request limiter is closed.".to_string()),
                                },
                                "connection-concurrency-closed",
                            );
                            continue;
                        }
                        Err(_) => {
                            let _ = queue_ws_envelope(
                                &out_tx,
                                ServerEnvelope::Response {
                                    id: id.clone(),
                                    ok: false,
                                    result: None,
                                    error: Some(
                                        "Too many concurrent websocket requests.".to_string(),
                                    ),
                                },
                                "connection-concurrency-limit",
                            );
                            continue;
                        }
                    },
                    ClientEnvelope::Ping { .. } => None,
                };
                let state = state.clone();
                let out_tx = out_tx.clone();
                let subscriptions = Arc::clone(&subscriptions);
                let auth = auth.clone();
                tokio::spawn(async move {
                    let _request_permit = request_permit;
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
                let _ = queue_ws_envelope(
                    &out_tx,
                    ServerEnvelope::Pong {
                        nonce: Some(URL_SAFE_NO_PAD.encode(payload)),
                    },
                    "socket-ping",
                );
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

pub(crate) fn queue_ws_envelope(
    out_tx: &WsOutbound,
    message: ServerEnvelope,
    context: &str,
) -> bool {
    if out_tx.invalidation_reason().is_some() {
        return false;
    }
    match out_tx.try_send(message) {
        Ok(()) => true,
        Err(mpsc::error::TrySendError::Full(_)) => {
            let reason = format!("outbound queue saturated while sending {context}");
            warn!(
                context = context,
                "invalidating websocket with saturated outbound queue"
            );
            out_tx.invalidate(reason);
            false
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            out_tx.invalidate(format!("outbound queue closed while sending {context}"));
            false
        }
    }
}

pub(crate) async fn try_acquire_profile_ws_request_slot(
    state: &AppState,
    profile_id: &str,
) -> Option<OwnedSemaphorePermit> {
    let slots = {
        let mut profile_slots = state.profile_request_slots.lock().await;
        profile_slots
            .entry(profile_id.to_string())
            .or_insert_with(|| Arc::new(Semaphore::new(WS_MAX_PROFILE_CONCURRENT_REQUESTS)))
            .clone()
    };
    tokio::time::timeout(WS_REQUEST_SLOT_WAIT, slots.acquire_owned())
        .await
        .ok()
        .and_then(Result::ok)
}

async fn handle_ws_message(
    state: &AppState,
    out_tx: &WsOutbound,
    subscriptions: &Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    auth: &AuthContext,
    payload: ClientEnvelope,
) -> Result<()> {
    match payload {
        ClientEnvelope::Ping { nonce } => {
            let _ = queue_ws_envelope(out_tx, ServerEnvelope::Pong { nonce }, "client-ping");
        }
        ClientEnvelope::Request { id, method, params } => {
            if let Err(error) = authorize_ws_method(&state.config, auth.role, &method, &params) {
                let _ = queue_ws_envelope(
                    out_tx,
                    ServerEnvelope::Response {
                        id,
                        ok: false,
                        result: None,
                        error: Some(redact_user_facing_error(&error.to_string())),
                    },
                    "authorization-error",
                );
                return Ok(());
            }

            let request_profile_id =
                match ws_request_default_profile_id(&state.config, auth, &params) {
                    Ok(profile_id) => profile_id,
                    Err(error) => {
                        let _ = queue_ws_envelope(
                            out_tx,
                            ServerEnvelope::Response {
                                id,
                                ok: false,
                                result: None,
                                error: Some(redact_user_facing_error(&error.to_string())),
                            },
                            "profile-resolution-error",
                        );
                        return Ok(());
                    }
                };

            let params_hash = request_params_hash(&params);
            let request_key = request_cache_key(&request_profile_id, &id, auth.role);
            let use_request_replay = ws_method_uses_request_replay(&method);

            if use_request_replay {
                match cached_response(state, &request_key, &method, &params_hash).await {
                    CachedResponseLookup::Hit(cached) => {
                        let _ = queue_ws_envelope(out_tx, cached, "cached-response");
                        return Ok(());
                    }
                    CachedResponseLookup::Conflict => {
                        let _ = queue_ws_envelope(
                            out_tx,
                            ServerEnvelope::Response {
                                id,
                                ok: false,
                                result: None,
                                error: Some(
                                    "WebSocket request id was already used with a different method or parameters."
                                        .to_string(),
                                ),
                            },
                            "request-cache-conflict",
                        );
                        return Ok(());
                    }
                    CachedResponseLookup::Miss => {}
                }

                match register_inflight_request(state, &request_key, &method, &params_hash, out_tx)
                    .await
                {
                    InflightRequestRegistration::Started => {}
                    InflightRequestRegistration::Joined => return Ok(()),
                    InflightRequestRegistration::Conflict => {
                        let _ = queue_ws_envelope(
                            out_tx,
                            ServerEnvelope::Response {
                                id,
                                ok: false,
                                result: None,
                                error: Some(
                                    "WebSocket request id is already in flight with a different method or parameters."
                                        .to_string(),
                                ),
                            },
                            "inflight-conflict",
                        );
                        return Ok(());
                    }
                    InflightRequestRegistration::Full => {
                        let _ = queue_ws_envelope(
                            out_tx,
                            ServerEnvelope::Response {
                                id,
                                ok: false,
                                result: None,
                                error: Some("Too many in-flight websocket requests.".to_string()),
                            },
                            "inflight-limit",
                        );
                        return Ok(());
                    }
                }
            }

            let Some(_profile_permit) =
                try_acquire_profile_ws_request_slot(state, &request_profile_id).await
            else {
                let message = ServerEnvelope::Response {
                    id,
                    ok: false,
                    result: None,
                    error: Some(
                        "Too many concurrent websocket requests for this profile.".to_string(),
                    ),
                };
                if use_request_replay {
                    resolve_inflight_request(state, &request_key, message).await;
                } else {
                    let _ = queue_ws_envelope(out_tx, message, "profile-concurrency-limit");
                }
                return Ok(());
            };

            let audit_target = summarize_audit_target(&params);
            let started_at = Instant::now();
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
                    error: Some(redact_user_facing_error(&error.to_string())),
                },
            };
            let elapsed = started_at.elapsed();
            let response_size = rough_ws_response_size(&message);
            if elapsed > Duration::from_millis(250) || response_size > 256 * 1024 {
                let now = Instant::now();
                let state = SLOW_WS_LOG_STATE.get_or_init(|| {
                    std::sync::Mutex::new(SlowWebSocketLogState {
                        next_log_at: Instant::now(),
                        suppressed_count: 0,
                        max_elapsed_ms: 0,
                        max_response_bytes: 0,
                    })
                });
                match state.lock() {
                    Ok(mut slow_log_state) => {
                        if now >= slow_log_state.next_log_at {
                            let suppressed_count = slow_log_state.suppressed_count;
                            let suppressed_max_elapsed_ms = slow_log_state.max_elapsed_ms;
                            let suppressed_max_response_bytes = slow_log_state.max_response_bytes;
                            slow_log_state.next_log_at = now + SLOW_WS_LOG_INTERVAL;
                            slow_log_state.suppressed_count = 0;
                            slow_log_state.max_elapsed_ms = 0;
                            slow_log_state.max_response_bytes = 0;
                            warn!(
                                method = %method,
                                profile_id = %request_profile_id,
                                elapsed_ms = elapsed.as_millis(),
                                response_bytes = response_size,
                                suppressed_count,
                                suppressed_max_elapsed_ms,
                                suppressed_max_response_bytes,
                                "slow websocket request"
                            );
                        } else {
                            slow_log_state.suppressed_count =
                                slow_log_state.suppressed_count.saturating_add(1);
                            slow_log_state.max_elapsed_ms =
                                slow_log_state.max_elapsed_ms.max(elapsed.as_millis());
                            slow_log_state.max_response_bytes =
                                slow_log_state.max_response_bytes.max(response_size);
                        }
                    }
                    Err(_) => {
                        warn!(
                            method = %method,
                            profile_id = %request_profile_id,
                            elapsed_ms = elapsed.as_millis(),
                            response_bytes = response_size,
                            "slow websocket request"
                        );
                    }
                }
            }
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
                                UserRole::Owner => "owner".to_string(),
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

            if use_request_replay {
                cache_response(state, &request_key, &method, &params_hash, message.clone()).await;
                resolve_inflight_request(state, &request_key, message).await;
            } else {
                let _ = queue_ws_envelope(out_tx, message, "response");
            }
        }
    }

    Ok(())
}
