use super::*;

pub(crate) async fn git_operation_lock(state: &AppState, repo_root: &str) -> Arc<Mutex<()>> {
    let mut locks = state.git_operation_locks.lock().await;
    Arc::clone(
        locks
            .entry(repo_root.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(()))),
    )
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
    let target_path = resolve_git_repository_file_path(&repo_root, file_path).await?;
    let repo_root_path = tokio_fs::canonicalize(&repo_root)
        .await
        .unwrap_or_else(|_| PathBuf::from(&repo_root));
    write_text_file_safely(&target_path, content, &[repo_root_path]).await?;
    get_git_file_payload(state, &repo_root, file_path).await
}

pub(crate) async fn stage_git_changes_payload(
    state: &AppState,
    repo_path: &str,
    file_path: Option<&str>,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let repo_lock = git_operation_lock(state, &repo_root).await;
    let _repo_guard = repo_lock.lock().await;
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
    let repo_lock = git_operation_lock(state, &repo_root).await;
    let _repo_guard = repo_lock.lock().await;
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
    let repo_lock = git_operation_lock(state, &repo_root).await;
    let _repo_guard = repo_lock.lock().await;
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
    let repo_lock = git_operation_lock(state, &repo_root).await;
    let _repo_guard = repo_lock.lock().await;
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
    let repo_lock = git_operation_lock(state, &repo_root).await;
    let _repo_guard = repo_lock.lock().await;
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
    let repo_lock = git_operation_lock(state, &repo_root).await;
    let _repo_guard = repo_lock.lock().await;
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
