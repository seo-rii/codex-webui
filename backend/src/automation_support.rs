use super::*;

fn automation_timer_key(profile_id: &str, automation_id: &str) -> String {
    format!("profile::{profile_id}::automation::{automation_id}")
}

fn trimmed_json_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn automation_schedule_mode(value: Option<&Value>) -> &'static str {
    match value.and_then(Value::as_str) {
        Some("interval") => "interval",
        _ => "manual",
    }
}

fn automation_target(value: Option<&Value>) -> &'static str {
    match value.and_then(Value::as_str) {
        Some("worktree") => "worktree",
        _ => "local",
    }
}

fn build_automation_thread_name(name: &str) -> String {
    format!("Automation · {}", name.trim())
}

fn build_automation_worktree_name(name: &str) -> String {
    let sanitized = name
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|character| match character {
            'a'..='z' | '0'..='9' => character,
            _ => '-',
        })
        .collect::<String>()
        .split('-')
        .filter(|entry| !entry.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    if sanitized.is_empty() {
        "automation".to_string()
    } else {
        sanitized.chars().take(48).collect()
    }
}

async fn emit_profile_automations_updated(state: &AppState, profile_id: &str) {
    let payload = with_ui_state_read(state, profile_id, |ui_state| {
        Ok(json!({
            "automations": {
                "items": sorted_automations_from_ui_state(ui_state),
                "recentRuns": recent_automation_runs_from_ui_state(ui_state, DEFAULT_AUTOMATION_RUN_HISTORY_LIMIT)
            }
        }))
    })
    .await;

    if let Ok(payload) = payload {
        emit_profile_config_updated(state, profile_id, payload).await;
    }
}

async fn clear_automation_timer(state: &AppState, profile_id: &str, automation_id: &str) {
    let timer_key = automation_timer_key(profile_id, automation_id);
    if let Some(handle) = state.automation_timers.lock().await.remove(&timer_key) {
        handle.abort();
    }
}

fn schedule_automation_timer(
    state: AppState,
    profile_id: String,
    automation: Value,
) -> futures_util::future::BoxFuture<'static, ()> {
    async move {
        let automation_id = trimmed_json_string(automation.get("id"));
        let enabled = automation
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let schedule_mode = automation_schedule_mode(automation.get("scheduleMode"));
        let next_run_at = automation.get("nextRunAt").and_then(Value::as_i64);

        let Some(automation_id) = automation_id else {
            return;
        };

        clear_automation_timer(&state, &profile_id, &automation_id).await;

        if !enabled || schedule_mode != "interval" {
            return;
        }

        let Some(next_run_at) = next_run_at else {
            return;
        };

        let timer_key = automation_timer_key(&profile_id, &automation_id);
        let sleep_ms = next_run_at.saturating_sub(now_unix_ms() as i64).max(0) as u64;
        let next_state = state.clone();
        let next_profile_id = profile_id.clone();
        let next_automation_id = automation_id.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
            next_state
                .automation_timers
                .lock()
                .await
                .remove(&automation_timer_key(&next_profile_id, &next_automation_id));
            if let Err(error) = run_automation_payload(
                &next_state,
                &next_profile_id,
                &next_automation_id,
                "schedule",
            )
            .await
            {
                warn!(
                    "scheduled automation run failed for {} on profile {}: {}",
                    next_automation_id, next_profile_id, error.message
                );
            }
        });
        state
            .automation_timers
            .lock()
            .await
            .insert(timer_key, handle);
    }
    .boxed()
}

pub(crate) async fn restore_automation_schedules(state: AppState) {
    let profile_ids = state.config.profiles.keys().cloned().collect::<Vec<_>>();
    for profile_id in profile_ids {
        let result = with_ui_state_read(&state, &profile_id, |ui_state| {
            Ok(sorted_automations_from_ui_state(ui_state))
        })
        .await;
        match result {
            Ok(automations) => {
                for automation in automations {
                    schedule_automation_timer(state.clone(), profile_id.clone(), automation).await;
                }
            }
            Err(error) => {
                warn!(
                    "failed to restore automation schedules for profile {}: {}",
                    profile_id, error.message
                );
            }
        }
    }
}

pub(crate) async fn save_automation_payload(
    state: &AppState,
    profile_id: &str,
    automation: Value,
) -> ApiResult<Value> {
    let automation_id = trimmed_json_string(automation.get("id"))
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "automation.id is required."))?;
    let automation_name = trimmed_json_string(automation.get("name"))
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "Automation name is required."))?;
    let automation_prompt = automation
        .get("prompt")
        .and_then(Value::as_str)
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "Automation prompt is required."))?;
    if automation_prompt.trim().is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Automation prompt is required.",
        ));
    }

    let schedule_mode = automation_schedule_mode(automation.get("scheduleMode"));
    let normalized_interval = if schedule_mode == "interval" {
        automation
            .get("intervalMinutes")
            .and_then(|value| {
                value
                    .as_i64()
                    .or_else(|| value.as_u64().map(|entry| entry as i64))
                    .or_else(|| value.as_f64().map(|entry| entry.round() as i64))
            })
            .map(|value| value.max(1))
            .ok_or_else(|| {
                api_error(
                    StatusCode::BAD_REQUEST,
                    "Automation interval must be at least 1 minute.",
                )
            })?
    } else {
        0
    };

    let normalized_target = automation_target(automation.get("target"));
    let repo_path = trimmed_json_string(automation.get("repoPath"));
    if normalized_target == "worktree" && repo_path.is_none() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Worktree automations require a repository.",
        ));
    }

    let now = now_unix_ms() as i64;
    let payload = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(automations) = ui_state.get_mut("automations").and_then(Value::as_array_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "automations state is missing",
            ));
        };

        let created_at = automations
            .iter()
            .find(|entry| entry.get("id").and_then(Value::as_str) == Some(automation_id.as_str()))
            .and_then(|entry| entry.get("createdAt").and_then(Value::as_i64))
            .or_else(|| automation.get("createdAt").and_then(Value::as_i64))
            .unwrap_or(now);
        let enabled = automation
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let next_run_at = if enabled && schedule_mode == "interval" {
            Some(now + normalized_interval * 60_000)
        } else {
            None
        };

        let next_automation = json!({
            "id": automation_id,
            "name": automation_name,
            "prompt": automation_prompt,
            "skills": selected_skills_from_value(automation.get("skills")),
            "enabled": enabled,
            "scheduleMode": schedule_mode,
            "intervalMinutes": if schedule_mode == "interval" { Value::from(normalized_interval) } else { Value::Null },
            "target": normalized_target,
            "repoPath": repo_path.clone().map(Value::from).unwrap_or(Value::Null),
            "cwd": trimmed_json_string(automation.get("cwd")).map(Value::from).unwrap_or(Value::Null),
            "model": trimmed_json_string(automation.get("model")).map(Value::from).unwrap_or(Value::Null),
            "effort": trimmed_json_string(automation.get("effort")).map(Value::from).unwrap_or(Value::Null),
            "speed": trimmed_json_string(automation.get("speed")).map(Value::from).unwrap_or(Value::Null),
            "mode": trimmed_json_string(automation.get("mode")).map(Value::from).unwrap_or(Value::Null),
            "createdAt": created_at,
            "updatedAt": now,
            "lastRunAt": automation.get("lastRunAt").cloned().unwrap_or(Value::Null),
            "nextRunAt": next_run_at.map(Value::from).unwrap_or(Value::Null)
        });

        let mut next_automations = vec![next_automation];
        next_automations.extend(
            automations
                .iter()
                .filter(|entry| entry.get("id").and_then(Value::as_str) != Some(automation_id.as_str()))
                .cloned(),
        );
        next_automations.truncate(80);
        next_automations.sort_by(|left, right| {
            right
                .get("updatedAt")
                .and_then(Value::as_i64)
                .unwrap_or_default()
                .cmp(
                    &left
                        .get("updatedAt")
                        .and_then(Value::as_i64)
                        .unwrap_or_default(),
                )
        });
        *automations = next_automations;

        Ok(json!({
            "automations": automations.clone()
        }))
    })
    .await?;

    if let Some(saved_automation) = payload
        .get("automations")
        .and_then(Value::as_array)
        .and_then(|entries| {
            entries.iter().find(|entry| {
                entry.get("id").and_then(Value::as_str) == Some(automation_id.as_str())
            })
        })
        .cloned()
    {
        schedule_automation_timer(state.clone(), profile_id.to_string(), saved_automation).await;
    } else {
        clear_automation_timer(state, profile_id, &automation_id).await;
    }

    emit_profile_automations_updated(state, profile_id).await;
    Ok(payload)
}

pub(crate) async fn delete_automation_payload(
    state: &AppState,
    profile_id: &str,
    automation_id: &str,
) -> ApiResult<Value> {
    let trimmed_automation_id = automation_id.trim();
    clear_automation_timer(state, profile_id, trimmed_automation_id).await;

    let payload = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(automations) = ui_state
            .get_mut("automations")
            .and_then(Value::as_array_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "automations state is missing",
            ));
        };

        *automations = automations
            .iter()
            .filter(|entry| entry.get("id").and_then(Value::as_str) != Some(trimmed_automation_id))
            .cloned()
            .collect::<Vec<_>>();
        automations.sort_by(|left, right| {
            right
                .get("updatedAt")
                .and_then(Value::as_i64)
                .unwrap_or_default()
                .cmp(
                    &left
                        .get("updatedAt")
                        .and_then(Value::as_i64)
                        .unwrap_or_default(),
                )
        });

        Ok(json!({
            "automations": automations.clone()
        }))
    })
    .await?;

    emit_profile_automations_updated(state, profile_id).await;
    Ok(payload)
}

pub(crate) async fn run_automation_payload(
    state: &AppState,
    profile_id: &str,
    automation_id: &str,
    trigger: &str,
) -> ApiResult<Value> {
    let automation = with_ui_state_read(state, profile_id, |ui_state| {
        sorted_automations_from_ui_state(ui_state)
            .into_iter()
            .find(|entry| entry.get("id").and_then(Value::as_str) == Some(automation_id))
            .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Automation not found."))
    })
    .await?;

    let automation_name = trimmed_json_string(automation.get("name"))
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "Automation name is required."))?;
    let automation_prompt = automation
        .get("prompt")
        .and_then(Value::as_str)
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "Automation prompt is required."))?
        .to_string();
    let automation_target = automation_target(automation.get("target"));
    let repo_path = trimmed_json_string(automation.get("repoPath"));
    let mut cwd = trimmed_json_string(automation.get("cwd")).or_else(|| repo_path.clone());
    let mut git_repo_path = repo_path.clone();
    let mut worktree_path: Option<String> = None;
    let run_id = Uuid::new_v4().to_string();
    let now = now_unix_ms() as i64;
    let normalized_trigger = if trigger == "schedule" {
        "schedule"
    } else {
        "manual"
    };

    with_ui_state_write(state, profile_id, |ui_state| {
        let Some(automation_runs) = ui_state
            .get_mut("automationRuns")
            .and_then(Value::as_array_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "automation runs state is missing",
            ));
        };

        if automation_runs.iter().any(|entry| {
            entry.get("automationId").and_then(Value::as_str) == Some(automation_id)
                && entry.get("status").and_then(Value::as_str) == Some("running")
        }) {
            return Err(api_error(
                StatusCode::CONFLICT,
                "Automation is already starting.",
            ));
        }

        let next_run = json!({
            "id": run_id,
            "automationId": automation_id,
            "automationName": automation_name,
            "status": "running",
            "trigger": normalized_trigger,
            "sessionId": Value::Null,
            "repoPath": git_repo_path.clone().map(Value::from).unwrap_or(Value::Null),
            "cwd": cwd.clone().map(Value::from).unwrap_or(Value::Null),
            "worktreePath": Value::Null,
            "startedAt": now,
            "completedAt": Value::Null,
            "error": Value::Null
        });
        let mut next_runs = vec![next_run];
        next_runs.extend(
            automation_runs
                .iter()
                .filter(|entry| entry.get("id").and_then(Value::as_str) != Some(run_id.as_str()))
                .cloned(),
        );
        next_runs.truncate(200);
        *automation_runs = next_runs;
        Ok(())
    })
    .await?;
    emit_profile_automations_updated(state, profile_id).await;

    let result: ApiResult<Value> = async {
        if automation_target == "worktree" {
            let repo_root = repo_path.clone().ok_or_else(|| {
                api_error(
                    StatusCode::BAD_REQUEST,
                    "Worktree automations require a repository.",
                )
            })?;
            let repo_root_path = PathBuf::from(&repo_root);
            let time_suffix = now.to_string();
            let worktree_name = build_automation_worktree_name(&automation_name);
            let worktree = repo_root_path
                .join(".codex-webui")
                .join("worktrees")
                .join(&worktree_name)
                .join(&time_suffix);
            let branch_name = format!("automation/{worktree_name}-{time_suffix}");

            create_git_worktree_payload(
                state,
                &repo_root,
                &worktree.display().to_string(),
                Some(&branch_name),
                true,
                false,
            )
            .await
            .map_err(|error| {
                api_error(
                    error.status,
                    format!(
                        "Failed to create the automation worktree: {}",
                        error.message
                    ),
                )
            })?;

            let worktree_display = worktree.display().to_string();
            worktree_path = Some(worktree_display.clone());
            cwd = Some(worktree_display.clone());
            git_repo_path = Some(worktree_display);
        }

        let mut preferences = serde_json::Map::new();
        if let Some(cwd) = &cwd {
            preferences.insert("cwd".to_string(), json!(cwd));
        }
        if let Some(git_repo_path) = &git_repo_path {
            preferences.insert("gitRepoPath".to_string(), json!(git_repo_path));
        }
        for key in ["model", "effort", "speed", "mode"] {
            if let Some(value) = trimmed_json_string(automation.get(key)) {
                preferences.insert(key.to_string(), Value::String(value));
            }
        }

        let session = create_session_payload(
            state,
            profile_id,
            Value::Object(preferences.clone()),
            automation.get("skills"),
            Some(&build_automation_thread_name(&automation_name)),
        )
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to create the automation session: {}", error.message),
            )
        })?;

        let session_id = session
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                api_error(
                    StatusCode::BAD_GATEWAY,
                    "Internal session creation returned an invalid payload.",
                )
            })?
            .to_string();

        with_ui_state_write(state, profile_id, |ui_state| {
            let Some(automation_runs) = ui_state
                .get_mut("automationRuns")
                .and_then(Value::as_array_mut)
            else {
                return Err(api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "automation runs state is missing",
                ));
            };

            if let Some(run) = automation_runs
                .iter_mut()
                .find(|entry| entry.get("id").and_then(Value::as_str) == Some(run_id.as_str()))
            {
                *run = json!({
                    "id": run_id,
                    "automationId": automation_id,
                    "automationName": automation_name,
                    "status": "started",
                    "trigger": normalized_trigger,
                    "sessionId": session_id,
                    "repoPath": git_repo_path.clone().map(Value::from).unwrap_or(Value::Null),
                    "cwd": cwd.clone().map(Value::from).unwrap_or(Value::Null),
                    "worktreePath": worktree_path.clone().map(Value::from).unwrap_or(Value::Null),
                    "startedAt": now,
                    "completedAt": Value::Null,
                    "error": Value::Null
                });
            }
            Ok(())
        })
        .await?;

        send_turn_payload(
            state,
            profile_id,
            &session_id,
            &automation_prompt,
            Some(&json!([])),
            automation.get("skills"),
            Value::Object(preferences),
        )
        .await
        .map(|_| ())
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to send the automation prompt: {}", error.message),
            )
        })?;

        let updated_automation = with_ui_state_write(state, profile_id, |ui_state| {
            let Some(automations) = ui_state
                .get_mut("automations")
                .and_then(Value::as_array_mut)
            else {
                return Err(api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "automations state is missing",
                ));
            };

            let Some(automation_entry) = automations
                .iter_mut()
                .find(|entry| entry.get("id").and_then(Value::as_str) == Some(automation_id))
            else {
                return Err(api_error(StatusCode::NOT_FOUND, "Automation not found."));
            };

            let enabled = automation_entry
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let interval_minutes = automation_entry
                .get("intervalMinutes")
                .and_then(Value::as_i64);
            let next_run_at = if enabled
                && automation_schedule_mode(automation_entry.get("scheduleMode")) == "interval"
            {
                interval_minutes.map(|value| now + value.max(1) * 60_000)
            } else {
                None
            };

            if let Some(object) = automation_entry.as_object_mut() {
                object.insert("lastRunAt".to_string(), Value::from(now));
                object.insert("updatedAt".to_string(), Value::from(now));
                object.insert(
                    "nextRunAt".to_string(),
                    next_run_at.map(Value::from).unwrap_or(Value::Null),
                );
            }

            Ok(automation_entry.clone())
        })
        .await?;

        emit_profile_automations_updated(state, profile_id).await;
        let schedule_state = state.clone();
        let schedule_profile_id = profile_id.to_string();
        tokio::spawn(async move {
            schedule_automation_timer(schedule_state, schedule_profile_id, updated_automation)
                .await;
        });

        let run = with_ui_state_read(state, profile_id, |ui_state| {
            recent_automation_runs_from_ui_state(ui_state, 200)
                .into_iter()
                .find(|entry| entry.get("id").and_then(Value::as_str) == Some(run_id.as_str()))
                .ok_or_else(|| {
                    api_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Failed to read the automation run after dispatch.",
                    )
                })
        })
        .await?;

        Ok(json!({
            "ok": true,
            "session": session,
            "run": run
        }))
    }
    .await;

    if let Err(error) = &result {
        let error_message = error.message.clone();
        let completed_at = now_unix_ms() as i64;
        let _ = with_ui_state_write(state, profile_id, |ui_state| {
            let Some(automation_runs) = ui_state
                .get_mut("automationRuns")
                .and_then(Value::as_array_mut)
            else {
                return Err(api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "automation runs state is missing",
                ));
            };

            if let Some(run) = automation_runs
                .iter_mut()
                .find(|entry| entry.get("id").and_then(Value::as_str) == Some(run_id.as_str()))
            {
                let session_id = run.get("sessionId").cloned().unwrap_or(Value::Null);
                *run = json!({
                    "id": run_id,
                    "automationId": automation_id,
                    "automationName": automation_name,
                    "status": "failed",
                    "trigger": normalized_trigger,
                    "sessionId": session_id,
                    "repoPath": git_repo_path.clone().map(Value::from).unwrap_or(Value::Null),
                    "cwd": cwd.clone().map(Value::from).unwrap_or(Value::Null),
                    "worktreePath": worktree_path.clone().map(Value::from).unwrap_or(Value::Null),
                    "startedAt": now,
                    "completedAt": completed_at,
                    "error": error_message
                });
            }
            Ok(())
        })
        .await;
        emit_profile_automations_updated(state, profile_id).await;

        if normalized_trigger == "schedule" {
            let interval_minutes = automation
                .get("intervalMinutes")
                .and_then(Value::as_i64)
                .unwrap_or(1)
                .max(1);
            let next_run_at = completed_at + interval_minutes * 60_000;
            if let Ok(updated_automation) = with_ui_state_write(state, profile_id, |ui_state| {
                let Some(automations) = ui_state
                    .get_mut("automations")
                    .and_then(Value::as_array_mut)
                else {
                    return Err(api_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "automations state is missing",
                    ));
                };
                let Some(automation_entry) = automations
                    .iter_mut()
                    .find(|entry| entry.get("id").and_then(Value::as_str) == Some(automation_id))
                else {
                    return Err(api_error(StatusCode::NOT_FOUND, "Automation not found."));
                };
                if let Some(object) = automation_entry.as_object_mut() {
                    object.insert("nextRunAt".to_string(), Value::from(next_run_at));
                }
                Ok(automation_entry.clone())
            })
            .await
            {
                let schedule_state = state.clone();
                let schedule_profile_id = profile_id.to_string();
                tokio::spawn(async move {
                    schedule_automation_timer(
                        schedule_state,
                        schedule_profile_id,
                        updated_automation,
                    )
                    .await;
                });
                emit_profile_automations_updated(state, profile_id).await;
            }
        }
    }

    result
}
