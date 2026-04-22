use super::*;

fn env_choice(var: &str, allowed: &[&str]) -> Option<String> {
    env::var(var)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| allowed.iter().any(|allowed_value| value == allowed_value))
}

fn env_bool(var: &str) -> Option<bool> {
    match env::var(var).ok()?.trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

pub(crate) async fn session_preferences_defaults_payload(
    state: &AppState,
    profile_id: &str,
) -> Value {
    let (_, profile) = resolve_runtime_profile_entry(&state.config, profile_id);
    let codex_home = profile.codex_home.clone();
    let codex_defaults = tokio::task::spawn_blocking(move || read_codex_toml_defaults(&codex_home))
        .await
        .unwrap_or_else(|_| CodexTomlDefaults {
            service_tier: "auto".to_string(),
            ..CodexTomlDefaults::default()
        });
    let allowed_roots = resolved_allowed_roots(&state.config).await;
    let default_cwd = allowed_roots
        .first()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| state.config.project_root.display().to_string());
    let mode = env_choice("CODEX_WEBUI_DEFAULT_MODE", &["default", "plan"])
        .unwrap_or_else(|| "default".to_string());
    let speed = env_choice("CODEX_WEBUI_DEFAULT_SPEED", &["auto", "fast", "flex"])
        .or_else(|| {
            (codex_defaults.service_tier == "fast" || codex_defaults.service_tier == "flex")
                .then(|| codex_defaults.service_tier.clone())
        })
        .unwrap_or_else(|| "auto".to_string());
    let sandbox_mode = env_choice(
        "CODEX_WEBUI_DEFAULT_SANDBOX",
        &["read-only", "workspace-write", "danger-full-access"],
    )
    .or_else(|| codex_defaults.sandbox_mode.clone())
    .unwrap_or_else(|| "workspace-write".to_string());
    let approval_policy = env_choice(
        "CODEX_WEBUI_DEFAULT_APPROVAL_POLICY",
        &["never", "on-request", "on-failure", "untrusted"],
    )
    .or_else(|| codex_defaults.approval_policy.clone())
    .unwrap_or_else(|| "on-request".to_string());
    let effort = env_choice(
        "CODEX_WEBUI_DEFAULT_EFFORT",
        &["minimal", "low", "medium", "high", "xhigh"],
    )
    .or_else(|| {
        if mode == "plan" {
            codex_defaults.plan_mode_reasoning_effort.clone()
        } else {
            codex_defaults.model_reasoning_effort.clone()
        }
    })
    .unwrap_or_else(|| "medium".to_string());
    let personality = env_choice(
        "CODEX_WEBUI_DEFAULT_PERSONALITY",
        &["none", "friendly", "pragmatic"],
    )
    .or_else(|| codex_defaults.personality.clone())
    .unwrap_or_else(|| "pragmatic".to_string());

    json!({
        "cwd": default_cwd,
        "model": env::var("CODEX_WEBUI_DEFAULT_MODEL")
            .ok()
            .map(Value::String)
            .or_else(|| codex_defaults.model.clone().map(Value::String))
            .unwrap_or(Value::Null),
        "modelContextWindow": codex_defaults
            .model_context_window
            .map(Value::from)
            .unwrap_or(Value::Null),
        "effort": effort,
        "speed": speed,
        "personality": personality,
        "mode": mode,
        "sendOnEnter": env_bool("CODEX_WEBUI_DEFAULT_SEND_ON_ENTER").unwrap_or(false),
        "sandboxMode": sandbox_mode,
        "approvalPolicy": approval_policy,
        "networkAccess": env_bool("CODEX_WEBUI_DEFAULT_NETWORK")
            .unwrap_or(codex_defaults.network_access.unwrap_or(false)),
        "autoApproveMode": env_choice(
            "CODEX_WEBUI_DEFAULT_AUTO_APPROVE",
            &["manual", "turn", "session"]
        )
        .unwrap_or_else(|| "manual".to_string()),
        "steeringResumeMode": env_choice(
            "CODEX_WEBUI_DEFAULT_STEERING_RESUME",
            &["ask", "auto"]
        )
        .unwrap_or_else(|| "ask".to_string()),
        "shutdownOnCompletion": false,
        "gitRepoPath": Value::Null
    })
}

pub(crate) async fn config_models_payload(
    state: &AppState,
    profile_id: &str,
) -> ApiResult<Vec<Value>> {
    let client = app_server_client(state, profile_id)
        .await
        .map_err(|error| api_error(StatusCode::BAD_GATEWAY, error.to_string()))?;
    let response = client
        .request("model/list", json!({ "includeHidden": false }))
        .await
        .map_err(|error| api_error(StatusCode::BAD_GATEWAY, error.to_string()))?;
    Ok(response
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|model| {
            json!({
                "id": model.get("id").and_then(Value::as_str).unwrap_or_default(),
                "displayName": model
                    .get("displayName")
                    .or_else(|| model.get("model"))
                    .or_else(|| model.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                "description": model.get("description").and_then(Value::as_str).unwrap_or_default(),
                "defaultReasoningEffort": model
                    .get("defaultReasoningEffort")
                    .and_then(Value::as_str)
                    .unwrap_or("medium"),
                "supportedReasoningEfforts": model
                    .get("supportedReasoningEfforts")
                    .and_then(Value::as_array)
                    .map(|entries| {
                        entries
                            .iter()
                            .filter_map(|entry| {
                                entry
                                    .get("reasoningEffort")
                                    .or_else(|| entry.get("effort"))
                                    .or(Some(entry))
                                    .and_then(Value::as_str)
                                    .map(str::to_string)
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
                "additionalSpeedTiers": model
                    .get("additionalSpeedTiers")
                    .and_then(Value::as_array)
                    .map(|entries| {
                        entries
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
                "inputModalities": model
                    .get("inputModalities")
                    .and_then(Value::as_array)
                    .map(|entries| {
                        entries
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
                "supportsPersonality": model
                    .get("supportsPersonality")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                "isDefault": model.get("isDefault").and_then(Value::as_bool).unwrap_or(false)
            })
        })
        .collect())
}

pub(crate) async fn config_collaboration_modes_payload(
    state: &AppState,
    profile_id: &str,
) -> ApiResult<Vec<Value>> {
    let client = app_server_client(state, profile_id)
        .await
        .map_err(|error| api_error(StatusCode::BAD_GATEWAY, error.to_string()))?;
    let response = client
        .request("collaborationMode/list", json!({}))
        .await
        .map_err(|error| api_error(StatusCode::BAD_GATEWAY, error.to_string()))?;
    Ok(response
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|mode| {
            json!({
                "name": mode.get("name").and_then(Value::as_str).unwrap_or_default(),
                "mode": mode.get("mode").cloned().unwrap_or(Value::Null),
                "model": mode.get("model").cloned().unwrap_or(Value::Null),
                "reasoning_effort": mode
                    .get("reasoning_effort")
                    .cloned()
                    .unwrap_or(Value::Null)
            })
        })
        .collect())
}

pub(crate) async fn get_config_payload(state: &AppState, profile_id: &str) -> ApiResult<Value> {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    let (
        defaults,
        allowed_roots_result,
        notifications_result,
        autostart_result,
        theme_override_result,
        models_result,
        collaboration_modes_result,
        account_state_result,
        shutdown_capability,
        paused_queues_result,
        ui_state_result,
    ) = tokio::join!(
        session_preferences_defaults_payload(state, profile_id),
        async {
            Ok::<Value, ApiError>(
                list_directories_payload(state, None)
                    .await?
                    .get("allowedRoots")
                    .cloned()
                    .unwrap_or_else(|| json!([])),
            )
        },
        get_notifications_payload(state, profile_id, DEFAULT_NOTIFICATION_LIMIT),
        async {
            get_autostart_state(&state.config)
                .await
                .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
        },
        async {
            read_stored_theme_settings(&state.config, profile_id)
                .await
                .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
        },
        config_models_payload(state, profile_id),
        config_collaboration_modes_payload(state, profile_id),
        async {
            get_account_state(state, profile_id)
                .await
                .map_err(|error| api_error(StatusCode::BAD_GATEWAY, error.to_string()))
        },
        system_shutdown_capability(&state.config),
        async {
            Ok::<Value, ApiError>(
                list_resume_pending_queues_payload(state, profile_id)
                    .await
                    .unwrap_or_else(|_| json!([])),
            )
        },
        with_ui_state_read(state, profile_id, |ui_state| {
            let notification_settings = ui_state
                .get("notifications")
                .and_then(Value::as_object)
                .and_then(|notifications| notifications.get("settings"))
                .map(|value| normalize_notification_settings_value(Some(value)))
                .unwrap_or_else(default_notification_settings_value);

            Ok((
                ui_state
                    .get("savedSessionFilters")
                    .cloned()
                    .unwrap_or_else(|| json!([])),
                known_tags_from_ui_state(ui_state),
                sorted_prompt_presets_from_ui_state(ui_state),
                sorted_automations_from_ui_state(ui_state),
                recent_automation_runs_from_ui_state(
                    ui_state,
                    DEFAULT_AUTOMATION_RUN_HISTORY_LIMIT,
                ),
                notification_settings,
                ui_state
                    .get("global")
                    .and_then(|value| value.get("shutdownAfterQueueCompletes"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                ui_state
                    .get("global")
                    .and_then(|value| value.get("scheduledShutdown"))
                    .cloned()
                    .unwrap_or(Value::Null),
            ))
        }),
    );

    let allowed_roots = allowed_roots_result?;
    let notifications = notifications_result?;
    let autostart = autostart_result?;
    let theme_override = theme_override_result?;
    let theme = theme_override.unwrap_or_else(|| json!({}));
    let models = models_result?;
    let collaboration_modes = collaboration_modes_result?;
    let account_state = account_state_result?;
    let (shutdown_available, _) = shutdown_capability;
    let paused_queues = paused_queues_result?;
    let (
        saved_filters,
        known_tags,
        prompt_presets,
        automations,
        recent_runs,
        notification_settings,
        shutdown_after_queue_completes,
        scheduled_shutdown,
    ) = ui_state_result?;

    let next_scheduled_shutdown = if shutdown_available
        && scheduled_shutdown
            .get("scheduledFor")
            .and_then(Value::as_u64)
            .is_some_and(|value| value > now_unix_ms())
    {
        scheduled_shutdown
    } else {
        Value::Null
    };
    let mut profiles = state
        .config
        .profiles
        .iter()
        .map(|(id, profile)| {
            json!({
                "id": id,
                "label": profile.label,
                "codexHome": profile.codex_home.display().to_string(),
                "active": id == &resolved_profile_id
            })
        })
        .collect::<Vec<_>>();
    profiles.sort_by(|left, right| {
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

    let profile = resolve_runtime_profile(&state.config, profile_id);
    let account = account_state
        .get("account")
        .cloned()
        .unwrap_or_else(|| json!({}));

    Ok(json!({
        "models": models,
        "collaborationModes": collaboration_modes,
        "allowedRoots": allowed_roots,
        "defaults": defaults,
        "paths": {
            "codexHome": profile.codex_home.display().to_string(),
            "configFilePath": config_toml_path(&profile.codex_home).display().to_string()
        },
        "git": {
            "discoveryDepth": state.config.git_discovery_depth
        },
        "autostart": autostart,
        "systemShutdown": {
            "available": shutdown_available,
            "delaySeconds": state.config.system_shutdown_delay_seconds,
            "armed": shutdown_available
                && state.config.system_shutdown_enabled
                && shutdown_after_queue_completes
        },
        "startup": {
            "pausedQueues": paused_queues,
            "scheduledShutdown": next_scheduled_shutdown
        },
        "notifications": {
            "unreadCount": notifications.get("unreadCount").cloned().unwrap_or_else(|| json!(0)),
            "settings": notification_settings
        },
        "sessionOrganization": {
            "savedFilters": saved_filters,
            "knownTags": known_tags
        },
        "promptPresets": prompt_presets,
        "automations": {
            "items": automations,
            "recentRuns": recent_runs
        },
        "account": {
            "type": account.get("type").cloned().unwrap_or(Value::Null),
            "email": account.get("email").cloned().unwrap_or(Value::Null),
            "planType": account.get("planType").cloned().unwrap_or(Value::Null),
            "requiresOpenaiAuth": account_state
                .get("requiresOpenaiAuth")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        },
        "theme": theme,
        "profiles": profiles
    }))
}

pub(crate) async fn update_config_payload(
    state: &AppState,
    profile_id: &str,
    payload: Value,
) -> ApiResult<Value> {
    let mut event_patch = serde_json::Map::new();

    if let Some(theme) = payload.get("theme").filter(|value| !value.is_null()) {
        let saved_theme = write_stored_theme_settings(&state.config, profile_id, theme)
            .await
            .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
        event_patch.insert("theme".to_string(), saved_theme);
    }

    if let Some(enabled) = payload
        .get("autostart")
        .and_then(|value| value.get("enabled"))
        .and_then(Value::as_bool)
    {
        let autostart = save_autostart_enabled(&state.config, enabled)
            .await
            .map_err(|error| api_error(StatusCode::BAD_REQUEST, error.to_string()))?;
        event_patch.insert("autostart".to_string(), autostart);
    }

    if let Some(armed) = payload
        .get("systemShutdown")
        .and_then(|value| value.get("armed"))
        .and_then(Value::as_bool)
    {
        let shutdown_primed = if armed {
            has_outstanding_queued_work(state, profile_id).await
                || has_active_work_across_threads(state, profile_id).await
        } else {
            false
        };
        with_ui_state_write(state, profile_id, |ui_state| {
            let Some(global) = ui_state.get_mut("global").and_then(Value::as_object_mut) else {
                return Err(api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "global state is missing",
                ));
            };
            global.insert("shutdownAfterQueueCompletes".to_string(), json!(armed));
            global.insert(
                "shutdownAfterQueueCompletesPrimed".to_string(),
                json!(shutdown_primed),
            );
            global.insert("scheduledShutdown".to_string(), Value::Null);
            Ok(())
        })
        .await?;
        if armed {
            maybe_schedule_global_shutdown(state, profile_id, None).await;
        } else {
            clear_scheduled_shutdown(state, profile_id).await;
        }
    }

    if !event_patch.is_empty() {
        emit_profile_config_updated(state, profile_id, Value::Object(event_patch)).await;
    }
    if payload.get("systemShutdown").is_some() {
        emit_runtime_profile_config_updated(state, profile_id).await;
    }

    get_config_payload(state, profile_id).await
}

pub(crate) async fn handle_config_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
) -> Response {
    let result = match request.method() {
        &Method::GET => get_config_payload(&state, &auth.profile_id).await,
        &Method::PATCH => {
            if auth.role != UserRole::Admin {
                return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
            }

            let body = to_bytes(request.into_body(), usize::MAX)
                .await
                .context("failed to read config request body");
            match body {
                Ok(body) => {
                    let payload: Value =
                        serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
                    update_config_payload(&state, &auth.profile_id, payload).await
                }
                Err(_) => Err(api_error(
                    StatusCode::BAD_REQUEST,
                    "Failed to read config request body.",
                )),
            }
        }
        _ => return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed."),
    };

    match result {
        Ok(payload) => {
            let mut response = Json(payload).into_response();
            *response.status_mut() = StatusCode::CREATED;
            response
        }
        Err(error) => json_error(error.status, &error.message),
    }
}
