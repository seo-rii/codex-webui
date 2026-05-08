use super::*;

tokio::task_local! {
    static ACTIVE_PROFILE_ID: String;
}

const SLOW_WS_LOG_INTERVAL: Duration = Duration::from_secs(10);

struct SlowWebSocketLogState {
    next_log_at: Instant,
    suppressed_count: u64,
    max_elapsed_ms: u128,
    max_response_bytes: usize,
}

static SLOW_WS_LOG_STATE: std::sync::OnceLock<std::sync::Mutex<SlowWebSocketLogState>> =
    std::sync::OnceLock::new();

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

    let mut response = ws
        .max_message_size(WS_MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| websocket_session(socket, state, auth))
        .into_response();
    apply_security_headers(response.headers_mut());
    response
}

async fn websocket_session(socket: WebSocket, state: AppState, auth: AuthContext) {
    let (mut sender, mut receiver) = socket.split();
    let (out_tx, mut out_rx) = mpsc::channel::<ServerEnvelope>(WS_OUTBOUND_QUEUE_CAPACITY);
    let connection_id = Uuid::new_v4().to_string();
    let subscriptions: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let request_slots = Arc::new(tokio::sync::Semaphore::new(WS_MAX_CONCURRENT_REQUESTS));

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

    let _ = out_tx
        .send(ServerEnvelope::Ready {
            connection_id: connection_id.clone(),
        })
        .await;

    while let Some(Ok(message)) = receiver.next().await {
        match message {
            Message::Text(text) => {
                let payload = match serde_json::from_str::<ClientEnvelope>(&text) {
                    Ok(payload) => payload,
                    Err(error) => {
                        let _ = out_tx
                            .send(ServerEnvelope::Response {
                                id: Uuid::new_v4().to_string(),
                                ok: false,
                                result: None,
                                error: Some(format!("Invalid websocket payload: {error}")),
                            })
                            .await;
                        continue;
                    }
                };

                let request_permit = match &payload {
                    ClientEnvelope::Request { id, .. } => {
                        match Arc::clone(&request_slots).try_acquire_owned() {
                            Ok(permit) => Some(permit),
                            Err(_) => {
                                let _ = out_tx
                                    .send(ServerEnvelope::Response {
                                        id: id.clone(),
                                        ok: false,
                                        result: None,
                                        error: Some(
                                            "Too many concurrent websocket requests.".to_string(),
                                        ),
                                    })
                                    .await;
                                continue;
                            }
                        }
                    }
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
                let _ = out_tx
                    .send(ServerEnvelope::Pong {
                        nonce: Some(URL_SAFE_NO_PAD.encode(payload)),
                    })
                    .await;
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
    slots.try_acquire_owned().ok()
}

async fn handle_ws_message(
    state: &AppState,
    out_tx: &mpsc::Sender<ServerEnvelope>,
    subscriptions: &Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    auth: &AuthContext,
    payload: ClientEnvelope,
) -> Result<()> {
    match payload {
        ClientEnvelope::Ping { nonce } => {
            let _ = out_tx.send(ServerEnvelope::Pong { nonce }).await;
        }
        ClientEnvelope::Request { id, method, params } => {
            if let Err(error) = authorize_ws_method(&state.config, auth.role, &method, &params) {
                let _ = out_tx
                    .send(ServerEnvelope::Response {
                        id,
                        ok: false,
                        result: None,
                        error: Some(redact_user_facing_error(&error.to_string())),
                    })
                    .await;
                return Ok(());
            }

            let params_hash = request_params_hash(&params);
            let request_key = request_cache_key(&auth.profile_id, &id, auth.role);
            let use_request_replay = ws_method_uses_request_replay(&method);

            if use_request_replay {
                match cached_response(state, &request_key, &method, &params_hash).await {
                    CachedResponseLookup::Hit(cached) => {
                        let _ = out_tx.send(cached).await;
                        return Ok(());
                    }
                    CachedResponseLookup::Conflict => {
                        let _ = out_tx
                            .send(ServerEnvelope::Response {
                                id,
                                ok: false,
                                result: None,
                                error: Some(
                                    "WebSocket request id was already used with a different method or parameters."
                                        .to_string(),
                                ),
                            })
                            .await;
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
                        let _ = out_tx
                            .send(ServerEnvelope::Response {
                                id,
                                ok: false,
                                result: None,
                                error: Some(
                                    "WebSocket request id is already in flight with a different method or parameters."
                                        .to_string(),
                                ),
                            })
                            .await;
                        return Ok(());
                    }
                    InflightRequestRegistration::Full => {
                        let _ = out_tx
                            .send(ServerEnvelope::Response {
                                id,
                                ok: false,
                                result: None,
                                error: Some("Too many in-flight websocket requests.".to_string()),
                            })
                            .await;
                        return Ok(());
                    }
                }
            }

            let Some(_profile_permit) =
                try_acquire_profile_ws_request_slot(state, &auth.profile_id).await
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
                    let _ = out_tx.send(message).await;
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
            let response_size = serde_json::to_vec(&message)
                .map(|bytes| bytes.len())
                .unwrap_or_default();
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
                                profile_id = %auth.profile_id,
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
                            profile_id = %auth.profile_id,
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
                let _ = out_tx.send(message).await;
            }
        }
    }

    Ok(())
}
