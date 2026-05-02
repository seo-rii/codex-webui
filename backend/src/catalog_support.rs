use super::*;

fn parse_front_matter(raw: &str) -> (Option<String>, Option<String>) {
    let Some(stripped) = raw.strip_prefix("---\n") else {
        return (None, None);
    };
    let Some((front_matter, _)) = stripped.split_once("\n---") else {
        return (None, None);
    };

    let mut name = None;
    let mut description = None;
    for line in front_matter.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "name" if !value.is_empty() => name = Some(value.to_string()),
            "description" if !value.is_empty() => description = Some(value.to_string()),
            _ => {}
        }
    }
    (name, description)
}

fn walk_matching_files(root: &Path, matcher: &dyn Fn(&Path) -> bool, results: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_matching_files(&path, matcher, results);
        } else if path.is_file() && matcher(&path) {
            results.push(path);
        }
    }
}

pub(crate) fn build_catalog_payload_for_codex_home(codex_home: &Path) -> Value {
    let skills_root = codex_home.join("skills");
    let plugins_root = codex_home.join("plugins");

    let mut skill_files = Vec::new();
    let mut plugin_skill_files = Vec::new();
    walk_matching_files(
        &skills_root,
        &|path| path.file_name().and_then(|value| value.to_str()) == Some("SKILL.md"),
        &mut skill_files,
    );
    walk_matching_files(
        &plugins_root,
        &|path| path.file_name().and_then(|value| value.to_str()) == Some("SKILL.md"),
        &mut plugin_skill_files,
    );

    let mut skills = skill_files
        .into_iter()
        .chain(plugin_skill_files)
        .map(|path| {
            let raw = fs::read_to_string(&path).unwrap_or_default();
            let (name, description) = parse_front_matter(&raw);
            let (normalized_relative, source, plugin_name) =
                if let Ok(relative) = path.strip_prefix(&skills_root) {
                    let relative_string = relative.to_string_lossy().replace('\\', "/");
                    let source = if relative_string.starts_with(".system/") {
                        "system"
                    } else {
                        "local"
                    };
                    (relative_string, source, Value::Null)
                } else if let Ok(relative) = path.strip_prefix(&plugins_root) {
                    let relative_string = relative.to_string_lossy().replace('\\', "/");
                    let plugin_name = relative_string
                        .split('/')
                        .next()
                        .filter(|value| !value.is_empty())
                        .map(|value| Value::String(value.to_string()))
                        .unwrap_or(Value::Null);
                    let source = if relative_string.starts_with(".system/") {
                        "system"
                    } else {
                        "plugin"
                    };
                    (relative_string, source, plugin_name)
                } else {
                    (
                        path.file_name()
                            .and_then(|value| value.to_str())
                            .unwrap_or("SKILL.md")
                            .to_string(),
                        "local",
                        Value::Null,
                    )
                };

            let skill_name = name
                .or_else(|| {
                    path.parent()
                        .and_then(|parent| parent.file_name())
                        .and_then(|value| value.to_str())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| "Skill".to_string());

            json!({
                "id": normalized_relative.trim_end_matches("/SKILL.md"),
                "name": skill_name,
                "description": description.unwrap_or_default(),
                "path": path.display().to_string(),
                "source": source,
                "pluginName": plugin_name
            })
        })
        .collect::<Vec<_>>();

    skills.sort_by(|left, right| {
        left.get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(
                right
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
    });

    let mut plugin_files = Vec::new();
    walk_matching_files(
        &plugins_root,
        &|path| path.ends_with(Path::new(".codex-plugin").join("plugin.json")),
        &mut plugin_files,
    );

    let mut plugins = plugin_files
        .into_iter()
        .map(|path| {
            let raw = fs::read_to_string(&path).unwrap_or_else(|_| "{}".to_string());
            let parsed: Value = serde_json::from_str(&raw).unwrap_or_else(|_| json!({}));
            let plugin_base = path
                .parent()
                .and_then(Path::parent)
                .map(Path::to_path_buf)
                .unwrap_or_else(|| path.clone());
            let interface = parsed.get("interface").cloned().unwrap_or_else(|| json!({}));
            let skills_dir = parsed
                .get("skills")
                .and_then(Value::as_str)
                .map(|value| plugin_base.join(value));
            let mut plugin_skill_entries = Vec::new();
            if let Some(skills_dir) = skills_dir {
                walk_matching_files(
                    &skills_dir,
                    &|candidate| {
                        candidate.file_name().and_then(|value| value.to_str()) == Some("SKILL.md")
                    },
                    &mut plugin_skill_entries,
                );
            }
            plugin_skill_entries.sort();

            let name = parsed
                .get("name")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string)
                .or_else(|| {
                    plugin_base
                        .file_name()
                        .and_then(|value| value.to_str())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| "plugin".to_string());
            let display_name = interface
                .get("displayName")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| name.clone());

            json!({
                "name": name,
                "displayName": display_name,
                "description": parsed.get("description").and_then(Value::as_str).unwrap_or_default(),
                "version": parsed.get("version").cloned().unwrap_or(Value::Null),
                "developerName": interface.get("developerName").cloned().unwrap_or(Value::Null),
                "category": interface.get("category").cloned().unwrap_or(Value::Null),
                "path": plugin_base.display().to_string(),
                "skills": plugin_skill_entries
                    .iter()
                    .filter_map(|skill_path| {
                        skill_path
                            .parent()
                            .and_then(Path::file_name)
                            .and_then(|value| value.to_str())
                            .map(str::to_string)
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect::<Vec<_>>();

    plugins.sort_by(|left, right| {
        left.get("displayName")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(
                right
                    .get("displayName")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
    });

    json!({
        "plugins": plugins,
        "skills": skills
    })
}

pub(crate) async fn get_catalog_payload(state: &AppState, profile_id: &str) -> ApiResult<Value> {
    let codex_home = resolve_runtime_profile(&state.config, profile_id)
        .codex_home
        .display()
        .to_string();

    {
        let mut cache = state.catalog_cache.lock().await;
        cache.retain(|_, cached| cached.created_at.elapsed() < CATALOG_CACHE_TTL);
        if let Some(cached) = cache.get(&codex_home).cloned() {
            return Ok(cached.payload);
        }
    }

    let codex_home_path = resolve_runtime_profile(&state.config, profile_id)
        .codex_home
        .clone();
    let payload =
        tokio::task::spawn_blocking(move || build_catalog_payload_for_codex_home(&codex_home_path))
            .await
            .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;

    {
        let mut cache = state.catalog_cache.lock().await;
        cache.insert(
            codex_home,
            CachedCatalog {
                created_at: Instant::now(),
                payload: payload.clone(),
            },
        );
        if cache.len() > CATALOG_CACHE_MAX_ENTRIES {
            let mut entries = cache
                .iter()
                .map(|(key, cached)| (key.clone(), cached.created_at))
                .collect::<Vec<_>>();
            entries.sort_by_key(|(_, created_at)| *created_at);
            for (key, _) in entries {
                if cache.len() <= CATALOG_CACHE_MAX_ENTRIES {
                    break;
                }
                cache.remove(&key);
            }
        }
    }

    Ok(payload)
}
