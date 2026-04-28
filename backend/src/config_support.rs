use super::*;

pub(crate) fn runtime_logs_dir(config: &Config) -> PathBuf {
    config.data_dir.join("logs")
}

pub(crate) fn runtime_error_log_path(config: &Config) -> PathBuf {
    runtime_logs_dir(config).join(RUNTIME_ERROR_LOG_NAME)
}

pub(crate) fn append_runtime_error_log(
    config: &Config,
    source: &str,
    message: &str,
    details: Value,
) {
    let path = runtime_error_log_path(config);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let entry = json!({
        "atMs": now_millis(),
        "pid": std::process::id(),
        "source": source,
        "message": message,
        "details": details
    });

    if let Ok(line) = serde_json::to_string(&entry) {
        if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = std::io::Write::write_all(&mut file, line.as_bytes());
            let _ = std::io::Write::write_all(&mut file, b"\n");
        }
    }
}

pub(crate) fn install_panic_logger(config: Arc<Config>) {
    std::panic::set_hook(Box::new(move |panic_info| {
        let location = panic_info
            .location()
            .map(|location| format!("{}:{}", location.file(), location.line()));
        let payload = panic_info
            .payload()
            .downcast_ref::<&str>()
            .map(|value| (*value).to_string())
            .or_else(|| panic_info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "panic without string payload".to_string());
        append_runtime_error_log(
            &config,
            "rust-gateway",
            "panic",
            json!({
                "payload": payload,
                "location": location,
                "backtrace": std::backtrace::Backtrace::force_capture().to_string()
            }),
        );
    }));
}

#[derive(Clone, Debug)]
pub(crate) struct Config {
    pub(crate) project_root: PathBuf,
    pub(crate) allowed_roots: Vec<PathBuf>,
    pub(crate) default_profile_id: String,
    pub(crate) profiles: HashMap<String, RuntimeProfile>,
    pub(crate) data_dir: PathBuf,
    pub(crate) base_path: String,
    pub(crate) static_dir: PathBuf,
    pub(crate) public_host: String,
    pub(crate) public_port: u16,
    pub(crate) codex_bin: String,
    pub(crate) max_upload_bytes: u64,
    pub(crate) git_discovery_depth: u64,
    pub(crate) system_shutdown_enabled: bool,
    pub(crate) system_shutdown_delay_seconds: u64,
    pub(crate) system_shutdown_command_override: Option<String>,
    pub(crate) password: Option<String>,
    pub(crate) password_hash: Option<String>,
    pub(crate) viewer_password: Option<String>,
    pub(crate) viewer_password_hash: Option<String>,
    pub(crate) hcaptcha_site_key: Option<String>,
    pub(crate) hcaptcha_secret_key: Option<String>,
    pub(crate) session_secret: Option<String>,
    pub(crate) cookie_same_site: SameSiteMode,
    pub(crate) cookie_secure_mode: CookieSecureMode,
    pub(crate) cors_allowed_origins: Vec<String>,
    pub(crate) trust_proxy_headers: bool,
}

impl Config {
    pub(crate) fn hcaptcha_site_key(&self) -> Option<&str> {
        self.hcaptcha_site_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    pub(crate) fn hcaptcha_secret_key(&self) -> Option<&str> {
        self.hcaptcha_secret_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    pub(crate) fn hcaptcha_enabled(&self) -> bool {
        self.hcaptcha_site_key().is_some() && self.hcaptcha_secret_key().is_some()
    }

    pub(crate) fn from_env() -> Result<Self> {
        let cwd = env::current_dir().context("failed to read current directory")?;
        load_dotenv(&cwd);
        let project_root = resolve_project_root(&cwd);
        let allowed_roots = parse_allowed_roots(&project_root);
        let base_path = normalize_base_path(env::var("CODEX_WEBUI_BASE_PATH").ok());
        let static_dir = project_root.join("build/static");
        let data_dir = env::var("CODEX_WEBUI_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| cwd.join(".data"));
        let public_host = env::var("HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
        let public_port = parse_port(env::var("PORT").ok(), 4173)?;
        let max_upload_bytes = env::var("CODEX_WEBUI_MAX_UPLOAD_MB")
            .ok()
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| *value > 0.0)
            .map(|value| (value * 1024.0 * 1024.0).round() as u64)
            .unwrap_or(20 * 1024 * 1024);
        let git_discovery_depth = env::var("CODEX_WEBUI_GIT_DISCOVERY_DEPTH")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(1);
        let system_shutdown_delay_seconds = env::var("CODEX_WEBUI_SHUTDOWN_DELAY_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(30);
        if !static_dir.exists() {
            return Err(anyhow!(
                "missing static frontend build at {}. Run `pnpm build` in codex-webui first.",
                static_dir.display()
            ));
        }

        let codex_home = resolve_codex_home()?;
        let (default_profile_id, profiles) = parse_runtime_profiles(&codex_home, &data_dir)?;

        Ok(Self {
            project_root,
            allowed_roots,
            default_profile_id,
            profiles,
            data_dir,
            base_path,
            static_dir,
            public_host,
            public_port,
            codex_bin: env::var("CODEX_WEBUI_CODEX_BIN").unwrap_or_else(|_| "codex".to_string()),
            max_upload_bytes,
            git_discovery_depth,
            system_shutdown_enabled: env::var("CODEX_WEBUI_ENABLE_SYSTEM_SHUTDOWN")
                .is_ok_and(|value| value == "true"),
            system_shutdown_delay_seconds,
            system_shutdown_command_override: env::var("CODEX_WEBUI_SHUTDOWN_COMMAND")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            password: env::var("CODEX_WEBUI_PASSWORD").ok(),
            password_hash: env::var("CODEX_WEBUI_PASSWORD_HASH").ok(),
            viewer_password: env::var("CODEX_WEBUI_VIEWER_PASSWORD").ok(),
            viewer_password_hash: env::var("CODEX_WEBUI_VIEWER_PASSWORD_HASH").ok(),
            hcaptcha_site_key: env::var("CODEX_WEBUI_HCAPTCHA_SITE_KEY").ok(),
            hcaptcha_secret_key: env::var("CODEX_WEBUI_HCAPTCHA_SECRET_KEY").ok(),
            session_secret: env::var("CODEX_WEBUI_SESSION_SECRET").ok(),
            cookie_same_site: parse_same_site(
                env::var("CODEX_WEBUI_COOKIE_SAMESITE").ok().as_deref(),
            ),
            cookie_secure_mode: parse_secure_mode(
                env::var("CODEX_WEBUI_COOKIE_SECURE").ok().as_deref(),
            ),
            cors_allowed_origins: parse_cors_origins(
                env::var("CODEX_WEBUI_CORS_ALLOWED_ORIGINS").ok(),
            )?,
            trust_proxy_headers: env::var("CODEX_WEBUI_TRUST_PROXY_HEADERS").is_ok_and(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            }),
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct RuntimeProfile {
    pub(crate) label: String,
    pub(crate) codex_home: PathBuf,
    pub(crate) data_dir: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SameSiteMode {
    Strict,
    Lax,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CookieSecureMode {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Deserialize)]
struct RuntimeProfileShape {
    id: Option<String>,
    label: Option<String>,
    #[serde(alias = "codex_home", alias = "codexHome")]
    codex_home: Option<String>,
    #[serde(alias = "data_dir", alias = "dataDir")]
    data_dir: Option<String>,
}

pub(crate) fn sanitize_profile_id(input: &str) -> String {
    let sanitized = input
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|character| match character {
            'a'..='z' | '0'..='9' | '.' | '_' | '-' => character,
            _ => '-',
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if sanitized.is_empty() {
        "default".to_string()
    } else {
        sanitized
    }
}

pub(crate) fn parse_runtime_profiles(
    default_codex_home: &PathBuf,
    root_data_dir: &PathBuf,
) -> Result<(String, HashMap<String, RuntimeProfile>)> {
    let default_profile_id = sanitize_profile_id(
        &env::var("CODEX_WEBUI_DEFAULT_PROFILE_ID").unwrap_or_else(|_| "default".to_string()),
    );
    let raw_profiles = env::var("CODEX_WEBUI_PROFILES_JSON").ok();

    let Some(raw_profiles) = raw_profiles.filter(|value| !value.trim().is_empty()) else {
        let mut profiles = HashMap::new();
        profiles.insert(
            default_profile_id.clone(),
            RuntimeProfile {
                label: "Default".to_string(),
                codex_home: default_codex_home.clone(),
                data_dir: root_data_dir.join("profiles").join(&default_profile_id),
            },
        );
        return Ok((default_profile_id, profiles));
    };

    let parsed: Vec<RuntimeProfileShape> =
        serde_json::from_str(&raw_profiles).context("invalid CODEX_WEBUI_PROFILES_JSON")?;
    let mut profiles = HashMap::new();

    for entry in parsed {
        let id = sanitize_profile_id(entry.id.as_deref().unwrap_or("default"));
        let label = entry
            .label
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                if id == default_profile_id {
                    "Default".to_string()
                } else {
                    id.clone()
                }
            });
        profiles
            .entry(id.clone())
            .or_insert_with(|| RuntimeProfile {
                label,
                codex_home: entry
                    .codex_home
                    .as_deref()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| default_codex_home.clone()),
                data_dir: entry
                    .data_dir
                    .as_deref()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| root_data_dir.join("profiles").join(&id)),
            });
    }

    if profiles.is_empty() {
        profiles.insert(
            default_profile_id.clone(),
            RuntimeProfile {
                label: "Default".to_string(),
                codex_home: default_codex_home.clone(),
                data_dir: root_data_dir.join("profiles").join(&default_profile_id),
            },
        );
    }

    let resolved_default_profile_id = if profiles.contains_key(&default_profile_id) {
        default_profile_id
    } else {
        profiles
            .keys()
            .next()
            .cloned()
            .unwrap_or_else(|| "default".to_string())
    };

    Ok((resolved_default_profile_id, profiles))
}

pub(crate) fn parse_allowed_roots(project_root: &Path) -> Vec<PathBuf> {
    let mut roots = env::var_os("CODEX_WEBUI_ALLOWED_ROOTS")
        .map(|value| {
            env::split_paths(&value)
                .map(|entry| {
                    normalize_path(if entry.is_absolute() {
                        entry
                    } else {
                        project_root.join(entry)
                    })
                })
                .filter(|entry| !entry.as_os_str().is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| {
            let fallback = project_root
                .parent()
                .filter(|parent| *parent != project_root)
                .unwrap_or(project_root);
            vec![fallback.to_path_buf()]
        });

    roots.dedup();
    roots
}

pub(crate) fn normalize_path(path: PathBuf) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(value) => normalized.push(value.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(value) => normalized.push(value),
        }
    }
    normalized
}

pub(crate) fn resolve_input_path(project_root: &Path, input: &str) -> PathBuf {
    let candidate = PathBuf::from(input);
    normalize_path(if candidate.is_absolute() {
        candidate
    } else {
        project_root.join(candidate)
    })
}

pub(crate) async fn real_path_safe(target: &Path) -> PathBuf {
    tokio_fs::canonicalize(target)
        .await
        .unwrap_or_else(|_| target.to_path_buf())
}

pub(crate) async fn resolved_allowed_roots(config: &Config) -> Vec<PathBuf> {
    let mut roots = Vec::with_capacity(config.allowed_roots.len());
    for root in &config.allowed_roots {
        roots.push(real_path_safe(root).await);
    }
    roots
}

pub(crate) fn path_is_within(root: &Path, candidate: &Path) -> bool {
    candidate == root || candidate.starts_with(root)
}

pub(crate) const CONFIG_SCHEMA_HEADER: &str =
    "#:schema https://developers.openai.com/codex/config-schema.json";

#[derive(Default)]
pub(crate) struct CodexTomlDefaults {
    pub(crate) model: Option<String>,
    pub(crate) model_context_window: Option<i64>,
    pub(crate) model_reasoning_effort: Option<String>,
    pub(crate) plan_mode_reasoning_effort: Option<String>,
    pub(crate) personality: Option<String>,
    pub(crate) approval_policy: Option<String>,
    pub(crate) sandbox_mode: Option<String>,
    pub(crate) service_tier: String,
    pub(crate) network_access: Option<bool>,
}

pub(crate) fn config_toml_path(codex_home: &Path) -> PathBuf {
    codex_home.join("config.toml")
}

fn parse_toml_section_name(line: &str) -> Option<String> {
    let trimmed = line.trim();
    trimmed
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn trim_toml_value(value: &str) -> String {
    let mut trimmed = String::new();
    let mut escaped = false;
    let mut quote = None;
    for character in value.chars() {
        if escaped {
            trimmed.push(character);
            escaped = false;
            continue;
        }
        if character == '\\' {
            trimmed.push(character);
            escaped = true;
            continue;
        }
        if matches!(character, '"' | '\'') && (quote.is_none() || quote == Some(character)) {
            quote = if quote.is_some() {
                None
            } else {
                Some(character)
            };
            trimmed.push(character);
            continue;
        }
        if character == '#' && quote.is_none() {
            break;
        }
        trimmed.push(character);
    }
    trimmed.trim().to_string()
}

fn get_toml_value(raw: &str, section: Option<&str>, key: &str) -> Option<String> {
    let mut current_section: Option<String> = None;
    for line in raw.lines() {
        if let Some(next_section) = parse_toml_section_name(line) {
            current_section = Some(next_section);
            continue;
        }
        if current_section.as_deref() != section || !matches_toml_key(line, key) {
            continue;
        }
        let (_, value) = line.split_once('=')?;
        return Some(trim_toml_value(value));
    }
    None
}

fn parse_toml_string_value(value: Option<String>) -> Option<String> {
    let value = value?;
    if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
        return serde_json::from_str::<String>(&value).ok().or_else(|| {
            Some(
                value[1..value.len().saturating_sub(1)]
                    .replace("\\\"", "\"")
                    .replace("\\\\", "\\"),
            )
        });
    }
    if value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2 {
        return Some(value[1..value.len().saturating_sub(1)].to_string());
    }
    None
}

fn parse_toml_bool_value(value: Option<String>) -> Option<bool> {
    match value.as_deref() {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    }
}

fn parse_toml_integer_value(value: Option<String>) -> Option<i64> {
    value?.replace('_', "").trim().parse::<i64>().ok()
}

pub(crate) fn read_codex_toml_defaults(codex_home: &Path) -> CodexTomlDefaults {
    let file_path = config_toml_path(codex_home);
    let Ok(raw) = fs::read_to_string(file_path) else {
        return CodexTomlDefaults {
            service_tier: "auto".to_string(),
            ..CodexTomlDefaults::default()
        };
    };

    let service_tier = parse_toml_string_value(get_toml_value(&raw, None, "service_tier"))
        .filter(|value| value == "fast" || value == "flex")
        .unwrap_or_else(|| "auto".to_string());

    CodexTomlDefaults {
        model: parse_toml_string_value(get_toml_value(&raw, None, "model")),
        model_context_window: parse_toml_integer_value(get_toml_value(
            &raw,
            None,
            "model_context_window",
        ))
        .filter(|value| *value > 0),
        model_reasoning_effort: parse_toml_string_value(get_toml_value(
            &raw,
            None,
            "model_reasoning_effort",
        )),
        plan_mode_reasoning_effort: parse_toml_string_value(get_toml_value(
            &raw,
            None,
            "plan_mode_reasoning_effort",
        )),
        personality: parse_toml_string_value(get_toml_value(&raw, None, "personality"))
            .filter(|value| matches!(value.as_str(), "none" | "friendly" | "pragmatic")),
        approval_policy: parse_toml_string_value(get_toml_value(&raw, None, "approval_policy")),
        sandbox_mode: parse_toml_string_value(get_toml_value(&raw, None, "sandbox_mode")),
        service_tier,
        network_access: parse_toml_bool_value(get_toml_value(
            &raw,
            Some("sandbox_workspace_write"),
            "network_access",
        )),
    }
}

fn matches_toml_key(line: &str, key: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed
        .strip_prefix(key)
        .is_some_and(|rest| rest.trim_start().starts_with('='))
}

fn normalize_toml_lines(raw: &str) -> Vec<String> {
    let mut lines = raw
        .split('\n')
        .map(|line| line.trim_end_matches('\r').to_string())
        .collect::<Vec<_>>();
    while lines.len() > 1 && lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines
}

fn stringify_toml_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

pub(crate) fn preferences_model_context_window(preferences: &Value) -> Option<i64> {
    preferences
        .get("modelContextWindow")
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
}

pub(crate) fn preferences_model_context_config(preferences: &Value) -> Value {
    preferences_model_context_window(preferences)
        .map(|value| json!({ "model_context_window": value }))
        .unwrap_or(Value::Null)
}

pub(crate) fn upsert_toml_value(
    raw: &str,
    section: Option<&str>,
    key: &str,
    value: Option<String>,
) -> String {
    let mut lines = normalize_toml_lines(raw);
    let mut current_section: Option<String> = None;
    let mut section_start = if section.is_none() {
        Some(0usize)
    } else {
        None
    };
    let mut section_end = lines.len();
    let mut replaced = false;

    for index in 0..lines.len() {
        if let Some(next_section) = parse_toml_section_name(&lines[index]) {
            if current_section.as_deref() == section && section_end == lines.len() {
                section_end = index;
            }
            current_section = Some(next_section.clone());
            if section.is_some() && section_start.is_none() && current_section.as_deref() == section
            {
                section_start = Some(index);
            }
            continue;
        }

        if current_section.as_deref() != section || !matches_toml_key(&lines[index], key) {
            continue;
        }

        replaced = true;
        if let Some(value) = &value {
            lines[index] = format!("{key} = {value}");
        } else {
            lines.remove(index);
            return upsert_toml_value(&lines.join("\n"), section, key, None);
        }
    }

    if !replaced {
        if let Some(value) = value {
            if section.is_none() {
                let insert_index = lines
                    .iter()
                    .position(|line| parse_toml_section_name(line).is_some())
                    .unwrap_or(lines.len());
                lines.insert(insert_index, format!("{key} = {value}"));
            } else if let Some(section_start) = section_start {
                lines.insert(
                    section_end.max(section_start + 1),
                    format!("{key} = {value}"),
                );
            } else {
                if !lines.is_empty() && lines.last().is_some_and(|line| !line.is_empty()) {
                    lines.push(String::new());
                }
                lines.push(format!("[{}]", section.unwrap_or_default()));
                lines.push(format!("{key} = {value}"));
            }
        }
    }

    while lines.len() > 1 && lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }

    format!("{}\n", lines.join("\n"))
}

pub(crate) async fn sync_codex_toml_with_preferences(
    codex_home: &Path,
    preferences: &Value,
) -> Result<()> {
    let file_path = config_toml_path(codex_home);
    if let Some(parent) = file_path.parent() {
        tokio_fs::create_dir_all(parent)
            .await
            .context("failed to create the Codex config directory")?;
    }

    let mut raw = match tokio_fs::read_to_string(&file_path).await {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error).context("failed to read config.toml"),
    };
    if raw.trim().is_empty() {
        raw = format!("{CONFIG_SCHEMA_HEADER}\n");
    }

    raw = upsert_toml_value(
        &raw,
        None,
        "model",
        preferences
            .get("model")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(stringify_toml_string),
    );
    raw = upsert_toml_value(
        &raw,
        None,
        "model_context_window",
        preferences_model_context_window(preferences).map(|value| value.to_string()),
    );
    raw = upsert_toml_value(
        &raw,
        None,
        "personality",
        preferences
            .get("personality")
            .and_then(Value::as_str)
            .filter(|value| matches!(*value, "none" | "friendly" | "pragmatic"))
            .map(stringify_toml_string),
    );
    raw = upsert_toml_value(
        &raw,
        None,
        "approval_policy",
        preferences
            .get("approvalPolicy")
            .and_then(Value::as_str)
            .map(stringify_toml_string),
    );
    raw = upsert_toml_value(
        &raw,
        None,
        "sandbox_mode",
        preferences
            .get("sandboxMode")
            .and_then(Value::as_str)
            .map(stringify_toml_string),
    );
    raw = upsert_toml_value(
        &raw,
        None,
        "service_tier",
        preferences
            .get("speed")
            .and_then(Value::as_str)
            .filter(|value| *value == "fast" || *value == "flex")
            .map(stringify_toml_string),
    );

    let effort_key = if preferences.get("mode").and_then(Value::as_str) == Some("plan") {
        "plan_mode_reasoning_effort"
    } else {
        "model_reasoning_effort"
    };
    raw = upsert_toml_value(
        &raw,
        None,
        effort_key,
        preferences
            .get("effort")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(stringify_toml_string),
    );
    raw = upsert_toml_value(
        &raw,
        Some("sandbox_workspace_write"),
        "network_access",
        Some(
            if preferences
                .get("networkAccess")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                "true"
            } else {
                "false"
            }
            .to_string(),
        ),
    );

    tokio_fs::write(&file_path, raw)
        .await
        .context("failed to write config.toml")
}

pub(crate) fn normalize_origin(value: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(value).ok()?;
    let host = parsed.host_str()?;
    let mut origin = format!("{}://{}", parsed.scheme(), host);
    if let Some(port) = parsed.port() {
        origin.push(':');
        origin.push_str(&port.to_string());
    }
    Some(origin)
}

pub(crate) fn normalize_base_path(value: Option<String>) -> String {
    let Some(value) = value.map(|value| value.trim().to_string()) else {
        return String::new();
    };
    if value.is_empty() || value == "/" {
        return String::new();
    }
    let trimmed = value.trim_end_matches('/');
    if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    }
}

fn parse_port(value: Option<String>, fallback: u16) -> Result<u16> {
    match value {
        Some(value) => value
            .parse::<u16>()
            .with_context(|| format!("invalid port: {value}")),
        None => Ok(fallback),
    }
}

fn parse_same_site(value: Option<&str>) -> SameSiteMode {
    match value.unwrap_or("strict").to_ascii_lowercase().as_str() {
        "lax" => SameSiteMode::Lax,
        "none" => SameSiteMode::None,
        _ => SameSiteMode::Strict,
    }
}

fn parse_secure_mode(value: Option<&str>) -> CookieSecureMode {
    match value.unwrap_or("auto").to_ascii_lowercase().as_str() {
        "always" => CookieSecureMode::Always,
        "never" => CookieSecureMode::Never,
        _ => CookieSecureMode::Auto,
    }
}

fn parse_cors_origins(value: Option<String>) -> Result<Vec<String>> {
    let Some(raw) = value else {
        return Ok(Vec::new());
    };
    raw.split([',', '\n'])
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| normalize_origin(entry).ok_or_else(|| anyhow!("Invalid CORS origin: {entry}")))
        .collect()
}

fn resolve_codex_home() -> Result<PathBuf> {
    if let Some(value) = env::var_os("CODEX_HOME") {
        return Ok(PathBuf::from(value));
    }

    if let Some(home) = env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")) {
        return Ok(PathBuf::from(home).join(".codex"));
    }

    Err(anyhow!("Could not determine CODEX_HOME. Set CODEX_HOME."))
}

fn load_dotenv(cwd: &PathBuf) {
    let project_root = resolve_project_root(cwd);
    let path = project_root.join(".env");
    if path.exists() {
        let _ = dotenvy::from_path(path);
    }
}

fn resolve_project_root(cwd: &PathBuf) -> PathBuf {
    if cwd.join("build/static").exists() || cwd.join("svelte.config.js").exists() {
        return cwd.clone();
    }

    if cwd
        .file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value == "backend")
    {
        if let Some(parent) = cwd.parent() {
            let parent = parent.to_path_buf();
            if parent.join("build/static").exists() || parent.join("svelte.config.js").exists() {
                return parent;
            }
        }
    }

    cwd.clone()
}
