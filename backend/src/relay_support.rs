use super::*;

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
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    let Some(event) = filter_session_event_for_role(role, event) else {
                        continue;
                    };
                    if stream_out_tx
                        .try_send(ServerEnvelope::Event {
                            session_id: session_key.clone(),
                            event,
                        })
                        .is_err()
                    {
                        warn!("dropping session subscription for slow websocket client");
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!("websocket lagged on session {session_key}: skipped {skipped} messages");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
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
                    if out_tx
                        .try_send(ServerEnvelope::TerminalEvent {
                            terminal_id: terminal_key.clone(),
                            event,
                        })
                        .is_err()
                    {
                        warn!("dropping terminal subscription for slow websocket client");
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
                    if out_tx
                        .try_send(ServerEnvelope::GlobalEvent { event })
                        .is_err()
                    {
                        warn!("dropping global subscription for slow websocket client");
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

fn redacted_queue_payload(queue: &Value) -> Value {
    let item_count = queue
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    json!({
        "sessionId": queue.get("sessionId").cloned().unwrap_or(Value::Null),
        "itemCount": item_count,
        "resumeRequired": queue
            .get("resumeRequired")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        "updatedAt": queue.get("updatedAt").cloned().unwrap_or(Value::Null)
    })
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
