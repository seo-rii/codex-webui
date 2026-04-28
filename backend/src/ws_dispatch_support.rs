use super::*;

pub(crate) async fn execute_ws_method(
    state: &AppState,
    out_tx: &mpsc::Sender<ServerEnvelope>,
    subscriptions: &Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    auth: &AuthContext,
    method: &str,
    params: Value,
) -> Result<Value> {
    if !is_ws_method_allowed(auth.role, method) {
        return Err(anyhow!(
            "{{\"code\":\"FORBIDDEN_ROLE\",\"message\":\"This action requires an admin role.\"}}"
        ));
    }
    if ws_method_requires_owner(method, &params) && !role_has_owner_access(&state.config, auth.role)
    {
        return Err(anyhow!(owner_required_error_value()));
    }

    match method {
        "config/get" => get_config_payload(state, &auth.profile_id)
            .await
            .map_err(anyhow::Error::from),
        "config/update" => update_config_payload(state, &auth.profile_id, params)
            .await
            .map_err(anyhow::Error::from),
        "notifications/list" => {
            let limit = params
                .get("limit")
                .and_then(Value::as_u64)
                .map(|value| value.clamp(1, 200) as usize)
                .unwrap_or(DEFAULT_NOTIFICATION_LIMIT);
            get_notifications_payload(state, &auth.profile_id, limit)
                .await
                .map_err(anyhow::Error::from)
        }
        "audit/list" => {
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(120) as usize;
            list_audit_log(&state.config, limit).await
        }
        "notifications/markRead" => {
            let ids = params.get("ids").and_then(Value::as_array).map(|entries| {
                entries
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            });
            mark_notifications_read_payload(state, &auth.profile_id, ids)
                .await
                .map_err(anyhow::Error::from)
        }
        "notifications/clear" => clear_notifications_payload(state, &auth.profile_id)
            .await
            .map_err(anyhow::Error::from),
        "notifications/settings/update" => {
            let payload = json!({
                "enabledEventTypes": params.get("enabledEventTypes").cloned().unwrap_or(Value::Null),
                "slackWebhookUrl": params.get("slackWebhookUrl").cloned().unwrap_or(Value::Null),
                "webhookUrl": params.get("webhookUrl").cloned().unwrap_or(Value::Null)
            });
            update_notification_settings_payload(state, &auth.profile_id, payload)
                .await
                .map_err(anyhow::Error::from)
        }
        "automations/save" => save_automation_payload(
            state,
            &auth.profile_id,
            params.get("automation").cloned().unwrap_or(Value::Null),
        )
        .await
        .map_err(anyhow::Error::from),
        "automations/delete" => {
            let automation_id = require_string(&params, "automationId")?;
            delete_automation_payload(state, &auth.profile_id, &automation_id)
                .await
                .map_err(anyhow::Error::from)
        }
        "automations/run" => {
            let automation_id = require_string(&params, "automationId")?;
            let trigger = if params.get("trigger").and_then(Value::as_str) == Some("schedule") {
                "schedule"
            } else {
                "manual"
            };
            run_automation_payload(state, &auth.profile_id, &automation_id, trigger)
                .await
                .map_err(anyhow::Error::from)
        }
        "runtime/status" => codex_runtime_status(state, false).await,
        "runtime/checkUpdate" => codex_runtime_status(state, true).await,
        "runtime/quota" => {
            codex_quota_status(
                state,
                params
                    .get("refresh")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                &auth.profile_id,
            )
            .await
        }
        "catalog/get" => get_catalog_payload(state, &auth.profile_id)
            .await
            .map_err(anyhow::Error::from),
        "editor/file/get" => {
            let file_path = require_string(&params, "filePath")?;
            read_editable_file_payload(state, &auth.profile_id, &file_path)
                .await
                .map_err(anyhow::Error::from)
        }
        "editor/file/save" => {
            let file_path = require_string(&params, "filePath")?;
            let content = params
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            write_editable_file_payload(state, &auth.profile_id, &file_path, &content)
                .await
                .map_err(anyhow::Error::from)
        }
        "runtime/install" => install_or_update_codex(state, true).await,
        "runtime/update" => install_or_update_codex(state, false).await,
        "sessions/list" => {
            let archived = params
                .get("archived")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let known_version = params
                .get("knownVersion")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let known_summary_versions =
                session_summary_versions_from_value(params.get("knownSummaryVersions"));
            let known_state_hash = params
                .get("knownStateHash")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let cursor = params
                .get("cursor")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty());
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(20);
            let filter = session_filter_from_value(params.get("filter"));
            let payload =
                list_sessions_payload(state, &auth.profile_id, archived, cursor, limit, &filter)
                    .await
                    .map_err(anyhow::Error::from)?;
            Ok(cacheable_session_list_response(
                payload,
                known_version,
                known_summary_versions,
                known_state_hash,
            ))
        }
        "sessions/search" => {
            let archived = params
                .get("archived")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let query_raw = require_string(&params, "query")?;
            let known_version = params
                .get("knownVersion")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let known_summary_versions =
                session_summary_versions_from_value(params.get("knownSummaryVersions"));
            let known_state_hash = params
                .get("knownStateHash")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let scope = if params.get("scope").and_then(Value::as_str) == Some("full") {
                "full"
            } else {
                "summary"
            };
            let cursor = params
                .get("cursor")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty());
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(20);
            let filter = session_filter_from_value(params.get("filter"));
            let payload = search_sessions_payload(
                state,
                &auth.profile_id,
                &query_raw,
                scope,
                archived,
                cursor,
                limit,
                &filter,
            )
            .await
            .map_err(anyhow::Error::from)?;
            Ok(cacheable_session_list_response(
                payload,
                known_version,
                known_summary_versions,
                known_state_hash,
            ))
        }
        "session/create" => create_session_payload(
            state,
            &auth.profile_id,
            params
                .get("preferences")
                .cloned()
                .unwrap_or_else(|| json!({})),
            params.get("selectedSkills"),
            params.get("name").and_then(Value::as_str),
        )
        .await
        .map_err(anyhow::Error::from),
        "session/organization/update" => {
            let session_id = require_session_id(&params, "sessionId")?;
            update_session_organization_payload(state, &auth.profile_id, &session_id, params)
                .await
                .map_err(anyhow::Error::from)
        }
        "sessionFilters/save" => save_session_filter_payload(
            state,
            &auth.profile_id,
            params.get("filter").cloned().unwrap_or(Value::Null),
        )
        .await
        .map_err(anyhow::Error::from),
        "sessionFilters/delete" => {
            let filter_id = require_string(&params, "filterId")?;
            delete_session_filter_payload(state, &auth.profile_id, &filter_id)
                .await
                .map_err(anyhow::Error::from)
        }
        "promptPresets/save" => save_prompt_preset_payload(
            state,
            &auth.profile_id,
            params.get("preset").cloned().unwrap_or(Value::Null),
        )
        .await
        .map_err(anyhow::Error::from),
        "promptPresets/delete" => {
            let preset_id = require_string(&params, "presetId")?;
            delete_prompt_preset_payload(state, &auth.profile_id, &preset_id)
                .await
                .map_err(anyhow::Error::from)
        }
        "session/get" => {
            let session_id = require_session_id(&params, "sessionId")?;
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(20);
            let known_version = params
                .get("knownVersion")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let known_turn_versions =
                session_detail_turn_versions_from_value(params.get("knownTurnVersions"));
            let known_state_hash = params
                .get("knownStateHash")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let payload = session_detail_payload(state, &auth.profile_id, &session_id, limit)
                .await
                .map_err(anyhow::Error::from)?;
            Ok(cacheable_session_detail_response(
                payload,
                known_version,
                known_turn_versions,
                known_state_hash,
            ))
        }
        "session/recovery" => {
            let session_id = require_session_id(&params, "sessionId")?;
            recover_session_rollout_payload(state, &auth.profile_id, auth.role, &session_id)
                .await
                .map_err(RolloutRecoveryActionError::into_ws_error)
        }
        "session/fork" => {
            let session_id = require_session_id(&params, "sessionId")?;
            fork_session_payload(
                state,
                &auth.profile_id,
                &session_id,
                params.get("mode").and_then(Value::as_str).unwrap_or("fork"),
                params.get("turnId").and_then(Value::as_str),
                params.get("messageText").and_then(Value::as_str),
            )
            .await
            .map_err(anyhow::Error::from)
        }
        "session/search" => {
            let session_id = require_session_id(&params, "sessionId")?;
            let query_raw = require_string(&params, "query")?;
            let cursor = params
                .get("cursor")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty());
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(20);
            search_session_turns_payload(
                state,
                &auth.profile_id,
                &session_id,
                &query_raw,
                cursor,
                limit,
            )
            .await
            .map_err(anyhow::Error::from)
        }
        "session/olderTurns/get" => {
            let session_id = require_session_id(&params, "sessionId")?;
            let before_turn_id = require_string(&params, "beforeTurnId")?;
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(20);
            session_older_turns_payload(
                state,
                &auth.profile_id,
                &session_id,
                &before_turn_id,
                limit,
            )
            .await
            .map_err(anyhow::Error::from)
        }
        "session/turn/get" => {
            let session_id = require_session_id(&params, "sessionId")?;
            let turn_id = require_string(&params, "turnId")?;
            session_turn_payload(state, &auth.profile_id, &session_id, &turn_id)
                .await
                .map_err(anyhow::Error::from)
        }
        "session/itemDetail/get" => {
            let session_id = require_session_id(&params, "sessionId")?;
            let turn_id = require_string(&params, "turnId")?;
            let item_id = require_string(&params, "itemId")?;
            session_item_detail_payload(state, &auth.profile_id, &session_id, &turn_id, &item_id)
                .await
                .map_err(anyhow::Error::from)
        }
        "session/draft/get" => {
            let session_id = require_session_id(&params, "sessionId")?;
            get_session_draft_payload(state, &auth.profile_id, &session_id)
                .await
                .map_err(anyhow::Error::from)
        }
        "session/draft/save" => {
            let session_id = require_session_id(&params, "sessionId")?;
            save_session_draft_payload(
                state,
                &auth.profile_id,
                &session_id,
                params
                    .get("draft")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                params
                    .get("intent")
                    .and_then(Value::as_str)
                    .unwrap_or("message"),
            )
            .await
            .map_err(anyhow::Error::from)
        }
        "session/draft/clear" => {
            let session_id = require_session_id(&params, "sessionId")?;
            clear_session_draft_payload(state, &auth.profile_id, &session_id)
                .await
                .map_err(anyhow::Error::from)
        }
        "session/queue/get" => {
            let session_id = require_session_id(&params, "sessionId")?;
            get_session_queue_payload(state, &auth.profile_id, &session_id)
                .await
                .map_err(anyhow::Error::from)
        }
        "session/queue/enqueue" => {
            let session_id = require_session_id(&params, "sessionId")?;
            enqueue_session_queue_payload(
                state,
                &auth.profile_id,
                &session_id,
                params
                    .get("prompt")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                params.get("clientRequestId").and_then(Value::as_str),
                params.get("skills"),
                params.get("attachmentIds"),
            )
            .await
            .map_err(anyhow::Error::from)
        }
        "session/queue/resume" => {
            let session_id = require_session_id(&params, "sessionId")?;
            resume_session_queue_payload(state, &auth.profile_id, &session_id)
                .await
                .map_err(anyhow::Error::from)
        }
        "session/queue/remove" => {
            let session_id = require_session_id(&params, "sessionId")?;
            let queue_id = require_string(&params, "queueId")?;
            remove_session_queue_item_payload(state, &auth.profile_id, &session_id, &queue_id)
                .await
                .map_err(anyhow::Error::from)
        }
        "session/queue/update" => {
            let session_id = require_session_id(&params, "sessionId")?;
            let queue_id = require_string(&params, "queueId")?;
            update_session_queue_item_payload(
                state,
                &auth.profile_id,
                &session_id,
                &queue_id,
                params.get("prompt").and_then(Value::as_str),
                params.get("skills"),
                params.get("attachmentIds"),
            )
            .await
            .map_err(anyhow::Error::from)
        }
        "session/queue/reorder" => {
            let session_id = require_session_id(&params, "sessionId")?;
            let queue_ids = string_array_from_value(params.get("queueIds"));
            reorder_session_queue_payload(state, &auth.profile_id, &session_id, &queue_ids)
                .await
                .map_err(anyhow::Error::from)
        }
        "session/queue/dispatch" => {
            let session_id = require_session_id(&params, "sessionId")?;
            let queue_id = require_string(&params, "queueId")?;
            dispatch_session_queue_item_payload(
                state,
                &auth.profile_id,
                &session_id,
                &queue_id,
                &require_string(&params, "mode")?,
            )
            .await
            .map_err(anyhow::Error::from)
        }
        "session/savePreferences" => {
            let session_id = require_session_id(&params, "sessionId")?;
            save_session_preferences_payload(
                state,
                &auth.profile_id,
                &session_id,
                params
                    .get("preferences")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
            )
            .await
            .map_err(anyhow::Error::from)
        }
        "session/skills/save" => {
            let session_id = require_session_id(&params, "sessionId")?;
            save_session_skills_payload(state, &auth.profile_id, &session_id, params.get("skills"))
                .await
                .map_err(anyhow::Error::from)
        }
        "session/rename" => {
            let session_id = require_session_id(&params, "sessionId")?;
            rename_session_payload(
                state,
                &auth.profile_id,
                &session_id,
                &require_string(&params, "name")?,
            )
            .await
            .map_err(anyhow::Error::from)
        }
        "session/archive" => {
            let session_id = require_session_id(&params, "sessionId")?;
            archive_session_payload(state, &auth.profile_id, &session_id)
                .await
                .map_err(anyhow::Error::from)
        }
        "session/unarchive" => {
            let session_id = require_session_id(&params, "sessionId")?;
            unarchive_session_payload(state, &auth.profile_id, &session_id)
                .await
                .map_err(anyhow::Error::from)
        }
        "turn/send" => {
            let session_id = require_session_id(&params, "sessionId")?;
            send_turn_payload(
                state,
                &auth.profile_id,
                &session_id,
                params
                    .get("prompt")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                params.get("attachmentIds"),
                params.get("skills"),
                params
                    .get("preferences")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
            )
            .await
            .map_err(anyhow::Error::from)
        }
        "turn/steer" => {
            let session_id = require_session_id(&params, "sessionId")?;
            let prompt = require_string(&params, "prompt")?;
            steer_turn_payload(
                state,
                &auth.profile_id,
                &session_id,
                &prompt,
                params.get("attachmentIds"),
                params.get("skills"),
            )
            .await
            .map_err(anyhow::Error::from)
        }
        "turn/abort" => {
            let session_id = require_session_id(&params, "sessionId")?;
            abort_turn_payload(state, &auth.profile_id, &session_id)
                .await
                .map_err(anyhow::Error::from)
        }
        "approval/resolve" => {
            let session_id = require_session_id(&params, "sessionId")?;
            let request_id = require_string(&params, "requestId")?;
            resolve_server_request_payload(
                state,
                &auth.profile_id,
                &session_id,
                &request_id,
                params.get("result").cloned().unwrap_or(Value::Null),
            )
            .await
            .map_err(anyhow::Error::from)
        }
        "directories/browse" => {
            let current_path = params.get("currentPath").and_then(Value::as_str);
            list_directories_payload(state, current_path)
                .await
                .map_err(anyhow::Error::from)
        }
        "attachments/upload" => {
            let session_id = require_session_id(&params, "sessionId")?;
            let files = params
                .get("files")
                .cloned()
                .ok_or_else(|| anyhow!("files is required"))?;
            let files: Vec<UploadFilePayload> = serde_json::from_value(files)?;
            upload_attachments(state, &auth.profile_id, &session_id, files)
                .await
                .map_err(anyhow::Error::from)
        }
        "attachments/delete" => {
            let session_id = require_session_id(&params, "sessionId")?;
            let attachment_id = require_string(&params, "attachmentId")?;
            delete_attachment_payload(state, &auth.profile_id, &session_id, &attachment_id)
                .await
                .map_err(anyhow::Error::from)
        }
        "account/get" => get_account_state(state, &auth.profile_id).await,
        "account/login/start" => start_account_login(state, &auth.profile_id, &params).await,
        "account/login/cancel" => cancel_account_login(state, &auth.profile_id, &params).await,
        "account/logout" => logout_account(state, &auth.profile_id).await,
        "arena/list" => list_arena_runs_payload(state, &auth.profile_id)
            .await
            .map_err(anyhow::Error::from),
        "arena/start" => start_arena_run_payload(
            state,
            &auth.profile_id,
            params
                .get("prompt")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            params.get("contestants").unwrap_or(&Value::Null),
            params.get("preferences").unwrap_or(&Value::Null),
        )
        .await
        .map_err(anyhow::Error::from),
        "git/repositories/list" => list_git_repositories_payload(state, false)
            .await
            .map_err(anyhow::Error::from),
        "git/status" => get_git_status_payload(state, &require_string(&params, "repoPath")?)
            .await
            .map_err(anyhow::Error::from),
        "git/github/pulls" => list_github_pull_requests_payload(
            state,
            &require_string(&params, "repoPath")?,
            params
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or("open"),
            params.get("limit").and_then(Value::as_u64).unwrap_or(20),
        )
        .await
        .map_err(anyhow::Error::from),
        "git/github/pull" => {
            let number = params
                .get("number")
                .and_then(Value::as_u64)
                .ok_or_else(|| anyhow!("number is required"))?;
            get_github_pull_request_payload(state, &require_string(&params, "repoPath")?, number)
                .await
                .map_err(anyhow::Error::from)
        }
        "git/github/pull/checkout" => {
            let number = params
                .get("number")
                .and_then(Value::as_u64)
                .ok_or_else(|| anyhow!("number is required"))?;
            checkout_github_pull_request_payload(
                state,
                &require_string(&params, "repoPath")?,
                number,
            )
            .await
            .map_err(anyhow::Error::from)
        }
        "git/worktrees/list" => {
            list_git_worktrees_payload(state, &require_string(&params, "repoPath")?)
                .await
                .map_err(anyhow::Error::from)
        }
        "git/worktrees/create" => create_git_worktree_payload(
            state,
            &require_string(&params, "repoPath")?,
            &require_string(&params, "worktreePath")?,
            params.get("branchName").and_then(Value::as_str),
            params
                .get("createBranch")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            params
                .get("detach")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        )
        .await
        .map_err(anyhow::Error::from),
        "git/worktrees/remove" => remove_git_worktree_payload(
            state,
            &require_string(&params, "repoPath")?,
            &require_string(&params, "worktreePath")?,
            params
                .get("force")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        )
        .await
        .map_err(anyhow::Error::from),
        "git/file/get" => get_git_file_payload(
            state,
            &require_string(&params, "repoPath")?,
            &require_string(&params, "filePath")?,
        )
        .await
        .map_err(anyhow::Error::from),
        "git/file/resolve" => resolve_git_file_from_absolute_path_payload(
            state,
            &require_string(&params, "filePath")?,
        )
        .await
        .map_err(anyhow::Error::from),
        "git/file/save" => save_git_file_payload(
            state,
            &require_string(&params, "repoPath")?,
            &require_string(&params, "filePath")?,
            params
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )
        .await
        .map_err(anyhow::Error::from),
        "git/stage" => stage_git_changes_payload(
            state,
            &require_string(&params, "repoPath")?,
            params.get("filePath").and_then(Value::as_str),
        )
        .await
        .map_err(anyhow::Error::from),
        "git/unstage" => unstage_git_changes_payload(
            state,
            &require_string(&params, "repoPath")?,
            params.get("filePath").and_then(Value::as_str),
        )
        .await
        .map_err(anyhow::Error::from),
        "git/fetch" => fetch_git_repository_payload(state, &require_string(&params, "repoPath")?)
            .await
            .map_err(anyhow::Error::from),
        "git/pull" => pull_git_repository_payload(state, &require_string(&params, "repoPath")?)
            .await
            .map_err(anyhow::Error::from),
        "git/commit" => commit_git_changes_payload(
            state,
            &require_string(&params, "repoPath")?,
            &require_string(&params, "message")?,
        )
        .await
        .map_err(anyhow::Error::from),
        "git/commit/diff" => get_git_commit_diff_payload(
            state,
            &require_string(&params, "repoPath")?,
            &require_string(&params, "commitHash")?,
        )
        .await
        .map_err(anyhow::Error::from),
        "git/checkout" => checkout_git_branch_payload(
            state,
            &require_string(&params, "repoPath")?,
            &require_string(&params, "branchName")?,
            params
                .get("create")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        )
        .await
        .map_err(anyhow::Error::from),
        "terminal/list" => list_terminals(state).await,
        "terminal/create" => {
            let cwd = params
                .get("cwd")
                .and_then(Value::as_str)
                .map(str::to_string);
            let title = params
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_string);
            create_terminal(state.clone(), cwd, title).await
        }
        "terminal/read" => {
            let terminal_id = require_string(&params, "terminalId")?;
            read_terminal(state, &terminal_id).await
        }
        "terminal/context/attach" => {
            let session_id = require_session_id(&params, "sessionId")?;
            let terminal_id = require_string(&params, "terminalId")?;
            let max_bytes = params
                .get("maxBytes")
                .and_then(Value::as_u64)
                .map(|value| value.clamp(2_048, 128_000) as usize)
                .unwrap_or(24_000);
            let session = get_terminal_session(state, &terminal_id).await?;
            let (summary, snapshot) = session.snapshot().await;

            let mut cleaned = String::with_capacity(snapshot.len());
            let mut chars = snapshot.chars().peekable();
            while let Some(ch) = chars.next() {
                if ch == '\u{1b}' {
                    if matches!(
                        chars.peek(),
                        Some('[' | ']' | '(' | ')' | '#' | 'P' | '_' | '^')
                    ) {
                        while let Some(next) = chars.next() {
                            if ('@'..='~').contains(&next) {
                                break;
                            }
                        }
                    }
                    continue;
                }

                if ch != '\r' {
                    cleaned.push(ch);
                }
            }

            let trimmed = cleaned.trim();
            if trimmed.is_empty() {
                anyhow::bail!("terminal has no output to attach yet.");
            }

            let excerpt = if trimmed.len() > max_bytes {
                let start = trimmed
                    .char_indices()
                    .nth(trimmed.chars().count().saturating_sub(max_bytes))
                    .map(|(index, _)| index)
                    .unwrap_or(0);
                trimmed[start..].to_string()
            } else {
                trimmed.to_string()
            };

            let terminal_slug = {
                let value = sanitize_profile_id(&summary.title);
                if value.is_empty() {
                    sanitize_profile_id(&terminal_id)
                } else {
                    value
                }
            };
            let captured_at = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let content = format!(
                "# Terminal context\n\nTerminal: {}\nWorking directory: {}\nStatus: {}{}\nCaptured at: {}\n\n```text\n{}\n```\n",
                summary.title,
                summary.cwd,
                summary.status,
                summary
                    .exit_code
                    .map(|exit_code| format!(" (exit {})", exit_code))
                    .unwrap_or_default(),
                captured_at,
                excerpt
            );
            let upload = UploadFilePayload {
                name: format!("terminal-{}-{}.md", terminal_slug, captured_at),
                mime_type: Some("text/markdown".to_string()),
                data_base64: base64::engine::general_purpose::STANDARD.encode(content.as_bytes()),
            };
            let uploaded = upload_attachments(state, &auth.profile_id, &session_id, vec![upload])
                .await
                .map_err(anyhow::Error::from)?;
            Ok(json!({
                "terminal": summary,
                "attachments": uploaded.get("attachments").cloned().unwrap_or_else(|| json!([])),
                "excerpt": excerpt
            }))
        }
        "terminal/input" => {
            let terminal_id = require_string(&params, "terminalId")?;
            let data = require_string(&params, "data")?;
            write_terminal_input(state, &terminal_id, &data).await
        }
        "terminal/close" => {
            let terminal_id = require_string(&params, "terminalId")?;
            close_terminal(state.clone(), &terminal_id).await
        }
        "session/subscribe" => {
            let session_id = require_session_id(&params, "sessionId")?;
            subscribe_session(
                state.clone(),
                out_tx.clone(),
                subscriptions.clone(),
                auth.profile_id.clone(),
                session_id.clone(),
            )
            .await?;
            Ok(json!({ "subscribed": true, "sessionId": session_id }))
        }
        "session/unsubscribe" => {
            let session_id = require_session_id(&params, "sessionId")?;
            let mut current = subscriptions.lock().await;
            if let Some(handle) = current.remove(&session_relay_key(&auth.profile_id, &session_id))
            {
                handle.abort();
            }
            Ok(json!({ "subscribed": false, "sessionId": session_id }))
        }
        "terminal/subscribe" => {
            let terminal_id = require_string(&params, "terminalId")?;
            subscribe_terminal(
                state.clone(),
                out_tx.clone(),
                subscriptions.clone(),
                terminal_id.clone(),
            )
            .await?;
            Ok(json!({ "subscribed": true, "terminalId": terminal_id }))
        }
        "terminal/unsubscribe" => {
            let terminal_id = require_string(&params, "terminalId")?;
            let mut current = subscriptions.lock().await;
            if let Some(handle) = current.remove(&format!("{TERMINAL_RELAY_PREFIX}{terminal_id}")) {
                handle.abort();
            }
            Ok(json!({ "subscribed": false, "terminalId": terminal_id }))
        }
        "events/subscribe" => {
            subscribe_global(
                state.clone(),
                out_tx.clone(),
                subscriptions.clone(),
                auth.profile_id.clone(),
            )
            .await?;
            Ok(json!({ "subscribed": true, "scope": "global" }))
        }
        "events/unsubscribe" => {
            let mut current = subscriptions.lock().await;
            if let Some(handle) = current.remove(&global_relay_key(&auth.profile_id)) {
                handle.abort();
            }
            Ok(json!({ "subscribed": false, "scope": "global" }))
        }
        _ => Err(anyhow!("Unknown websocket method: {method}")),
    }
}
