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
    Ok(repo_root)
}

async fn resolve_git_worktree_path(state: &AppState, worktree_path: &str) -> ApiResult<String> {
    if worktree_path.trim().is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "worktreePath is required.",
        ));
    }

    let candidate = resolve_input_path(&state.config.project_root, worktree_path);
    let existing = tokio_fs::canonicalize(&candidate).await.ok();
    let path_to_check = existing.unwrap_or_else(|| candidate.clone());
    let allowed_roots = resolved_allowed_roots(&state.config).await;
    if !allowed_roots
        .iter()
        .any(|root| path_is_within(root, &path_to_check))
    {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "The selected worktree path is outside the allowed roots.",
        ));
    }

    Ok(candidate.display().to_string())
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

async fn build_git_repository_payload(
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

fn parse_git_worktrees_payload(repo_path: &str, output: &str) -> Vec<Value> {
    let mut worktrees = Vec::new();
    let mut current: Option<serde_json::Map<String, Value>> = None;

    for raw_line in output.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.is_empty() {
            if let Some(entry) = current.take() {
                worktrees.push(Value::Object(entry));
            }
            continue;
        }

        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(entry) = current.take() {
                worktrees.push(Value::Object(entry));
            }
            let mut entry = serde_json::Map::new();
            entry.insert("path".to_string(), Value::String(path.to_string()));
            entry.insert("branch".to_string(), Value::Null);
            entry.insert("head".to_string(), Value::Null);
            entry.insert("bare".to_string(), Value::Bool(false));
            entry.insert("detached".to_string(), Value::Bool(false));
            entry.insert("locked".to_string(), Value::Bool(false));
            entry.insert("prunable".to_string(), Value::Bool(false));
            entry.insert("current".to_string(), Value::Bool(path == repo_path));
            current = Some(entry);
            continue;
        }

        let Some(entry) = current.as_mut() else {
            continue;
        };
        if let Some(head) = line.strip_prefix("HEAD ") {
            entry.insert("head".to_string(), Value::String(head.to_string()));
        } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
            entry.insert("branch".to_string(), Value::String(branch.to_string()));
        } else if line == "bare" {
            entry.insert("bare".to_string(), Value::Bool(true));
        } else if line == "detached" {
            entry.insert("detached".to_string(), Value::Bool(true));
        } else if line.starts_with("locked") {
            entry.insert("locked".to_string(), Value::Bool(true));
        } else if line.starts_with("prunable") {
            entry.insert("prunable".to_string(), Value::Bool(true));
        }
    }

    if let Some(entry) = current.take() {
        worktrees.push(Value::Object(entry));
    }

    worktrees
}

pub(crate) async fn list_git_worktrees_payload(
    state: &AppState,
    repo_path: &str,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let output = run_git_text_payload(
        state,
        &repo_root,
        vec![
            "worktree".to_string(),
            "list".to_string(),
            "--porcelain".to_string(),
        ],
    )
    .await?;
    Ok(json!({
        "repoPath": repo_root,
        "worktrees": parse_git_worktrees_payload(&repo_root, &output)
    }))
}

pub(crate) async fn create_git_worktree_payload(
    state: &AppState,
    repo_path: &str,
    worktree_path: &str,
    branch_name: Option<&str>,
    create_branch: bool,
    detach: bool,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let resolved_worktree_path = resolve_git_worktree_path(state, worktree_path).await?;
    let trimmed_branch_name = branch_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if !detach && trimmed_branch_name.is_none() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Provide a branch name or create a detached worktree.",
        ));
    }

    let mut args = vec!["worktree".to_string(), "add".to_string()];
    if detach {
        args.push("--detach".to_string());
    } else if create_branch {
        let Some(branch_name) = trimmed_branch_name.clone() else {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "Provide a branch name or create a detached worktree.",
            ));
        };
        args.push("-b".to_string());
        args.push(branch_name);
    }
    args.push(resolved_worktree_path.clone());
    if !detach && !create_branch {
        if let Some(branch_name) = trimmed_branch_name {
            args.push(branch_name);
        }
    }

    run_git_text_payload(state, &repo_root, args).await?;
    invalidate_git_repository_cache(state).await;
    let allowed_roots = resolved_allowed_roots(&state.config).await;
    if let Ok(repository) =
        build_git_repository_payload(state, Path::new(&resolved_worktree_path), &allowed_roots)
            .await
    {
        state
            .pinned_git_repositories
            .lock()
            .await
            .insert(resolved_worktree_path.clone(), repository);
    }
    list_git_worktrees_payload(state, &repo_root).await
}

pub(crate) async fn remove_git_worktree_payload(
    state: &AppState,
    repo_path: &str,
    worktree_path: &str,
    force: bool,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let resolved_worktree_path = resolve_git_worktree_path(state, worktree_path).await?;
    let mut args = vec!["worktree".to_string(), "remove".to_string()];
    if force {
        args.push("--force".to_string());
    }
    args.push(resolved_worktree_path.clone());
    run_git_text_payload(state, &repo_root, args).await?;
    invalidate_git_repository_cache(state).await;
    state
        .pinned_git_repositories
        .lock()
        .await
        .remove(&resolved_worktree_path);
    list_git_worktrees_payload(state, &repo_root).await
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

pub(crate) async fn get_git_status_payload(state: &AppState, repo_path: &str) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let allowed_roots = resolved_allowed_roots(&state.config).await;
    let repository =
        build_git_repository_payload(state, Path::new(&repo_root), &allowed_roots).await?;
    let output = run_git_text_payload(
        state,
        &repo_root,
        vec![
            "status".to_string(),
            "--porcelain=v1".to_string(),
            "--branch".to_string(),
        ],
    )
    .await?;
    let lines = output
        .lines()
        .map(|line| line.trim_end_matches('\r').to_string())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let header = lines
        .iter()
        .find(|line| line.starts_with("## "))
        .cloned()
        .unwrap_or_else(|| "## HEAD".to_string());
    let summary = header.trim_start_matches("## ").to_string();
    let (branch_part, tracking_part) = summary
        .split_once("...")
        .map(|(left, right)| (left.trim().to_string(), right.to_string()))
        .unwrap_or_else(|| (summary.trim().to_string(), String::new()));
    let branch = if branch_part == "HEAD (no branch)" {
        None
    } else {
        let trimmed = branch_part.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    };
    let extract_count = |needle: &str| -> u64 {
        tracking_part
            .split(needle)
            .nth(1)
            .map(str::trim_start)
            .and_then(|value| {
                value
                    .chars()
                    .take_while(|ch| ch.is_ascii_digit())
                    .collect::<String>()
                    .parse::<u64>()
                    .ok()
            })
            .unwrap_or(0)
    };
    let ahead = extract_count("ahead ");
    let behind = extract_count("behind ");

    let files = lines
        .iter()
        .filter(|line| !line.starts_with("## "))
        .map(|line| {
            let staged_code = line.chars().next().unwrap_or(' ');
            let unstaged_code = line.chars().nth(1).unwrap_or(' ');
            let raw_path = line.get(3..).unwrap_or_default();
            let (original_path, file_path) = raw_path
                .split_once(" -> ")
                .map(|(left, right)| (Some(left.to_string()), right.to_string()))
                .unwrap_or_else(|| (None, raw_path.to_string()));
            let map_code = |code: char| match code {
                'M' => "modified",
                'A' => "added",
                'D' => "deleted",
                'R' => "renamed",
                'C' => "copied",
                'U' => "unmerged",
                '?' => "untracked",
                '!' => "ignored",
                _ => "clean",
            };
            json!({
                "path": file_path,
                "originalPath": original_path,
                "stagedCode": staged_code.to_string(),
                "unstagedCode": unstaged_code.to_string(),
                "stagedLabel": map_code(staged_code),
                "unstagedLabel": map_code(unstaged_code),
                "hasStagedChanges": staged_code != ' ' && staged_code != '?',
                "hasUnstagedChanges": unstaged_code != ' ' && unstaged_code != '?',
                "isUntracked": staged_code == '?' && unstaged_code == '?'
            })
        })
        .collect::<Vec<_>>();

    let branches_output = run_git_text_payload(
        state,
        &repo_root,
        vec![
            "for-each-ref".to_string(),
            "refs/heads".to_string(),
            "--format=%(refname:short)\t%(HEAD)\t%(upstream:short)".to_string(),
        ],
    )
    .await?;
    let branches = branches_output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut parts = line.split('\t');
            let name = parts.next().unwrap_or_default();
            let current = parts.next().unwrap_or_default();
            let upstream = parts.next().unwrap_or_default();
            json!({
                "name": name,
                "current": current == "*",
                "upstream": if upstream.trim().is_empty() { Value::Null } else { Value::String(upstream.to_string()) }
            })
        })
        .collect::<Vec<_>>();

    let commits_output = run_git_text_payload(
        state,
        &repo_root,
        vec![
            "log".to_string(),
            "--max-count=12".to_string(),
            "--pretty=format:%H%x09%h%x09%an%x09%aI%x09%s".to_string(),
        ],
    )
    .await?;
    let commits = commits_output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut parts = line.split('\t');
            json!({
                "hash": parts.next().unwrap_or_default(),
                "shortHash": parts.next().unwrap_or_default(),
                "author": parts.next().unwrap_or_default(),
                "authoredAt": parts.next().unwrap_or_default(),
                "subject": parts.next().unwrap_or_default()
            })
        })
        .collect::<Vec<_>>();

    Ok(json!({
        "repo": {
            "path": repository.get("path").cloned().unwrap_or(Value::String(repo_root.clone())),
            "name": repository.get("name").cloned().unwrap_or(Value::Null),
            "rootPath": repository.get("rootPath").cloned().unwrap_or(Value::Null),
            "relativePath": repository.get("relativePath").cloned().unwrap_or(Value::Null),
            "currentBranch": branch.clone()
        },
        "branch": branch,
        "ahead": ahead,
        "behind": behind,
        "clean": files.is_empty(),
        "files": files,
        "branches": branches,
        "commits": commits
    }))
}

pub(crate) async fn get_git_file_payload(
    state: &AppState,
    repo_path: &str,
    file_path: &str,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let status_payload = get_git_status_payload(state, &repo_root).await?;
    let status = status_payload
        .get("files")
        .and_then(Value::as_array)
        .and_then(|files| {
            files
                .iter()
                .find(|entry| entry.get("path").and_then(Value::as_str) == Some(file_path))
        })
        .cloned()
        .unwrap_or(Value::Null);

    let candidate_path = resolve_git_repository_file_path(&repo_root, file_path).await?;
    let modified_bytes = tokio_fs::read(&candidate_path).await.unwrap_or_default();
    let modified_is_binary = modified_bytes.contains(&0);
    let modified_content = if modified_is_binary {
        String::new()
    } else {
        String::from_utf8_lossy(&modified_bytes).to_string()
    };

    let head_path = status
        .get("originalPath")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(file_path);
    let head_output = run_command_with_timeout(
        "git",
        vec![
            "-C".to_string(),
            repo_root.clone(),
            "show".to_string(),
            format!("HEAD:{}", head_path.replace('\\', "/")),
        ],
        Duration::from_secs(20),
    )
    .await
    .ok();
    let (original_content, original_is_binary) = if let Some(output) = head_output {
        if output.status.success() {
            let is_binary = output.stdout.contains(&0);
            (
                if is_binary {
                    String::new()
                } else {
                    String::from_utf8_lossy(&output.stdout).to_string()
                },
                is_binary,
            )
        } else {
            (String::new(), false)
        }
    } else {
        (String::new(), false)
    };

    let language = match Path::new(file_path)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .as_deref()
    {
        Some("ts" | "tsx") => "typescript",
        Some("js" | "mjs" | "cjs" | "jsx") => "javascript",
        Some("svelte" | "html") => "html",
        Some("json") => "json",
        Some("css") => "css",
        Some("scss") => "scss",
        Some("md") => "markdown",
        Some("yml" | "yaml") => "yaml",
        Some("sh") => "shell",
        Some("rs") => "rust",
        Some("py") => "python",
        Some("go") => "go",
        Some("java") => "java",
        Some("kt") => "kotlin",
        Some("swift") => "swift",
        _ => "plaintext",
    };

    Ok(json!({
        "repoPath": repo_root,
        "filePath": file_path,
        "originalPath": status.get("originalPath").cloned().unwrap_or(Value::Null),
        "originalContent": original_content,
        "modifiedContent": modified_content,
        "language": language,
        "isBinary": original_is_binary || modified_is_binary,
        "status": status
    }))
}

async fn resolve_git_repository_file_path(repo_root: &str, file_path: &str) -> ApiResult<PathBuf> {
    let repo_root_path = PathBuf::from(repo_root);
    let candidate_path = normalize_path(repo_root_path.join(file_path));
    let existing_path = tokio_fs::canonicalize(&candidate_path)
        .await
        .unwrap_or_else(|_| candidate_path.clone());
    if !path_is_within(&repo_root_path, &existing_path) {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "The selected file is outside the repository root.",
        ));
    }
    Ok(candidate_path)
}

pub(crate) async fn save_git_file_payload(
    state: &AppState,
    repo_path: &str,
    file_path: &str,
    content: &str,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let target_path = resolve_git_repository_file_path(&repo_root, file_path).await?;
    if let Some(parent) = target_path.parent() {
        tokio_fs::create_dir_all(parent)
            .await
            .map_err(|error| api_error(StatusCode::BAD_REQUEST, error.to_string()))?;
    }
    tokio_fs::write(&target_path, content)
        .await
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error.to_string()))?;
    get_git_file_payload(state, &repo_root, file_path).await
}

pub(crate) async fn stage_git_changes_payload(
    state: &AppState,
    repo_path: &str,
    file_path: Option<&str>,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let args = if let Some(file_path) = file_path.filter(|value| !value.trim().is_empty()) {
        vec!["add".to_string(), "--".to_string(), file_path.to_string()]
    } else {
        vec!["add".to_string(), "-A".to_string()]
    };
    run_git_text_payload(state, &repo_root, args).await?;
    get_git_status_payload(state, &repo_root).await
}

pub(crate) async fn unstage_git_changes_payload(
    state: &AppState,
    repo_path: &str,
    file_path: Option<&str>,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let args = if let Some(file_path) = file_path.filter(|value| !value.trim().is_empty()) {
        vec![
            "restore".to_string(),
            "--staged".to_string(),
            "--".to_string(),
            file_path.to_string(),
        ]
    } else {
        vec![
            "restore".to_string(),
            "--staged".to_string(),
            ".".to_string(),
        ]
    };
    run_git_text_payload(state, &repo_root, args).await?;
    get_git_status_payload(state, &repo_root).await
}

pub(crate) async fn fetch_git_repository_payload(
    state: &AppState,
    repo_path: &str,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    run_git_text_payload(
        state,
        &repo_root,
        vec![
            "fetch".to_string(),
            "--all".to_string(),
            "--prune".to_string(),
        ],
    )
    .await?;
    invalidate_git_repository_cache(state).await;
    get_git_status_payload(state, &repo_root).await
}

pub(crate) async fn pull_git_repository_payload(
    state: &AppState,
    repo_path: &str,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    run_git_text_payload(
        state,
        &repo_root,
        vec!["pull".to_string(), "--ff-only".to_string()],
    )
    .await?;
    invalidate_git_repository_cache(state).await;
    get_git_status_payload(state, &repo_root).await
}

pub(crate) async fn commit_git_changes_payload(
    state: &AppState,
    repo_path: &str,
    message: &str,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let trimmed_message = message.trim();
    if trimmed_message.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Commit message is required.",
        ));
    }
    run_git_text_payload(
        state,
        &repo_root,
        vec![
            "commit".to_string(),
            "-m".to_string(),
            trimmed_message.to_string(),
        ],
    )
    .await?;
    get_git_status_payload(state, &repo_root).await
}

pub(crate) async fn checkout_git_branch_payload(
    state: &AppState,
    repo_path: &str,
    branch_name: &str,
    create: bool,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let trimmed_branch_name = branch_name.trim();
    if trimmed_branch_name.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "branchName is required.",
        ));
    }
    let args = if create {
        vec![
            "switch".to_string(),
            "-c".to_string(),
            trimmed_branch_name.to_string(),
        ]
    } else {
        vec!["switch".to_string(), trimmed_branch_name.to_string()]
    };
    run_git_text_payload(state, &repo_root, args).await?;
    invalidate_git_repository_cache(state).await;
    get_git_status_payload(state, &repo_root).await
}

pub(crate) async fn get_git_commit_diff_payload(
    state: &AppState,
    repo_path: &str,
    commit_hash: &str,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let normalized_commit_hash = commit_hash.trim();
    if normalized_commit_hash.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "commitHash is required.",
        ));
    }
    let diff = run_git_text_payload(
        state,
        &repo_root,
        vec![
            "show".to_string(),
            "--format=".to_string(),
            "--find-renames".to_string(),
            "--find-copies".to_string(),
            "--no-ext-diff".to_string(),
            normalized_commit_hash.to_string(),
        ],
    )
    .await?;
    Ok(json!({
        "repoPath": repo_root,
        "commitHash": normalized_commit_hash,
        "diff": diff
    }))
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
