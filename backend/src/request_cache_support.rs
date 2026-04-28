use super::*;

pub(crate) enum CachedResponseLookup {
    Hit(ServerEnvelope),
    Conflict,
    Miss,
}

pub(crate) enum InflightRequestRegistration {
    Started,
    Joined,
    Conflict,
}

pub(crate) async fn cached_response(
    state: &AppState,
    request_id: &str,
    method: &str,
    params_hash: &str,
) -> CachedResponseLookup {
    let mut cache = state.response_cache.lock().await;
    prune_response_cache(&mut cache);
    match cache.get(request_id) {
        Some(entry) if entry.method == method && entry.params_hash == params_hash => {
            CachedResponseLookup::Hit(entry.message.clone())
        }
        Some(_) => CachedResponseLookup::Conflict,
        None => CachedResponseLookup::Miss,
    }
}

pub(crate) async fn cache_response(
    state: &AppState,
    request_id: &str,
    method: &str,
    params_hash: &str,
    message: ServerEnvelope,
) {
    let mut cache = state.response_cache.lock().await;
    prune_response_cache(&mut cache);
    cache.insert(
        request_id.to_string(),
        CachedResponse {
            created_at: Instant::now(),
            method: method.to_string(),
            params_hash: params_hash.to_string(),
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
    method: &str,
    params_hash: &str,
    out_tx: &mpsc::UnboundedSender<ServerEnvelope>,
) -> InflightRequestRegistration {
    let mut inflight = state.inflight_requests.lock().await;
    inflight.retain(|_, request| !request.waiters.is_empty());

    if let Some(request) = inflight.get_mut(request_id) {
        if request.method != method || request.params_hash != params_hash {
            return InflightRequestRegistration::Conflict;
        }
        request.waiters.push(out_tx.clone());
        return InflightRequestRegistration::Joined;
    }

    inflight.insert(
        request_id.to_string(),
        InflightRequest {
            method: method.to_string(),
            params_hash: params_hash.to_string(),
            waiters: vec![out_tx.clone()],
        },
    );
    InflightRequestRegistration::Started
}

pub(crate) async fn resolve_inflight_request(
    state: &AppState,
    request_id: &str,
    message: ServerEnvelope,
) {
    let waiters = {
        let mut inflight = state.inflight_requests.lock().await;
        inflight
            .remove(request_id)
            .map(|request| request.waiters)
            .unwrap_or_default()
    };

    for waiter in waiters {
        let _ = waiter.send(message.clone());
    }
}
