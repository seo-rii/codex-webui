use super::*;

const RELAY_SEND_TIMEOUT: Duration = Duration::from_secs(3);

async fn send_relay_envelope(
    out_tx: &mpsc::Sender<ServerEnvelope>,
    message: ServerEnvelope,
    context: &str,
) -> bool {
    match tokio::time::timeout(RELAY_SEND_TIMEOUT, out_tx.send(message)).await {
        Ok(Ok(())) => true,
        Ok(Err(_)) => false,
        Err(_) => {
            warn!(
                context = context,
                "dropping websocket subscription after stalled relay send"
            );
            false
        }
    }
}

pub(crate) async fn subscribe_session(
    state: AppState,
    out_tx: mpsc::Sender<ServerEnvelope>,
    subscriptions: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    profile_id: String,
    session_id: String,
    role: UserRole,
) -> Result<()> {
    let relay = ensure_stream_relay(&state, &profile_id, &session_id).await?;
    let mut receiver = relay.subscribe();
    let session_key = session_id.clone();
    let profile_key = profile_id.clone();
    let cleanup_state = state.clone();
    let stream_out_tx = out_tx.clone();
    let handle = tokio::spawn(async move {
        let mut pending_delta_event: Option<Value> = None;
        let mut pending_delta_key = String::new();
        loop {
            match tokio::time::timeout(Duration::from_millis(50), receiver.recv()).await {
                Err(_) => {
                    if let Some(event) = pending_delta_event.take() {
                        if !send_relay_envelope(
                            &stream_out_tx,
                            ServerEnvelope::Event {
                                session_id: session_key.clone(),
                                event,
                            },
                            "session-subscription-output-delta",
                        )
                        .await
                        {
                            break;
                        }
                    }
                    pending_delta_key.clear();
                }
                Ok(Ok(event)) => {
                    let Some(mut event) = filter_session_event_for_role(role, event) else {
                        continue;
                    };

                    if event.get("method").and_then(Value::as_str)
                        == Some("item/commandExecution/outputDelta")
                    {
                        let params = event.get("params").and_then(Value::as_object);
                        let turn_id = params
                            .and_then(|params| params.get("turnId"))
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let item_id = params
                            .and_then(|params| params.get("itemId"))
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let delta = params
                            .and_then(|params| params.get("delta"))
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        if delta.is_empty() {
                            continue;
                        }
                        if turn_id.is_empty() || item_id.is_empty() {
                            if let Some(pending_event) = pending_delta_event.take() {
                                if !send_relay_envelope(
                                    &stream_out_tx,
                                    ServerEnvelope::Event {
                                        session_id: session_key.clone(),
                                        event: pending_event,
                                    },
                                    "session-subscription-output-delta",
                                )
                                .await
                                {
                                    break;
                                }
                            }
                            pending_delta_key.clear();
                        } else {
                            let next_key = format!("{turn_id}\u{0}{item_id}");
                            if pending_delta_event.is_some() && pending_delta_key == next_key {
                                if let Some(params) = pending_delta_event
                                    .as_mut()
                                    .and_then(|pending| pending.get_mut("params"))
                                    .and_then(Value::as_object_mut)
                                {
                                    let mut next_delta = params
                                        .get("delta")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                        .to_string();
                                    next_delta.push_str(&delta);
                                    params.insert(
                                        "deltaLength".to_string(),
                                        json!(next_delta.chars().count()),
                                    );
                                    params.insert("delta".to_string(), Value::String(next_delta));
                                }
                            } else {
                                if let Some(pending_event) = pending_delta_event.take() {
                                    if !send_relay_envelope(
                                        &stream_out_tx,
                                        ServerEnvelope::Event {
                                            session_id: session_key.clone(),
                                            event: pending_event,
                                        },
                                        "session-subscription-output-delta",
                                    )
                                    .await
                                    {
                                        break;
                                    }
                                }
                                pending_delta_key = next_key;
                                event.get_mut("params").and_then(Value::as_object_mut).map(
                                    |params| {
                                        params.insert(
                                            "deltaLength".to_string(),
                                            json!(delta.chars().count()),
                                        );
                                    },
                                );
                                pending_delta_event = Some(event);
                            }
                            if pending_delta_event
                                .as_ref()
                                .and_then(|pending| pending.get("params"))
                                .and_then(|params| params.get("delta"))
                                .and_then(Value::as_str)
                                .is_some_and(|value| value.len() >= 16 * 1024)
                            {
                                if let Some(pending_event) = pending_delta_event.take() {
                                    if !send_relay_envelope(
                                        &stream_out_tx,
                                        ServerEnvelope::Event {
                                            session_id: session_key.clone(),
                                            event: pending_event,
                                        },
                                        "session-subscription-output-delta",
                                    )
                                    .await
                                    {
                                        break;
                                    }
                                }
                                pending_delta_key.clear();
                            }
                            continue;
                        }
                    }

                    if let Some(pending_event) = pending_delta_event.take() {
                        if !send_relay_envelope(
                            &stream_out_tx,
                            ServerEnvelope::Event {
                                session_id: session_key.clone(),
                                event: pending_event,
                            },
                            "session-subscription-output-delta",
                        )
                        .await
                        {
                            break;
                        }
                    }
                    pending_delta_key.clear();
                    if !send_relay_envelope(
                        &stream_out_tx,
                        ServerEnvelope::Event {
                            session_id: session_key.clone(),
                            event,
                        },
                        "session-subscription",
                    )
                    .await
                    {
                        break;
                    }
                }
                Ok(Err(broadcast::error::RecvError::Lagged(skipped))) => {
                    warn!("websocket lagged on session {session_key}: skipped {skipped} messages");
                }
                Ok(Err(broadcast::error::RecvError::Closed)) => break,
            }
        }
        if let Some(event) = pending_delta_event.take() {
            let _ = send_relay_envelope(
                &stream_out_tx,
                ServerEnvelope::Event {
                    session_id: session_key.clone(),
                    event,
                },
                "session-subscription-output-delta",
            )
            .await;
        }
        drop(receiver);
        prune_unused_session_relay(&cleanup_state, &profile_key, &session_key).await;
    });

    let mut current = subscriptions.lock().await;
    if let Some(existing) = current.insert(session_relay_key(&profile_id, &session_id), handle) {
        existing.abort();
    }

    if let Ok(queue) = get_session_queue_payload(&state, &profile_id, &session_id).await {
        let queue = if role_has_admin_access(role) {
            queue
        } else {
            redacted_queue_payload(&queue)
        };
        let _ = queue_ws_envelope(
            &out_tx,
            ServerEnvelope::Event {
                session_id: session_id.clone(),
                event: json!({
                    "kind": "notification",
                    "method": "codex-webui/queueUpdated",
                    "params": {
                        "queue": queue
                    }
                }),
            },
            "session-subscribe-initial-queue",
        );
    }

    Ok(())
}

pub(crate) async fn subscribe_terminal(
    state: AppState,
    out_tx: mpsc::Sender<ServerEnvelope>,
    subscriptions: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    terminal_id: String,
) -> Result<()> {
    let terminal = get_terminal_session(&state, &terminal_id).await?;
    let mut receiver = terminal.relay.subscribe();
    let relay_key = format!("{TERMINAL_RELAY_PREFIX}{terminal_id}");
    let terminal_key = terminal_id.clone();
    let handle = tokio::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    if !send_relay_envelope(
                        &out_tx,
                        ServerEnvelope::TerminalEvent {
                            terminal_id: terminal_key.clone(),
                            event,
                        },
                        "terminal-subscription",
                    )
                    .await
                    {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(
                        "websocket lagged on terminal {terminal_key}: skipped {skipped} messages"
                    );
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let mut current = subscriptions.lock().await;
    if let Some(existing) = current.insert(relay_key, handle) {
        existing.abort();
    }
    Ok(())
}

pub(crate) async fn subscribe_global(
    state: AppState,
    out_tx: mpsc::Sender<ServerEnvelope>,
    subscriptions: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    profile_id: String,
    role: UserRole,
) -> Result<()> {
    let relay = ensure_global_relay(&state, &profile_id).await?;
    let mut receiver = relay.subscribe();
    let handle = tokio::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    let Some(event) = filter_global_event_for_role(role, event) else {
                        continue;
                    };
                    if !send_relay_envelope(
                        &out_tx,
                        ServerEnvelope::GlobalEvent { event },
                        "global-subscription",
                    )
                    .await
                    {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!("websocket lagged on global relay: skipped {skipped} messages");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let mut current = subscriptions.lock().await;
    if let Some(existing) = current.insert(global_relay_key(&profile_id), handle) {
        existing.abort();
    }
    Ok(())
}

fn filter_session_event_for_role(role: UserRole, event: Value) -> Option<Value> {
    if role_has_admin_access(role) {
        return Some(event);
    }

    if event.get("method").and_then(Value::as_str) != Some("codex-webui/queueUpdated") {
        return Some(event);
    }

    let queue = event
        .get("params")
        .and_then(|params| params.get("queue"))
        .map(redacted_queue_payload)
        .unwrap_or_else(|| redacted_queue_payload(&Value::Null));
    Some(json!({
        "kind": event
            .get("kind")
            .cloned()
            .unwrap_or_else(|| json!("notification")),
        "method": "codex-webui/queueUpdated",
        "params": {
            "queue": queue
        }
    }))
}

fn redacted_notification_added_event(event: Value) -> Value {
    let params = event.get("params").cloned().unwrap_or_else(|| json!({}));
    let notification = params.get("notification").cloned().unwrap_or(Value::Null);
    json!({
        "kind": event
            .get("kind")
            .cloned()
            .unwrap_or_else(|| json!("notification")),
        "method": "codex-webui/notificationAdded",
        "params": {
            "notification": {
                "id": notification.get("id").cloned().unwrap_or(Value::Null),
                "type": notification.get("type").cloned().unwrap_or(Value::Null),
                "createdAt": notification.get("createdAt").cloned().unwrap_or(Value::Null),
                "readAt": notification.get("readAt").cloned().unwrap_or(Value::Null),
                "sessionId": notification.get("sessionId").cloned().unwrap_or(Value::Null)
            },
            "unreadCount": params.get("unreadCount").cloned().unwrap_or_else(|| json!(0))
        }
    })
}

fn filter_global_event_for_role(role: UserRole, event: Value) -> Option<Value> {
    if role_has_admin_access(role) {
        return Some(event);
    }

    match event.get("method").and_then(Value::as_str) {
        Some("codex-webui/sessionListsInvalidated")
        | Some("codex-webui/sessionSummaryUpdated")
        | Some("codex-webui/sessionAttention")
        | Some("codex-webui/notificationStateUpdated") => Some(event),
        Some("codex-webui/notificationAdded") => Some(redacted_notification_added_event(event)),
        _ => None,
    }
}

pub(crate) async fn ensure_stream_relay(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> Result<broadcast::Sender<Value>> {
    let relay_key = session_relay_key(profile_id, session_id);
    let mut relays = state.relays.lock().await;
    if let Some(existing) = relays.get(&relay_key) {
        return Ok(existing.clone());
    }

    let (sender, _) = broadcast::channel(256);
    relays.insert(relay_key, sender.clone());

    Ok(sender)
}

pub(crate) async fn session_stream_has_subscribers(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> bool {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id).0;
    state
        .relays
        .lock()
        .await
        .get(&session_relay_key(resolved_profile_id, session_id))
        .is_some_and(|relay| relay.receiver_count() > 0)
}

pub(crate) async fn prune_unused_session_relay(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) {
    prune_session_relay_with_receiver_limit(state, profile_id, session_id, 0).await;
}

pub(crate) async fn prune_unsubscribed_session_relay(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) {
    prune_session_relay_with_receiver_limit(state, profile_id, session_id, 1).await;
}

async fn prune_session_relay_with_receiver_limit(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    max_receivers: usize,
) {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id).0;
    let relay_key = session_relay_key(resolved_profile_id, session_id);
    let mut relays = state.relays.lock().await;
    if relays
        .get(&relay_key)
        .is_some_and(|relay| relay.receiver_count() <= max_receivers)
    {
        relays.remove(&relay_key);
    }
}

pub(crate) async fn ensure_global_relay(
    state: &AppState,
    profile_id: &str,
) -> Result<broadcast::Sender<Value>> {
    let relay_key = global_relay_key(profile_id);
    let mut relays = state.relays.lock().await;
    if let Some(existing) = relays.get(&relay_key) {
        return Ok(existing.clone());
    }

    let (sender, _) = broadcast::channel(256);
    relays.insert(relay_key, sender.clone());

    let state = state.clone();
    let profile_id = profile_id.to_string();
    let relay_sender = sender.clone();
    tokio::spawn(bridge_app_server_global_notifications(
        state.clone(),
        relay_sender.clone(),
        profile_id.clone(),
    ));

    Ok(sender)
}

async fn bridge_app_server_global_notifications(
    state: AppState,
    sender: broadcast::Sender<Value>,
    profile_id: String,
) {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, &profile_id)
        .0
        .to_string();
    let client = match app_server_client(&state, &profile_id).await {
        Ok(client) => client,
        Err(error) => {
            warn!("failed to create app-server bridge for {profile_id}: {error:#}");
            return;
        }
    };
    let mut notifications = client.subscribe_notifications();

    loop {
        match notifications.recv().await {
            Ok(notification) => {
                if matches!(
                    notification.method.as_str(),
                    "account/updated" | "account/rateLimits/updated"
                ) {
                    state.quota_cache.lock().await.remove(&resolved_profile_id);
                }

                if let Some(event) = map_app_server_global_notification(&notification) {
                    let _ = sender.send(event);
                }
            }
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                warn!(
                    "global app-server relay lagged for {profile_id}: skipped {skipped} messages"
                );
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

pub(crate) async fn emit_global_notification(state: &AppState, event: Value) {
    let relays = {
        let relays = state.relays.lock().await;
        relays
            .iter()
            .filter(|(key, _)| key.contains(GLOBAL_RELAY_KEY))
            .map(|(_, relay)| relay.clone())
            .collect::<Vec<_>>()
    };

    for relay in relays {
        let _ = relay.send(event.clone());
    }
}

pub(crate) async fn emit_profile_global_notification(
    state: &AppState,
    profile_id: &str,
    event: Value,
) {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id).0;
    let relay = {
        let relays = state.relays.lock().await;
        relays.get(&global_relay_key(resolved_profile_id)).cloned()
    };

    if let Some(relay) = relay {
        let _ = relay.send(event);
    }
}

pub(crate) async fn emit_profile_config_updated(state: &AppState, profile_id: &str, params: Value) {
    emit_profile_global_notification(
        state,
        profile_id,
        json!({
            "kind": "notification",
            "method": "codex-webui/configUpdated",
            "params": params
        }),
    )
    .await;
}
