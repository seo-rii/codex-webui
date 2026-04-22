use super::*;

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
