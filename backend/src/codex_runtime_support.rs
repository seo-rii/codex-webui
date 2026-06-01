use super::*;

const CODEX_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const CODEX_OAUTH_DEFAULT_ISSUER: &str = "https://auth.openai.com";
const ACCOUNT_LOGIN_FLOW_TTL: Duration = Duration::from_secs(15 * 60);
const ACCOUNT_APP_SERVER_REQUEST_TIMEOUT: Duration = Duration::from_millis(1_500);
const ACCOUNT_APP_SERVER_RECOVERY_RETRY_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) async fn codex_runtime_status(state: &AppState, check_latest: bool) -> Result<Value> {
    let configured_bin = state.config.codex_bin.clone();
    let resolved_bin_path = resolve_binary_path(&configured_bin).await;
    let npm_available = command_available(npm_command()).await;
    let install_command = format!("npm install -g {CODEX_NPM_PACKAGE}@latest");
    let update_command = install_command.clone();
    let webui_build_commit = option_env!("CODEX_WEBUI_BUILD_COMMIT").unwrap_or("unknown");
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
        "webuiVersion": env!("CARGO_PKG_VERSION"),
        "webuiBuildVersion": option_env!("CODEX_WEBUI_BUILD_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")),
        "webuiBuildCommit": webui_build_commit,
        "webuiBuildCommitShort": option_env!("CODEX_WEBUI_BUILD_COMMIT_SHORT").unwrap_or(webui_build_commit),
        "webuiBuildDirty": option_env!("CODEX_WEBUI_BUILD_DIRTY").unwrap_or("false") == "true",
        "webuiBuildTimestamp": option_env!("CODEX_WEBUI_BUILD_TIMESTAMP").unwrap_or("unknown"),
        "hostResources": host_resource_diagnostics_payload(),
        "issues": issues,
    }))
}

pub(crate) fn host_resource_diagnostics_payload() -> Value {
    let read_trimmed = |path: &str| {
        fs::read_to_string(path)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };
    let parse_u64 = |value: &str| value.parse::<u64>().ok();
    let read_u64 = |path: &str| read_trimmed(path).and_then(|value| parse_u64(&value));

    let memory_current_bytes = read_u64("/sys/fs/cgroup/memory.current")
        .or_else(|| read_u64("/sys/fs/cgroup/memory/memory.usage_in_bytes"));
    let memory_max_bytes = read_trimmed("/sys/fs/cgroup/memory.max")
        .and_then(|value| (value != "max").then(|| parse_u64(&value)).flatten())
        .or_else(|| read_u64("/sys/fs/cgroup/memory/memory.limit_in_bytes"));
    let proc_mem_total_bytes = fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|content| {
            content.lines().find_map(|line| {
                let rest = line.strip_prefix("MemTotal:")?.trim();
                let kib = rest.split_whitespace().next()?.parse::<u64>().ok()?;
                Some(kib.saturating_mul(1024))
            })
        });
    let mut memory_events = serde_json::Map::new();
    if let Some(content) = fs::read_to_string("/sys/fs/cgroup/memory.events").ok() {
        for line in content.lines() {
            let mut parts = line.split_whitespace();
            let Some(key) = parts.next() else {
                continue;
            };
            let Some(value) = parts.next().and_then(|value| value.parse::<u64>().ok()) else {
                continue;
            };
            memory_events.insert(key.to_string(), json!(value));
        }
    } else if let Some(fail_count) = read_u64("/sys/fs/cgroup/memory/memory.failcnt") {
        memory_events.insert("failcnt".to_string(), json!(fail_count));
    }
    let oom_count = memory_events.get("oom").and_then(Value::as_u64);
    let oom_kill_count = memory_events
        .get("oom_kill")
        .and_then(Value::as_u64)
        .or_else(|| memory_events.get("failcnt").and_then(Value::as_u64));
    let memory_usage_ratio =
        memory_current_bytes
            .zip(memory_max_bytes)
            .and_then(|(current, max)| {
                (max > 0 && current <= max).then_some((current as f64) / (max as f64))
            });

    json!({
        "memoryCurrentBytes": memory_current_bytes,
        "memoryMaxBytes": memory_max_bytes,
        "memoryUsageRatio": memory_usage_ratio,
        "procMemTotalBytes": proc_mem_total_bytes,
        "oomCount": oom_count,
        "oomKillCount": oom_kill_count,
        "memoryEvents": memory_events
    })
}

pub(crate) async fn codex_runtime_processes_payload(
    state: &AppState,
    profile_id: &str,
) -> Result<Value> {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    let configured_profiles = state
        .config
        .profiles
        .iter()
        .map(|(profile_id, profile)| AppServerProfile {
            id: profile_id.clone(),
            codex_home: profile.codex_home.clone(),
        })
        .collect::<Vec<_>>();
    let snapshots = state
        .app_servers
        .process_snapshots_for_profiles(configured_profiles)
        .await;
    let mut profile_ids = snapshots
        .iter()
        .map(|snapshot| snapshot.profile_id.clone())
        .collect::<HashSet<_>>();
    profile_ids.insert(resolved_profile_id.clone());

    let mut sessions_by_profile: HashMap<String, HashMap<String, Value>> = HashMap::new();
    for process_profile_id in &profile_ids {
        sessions_by_profile.insert(process_profile_id.clone(), HashMap::new());
    }

    {
        let active_turns = state.active_turns.lock().await;
        for process_profile_id in &profile_ids {
            let prefix = format!("profile::{process_profile_id}::session-runtime::");
            for (runtime_key, turn_id) in active_turns.iter() {
                let Some(session_id) = runtime_key.strip_prefix(&prefix) else {
                    continue;
                };
                sessions_by_profile
                    .entry(process_profile_id.clone())
                    .or_default()
                    .insert(
                        session_id.to_string(),
                        json!({
                            "sessionId": session_id,
                            "title": Value::Null,
                            "status": "running",
                            "turnId": turn_id,
                            "source": "activeTurn"
                        }),
                    );
            }
        }
    }

    {
        let pending_turn_starts = state.pending_turn_starts.lock().await;
        for process_profile_id in &profile_ids {
            let prefix = format!("profile::{process_profile_id}::session-runtime::");
            for runtime_key in pending_turn_starts.iter() {
                let Some(session_id) = runtime_key.strip_prefix(&prefix) else {
                    continue;
                };
                sessions_by_profile
                    .entry(process_profile_id.clone())
                    .or_default()
                    .entry(session_id.to_string())
                    .or_insert_with(|| {
                        json!({
                            "sessionId": session_id,
                            "title": Value::Null,
                            "status": "starting",
                            "turnId": Value::Null,
                            "source": "pendingStart"
                        })
                    });
            }
        }
    }

    for process_profile_id in profile_ids.clone() {
        let live_runtime_sessions = with_ui_state_read(state, &process_profile_id, |ui_state| {
            let metadata_by_thread_id = ui_state
                .get("sessionMetaByThreadId")
                .and_then(Value::as_object);
            let sessions = ui_state
                .get("runtimeStatusByThreadId")
                .and_then(Value::as_object)
                .map(|statuses| {
                    statuses
                        .iter()
                        .filter_map(|(session_id, status_value)| {
                            let status = normalized_thread_status(Some(status_value))?;
                            if !is_live_thread_status(&status) && status != "starting" {
                                return None;
                            }
                            let metadata = metadata_by_thread_id
                                .and_then(|entries| entries.get(session_id))
                                .and_then(Value::as_object);
                            let title = metadata
                                .and_then(|entry| entry.get("title"))
                                .and_then(Value::as_str)
                                .map(str::trim)
                                .filter(|value| !value.is_empty())
                                .map(str::to_string);
                            Some(json!({
                                "sessionId": session_id,
                                "title": title,
                                "status": status,
                                "turnId": status_value.get("turnId").cloned().unwrap_or(Value::Null),
                                "updatedAt": status_value.get("updatedAt").cloned().unwrap_or(Value::Null),
                                "reason": status_value.get("reason").cloned().unwrap_or(Value::Null),
                                "source": "runtimeStatus"
                            }))
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            Ok(sessions)
        })
        .await
        .unwrap_or_default();

        for session in live_runtime_sessions {
            let Some(session_id) = session.get("sessionId").and_then(Value::as_str) else {
                continue;
            };
            let entries = sessions_by_profile
                .entry(process_profile_id.clone())
                .or_default();
            if let Some(existing) = entries.get_mut(session_id) {
                if existing.get("title").and_then(Value::as_str).is_none() {
                    existing["title"] = session.get("title").cloned().unwrap_or(Value::Null);
                }
                if existing.get("updatedAt").is_none() {
                    existing["updatedAt"] =
                        session.get("updatedAt").cloned().unwrap_or(Value::Null);
                }
                if existing.get("reason").is_none() {
                    existing["reason"] = session.get("reason").cloned().unwrap_or(Value::Null);
                }
            } else {
                entries.insert(session_id.to_string(), session);
            }
        }
    }

    let mut processes = Vec::new();
    for snapshot in &snapshots {
        let process_session_ids =
            session_ids_for_app_server_client(state, &snapshot.profile_id, &snapshot.client_key)
                .await;
        let mut sessions = sessions_by_profile
            .get(&snapshot.profile_id)
            .map(|entries| {
                entries
                    .values()
                    .filter(|session| {
                        let Some(session_id) = session.get("sessionId").and_then(Value::as_str)
                        else {
                            return false;
                        };
                        process_session_ids.contains(session_id)
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        sessions.sort_by(|left, right| {
            left.get("sessionId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .cmp(
                    right
                        .get("sessionId")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                )
        });
        processes.push(codex_process_snapshot_payload(snapshot, sessions));
    }

    Ok(json!({
        "processes": processes,
        "activeProfileId": resolved_profile_id,
        "fetchedAt": now_unix_ms(),
    }))
}

pub(crate) async fn force_kill_codex_process_payload(
    state: &AppState,
    profile_id: &str,
    params: Value,
) -> Result<Value> {
    let target_profile_id = params
        .get("profileId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(profile_id);
    let pid = params
        .get("pid")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0 && *value <= u32::MAX as u64)
        .ok_or_else(|| anyhow!("Codex process id is required."))? as u32;
    let (resolved_profile_id, profile) =
        resolve_runtime_profile_entry(&state.config, target_profile_id);
    let snapshot = state
        .app_servers
        .force_kill_process(
            AppServerProfile {
                id: resolved_profile_id.to_string(),
                codex_home: profile.codex_home.clone(),
            },
            pid,
        )
        .await?;
    let reason = format!("Codex process {pid} was force killed from settings.");
    let affected_session_ids = clear_runtime_activity_after_app_server_client_exit(
        state,
        resolved_profile_id,
        &snapshot.client_key,
        Some(&reason),
    )
    .await;

    Ok(json!({
        "ok": true,
        "process": codex_process_snapshot_payload(&snapshot, Vec::new()),
        "affectedSessionIds": affected_session_ids,
    }))
}

fn codex_process_snapshot_payload(
    snapshot: &AppServerProcessSnapshot,
    sessions: Vec<Value>,
) -> Value {
    let session_count = sessions.len();
    json!({
        "clientKey": snapshot.client_key.clone(),
        "profileId": snapshot.profile_id.clone(),
        "codexHome": snapshot.codex_home.display().to_string(),
        "pid": snapshot.pid,
        "kind": snapshot.kind.clone(),
        "handoffProxy": snapshot.handoff_proxy,
        "socketPath": snapshot.socket_path.as_ref().map(|path| path.display().to_string()),
        "logPath": snapshot.log_path.as_ref().map(|path| path.display().to_string()),
        "startedAtMs": snapshot.started_at_ms,
        "codexBin": snapshot.codex_bin.clone(),
        "pendingRequestCount": snapshot.pending_request_count,
        "sessions": sessions,
        "sessionCount": session_count,
    })
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
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    let cached_payload = {
        let cache = state.quota_cache.lock().await;
        if let Some(cached) = cache.get(&resolved_profile_id) {
            let ttl = if refresh {
                QUOTA_FORCE_MIN_REFRESH_INTERVAL
            } else {
                QUOTA_CACHE_TTL
            };
            if cached.created_at.elapsed() < ttl {
                return Ok(cached.payload.clone());
            }
            Some(cached.payload.clone())
        } else {
            None
        }
    };

    {
        let mut refreshes = state.quota_refreshes.lock().await;
        if !refreshes.insert(resolved_profile_id.clone()) {
            return Ok(cached_payload
                .map(|mut payload| {
                    if let Some(object) = payload.as_object_mut() {
                        object.insert("refreshing".to_string(), Value::Bool(true));
                    }
                    payload
                })
                .unwrap_or_else(|| {
                    json!({
                        "available": false,
                        "source": Value::Null,
                        "fetchedAt": now_unix_ms(),
                        "account": Value::Null,
                        "plan": Value::Null,
                        "fiveHour": Value::Null,
                        "weekly": Value::Null,
                        "refreshing": true,
                        "error": Value::Null,
                    })
                }));
        }
    }

    let payload = match fetch_codex_quota(state, &resolved_profile_id).await {
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
    state
        .quota_refreshes
        .lock()
        .await
        .remove(&resolved_profile_id);

    let mut cache = state.quota_cache.lock().await;
    cache.insert(
        resolved_profile_id,
        CachedQuota {
            created_at: Instant::now(),
            payload: payload.clone(),
        },
    );

    Ok(payload)
}

pub(crate) async fn get_account_state(state: &AppState, profile_id: &str) -> Result<Value> {
    let client = app_server_client(state, profile_id).await?;
    let response = match client
        .request_with_timeout(
            "account/read",
            json!({ "refreshToken": false }),
            ACCOUNT_APP_SERVER_REQUEST_TIMEOUT,
            true,
        )
        .await
    {
        Ok(response) => response,
        Err(error)
            if app_server_timeout_recovered(&error) || app_server_request_interrupted(&error) =>
        {
            match client
                .request_with_timeout(
                    "account/read",
                    json!({ "refreshToken": false }),
                    if app_server_timeout_recovered(&error) {
                        ACCOUNT_APP_SERVER_RECOVERY_RETRY_TIMEOUT
                    } else {
                        ACCOUNT_APP_SERVER_REQUEST_TIMEOUT
                    },
                    false,
                )
                .await
            {
                Ok(response) => response,
                Err(retry_error)
                    if is_invalid_refresh_token_error_message(&retry_error.to_string()) =>
                {
                    return Ok(json!({
                        "account": {},
                        "requiresOpenaiAuth": true,
                    }));
                }
                Err(retry_error) if app_server_request_timed_out(&retry_error) => {
                    return Ok(json!({
                        "account": {},
                        "requiresOpenaiAuth": false,
                        "degraded": true,
                        "error": "Timed out while loading Codex account state."
                    }));
                }
                Err(retry_error) => return Err(retry_error),
            }
        }
        Err(error) if is_invalid_refresh_token_error_message(&error.to_string()) => {
            return Ok(json!({
                "account": {},
                "requiresOpenaiAuth": true,
            }));
        }
        Err(error) if app_server_request_timed_out(&error) => {
            return Ok(json!({
                "account": {},
                "requiresOpenaiAuth": false,
                "degraded": true,
                "error": "Timed out while loading Codex account state."
            }));
        }
        Err(error) => return Err(error),
    };

    Ok(json!({
            "account": response.get("account").cloned().unwrap_or_else(|| json!({})),
            "requiresOpenaiAuth": response
                .get("requiresOpenaiAuth")
                .and_then(Value::as_bool)
                .unwrap_or(false),
    }))
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

    if login_type == "chatgpt" {
        if let Some(browser_base_url) = params
            .get("browserBaseUrl")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return start_browser_account_login(state, &resolved_profile_id, browser_base_url)
                .await;
        }
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
    let login_id = require_string(params, "loginId")?;
    if state
        .account_login_flows
        .lock()
        .await
        .remove(&login_id)
        .is_some()
    {
        return Ok(json!({ "status": "canceled" }));
    }

    let client = app_server_client(state, profile_id).await?;
    client
        .request(
            "account/login/cancel",
            json!({
                "loginId": login_id
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

pub(crate) async fn complete_account_oauth_callback(
    state: &AppState,
    query: Option<&str>,
) -> Response {
    let Some(return_url) = complete_account_oauth_callback_inner(state, query).await else {
        return (
            StatusCode::BAD_REQUEST,
            "Invalid or expired account login flow.",
        )
            .into_response();
    };

    Redirect::temporary(&return_url).into_response()
}

async fn complete_account_oauth_callback_inner(
    state: &AppState,
    query: Option<&str>,
) -> Option<String> {
    let state_token = query_param_value(query, "state")?;
    let (login_id, flow) = take_account_login_flow(state, &state_token).await?;
    let return_url = flow.return_url.clone();

    if let Some(error) = query_param_value(query, "error") {
        emit_account_login_completed(state, &flow.profile_id, &login_id, false, Some(error)).await;
        return Some(append_account_login_result(&return_url, "error"));
    }

    let code = match query_param_value(query, "code") {
        Some(code) if !code.trim().is_empty() => code,
        _ => {
            emit_account_login_completed(
                state,
                &flow.profile_id,
                &login_id,
                false,
                Some("Missing authorization code.".to_string()),
            )
            .await;
            return Some(append_account_login_result(&return_url, "error"));
        }
    };

    match exchange_account_oauth_code(state, &flow, &code).await {
        Ok(tokens) => match persist_managed_chatgpt_auth(state, &flow.profile_id, tokens).await {
            Ok(()) => {
                emit_account_login_completed(state, &flow.profile_id, &login_id, true, None).await;
                emit_profile_global_notification(
                    state,
                    &flow.profile_id,
                    json!({
                        "kind": "notification",
                        "method": "codex-webui/accountUpdated",
                        "params": {}
                    }),
                )
                .await;
                Some(append_account_login_result(&return_url, "success"))
            }
            Err(error) => {
                emit_account_login_completed(
                    state,
                    &flow.profile_id,
                    &login_id,
                    false,
                    Some(format!("Failed to save account credentials: {error}")),
                )
                .await;
                Some(append_account_login_result(&return_url, "error"))
            }
        },
        Err(error) => {
            emit_account_login_completed(
                state,
                &flow.profile_id,
                &login_id,
                false,
                Some(format!("OAuth token exchange failed: {error}")),
            )
            .await;
            Some(append_account_login_result(&return_url, "error"))
        }
    }
}

async fn start_browser_account_login(
    state: &AppState,
    profile_id: &str,
    browser_base_url: &str,
) -> Result<Value> {
    let return_url = normalize_browser_account_base_url(&state.config, browser_base_url)?;
    let redirect_uri = format!("{return_url}/api/account/oauth/callback");
    let (code_verifier, code_challenge) = generate_pkce_pair();
    let state_token = generate_oauth_state();
    let login_id = Uuid::new_v4().to_string();
    let auth_url = build_account_authorize_url(&redirect_uri, &code_challenge, &state_token);

    prune_account_login_flows(state).await;
    state.account_login_flows.lock().await.insert(
        login_id.clone(),
        PendingAccountLoginFlow {
            profile_id: profile_id.to_string(),
            state: state_token,
            code_verifier,
            redirect_uri,
            return_url,
            created_at: Instant::now(),
        },
    );

    state.quota_cache.lock().await.remove(profile_id);
    Ok(json!({
        "type": "chatgpt",
        "loginId": login_id,
        "authUrl": auth_url
    }))
}

fn normalize_browser_account_base_url(config: &Config, raw: &str) -> Result<String> {
    let url = reqwest::Url::parse(raw.trim()).context("Invalid browser base URL.")?;
    if !matches!(url.scheme(), "http" | "https") {
        anyhow::bail!("Browser base URL must use http or https.");
    }
    if !url.username().is_empty() || url.password().is_some() {
        anyhow::bail!("Browser base URL must not include credentials.");
    }
    if url.query().is_some() || url.fragment().is_some() {
        anyhow::bail!("Browser base URL must not include query or fragment.");
    }

    let expected_path = if state_base_path(config).is_empty() {
        ""
    } else {
        state_base_path(config).trim_end_matches('/')
    };
    let actual_path = url.path().trim_end_matches('/');
    if expected_path.is_empty() {
        if !actual_path.is_empty() {
            anyhow::bail!("Browser base URL path must match the configured base path.");
        }
    } else if actual_path != expected_path {
        anyhow::bail!("Browser base URL path must match the configured base path.");
    }

    let origin = url.origin().ascii_serialization();
    if expected_path.is_empty() {
        Ok(origin)
    } else {
        Ok(format!("{origin}{expected_path}"))
    }
}

fn state_base_path(config: &Config) -> &str {
    config.base_path.as_str()
}

fn generate_oauth_state() -> String {
    let mut bytes = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rng(), &mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn generate_pkce_pair() -> (String, String) {
    let mut bytes = [0u8; 64];
    rand::RngCore::fill_bytes(&mut rand::rng(), &mut bytes);
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let challenge = URL_SAFE_NO_PAD.encode(<Sha256 as sha2::Digest>::digest(verifier.as_bytes()));
    (verifier, challenge)
}

fn account_login_issuer() -> String {
    env::var("CODEX_APP_SERVER_LOGIN_ISSUER")
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| CODEX_OAUTH_DEFAULT_ISSUER.to_string())
}

fn build_account_authorize_url(redirect_uri: &str, code_challenge: &str, state: &str) -> String {
    let query = [
        ("response_type", "code"),
        ("client_id", CODEX_OAUTH_CLIENT_ID),
        ("redirect_uri", redirect_uri),
        (
            "scope",
            "openid profile email offline_access api.connectors.read api.connectors.invoke",
        ),
        ("code_challenge", code_challenge),
        ("code_challenge_method", "S256"),
        ("id_token_add_organizations", "true"),
        ("codex_cli_simplified_flow", "true"),
        ("state", state),
        ("originator", "codex_cli_rs"),
    ]
    .into_iter()
    .map(|(key, value)| format!("{key}={}", urlencoding::encode(value)))
    .collect::<Vec<_>>()
    .join("&");
    format!("{}/oauth/authorize?{query}", account_login_issuer())
}

async fn prune_account_login_flows(state: &AppState) {
    let now = Instant::now();
    state
        .account_login_flows
        .lock()
        .await
        .retain(|_, flow| now.duration_since(flow.created_at) <= ACCOUNT_LOGIN_FLOW_TTL);
}

async fn take_account_login_flow(
    state: &AppState,
    state_token: &str,
) -> Option<(String, PendingAccountLoginFlow)> {
    prune_account_login_flows(state).await;
    let mut flows = state.account_login_flows.lock().await;
    let login_id = flows
        .iter()
        .find_map(|(login_id, flow)| (flow.state == state_token).then(|| login_id.clone()))?;
    flows.remove_entry(&login_id)
}

#[derive(Debug)]
struct ExchangedAccountTokens {
    id_token: String,
    access_token: String,
    refresh_token: String,
}

#[derive(Deserialize)]
struct AccountTokenResponse {
    id_token: String,
    access_token: String,
    refresh_token: String,
}

async fn exchange_account_oauth_code(
    state: &AppState,
    flow: &PendingAccountLoginFlow,
    code: &str,
) -> Result<ExchangedAccountTokens> {
    let response = state
        .http
        .post(format!("{}/oauth/token", account_login_issuer()))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(format!(
            "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&code_verifier={}",
            urlencoding::encode(code),
            urlencoding::encode(&flow.redirect_uri),
            urlencoding::encode(CODEX_OAUTH_CLIENT_ID),
            urlencoding::encode(&flow.code_verifier)
        ))
        .send()
        .await
        .context("failed to send OAuth token exchange request")?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("token endpoint returned {status}: {body}");
    }
    let tokens = response
        .json::<AccountTokenResponse>()
        .await
        .context("failed to decode OAuth token response")?;
    Ok(ExchangedAccountTokens {
        id_token: tokens.id_token,
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
    })
}

async fn persist_managed_chatgpt_auth(
    state: &AppState,
    profile_id: &str,
    tokens: ExchangedAccountTokens,
) -> Result<()> {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    let profile = resolve_runtime_profile(&state.config, profile_id);
    let id_claims = jwt_payload_value(&tokens.id_token)?;
    let access_claims = jwt_payload_value(&tokens.access_token).unwrap_or_else(|_| json!({}));
    let account_id = jwt_claim_string(
        &id_claims,
        &["https://api.openai.com/auth", "chatgpt_account_id"],
    )
    .or_else(|| jwt_claim_string(&access_claims, &["chatgpt_account_id"]))
    .ok_or_else(|| anyhow!("OAuth response did not include a ChatGPT account id."))?;

    let auth = json!({
        "auth_mode": "chatgpt",
        "OPENAI_API_KEY": Value::Null,
        "tokens": {
            "id_token": tokens.id_token,
            "access_token": tokens.access_token,
            "refresh_token": tokens.refresh_token,
            "account_id": account_id
        }
    });
    let bytes = serde_json::to_vec_pretty(&auth)?;
    write_file_atomically(&profile.codex_home.join("auth.json"), bytes).await?;
    state.quota_cache.lock().await.remove(&resolved_profile_id);
    state
        .app_servers
        .close_profile(&resolved_profile_id)
        .await?;
    Ok(())
}

fn jwt_payload_value(jwt: &str) -> Result<Value> {
    let mut parts = jwt.split('.');
    let (_header, payload, _signature) = match (parts.next(), parts.next(), parts.next()) {
        (Some(header), Some(payload), Some(signature))
            if !header.is_empty() && !payload.is_empty() && !signature.is_empty() =>
        {
            (header, payload, signature)
        }
        _ => anyhow::bail!("Invalid JWT format."),
    };
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .context("failed to decode JWT payload")?;
    serde_json::from_slice(&bytes).context("failed to parse JWT payload")
}

fn jwt_claim_string(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current.as_str().map(str::to_string)
}

async fn emit_account_login_completed(
    state: &AppState,
    profile_id: &str,
    login_id: &str,
    success: bool,
    error: Option<String>,
) {
    emit_profile_global_notification(
        state,
        profile_id,
        json!({
            "kind": "notification",
            "method": "codex-webui/accountLoginCompleted",
            "params": {
                "loginId": login_id,
                "success": success,
                "error": error
            }
        }),
    )
    .await;
}

fn append_account_login_result(return_url: &str, result: &str) -> String {
    let separator = if return_url.contains('?') { '&' } else { '?' };
    format!(
        "{return_url}{separator}accountLogin={}",
        urlencoding::encode(result)
    )
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
