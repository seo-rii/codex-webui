use super::*;

pub(crate) async fn subscribe_session(
    state: AppState,
    out_tx: mpsc::UnboundedSender<ServerEnvelope>,
    subscriptions: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    profile_id: String,
    session_id: String,
) -> Result<()> {
    let relay = ensure_stream_relay(&state, &profile_id, &session_id).await?;
    let mut receiver = relay.subscribe();
    let session_key = session_id.clone();
    let handle = tokio::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    let _ = out_tx.send(ServerEnvelope::Event {
                        session_id: session_key.clone(),
                        event,
                    });
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!("websocket lagged on session {session_key}: skipped {skipped} messages");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let mut current = subscriptions.lock().await;
    if let Some(existing) = current.insert(session_relay_key(&profile_id, &session_id), handle) {
        existing.abort();
    }
    Ok(())
}

pub(crate) async fn subscribe_terminal(
    state: AppState,
    out_tx: mpsc::UnboundedSender<ServerEnvelope>,
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
                    let _ = out_tx.send(ServerEnvelope::TerminalEvent {
                        terminal_id: terminal_key.clone(),
                        event,
                    });
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
    out_tx: mpsc::UnboundedSender<ServerEnvelope>,
    subscriptions: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    profile_id: String,
) -> Result<()> {
    let relay = ensure_global_relay(&state, &profile_id).await?;
    let mut receiver = relay.subscribe();
    let handle = tokio::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    let _ = out_tx.send(ServerEnvelope::GlobalEvent { event });
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
