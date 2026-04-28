use super::*;

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
    let repo_lock = git_operation_lock(state, &repo_root).await;
    let _repo_guard = repo_lock.lock().await;
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
    let repo_lock = git_operation_lock(state, &repo_root).await;
    let _repo_guard = repo_lock.lock().await;
    let resolved_worktree_path = resolve_git_worktree_path(state, worktree_path).await?;
    let worktrees_output = run_git_text_payload(
        state,
        &repo_root,
        vec![
            "worktree".to_string(),
            "list".to_string(),
            "--porcelain".to_string(),
        ],
    )
    .await?;
    let registered = parse_git_worktrees_payload(&repo_root, &worktrees_output)
        .iter()
        .any(|entry| {
            entry.get("path").and_then(Value::as_str) == Some(resolved_worktree_path.as_str())
        });
    if !registered {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "The selected path is not a registered worktree.",
        ));
    }
    if force {
        let dirty_output = run_git_text_payload(
            state,
            &resolved_worktree_path,
            vec!["status".to_string(), "--porcelain".to_string()],
        )
        .await?;
        if !dirty_output.trim().is_empty() {
            return Err(api_error(
                StatusCode::CONFLICT,
                "Refusing to force-remove a worktree with uncommitted changes.",
            ));
        }
    }
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
