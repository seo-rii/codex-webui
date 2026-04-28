use super::*;

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
    ensure_text_file_preview_size(&candidate_path).await?;
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
            if output.stdout.len() as u64 > TEXT_FILE_PREVIEW_LIMIT_BYTES {
                return Err(api_error(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "The selected Git file is too large to preview.",
                ));
            }
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

pub(crate) async fn resolve_git_repository_file_path(
    repo_root: &str,
    file_path: &str,
) -> ApiResult<PathBuf> {
    let repo_root_path = tokio_fs::canonicalize(repo_root)
        .await
        .unwrap_or_else(|_| PathBuf::from(repo_root));
    let candidate_path = normalize_path(repo_root_path.join(file_path));
    ensure_not_sensitive_file_path(&candidate_path)?;
    let existing_path = tokio_fs::canonicalize(&candidate_path)
        .await
        .unwrap_or_else(|_| candidate_path.clone());
    ensure_not_sensitive_file_path(&existing_path)?;
    if !path_is_within(&repo_root_path, &existing_path) {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "The selected file is outside the repository root.",
        ));
    }
    Ok(candidate_path)
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
    let changed_files = run_git_text_payload(
        state,
        &repo_root,
        vec![
            "show".to_string(),
            "--format=".to_string(),
            "--name-only".to_string(),
            normalized_commit_hash.to_string(),
        ],
    )
    .await?
    .lines()
    .map(str::trim)
    .filter(|line| !line.is_empty())
    .count();
    if changed_files > GIT_DIFF_PREVIEW_MAX_FILES {
        return Err(api_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "The selected commit changes {changed_files} files, which exceeds the {GIT_DIFF_PREVIEW_MAX_FILES} file preview limit."
            ),
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
    if diff.len() > GIT_DIFF_PREVIEW_LIMIT_BYTES {
        return Err(api_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "The selected commit diff exceeds the {} byte preview limit.",
                GIT_DIFF_PREVIEW_LIMIT_BYTES
            ),
        ));
    }
    Ok(json!({
        "repoPath": repo_root,
        "commitHash": normalized_commit_hash,
        "changedFileCount": changed_files,
        "diff": diff
    }))
}
