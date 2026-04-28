use super::*;

pub(crate) async fn cached_response(state: &AppState, request_id: &str) -> Option<ServerEnvelope> {
    let mut cache = state.response_cache.lock().await;
    prune_response_cache(&mut cache);
    cache.get(request_id).map(|entry| entry.message.clone())
}

pub(crate) async fn cache_response(state: &AppState, request_id: &str, message: ServerEnvelope) {
    let mut cache = state.response_cache.lock().await;
    prune_response_cache(&mut cache);
    cache.insert(
        request_id.to_string(),
        CachedResponse {
            created_at: Instant::now(),
            message,
        },
    );
    prune_response_cache(&mut cache);
}

fn prune_response_cache(cache: &mut HashMap<String, CachedResponse>) {
    cache.retain(|_, entry| entry.created_at.elapsed() < CACHE_TTL);
    if cache.len() <= RESPONSE_CACHE_MAX_ENTRIES {
        return;
    }

    let mut entries = cache
        .iter()
        .map(|(key, entry)| (key.clone(), entry.created_at))
        .collect::<Vec<_>>();
    entries.sort_by_key(|(_, created_at)| *created_at);
    for (key, _) in entries
        .into_iter()
        .take(cache.len().saturating_sub(RESPONSE_CACHE_MAX_ENTRIES))
    {
        cache.remove(&key);
    }
}

pub(crate) async fn register_inflight_request(
    state: &AppState,
    request_id: &str,
    out_tx: &mpsc::UnboundedSender<ServerEnvelope>,
) -> bool {
    let mut inflight = state.inflight_requests.lock().await;
    inflight.retain(|_, waiters| !waiters.is_empty());

    if let Some(waiters) = inflight.get_mut(request_id) {
        waiters.push(out_tx.clone());
        return false;
    }

    inflight.insert(request_id.to_string(), vec![out_tx.clone()]);
    true
}

pub(crate) async fn resolve_inflight_request(
    state: &AppState,
    request_id: &str,
    message: ServerEnvelope,
) {
    let waiters = {
        let mut inflight = state.inflight_requests.lock().await;
        inflight.remove(request_id).unwrap_or_default()
    };

    for waiter in waiters {
        let _ = waiter.send(message.clone());
    }
}
