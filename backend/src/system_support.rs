use super::*;

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) struct SystemShutdownPlan {
    pub(crate) command: String,
    pub(crate) args: Vec<String>,
    pub(crate) availability_check: Option<(String, Vec<String>)>,
}

pub(crate) async fn is_root_user() -> bool {
    run_command_with_timeout("id", vec!["-u".to_string()], Duration::from_secs(2))
        .await
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim() == "0")
        .unwrap_or(false)
}

pub(crate) async fn resolve_command_path(command: &str) -> Option<String> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return None;
    }

    let candidate = PathBuf::from(trimmed);
    if candidate.is_absolute() || trimmed.contains('/') || trimmed.contains('\\') {
        if candidate.exists() {
            return Some(candidate.display().to_string());
        }
    }

    resolve_binary_path(trimmed).await
}

pub(crate) async fn resolve_system_shutdown_plan(config: &Config) -> Option<SystemShutdownPlan> {
    if !config.system_shutdown_enabled {
        return None;
    }

    if cfg!(windows) {
        let command = config
            .system_shutdown_command_override
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("shutdown")
            .to_string();
        return Some(SystemShutdownPlan {
            command,
            args: if config.system_shutdown_command_override.is_some() {
                Vec::new()
            } else {
                vec!["/s".to_string(), "/t".to_string(), "0".to_string()]
            },
            availability_check: None,
        });
    }

    let override_command = config
        .system_shutdown_command_override
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .to_string();
    let direct_command = if !override_command.is_empty() {
        resolve_command_path(&override_command).await
    } else if let Some(command) = resolve_command_path("shutdown").await {
        Some(command)
    } else if let Some(command) = resolve_command_path("/usr/sbin/shutdown").await {
        Some(command)
    } else if let Some(command) = resolve_command_path("/sbin/shutdown").await {
        Some(command)
    } else {
        resolve_command_path("systemctl").await
    }?;

    let direct_args = if !override_command.is_empty() {
        Vec::new()
    } else if Path::new(&direct_command)
        .file_name()
        .and_then(|value| value.to_str())
        == Some("systemctl")
    {
        vec!["poweroff".to_string()]
    } else {
        vec!["-h".to_string(), "now".to_string()]
    };

    if is_root_user().await {
        return Some(SystemShutdownPlan {
            command: direct_command,
            args: direct_args,
            availability_check: None,
        });
    }

    let sudo_command = resolve_command_path("sudo").await?;
    let mut sudo_args = vec!["-n".to_string(), direct_command.clone()];
    sudo_args.extend(direct_args.clone());
    let mut check_args = vec!["-n".to_string(), "-l".to_string(), direct_command];
    check_args.extend(direct_args);
    Some(SystemShutdownPlan {
        command: sudo_command.clone(),
        args: sudo_args,
        availability_check: Some((sudo_command, check_args)),
    })
}

pub(crate) async fn system_shutdown_capability(
    config: &Config,
) -> (bool, Option<SystemShutdownPlan>) {
    let Some(plan) = resolve_system_shutdown_plan(config).await else {
        return (false, None);
    };

    let Some((check_command, check_args)) = plan.availability_check.clone() else {
        return (true, Some(plan));
    };

    let available =
        run_command_with_timeout(&check_command, check_args, Duration::from_millis(1500))
            .await
            .map(|output| output.status.success())
            .unwrap_or(false);
    if available {
        (true, Some(plan))
    } else {
        (false, None)
    }
}
