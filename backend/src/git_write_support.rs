use super::*;

pub(crate) async fn git_operation_lock(state: &AppState, repo_root: &str) -> Arc<Mutex<()>> {
    let lock_key = git_common_dir_lock_key(repo_root).await;
    let mut locks = state.git_operation_locks.lock().await;
    locks.retain(|_, lock| Arc::strong_count(lock) > 1);
    if !locks.contains_key(&lock_key) && locks.len() >= GIT_OPERATION_LOCK_MAX_ENTRIES {
        if let Some(idle_key) = locks
            .iter()
            .find(|(_, lock)| Arc::strong_count(lock) == 1)
            .map(|(key, _)| key.clone())
        {
            locks.remove(&idle_key);
        }
    }
    Arc::clone(
        locks
            .entry(lock_key)
            .or_insert_with(|| Arc::new(Mutex::new(()))),
    )
}

fn runtime_session_key_parts(key: &str) -> Option<(&str, &str)> {
    key.strip_prefix("profile::")?
        .split_once("::session-runtime::")
}

pub(crate) async fn reject_git_mutation_if_repo_busy(
    state: &AppState,
    repo_root: &str,
) -> ApiResult<()> {
    let repo_root_path = tokio_fs::canonicalize(repo_root)
        .await
        .unwrap_or_else(|_| PathBuf::from(repo_root));
    let repo_common_dir = git_common_dir_lock_key(repo_root).await;
    let runtime_keys = {
        let active_turns = state.active_turns.lock().await;
        let pending_turn_starts = state.pending_turn_starts.lock().await;
        active_turns
            .keys()
            .chain(pending_turn_starts.iter())
            .cloned()
            .collect::<HashSet<_>>()
    };

    for runtime_key in runtime_keys {
        let Some((profile_id, session_id)) = runtime_session_key_parts(&runtime_key) else {
            continue;
        };
        let candidate_paths = with_ui_state_read(state, profile_id, |ui_state| {
            let preferences = ui_state
                .get("preferencesByThreadId")
                .and_then(Value::as_object)
                .and_then(|entries| entries.get(session_id))
                .and_then(Value::as_object);
            let mut paths = Vec::new();
            if let Some(preferences) = preferences {
                for key in ["gitRepoPath", "cwd"] {
                    if let Some(path) = preferences
                        .get(key)
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                    {
                        paths.push(path.to_string());
                    }
                }
            }
            Ok(paths)
        })
        .await
        .unwrap_or_default();

        for candidate in candidate_paths {
            let candidate_path = tokio_fs::canonicalize(&candidate)
                .await
                .unwrap_or_else(|_| PathBuf::from(&candidate));
            if path_is_within(&repo_root_path, &candidate_path)
                || path_is_within(&candidate_path, &repo_root_path)
                || git_common_dir_lock_key(&candidate).await == repo_common_dir
            {
                return Err(api_error(
                    StatusCode::CONFLICT,
                    "Refusing to mutate this repository while a Codex turn is active.",
                ));
            }
        }
    }

    Ok(())
}

pub(crate) async fn save_git_file_payload(
    state: &AppState,
    repo_path: &str,
    file_path: &str,
    content: &str,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let repo_lock = git_operation_lock(state, &repo_root).await;
    let _repo_guard = repo_lock.lock().await;
    reject_git_mutation_if_repo_busy(state, &repo_root).await?;
    let target_path = resolve_git_repository_file_path(&repo_root, file_path).await?;
    let repo_root_path = tokio_fs::canonicalize(&repo_root)
        .await
        .unwrap_or_else(|_| PathBuf::from(&repo_root));
    write_text_file_safely(&target_path, content, &[repo_root_path]).await?;
    get_git_file_payload_for_root(state, &repo_root, file_path).await
}

pub(crate) async fn stage_git_changes_payload(
    state: &AppState,
    repo_path: &str,
    file_path: Option<&str>,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let repo_lock = git_operation_lock(state, &repo_root).await;
    let _repo_guard = repo_lock.lock().await;
    reject_git_mutation_if_repo_busy(state, &repo_root).await?;
    let args = if let Some(file_path) = file_path.filter(|value| !value.trim().is_empty()) {
        vec![
            "--literal-pathspecs".to_string(),
            "add".to_string(),
            "--".to_string(),
            file_path.to_string(),
        ]
    } else {
        vec!["add".to_string(), "-A".to_string()]
    };
    run_git_text_payload(state, &repo_root, args).await?;
    get_git_status_payload_for_root(state, &repo_root).await
}

pub(crate) async fn unstage_git_changes_payload(
    state: &AppState,
    repo_path: &str,
    file_path: Option<&str>,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let repo_lock = git_operation_lock(state, &repo_root).await;
    let _repo_guard = repo_lock.lock().await;
    reject_git_mutation_if_repo_busy(state, &repo_root).await?;
    let args = if let Some(file_path) = file_path.filter(|value| !value.trim().is_empty()) {
        vec![
            "--literal-pathspecs".to_string(),
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
    get_git_status_payload_for_root(state, &repo_root).await
}

pub(crate) async fn fetch_git_repository_payload(
    state: &AppState,
    repo_path: &str,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let repo_lock = git_operation_lock(state, &repo_root).await;
    let _repo_guard = repo_lock.lock().await;
    reject_git_mutation_if_repo_busy(state, &repo_root).await?;
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
    get_git_status_payload_for_root(state, &repo_root).await
}

pub(crate) async fn pull_git_repository_payload(
    state: &AppState,
    repo_path: &str,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let repo_lock = git_operation_lock(state, &repo_root).await;
    let _repo_guard = repo_lock.lock().await;
    reject_git_mutation_if_repo_busy(state, &repo_root).await?;
    run_git_text_payload(
        state,
        &repo_root,
        vec!["pull".to_string(), "--ff-only".to_string()],
    )
    .await?;
    invalidate_git_repository_cache(state).await;
    get_git_status_payload_for_root(state, &repo_root).await
}

pub(crate) async fn commit_git_changes_payload(
    state: &AppState,
    repo_path: &str,
    message: &str,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let repo_lock = git_operation_lock(state, &repo_root).await;
    let _repo_guard = repo_lock.lock().await;
    reject_git_mutation_if_repo_busy(state, &repo_root).await?;
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
    get_git_status_payload_for_root(state, &repo_root).await
}

pub(crate) async fn checkout_git_branch_payload(
    state: &AppState,
    repo_path: &str,
    branch_name: &str,
    create: bool,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let repo_lock = git_operation_lock(state, &repo_root).await;
    let _repo_guard = repo_lock.lock().await;
    reject_git_mutation_if_repo_busy(state, &repo_root).await?;
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
    get_git_status_payload_for_root(state, &repo_root).await
}
