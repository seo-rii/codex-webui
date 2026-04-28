use super::*;

pub(crate) async fn serve_static_asset(state: AppState, route_path: &str) -> Response {
    let Some(relative_path) = sanitize_static_relative_path(route_path) else {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    };

    let cache_key = relative_path.to_string_lossy().into_owned();
    let cacheable = static_asset_backend_cacheable(route_path);
    if cacheable {
        if let Some(cached) = state
            .static_asset_cache
            .lock()
            .await
            .get(&cache_key)
            .cloned()
        {
            return static_asset_response(cached);
        }
    }

    let asset_path = state.config.static_dir.join(&relative_path);
    if let Some(asset) = load_static_asset(&state.config, &asset_path, route_path).await {
        if cacheable {
            let mut cache = state.static_asset_cache.lock().await;
            if asset.bytes.len() <= STATIC_ASSET_CACHE_MAX_BYTES {
                cache.insert(cache_key, asset.clone());
            }

            let mut total_bytes = cache.values().map(|asset| asset.bytes.len()).sum::<usize>();
            if cache.len() > STATIC_ASSET_CACHE_MAX_ENTRIES
                || total_bytes > STATIC_ASSET_CACHE_MAX_BYTES
            {
                let mut entries = cache
                    .iter()
                    .map(|(key, asset)| (key.clone(), asset.created_at))
                    .collect::<Vec<_>>();
                entries.sort_by_key(|(_, created_at)| *created_at);
                for (key, _) in entries {
                    if cache.len() <= STATIC_ASSET_CACHE_MAX_ENTRIES
                        && total_bytes <= STATIC_ASSET_CACHE_MAX_BYTES
                    {
                        break;
                    }
                    if let Some(removed) = cache.remove(&key) {
                        total_bytes = total_bytes.saturating_sub(removed.bytes.len());
                    }
                }
            }
        }
        return static_asset_response(asset);
    }

    if looks_like_static_asset(route_path) {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }

    let fallback_name = if route_path == "/" {
        "index.html"
    } else {
        "200.html"
    };
    let fallback_path = state.config.static_dir.join(fallback_name);
    if let Some(asset) = load_static_asset(&state.config, &fallback_path, route_path).await {
        return static_asset_response(asset);
    }

    (StatusCode::NOT_FOUND, "Not found").into_response()
}

fn sanitize_static_relative_path(route_path: &str) -> Option<PathBuf> {
    let raw = route_path.trim_start_matches('/');
    if raw.is_empty() {
        return Some(PathBuf::from("index.html"));
    }

    let mut sanitized = PathBuf::new();
    for component in Path::new(raw).components() {
        match component {
            Component::Normal(value) => sanitized.push(value),
            Component::CurDir => {}
            Component::ParentDir | Component::Prefix(_) | Component::RootDir => return None,
        }
    }

    if sanitized.as_os_str().is_empty() {
        Some(PathBuf::from("index.html"))
    } else {
        Some(sanitized)
    }
}

fn looks_like_static_asset(route_path: &str) -> bool {
    Path::new(route_path)
        .file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.contains('.'))
}

fn static_asset_backend_cacheable(route_path: &str) -> bool {
    route_path.starts_with("/_app/immutable/")
}

async fn load_static_asset(
    config: &Config,
    asset_path: &Path,
    route_path: &str,
) -> Option<CachedStaticAsset> {
    let metadata = tokio_fs::metadata(asset_path).await.ok()?;
    if !metadata.is_file() {
        return None;
    }
    if metadata.len() > STATIC_ASSET_MAX_BYTES {
        warn!(
            path = %asset_path.display(),
            size = metadata.len(),
            max_size = STATIC_ASSET_MAX_BYTES,
            "static asset exceeds preview limit"
        );
        return None;
    }

    let content_type = static_content_type(asset_path);
    let cache_control = static_cache_control(route_path, asset_path);

    if static_asset_is_text(asset_path) {
        let text = tokio_fs::read_to_string(asset_path).await.ok()?;
        let replaced = text.replace(STATIC_BASE_PLACEHOLDER, &config.base_path);
        return Some(CachedStaticAsset {
            created_at: Instant::now(),
            bytes: Bytes::from(replaced),
            content_type,
            cache_control,
        });
    }

    let bytes = tokio_fs::read(asset_path).await.ok()?;
    Some(CachedStaticAsset {
        created_at: Instant::now(),
        bytes: Bytes::from(bytes),
        content_type,
        cache_control,
    })
}

fn static_asset_response(asset: CachedStaticAsset) -> Response {
    let mut response = Response::new(Body::from(asset.bytes));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(asset.content_type),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(asset.cache_control),
    );
    headers.insert(
        header::HeaderName::from_static("content-security-policy"),
        HeaderValue::from_static(
            "default-src 'self'; script-src 'self' 'unsafe-inline' https://hcaptcha.com https://*.hcaptcha.com; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob: http: https:; font-src 'self' data:; connect-src 'self' ws: wss: http: https:; worker-src 'self' blob:; frame-src https://hcaptcha.com https://*.hcaptcha.com; object-src 'none'; base-uri 'none'; frame-ancestors 'none'",
        ),
    );
    headers.insert(
        header::HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::HeaderName::from_static("referrer-policy"),
        HeaderValue::from_static("same-origin"),
    );
    response
}

fn static_asset_is_text(asset_path: &Path) -> bool {
    matches!(
        asset_path.extension().and_then(|value| value.to_str()),
        Some("html" | "js" | "mjs" | "css" | "json" | "map" | "svg" | "txt" | "webmanifest")
    )
}

fn static_content_type(asset_path: &Path) -> &'static str {
    match asset_path.extension().and_then(|value| value.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map" | "webmanifest") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("wasm") => "application/wasm",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn static_cache_control(route_path: &str, asset_path: &Path) -> &'static str {
    if route_path == "/"
        || matches!(
            asset_path.extension().and_then(|value| value.to_str()),
            Some("html")
        )
    {
        "no-store, max-age=0, must-revalidate"
    } else if route_path == "/service-worker.js"
        || route_path == "/_app/version.json"
        || route_path == "/_app/env.js"
    {
        "no-store, max-age=0, must-revalidate"
    } else if matches!(
        asset_path.file_name().and_then(|value| value.to_str()),
        Some("manifest.webmanifest")
    ) {
        "no-cache, max-age=0, must-revalidate"
    } else if route_path.starts_with("/_app/immutable/") {
        "public, max-age=31536000, immutable"
    } else {
        "public, max-age=300, must-revalidate"
    }
}
