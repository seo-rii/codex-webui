use super::*;

pub(crate) async fn list_terminal_summaries(state: &AppState) -> Vec<TerminalSummaryState> {
    let terminals = {
        let current = state.terminals.lock().await;
        current.values().cloned().collect::<Vec<_>>()
    };

    let mut summaries = Vec::with_capacity(terminals.len());
    for terminal in terminals {
        summaries.push(terminal.summary().await);
    }
    summaries.sort_by(|left, right| right.last_activity_at.cmp(&left.last_activity_at));
    summaries
}

pub(crate) async fn emit_terminals_updated(state: &AppState) {
    emit_global_notification(
        state,
        json!({
            "kind": "notification",
            "method": "codex-webui/terminalsUpdated",
            "params": {
                "terminals": list_terminal_summaries(state).await
            }
        }),
    )
    .await;
}

pub(crate) async fn get_terminal_session(
    state: &AppState,
    terminal_id: &str,
) -> Result<Arc<TerminalSession>> {
    state
        .terminals
        .lock()
        .await
        .get(terminal_id)
        .cloned()
        .ok_or_else(|| anyhow!("Terminal not found."))
}

async fn validate_terminal_cwd(state: &AppState, requested_cwd: Option<String>) -> Result<String> {
    let candidate = requested_cwd
        .map(PathBuf::from)
        .unwrap_or_else(|| state.config.project_root.clone());
    let resolved = fs::canonicalize(&candidate).with_context(|| {
        format!(
            "terminal working directory is invalid: {}",
            candidate.display()
        )
    })?;
    let metadata = fs::metadata(&resolved)
        .with_context(|| format!("failed to inspect {}", resolved.display()))?;
    if !metadata.is_dir() {
        anyhow::bail!("terminal working directory must be a directory.");
    }

    let allowed_roots = resolved_allowed_roots(&state.config).await;
    let allowed = allowed_roots
        .iter()
        .any(|root| path_is_within(root, &resolved));

    if !allowed {
        anyhow::bail!("terminal working directory must stay within allowed roots.");
    }

    Ok(resolved.display().to_string())
}

async fn spawn_terminal_process(cwd: &str) -> Result<Child> {
    if cfg!(windows) {
        Command::new("powershell.exe")
            .current_dir(cwd)
            .arg("-NoLogo")
            .arg("-NoExit")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("failed to start terminal process")
    } else {
        Command::new("script")
            .current_dir(cwd)
            .args([
                "-q",
                "-f",
                "-c",
                "env TERM=xterm-256color bash --noprofile --norc -i",
                "/dev/null",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("failed to start terminal process")
    }
}

pub(crate) async fn create_terminal(
    state: AppState,
    cwd: Option<String>,
    title: Option<String>,
) -> Result<Value> {
    let cwd = validate_terminal_cwd(&state, cwd).await?;
    let mut child = spawn_terminal_process(&cwd).await?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("terminal stdout unavailable"))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("terminal stdin unavailable"))?;
    let terminal_id = Uuid::new_v4().to_string();
    let created_at = now_unix_ms();
    let (relay, _) = broadcast::channel(256);
    let session = Arc::new(TerminalSession {
        summary: Mutex::new(TerminalSummaryState {
            id: terminal_id.clone(),
            title: title
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| {
                    PathBuf::from(&cwd)
                        .file_name()
                        .and_then(|value| value.to_str())
                        .filter(|value| !value.is_empty())
                        .map(|value| format!("{value} shell"))
                        .unwrap_or_else(|| "Terminal".to_string())
                }),
            cwd: cwd.clone(),
            created_at,
            last_activity_at: created_at,
            status: "running".to_string(),
            exit_code: None,
        }),
        buffer: Mutex::new(String::new()),
        stdin: Mutex::new(Some(stdin)),
        relay,
        pid: child.id(),
    });

    state
        .terminals
        .lock()
        .await
        .insert(terminal_id.clone(), session.clone());

    let output_session = session.clone();
    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout);
        let mut buffer = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) => break,
                Ok(read) => {
                    let text = String::from_utf8_lossy(&buffer[..read]).to_string();
                    output_session.append_output(&text).await;
                }
                Err(error) => {
                    warn!("terminal output stream failed: {error:#}");
                    break;
                }
            }
        }
    });

    let exit_session = session.clone();
    let exit_state = state.clone();
    tokio::spawn(async move {
        let exit_code = child.wait().await.ok().and_then(|status| status.code());
        exit_session.mark_exited(exit_code).await;
        emit_terminals_updated(&exit_state).await;
    });

    emit_terminals_updated(&state).await;

    let (summary, snapshot) = session.snapshot().await;
    Ok(json!({
        "terminal": summary,
        "snapshot": snapshot
    }))
}

pub(crate) async fn list_terminals(state: &AppState) -> Result<Value> {
    Ok(json!({
        "terminals": list_terminal_summaries(state).await
    }))
}

pub(crate) async fn read_terminal(state: &AppState, terminal_id: &str) -> Result<Value> {
    let session = get_terminal_session(state, terminal_id).await?;
    let (summary, snapshot) = session.snapshot().await;
    Ok(json!({
        "terminal": summary,
        "snapshot": snapshot
    }))
}

pub(crate) async fn write_terminal_input(
    state: &AppState,
    terminal_id: &str,
    data: &str,
) -> Result<Value> {
    let session = get_terminal_session(state, terminal_id).await?;
    session.write_input(data).await?;
    Ok(json!({ "ok": true }))
}

pub(crate) async fn close_terminal(state: AppState, terminal_id: &str) -> Result<Value> {
    let session = state
        .terminals
        .lock()
        .await
        .remove(terminal_id)
        .ok_or_else(|| anyhow!("Terminal not found."))?;

    let _ = session.write_input("exit\r").await;
    if let Some(pid) = session.pid {
        let _ = terminate_process(pid).await;
    }

    emit_terminals_updated(&state).await;
    Ok(json!({ "ok": true }))
}
