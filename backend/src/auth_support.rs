use super::*;

static REVOKED_AUTH_SESSIONS: std::sync::OnceLock<
    std::sync::Mutex<HashMap<String, AuthRevocationStore>>,
> = std::sync::OnceLock::new();

#[derive(Default)]
struct AuthRevocationStore {
    loaded: bool,
    entries: HashMap<String, u128>,
}

pub(crate) fn issue_auth_cookie(
    config: &Config,
    jar: CookieJar,
    secure_request: bool,
    role: UserRole,
) -> Result<CookieJar> {
    let secure = resolve_cookie_secure(config, secure_request)?;
    let cookie_value = make_auth_token(config, role)?;
    let mut cookie = Cookie::new(AUTH_COOKIE, cookie_value);
    cookie.set_path(auth_cookie_path(config));
    cookie.set_http_only(true);
    cookie.set_same_site(match config.cookie_same_site {
        SameSiteMode::Strict => SameSite::Strict,
        SameSiteMode::Lax => SameSite::Lax,
        SameSiteMode::None => SameSite::None,
    });
    cookie.set_secure(secure);
    cookie.set_max_age(CookieDuration::days(7));
    Ok(jar.add(cookie))
}

pub(crate) fn issue_profile_cookie(
    config: &Config,
    jar: CookieJar,
    secure_request: bool,
    profile_id: &str,
) -> Result<CookieJar> {
    let secure = resolve_cookie_secure(config, secure_request)?;
    let mut cookie = Cookie::new(PROFILE_COOKIE, profile_id.to_string());
    cookie.set_path(auth_cookie_path(config));
    cookie.set_http_only(true);
    cookie.set_same_site(match config.cookie_same_site {
        SameSiteMode::Strict => SameSite::Strict,
        SameSiteMode::Lax => SameSite::Lax,
        SameSiteMode::None => SameSite::None,
    });
    cookie.set_secure(secure);
    cookie.set_max_age(CookieDuration::days(30));
    Ok(jar.add(cookie))
}

pub(crate) fn issue_csrf_cookie(
    config: &Config,
    jar: CookieJar,
    secure_request: bool,
) -> Result<CookieJar> {
    let secure = resolve_cookie_secure(config, secure_request)?;
    let cookie_value = make_csrf_token(config)?;
    let mut cookie = Cookie::new(CSRF_COOKIE, cookie_value);
    cookie.set_path(auth_cookie_path(config));
    cookie.set_http_only(false);
    cookie.set_same_site(match config.cookie_same_site {
        SameSiteMode::Strict => SameSite::Strict,
        SameSiteMode::Lax => SameSite::Lax,
        SameSiteMode::None => SameSite::None,
    });
    cookie.set_secure(secure);
    cookie.set_max_age(CookieDuration::days(7));
    Ok(jar.add(cookie))
}

pub(crate) fn clear_csrf_cookie(config: &Config, jar: CookieJar) -> CookieJar {
    let mut cookie = Cookie::new(CSRF_COOKIE, "");
    cookie.set_path(auth_cookie_path(config));
    cookie.set_max_age(CookieDuration::seconds(0));
    jar.remove(cookie)
}

pub(crate) fn append_legacy_root_auth_cookie_clears(config: &Config, response: &mut Response) {
    if auth_cookie_path(config) == "/" {
        return;
    }
    append_auth_cookie_clears_for_path(config, response, "/");
}

pub(crate) fn append_current_path_auth_cookie_clears(config: &Config, response: &mut Response) {
    let path = auth_cookie_path(config);
    append_auth_cookie_clears_for_path(config, response, &path);
}

fn append_auth_cookie_clears_for_path(config: &Config, response: &mut Response, path: &str) {
    for name in [AUTH_COOKIE, PROFILE_COOKIE, CSRF_COOKIE] {
        let mut cookie = Cookie::new(name, "");
        cookie.set_path(path.to_string());
        cookie.set_max_age(CookieDuration::seconds(0));
        cookie.set_same_site(match config.cookie_same_site {
            SameSiteMode::Strict => SameSite::Strict,
            SameSiteMode::Lax => SameSite::Lax,
            SameSiteMode::None => SameSite::None,
        });
        if config.cookie_secure_mode == CookieSecureMode::Always {
            cookie.set_secure(true);
        }
        if name != CSRF_COOKIE {
            cookie.set_http_only(true);
        }
        if let Ok(value) = HeaderValue::from_str(&cookie.to_string()) {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
    }
}

pub(crate) fn auth_cookie_path(config: &Config) -> String {
    if config.base_path.is_empty() {
        "/".to_string()
    } else {
        config.base_path.clone()
    }
}

pub(crate) fn resolve_cookie_secure(config: &Config, secure_request: bool) -> Result<bool> {
    if config.cookie_same_site == SameSiteMode::None
        && config.cookie_secure_mode == CookieSecureMode::Never
    {
        return Err(anyhow!(
            "CODEX_WEBUI_COOKIE_SAMESITE=none cannot be combined with CODEX_WEBUI_COOKIE_SECURE=never."
        ));
    }

    match config.cookie_secure_mode {
        CookieSecureMode::Always => Ok(true),
        CookieSecureMode::Never => Ok(false),
        CookieSecureMode::Auto => {
            if config.cookie_same_site == SameSiteMode::None && !secure_request {
                Err(anyhow!(
                    "CODEX_WEBUI_COOKIE_SAMESITE=none requires HTTPS or CODEX_WEBUI_COOKIE_SECURE=always."
                ))
            } else {
                Ok(secure_request)
            }
        }
    }
}

pub(crate) fn make_auth_token(config: &Config, role: UserRole) -> Result<String> {
    let now = now_millis();
    let expires = now + 7 * 24 * 60 * 60 * 1000;
    let nonce = Uuid::new_v4().simple().to_string();
    let payload = format!("{now}.{expires}.{}.{}", user_role_label(role), nonce);
    let signature = sign(config, &payload)?;
    Ok(format!("{payload}.{signature}"))
}

fn parse_auth_token(config: &Config, token: &str) -> Option<(UserRole, String, u128)> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 4 && parts.len() != 5 {
        return None;
    }
    let payload = parts[..parts.len() - 1].join(".");
    let Ok(expected) = sign(config, &payload) else {
        return None;
    };
    if expected
        .as_bytes()
        .ct_eq(parts[parts.len() - 1].as_bytes())
        .unwrap_u8()
        != 1
    {
        return None;
    }
    let expires = parts[1].parse::<u128>().ok()?;
    if now_millis() >= expires {
        return None;
    }
    let (role, nonce) = if parts.len() == 5 {
        let role = match parts[2] {
            "owner" => UserRole::Owner,
            "viewer" => UserRole::Viewer,
            _ => UserRole::Admin,
        };
        (role, parts[3])
    } else {
        (UserRole::Admin, parts[2])
    };
    if nonce.is_empty() {
        return None;
    }
    Some((role, nonce.to_string(), expires))
}

fn auth_revocations_path(config: &Config) -> PathBuf {
    config.data_dir.join("auth-revocations.jsonl")
}

fn prune_revoked_auth_sessions(entries: &mut HashMap<String, u128>, now: u128) {
    entries.retain(|_, expires| *expires > now);
    if entries.len() <= AUTH_REVOKED_SESSION_MAX_ENTRIES {
        return;
    }

    let mut oldest = entries
        .iter()
        .map(|(nonce, expires)| (nonce.clone(), *expires))
        .collect::<Vec<_>>();
    oldest.sort_by_key(|(_, expires)| *expires);
    for (nonce, _) in oldest {
        if entries.len() <= AUTH_REVOKED_SESSION_MAX_ENTRIES {
            break;
        }
        entries.remove(&nonce);
    }
}

fn load_revoked_auth_sessions(config: &Config, store: &mut AuthRevocationStore, now: u128) {
    if store.loaded {
        prune_revoked_auth_sessions(&mut store.entries, now);
        return;
    }
    store.loaded = true;
    let Ok(raw) = fs::read_to_string(auth_revocations_path(config)) else {
        prune_revoked_auth_sessions(&mut store.entries, now);
        return;
    };
    for line in raw.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(nonce) = entry
            .get("nonce")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let Some(expires) = entry.get("expires").and_then(Value::as_u64) else {
            continue;
        };
        let expires = expires as u128;
        if expires > now {
            store.entries.insert(nonce.to_string(), expires);
        }
    }
    prune_revoked_auth_sessions(&mut store.entries, now);
}

fn revoked_auth_sessions() -> &'static std::sync::Mutex<HashMap<String, AuthRevocationStore>> {
    REVOKED_AUTH_SESSIONS.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

fn auth_revocation_store_key(config: &Config) -> String {
    config.data_dir.display().to_string()
}

fn auth_session_is_revoked(config: &Config, nonce: &str, expires: u128) -> bool {
    let now = now_millis();
    let Ok(mut stores) = revoked_auth_sessions().lock() else {
        return false;
    };
    let store = stores
        .entry(auth_revocation_store_key(config))
        .or_insert_with(AuthRevocationStore::default);
    load_revoked_auth_sessions(config, store, now);
    store
        .entries
        .get(nonce)
        .is_some_and(|revoked_expires| *revoked_expires == expires)
}

pub(crate) fn revoke_auth_cookie(config: &Config, jar: &CookieJar) -> bool {
    let Some((_, nonce, expires)) = jar
        .get(AUTH_COOKIE)
        .and_then(|cookie| parse_auth_token(config, cookie.value()))
    else {
        return false;
    };
    let now = now_millis();
    let Ok(mut stores) = revoked_auth_sessions().lock() else {
        return false;
    };
    let store = stores
        .entry(auth_revocation_store_key(config))
        .or_insert_with(AuthRevocationStore::default);
    load_revoked_auth_sessions(config, store, now);
    store.entries.insert(nonce.clone(), expires);
    prune_revoked_auth_sessions(&mut store.entries, now);
    drop(stores);

    if let Some(parent) = auth_revocations_path(config).parent() {
        let _ = fs::create_dir_all(parent);
    }
    let entry = json!({
        "nonce": nonce,
        "expires": expires,
        "revokedAt": now,
    });
    if let Ok(line) = serde_json::to_string(&entry) {
        if let Ok(mut file) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(auth_revocations_path(config))
        {
            let mut record = Vec::with_capacity(line.len() + 1);
            record.extend_from_slice(line.as_bytes());
            record.push(b'\n');
            let _ = std::io::Write::write_all(&mut file, &record);
        }
    }
    true
}

pub(crate) fn make_csrf_token(config: &Config) -> Result<String> {
    let nonce = Uuid::new_v4().simple().to_string();
    let payload = format!("csrf.{nonce}");
    let signature = sign(config, &payload)?;
    Ok(format!("{nonce}.{signature}"))
}

pub(crate) fn verify_csrf_token(config: &Config, jar: &CookieJar, headers: &HeaderMap) -> bool {
    let Some(cookie_token) = jar.get(CSRF_COOKIE).map(|cookie| cookie.value().trim()) else {
        return false;
    };
    let Some(header_token) = headers
        .get(CSRF_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
    else {
        return false;
    };
    if cookie_token.is_empty()
        || header_token.is_empty()
        || cookie_token
            .as_bytes()
            .ct_eq(header_token.as_bytes())
            .unwrap_u8()
            != 1
    {
        return false;
    }

    let mut parts = cookie_token.split('.');
    let Some(nonce) = parts.next() else {
        return false;
    };
    let Some(signature) = parts.next() else {
        return false;
    };
    if parts.next().is_some() || nonce.is_empty() || signature.is_empty() {
        return false;
    }
    let Ok(expected) = sign(config, &format!("csrf.{nonce}")) else {
        return false;
    };
    expected.as_bytes().ct_eq(signature.as_bytes()).unwrap_u8() == 1
}

pub(crate) async fn select_profile(
    config: Arc<Config>,
    jar: CookieJar,
    headers: HeaderMap,
    request: Request,
    auth: AuthContext,
    peer_addr: Option<SocketAddr>,
) -> std::result::Result<Response, String> {
    let secure_request = request_is_secure(&config, &headers, peer_addr);
    let payload = match read_json_body(request, SMALL_JSON_BODY_LIMIT, "profile request body").await
    {
        Ok(payload) => payload,
        Err(error) => return Ok(json_error(error.status, &error.message)),
    };
    let requested_profile_id = payload
        .get("profileId")
        .and_then(Value::as_str)
        .map(sanitize_profile_id)
        .unwrap_or_else(|| config.default_profile_id.clone());

    if !config.profiles.contains_key(&requested_profile_id) {
        return Ok(json_error(StatusCode::BAD_REQUEST, "Unknown profile."));
    }

    let next_jar = issue_profile_cookie(&config, jar, secure_request, &requested_profile_id)
        .map_err(|error| error.to_string())?;
    let _ = append_audit_log(
        &config,
        AuditLogEntry {
            id: Uuid::new_v4().to_string(),
            at: now_unix_ms(),
            role: user_role_label(auth.role).to_string(),
            method: "auth/profile".to_string(),
            target: Some(requested_profile_id.clone()),
            ok: true,
            error: None,
        },
    )
    .await;

    Ok((
        next_jar,
        Json(json!({
            "ok": true,
            "activeProfileId": requested_profile_id,
        })),
    )
        .into_response())
}

pub(crate) fn auth_context(config: &Config, jar: &CookieJar) -> Option<AuthContext> {
    let role = if let Some(cookie) = jar.get(AUTH_COOKIE) {
        let Some((role, nonce, expires)) = parse_auth_token(config, cookie.value()) else {
            return None;
        };
        if auth_session_is_revoked(config, &nonce, expires) {
            return None;
        }
        role
    } else if authless_admin_allowed(config) {
        UserRole::Admin
    } else {
        return None;
    };

    let profile_id = jar
        .get(PROFILE_COOKIE)
        .map(|cookie| sanitize_profile_id(cookie.value()))
        .filter(|value| config.profiles.contains_key(value))
        .unwrap_or_else(|| config.default_profile_id.clone());

    Some(AuthContext { role, profile_id })
}

pub(crate) fn sign(config: &Config, payload: &str) -> Result<String> {
    let secret = validate_session_secret_value(config.session_secret.as_deref())?;
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).context("failed to initialize HMAC")?;
    mac.update(payload.as_bytes());
    Ok(URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
}

pub(crate) fn forwarded_headers_allowed(config: &Config, peer_addr: Option<SocketAddr>) -> bool {
    if !config.trust_proxy_headers {
        return false;
    }
    let Some(peer_ip) = peer_addr.map(|addr| addr.ip()) else {
        return false;
    };
    if config.trusted_proxy_cidrs.is_empty() {
        return peer_ip.is_loopback();
    }
    config
        .trusted_proxy_cidrs
        .iter()
        .any(|proxy| proxy.contains(peer_ip))
}

pub(crate) fn request_is_secure(
    config: &Config,
    headers: &HeaderMap,
    peer_addr: Option<SocketAddr>,
) -> bool {
    if !forwarded_headers_allowed(config, peer_addr) {
        return false;
    }
    if let Some(forwarded) = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return forwarded.eq_ignore_ascii_case("https");
    }
    false
}

pub(crate) fn extract_origin(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .and_then(normalize_origin)
}

pub(crate) fn allowed_cors_origin(config: &Config, origin: &Option<String>) -> Option<String> {
    let origin = origin.as_ref()?;
    if config
        .cors_allowed_origins
        .iter()
        .any(|allowed| allowed == origin)
    {
        Some(origin.clone())
    } else {
        None
    }
}

pub(crate) fn websocket_origin_allowed(
    config: &Config,
    headers: &HeaderMap,
    peer_addr: Option<SocketAddr>,
) -> bool {
    request_origin_allowed(config, headers, peer_addr)
}

pub(crate) fn request_origin_allowed(
    config: &Config,
    headers: &HeaderMap,
    peer_addr: Option<SocketAddr>,
) -> bool {
    let Some(origin) = extract_origin(headers) else {
        return !config.require_origin_header && public_host_is_loopback(config);
    };
    if allowed_cors_origin(config, &Some(origin.clone())).is_some() {
        return true;
    }

    let Some(host) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return false;
    };
    let scheme = if request_is_secure(config, headers, peer_addr) {
        "https"
    } else {
        "http"
    };
    let Some(expected_origin) = normalize_origin(&format!("{scheme}://{host}")) else {
        return false;
    };
    origin == expected_origin
}

pub(crate) fn apply_cors_headers(
    headers: &mut HeaderMap,
    origin: &str,
    request_headers: Option<&str>,
) {
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_str(origin).unwrap_or_else(|_| HeaderValue::from_static("null")),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
        HeaderValue::from_static("true"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET,HEAD,POST,PATCH,PUT,DELETE,OPTIONS"),
    );
    headers.insert(
        header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static("600"),
    );
    append_vary(headers, "Origin");
    if let Some(request_headers) = request_headers {
        if let Ok(value) = HeaderValue::from_str(request_headers) {
            headers.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, value);
        }
        append_vary(headers, "Access-Control-Request-Headers");
    }
}

fn append_vary(headers: &mut HeaderMap, value: &str) {
    let existing = headers
        .get(header::VARY)
        .and_then(|current| current.to_str().ok())
        .unwrap_or_default();
    let mut values: Vec<&str> = existing
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .collect();
    if !values.iter().any(|entry| *entry == value) {
        values.push(value);
    }
    if let Ok(header_value) = HeaderValue::from_str(&values.join(", ")) {
        headers.insert(header::VARY, header_value);
    }
}

pub(crate) async fn check_rate_limit(state: &AppState, identifier: &str) -> bool {
    let now = now_millis();
    let mut attempts = state.login_attempts.lock().await;
    prune_login_attempts(&mut attempts, now, identifier);
    let history = attempts.entry(identifier.to_string()).or_default();
    history.retain(|entry| now.saturating_sub(*entry) < LOGIN_WINDOW_MS);
    history.len() < LOGIN_MAX_ATTEMPTS
}

pub(crate) async fn record_login_failure(state: &AppState, identifier: &str) {
    let now = now_millis();
    let mut attempts = state.login_attempts.lock().await;
    prune_login_attempts(&mut attempts, now, identifier);
    let history = attempts.entry(identifier.to_string()).or_default();
    history.retain(|entry| now.saturating_sub(*entry) < LOGIN_WINDOW_MS);
    history.push(now);
}

pub(crate) async fn clear_login_failures(state: &AppState, identifier: &str) {
    state.login_attempts.lock().await.remove(identifier);
}

fn prune_login_attempts(
    attempts: &mut HashMap<String, Vec<u128>>,
    now: u128,
    protected_identifier: &str,
) {
    attempts.retain(|_, history| {
        history.retain(|entry| now.saturating_sub(*entry) < LOGIN_WINDOW_MS);
        !history.is_empty()
    });
    if attempts.len() < LOGIN_RATE_LIMIT_MAX_IDENTIFIERS
        || attempts.contains_key(protected_identifier)
    {
        return;
    }

    let mut buckets = attempts
        .iter()
        .filter(|(identifier, _)| identifier.as_str() != protected_identifier)
        .map(|(identifier, history)| {
            (
                identifier.clone(),
                history.iter().copied().max().unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();
    buckets.sort_by_key(|(_, newest_attempt)| *newest_attempt);
    for (identifier, _) in buckets {
        if attempts.len() < LOGIN_RATE_LIMIT_MAX_IDENTIFIERS {
            break;
        }
        attempts.remove(&identifier);
    }
}

pub(crate) fn verify_password_pair(
    plain: Option<&String>,
    hashed: Option<&String>,
    input: &str,
    required_error: &str,
) -> Result<bool> {
    if let Some(password) = plain {
        return Ok(password.as_bytes().ct_eq(input.as_bytes()).into());
    }

    let Some(password_hash) = hashed else {
        return Err(anyhow!(required_error.to_string()));
    };

    let mut parts = password_hash.split('$');
    let Some(kind) = parts.next() else {
        return Ok(false);
    };
    let Some(saved_salt) = parts.next() else {
        return Ok(false);
    };
    let Some(saved_key) = parts.next() else {
        return Ok(false);
    };

    if kind != "scrypt" {
        return Err(anyhow!("Unsupported password hash format."));
    }

    let salt = URL_SAFE_NO_PAD
        .decode(saved_salt)
        .context("invalid password hash salt")?;
    let expected = URL_SAFE_NO_PAD
        .decode(saved_key)
        .context("invalid password hash key")?;
    let params = ScryptParams::new(14, 8, 1, expected.len())?;
    let mut derived = vec![0_u8; expected.len()];
    scrypt(input.as_bytes(), &salt, &params, &mut derived)
        .context("failed to derive password hash")?;
    Ok(derived.ct_eq(&expected).into())
}

pub(crate) fn authenticate_role(config: &Config, input: &str) -> Result<Option<UserRole>> {
    if authless_admin_allowed(config) {
        return Ok(Some(UserRole::Admin));
    }

    if (config.owner_password.is_some() || config.owner_password_hash.is_some())
        && verify_password_pair(
            config.owner_password.as_ref(),
            config.owner_password_hash.as_ref(),
            input,
            "Failed to verify owner password.",
        )?
    {
        return Ok(Some(UserRole::Owner));
    }

    if verify_password_pair(
        config.password.as_ref(),
        config.password_hash.as_ref(),
        input,
        "Set CODEX_WEBUI_PASSWORD_HASH or CODEX_WEBUI_PASSWORD before using the Rust gateway.",
    )? {
        return Ok(Some(UserRole::Admin));
    }

    if config.viewer_password.is_none() && config.viewer_password_hash.is_none() {
        return Ok(None);
    }

    if verify_password_pair(
        config.viewer_password.as_ref(),
        config.viewer_password_hash.as_ref(),
        input,
        "Failed to verify viewer password.",
    )? {
        return Ok(Some(UserRole::Viewer));
    }

    Ok(None)
}
