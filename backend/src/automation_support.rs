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

fn automation_run_is_active(status: Option<&str>) -> bool {
    matches!(status, Some("running" | "started"))
}

pub(crate) fn automation_status_for_thread_status(status: &str) -> (String, Option<String>) {
    match status {
        "failed" | "error" => (
            "failed".to_string(),
            Some(format!("Thread ended with status: {status}.")),
        ),
        "cancelled" | "canceled" | "aborted" => (
            "cancelled".to_string(),
            Some(format!("Thread ended with status: {status}.")),
        ),
        _ => ("completed".to_string(), None),
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

pub(crate) async fn complete_active_automation_runs_for_session(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    status: &str,
    error: Option<&str>,
) {
    let completed_at = now_unix_ms() as i64;
    let changed = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(automation_runs) = ui_state
            .get_mut("automationRuns")
            .and_then(Value::as_array_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "automation runs state is missing",
            ));
        };

        let mut changed = false;
        for run in automation_runs.iter_mut() {
            if run.get("sessionId").and_then(Value::as_str) != Some(session_id) {
                continue;
            }
            let current_status = run.get("status").and_then(Value::as_str);
            let can_override_completed_failure =
                current_status == Some("completed") && matches!(status, "failed" | "cancelled");
            if !automation_run_is_active(current_status) && !can_override_completed_failure {
                continue;
            }
            if let Some(object) = run.as_object_mut() {
                object.insert("status".to_string(), json!(status));
                object.insert("completedAt".to_string(), json!(completed_at));
                object.insert(
                    "error".to_string(),
                    error.map(Value::from).unwrap_or(Value::Null),
                );
                changed = true;
            }
        }
        Ok(changed)
    })
    .await
    .unwrap_or(false);

    if changed {
        emit_profile_automations_updated(state, profile_id).await;
    }
}

pub(crate) async fn reconcile_stale_automation_runs_for_profile(
    state: &AppState,
    profile_id: &str,
) -> usize {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    let runtime_key_prefix = format!("profile::{resolved_profile_id}::session-runtime::");
    let mut active_session_ids = HashSet::new();
    {
        let active_turns = state.active_turns.lock().await;
        for key in active_turns.keys() {
            if let Some(session_id) = key.strip_prefix(&runtime_key_prefix) {
                active_session_ids.insert(session_id.to_string());
            }
        }
    }
    {
        let pending_turn_starts = state.pending_turn_starts.lock().await;
        for key in pending_turn_starts.iter() {
            if let Some(session_id) = key.strip_prefix(&runtime_key_prefix) {
                active_session_ids.insert(session_id.to_string());
            }
        }
    }

    let stale_runs = with_ui_state_read(state, profile_id, |ui_state| {
        let runtime_status_by_thread_id = ui_state
            .get("runtimeStatusByThreadId")
            .and_then(Value::as_object);
        let automation_runs = ui_state
            .get("automationRuns")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut stale_runs = Vec::new();
        let mut seen_sessions = HashSet::new();
        for run in automation_runs {
            if !automation_run_is_active(run.get("status").and_then(Value::as_str)) {
                continue;
            }
            let Some(session_id) = trimmed_json_string(run.get("sessionId")) else {
                continue;
            };
            if active_session_ids.contains(&session_id) || !seen_sessions.insert(session_id.clone())
            {
                continue;
            }
            let Some(status) = runtime_status_by_thread_id
                .and_then(|entries| entries.get(&session_id))
                .and_then(|status_value| normalized_thread_status(Some(status_value)))
            else {
                continue;
            };
            if is_live_thread_status(&status) {
                continue;
            }
            let (automation_status, automation_error) =
                automation_status_for_thread_status(&status);
            stale_runs.push((session_id, automation_status, automation_error));
        }
        Ok(stale_runs)
    })
    .await
    .unwrap_or_default();

    let count = stale_runs.len();
    for (session_id, status, error) in stale_runs {
        complete_active_automation_runs_for_session(
            state,
            profile_id,
            &session_id,
            &status,
            error.as_deref(),
        )
        .await;
    }
    count
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

async fn skip_overlapping_scheduled_automation(
    state: &AppState,
    profile_id: &str,
    automation_id: &str,
    conflict_message: &str,
) -> ApiResult<Value> {
    let now = now_unix_ms() as i64;
    let run_id = Uuid::new_v4().to_string();
    let (updated_automation, run) = with_ui_state_write(state, profile_id, |ui_state| {
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

        let automation_name = trimmed_json_string(automation_entry.get("name"))
            .unwrap_or_else(|| "Automation".into());
        let enabled = automation_entry
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let interval_minutes = automation_entry
            .get("intervalMinutes")
            .and_then(Value::as_i64)
            .unwrap_or(1)
            .max(1);
        let next_run_at = if enabled
            && automation_schedule_mode(automation_entry.get("scheduleMode")) == "interval"
        {
            Some(now + interval_minutes * 60_000)
        } else {
            None
        };
        if let Some(object) = automation_entry.as_object_mut() {
            object.insert("updatedAt".to_string(), Value::from(now));
            object.insert(
                "nextRunAt".to_string(),
                next_run_at.map(Value::from).unwrap_or(Value::Null),
            );
        }
        let repo_path = automation_entry
            .get("repoPath")
            .cloned()
            .unwrap_or(Value::Null);
        let cwd = automation_entry.get("cwd").cloned().unwrap_or(Value::Null);
        let updated_automation = automation_entry.clone();

        let Some(automation_runs) = ui_state
            .get_mut("automationRuns")
            .and_then(Value::as_array_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "automation runs state is missing",
            ));
        };
        let run = json!({
            "id": run_id,
            "automationId": automation_id,
            "automationName": automation_name,
            "status": "skipped",
            "trigger": "schedule",
            "sessionId": Value::Null,
            "repoPath": repo_path,
            "cwd": cwd,
            "worktreePath": Value::Null,
            "startedAt": now,
            "completedAt": now,
            "error": conflict_message
        });
        automation_runs.insert(0, run.clone());
        if automation_runs.len() > 200 {
            automation_runs.truncate(200);
        }
        Ok((updated_automation, run))
    })
    .await?;

    emit_profile_automations_updated(state, profile_id).await;
    schedule_automation_timer(state.clone(), profile_id.to_string(), updated_automation).await;

    Ok(json!({
        "ok": true,
        "skipped": true,
        "reason": "activeRun",
        "run": run
    }))
}

pub(crate) async fn restore_automation_schedules(state: AppState) {
    let profile_ids = state.config.profiles.keys().cloned().collect::<Vec<_>>();
    for profile_id in profile_ids {
        reconcile_stale_automation_runs_for_profile(&state, &profile_id).await;
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

pub(crate) async fn cleanup_automation_worktrees_payload(
    state: &AppState,
    profile_id: &str,
    keep_recent: usize,
    dry_run: bool,
) -> ApiResult<Value> {
    #[derive(Clone)]
    struct CleanupCandidate {
        run_id: String,
        automation_id: String,
        repo_path: String,
        worktree_path: String,
    }

    let keep_recent = keep_recent.min(1000);
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    let mut runs = with_ui_state_read(state, profile_id, |ui_state| {
        let mut runs = recent_automation_runs_from_ui_state(ui_state, 1000);
        runs.sort_by(|left, right| {
            right
                .get("completedAt")
                .and_then(Value::as_i64)
                .or_else(|| right.get("startedAt").and_then(Value::as_i64))
                .unwrap_or_default()
                .cmp(
                    &left
                        .get("completedAt")
                        .and_then(Value::as_i64)
                        .or_else(|| left.get("startedAt").and_then(Value::as_i64))
                        .unwrap_or_default(),
                )
        });
        Ok(runs)
    })
    .await?;
    let active_runtime_keys = state
        .active_turns
        .lock()
        .await
        .keys()
        .cloned()
        .collect::<HashSet<_>>();
    let pending_runtime_keys = state
        .pending_turn_starts
        .lock()
        .await
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    let known_worktree_paths = runs
        .iter()
        .filter_map(|run| trimmed_json_string(run.get("worktreePath")))
        .collect::<HashSet<_>>();
    let mut retained_by_automation = HashMap::<String, usize>::new();
    let mut candidates = Vec::new();
    let mut skipped_active = 0_u64;

    for run in runs.drain(..) {
        if run.get("worktreeRemovedAt").is_some() {
            continue;
        }
        let status = run.get("status").and_then(Value::as_str);
        if !matches!(status, Some("completed" | "failed" | "cancelled")) {
            continue;
        }
        let Some(automation_id) = trimmed_json_string(run.get("automationId")) else {
            continue;
        };
        let Some(repo_path) = trimmed_json_string(run.get("repoPath")) else {
            continue;
        };
        let Some(worktree_path) = trimmed_json_string(run.get("worktreePath")) else {
            continue;
        };
        let retained = retained_by_automation
            .entry(automation_id.clone())
            .or_insert(0);
        if *retained < keep_recent {
            *retained += 1;
            continue;
        }
        if let Some(session_id) = trimmed_json_string(run.get("sessionId")) {
            let runtime_key = runtime_session_key(&resolved_profile_id, &session_id);
            if active_runtime_keys.contains(&runtime_key)
                || pending_runtime_keys.contains(&runtime_key)
            {
                skipped_active = skipped_active.saturating_add(1);
                continue;
            }
        }
        let Some(run_id) = trimmed_json_string(run.get("id")) else {
            continue;
        };
        candidates.push(CleanupCandidate {
            run_id,
            automation_id,
            repo_path,
            worktree_path,
        });
    }

    let mut removed_run_ids = HashSet::<String>::new();
    let mut failed_errors = Vec::<Value>::new();
    if !dry_run {
        for candidate in &candidates {
            match remove_git_worktree_payload(
                state,
                &candidate.repo_path,
                &candidate.worktree_path,
                false,
            )
            .await
            {
                Ok(_) => {
                    removed_run_ids.insert(candidate.run_id.clone());
                }
                Err(error) => {
                    failed_errors.push(json!({
                        "runId": candidate.run_id,
                        "automationId": candidate.automation_id,
                        "worktreePath": candidate.worktree_path,
                        "status": error.status.as_u16(),
                        "message": error.message
                    }));
                }
            }
        }

        if !removed_run_ids.is_empty() || !failed_errors.is_empty() {
            let removed_at = now_unix_ms() as i64;
            let failed_by_run_id = failed_errors
                .iter()
                .filter_map(|entry| {
                    Some((
                        entry.get("runId")?.as_str()?.to_string(),
                        entry.get("message")?.as_str()?.to_string(),
                    ))
                })
                .collect::<HashMap<_, _>>();
            with_ui_state_write(state, profile_id, |ui_state| {
                let Some(runs) = ui_state
                    .get_mut("automationRuns")
                    .and_then(Value::as_array_mut)
                else {
                    return Err(api_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "automation runs state is missing",
                    ));
                };
                for run in runs {
                    let Some(run_id) = run.get("id").and_then(Value::as_str).map(str::to_string)
                    else {
                        continue;
                    };
                    if let Some(object) = run.as_object_mut() {
                        if removed_run_ids.contains(&run_id) {
                            object.insert("worktreeRemovedAt".to_string(), Value::from(removed_at));
                            object.remove("worktreeCleanupError");
                        } else if let Some(error) = failed_by_run_id.get(&run_id) {
                            object.insert(
                                "worktreeCleanupError".to_string(),
                                Value::from(error.clone()),
                            );
                        }
                    }
                }
                Ok(())
            })
            .await?;
            emit_profile_automations_updated(state, profile_id).await;
        }
    }

    let mut orphan_candidates = Vec::<Value>::new();
    for root in resolved_allowed_roots(&state.config).await {
        let mut roots_to_scan = vec![root.clone()];
        if let Ok(mut children) = tokio_fs::read_dir(&root).await {
            while let Ok(Some(child)) = children.next_entry().await {
                let child_path = child.path();
                if child_path.is_dir() {
                    roots_to_scan.push(child_path);
                }
            }
        }
        for repo_root in roots_to_scan {
            let worktrees_root = repo_root.join(".codex-webui").join("worktrees");
            let mut automation_dirs = match tokio_fs::read_dir(&worktrees_root).await {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            while let Ok(Some(automation_dir)) = automation_dirs.next_entry().await {
                let automation_dir_path = automation_dir.path();
                if !automation_dir_path.is_dir() {
                    continue;
                }
                let automation_id = automation_dir
                    .file_name()
                    .to_string_lossy()
                    .trim()
                    .to_string();
                let mut worktree_dirs = match tokio_fs::read_dir(&automation_dir_path).await {
                    Ok(entries) => entries,
                    Err(_) => continue,
                };
                while let Ok(Some(worktree_dir)) = worktree_dirs.next_entry().await {
                    let worktree_path = worktree_dir.path();
                    if !worktree_path.is_dir() {
                        continue;
                    }
                    let worktree_path_text = worktree_path.display().to_string();
                    if known_worktree_paths.contains(&worktree_path_text) {
                        continue;
                    }
                    orphan_candidates.push(json!({
                        "automationId": automation_id,
                        "repoPath": repo_root.display().to_string(),
                        "worktreePath": worktree_path_text
                    }));
                }
            }
        }
    }
    let orphan_count = orphan_candidates.len();

    Ok(json!({
        "ok": true,
        "dryRun": dry_run,
        "keepRecent": keep_recent,
        "candidates": candidates.len(),
        "removed": removed_run_ids.len(),
        "failed": failed_errors.len(),
        "skippedActive": skipped_active,
        "orphans": orphan_count,
        "orphanCandidates": orphan_candidates,
        "worktrees": candidates
            .iter()
            .map(|candidate| {
                json!({
                    "runId": candidate.run_id,
                    "automationId": candidate.automation_id,
                    "repoPath": candidate.repo_path,
                    "worktreePath": candidate.worktree_path
                })
            })
            .collect::<Vec<_>>(),
        "errors": failed_errors
    }))
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

    let start_record_result = with_ui_state_write(state, profile_id, |ui_state| {
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
                && automation_run_is_active(entry.get("status").and_then(Value::as_str))
        }) {
            return Err(api_error(
                StatusCode::CONFLICT,
                "Automation is already running.",
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
    .await;
    if let Err(error) = start_record_result {
        if normalized_trigger == "schedule"
            && error.status == StatusCode::CONFLICT
            && error.message == "Automation is already running."
        {
            return skip_overlapping_scheduled_automation(
                state,
                profile_id,
                automation_id,
                &error.message,
            )
            .await;
        }
        return Err(error);
    }
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
            let git_dir = repo_root_path.join(".git");
            if git_dir.is_dir() {
                let exclude_result: Result<()> = async {
                    let exclude_path = git_dir.join("info").join("exclude");
                    if let Some(parent) = exclude_path.parent() {
                        tokio_fs::create_dir_all(parent).await?;
                    }
                    let existing = tokio_fs::read_to_string(&exclude_path)
                        .await
                        .unwrap_or_default();
                    if !existing
                        .lines()
                        .any(|line| line.trim() == ".codex-webui/worktrees/")
                    {
                        let mut file = tokio_fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(&exclude_path)
                            .await?;
                        if !existing.is_empty() && !existing.ends_with('\n') {
                            file.write_all(b"\n").await?;
                        }
                        file.write_all(b".codex-webui/worktrees/\n").await?;
                        file.sync_all().await?;
                    }
                    Ok(())
                }
                .await;
                if let Err(error) = exclude_result {
                    warn!(
                        "failed to add automation worktree exclude for {}: {error}",
                        repo_root_path.display()
                    );
                }
            }

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
            None,
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
