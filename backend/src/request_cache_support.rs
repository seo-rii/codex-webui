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
    Full,
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
    let response_bytes = serde_json::to_vec(&message)
        .map(|bytes| bytes.len())
        .unwrap_or_default();
    if response_bytes > RESPONSE_CACHE_MAX_ENTRY_BYTES {
        return;
    }

    let mut cache = state.response_cache.lock().await;
    prune_response_cache(&mut cache);
    cache.insert(
        request_id.to_string(),
        CachedResponse {
            created_at: Instant::now(),
            method: method.to_string(),
            params_hash: params_hash.to_string(),
            response_bytes,
            message,
        },
    );
    prune_response_cache(&mut cache);
}

fn prune_response_cache(cache: &mut HashMap<String, CachedResponse>) {
    cache.retain(|_, entry| entry.created_at.elapsed() < CACHE_TTL);
    let mut total_bytes = cache
        .values()
        .map(|entry| entry.response_bytes)
        .sum::<usize>();
    if cache.len() <= RESPONSE_CACHE_MAX_ENTRIES && total_bytes <= RESPONSE_CACHE_MAX_BYTES {
        return;
    }

    let mut entries = cache
        .iter()
        .map(|(key, entry)| (key.clone(), entry.created_at))
        .collect::<Vec<_>>();
    entries.sort_by_key(|(_, created_at)| *created_at);
    for (key, _) in entries.into_iter() {
        if cache.len() <= RESPONSE_CACHE_MAX_ENTRIES && total_bytes <= RESPONSE_CACHE_MAX_BYTES {
            break;
        }
        if let Some(removed) = cache.remove(&key) {
            total_bytes = total_bytes.saturating_sub(removed.response_bytes);
        }
    }
}

pub(crate) async fn register_inflight_request(
    state: &AppState,
    request_id: &str,
    method: &str,
    params_hash: &str,
    out_tx: &WsOutbound,
) -> InflightRequestRegistration {
    let mut inflight = state.inflight_requests.lock().await;
    inflight.retain(|_, request| {
        !request.waiters.is_empty() && request.created_at.elapsed() < INFLIGHT_REQUEST_TTL
    });

    if let Some(request) = inflight.get_mut(request_id) {
        if request.method != method || request.params_hash != params_hash {
            return InflightRequestRegistration::Conflict;
        }
        if request.waiters.len() >= INFLIGHT_REQUEST_MAX_WAITERS {
            return InflightRequestRegistration::Full;
        }
        request.waiters.push(out_tx.clone());
        return InflightRequestRegistration::Joined;
    }

    if inflight.len() >= INFLIGHT_REQUEST_MAX_ENTRIES {
        return InflightRequestRegistration::Full;
    }

    inflight.insert(
        request_id.to_string(),
        InflightRequest {
            created_at: Instant::now(),
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
        let _ = queue_ws_envelope(&waiter, message.clone(), "inflight-response");
    }
}
