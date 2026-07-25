use super::*;

const CODEX_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const CODEX_OAUTH_DEFAULT_ISSUER: &str = "https://auth.openai.com";
const ACCOUNT_LOGIN_FLOW_TTL: Duration = Duration::from_secs(15 * 60);
const ACCOUNT_APP_SERVER_REQUEST_TIMEOUT: Duration = Duration::from_millis(1_500);
const ACCOUNT_APP_SERVER_RECOVERY_RETRY_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_AUTH_JSON_IMPORT_BYTES: u64 = 512 * 1024;

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
    let (_, profiles_snapshot) = runtime_profiles_snapshot(&state.config);
    let configured_profiles = profiles_snapshot
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
        &resolved_profile_id,
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
                        "limits": [],
                        "windows": [],
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
            "limits": [],
            "windows": [],
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

pub(crate) async fn codex_profile_accounts_payload(
    state: &AppState,
    active_profile_id: &str,
    refresh: bool,
) -> Result<Value> {
    let resolved_active_profile_id =
        resolve_runtime_profile_entry(&state.config, active_profile_id)
            .0
            .to_string();
    let (_, profiles_snapshot) = runtime_profiles_snapshot(&state.config);
    let profiles = profiles_snapshot
        .iter()
        .map(|(profile_id, profile)| {
            (
                profile_id.clone(),
                profile.label.clone(),
                profile.codex_home.clone(),
            )
        })
        .collect::<Vec<_>>();

    let mut account_profiles = futures_util::stream::iter(profiles)
        .map(|(profile_id, label, codex_home)| {
            let state = state.clone();
            let resolved_active_profile_id = resolved_active_profile_id.clone();
            async move {
                let auth_path = codex_home.join("auth.json");
                let auth_payload = tokio_fs::read(&auth_path)
                    .await
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
                let auth_mode = auth_payload
                    .as_ref()
                    .and_then(|value| value.get("auth_mode").or_else(|| value.get("authMode")))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string);
                let has_api_key = auth_payload
                    .as_ref()
                    .and_then(|value| value.get("OPENAI_API_KEY"))
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.trim().is_empty());
                let has_chatgpt_tokens = auth_payload
                    .as_ref()
                    .and_then(|value| value.get("tokens"))
                    .and_then(Value::as_object)
                    .is_some_and(|tokens| {
                        tokens
                            .get("access_token")
                            .and_then(Value::as_str)
                            .is_some_and(|value| !value.trim().is_empty())
                            && tokens
                                .get("account_id")
                                .and_then(Value::as_str)
                                .is_some_and(|value| !value.trim().is_empty())
                    });
                let account_type = if has_api_key
                    || auth_mode
                        .as_deref()
                        .is_some_and(|value| value.eq_ignore_ascii_case("apikey"))
                {
                    Some("apiKey")
                } else if has_chatgpt_tokens
                    || auth_mode
                        .as_deref()
                        .is_some_and(|value| value.eq_ignore_ascii_case("chatgpt"))
                {
                    Some("chatgpt")
                } else {
                    None
                };
                let quota = codex_quota_status(&state, refresh, &profile_id)
                    .await
                    .unwrap_or_else(|error| {
                        json!({
                            "available": false,
                            "source": Value::Null,
                            "fetchedAt": now_unix_ms(),
                            "account": Value::Null,
                            "plan": Value::Null,
                            "fiveHour": Value::Null,
                            "weekly": Value::Null,
                            "error": error.to_string(),
                        })
                    });
                let active = profile_id == resolved_active_profile_id;
                let requires_openai_auth = account_type.is_none();

                json!({
                    "profileId": profile_id,
                    "label": label,
                    "codexHome": codex_home.display().to_string(),
                    "active": active,
                    "account": {
                        "type": account_type,
                        "email": quota.get("account").cloned().unwrap_or(Value::Null),
                        "planType": quota.get("plan").cloned().unwrap_or(Value::Null),
                        "requiresOpenaiAuth": requires_openai_auth
                    },
                    "quota": quota
                })
            }
        })
        .buffer_unordered(3)
        .collect::<Vec<_>>()
        .await;

    account_profiles.sort_by(|left, right| {
        right
            .get("active")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            .cmp(&left.get("active").and_then(Value::as_bool).unwrap_or(false))
            .then_with(|| {
                left.get("label")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .cmp(
                        right
                            .get("label")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    )
            })
    });

    Ok(json!({
        "profiles": account_profiles,
        "fetchedAt": now_unix_ms()
    }))
}

pub(crate) async fn codex_reset_tickets_payload(
    state: &AppState,
    profile_id: &str,
    refresh: bool,
) -> Result<Value> {
    let client = app_server_client(state, profile_id).await?;
    let response = match client
        .request_with_timeout(
            "account/rateLimits/read",
            Value::Null,
            ACCOUNT_APP_SERVER_RECOVERY_RETRY_TIMEOUT,
            false,
        )
        .await
    {
        Ok(response) => response,
        Err(error) => {
            return Ok(json!({
                "available": false,
                "supported": false,
                "availableCount": 0,
                "tickets": [],
                "rateLimits": Value::Null,
                "rateLimitsByLimitId": Value::Null,
                "fetchedAt": now_unix_ms(),
                "refresh": refresh,
                "message": "This Codex build did not return reset-ticket information.",
                "error": error.to_string(),
            }));
        }
    };
    let (mut tickets, saw_reset_ticket_field) = extract_codex_reset_tickets(&response);
    let reset_credit_count = extract_rate_limit_reset_credit_count(&response);
    if tickets.is_empty() {
        if let Some(count) = reset_credit_count.filter(|count| *count > 0) {
            let visible_count = count.min(20);
            tickets.extend((0..visible_count).map(|index| {
                json!({
                    "id": format!("rate-limit-reset-credit-{}", index + 1),
                    "label": if count == 1 {
                        "Earned reset credit".to_string()
                    } else {
                        format!("Earned reset credit {} of {}", index + 1, count)
                    },
                    "limitId": Value::Null,
                    "limitName": "Codex",
                    "expiresAt": Value::Null,
                    "expirationStatus": "unknown",
                    "createdAt": Value::Null,
                    "usedAt": Value::Null,
                    "available": true,
                    "raw": {
                        "rateLimitResetCredits": {
                            "availableCount": count
                        }
                    },
                })
            }));
        }
    }

    Ok(json!({
        "available": true,
        "supported": saw_reset_ticket_field || reset_credit_count.is_some(),
        "availableCount": reset_credit_count.unwrap_or(tickets.iter().filter(|ticket| {
            ticket.get("available").and_then(Value::as_bool).unwrap_or(false)
        }).count() as i64),
        "tickets": tickets,
        "rateLimits": response.get("rateLimits").cloned().unwrap_or(Value::Null),
        "rateLimitsByLimitId": response
            .get("rateLimitsByLimitId")
            .or_else(|| response.get("rate_limits_by_limit_id"))
            .cloned()
            .unwrap_or(Value::Null),
        "fetchedAt": now_unix_ms(),
        "refresh": refresh,
        "message": Value::Null,
        "error": Value::Null,
    }))
}

pub(crate) async fn use_codex_reset_ticket_payload(
    state: &AppState,
    profile_id: &str,
    params: Value,
) -> Result<Value> {
    let ticket_id = require_string(&params, "ticketId")?;
    let limit_id = params
        .get("limitId")
        .or_else(|| params.get("limit_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let client = app_server_client(state, profile_id).await?;
    let idempotency_key = params
        .get("idempotencyKey")
        .or_else(|| params.get("idempotency_key"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let mut consume_payload = json!({ "idempotencyKey": idempotency_key });
    if !ticket_id.starts_with("rate-limit-reset-credit-") {
        consume_payload["creditId"] = Value::String(ticket_id.clone());
    }
    match client
        .request_with_timeout(
            "account/rateLimitResetCredit/consume",
            consume_payload,
            ACCOUNT_APP_SERVER_RECOVERY_RETRY_TIMEOUT,
            false,
        )
        .await
    {
        Ok(result) => {
            let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
                .0
                .to_string();
            state.quota_cache.lock().await.remove(&resolved_profile_id);
            return Ok(json!({
                "ok": true,
                "method": "account/rateLimitResetCredit/consume",
                "ticketId": ticket_id,
                "limitId": limit_id,
                "idempotencyKey": idempotency_key,
                "outcome": result.get("outcome").cloned().unwrap_or(Value::Null),
                "result": result,
            }));
        }
        Err(error) if !app_server_method_unsupported_error(&error) => return Err(error),
        Err(_) => {}
    }

    let mut payload = json!({ "ticketId": ticket_id });
    if let Some(limit_id) = limit_id.as_deref() {
        payload["limitId"] = json!(limit_id);
    }

    let mut failures = Vec::new();
    for method in [
        "account/rateLimits/resetTicket/use",
        "account/resetTickets/use",
        "account/resetTicket/use",
        "account/rateLimitResetTicket/use",
    ] {
        match client
            .request_with_timeout(
                method,
                payload.clone(),
                ACCOUNT_APP_SERVER_RECOVERY_RETRY_TIMEOUT,
                false,
            )
            .await
        {
            Ok(result) => {
                let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
                    .0
                    .to_string();
                state.quota_cache.lock().await.remove(&resolved_profile_id);
                return Ok(json!({
                    "ok": true,
                    "method": method,
                    "ticketId": ticket_id,
                    "limitId": limit_id,
                    "result": result,
                }));
            }
            Err(error) => failures.push(format!("{method}: {error}")),
        }
    }

    anyhow::bail!(
        "Codex reset-ticket use is not exposed by this Codex version. {}",
        failures.join("; ")
    );
}

fn app_server_method_unsupported_error(error: &anyhow::Error) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    message.contains("unknown method")
        || message.contains("method not found")
        || message.contains("not implemented")
        || message.contains("unsupported method")
}

fn extract_codex_reset_tickets(response: &Value) -> (Vec<Value>, bool) {
    let mut entries = Vec::new();
    let saw_reset_ticket_field = collect_reset_ticket_entries(response, &mut entries);
    let tickets = entries
        .into_iter()
        .enumerate()
        .filter_map(|(index, (value, fallback_id))| {
            normalize_reset_ticket_entry(value, fallback_id, index)
        })
        .collect();
    (tickets, saw_reset_ticket_field)
}

fn extract_rate_limit_reset_credit_count(response: &Value) -> Option<i64> {
    response
        .get("rateLimitResetCredits")
        .or_else(|| response.get("rate_limit_reset_credits"))
        .and_then(|value| {
            value
                .get("availableCount")
                .or_else(|| value.get("available_count"))
                .and_then(Value::as_i64)
        })
}

fn collect_reset_ticket_entries(value: &Value, entries: &mut Vec<(Value, Option<String>)>) -> bool {
    match value {
        Value::Array(items) => {
            let mut found = false;
            for item in items {
                found |= collect_reset_ticket_entries(item, entries);
            }
            found
        }
        Value::Object(object) => {
            let mut found = false;
            for (key, child) in object {
                let normalized_key = key
                    .chars()
                    .filter(|ch| ch.is_ascii_alphanumeric())
                    .collect::<String>()
                    .to_ascii_lowercase();
                if normalized_key == "ratelimitresetcredits" {
                    if let Some(credits) = child
                        .get("credits")
                        .or_else(|| child.get("resetCredits"))
                        .or_else(|| child.get("reset_credits"))
                    {
                        collect_reset_ticket_field(credits, entries);
                    }
                    found = true;
                } else if normalized_key.contains("resetticket") {
                    collect_reset_ticket_field(child, entries);
                    found = true;
                } else {
                    found |= collect_reset_ticket_entries(child, entries);
                }
            }
            found
        }
        _ => false,
    }
}

fn collect_reset_ticket_field(value: &Value, entries: &mut Vec<(Value, Option<String>)>) {
    match value {
        Value::Array(items) => {
            for item in items {
                entries.push((item.clone(), None));
            }
        }
        Value::Object(object) => {
            if object
                .keys()
                .any(|key| matches!(key.as_str(), "id" | "ticketId" | "ticket_id"))
            {
                entries.push((value.clone(), None));
            } else {
                for (key, child) in object {
                    entries.push((child.clone(), Some(key.clone())));
                }
            }
        }
        Value::String(_) => entries.push((value.clone(), None)),
        _ => {}
    }
}

fn normalize_reset_ticket_entry(
    value: Value,
    fallback_id: Option<String>,
    index: usize,
) -> Option<Value> {
    match value {
        Value::String(ticket_id) => {
            let ticket_id = ticket_id.trim();
            if ticket_id.is_empty() {
                return None;
            }
            Some(json!({
                "id": ticket_id,
                "label": Value::Null,
                "limitId": Value::Null,
                "limitName": Value::Null,
                "expiresAt": Value::Null,
                "expirationStatus": "unknown",
                "createdAt": Value::Null,
                "usedAt": Value::Null,
                "available": true,
                "raw": ticket_id,
            }))
        }
        Value::Object(object) => {
            let string_field = |names: &[&str]| {
                names.iter().find_map(|name| {
                    object
                        .get(*name)
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string)
                })
            };
            let timestamp_field = |names: &[&str]| {
                names.iter().find_map(|name| {
                    object.get(*name).and_then(|value| match value {
                        Value::String(value) if !value.trim().is_empty() => {
                            Some(Value::String(value.trim().to_string()))
                        }
                        Value::Number(value) => {
                            value.as_i64().filter(|value| *value > 0).map(|value| {
                                let value = value as u64;
                                Value::Number(
                                    (if value >= 10_000_000_000 {
                                        value
                                    } else {
                                        value.saturating_mul(1000)
                                    })
                                    .into(),
                                )
                            })
                        }
                        _ => None,
                    })
                })
            };
            let ticket_id = string_field(&[
                "id",
                "ticketId",
                "ticket_id",
                "resetTicketId",
                "reset_ticket_id",
            ])
            .or(fallback_id)
            .unwrap_or_else(|| format!("ticket-{}", index + 1));
            let used_at = timestamp_field(&["usedAt", "used_at", "redeemedAt", "redeemed_at"]);
            let expiration_field_names = ["expiresAt", "expires_at", "expiration", "expires"];
            let expiration_field_present = expiration_field_names
                .iter()
                .any(|name| object.contains_key(*name));
            let expires_at = timestamp_field(&expiration_field_names);
            let expiration_status = if expires_at.is_some() {
                "expires"
            } else if expiration_field_present {
                "never"
            } else {
                "unknown"
            };
            let status = string_field(&["status"]);
            let available = object
                .get("available")
                .or_else(|| object.get("active"))
                .and_then(Value::as_bool)
                .unwrap_or_else(|| {
                    status
                        .as_deref()
                        .map(|status| status.eq_ignore_ascii_case("available"))
                        .unwrap_or(used_at.is_none())
                });
            Some(json!({
                "id": ticket_id,
                "label": string_field(&["label", "name", "title", "description"]),
                "limitId": string_field(&["limitId", "limit_id", "rateLimitId", "rate_limit_id"]),
                "limitName": string_field(&["limitName", "limit_name", "rateLimitName", "rate_limit_name"]),
                "resetType": string_field(&["resetType", "reset_type"]),
                "status": status,
                "expiresAt": expires_at,
                "expirationStatus": expiration_status,
                "createdAt": timestamp_field(&["createdAt", "created_at", "grantedAt", "granted_at"]),
                "usedAt": used_at,
                "available": available,
                "raw": Value::Object(object),
            }))
        }
        _ => None,
    }
}

pub(crate) async fn get_account_state(state: &AppState, profile_id: &str) -> Result<Value> {
    let client = app_server_client(state, profile_id).await?;
    let response = match client
        .request_with_timeout(
            "account/read",
            json!({ "refreshToken": false }),
            ACCOUNT_APP_SERVER_REQUEST_TIMEOUT,
            false,
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
        "authJsonFile" => {
            let auth_json_path = params
                .get("authJsonPath")
                .or_else(|| params.get("credentialsJsonPath"))
                .or_else(|| params.get("path"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| anyhow!("Credentials JSON file path is required."))?;
            let create_profile = params
                .get("createProfile")
                .or_else(|| params.get("create_profile"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let profile_label = params
                .get("profileLabel")
                .or_else(|| params.get("profile_label"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let profile_id_hint = params
                .get("profileId")
                .or_else(|| params.get("profile_id"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let import_result = import_account_auth_json_file(
                state,
                profile_id,
                auth_json_path,
                create_profile,
                profile_label,
                profile_id_hint,
            )
            .await?;
            emit_profile_global_notification(
                state,
                &resolved_profile_id,
                json!({
                    "kind": "notification",
                    "method": "codex-webui/accountUpdated",
                    "params": {}
                }),
            )
            .await;
            return Ok(json!({
                "type": "authJsonFile",
                "imported": true,
                "profile": import_result.get("profile").cloned().unwrap_or(Value::Null),
                "restartRequired": import_result
                    .get("restartRequired")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                "configPath": import_result.get("configPath").cloned().unwrap_or(Value::Null)
            }));
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

async fn import_account_auth_json_file(
    state: &AppState,
    profile_id: &str,
    raw_path: &str,
    create_profile: bool,
    profile_label: Option<&str>,
    profile_id_hint: Option<&str>,
) -> Result<Value> {
    let input_path = expand_server_auth_json_path(raw_path);
    let metadata = tokio_fs::metadata(&input_path).await.with_context(|| {
        format!(
            "failed to read credentials file metadata at {}",
            input_path.display()
        )
    })?;
    if !metadata.is_file() {
        anyhow::bail!("Credentials JSON path must point to a file.");
    }
    if metadata.len() > MAX_AUTH_JSON_IMPORT_BYTES {
        anyhow::bail!(
            "Credentials JSON file is too large. Limit is {} KiB.",
            MAX_AUTH_JSON_IMPORT_BYTES / 1024
        );
    }
    let bytes = tokio_fs::read(&input_path).await.with_context(|| {
        format!(
            "failed to read credentials file at {}",
            input_path.display()
        )
    })?;
    let parsed: Value =
        serde_json::from_slice(&bytes).context("credentials file must be valid JSON")?;
    if !parsed.is_object() {
        anyhow::bail!("Credentials JSON file must contain a JSON object.");
    }
    let pretty = serde_json::to_vec_pretty(&parsed)?;
    if create_profile {
        let profile_payload = create_profile_from_auth_json_import(
            state,
            &input_path,
            &pretty,
            profile_label,
            profile_id_hint,
            &parsed,
        )
        .await?;
        return Ok(json!({
            "profile": profile_payload,
            "restartRequired": false,
            "configPath": codex_webui_config_path(state).display().to_string()
        }));
    }

    let profile = resolve_runtime_profile(&state.config, profile_id);
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    write_file_atomically(&profile.codex_home.join("auth.json"), pretty).await?;
    state.quota_cache.lock().await.remove(&resolved_profile_id);
    state
        .app_servers
        .close_profile(&resolved_profile_id)
        .await?;
    Ok(json!({
        "profile": {
            "id": resolved_profile_id,
            "label": profile.label,
            "codexHome": profile.codex_home.display().to_string(),
            "active": true
        },
        "restartRequired": false,
        "configPath": Value::Null
    }))
}

fn expand_server_auth_json_path(raw_path: &str) -> PathBuf {
    let trimmed = raw_path.trim();
    let expanded = if trimmed == "~" {
        home_dir_path().unwrap_or_else(|| PathBuf::from(trimmed))
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        home_dir_path()
            .map(|home| home.join(rest))
            .unwrap_or_else(|| PathBuf::from(trimmed))
    } else {
        PathBuf::from(trimmed)
    };
    normalize_path(expanded)
}

fn codex_webui_config_path(state: &AppState) -> PathBuf {
    state.config.config_file_path.clone().unwrap_or_else(|| {
        home_dir_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".codex")
            .join("codex-webui.yml")
    })
}

async fn read_codex_webui_config_yaml(state: &AppState) -> Result<(PathBuf, yaml_rust2::Yaml)> {
    use yaml_rust2::YamlLoader;
    use yaml_rust2::yaml::{Hash as YamlHash, Yaml};

    let config_path = codex_webui_config_path(state);
    let raw_config = tokio_fs::read_to_string(&config_path)
        .await
        .unwrap_or_default();
    let config_value = if raw_config.trim().is_empty() {
        Yaml::Hash(YamlHash::new())
    } else {
        YamlLoader::load_from_str(&raw_config)
            .with_context(|| format!("failed to parse {}", config_path.display()))?
            .into_iter()
            .next()
            .unwrap_or_else(|| Yaml::Hash(YamlHash::new()))
    };
    if !config_value.is_hash() {
        anyhow::bail!("codex-webui.yml must contain a YAML object to edit profiles.");
    }
    Ok((config_path, config_value))
}

fn profile_yaml_entries_from_state(state: &AppState) -> Vec<yaml_rust2::Yaml> {
    use yaml_rust2::yaml::{Hash as YamlHash, Yaml};

    state
        .config
        .profiles
        .iter()
        .map(|(profile_id, profile)| {
            let mut entry = YamlHash::new();
            entry.insert(
                Yaml::String("id".to_string()),
                Yaml::String(profile_id.clone()),
            );
            entry.insert(
                Yaml::String("label".to_string()),
                Yaml::String(profile.label.clone()),
            );
            entry.insert(
                Yaml::String("codexHome".to_string()),
                Yaml::String(profile.codex_home.display().to_string()),
            );
            entry.insert(
                Yaml::String("dataDir".to_string()),
                Yaml::String(profile.data_dir.display().to_string()),
            );
            Yaml::Hash(entry)
        })
        .collect()
}

fn ensure_profile_yaml_array(config_value: &mut yaml_rust2::Yaml, state: &AppState) -> Result<()> {
    use yaml_rust2::Yaml;

    let root = config_value
        .as_mut_hash()
        .ok_or_else(|| anyhow!("codex-webui.yml must contain a YAML object to edit profiles."))?;
    let profiles_key = Yaml::String("profiles".to_string());
    if !root.get(&profiles_key).is_some_and(Yaml::is_array) {
        root.insert(
            profiles_key,
            Yaml::Array(profile_yaml_entries_from_state(state)),
        );
    }
    Ok(())
}

fn profile_yaml_array_mut(
    config_value: &mut yaml_rust2::Yaml,
) -> Result<&mut Vec<yaml_rust2::Yaml>> {
    config_value["profiles"]
        .as_mut_vec()
        .ok_or_else(|| anyhow!("codex-webui.yml profiles must be a YAML array."))
}

fn profile_id_from_yaml(profile: &yaml_rust2::Yaml) -> Option<String> {
    profile["id"].as_str().map(sanitize_profile_id)
}

async fn write_codex_webui_config_yaml(
    config_path: &Path,
    config_value: &yaml_rust2::Yaml,
) -> Result<()> {
    use yaml_rust2::YamlEmitter;

    let mut encoded = String::new();
    YamlEmitter::new(&mut encoded)
        .dump(config_value)
        .map_err(|error| anyhow!("failed to encode codex-webui.yml: {error}"))?;
    encoded.push('\n');
    write_file_atomically(config_path, encoded.into_bytes()).await?;
    invalidate_runtime_profiles_snapshot(config_path);
    Ok(())
}

fn default_import_profile_label(parsed: &Value, input_path: &Path) -> String {
    parsed
        .get("account")
        .and_then(Value::as_object)
        .and_then(|account| account.get("email"))
        .and_then(Value::as_str)
        .or_else(|| {
            parsed
                .get("tokens")
                .and_then(Value::as_object)
                .and_then(|tokens| tokens.get("email"))
                .and_then(Value::as_str)
        })
        .or_else(|| input_path.file_stem().and_then(|value| value.to_str()))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| "Imported account".to_string())
}

async fn create_profile_from_auth_json_import(
    state: &AppState,
    input_path: &Path,
    auth_json_bytes: &[u8],
    profile_label: Option<&str>,
    profile_id_hint: Option<&str>,
    parsed: &Value,
) -> Result<Value> {
    let label = profile_label
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| default_import_profile_label(parsed, input_path));
    let requested_id = profile_id_hint
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| label.clone());
    let base_id = sanitize_profile_id(&requested_id);
    use yaml_rust2::yaml::{Hash as YamlHash, Yaml};
    let (config_path, mut config_value) = read_codex_webui_config_yaml(state).await?;

    let root_data_dir = config_value["dataDir"]
        .as_str()
        .map(expand_server_auth_json_path)
        .unwrap_or_else(|| state.config.data_dir.clone());
    let (_, profiles_snapshot) = runtime_profiles_snapshot(&state.config);
    let mut existing_ids = profiles_snapshot.keys().cloned().collect::<HashSet<_>>();
    if let Some(profiles) = config_value["profiles"].as_vec() {
        for profile in profiles {
            if let Some(id) = profile["id"].as_str().map(sanitize_profile_id) {
                existing_ids.insert(id);
            }
        }
    }

    let mut id = base_id.clone();
    let mut suffix = 2u64;
    while existing_ids.contains(&id) {
        id = format!("{base_id}-{suffix}");
        suffix = suffix.saturating_add(1);
    }

    let codex_home = root_data_dir.join("accounts").join(&id).join("codex-home");
    let data_dir = root_data_dir.join("profiles").join(&id);
    write_file_atomically(&codex_home.join("auth.json"), auth_json_bytes.to_vec()).await?;
    tokio_fs::create_dir_all(&data_dir).await?;

    ensure_profile_yaml_array(&mut config_value, state)?;
    let profiles = profile_yaml_array_mut(&mut config_value)?;
    let mut entry = YamlHash::new();
    entry.insert(Yaml::String("id".to_string()), Yaml::String(id.clone()));
    entry.insert(
        Yaml::String("label".to_string()),
        Yaml::String(label.clone()),
    );
    entry.insert(
        Yaml::String("codexHome".to_string()),
        Yaml::String(codex_home.display().to_string()),
    );
    entry.insert(
        Yaml::String("dataDir".to_string()),
        Yaml::String(data_dir.display().to_string()),
    );
    profiles.push(Yaml::Hash(entry));

    write_codex_webui_config_yaml(&config_path, &config_value).await?;

    Ok(json!({
        "id": id,
        "label": label,
        "codexHome": codex_home.display().to_string(),
        "dataDir": data_dir.display().to_string(),
        "active": false
    }))
}

pub(crate) async fn select_account_profile_payload(
    state: &AppState,
    role: UserRole,
    params: &Value,
) -> Result<Value> {
    let requested_profile_id = params
        .get("profileId")
        .and_then(Value::as_str)
        .map(sanitize_profile_id)
        .unwrap_or_else(|| state.config.default_profile_id.clone());

    let (_, profiles_snapshot) = runtime_profiles_snapshot(&state.config);
    if !profiles_snapshot.contains_key(&requested_profile_id) {
        anyhow::bail!("Unknown profile.");
    }

    let _ = append_audit_log(
        &state.config,
        AuditLogEntry {
            id: Uuid::new_v4().to_string(),
            at: now_unix_ms(),
            role: user_role_label(role).to_string(),
            method: "account/profile/select".to_string(),
            target: Some(requested_profile_id.clone()),
            ok: true,
            error: None,
        },
    )
    .await;

    Ok(json!({
        "ok": true,
        "activeProfileId": requested_profile_id,
        "profileCookie": {
            "name": PROFILE_COOKIE,
            "path": auth_cookie_path(&state.config),
            "maxAgeSeconds": 30 * 24 * 60 * 60,
            "sameSite": match state.config.cookie_same_site {
                SameSiteMode::Strict => "Strict",
                SameSiteMode::Lax => "Lax",
                SameSiteMode::None => "None",
            }
        }
    }))
}

pub(crate) async fn update_account_profile_payload(
    state: &AppState,
    active_profile_id: &str,
    params: &Value,
) -> Result<Value> {
    let profile_id = params
        .get("profileId")
        .and_then(Value::as_str)
        .map(sanitize_profile_id)
        .ok_or_else(|| anyhow!("profileId is required."))?;
    let label = params
        .get("label")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("Profile label is required."))?;
    if label.chars().count() > 80 {
        anyhow::bail!("Profile label is too long.");
    }
    let (_, profiles_snapshot) = runtime_profiles_snapshot(&state.config);
    if !profiles_snapshot.contains_key(&profile_id) {
        anyhow::bail!("Unknown profile.");
    }

    let (config_path, mut config_value) = read_codex_webui_config_yaml(state).await?;
    ensure_profile_yaml_array(&mut config_value, state)?;
    let profiles = profile_yaml_array_mut(&mut config_value)?;
    let Some(profile) = profiles
        .iter_mut()
        .find(|profile| profile_id_from_yaml(profile).as_deref() == Some(profile_id.as_str()))
    else {
        anyhow::bail!("Profile is not present in codex-webui.yml.");
    };
    let Some(entry) = profile.as_mut_hash() else {
        anyhow::bail!("Profile entry must be a YAML object.");
    };
    entry.insert(
        yaml_rust2::Yaml::String("label".to_string()),
        yaml_rust2::Yaml::String(label.to_string()),
    );
    write_codex_webui_config_yaml(&config_path, &config_value).await?;

    let _ = emit_profile_global_notification(
        state,
        active_profile_id,
        json!({
            "method": "codex-webui/profileUpdated",
            "profileId": profile_id,
            "label": label,
            "restartRequired": false
        }),
    )
    .await;

    Ok(json!({
        "ok": true,
        "profile": {
            "id": profile_id,
            "label": label
        },
        "restartRequired": false
    }))
}

pub(crate) async fn delete_account_profile_payload(
    state: &AppState,
    active_profile_id: &str,
    params: &Value,
) -> Result<Value> {
    let profile_id = params
        .get("profileId")
        .and_then(Value::as_str)
        .map(sanitize_profile_id)
        .ok_or_else(|| anyhow!("profileId is required."))?;
    let delete_data = params
        .get("deleteData")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let resolved_active_profile_id =
        resolve_runtime_profile_entry(&state.config, active_profile_id)
            .0
            .to_string();
    let (default_profile_id, profiles_snapshot) = runtime_profiles_snapshot(&state.config);
    if profile_id == default_profile_id {
        anyhow::bail!("Default profile cannot be deleted.");
    }
    if profile_id == resolved_active_profile_id {
        anyhow::bail!("Switch to another profile before deleting this profile.");
    }
    if profiles_snapshot.len() <= 1 {
        anyhow::bail!("The last profile cannot be deleted.");
    }
    let Some(profile) = profiles_snapshot.get(&profile_id).cloned() else {
        anyhow::bail!("Unknown profile.");
    };

    let (config_path, mut config_value) = read_codex_webui_config_yaml(state).await?;
    ensure_profile_yaml_array(&mut config_value, state)?;
    let profiles = profile_yaml_array_mut(&mut config_value)?;
    let before_len = profiles.len();
    profiles
        .retain(|profile| profile_id_from_yaml(profile).as_deref() != Some(profile_id.as_str()));
    if profiles.len() == before_len {
        anyhow::bail!("Profile is not present in codex-webui.yml.");
    }
    write_codex_webui_config_yaml(&config_path, &config_value).await?;

    state.app_servers.close_profile(&profile_id).await?;
    state.quota_cache.lock().await.remove(&profile_id);

    let mut deleted_data = false;
    if delete_data {
        let data_root = normalize_path(state.config.data_dir.clone());
        for path in [profile.codex_home, profile.data_dir] {
            let normalized = normalize_path(path);
            if normalized.starts_with(&data_root) && normalized.exists() {
                tokio_fs::remove_dir_all(&normalized).await?;
                deleted_data = true;
            }
        }
    }

    let _ = emit_profile_global_notification(
        state,
        active_profile_id,
        json!({
            "method": "codex-webui/profileDeleted",
            "profileId": profile_id,
            "deleteData": delete_data,
            "deletedData": deleted_data,
            "restartRequired": false
        }),
    )
    .await;

    Ok(json!({
        "ok": true,
        "profileId": profile_id,
        "deleteData": delete_data,
        "deletedData": deleted_data,
        "restartRequired": false
    }))
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
    Ok(normalize_quota_payload(payload))
}

pub(crate) fn normalize_quota_payload(payload: UsageResponseShape) -> Value {
    let individual_limit = payload
        .spend_control
        .as_ref()
        .and_then(|value| {
            value
                .get("individual_limit")
                .or_else(|| value.get("individualLimit"))
        })
        .cloned()
        .unwrap_or(Value::Null);
    let rate_limit_reached_type = payload.rate_limit_reached_type.as_ref().and_then(|value| {
        value
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| value.as_str().map(str::to_string))
    });
    let mut limits = vec![normalize_quota_limit(
        "codex",
        "Codex",
        payload.rate_limit.as_ref(),
        payload.credits.clone().unwrap_or(Value::Null),
        individual_limit.clone(),
        rate_limit_reached_type.clone(),
    )];
    limits.extend(
        payload
            .additional_rate_limits
            .as_deref()
            .unwrap_or_default()
            .iter()
            .enumerate()
            .map(|(index, limit)| {
                let limit_id = limit
                    .metered_feature
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("additional-{}", index + 1));
                let limit_name = limit
                    .limit_name
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or(&limit_id);
                normalize_quota_limit(
                    &limit_id,
                    limit_name,
                    limit.rate_limit.as_ref(),
                    Value::Null,
                    Value::Null,
                    None,
                )
            }),
    );

    let windows = limits
        .iter()
        .find(|limit| limit.get("id").and_then(Value::as_str) == Some("codex"))
        .and_then(|limit| limit.get("windows"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let five_hour = windows
        .iter()
        .find(|window| quota_window_matches_duration(window, 5 * 60))
        .cloned();
    let weekly = windows
        .iter()
        .find(|window| quota_window_matches_duration(window, 7 * 24 * 60))
        .cloned();
    let available = limits.iter().any(|limit| {
        limit
            .get("windows")
            .and_then(Value::as_array)
            .is_some_and(|windows| !windows.is_empty())
    });

    json!({
        "available": available,
        "source": "backend-api",
        "fetchedAt": now_unix_ms(),
        "account": payload.email,
        "plan": payload.plan_type,
        "limits": limits,
        "windows": windows,
        "fiveHour": five_hour,
        "weekly": weekly,
        "credits": payload.credits,
        "individualLimit": individual_limit,
        "rateLimitReachedType": rate_limit_reached_type,
        "error": Value::Null,
    })
}

fn read_codex_auth(codex_home: &PathBuf) -> Result<AuthFile> {
    let auth_path = codex_home.join("auth.json");
    let raw = fs::read_to_string(&auth_path)
        .with_context(|| format!("missing Codex auth file at {}.", auth_path.display()))?;
    serde_json::from_str(&raw).context("invalid Codex auth.json")
}

fn normalize_quota_limit(
    limit_id: &str,
    limit_name: &str,
    rate_limit: Option<&UsageRateLimitShape>,
    credits: Value,
    individual_limit: Value,
    rate_limit_reached_type: Option<String>,
) -> Value {
    let mut windows = Vec::with_capacity(2);
    if let Some(window) = rate_limit
        .and_then(|rate_limit| rate_limit.primary_window.as_ref())
        .and_then(|window| normalize_quota_window(window, "primary", limit_id, limit_name))
    {
        windows.push(window);
    }
    if let Some(window) = rate_limit
        .and_then(|rate_limit| rate_limit.secondary_window.as_ref())
        .and_then(|window| normalize_quota_window(window, "secondary", limit_id, limit_name))
    {
        windows.push(window);
    }

    json!({
        "id": limit_id,
        "name": limit_name,
        "windows": windows,
        "credits": credits,
        "individualLimit": individual_limit,
        "rateLimitReachedType": rate_limit_reached_type,
    })
}

fn normalize_quota_window(
    window: &UsageWindowShape,
    kind: &str,
    limit_id: &str,
    limit_name: &str,
) -> Option<Value> {
    let used_percent = (window.used_percent.unwrap_or(0.0))
        .clamp(0.0, 100.0)
        .round() as u64;
    let window_duration_minutes = window
        .window_duration_mins
        .filter(|value| *value > 0)
        .or_else(|| {
            window
                .limit_window_seconds
                .filter(|value| *value > 0)
                .map(|seconds| seconds.saturating_add(59) / 60)
        });
    let absolute_reset_at = window.reset_at.filter(|value| *value > 0).map(|value| {
        let value = value as u64;
        if value >= 10_000_000_000 {
            value
        } else {
            value.saturating_mul(1000)
        }
    });
    let reset_after_seconds = window
        .reset_after_seconds
        .filter(|value| *value > 0)
        .map(|value| value as u64)
        .or_else(|| {
            absolute_reset_at
                .map(|reset_at| reset_at.saturating_sub(now_unix_ms()).saturating_add(999) / 1000)
        });
    let reset_at = absolute_reset_at.or_else(|| {
        reset_after_seconds
            .map(|seconds| now_unix_ms().saturating_add(seconds.saturating_mul(1000)))
    });
    let label = quota_window_label(window_duration_minutes, kind == "secondary");

    Some(json!({
        "id": format!("{limit_id}:{kind}"),
        "kind": kind,
        "label": label,
        "limitId": limit_id,
        "limitName": limit_name,
        "usedPercent": used_percent,
        "remainingPercent": 100_u64.saturating_sub(used_percent),
        "windowDurationMinutes": window_duration_minutes,
        "resetAfterSeconds": reset_after_seconds,
        "resetAt": reset_at,
    }))
}

fn quota_window_matches_duration(window: &Value, expected_minutes: i64) -> bool {
    window
        .get("windowDurationMinutes")
        .and_then(Value::as_i64)
        .is_some_and(|minutes| approximate_quota_window(minutes, expected_minutes))
}

fn quota_window_label(window_minutes: Option<i64>, secondary: bool) -> String {
    let Some(window_minutes) = window_minutes else {
        return if secondary {
            "Secondary usage".to_string()
        } else {
            "Usage".to_string()
        };
    };

    for (expected, label) in [
        (5 * 60, "5h"),
        (24 * 60, "Daily"),
        (7 * 24 * 60, "Weekly"),
        (30 * 24 * 60, "Monthly"),
        (365 * 24 * 60, "Annual"),
    ] {
        if approximate_quota_window(window_minutes, expected) {
            return label.to_string();
        }
    }

    if secondary {
        "Secondary usage".to_string()
    } else {
        "Usage".to_string()
    }
}

fn approximate_quota_window(actual_minutes: i64, expected_minutes: i64) -> bool {
    let actual = actual_minutes.max(0) as f64;
    let expected = expected_minutes.max(0) as f64;
    actual >= expected * 0.95 && actual <= expected * 1.05
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
