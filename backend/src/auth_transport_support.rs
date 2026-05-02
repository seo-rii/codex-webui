use super::*;

#[derive(Debug, Deserialize)]
struct LoginPayload {
    password: Option<String>,
    #[serde(alias = "hcaptchaToken", alias = "hcaptcha_token")]
    hcaptcha_token: Option<String>,
}

pub(crate) async fn handle_auth_http(
    state: AppState,
    jar: CookieJar,
    method: Method,
    route_path: String,
    headers: HeaderMap,
    request: Request,
    peer_addr: Option<SocketAddr>,
) -> Response {
    let origin = extract_origin(&headers);
    let cors_origin = allowed_cors_origin(&state.config, &origin);
    let requested_headers = headers
        .get("access-control-request-headers")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    if method == Method::OPTIONS {
        if let Some(origin_value) = cors_origin {
            let mut response = Response::new(Body::empty());
            *response.status_mut() = StatusCode::NO_CONTENT;
            apply_cors_headers(
                response.headers_mut(),
                &origin_value,
                requested_headers.as_deref(),
            );
            return response;
        }
        return (StatusCode::FORBIDDEN, "CORS origin is not allowed.").into_response();
    }

    let result = match (method, route_path.as_str()) {
        (Method::POST, "/api/auth/login") => {
            auth_login(state.clone(), jar, headers, request, peer_addr).await
        }
        (Method::POST, "/api/auth/logout") => Ok(auth_logout(&state.config, jar)),
        (Method::POST, "/api/auth/profile") => {
            let Some(auth) = auth_context(&state.config, &jar) else {
                return json_error(StatusCode::UNAUTHORIZED, "Authentication required.");
            };
            select_profile(state.config.clone(), jar, headers, request, auth, peer_addr).await
        }
        (Method::GET, "/api/auth/session") => {
            let auth = auth_context(&state.config, &jar);
            let active_profile_id = auth
                .as_ref()
                .map(|context| context.profile_id.as_str())
                .unwrap_or(&state.config.default_profile_id);
            Ok((
                jar,
                Json(json!({
                    "authenticated": auth.is_some(),
                    "activeProfileId": active_profile_id,
                    "role": auth.map(|context| match context.role {
                        UserRole::Owner => "owner",
                        UserRole::Admin => "admin",
                        UserRole::Viewer => "viewer",
                    }),
                    "hcaptcha": {
                        "enabled": state.config.hcaptcha_enabled(),
                        "siteKey": state.config.hcaptcha_site_key(),
                    }
                })),
            )
                .into_response())
        }
        _ => Ok((StatusCode::NOT_FOUND, "Not found").into_response()),
    };

    let mut response = match result {
        Ok(response) => response,
        Err(error_message) => json_error(StatusCode::UNAUTHORIZED, &error_message),
    };

    if let Some(origin_value) = cors_origin {
        apply_cors_headers(
            response.headers_mut(),
            &origin_value,
            requested_headers.as_deref(),
        );
    }

    response
}

async fn auth_login(
    state: AppState,
    jar: CookieJar,
    headers: HeaderMap,
    request: Request,
    peer_addr: Option<SocketAddr>,
) -> std::result::Result<Response, String> {
    let secure_request = request_is_secure(&state.config, &headers, peer_addr);
    let body = match read_limited_body(request, SMALL_JSON_BODY_LIMIT, "login request body").await {
        Ok(body) => body,
        Err(error) => return Ok(json_error(error.status, &error.message)),
    };
    let payload: LoginPayload = serde_json::from_slice(&body).unwrap_or(LoginPayload {
        password: None,
        hcaptcha_token: None,
    });
    let password = payload.password.unwrap_or_default();
    let forwarded_ip = if forwarded_headers_allowed(&state.config, peer_addr) {
        headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(',').next())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .and_then(|value| value.parse::<std::net::IpAddr>().ok())
    } else {
        None
    };
    let remote_ip = forwarded_ip.or_else(|| peer_addr.map(|addr| addr.ip()));
    let identifier = remote_ip
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| "local".to_string());

    if !check_rate_limit(&state, &identifier).await {
        return Ok(json_error(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many login attempts. Try again later.",
        ));
    }

    if state.config.hcaptcha_enabled() {
        let Some(hcaptcha_secret_key) = state.config.hcaptcha_secret_key() else {
            return Ok(json_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "hCaptcha is not fully configured.",
            ));
        };
        let Some(hcaptcha_token) = payload
            .hcaptcha_token
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(json_error(
                StatusCode::BAD_REQUEST,
                "Complete the hCaptcha challenge before signing in.",
            ));
        };

        let mut verification_payload = vec![
            ("secret", hcaptcha_secret_key.to_string()),
            ("response", hcaptcha_token.to_string()),
        ];
        if let Some(remote_ip) = remote_ip {
            verification_payload.push(("remoteip", remote_ip.to_string()));
        }

        let verification_response = state
            .http
            .post("https://api.hcaptcha.com/siteverify")
            .form(&verification_payload)
            .send()
            .await
            .map_err(|error| {
                tracing::warn!("failed to verify hcaptcha: {error}");
                "Failed to verify hCaptcha."
            })?;

        if !verification_response.status().is_success() {
            tracing::warn!(
                status = %verification_response.status(),
                "hcaptcha verification request returned a non-success status"
            );
            return Ok(json_error(
                StatusCode::BAD_GATEWAY,
                "Failed to verify hCaptcha.",
            ));
        }

        let verification_result: Value = verification_response.json().await.map_err(|error| {
            tracing::warn!("failed to parse hcaptcha verification response: {error}");
            "Failed to verify hCaptcha."
        })?;
        let verification_ok = verification_result
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        if !verification_ok {
            record_login_failure(&state, &identifier).await;
            let _ = append_audit_log(
                &state.config,
                AuditLogEntry {
                    id: Uuid::new_v4().to_string(),
                    at: now_unix_ms(),
                    role: "anonymous".to_string(),
                    method: "auth/login".to_string(),
                    target: None,
                    ok: false,
                    error: Some("Failed hCaptcha verification.".to_string()),
                },
            )
            .await;
            return Ok(json_error(
                StatusCode::UNAUTHORIZED,
                "Complete the hCaptcha challenge before signing in.",
            ));
        }
    }

    let Some(role) =
        authenticate_role(&state.config, &password).map_err(|error| error.to_string())?
    else {
        record_login_failure(&state, &identifier).await;
        let _ = append_audit_log(
            &state.config,
            AuditLogEntry {
                id: Uuid::new_v4().to_string(),
                at: now_unix_ms(),
                role: "anonymous".to_string(),
                method: "auth/login".to_string(),
                target: None,
                ok: false,
                error: Some("Invalid password.".to_string()),
            },
        )
        .await;
        return Ok(json_error(StatusCode::UNAUTHORIZED, "Invalid password."));
    };

    clear_login_failures(&state, &identifier).await;
    let next_jar = issue_auth_cookie(&state.config, jar, secure_request, role)
        .map_err(|error| error.to_string())?;
    let _ = append_audit_log(
        &state.config,
        AuditLogEntry {
            id: Uuid::new_v4().to_string(),
            at: now_unix_ms(),
            role: user_role_label(role).to_string(),
            method: "auth/login".to_string(),
            target: None,
            ok: true,
            error: None,
        },
    )
    .await;
    Ok((
        next_jar,
        Json(json!({
            "ok": true,
            "role": user_role_label(role)
        })),
    )
        .into_response())
}

fn auth_logout(config: &Config, jar: CookieJar) -> Response {
    let mut cookie = Cookie::new(AUTH_COOKIE, "");
    cookie.set_path(auth_cookie_path(config));
    cookie.set_max_age(CookieDuration::seconds(0));
    let mut profile_cookie = Cookie::new(PROFILE_COOKIE, "");
    profile_cookie.set_path(auth_cookie_path(config));
    profile_cookie.set_max_age(CookieDuration::seconds(0));
    (
        jar.remove(cookie).remove(profile_cookie),
        Json(json!({ "ok": true })),
    )
        .into_response()
}
