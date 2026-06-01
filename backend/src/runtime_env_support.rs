use super::*;

pub(crate) async fn app_server_client(
    state: &AppState,
    profile_id: &str,
) -> Result<AppServerClient> {
    let (resolved_profile_id, profile) = resolve_runtime_profile_entry(&state.config, profile_id);
    app_server_client_with_key(
        state,
        resolved_profile_id,
        profile,
        resolved_profile_id.to_string(),
    )
    .await
}

pub(crate) async fn app_server_client_for_session(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> Result<AppServerClient> {
    let (resolved_profile_id, profile) = resolve_runtime_profile_entry(&state.config, profile_id);
    let runtime_key = runtime_session_key(resolved_profile_id, session_id);
    let client_key = state
        .session_app_server_assignments
        .lock()
        .await
        .get(&runtime_key)
        .cloned()
        .unwrap_or_else(|| resolved_profile_id.to_string());
    app_server_client_with_key(state, resolved_profile_id, profile, client_key).await
}

pub(crate) async fn app_server_client_for_session_turn(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> Result<AppServerClient> {
    let (resolved_profile_id, profile) = resolve_runtime_profile_entry(&state.config, profile_id);
    let runtime_key = runtime_session_key(resolved_profile_id, session_id);
    let cached_goal_is_active = with_ui_state_read(state, profile_id, |ui_state| {
        Ok(ui_state
            .get("goalsByThreadId")
            .and_then(Value::as_object)
            .and_then(|goals| goals.get(session_id))
            .and_then(|goal| goal.get("status"))
            .and_then(Value::as_str)
            .is_some_and(|status| status == "active"))
    })
    .await
    .unwrap_or(false);
    let client_key = {
        let mut assignments = state.session_app_server_assignments.lock().await;
        let desired_client_key = if state.config.per_session_app_servers {
            format!("{resolved_profile_id}::session::{session_id}")
        } else if cached_goal_is_active {
            format!("{resolved_profile_id}::goal::{session_id}")
        } else {
            resolved_profile_id.to_string()
        };
        assignments
            .entry(runtime_key)
            .or_insert(desired_client_key)
            .clone()
    };
    app_server_client_with_key(state, resolved_profile_id, profile, client_key).await
}

pub(crate) async fn app_server_client_for_goal_session(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> Result<AppServerClient> {
    let (resolved_profile_id, profile) = resolve_runtime_profile_entry(&state.config, profile_id);
    let runtime_key = runtime_session_key(resolved_profile_id, session_id);
    let existing_assignment = state
        .session_app_server_assignments
        .lock()
        .await
        .get(&runtime_key)
        .cloned();
    let client_key = if let Some(client_key) = existing_assignment {
        client_key
    } else {
        let has_cached_runtime_activity =
            state.active_turns.lock().await.contains_key(&runtime_key)
                || state
                    .pending_turn_starts
                    .lock()
                    .await
                    .contains(&runtime_key);
        let desired_client_key = if has_cached_runtime_activity {
            resolved_profile_id.to_string()
        } else {
            format!("{resolved_profile_id}::goal::{session_id}")
        };
        let mut assignments = state.session_app_server_assignments.lock().await;
        assignments
            .entry(runtime_key)
            .or_insert(desired_client_key)
            .clone()
    };
    app_server_client_with_key(state, resolved_profile_id, profile, client_key).await
}

pub(crate) async fn app_server_client_by_key(
    state: &AppState,
    profile_id: &str,
    client_key: &str,
) -> Result<AppServerClient> {
    let (resolved_profile_id, profile) = resolve_runtime_profile_entry(&state.config, profile_id);
    app_server_client_with_key(state, resolved_profile_id, profile, client_key.to_string()).await
}

pub(crate) async fn app_server_client_key_for_session(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> String {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id).0;
    let runtime_key = runtime_session_key(resolved_profile_id, session_id);
    state
        .session_app_server_assignments
        .lock()
        .await
        .get(&runtime_key)
        .cloned()
        .unwrap_or_else(|| resolved_profile_id.to_string())
}

async fn app_server_client_with_key(
    state: &AppState,
    resolved_profile_id: &str,
    profile: &RuntimeProfile,
    client_key: String,
) -> Result<AppServerClient> {
    let client = state
        .app_servers
        .get_or_create_with_key(
            client_key.clone(),
            AppServerProfile {
                id: resolved_profile_id.to_string(),
                codex_home: profile.codex_home.clone(),
            },
        )
        .await;
    register_runtime_profile_monitor(
        state,
        resolved_profile_id,
        &client_key,
        client.subscribe_notifications(),
        client.subscribe_requests(),
    );
    Ok(client)
}

pub(crate) async fn session_ids_for_app_server_client(
    state: &AppState,
    profile_id: &str,
    client_key: &str,
) -> HashSet<String> {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id).0;
    let runtime_key_prefix = format!("profile::{resolved_profile_id}::session-runtime::");
    let assignments = state.session_app_server_assignments.lock().await.clone();
    let mut session_ids = HashSet::new();

    for (runtime_key, assigned_client_key) in &assignments {
        if assigned_client_key != client_key {
            continue;
        }
        if let Some(session_id) = runtime_key.strip_prefix(&runtime_key_prefix) {
            session_ids.insert(session_id.to_string());
        }
    }

    let client_is_default = client_key == resolved_profile_id;
    {
        let active_turns = state.active_turns.lock().await;
        for runtime_key in active_turns.keys() {
            let Some(session_id) = runtime_key.strip_prefix(&runtime_key_prefix) else {
                continue;
            };
            let assigned_client_key = assignments.get(runtime_key);
            if assigned_client_key.is_some_and(|assigned| assigned != client_key) {
                continue;
            }
            if client_is_default || assigned_client_key.is_some() {
                session_ids.insert(session_id.to_string());
            }
        }
    }
    {
        let pending_turn_starts = state.pending_turn_starts.lock().await;
        for runtime_key in pending_turn_starts.iter() {
            let Some(session_id) = runtime_key.strip_prefix(&runtime_key_prefix) else {
                continue;
            };
            let assigned_client_key = assignments.get(runtime_key);
            if assigned_client_key.is_some_and(|assigned| assigned != client_key) {
                continue;
            }
            if client_is_default || assigned_client_key.is_some() {
                session_ids.insert(session_id.to_string());
            }
        }
    }

    if client_is_default {
        let live_runtime_status_session_ids = with_ui_state_read(state, profile_id, |ui_state| {
            let session_ids = ui_state
                .get("runtimeStatusByThreadId")
                .and_then(Value::as_object)
                .map(|entries| {
                    entries
                        .iter()
                        .filter_map(|(session_id, status)| {
                            let runtime_key = runtime_session_key(resolved_profile_id, session_id);
                            if assignments.contains_key(&runtime_key) {
                                return None;
                            }
                            normalized_thread_status(Some(status))
                                .as_deref()
                                .is_some_and(is_live_thread_status)
                                .then(|| session_id.clone())
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            Ok(session_ids)
        })
        .await
        .unwrap_or_default();
        session_ids.extend(live_runtime_status_session_ids);
    }

    session_ids
}

pub(crate) async fn clear_app_server_assignments_for_sessions(
    state: &AppState,
    profile_id: &str,
    session_ids: &[String],
) {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id).0;
    let mut assignments = state.session_app_server_assignments.lock().await;
    for session_id in session_ids {
        assignments.remove(&runtime_session_key(resolved_profile_id, session_id));
    }
}

pub(crate) fn resolve_runtime_profile_entry<'a>(
    config: &'a Config,
    profile_id: &'a str,
) -> (&'a str, &'a RuntimeProfile) {
    if let Some(profile) = config.profiles.get(profile_id) {
        return (profile_id, profile);
    }

    if let Some(profile) = config.profiles.get(&config.default_profile_id) {
        return (config.default_profile_id.as_str(), profile);
    }

    config
        .profiles
        .iter()
        .next()
        .map(|(resolved_profile_id, profile)| (resolved_profile_id.as_str(), profile))
        .expect("at least one runtime profile must exist")
}

pub(crate) fn resolve_runtime_profile<'a>(
    config: &'a Config,
    profile_id: &'a str,
) -> &'a RuntimeProfile {
    resolve_runtime_profile_entry(config, profile_id).1
}

pub(crate) async fn read_codex_version(state: &AppState) -> Result<String> {
    let output = run_command_with_timeout(
        &state.config.codex_bin,
        vec!["--version".to_string()],
        Duration::from_secs(5),
    )
    .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let message = if !stderr.is_empty() { stderr } else { stdout };
        anyhow::bail!(if message.is_empty() {
            "Codex binary did not report a version.".to_string()
        } else {
            message
        });
    }

    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if version.is_empty() {
        anyhow::bail!("Codex version output was empty.");
    }
    Ok(version)
}

pub(crate) async fn fetch_latest_published_version() -> Result<Option<String>> {
    let output = run_command_with_timeout(
        npm_command(),
        vec![
            "view".to_string(),
            CODEX_NPM_PACKAGE.to_string(),
            "version".to_string(),
            "--json".to_string(),
        ],
        NPM_VIEW_TIMEOUT,
    )
    .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let message = if !stderr.is_empty() { stderr } else { stdout };
        anyhow::bail!(if message.is_empty() {
            "Failed to query npm for the latest Codex version.".to_string()
        } else {
            message
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        return Ok(None);
    }

    if let Ok(value) = serde_json::from_str::<String>(&stdout) {
        return Ok(Some(value));
    }

    Ok(Some(stdout))
}

pub(crate) async fn run_command_with_timeout(
    command: &str,
    args: Vec<String>,
    timeout: Duration,
) -> Result<std::process::Output> {
    let mut command_builder = Command::new(command);
    command_builder
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        command_builder.process_group(0);
    }
    let mut child = command_builder
        .spawn()
        .with_context(|| format!("failed to start `{command}`"))?;
    let child_pid = child.id();
    let stdout = child
        .stdout
        .take()
        .with_context(|| format!("failed to capture `{command}` stdout"))?;
    let stderr = child
        .stderr
        .take()
        .with_context(|| format!("failed to capture `{command}` stderr"))?;

    match tokio::time::timeout(timeout, async {
        let (status, stdout, stderr) = tokio::try_join!(
            async {
                child
                    .wait()
                    .await
                    .with_context(|| format!("failed to wait for `{command}`"))
            },
            read_child_pipe_limited(stdout, "stdout"),
            read_child_pipe_limited(stderr, "stderr")
        )?;
        Result::<std::process::Output>::Ok(std::process::Output {
            status,
            stdout,
            stderr,
        })
    })
    .await
    {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => {
            terminate_child_process_group(&mut child, child_pid).await;
            Err(error).with_context(|| format!("failed to wait for `{command}`"))
        }
        Err(_) => {
            terminate_child_process_group(&mut child, child_pid).await;
            Err(anyhow!("`{command}` timed out"))
        }
    }
}

pub(crate) async fn terminate_child_process_group(child: &mut Child, _child_pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = _child_pid {
        let group = format!("-{pid}");
        let _ = Command::new("kill")
            .args(["-TERM", "--", group.as_str()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        let _ = Command::new("kill")
            .args(["-KILL", "--", group.as_str()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}

pub(crate) async fn read_child_pipe_limited<R>(mut reader: R, label: &str) -> Result<Vec<u8>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut output = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let count = reader.read(&mut chunk).await?;
        if count == 0 {
            break;
        }
        if output.len().saturating_add(count) > CHILD_OUTPUT_LIMIT_BYTES {
            return Err(anyhow!(
                "command {label} exceeded the {CHILD_OUTPUT_LIMIT_BYTES} byte output limit"
            ));
        }
        output.extend_from_slice(&chunk[..count]);
    }
    Ok(output)
}

pub(crate) async fn command_available(name: &str) -> bool {
    run_command_with_timeout(
        which_command(),
        vec![name.to_string()],
        Duration::from_secs(2),
    )
    .await
    .map(|output| output.status.success())
    .unwrap_or(false)
}

pub(crate) async fn resolve_binary_path(command: &str) -> Option<String> {
    let candidate = PathBuf::from(command);
    if candidate.exists() {
        return Some(candidate.display().to_string());
    }

    let output = run_command_with_timeout(
        which_command(),
        vec![command.to_string()],
        Duration::from_secs(2),
    )
    .await
    .ok()?;
    if !output.status.success() {
        return None;
    }

    let path = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()?
        .trim()
        .to_string();
    if path.is_empty() { None } else { Some(path) }
}

pub(crate) fn which_command() -> &'static str {
    if cfg!(windows) { "where" } else { "which" }
}

pub(crate) fn npm_command() -> &'static str {
    if cfg!(windows) { "npm.cmd" } else { "npm" }
}

pub(crate) fn extract_semver(value: &str) -> Option<(u64, u64, u64)> {
    let mut parts = value
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .find(|part| part.split('.').count() >= 3)?
        .split('.');

    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    Some((major, minor, patch))
}

pub(crate) fn compare_versions(left: &(u64, u64, u64), right: &(u64, u64, u64)) -> i8 {
    match left.cmp(right) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}
