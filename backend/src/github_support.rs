use super::*;

const GITHUB_PULL_REQUEST_FILES_PER_PAGE: u64 = 100;
const GITHUB_PULL_REQUEST_FILES_MAX_PAGES: u64 = 30;

pub(crate) fn parse_github_remote_payload(remote_name: &str, remote_url: &str) -> Option<Value> {
    let trimmed = remote_url.trim();
    if trimmed.is_empty() {
        return None;
    }

    let (host, owner, raw_name) = if let Some(rest) = trimmed.strip_prefix("git@") {
        let (host, path) = rest.split_once(':')?;
        let mut parts = path.split('/');
        let owner = parts.next()?;
        let raw_name = parts.collect::<Vec<_>>().join("/");
        (host.to_string(), owner.to_string(), raw_name)
    } else {
        let (_, rest) = trimmed.split_once("://")?;
        let rest = rest.strip_prefix("git@").unwrap_or(rest);
        let mut parts = rest.splitn(3, '/');
        let host = parts.next()?.to_string();
        let owner = parts.next()?.to_string();
        let raw_name = parts.next()?.to_string();
        (host, owner, raw_name)
    };

    let name = raw_name
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .to_string();
    if owner.is_empty() || name.is_empty() {
        return None;
    }

    Some(json!({
        "host": host,
        "owner": owner,
        "name": name,
        "remoteName": remote_name,
        "url": format!("https://{host}/{owner}/{name}")
    }))
}

async fn run_gh_text_payload(repo_path: &str, args: Vec<String>) -> ApiResult<String> {
    let mut command = Command::new("gh");
    command
        .args(args)
        .current_dir(repo_path)
        .envs(env::vars())
        .env("GH_PAGER", "cat")
        .env("GH_PROMPT_DISABLED", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("PAGER", "cat")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        command.process_group(0);
    }

    let mut child = command.spawn().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            api_error(
                StatusCode::BAD_REQUEST,
                "GitHub CLI (gh) is not installed on the server.",
            )
        } else {
            api_error(StatusCode::BAD_REQUEST, error.to_string())
        }
    })?;
    let child_pid = child.id();
    let stdout = child.stdout.take().ok_or_else(|| {
        api_error(
            StatusCode::BAD_REQUEST,
            "failed to capture gh command stdout.",
        )
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        api_error(
            StatusCode::BAD_REQUEST,
            "failed to capture gh command stderr.",
        )
    })?;

    let output = match tokio::time::timeout(Duration::from_secs(30), async {
        let (status, stdout, stderr) = tokio::try_join!(
            async {
                child
                    .wait()
                    .await
                    .with_context(|| "failed to wait for gh command")
            },
            read_child_pipe_limited(stdout, "stdout"),
            read_child_pipe_limited(stderr, "stderr")
        )?;
        Result::<std::process::Output>::Ok(std::process::Output {
            status,
            stdout,
            stderr,
        })
    })
    .await
    {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            terminate_child_process_group(&mut child, child_pid).await;
            return Err(api_error(StatusCode::BAD_REQUEST, error.to_string()));
        }
        Err(_) => {
            terminate_child_process_group(&mut child, child_pid).await;
            return Err(api_error(StatusCode::BAD_REQUEST, "`gh` timed out"));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.contains("executable file not found") || stderr.contains("not found") {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "GitHub CLI (gh) is not installed on the server.",
            ));
        }
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            if stderr.is_empty() {
                "gh command failed.".to_string()
            } else {
                stderr
            },
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub(crate) fn github_pull_request_files_truncated(
    expected_changed_files: u64,
    loaded_files: usize,
    hit_page_cap: bool,
) -> bool {
    hit_page_cap || expected_changed_files > loaded_files as u64
}

async fn list_github_pull_request_files_payloads(
    repo_root: &str,
    owner: &str,
    name: &str,
    pull_request_number: u64,
    expected_changed_files: u64,
) -> ApiResult<(Vec<Value>, bool)> {
    let mut files = Vec::new();
    let mut hit_page_cap = false;

    for page in 1..=GITHUB_PULL_REQUEST_FILES_MAX_PAGES {
        let files_raw = run_gh_text_payload(
            repo_root,
            vec![
                "api".to_string(),
                format!(
                    "repos/{owner}/{name}/pulls/{pull_request_number}/files?per_page={GITHUB_PULL_REQUEST_FILES_PER_PAGE}&page={page}"
                ),
            ],
        )
        .await?;
        let files_page = serde_json::from_str::<Value>(&files_raw)
            .map_err(|error| api_error(StatusCode::BAD_REQUEST, error.to_string()))?;
        let mut page_files = files_page.as_array().cloned().unwrap_or_default();
        let page_file_count = page_files.len() as u64;
        files.append(&mut page_files);

        if page_file_count < GITHUB_PULL_REQUEST_FILES_PER_PAGE {
            let loaded_files = files.len();
            return Ok((
                files,
                github_pull_request_files_truncated(expected_changed_files, loaded_files, false),
            ));
        }

        hit_page_cap = page == GITHUB_PULL_REQUEST_FILES_MAX_PAGES;
    }

    let loaded_files = files.len();
    Ok((
        files,
        github_pull_request_files_truncated(expected_changed_files, loaded_files, hit_page_cap),
    ))
}

pub(crate) async fn resolve_github_repository_payload(
    state: &AppState,
    repo_path: &str,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let remote_names = run_git_text_payload(state, &repo_root, vec!["remote".to_string()])
        .await?
        .lines()
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();

    let mut ordered_remote_names = vec!["origin".to_string()];
    ordered_remote_names.extend(
        remote_names
            .into_iter()
            .filter(|entry| entry != "origin")
            .collect::<Vec<_>>(),
    );

    for remote_name in ordered_remote_names {
        let remote_url = run_git_text_payload(
            state,
            &repo_root,
            vec![
                "config".to_string(),
                "--get".to_string(),
                format!("remote.{remote_name}.url"),
            ],
        )
        .await
        .unwrap_or_default();
        if let Some(parsed) = parse_github_remote_payload(&remote_name, &remote_url) {
            return Ok(parsed);
        }
    }

    Err(api_error(
        StatusCode::BAD_REQUEST,
        "No GitHub remote was found for the selected repository.",
    ))
}

fn map_github_pull_request_summary_payload(pull_request: &Value) -> Value {
    let merged_at = pull_request.get("merged_at").and_then(Value::as_str);
    let labels = pull_request
        .get("labels")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("name").and_then(Value::as_str))
                .map(|label| Value::String(label.to_string()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    json!({
        "number": pull_request.get("number").and_then(Value::as_u64).unwrap_or(0),
        "title": pull_request.get("title").and_then(Value::as_str).unwrap_or("Untitled PR"),
        "state": if merged_at.is_some() {
            "merged"
        } else if pull_request.get("state").and_then(Value::as_str) == Some("closed") {
            "closed"
        } else {
            "open"
        },
        "isDraft": pull_request.get("draft").and_then(Value::as_bool).unwrap_or(false),
        "url": pull_request.get("html_url").and_then(Value::as_str).unwrap_or_default(),
        "author": pull_request
            .get("user")
            .and_then(|value| value.get("login"))
            .and_then(Value::as_str)
            .map(Value::from)
            .unwrap_or(Value::Null),
        "authorUrl": pull_request
            .get("user")
            .and_then(|value| value.get("html_url"))
            .and_then(Value::as_str)
            .map(Value::from)
            .unwrap_or(Value::Null),
        "baseRefName": pull_request
            .get("base")
            .and_then(|value| value.get("ref"))
            .and_then(Value::as_str)
            .unwrap_or_default(),
        "headRefName": pull_request
            .get("head")
            .and_then(|value| value.get("ref"))
            .and_then(Value::as_str)
            .unwrap_or_default(),
        "updatedAt": pull_request
            .get("updated_at")
            .and_then(Value::as_str)
            .map(Value::from)
            .unwrap_or(Value::Null),
        "additions": pull_request.get("additions").and_then(Value::as_i64).unwrap_or(0),
        "deletions": pull_request.get("deletions").and_then(Value::as_i64).unwrap_or(0),
        "changedFiles": pull_request.get("changed_files").and_then(Value::as_i64).unwrap_or(0),
        "labels": labels
    })
}

fn map_github_pull_request_file_payload(file: &Value) -> Value {
    json!({
        "path": file.get("filename").and_then(Value::as_str).unwrap_or_default(),
        "previousPath": file
            .get("previous_filename")
            .and_then(Value::as_str)
            .map(Value::from)
            .unwrap_or(Value::Null),
        "status": file.get("status").and_then(Value::as_str).unwrap_or("modified"),
        "additions": file.get("additions").and_then(Value::as_i64).unwrap_or(0),
        "deletions": file.get("deletions").and_then(Value::as_i64).unwrap_or(0),
        "patch": file
            .get("patch")
            .and_then(Value::as_str)
            .map(Value::from)
            .unwrap_or(Value::Null)
    })
}

pub(crate) async fn list_github_pull_requests_payload(
    state: &AppState,
    repo_path: &str,
    pr_state: &str,
    limit: u64,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let repository = resolve_github_repository_payload(state, &repo_root).await?;
    let owner = repository
        .get("owner")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let name = repository
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let normalized_state = match pr_state {
        "closed" | "all" => pr_state,
        _ => "open",
    };
    let normalized_limit = limit.clamp(1, 50);
    let raw = run_gh_text_payload(
        &repo_root,
        vec![
            "api".to_string(),
            format!(
                "repos/{owner}/{name}/pulls?state={}&per_page={normalized_limit}",
                urlencoding::encode(normalized_state)
            ),
        ],
    )
    .await?;
    let pull_requests = serde_json::from_str::<Value>(&raw)
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error.to_string()))?;
    let summaries = pull_requests
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|entry| map_github_pull_request_summary_payload(&entry))
        .collect::<Vec<_>>();

    Ok(json!({
        "repository": repository,
        "pullRequests": summaries
    }))
}

pub(crate) async fn get_github_pull_request_payload(
    state: &AppState,
    repo_path: &str,
    number: u64,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let repository = resolve_github_repository_payload(state, &repo_root).await?;
    let owner = repository
        .get("owner")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let name = repository
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let pull_request_number = number.max(1);
    let pull_request_raw = run_gh_text_payload(
        &repo_root,
        vec![
            "api".to_string(),
            format!("repos/{owner}/{name}/pulls/{pull_request_number}"),
        ],
    )
    .await?;
    let pull_request = serde_json::from_str::<Value>(&pull_request_raw)
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error.to_string()))?;
    let expected_changed_files = pull_request
        .get("changed_files")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let (files, files_truncated) = list_github_pull_request_files_payloads(
        &repo_root,
        owner,
        name,
        pull_request_number,
        expected_changed_files,
    )
    .await?;
    let files_loaded = files.len() as u64;

    let mut detail = map_github_pull_request_summary_payload(&pull_request)
        .as_object()
        .cloned()
        .unwrap_or_default();
    detail.insert(
        "body".to_string(),
        pull_request
            .get("body")
            .and_then(Value::as_str)
            .map(Value::from)
            .unwrap_or_else(|| Value::String(String::new())),
    );
    detail.insert(
        "reviewDecision".to_string(),
        pull_request
            .get("review_decision")
            .and_then(Value::as_str)
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    detail.insert(
        "mergeStateStatus".to_string(),
        pull_request
            .get("mergeable_state")
            .and_then(Value::as_str)
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    detail.insert(
        "commits".to_string(),
        Value::from(
            pull_request
                .get("commits")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        ),
    );
    detail.insert(
        "files".to_string(),
        Value::Array(
            files
                .into_iter()
                .map(|entry| map_github_pull_request_file_payload(&entry))
                .collect(),
        ),
    );
    detail.insert("filesLoaded".to_string(), Value::from(files_loaded));
    detail.insert("filesTruncated".to_string(), Value::from(files_truncated));

    Ok(json!({
        "repository": repository,
        "pullRequest": Value::Object(detail)
    }))
}

pub(crate) async fn checkout_github_pull_request_payload(
    state: &AppState,
    repo_path: &str,
    number: u64,
) -> ApiResult<Value> {
    let repo_root = resolve_git_repo_root(state, repo_path).await?;
    let repo_lock = git_operation_lock(state, &repo_root).await;
    let _repo_guard = repo_lock.lock().await;
    reject_git_mutation_if_repo_busy(state, &repo_root).await?;
    let pull_request_number = number.max(1);
    run_gh_text_payload(
        &repo_root,
        vec![
            "pr".to_string(),
            "checkout".to_string(),
            pull_request_number.to_string(),
        ],
    )
    .await?;
    invalidate_git_repository_cache(state).await;
    get_git_status_payload(state, &repo_root).await
}
