use super::*;

pub(crate) async fn codex_runtime_status(state: &AppState, check_latest: bool) -> Result<Value> {
    let configured_bin = state.config.codex_bin.clone();
    let resolved_bin_path = resolve_binary_path(&configured_bin).await;
    let npm_available = command_available(npm_command()).await;
    let install_command = format!("npm install -g {CODEX_NPM_PACKAGE}@latest");
    let update_command = install_command.clone();
    let mut issues = Vec::new();

    let version = match read_codex_version(state).await {
        Ok(version) => Some(version),
        Err(error) => {
            issues.push(error.to_string());
            None
        }
    };

    let mut latest_version: Option<String> = None;
    let mut update_available: Option<bool> = None;
    let mut last_checked_at: Option<String> = None;

    if check_latest {
        last_checked_at = Some(now_rfc3339());
        if npm_available {
            match fetch_latest_published_version().await {
                Ok(value) => {
                    latest_version = value;
                    update_available = latest_version
                        .as_deref()
                        .and_then(extract_semver)
                        .zip(version.as_deref().and_then(extract_semver))
                        .map(|(latest, current)| compare_versions(&latest, &current) > 0);
                }
                Err(error) => issues.push(error.to_string()),
            }
        } else {
            issues.push("npm was not found in PATH.".to_string());
        }
    }

    Ok(json!({
        "installed": version.is_some(),
        "configuredBin": configured_bin,
        "resolvedBinPath": resolved_bin_path,
        "npmAvailable": npm_available,
        "version": version,
        "latestVersion": latest_version,
        "updateAvailable": update_available,
        "installCommand": install_command,
        "updateCommand": update_command,
        "lastCheckedAt": last_checked_at,
        "issues": issues,
    }))
}

pub(crate) async fn install_or_update_codex(
    state: &AppState,
    install_if_missing: bool,
) -> Result<Value> {
    let before = codex_runtime_status(state, false).await?;
    let installed = before
        .get("installed")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    if !install_if_missing && !installed {
        anyhow::bail!("Codex is not installed yet. Install it first.");
    }

    if !command_available(npm_command()).await {
        anyhow::bail!("npm was not found in PATH.");
    }

    let package_spec = format!("{CODEX_NPM_PACKAGE}@latest");
    let output = run_command_with_timeout(
        npm_command(),
        vec!["install".to_string(), "-g".to_string(), package_spec],
        NPM_INSTALL_TIMEOUT,
    )
    .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let message = if !stderr.is_empty() { stderr } else { stdout };
        anyhow::bail!(if message.is_empty() {
            "npm install -g failed.".to_string()
        } else {
            message
        });
    }

    let runtime = codex_runtime_status(state, true).await?;
    Ok(json!({
        "ok": true,
        "message": if install_if_missing && !installed {
            "Codex installed successfully."
        } else {
            "Codex updated successfully."
        },
        "runtime": runtime,
    }))
}

pub(crate) async fn codex_quota_status(
    state: &AppState,
    refresh: bool,
    profile_id: &str,
) -> Result<Value> {
    if !refresh {
        let cache = state.quota_cache.lock().await;
        if let Some(cached) = cache.get(profile_id) {
            if cached.created_at.elapsed() < QUOTA_CACHE_TTL {
                return Ok(cached.payload.clone());
            }
        }
    }

    let payload = match fetch_codex_quota(state, profile_id).await {
        Ok(payload) => payload,
        Err(error) => json!({
            "available": false,
            "source": Value::Null,
            "fetchedAt": now_unix_ms(),
            "account": Value::Null,
            "plan": Value::Null,
            "fiveHour": Value::Null,
            "weekly": Value::Null,
            "error": error.to_string(),
        }),
    };

    let mut cache = state.quota_cache.lock().await;
    cache.insert(
        profile_id.to_string(),
        CachedQuota {
            created_at: Instant::now(),
            payload: payload.clone(),
        },
    );

    Ok(payload)
}

pub(crate) async fn get_account_state(state: &AppState, profile_id: &str) -> Result<Value> {
    let client = app_server_client(state, profile_id).await?;
    match client
        .request("account/read", json!({ "refreshToken": false }))
        .await
    {
        Ok(response) => Ok(json!({
            "account": response.get("account").cloned().unwrap_or_else(|| json!({})),
            "requiresOpenaiAuth": response
                .get("requiresOpenaiAuth")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })),
        Err(error) if is_invalid_refresh_token_error_message(&error.to_string()) => Ok(json!({
            "account": {},
            "requiresOpenaiAuth": true,
        })),
        Err(error) => Err(error),
    }
}

pub(crate) async fn start_account_login(
    state: &AppState,
    profile_id: &str,
    params: &Value,
) -> Result<Value> {
    let login_type = require_string(params, "type")?;
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    match login_type.as_str() {
        "chatgpt" | "chatgptDeviceCode" => {}
        "apiKey" => {
            let api_key = params
                .get("apiKey")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| anyhow!("API key is required."))?;
            let client = app_server_client(state, profile_id).await?;
            state.quota_cache.lock().await.remove(&resolved_profile_id);
            return client
                .request(
                    "account/login/start",
                    json!({ "type": login_type, "apiKey": api_key }),
                )
                .await;
        }
        _ => anyhow::bail!("Invalid account login type."),
    }

    let client = app_server_client(state, profile_id).await?;
    state.quota_cache.lock().await.remove(&resolved_profile_id);
    client
        .request("account/login/start", json!({ "type": login_type }))
        .await
}

pub(crate) async fn cancel_account_login(
    state: &AppState,
    profile_id: &str,
    params: &Value,
) -> Result<Value> {
    let client = app_server_client(state, profile_id).await?;
    client
        .request(
            "account/login/cancel",
            json!({
                "loginId": require_string(params, "loginId")?
            }),
        )
        .await
}

pub(crate) async fn logout_account(state: &AppState, profile_id: &str) -> Result<Value> {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    let client = app_server_client(state, profile_id).await?;
    state.quota_cache.lock().await.remove(&resolved_profile_id);
    client.request("account/logout", json!({})).await
}

async fn fetch_codex_quota(state: &AppState, profile_id: &str) -> Result<Value> {
    let profile = resolve_runtime_profile(&state.config, profile_id);
    let auth = read_codex_auth(&profile.codex_home)?;
    let access_token = auth
        .tokens
        .as_ref()
        .and_then(|tokens| tokens.access_token.as_deref())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("No access token found in CODEX_HOME auth.json."))?;
    let account_id = auth
        .tokens
        .as_ref()
        .and_then(|tokens| tokens.account_id.as_deref())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("No account id found in CODEX_HOME auth.json."))?;

    let response = state
        .http
        .get(CODEX_USAGE_URL)
        .timeout(QUOTA_REQUEST_TIMEOUT)
        .header("authorization", format!("Bearer {access_token}"))
        .header("chatgpt-account-id", account_id)
        .header("user-agent", CODEX_USAGE_USER_AGENT)
        .send()
        .await
        .context("failed to fetch Codex quota")?;

    if response.status() == StatusCode::UNAUTHORIZED {
        anyhow::bail!("Codex quota token expired. Re-authenticate Codex and refresh quota.");
    }

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!(if body.trim().is_empty() {
            format!("Codex quota request failed with {status}.")
        } else {
            format!("Codex quota request failed with {status}: {}", body.trim())
        });
    }

    let payload: UsageResponseShape = response
        .json()
        .await
        .context("invalid Codex quota response")?;
    let five_hour = normalize_quota_window(
        payload
            .rate_limit
            .as_ref()
            .and_then(|rate_limit| rate_limit.primary_window.as_ref()),
    );
    let weekly = normalize_quota_window(
        payload
            .rate_limit
            .as_ref()
            .and_then(|rate_limit| rate_limit.secondary_window.as_ref()),
    );

    Ok(json!({
        "available": five_hour.is_some() || weekly.is_some(),
        "source": "backend-api",
        "fetchedAt": now_unix_ms(),
        "account": payload.email,
        "plan": payload.plan_type,
        "fiveHour": five_hour,
        "weekly": weekly,
        "error": Value::Null,
    }))
}

fn read_codex_auth(codex_home: &PathBuf) -> Result<AuthFile> {
    let auth_path = codex_home.join("auth.json");
    let raw = fs::read_to_string(&auth_path)
        .with_context(|| format!("missing Codex auth file at {}.", auth_path.display()))?;
    serde_json::from_str(&raw).context("invalid Codex auth.json")
}

fn normalize_quota_window(window: Option<&UsageWindowShape>) -> Option<Value> {
    let window = window?;
    let used_percent = (window.used_percent.unwrap_or(0.0))
        .clamp(0.0, 100.0)
        .round() as u64;
    let reset_after_seconds = window
        .reset_after_seconds
        .filter(|value| *value > 0)
        .map(|value| value as u64);
    let reset_at = reset_after_seconds
        .map(|seconds| now_unix_ms().saturating_add(seconds.saturating_mul(1000)));

    Some(json!({
        "usedPercent": used_percent,
        "remainingPercent": 100_u64.saturating_sub(used_percent),
        "resetAfterSeconds": reset_after_seconds,
        "resetAt": reset_at,
    }))
}

pub(crate) fn is_invalid_refresh_token_error_message(message: &str) -> bool {
    let lowered = message.to_ascii_lowercase();
    lowered.contains("tokenrefreshfailed") || lowered.contains("invalid refresh token")
}

pub(crate) fn normalize_token_usage_payload(value: Option<&Value>) -> Value {
    let Some(record) = value.and_then(Value::as_object) else {
        return Value::Null;
    };

    let normalize_breakdown = |input: Option<&Value>| {
        let breakdown = input.and_then(Value::as_object);
        json!({
            "totalTokens": breakdown
                .and_then(|value| value.get("totalTokens"))
                .and_then(Value::as_u64)
                .unwrap_or(0),
            "inputTokens": breakdown
                .and_then(|value| value.get("inputTokens"))
                .and_then(Value::as_u64)
                .unwrap_or(0),
            "cachedInputTokens": breakdown
                .and_then(|value| value.get("cachedInputTokens"))
                .and_then(Value::as_u64)
                .unwrap_or(0),
            "outputTokens": breakdown
                .and_then(|value| value.get("outputTokens"))
                .and_then(Value::as_u64)
                .unwrap_or(0),
            "reasoningOutputTokens": breakdown
                .and_then(|value| value.get("reasoningOutputTokens"))
                .and_then(Value::as_u64)
                .unwrap_or(0)
        })
    };

    json!({
        "total": normalize_breakdown(record.get("total")),
        "last": normalize_breakdown(record.get("last")),
        "modelContextWindow": record
            .get("modelContextWindow")
            .and_then(Value::as_u64)
            .map(Value::from)
            .unwrap_or(Value::Null)
    })
}
