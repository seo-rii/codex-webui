use super::*;

pub(crate) async fn handle_account_api_http(
    state: AppState,
    method: Method,
    route_path: String,
    request: Request,
    auth: AuthContext,
) -> Response {
    let result = match (method, route_path.as_str()) {
        (Method::GET, "/api/account") => get_account_state(&state, &auth.profile_id).await,
        (Method::POST, "/api/account/login") => {
            let body = to_bytes(request.into_body(), usize::MAX)
                .await
                .context("failed to read account login request body");
            match body {
                Ok(body) => {
                    let payload: Value =
                        serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
                    start_account_login(&state, &auth.profile_id, &payload).await
                }
                Err(error) => Err(error),
            }
        }
        (Method::POST, "/api/account/login/cancel") => {
            let body = to_bytes(request.into_body(), usize::MAX)
                .await
                .context("failed to read account login cancel request body");
            match body {
                Ok(body) => {
                    let payload: Value =
                        serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
                    cancel_account_login(&state, &auth.profile_id, &payload).await
                }
                Err(error) => Err(error),
            }
        }
        (Method::POST, "/api/account/logout") => logout_account(&state, &auth.profile_id).await,
        _ => return json_error(StatusCode::NOT_FOUND, "Not found."),
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => {
            let message = error.to_string();
            let status = if message.contains("required")
                || message.contains("Invalid account login type")
                || message.contains("API key is required")
            {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::BAD_GATEWAY
            };
            json_error(status, &message)
        }
    }
}

pub(crate) async fn handle_directories_api_http(state: AppState, request: Request) -> Response {
    if request.method() != Method::GET {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }

    let current_path = query_param_value(request.uri().query(), "path");
    match list_directories_payload(&state, current_path.as_deref()).await {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}

pub(crate) async fn handle_git_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    route_path: &str,
) -> Response {
    let method = request.method().clone();
    if method != Method::GET && auth.role != UserRole::Admin {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    let query = request.uri().query().map(str::to_string);
    let body = if matches!(
        method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    ) {
        match to_bytes(request.into_body(), usize::MAX)
            .await
            .context("failed to read git request body")
        {
            Ok(body) => Some(body),
            Err(_) => {
                return json_error(StatusCode::BAD_REQUEST, "Failed to read git request body.");
            }
        }
    } else {
        None
    };
    let payload = body
        .as_ref()
        .map(|body| serde_json::from_slice::<Value>(body).unwrap_or_else(|_| json!({})))
        .unwrap_or_else(|| json!({}));

    let result = if route_path == "/api/git/repositories" {
        if method != Method::GET {
            return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
        }
        list_git_repositories_payload(&state, true).await
    } else if route_path == "/api/git/status" {
        if method != Method::GET {
            return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
        }
        let Some(repo_path) = query_param_value(query.as_deref(), "repoPath") else {
            return json_error(StatusCode::BAD_REQUEST, "repoPath is required.");
        };
        get_git_status_payload(&state, &repo_path).await
    } else if route_path == "/api/git/file/resolve" {
        if method != Method::GET {
            return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
        }
        let Some(file_path) = query_param_value(query.as_deref(), "filePath") else {
            return json_error(StatusCode::BAD_REQUEST, "filePath is required.");
        };
        resolve_git_file_from_absolute_path_payload(&state, &file_path).await
    } else if route_path == "/api/git/file" {
        match method {
            Method::GET => {
                let Some(repo_path) = query_param_value(query.as_deref(), "repoPath") else {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        "repoPath and filePath are required.",
                    );
                };
                let Some(file_path) = query_param_value(query.as_deref(), "filePath") else {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        "repoPath and filePath are required.",
                    );
                };
                get_git_file_payload(&state, &repo_path, &file_path).await
            }
            Method::PUT => {
                let Some(repo_path) = payload.get("repoPath").and_then(Value::as_str) else {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        "repoPath, filePath, and content are required.",
                    );
                };
                let Some(file_path) = payload.get("filePath").and_then(Value::as_str) else {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        "repoPath, filePath, and content are required.",
                    );
                };
                let Some(content) = payload.get("content").and_then(Value::as_str) else {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        "repoPath, filePath, and content are required.",
                    );
                };
                save_git_file_payload(&state, repo_path, file_path, content).await
            }
            _ => return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed."),
        }
    } else if route_path == "/api/git/stage" {
        if method != Method::POST {
            return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
        }
        let Some(repo_path) = payload.get("repoPath").and_then(Value::as_str) else {
            return json_error(StatusCode::BAD_REQUEST, "repoPath is required.");
        };
        stage_git_changes_payload(
            &state,
            repo_path,
            payload.get("filePath").and_then(Value::as_str),
        )
        .await
    } else if route_path == "/api/git/unstage" {
        if method != Method::POST {
            return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
        }
        let Some(repo_path) = payload.get("repoPath").and_then(Value::as_str) else {
            return json_error(StatusCode::BAD_REQUEST, "repoPath is required.");
        };
        unstage_git_changes_payload(
            &state,
            repo_path,
            payload.get("filePath").and_then(Value::as_str),
        )
        .await
    } else if route_path == "/api/git/fetch" {
        if method != Method::POST {
            return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
        }
        let Some(repo_path) = payload.get("repoPath").and_then(Value::as_str) else {
            return json_error(StatusCode::BAD_REQUEST, "repoPath is required.");
        };
        fetch_git_repository_payload(&state, repo_path).await
    } else if route_path == "/api/git/pull" {
        if method != Method::POST {
            return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
        }
        let Some(repo_path) = payload.get("repoPath").and_then(Value::as_str) else {
            return json_error(StatusCode::BAD_REQUEST, "repoPath is required.");
        };
        pull_git_repository_payload(&state, repo_path).await
    } else if route_path == "/api/git/commit" {
        if method != Method::POST {
            return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
        }
        let Some(repo_path) = payload.get("repoPath").and_then(Value::as_str) else {
            return json_error(
                StatusCode::BAD_REQUEST,
                "repoPath and message are required.",
            );
        };
        let Some(message) = payload.get("message").and_then(Value::as_str) else {
            return json_error(
                StatusCode::BAD_REQUEST,
                "repoPath and message are required.",
            );
        };
        commit_git_changes_payload(&state, repo_path, message).await
    } else if route_path == "/api/git/checkout" {
        if method != Method::POST {
            return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
        }
        let Some(repo_path) = payload.get("repoPath").and_then(Value::as_str) else {
            return json_error(
                StatusCode::BAD_REQUEST,
                "repoPath and branchName are required.",
            );
        };
        let Some(branch_name) = payload.get("branchName").and_then(Value::as_str) else {
            return json_error(
                StatusCode::BAD_REQUEST,
                "repoPath and branchName are required.",
            );
        };
        checkout_git_branch_payload(
            &state,
            repo_path,
            branch_name,
            payload
                .get("create")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        )
        .await
    } else if route_path == "/api/git/worktrees" {
        match method {
            Method::GET => {
                let repo_path = query_param_value(query.as_deref(), "repoPath").unwrap_or_default();
                list_git_worktrees_payload(&state, &repo_path).await
            }
            Method::POST => {
                let repo_path = payload
                    .get("repoPath")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let worktree_path = payload
                    .get("worktreePath")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                create_git_worktree_payload(
                    &state,
                    repo_path,
                    worktree_path,
                    payload.get("branchName").and_then(Value::as_str),
                    payload
                        .get("createBranch")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    payload
                        .get("detach")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                )
                .await
            }
            Method::DELETE => {
                let repo_path = payload
                    .get("repoPath")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let worktree_path = payload
                    .get("worktreePath")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                remove_git_worktree_payload(
                    &state,
                    repo_path,
                    worktree_path,
                    payload
                        .get("force")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                )
                .await
            }
            _ => return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed."),
        }
    } else if route_path == "/api/git/commit/diff" {
        if method != Method::GET {
            return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
        }
        let Some(repo_path) = query_param_value(query.as_deref(), "repoPath") else {
            return json_error(
                StatusCode::BAD_REQUEST,
                "repoPath and commitHash are required.",
            );
        };
        let Some(commit_hash) = query_param_value(query.as_deref(), "commitHash") else {
            return json_error(
                StatusCode::BAD_REQUEST,
                "repoPath and commitHash are required.",
            );
        };
        get_git_commit_diff_payload(&state, &repo_path, &commit_hash).await
    } else if route_path == "/api/git/github/pulls" {
        if method != Method::GET {
            return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
        }
        let Some(repo_path) = query_param_value(query.as_deref(), "repoPath") else {
            return json_error(StatusCode::BAD_REQUEST, "repoPath is required.");
        };
        let pr_state =
            query_param_value(query.as_deref(), "state").unwrap_or_else(|| "open".to_string());
        let limit = query_param_value(query.as_deref(), "limit")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(20);
        list_github_pull_requests_payload(&state, &repo_path, &pr_state, limit).await
    } else if let Some(suffix) = route_path.strip_prefix("/api/git/github/pulls/") {
        if let Some(number_text) = suffix.strip_suffix("/checkout") {
            if method != Method::POST {
                return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
            }
            let Ok(number) = number_text.parse::<u64>() else {
                return json_error(
                    StatusCode::BAD_REQUEST,
                    "repoPath and pull request number are required.",
                );
            };
            let Some(repo_path) = payload.get("repoPath").and_then(Value::as_str) else {
                return json_error(
                    StatusCode::BAD_REQUEST,
                    "repoPath and pull request number are required.",
                );
            };
            checkout_github_pull_request_payload(&state, repo_path, number).await
        } else {
            if method != Method::GET {
                return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
            }
            let Ok(number) = suffix.parse::<u64>() else {
                return json_error(
                    StatusCode::BAD_REQUEST,
                    "repoPath and pull request number are required.",
                );
            };
            let Some(repo_path) = query_param_value(query.as_deref(), "repoPath") else {
                return json_error(
                    StatusCode::BAD_REQUEST,
                    "repoPath and pull request number are required.",
                );
            };
            get_github_pull_request_payload(&state, &repo_path, number).await
        }
    } else {
        return json_error(StatusCode::NOT_FOUND, "Not found.");
    };

    match result {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}
