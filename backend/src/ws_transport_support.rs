use super::*;

tokio::task_local! {
    static ACTIVE_PROFILE_ID: String;
}

pub(crate) async fn handle_ws(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !websocket_origin_allowed(&state.config, &headers) {
        return (StatusCode::FORBIDDEN, "WebSocket origin is not allowed.").into_response();
    }

    let Some(auth) = auth_context(&state.config, &jar) else {
        return (StatusCode::UNAUTHORIZED, "Authentication required.").into_response();
    };

    ws.on_upgrade(move |socket| websocket_session(socket, state, auth))
        .into_response()
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
            let params_hash = request_params_hash(&params);
            let request_key =
                request_cache_key(&auth.profile_id, &id, auth.role, &method, &params_hash);

            if let Some(cached) = cached_response(state, &request_key).await {
                let _ = out_tx.send(cached);
                return Ok(());
            }

            if !register_inflight_request(state, &request_key, out_tx).await {
                return Ok(());
            }

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
                    error: Some(error.to_string()),
                },
            };
            let elapsed = started_at.elapsed();
            let response_size = serde_json::to_vec(&message)
                .map(|bytes| bytes.len())
                .unwrap_or_default();
            if elapsed > Duration::from_millis(250) || response_size > 256 * 1024 {
                warn!(
                    method = %method,
                    profile_id = %auth.profile_id,
                    elapsed_ms = elapsed.as_millis(),
                    response_bytes = response_size,
                    "slow websocket request"
                );
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
