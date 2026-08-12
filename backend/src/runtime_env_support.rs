use super::*;

const APP_SERVER_ASSIGNMENTS_VERSION: u32 = 1;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PersistedAppServerAssignments {
    version: u32,
    assignments: HashMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionAssignmentFence {
    runtime_key: String,
    client_key: String,
    epoch: u64,
}

fn app_server_assignments_path(state: &AppState) -> PathBuf {
    state.config.data_dir.join("app-server-assignments.json")
}

fn loaded_assignment_stores() -> &'static tokio::sync::Mutex<HashSet<PathBuf>> {
    static STORES: std::sync::OnceLock<tokio::sync::Mutex<HashSet<PathBuf>>> =
        std::sync::OnceLock::new();
    STORES.get_or_init(|| tokio::sync::Mutex::new(HashSet::new()))
}

fn assignment_store_write_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn valid_persisted_assignment(runtime_key: &str, client_key: &str) -> bool {
    let Some(rest) = runtime_key.strip_prefix("profile::") else {
        return false;
    };
    let Some((profile_id, session_id)) = rest.split_once("::session-runtime::") else {
        return false;
    };
    if profile_id.is_empty() || session_id.is_empty() {
        return false;
    }
    client_key == profile_id
        || client_key.strip_prefix(profile_id).is_some_and(|suffix| {
            suffix.strip_prefix("::session::") == Some(session_id)
                || suffix.strip_prefix("::goal::") == Some(session_id)
        })
}

async fn ensure_app_server_assignments_loaded(state: &AppState) {
    let path = app_server_assignments_path(state);
    let mut loaded = loaded_assignment_stores().lock().await;
    if loaded.contains(&path) {
        return;
    }
    if let Some(persisted) = read_app_server_assignments(&path).await {
        let mut assignments = state.session_app_server_assignments.lock().await;
        restore_persisted_assignments(&mut assignments, persisted);
    }
    loaded.insert(path);
}

fn restore_persisted_assignments(
    assignments: &mut HashMap<String, String>,
    persisted: PersistedAppServerAssignments,
) {
    for (runtime_key, client_key) in persisted.assignments {
        if valid_persisted_assignment(&runtime_key, &client_key) {
            assignments.entry(runtime_key).or_insert(client_key);
        }
    }
}

async fn persist_app_server_assignments(state: &AppState) -> Result<()> {
    ensure_app_server_assignments_loaded(state).await;
    let _write_guard = assignment_store_write_lock().lock().await;
    let path = app_server_assignments_path(state);
    let assignments = state.session_app_server_assignments.lock().await.clone();
    write_app_server_assignments(&path, assignments).await
}

async fn read_app_server_assignments(path: &Path) -> Option<PersistedAppServerAssignments> {
    tokio::fs::read(path)
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice::<PersistedAppServerAssignments>(&bytes).ok())
        .filter(|payload| payload.version == APP_SERVER_ASSIGNMENTS_VERSION)
}

async fn write_app_server_assignments(
    path: &Path,
    assignments: HashMap<String, String>,
) -> Result<()> {
    let payload = PersistedAppServerAssignments {
        version: APP_SERVER_ASSIGNMENTS_VERSION,
        assignments,
    };
    let bytes = serde_json::to_vec_pretty(&payload)
        .context("failed to encode app-server assignment metadata")?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("app-server assignment path has no parent"))?;
    tokio::fs::create_dir_all(parent).await?;
    let temp_path = parent.join(format!(
        ".app-server-assignments.tmp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let mut file = tokio::fs::File::create(&temp_path).await?;
    file.write_all(&bytes).await?;
    file.sync_all().await?;
    drop(file);
    tokio::fs::rename(&temp_path, &path).await?;
    if let Ok(directory) = tokio::fs::File::open(parent).await {
        let _ = directory.sync_all().await;
    }
    Ok(())
}

pub(crate) async fn app_server_client(
    state: &AppState,
    profile_id: &str,
) -> Result<AppServerClient> {
    let (resolved_profile_id, profile) = resolve_runtime_profile_entry(&state.config, profile_id);
    app_server_client_with_key(
        state,
        &resolved_profile_id,
        &profile,
        resolved_profile_id.to_string(),
    )
    .await
}

pub(crate) async fn app_server_client_for_session(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> Result<AppServerClient> {
    ensure_app_server_assignments_loaded(state).await;
    let (resolved_profile_id, profile) = resolve_runtime_profile_entry(&state.config, profile_id);
    let runtime_key = runtime_session_key(&resolved_profile_id, session_id);
    let client_key = state
        .session_app_server_assignments
        .lock()
        .await
        .get(&runtime_key)
        .cloned()
        .unwrap_or_else(|| resolved_profile_id.to_string());
    app_server_client_with_key(state, &resolved_profile_id, &profile, client_key).await
}

pub(crate) async fn app_server_client_for_session_turn(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> Result<AppServerClient> {
    ensure_app_server_assignments_loaded(state).await;
    let (resolved_profile_id, profile) = resolve_runtime_profile_entry(&state.config, profile_id);
    let runtime_key = runtime_session_key(&resolved_profile_id, session_id);
    let cached_goal_uses_goal_app_server = with_ui_state_read(state, profile_id, |ui_state| {
        Ok(ui_state
            .get("goalsByThreadId")
            .and_then(Value::as_object)
            .and_then(|goals| goals.get(session_id))
            .and_then(|goal| goal.get("status"))
            .and_then(Value::as_str)
            .is_some_and(goal_status_uses_goal_app_server))
    })
    .await
    .unwrap_or(false);
    let client_key = {
        let assignments_snapshot = state.session_app_server_assignments.lock().await.clone();
        let runtime_key_prefix = format!("profile::{resolved_profile_id}::session-runtime::");
        let default_client_key = resolved_profile_id.to_string();
        let active_runtime_keys = state
            .active_turns
            .lock()
            .await
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let pending_runtime_keys = state
            .pending_turn_starts
            .lock()
            .await
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let another_default_client_session_is_active = active_runtime_keys
            .iter()
            .chain(pending_runtime_keys.iter())
            .any(|other_runtime_key| {
                other_runtime_key != &runtime_key
                    && other_runtime_key.starts_with(&runtime_key_prefix)
                    && assignments_snapshot.get(other_runtime_key).is_none_or(
                        |assigned_client_key| assigned_client_key == &default_client_key,
                    )
            });
        let desired_client_key = if state.config.per_session_app_servers {
            format!("{resolved_profile_id}::session::{session_id}")
        } else if cached_goal_uses_goal_app_server {
            format!("{resolved_profile_id}::goal::{session_id}")
        } else if another_default_client_session_is_active {
            format!("{resolved_profile_id}::session::{session_id}")
        } else {
            resolved_profile_id.to_string()
        };
        let mut assignments = state.session_app_server_assignments.lock().await;
        assignments
            .entry(runtime_key)
            .or_insert(desired_client_key)
            .clone()
    };
    if let Err(error) = persist_app_server_assignments(state).await {
        warn!(
            profile_id = %resolved_profile_id,
            session_id,
            error = %error,
            "failed to persist app-server assignment"
        );
    }
    app_server_client_with_key(state, &resolved_profile_id, &profile, client_key).await
}

pub(crate) async fn app_server_client_for_goal_session(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> Result<AppServerClient> {
    ensure_app_server_assignments_loaded(state).await;
    let (resolved_profile_id, profile) = resolve_runtime_profile_entry(&state.config, profile_id);
    let runtime_key = runtime_session_key(&resolved_profile_id, session_id);
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
    if let Err(error) = persist_app_server_assignments(state).await {
        warn!(
            profile_id = %resolved_profile_id,
            session_id,
            error = %error,
            "failed to persist goal app-server assignment"
        );
    }
    app_server_client_with_key(state, &resolved_profile_id, &profile, client_key).await
}

pub(crate) async fn app_server_client_by_key(
    state: &AppState,
    profile_id: &str,
    client_key: &str,
) -> Result<AppServerClient> {
    let (resolved_profile_id, profile) = resolve_runtime_profile_entry(&state.config, profile_id);
    app_server_client_with_key(
        state,
        &resolved_profile_id,
        &profile,
        client_key.to_string(),
    )
    .await
}

pub(crate) async fn app_server_client_key_for_session(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> String {
    ensure_app_server_assignments_loaded(state).await;
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id).0;
    let runtime_key = runtime_session_key(&resolved_profile_id, session_id);
    state
        .session_app_server_assignments
        .lock()
        .await
        .get(&runtime_key)
        .cloned()
        .unwrap_or_else(|| resolved_profile_id.to_string())
}

pub(crate) async fn capture_session_assignment_fence(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> SessionAssignmentFence {
    ensure_app_server_assignments_loaded(state).await;
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id).0;
    let runtime_key = runtime_session_key(&resolved_profile_id, session_id);
    let assignments = state.session_app_server_assignments.lock().await;
    let epochs = state.session_assignment_epochs.lock().await;
    SessionAssignmentFence {
        client_key: assignments
            .get(&runtime_key)
            .cloned()
            .unwrap_or(resolved_profile_id),
        epoch: epochs.get(&runtime_key).copied().unwrap_or_default(),
        runtime_key,
    }
}

pub(crate) async fn session_assignment_fence_is_current(
    state: &AppState,
    fence: &SessionAssignmentFence,
) -> bool {
    ensure_app_server_assignments_loaded(state).await;
    let Some((profile_id, _)) = fence
        .runtime_key
        .strip_prefix("profile::")
        .and_then(|value| value.split_once("::session-runtime::"))
    else {
        return false;
    };
    let assignments = state.session_app_server_assignments.lock().await;
    let epochs = state.session_assignment_epochs.lock().await;
    let current_client_key = assignments
        .get(&fence.runtime_key)
        .map(String::as_str)
        .unwrap_or(profile_id);
    current_client_key == fence.client_key
        && epochs.get(&fence.runtime_key).copied().unwrap_or_default() == fence.epoch
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
    // Subscribe before taking the snapshot so requests created during monitor
    // registration are either in the snapshot, in the receiver, or both.
    // The request handler is idempotent for the possible overlap.
    let notifications = client.subscribe_notifications();
    let requests = client.subscribe_requests();
    let pending_requests = client.pending_server_requests().await;
    register_runtime_profile_monitor(
        state,
        resolved_profile_id,
        &client_key,
        client.instance_id(),
        notifications,
        requests,
        pending_requests,
    );
    Ok(client)
}

pub(crate) async fn session_ids_for_app_server_client(
    state: &AppState,
    profile_id: &str,
    client_key: &str,
) -> HashSet<String> {
    ensure_app_server_assignments_loaded(state).await;
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
            if client_key == resolved_profile_id || assigned_client_key.is_some() {
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
            if client_key == resolved_profile_id || assigned_client_key.is_some() {
                session_ids.insert(session_id.to_string());
            }
        }
    }

    session_ids
}

pub(crate) async fn clear_app_server_assignments_for_sessions(
    state: &AppState,
    profile_id: &str,
    session_ids: &[String],
) {
    ensure_app_server_assignments_loaded(state).await;
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id).0;
    let mut assignments = state.session_app_server_assignments.lock().await;
    let mut epochs = state.session_assignment_epochs.lock().await;
    for session_id in session_ids {
        let runtime_key = runtime_session_key(&resolved_profile_id, session_id);
        assignments.remove(&runtime_key);
        let epoch = epochs.entry(runtime_key).or_default();
        *epoch = epoch.saturating_add(1);
    }
    drop(epochs);
    drop(assignments);
    if let Err(error) = persist_app_server_assignments(state).await {
        warn!(
            profile_id = %resolved_profile_id,
            error = %error,
            "failed to persist cleared app-server assignments"
        );
    }
}

pub(crate) fn resolve_runtime_profile_entry(
    config: &Config,
    profile_id: &str,
) -> (String, RuntimeProfile) {
    let (default_profile_id, profiles) = runtime_profiles_snapshot(config);
    if let Some(profile) = profiles.get(profile_id) {
        return (profile_id.to_string(), profile.clone());
    }

    if let Some(profile) = profiles.get(&default_profile_id) {
        return (default_profile_id, profile.clone());
    }

    profiles
        .iter()
        .next()
        .map(|(resolved_profile_id, profile)| (resolved_profile_id.clone(), profile.clone()))
        .expect("at least one runtime profile must exist")
}

pub(crate) fn resolve_runtime_profile(config: &Config, profile_id: &str) -> RuntimeProfile {
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

#[cfg(test)]
mod assignment_tests {
    use super::*;

    #[tokio::test]
    async fn dedicated_session_client_key_survives_assignment_store_restart() {
        let directory =
            std::env::temp_dir().join(format!("codex-webui-assignment-restart-{}", Uuid::new_v4()));
        let path = directory.join("app-server-assignments.json");
        let runtime_key = "profile::work::session-runtime::thread-1".to_string();
        let client_key = "work::goal::thread-1".to_string();
        write_app_server_assignments(
            &path,
            HashMap::from([(runtime_key.clone(), client_key.clone())]),
        )
        .await
        .expect("assignment metadata should persist atomically");

        let persisted = read_app_server_assignments(&path)
            .await
            .expect("a restarted gateway should read assignment metadata");
        let mut restored = HashMap::new();
        restore_persisted_assignments(&mut restored, persisted);
        assert_eq!(restored.get(&runtime_key), Some(&client_key));
        assert!(valid_persisted_assignment(&runtime_key, &client_key));

        let _ = tokio::fs::remove_dir_all(directory).await;
    }
}
