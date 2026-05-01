use super::*;

pub(crate) async fn normalize_git_repo_path(
    state: &AppState,
    git_repo_path: &Value,
) -> ApiResult<Value> {
    let Some(raw_path) = git_repo_path
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(Value::Null);
    };

    let candidate = resolve_input_path(&state.config.project_root, raw_path);
    let resolved = tokio_fs::canonicalize(&candidate).await.map_err(|_| {
        api_error(
            StatusCode::BAD_REQUEST,
            "The selected repository path does not exist.",
        )
    })?;
    let allowed_roots = resolved_allowed_roots(&state.config).await;
    if !allowed_roots
        .iter()
        .any(|root| path_is_within(root, &resolved))
    {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "The selected repository path is outside the allowed roots.",
        ));
    }

    Ok(Value::String(resolved.display().to_string()))
}

pub(crate) async fn resolve_git_repo_root(state: &AppState, repo_path: &str) -> ApiResult<String> {
    let normalized = normalize_git_repo_path(state, &Value::String(repo_path.to_string())).await?;
    let resolved_repo_path = normalized
        .as_str()
        .ok_or_else(|| {
            api_error(
                StatusCode::BAD_REQUEST,
                "The selected repository path is invalid.",
            )
        })?
        .to_string();

    let output = run_command_with_timeout(
        "git",
        vec![
            "-C".to_string(),
            resolved_repo_path.clone(),
            "rev-parse".to_string(),
            "--show-toplevel".to_string(),
        ],
        Duration::from_secs(10),
    )
    .await
    .map_err(|error| api_error(StatusCode::BAD_REQUEST, error.to_string()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            if stderr.is_empty() {
                "The selected path is not inside a Git repository.".to_string()
            } else {
                stderr
            },
        ));
    }

    let repo_root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if repo_root.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "The selected repository path is not inside a Git repository.",
        ));
    }
    let repo_root_path = tokio_fs::canonicalize(&repo_root).await.map_err(|_| {
        api_error(
            StatusCode::BAD_REQUEST,
            "The selected Git repository root does not exist.",
        )
    })?;
    let allowed_roots = resolved_allowed_roots(&state.config).await;
    if !allowed_roots
        .iter()
        .any(|root| path_is_within(root, &repo_root_path))
    {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "The selected Git repository root is outside the allowed roots.",
        ));
    }
    Ok(repo_root_path.display().to_string())
}

pub(crate) async fn run_git_text_payload(
    _state: &AppState,
    repo_path: &str,
    args: Vec<String>,
) -> ApiResult<String> {
    let output = run_git_output_payload(repo_path, args.clone(), Duration::from_secs(20)).await?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

async fn run_git_output_payload(
    repo_path: &str,
    args: Vec<String>,
    timeout: Duration,
) -> ApiResult<std::process::Output> {
    let mut command_args = vec!["-C".to_string(), repo_path.to_string()];
    command_args.extend(args.clone());
    let output = run_command_with_timeout("git", command_args, timeout)
        .await
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error.to_string()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            if stderr.is_empty() {
                format!(
                    "git {} failed.",
                    args.first().map(String::as_str).unwrap_or("command")
                )
            } else {
                stderr
            },
        ));
    }
    Ok(output)
}

fn git_skip_dir(name: &str) -> bool {
    matches!(
        name,
        ".git" | "node_modules" | ".svelte-kit" | "build" | "dist" | ".next" | "coverage"
    )
}

async fn has_git_marker(path: &Path) -> bool {
    tokio_fs::metadata(path.join(".git"))
        .await
        .map(|metadata| metadata.is_dir() || metadata.is_file())
        .unwrap_or(false)
}

async fn list_git_child_directories(path: &Path) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    let Ok(mut entries) = tokio_fs::read_dir(path).await else {
        return directories;
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if git_skip_dir(name) {
            continue;
        }
        if entry
            .file_type()
            .await
            .map(|file_type| file_type.is_dir())
            .unwrap_or(false)
        {
            directories.push(entry.path());
        }
    }

    directories
}

pub(crate) async fn build_git_repository_payload(
    _state: &AppState,
    repo_path: &Path,
    allowed_roots: &[PathBuf],
) -> ApiResult<Value> {
    let normalized_repo_path = real_path_safe(repo_path).await;
    let Some(root_path) = allowed_roots
        .iter()
        .find(|candidate| path_is_within(candidate, &normalized_repo_path))
    else {
        return Err(api_error(
            StatusCode::NOT_FOUND,
            "The selected repository was not found within allowed roots.",
        ));
    };

    let current_branch = run_command_with_timeout(
        "git",
        vec![
            "-C".to_string(),
            normalized_repo_path.display().to_string(),
            "branch".to_string(),
            "--show-current".to_string(),
        ],
        Duration::from_secs(5),
    )
    .await
    .ok()
    .filter(|output| output.status.success())
    .and_then(|output| {
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!branch.is_empty()).then_some(branch)
    });
    let relative_path = normalized_repo_path
        .strip_prefix(root_path)
        .ok()
        .and_then(|value| {
            let text = value.display().to_string();
            (!text.is_empty()).then_some(text)
        })
        .unwrap_or_else(|| ".".to_string());

    Ok(json!({
        "path": normalized_repo_path.display().to_string(),
        "name": normalized_repo_path
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| normalized_repo_path.as_os_str().to_str().unwrap_or(".")),
        "rootPath": root_path.display().to_string(),
        "relativePath": relative_path,
        "currentBranch": current_branch
    }))
}

pub(crate) async fn invalidate_git_repository_cache(state: &AppState) {
    *state.git_repository_cache.lock().await = None;
}

pub(crate) async fn list_git_repositories_payload(
    state: &AppState,
    force_refresh: bool,
) -> ApiResult<Value> {
    if !force_refresh {
        if let Some(cached) = state
            .git_repository_cache
            .lock()
            .await
            .clone()
            .filter(|cached| cached.created_at.elapsed() < GIT_REPOSITORY_CACHE_TTL)
        {
            let pinned = state
                .pinned_git_repositories
                .lock()
                .await
                .values()
                .cloned()
                .collect::<Vec<_>>();
            let mut repositories_by_path = HashMap::new();
            for repository in cached.repositories.into_iter().chain(pinned.into_iter()) {
                if let Some(path) = repository.get("path").and_then(Value::as_str) {
                    repositories_by_path.insert(path.to_string(), repository);
                }
            }
            let mut repositories = repositories_by_path.into_values().collect::<Vec<_>>();
            repositories.sort_by(|left, right| {
                left.get("relativePath")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .cmp(
                        right
                            .get("relativePath")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    )
                    .then_with(|| {
                        left.get("path")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .cmp(
                                right
                                    .get("path")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default(),
                            )
                    })
            });
            return Ok(json!({ "repositories": repositories }));
        }
    }

    let allowed_roots = resolved_allowed_roots(&state.config).await;
    let mut repositories = Vec::new();
    for root in &allowed_roots {
        let mut queue = VecDeque::from([(root.clone(), 0_u64)]);
        while let Some((current_path, depth)) = queue.pop_front() {
            if has_git_marker(&current_path).await {
                if let Ok(repository) =
                    build_git_repository_payload(state, &current_path, &allowed_roots).await
                {
                    repositories.push(repository);
                }
            }
            if depth >= state.config.git_discovery_depth {
                continue;
            }
            for child in list_git_child_directories(&current_path).await {
                queue.push_back((child, depth + 1));
            }
        }
    }

    let pinned = state
        .pinned_git_repositories
        .lock()
        .await
        .values()
        .cloned()
        .collect::<Vec<_>>();
    let mut repositories_by_path = HashMap::new();
    for repository in repositories.into_iter().chain(pinned.into_iter()) {
        if let Some(path) = repository.get("path").and_then(Value::as_str) {
            repositories_by_path.insert(path.to_string(), repository);
        }
    }
    let mut repositories = repositories_by_path.into_values().collect::<Vec<_>>();
    repositories.sort_by(|left, right| {
        left.get("relativePath")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(
                right
                    .get("relativePath")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
            .then_with(|| {
                left.get("path")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .cmp(
                        right
                            .get("path")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    )
            })
    });
    *state.git_repository_cache.lock().await = Some(CachedGitRepositories {
        created_at: Instant::now(),
        repositories: repositories.clone(),
    });
    Ok(json!({ "repositories": repositories }))
}

pub(crate) async fn resolve_git_file_from_absolute_path_payload(
    state: &AppState,
    file_path: &str,
) -> ApiResult<Value> {
    if file_path.trim().is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "filePath is required."));
    }

    let normalized = real_path_safe(Path::new(file_path)).await;
    let target_metadata = tokio_fs::metadata(&normalized).await.ok();
    let repositories = list_git_repositories_payload(state, false)
        .await?
        .get("repositories")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut repositories = repositories;
    repositories.sort_by(|left, right| {
        right
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .len()
            .cmp(
                &left
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .len(),
            )
    });
    let allowed_roots = resolved_allowed_roots(&state.config).await;

    let mut resolved_repository = repositories.into_iter().find(|repository| {
        repository
            .get("path")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .is_some_and(|candidate| path_is_within(&candidate, &normalized))
    });

    if resolved_repository.is_none() {
        let mut current_path = if target_metadata
            .as_ref()
            .is_some_and(|metadata| metadata.is_dir())
        {
            normalized.clone()
        } else {
            normalized
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| normalized.clone())
        };

        while allowed_roots
            .iter()
            .any(|root| path_is_within(root, &current_path))
        {
            if has_git_marker(&current_path).await {
                if let Ok(repository) =
                    build_git_repository_payload(state, &current_path, &allowed_roots).await
                {
                    resolved_repository = Some(repository);
                    break;
                }
            }
            let parent = current_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| current_path.clone());
            if parent == current_path {
                break;
            }
            current_path = parent;
        }
    }

    if resolved_repository.is_none() {
        let stats = tokio_fs::metadata(&normalized).await.ok();
        if stats.as_ref().is_some_and(|metadata| metadata.is_dir()) {
            let max_depth = state.config.git_discovery_depth.saturating_add(2).max(3);
            let mut queue = VecDeque::from([(normalized.clone(), 0_u64)]);
            while let Some((current_path, depth)) = queue.pop_front() {
                if depth > 0 && has_git_marker(&current_path).await {
                    if let Ok(repository) =
                        build_git_repository_payload(state, &current_path, &allowed_roots).await
                    {
                        resolved_repository = Some(repository);
                        break;
                    }
                }
                if depth >= max_depth {
                    continue;
                }
                for child in list_git_child_directories(&current_path).await {
                    queue.push_back((child, depth + 1));
                }
            }
        }
    }

    let Some(repository) = resolved_repository else {
        return Err(api_error(
            StatusCode::NOT_FOUND,
            "The selected path could not be mapped to a Git repository within allowed roots.",
        ));
    };

    let repo_path = repository
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            api_error(
                StatusCode::BAD_REQUEST,
                "The selected repository path is invalid.",
            )
        })?
        .to_string();
    let repo_root = PathBuf::from(&repo_path);
    let relative_path = if target_metadata
        .as_ref()
        .is_some_and(|metadata| !metadata.is_dir())
    {
        normalized
            .strip_prefix(&repo_root)
            .ok()
            .and_then(|relative| {
                let text = relative
                    .components()
                    .filter_map(|component| match component {
                        Component::Normal(value) => value.to_str().map(str::to_string),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("/");
                (!text.is_empty()).then_some(text)
            })
    } else {
        None
    };

    Ok(json!({
        "repoPath": repo_path,
        "filePath": relative_path
    }))
}
