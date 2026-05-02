use super::*;

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

pub(crate) async fn select_profile(
    config: Arc<Config>,
    jar: CookieJar,
    headers: HeaderMap,
    request: Request,
    auth: AuthContext,
) -> std::result::Result<Response, String> {
    let secure_request = request_is_secure(&config, &headers);
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
        let token = cookie.value();
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

        if parts.len() == 5 {
            match parts[2] {
                "owner" => UserRole::Owner,
                "viewer" => UserRole::Viewer,
                _ => UserRole::Admin,
            }
        } else {
            UserRole::Admin
        }
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

pub(crate) fn request_is_secure(config: &Config, headers: &HeaderMap) -> bool {
    if !config.trust_proxy_headers {
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

pub(crate) fn websocket_origin_allowed(config: &Config, headers: &HeaderMap) -> bool {
    request_origin_allowed(config, headers)
}

pub(crate) fn request_origin_allowed(config: &Config, headers: &HeaderMap) -> bool {
    let Some(origin) = extract_origin(headers) else {
        return true;
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
    let scheme = if request_is_secure(config, headers) {
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
    let history = attempts.entry(identifier.to_string()).or_default();
    history.retain(|entry| now.saturating_sub(*entry) < LOGIN_WINDOW_MS);
    history.len() < LOGIN_MAX_ATTEMPTS
}

pub(crate) async fn record_login_failure(state: &AppState, identifier: &str) {
    let now = now_millis();
    let mut attempts = state.login_attempts.lock().await;
    let history = attempts.entry(identifier.to_string()).or_default();
    history.retain(|entry| now.saturating_sub(*entry) < LOGIN_WINDOW_MS);
    history.push(now);
}

pub(crate) async fn clear_login_failures(state: &AppState, identifier: &str) {
    state.login_attempts.lock().await.remove(identifier);
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
