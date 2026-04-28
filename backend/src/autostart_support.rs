use super::*;

pub(crate) fn home_dir_path() -> Option<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("USERPROFILE").map(PathBuf::from))
}

pub(crate) fn config_home_path() -> Option<PathBuf> {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir_path().map(|home| home.join(".config")))
}

pub(crate) fn windows_startup_path() -> Option<PathBuf> {
    env::var_os("APPDATA").map(PathBuf::from).map(|value| {
        value
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs")
            .join("Startup")
            .join(WINDOWS_STARTUP_SCRIPT)
    })
}

pub(crate) fn macos_launch_agent_path() -> Option<PathBuf> {
    home_dir_path().map(|home| {
        home.join("Library")
            .join("LaunchAgents")
            .join(MACOS_LAUNCH_AGENT)
    })
}

pub(crate) fn linux_systemd_user_path(config_home: &Path) -> PathBuf {
    config_home
        .join("systemd")
        .join("user")
        .join(LINUX_SYSTEMD_SERVICE)
}

pub(crate) fn linux_desktop_entry_path(config_home: &Path) -> PathBuf {
    config_home.join("autostart").join(LINUX_DESKTOP_ENTRY)
}

pub(crate) async fn path_exists_async(path: Option<&Path>) -> bool {
    match path {
        Some(path) => tokio_fs::metadata(path).await.is_ok(),
        None => false,
    }
}

pub(crate) fn current_launch_command(config: &Config) -> Result<(PathBuf, PathBuf)> {
    let executable = env::current_exe().context("failed to resolve the current executable")?;
    if !executable.exists() {
        anyhow::bail!(
            "Could not resolve the codex-webui executable at {}.",
            executable.display()
        );
    }
    Ok((executable, config.project_root.clone()))
}

fn escape_windows_vbs_string(value: &str) -> String {
    value.replace('"', "\"\"")
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn escape_systemd_value(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn escape_desktop_value(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

pub(crate) async fn can_use_linux_systemd_user() -> bool {
    run_command_with_timeout(
        "systemctl",
        vec!["--user".to_string(), "show-environment".to_string()],
        Duration::from_secs(3),
    )
    .await
    .map(|output| output.status.success())
    .unwrap_or(false)
}

pub(crate) async fn preferred_linux_autostart_provider(config_home: &Path) -> &'static str {
    if path_exists_async(Some(linux_systemd_user_path(config_home).as_path())).await {
        return "linux-systemd-user";
    }
    if path_exists_async(Some(linux_desktop_entry_path(config_home).as_path())).await {
        return "linux-xdg-autostart";
    }
    if can_use_linux_systemd_user().await {
        "linux-systemd-user"
    } else {
        "linux-xdg-autostart"
    }
}

pub(crate) async fn get_autostart_state(config: &Config) -> Result<Value> {
    if current_launch_command(config).is_err() {
        return Ok(json!({
            "available": false,
            "enabled": false,
            "provider": Value::Null,
            "location": Value::Null
        }));
    }

    if cfg!(windows) {
        let location = windows_startup_path();
        return Ok(json!({
            "available": location.is_some(),
            "enabled": path_exists_async(location.as_deref()).await,
            "provider": location.as_ref().map(|_| "windows-startup"),
            "location": location.map(|value| value.display().to_string())
        }));
    }

    if cfg!(target_os = "macos") {
        let location = macos_launch_agent_path();
        return Ok(json!({
            "available": location.is_some(),
            "enabled": path_exists_async(location.as_deref()).await,
            "provider": location.as_ref().map(|_| "macos-launch-agent"),
            "location": location.map(|value| value.display().to_string())
        }));
    }

    if cfg!(target_os = "linux") {
        let Some(config_home) = config_home_path() else {
            return Ok(json!({
                "available": false,
                "enabled": false,
                "provider": Value::Null,
                "location": Value::Null
            }));
        };
        let provider = preferred_linux_autostart_provider(&config_home).await;
        let location = if provider == "linux-systemd-user" {
            linux_systemd_user_path(&config_home)
        } else {
            linux_desktop_entry_path(&config_home)
        };
        return Ok(json!({
            "available": true,
            "enabled": path_exists_async(Some(location.as_path())).await,
            "provider": provider,
            "location": location.display().to_string()
        }));
    }

    Ok(json!({
        "available": false,
        "enabled": false,
        "provider": Value::Null,
        "location": Value::Null
    }))
}

pub(crate) async fn write_windows_startup_script(config: &Config) -> Result<()> {
    let target_path =
        windows_startup_path().ok_or_else(|| anyhow!("Windows startup folder is unavailable."))?;
    let (executable, working_directory) = current_launch_command(config)?;
    if let Some(parent) = target_path.parent() {
        tokio_fs::create_dir_all(parent)
            .await
            .context("failed to create the Windows startup directory")?;
    }
    write_file_atomically(
        &target_path,
        [
            "Set WshShell = CreateObject(\"WScript.Shell\")".to_string(),
            format!(
                "WshShell.CurrentDirectory = \"{}\"",
                escape_windows_vbs_string(&working_directory.display().to_string())
            ),
            format!(
                "WshShell.Run \"\"\"\" & \"{}\" & \"\"\"\", 0, False",
                escape_windows_vbs_string(&executable.display().to_string())
            ),
        ]
        .join("\r\n")
        .into_bytes(),
    )
    .await
    .context("failed to write the Windows startup script")?;
    Ok(())
}

pub(crate) async fn write_macos_launch_agent(config: &Config) -> Result<()> {
    let target_path = macos_launch_agent_path()
        .ok_or_else(|| anyhow!("LaunchAgents directory is unavailable."))?;
    let (executable, working_directory) = current_launch_command(config)?;
    if let Some(parent) = target_path.parent() {
        tokio_fs::create_dir_all(parent)
            .await
            .context("failed to create the LaunchAgents directory")?;
    }
    let log_path = config.data_dir.join("autostart-launch.log");
    if let Some(parent) = log_path.parent() {
        tokio_fs::create_dir_all(parent)
            .await
            .context("failed to create the autostart log directory")?;
    }
    write_file_atomically(
        &target_path,
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n  <dict>\n    <key>Label</key>\n    <string>{}</string>\n    <key>ProgramArguments</key>\n    <array>\n      <string>{}</string>\n    </array>\n    <key>WorkingDirectory</key>\n    <string>{}</string>\n    <key>RunAtLoad</key>\n    <true/>\n    <key>KeepAlive</key>\n    <false/>\n    <key>StandardOutPath</key>\n    <string>{}</string>\n    <key>StandardErrorPath</key>\n    <string>{}</string>\n  </dict>\n</plist>\n",
            AUTOSTART_LABEL,
            escape_xml(&executable.display().to_string()),
            escape_xml(&working_directory.display().to_string()),
            escape_xml(&log_path.display().to_string()),
            escape_xml(&log_path.display().to_string())
        )
        .into_bytes(),
    )
    .await
    .context("failed to write the launch agent")?;

    if let Ok(uid) = env::var("UID").or_else(|_| env::var("EUID")) {
        let domain = format!("gui/{uid}");
        let _ = run_command_with_timeout(
            "launchctl",
            vec![
                "bootout".to_string(),
                domain.clone(),
                target_path.display().to_string(),
            ],
            Duration::from_secs(4),
        )
        .await;
        let _ = run_command_with_timeout(
            "launchctl",
            vec![
                "bootstrap".to_string(),
                domain,
                target_path.display().to_string(),
            ],
            Duration::from_secs(4),
        )
        .await;
    }

    Ok(())
}

pub(crate) async fn write_linux_systemd_user_service(config: &Config) -> Result<()> {
    let config_home =
        config_home_path().ok_or_else(|| anyhow!("XDG config home is unavailable."))?;
    let target_path = linux_systemd_user_path(&config_home);
    let (executable, working_directory) = current_launch_command(config)?;
    if let Some(parent) = target_path.parent() {
        tokio_fs::create_dir_all(parent)
            .await
            .context("failed to create the systemd user directory")?;
    }
    write_file_atomically(
        &target_path,
        format!(
            "[Unit]\nDescription=Codex Web UI autostart\n\n[Service]\nType=simple\nWorkingDirectory={}\nExecStart={}\nRestart=on-failure\nRestartSec=5\n\n[Install]\nWantedBy=default.target\n",
            escape_systemd_value(&working_directory.display().to_string()),
            escape_systemd_value(&executable.display().to_string())
        )
        .into_bytes(),
    )
    .await
    .context("failed to write the systemd user service")?;

    let daemon_reload = run_command_with_timeout(
        "systemctl",
        vec!["--user".to_string(), "daemon-reload".to_string()],
        Duration::from_secs(5),
    )
    .await?;
    if !daemon_reload.status.success() {
        anyhow::bail!("Failed to reload the user systemd daemon.");
    }

    let enable = run_command_with_timeout(
        "systemctl",
        vec![
            "--user".to_string(),
            "enable".to_string(),
            LINUX_SYSTEMD_SERVICE.to_string(),
        ],
        Duration::from_secs(5),
    )
    .await?;
    if !enable.status.success() {
        anyhow::bail!("Failed to enable the user systemd service.");
    }

    Ok(())
}

pub(crate) async fn write_linux_desktop_entry(config: &Config) -> Result<()> {
    let config_home =
        config_home_path().ok_or_else(|| anyhow!("XDG config home is unavailable."))?;
    let target_path = linux_desktop_entry_path(&config_home);
    let (executable, working_directory) = current_launch_command(config)?;
    if let Some(parent) = target_path.parent() {
        tokio_fs::create_dir_all(parent)
            .await
            .context("failed to create the desktop autostart directory")?;
    }
    write_file_atomically(
        &target_path,
        format!(
            "[Desktop Entry]\nType=Application\nVersion=1.0\nName=Codex Web UI\nComment=Start Codex Web UI automatically when you sign in\nExec={}\nPath={}\nTerminal=false\nX-GNOME-Autostart-enabled=true\nHidden=false\n",
            escape_desktop_value(&executable.display().to_string()),
            escape_desktop_value(&working_directory.display().to_string())
        )
        .into_bytes(),
    )
    .await
    .context("failed to write the desktop autostart entry")?;
    Ok(())
}

pub(crate) async fn disable_windows_startup() {
    if let Some(path) = windows_startup_path() {
        let _ = tokio_fs::remove_file(path).await;
    }
}

pub(crate) async fn disable_macos_launch_agent() {
    if let Some(path) = macos_launch_agent_path() {
        if let Ok(uid) = env::var("UID").or_else(|_| env::var("EUID")) {
            let _ = run_command_with_timeout(
                "launchctl",
                vec![
                    "bootout".to_string(),
                    format!("gui/{uid}"),
                    path.display().to_string(),
                ],
                Duration::from_secs(4),
            )
            .await;
        }
        let _ = tokio_fs::remove_file(path).await;
    }
}

pub(crate) async fn disable_linux_autostart() {
    if let Some(config_home) = config_home_path() {
        let systemd_path = linux_systemd_user_path(&config_home);
        if path_exists_async(Some(systemd_path.as_path())).await {
            let _ = run_command_with_timeout(
                "systemctl",
                vec![
                    "--user".to_string(),
                    "disable".to_string(),
                    LINUX_SYSTEMD_SERVICE.to_string(),
                ],
                Duration::from_secs(5),
            )
            .await;
            let _ = tokio_fs::remove_file(&systemd_path).await;
            let _ = run_command_with_timeout(
                "systemctl",
                vec!["--user".to_string(), "daemon-reload".to_string()],
                Duration::from_secs(5),
            )
            .await;
        }
        let _ = tokio_fs::remove_file(linux_desktop_entry_path(&config_home)).await;
    }
}

pub(crate) async fn save_autostart_enabled(config: &Config, enabled: bool) -> Result<Value> {
    if !enabled {
        if cfg!(windows) {
            disable_windows_startup().await;
        } else if cfg!(target_os = "macos") {
            disable_macos_launch_agent().await;
        } else if cfg!(target_os = "linux") {
            disable_linux_autostart().await;
        }
        return get_autostart_state(config).await;
    }

    if cfg!(windows) {
        write_windows_startup_script(config).await?;
        return get_autostart_state(config).await;
    }

    if cfg!(target_os = "macos") {
        write_macos_launch_agent(config).await?;
        return get_autostart_state(config).await;
    }

    if cfg!(target_os = "linux") {
        if can_use_linux_systemd_user().await {
            match write_linux_systemd_user_service(config).await {
                Ok(()) => return get_autostart_state(config).await,
                Err(error) => {
                    warn!("failed to configure systemd user autostart: {error:#}");
                    if let Some(config_home) = config_home_path() {
                        let _ = tokio_fs::remove_file(linux_systemd_user_path(&config_home)).await;
                    }
                }
            }
        }

        write_linux_desktop_entry(config).await?;
        return get_autostart_state(config).await;
    }

    anyhow::bail!("Automatic startup is not supported on this operating system.");
}
