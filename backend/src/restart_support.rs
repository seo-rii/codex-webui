use super::*;
use std::fs::OpenOptions;

fn shell_restart_plan(command: &str, cwd: PathBuf) -> RestartPlan {
    #[cfg(windows)]
    {
        RestartPlan {
            command: "cmd".to_string(),
            args: vec!["/C".to_string(), command.to_string()],
            cwd: Some(cwd),
            mode: "command",
        }
    }

    #[cfg(not(windows))]
    {
        RestartPlan {
            command: "sh".to_string(),
            args: vec!["-lc".to_string(), command.to_string()],
            cwd: Some(cwd),
            mode: "command",
        }
    }
}

fn strip_deleted_suffix(path: PathBuf) -> PathBuf {
    let value = path.to_string_lossy();
    if let Some(stripped) = value.strip_suffix(" (deleted)") {
        return PathBuf::from(stripped);
    }
    path
}

pub(crate) fn build_gateway_restart_plan(config: &Config) -> Result<RestartPlan> {
    if let Some(command) = config.restart_command.as_deref().map(str::trim) {
        if !command.is_empty() {
            return Ok(shell_restart_plan(command, config.project_root.clone()));
        }
    }

    let command = env::var("CODEX_WEBUI_RESTART_BINARY")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            env::current_exe()
                .map(strip_deleted_suffix)
                .unwrap_or_else(|_| PathBuf::from("backend"))
        });
    if !command.is_file() {
        anyhow::bail!(
            "restart binary is not available at {}. Set CODEX_WEBUI_RESTART_COMMAND to the replacement gateway start command.",
            command.display()
        );
    }

    #[cfg(windows)]
    {
        Ok(RestartPlan {
            command: command.display().to_string(),
            args: env::args().skip(1).collect(),
            cwd: env::current_dir().ok(),
            mode: "current-binary",
        })
    }

    #[cfg(not(windows))]
    {
        let mut args = vec![
            "-lc".to_string(),
            r#"if [ -n "${NVM_BIN:-}" ]; then
  PATH="$NVM_BIN:$PATH"
  export PATH
fi
status=1
for attempt in $(seq 1 30); do
  "$@"
  status=$?
  if [ "$status" -eq 0 ]; then
    exit 0
  fi
  sleep 0.5
done
exit "$status""#
                .to_string(),
            "codex-webui-restart".to_string(),
            command.display().to_string(),
        ];
        args.extend(env::args().skip(1));
        Ok(RestartPlan {
            command: "sh".to_string(),
            args,
            cwd: env::current_dir().ok(),
            mode: "current-binary",
        })
    }
}

pub(crate) async fn prepare_gateway_restart_payload(state: &AppState) -> ApiResult<Value> {
    let plan = build_gateway_restart_plan(&state.config)
        .map_err(|error| api_error(StatusCode::SERVICE_UNAVAILABLE, error.to_string()))?;
    let mode = plan.mode;
    state
        .preserve_app_servers_on_shutdown
        .store(true, Ordering::SeqCst);

    spawn_gateway_restart(&state.config, plan)
        .await
        .map_err(|error| api_error(StatusCode::SERVICE_UNAVAILABLE, error.to_string()))?;

    let shutdown_notify = state.shutdown_notify.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(250)).await;
        shutdown_notify.notify_waiters();
    });

    Ok(json!({
        "ok": true,
        "handoffPrepared": true,
        "restartScheduled": true,
        "mode": mode
    }))
}

pub(crate) async fn spawn_gateway_restart(config: &Config, plan: RestartPlan) -> Result<()> {
    let mut command = Command::new(&plan.command);
    command.args(&plan.args).stdin(Stdio::null());
    if let Some(cwd) = &plan.cwd {
        command.current_dir(cwd);
    }
    #[cfg(unix)]
    {
        command.process_group(0);
    }

    let log_path = runtime_logs_dir(config).join("gateway-restart.log");
    if let Some(parent) = log_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    match OpenOptions::new().create(true).append(true).open(&log_path) {
        Ok(log_file) => match log_file.try_clone() {
            Ok(stderr_file) => {
                command.stdout(Stdio::from(log_file));
                command.stderr(Stdio::from(stderr_file));
            }
            Err(_) => {
                command.stdout(Stdio::null());
                command.stderr(Stdio::null());
            }
        },
        Err(_) => {
            command.stdout(Stdio::null());
            command.stderr(Stdio::null());
        }
    }

    let child = command.spawn().with_context(|| {
        format!(
            "failed to start restart command `{}` with {} args",
            plan.command,
            plan.args.len()
        )
    })?;
    info!(
        pid = child.id().unwrap_or_default(),
        mode = plan.mode,
        "started replacement codex-webui gateway"
    );
    Ok(())
}
