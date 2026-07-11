use super::*;

fn normalize_session_folder_name(value: Option<&Value>) -> ApiResult<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "Folder name is required."))
}

pub(crate) async fn update_session_organization_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    patch: Value,
) -> ApiResult<Value> {
    let payload = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(session_meta_by_thread_id) = ui_state
            .get_mut("sessionMetaByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "session metadata state is missing",
            ));
        };

        let current = session_meta_by_thread_id
            .get(session_id)
            .cloned()
            .unwrap_or_else(|| json!({ "pinned": false, "tags": [] }));
        let pinned = patch
            .get("pinned")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| {
                current
                    .get("pinned")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            });
        let mut tags = patch
            .get("tags")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| {
                current
                    .get("tags")
                    .and_then(Value::as_array)
                    .map(|entries| {
                        entries
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            });
        tags.sort();
        tags.dedup();

        let mut meta_object = current.as_object().cloned().unwrap_or_default();
        meta_object.insert("pinned".to_string(), json!(pinned));
        meta_object.insert("tags".to_string(), json!(tags));
        let has_title = meta_object
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty());
        let has_tags = meta_object
            .get("tags")
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty());
        if !pinned && !has_tags && !has_title {
            session_meta_by_thread_id.remove(session_id);
        } else {
            let meta = Value::Object(meta_object);
            session_meta_by_thread_id.insert(session_id.to_string(), meta.clone());
        }

        let meta = session_meta_by_thread_id
            .get(session_id)
            .cloned()
            .unwrap_or_else(|| json!({ "pinned": false, "tags": [] }));
        Ok(json!({
            "meta": meta,
            "knownTags": known_tags_from_ui_state(ui_state),
            "sessionFolders": session_folders_from_ui_state(ui_state)
        }))
    })
    .await?;

    emit_profile_config_updated(
        state,
        profile_id,
        json!({
            "sessionOrganization": {
                "knownTags": payload.get("knownTags").cloned().unwrap_or_else(|| json!([])),
                "sessionFolders": payload
                    .get("sessionFolders")
                    .cloned()
                    .unwrap_or_else(|| json!([]))
            }
        }),
    )
    .await;
    emit_session_summary_updated(state, profile_id, session_id, None, None).await;

    Ok(payload)
}

pub(crate) async fn move_session_profile_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    params: Value,
) -> ApiResult<Value> {
    let target_profile_id = params
        .get("targetProfileId")
        .or_else(|| params.get("target_profile_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(sanitize_profile_id)
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "Target profile is required."))?;
    let requested_source_profile_id = params
        .get("sourceProfileId")
        .or_else(|| params.get("source_profile_id"))
        .and_then(Value::as_str)
        .map(sanitize_profile_id)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| profile_id.to_string());
    let (_, profiles_snapshot) = runtime_profiles_snapshot(&state.config);
    let Some(source_profile) = profiles_snapshot.get(&requested_source_profile_id).cloned() else {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Source profile is not configured.",
        ));
    };
    let mut source_profile_id = requested_source_profile_id;
    let mut source_profile = source_profile;
    let Some(target_profile) = profiles_snapshot.get(&target_profile_id).cloned() else {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Target profile is not configured.",
        ));
    };

    let mut source_rollout_path = None;
    let mut source_archived = false;
    if source_profile_id == target_profile_id {
        'profiles: for (candidate_profile_id, candidate_profile) in &profiles_snapshot {
            if candidate_profile_id == &target_profile_id {
                continue;
            }
            for archived in [false, true] {
                let candidates =
                    list_rollout_candidates_payload(state, candidate_profile_id, archived).await?;
                for candidate in candidates {
                    if candidate.get("id").and_then(Value::as_str) != Some(session_id) {
                        continue;
                    }
                    if let Some(path) = candidate
                        .get("path")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                    {
                        source_profile_id = candidate_profile_id.clone();
                        source_profile = candidate_profile.clone();
                        source_rollout_path = Some(PathBuf::from(path));
                        source_archived = archived;
                        break 'profiles;
                    }
                }
            }
        }
        if source_profile_id == target_profile_id {
            return Err(api_error(
                StatusCode::CONFLICT,
                "The session is already in the target profile.",
            ));
        }
    }

    let runtime_key = runtime_session_key(&source_profile_id, session_id);
    if state.active_turns.lock().await.contains_key(&runtime_key)
        || state
            .pending_turn_starts
            .lock()
            .await
            .contains(&runtime_key)
        || state.queue_dispatching.lock().await.contains(&runtime_key)
    {
        return Err(api_error(
            StatusCode::CONFLICT,
            "Stop the running session before moving it to another account.",
        ));
    }

    for archived in [false, true] {
        if source_rollout_path.is_some() {
            break;
        }
        let candidates =
            list_rollout_candidates_payload(state, &source_profile_id, archived).await?;
        for candidate in candidates {
            if candidate.get("id").and_then(Value::as_str) != Some(session_id) {
                continue;
            }
            if let Some(path) = candidate
                .get("path")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                source_rollout_path = Some(PathBuf::from(path));
                source_archived = archived;
                break;
            }
        }
        if source_rollout_path.is_some() {
            break;
        }
    }
    let Some(source_rollout_path) = source_rollout_path else {
        return Err(api_error(
            StatusCode::NOT_FOUND,
            "Session rollout was not found.",
        ));
    };

    for archived in [false, true] {
        let candidates =
            list_rollout_candidates_payload(state, &target_profile_id, archived).await?;
        if candidates
            .iter()
            .any(|candidate| candidate.get("id").and_then(Value::as_str) == Some(session_id))
        {
            return Err(api_error(
                StatusCode::CONFLICT,
                "A session with the same id already exists in the target profile.",
            ));
        }
    }

    let source_root = if source_archived {
        source_profile.codex_home.join("archived_sessions")
    } else {
        source_profile.codex_home.join("sessions")
    };
    let target_root = if source_archived {
        target_profile.codex_home.join("archived_sessions")
    } else {
        target_profile.codex_home.join("sessions")
    };
    let relative_rollout_path = source_rollout_path
        .strip_prefix(&source_root)
        .map_err(|_| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Session rollout path escaped the source profile root.",
            )
        })?;
    let target_rollout_path = target_root.join(relative_rollout_path);
    if target_rollout_path.exists() {
        return Err(api_error(
            StatusCode::CONFLICT,
            "Target rollout path already exists.",
        ));
    }
    let source_uploads_dir = session_uploads_dir(state, &source_profile_id, session_id);
    let target_uploads_dir = session_uploads_dir(state, &target_profile_id, session_id);
    let source_uploads_exist = tokio_fs::metadata(&source_uploads_dir).await.is_ok();
    if source_uploads_exist && tokio_fs::metadata(&target_uploads_dir).await.is_ok() {
        return Err(api_error(
            StatusCode::CONFLICT,
            "Target profile already has uploads for this session.",
        ));
    }
    if let Some(parent) = target_rollout_path.parent() {
        tokio_fs::create_dir_all(parent)
            .await
            .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    }
    if let Err(rename_error) = tokio_fs::rename(&source_rollout_path, &target_rollout_path).await {
        tokio_fs::copy(&source_rollout_path, &target_rollout_path)
            .await
            .map_err(|copy_error| {
                api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!(
                        "Failed to move session rollout: {rename_error}; copy fallback failed: {copy_error}"
                    ),
                )
            })?;
        tokio_fs::remove_file(&source_rollout_path)
            .await
            .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    }

    let source_index_path = source_profile.codex_home.join("session_index.jsonl");
    if let Ok(raw_index) = tokio_fs::read_to_string(&source_index_path).await {
        let matching_index_lines = raw_index
            .lines()
            .filter(|line| {
                serde_json::from_str::<Value>(line)
                    .ok()
                    .and_then(|entry| entry.get("id").and_then(Value::as_str).map(str::to_string))
                    .is_some_and(|id| id == session_id)
            })
            .map(str::to_string)
            .collect::<Vec<_>>();
        if !matching_index_lines.is_empty() {
            let target_index_path = target_profile.codex_home.join("session_index.jsonl");
            let target_index = tokio_fs::read_to_string(&target_index_path)
                .await
                .unwrap_or_default();
            let mut target_lines = target_index
                .lines()
                .filter(|line| {
                    !serde_json::from_str::<Value>(line)
                        .ok()
                        .and_then(|entry| {
                            entry.get("id").and_then(Value::as_str).map(str::to_string)
                        })
                        .is_some_and(|id| id == session_id)
                })
                .map(str::to_string)
                .collect::<Vec<_>>();
            target_lines.extend(matching_index_lines);
            let target_index_bytes = format!("{}\n", target_lines.join("\n")).into_bytes();
            write_file_atomically(&target_index_path, target_index_bytes)
                .await
                .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;

            let retained_source_lines = raw_index
                .lines()
                .filter(|line| {
                    !serde_json::from_str::<Value>(line)
                        .ok()
                        .and_then(|entry| {
                            entry.get("id").and_then(Value::as_str).map(str::to_string)
                        })
                        .is_some_and(|id| id == session_id)
                })
                .collect::<Vec<_>>();
            let source_index_bytes = if retained_source_lines.is_empty() {
                Vec::new()
            } else {
                format!("{}\n", retained_source_lines.join("\n")).into_bytes()
            };
            write_file_atomically(&source_index_path, source_index_bytes)
                .await
                .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
        }
    }

    if source_uploads_exist {
        if let Some(parent) = target_uploads_dir.parent() {
            tokio_fs::create_dir_all(parent)
                .await
                .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
        }
        if let Err(rename_error) = tokio_fs::rename(&source_uploads_dir, &target_uploads_dir).await
        {
            let source = source_uploads_dir.clone();
            let target = target_uploads_dir.clone();
            tokio::task::spawn_blocking(move || {
                let mut pending = vec![(source.clone(), target.clone())];
                while let Some((from, to)) = pending.pop() {
                    std::fs::create_dir_all(&to)?;
                    for entry in std::fs::read_dir(&from)? {
                        let entry = entry?;
                        let from_path = entry.path();
                        let to_path = to.join(entry.file_name());
                        let file_type = entry.file_type()?;
                        if file_type.is_dir() {
                            pending.push((from_path, to_path));
                        } else if file_type.is_file() {
                            std::fs::copy(&from_path, &to_path)?;
                        }
                    }
                }
                std::fs::remove_dir_all(source)?;
                std::io::Result::Ok(())
            })
            .await
            .map_err(|error| {
                api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to move session uploads: {rename_error}; fallback join failed: {error}"),
                )
            })?
            .map_err(|error| {
                api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to move session uploads: {rename_error}; fallback failed: {error}"),
                )
            })?;
        }
    }

    let ui_state_sections = [
        "sessionMetaByThreadId",
        "preferencesByThreadId",
        "skillsByThreadId",
        "draftsByThreadId",
        "queuesByThreadId",
        "goalsByThreadId",
        "highlightsByThreadId",
        "languageBridgeByThreadId",
    ];
    let moved_ui_entries = with_ui_state_read(state, &source_profile_id, |ui_state| {
        let mut entries = Vec::new();
        for section in ui_state_sections {
            if let Some(value) = ui_state
                .get(section)
                .and_then(Value::as_object)
                .and_then(|items| items.get(session_id))
                .cloned()
            {
                entries.push((section.to_string(), value));
            }
        }
        Ok(entries)
    })
    .await?;
    let moved_folder_entries = with_ui_state_read(state, &source_profile_id, |ui_state| {
        let moved_tag_names = moved_ui_entries
            .iter()
            .find(|(section, _)| section == "sessionMetaByThreadId")
            .and_then(|(_, value)| value.get("tags"))
            .and_then(Value::as_array)
            .map(|tags| {
                tags.iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_default();
        let mut entries = Vec::new();
        if let Some(folders_by_name) = ui_state
            .get("sessionFoldersByName")
            .and_then(Value::as_object)
        {
            for folder_name in moved_tag_names {
                if let Some(value) = folders_by_name.get(&folder_name).cloned() {
                    entries.push((folder_name, value));
                }
            }
        }
        Ok(entries)
    })
    .await?;
    with_ui_state_write(state, &target_profile_id, |ui_state| {
        for (section, value) in &moved_ui_entries {
            let Some(section_object) = ui_state.get_mut(section).and_then(Value::as_object_mut)
            else {
                return Err(api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("{section} state is missing"),
                ));
            };
            section_object.insert(session_id.to_string(), value.clone());
        }
        if !moved_folder_entries.is_empty() {
            let Some(folders_by_name) = ui_state
                .get_mut("sessionFoldersByName")
                .and_then(Value::as_object_mut)
            else {
                return Err(api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "session folder state is missing",
                ));
            };
            for (folder_name, value) in &moved_folder_entries {
                folders_by_name
                    .entry(folder_name.clone())
                    .or_insert_with(|| value.clone());
            }
        }
        Ok(())
    })
    .await?;
    with_ui_state_write(state, &source_profile_id, |ui_state| {
        for section in ui_state_sections {
            if let Some(section_object) = ui_state.get_mut(section).and_then(Value::as_object_mut) {
                section_object.remove(session_id);
            }
        }
        if let Some(runtime_statuses) = ui_state
            .get_mut("runtimeStatusByThreadId")
            .and_then(Value::as_object_mut)
        {
            runtime_statuses.remove(session_id);
        }
        Ok(())
    })
    .await?;

    clear_app_server_assignments_for_sessions(state, &source_profile_id, &[session_id.to_string()])
        .await;
    invalidate_session_lists(state, &source_profile_id).await;
    invalidate_session_lists(state, &target_profile_id).await;
    emit_profile_global_notification(
        state,
        &source_profile_id,
        json!({
            "kind": "notification",
            "method": "codex-webui/sessionListsInvalidated",
            "params": { "reason": "sessionProfileMoved", "sessionId": session_id }
        }),
    )
    .await;
    emit_profile_global_notification(
        state,
        &target_profile_id,
        json!({
            "kind": "notification",
            "method": "codex-webui/sessionListsInvalidated",
            "params": { "reason": "sessionProfileMoved", "sessionId": session_id }
        }),
    )
    .await;

    let session = build_session_summary_payload(state, &target_profile_id, session_id, None, None)
        .await
        .ok();
    Ok(json!({
        "ok": true,
        "sessionId": session_id,
        "sourceProfileId": source_profile_id,
        "targetProfileId": target_profile_id,
        "targetProfileLabel": target_profile.label,
        "archived": source_archived,
        "session": session
    }))
}

pub(crate) async fn upsert_session_folder_payload(
    state: &AppState,
    profile_id: &str,
    params: Value,
) -> ApiResult<Value> {
    let name = normalize_session_folder_name(params.get("name"))?;
    let pinned_patch = params.get("pinned").and_then(Value::as_bool);
    let payload = with_ui_state_write(state, profile_id, |ui_state| {
        let now = now_unix_ms();
        let Some(folders_by_name) = ui_state
            .get_mut("sessionFoldersByName")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "session folders state is missing",
            ));
        };
        let current = folders_by_name
            .get(&name)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let pinned = pinned_patch.unwrap_or_else(|| {
            current
                .get("pinned")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        });
        let created_at = current
            .get("createdAt")
            .and_then(Value::as_u64)
            .unwrap_or(now);
        let folder = json!({
            "name": name,
            "pinned": pinned,
            "createdAt": created_at,
            "updatedAt": now
        });
        folders_by_name.insert(name.clone(), folder.clone());
        Ok(json!({
            "folder": folder,
            "knownTags": known_tags_from_ui_state(ui_state),
            "sessionFolders": session_folders_from_ui_state(ui_state)
        }))
    })
    .await?;

    emit_profile_config_updated(
        state,
        profile_id,
        json!({
            "sessionOrganization": {
                "knownTags": payload.get("knownTags").cloned().unwrap_or_else(|| json!([])),
                "sessionFolders": payload
                    .get("sessionFolders")
                    .cloned()
                    .unwrap_or_else(|| json!([]))
            }
        }),
    )
    .await;

    Ok(payload)
}

pub(crate) async fn delete_session_folder_payload(
    state: &AppState,
    profile_id: &str,
    params: Value,
) -> ApiResult<Value> {
    let name = normalize_session_folder_name(params.get("name"))?;
    let remove_from_sessions = params
        .get("removeFromSessions")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let payload = with_ui_state_write(state, profile_id, |ui_state| {
        if let Some(folders_by_name) = ui_state
            .get_mut("sessionFoldersByName")
            .and_then(Value::as_object_mut)
        {
            folders_by_name.remove(&name);
        }
        if remove_from_sessions {
            if let Some(session_meta_by_thread_id) = ui_state
                .get_mut("sessionMetaByThreadId")
                .and_then(Value::as_object_mut)
            {
                let mut empty_entries = Vec::new();
                for (session_id, meta) in session_meta_by_thread_id.iter_mut() {
                    if let Some(meta_object) = meta.as_object_mut() {
                        let mut next_tags = meta_object
                            .get("tags")
                            .and_then(Value::as_array)
                            .map(|tags| {
                                tags.iter()
                                    .filter_map(Value::as_str)
                                    .filter(|tag| tag.trim() != name)
                                    .map(str::to_string)
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();
                        next_tags.sort();
                        next_tags.dedup();
                        meta_object.insert("tags".to_string(), json!(next_tags));
                        let pinned = meta_object
                            .get("pinned")
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        let has_tags = meta_object
                            .get("tags")
                            .and_then(Value::as_array)
                            .is_some_and(|tags| !tags.is_empty());
                        let has_title = meta_object
                            .get("name")
                            .and_then(Value::as_str)
                            .is_some_and(|title| !title.trim().is_empty());
                        if !pinned && !has_tags && !has_title {
                            empty_entries.push(session_id.clone());
                        }
                    }
                }
                for session_id in empty_entries {
                    session_meta_by_thread_id.remove(&session_id);
                }
            }
        }
        Ok(json!({
            "removed": name,
            "knownTags": known_tags_from_ui_state(ui_state),
            "sessionFolders": session_folders_from_ui_state(ui_state)
        }))
    })
    .await?;

    emit_profile_config_updated(
        state,
        profile_id,
        json!({
            "sessionOrganization": {
                "knownTags": payload.get("knownTags").cloned().unwrap_or_else(|| json!([])),
                "sessionFolders": payload
                    .get("sessionFolders")
                    .cloned()
                    .unwrap_or_else(|| json!([]))
            }
        }),
    )
    .await;
    if remove_from_sessions {
        emit_profile_global_notification(
            state,
            profile_id,
            json!({
                "kind": "notification",
                "method": "codex-webui/sessionListsInvalidated",
                "params": { "reason": "folderDeleted" }
            }),
        )
        .await;
    }

    Ok(payload)
}

pub(crate) async fn save_session_title_metadata(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    name: Option<&str>,
) -> ApiResult<()> {
    let next_name = name
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "New thread")
        .map(str::to_string);

    with_ui_state_write(state, profile_id, |ui_state| {
        let Some(session_meta_by_thread_id) = ui_state
            .get_mut("sessionMetaByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "session metadata state is missing",
            ));
        };

        let current = session_meta_by_thread_id
            .get(session_id)
            .cloned()
            .unwrap_or_else(|| json!({ "pinned": false, "tags": [] }));
        let mut meta_object = current.as_object().cloned().unwrap_or_default();
        if let Some(next_name) = &next_name {
            meta_object.insert("name".to_string(), Value::String(next_name.clone()));
            meta_object.insert("nameUpdatedAt".to_string(), json!(now_unix_ms()));
        } else {
            meta_object.remove("name");
            meta_object.remove("nameUpdatedAt");
        }

        let pinned = meta_object
            .get("pinned")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let has_tags = meta_object
            .get("tags")
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty());
        let has_title = meta_object
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty());

        if !pinned && !has_tags && !has_title {
            session_meta_by_thread_id.remove(session_id);
        } else {
            meta_object
                .entry("pinned".to_string())
                .or_insert_with(|| json!(false));
            meta_object
                .entry("tags".to_string())
                .or_insert_with(|| json!([]));
            session_meta_by_thread_id.insert(session_id.to_string(), Value::Object(meta_object));
        }
        Ok(())
    })
    .await
}

pub(crate) async fn save_session_preferences_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    preferences: Value,
) -> ApiResult<Value> {
    let next_preferences =
        normalize_session_preferences_payload(state, profile_id, preferences).await?;
    with_ui_state_write(state, profile_id, |ui_state| {
        let Some(preferences_by_thread_id) = ui_state
            .get_mut("preferencesByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "preferences state is missing",
            ));
        };
        preferences_by_thread_id.insert(session_id.to_string(), next_preferences.clone());
        Ok(())
    })
    .await?;
    sync_codex_toml_with_preferences(
        &resolve_runtime_profile(&state.config, profile_id).codex_home,
        &next_preferences,
    )
    .await
    .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;

    emit_session_notification(
        state,
        profile_id,
        session_id,
        json!({
            "kind": "notification",
            "method": "codex-webui/preferencesUpdated",
            "params": {
                "preferences": next_preferences.clone()
            }
        }),
    )
    .await;
    emit_profile_config_updated(
        state,
        profile_id,
        json!({
            "defaults": next_preferences.clone()
        }),
    )
    .await;
    emit_session_summary_updated(
        state,
        profile_id,
        session_id,
        Some(next_preferences.clone()),
        None,
    )
    .await;

    Ok(next_preferences)
}

pub(crate) async fn save_session_skills_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    skills: Option<&Value>,
) -> ApiResult<Value> {
    let next_skills = selected_skills_from_value(skills);
    with_ui_state_write(state, profile_id, |ui_state| {
        let Some(skills_by_thread_id) = ui_state
            .get_mut("skillsByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "skills state is missing",
            ));
        };
        skills_by_thread_id.insert(session_id.to_string(), Value::Array(next_skills.clone()));
        Ok(())
    })
    .await?;

    emit_session_notification(
        state,
        profile_id,
        session_id,
        json!({
            "kind": "notification",
            "method": "codex-webui/skillsUpdated",
            "params": {
                "selectedSkills": next_skills.clone()
            }
        }),
    )
    .await;

    Ok(Value::Array(next_skills))
}

pub(crate) async fn rename_session_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    name: &str,
) -> ApiResult<Value> {
    let trimmed_name = name.trim();
    if trimmed_name.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Session name is required.",
        ));
    }

    app_server_client_for_session(state, profile_id, session_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to codex app-server: {error}"),
            )
        })?
        .request(
            "thread/name/set",
            json!({
                "threadId": session_id,
                "name": trimmed_name
            }),
        )
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to rename the session: {error}"),
            )
        })?;

    save_session_title_metadata(state, profile_id, session_id, Some(trimmed_name)).await?;
    emit_session_notification(
        state,
        profile_id,
        session_id,
        json!({
            "kind": "notification",
            "method": "thread/name/updated",
            "params": {
                "threadId": session_id,
                "threadName": trimmed_name
            }
        }),
    )
    .await;
    emit_session_summary_updated(state, profile_id, session_id, None, None).await;

    Ok(json!({
        "ok": true,
        "name": trimmed_name
    }))
}

pub(crate) async fn archive_session_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<Value> {
    app_server_client_for_session(state, profile_id, session_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to codex app-server: {error}"),
            )
        })?
        .request(
            "thread/archive",
            json!({
                "threadId": session_id
            }),
        )
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to archive the session: {error}"),
            )
        })?;

    invalidate_session_lists(state, profile_id).await;
    Ok(json!({ "ok": true }))
}

pub(crate) async fn unarchive_session_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<Value> {
    app_server_client_for_session(state, profile_id, session_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to codex app-server: {error}"),
            )
        })?
        .request(
            "thread/unarchive",
            json!({
                "threadId": session_id
            }),
        )
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to unarchive the session: {error}"),
            )
        })?;

    invalidate_session_lists(state, profile_id).await;
    let session = build_session_summary_payload(state, profile_id, session_id, None, None).await?;
    Ok(json!({
        "ok": true,
        "session": session
    }))
}
