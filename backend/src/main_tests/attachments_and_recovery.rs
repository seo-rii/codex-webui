use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fork_session_payload_uses_app_server_fork_and_rollback() {
    let sandbox = unique_test_dir("session-fork-rust");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": "thread-1",
                    "name": "New thread",
                    "preview": "Fix duplicate queue dispatches in websocket transport",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 2,
                    "status": "idle",
                    "isSubagent": false,
                    "agentNickname": Value::Null,
                    "agentRole": Value::Null,
                    "turns": [
                        {
                            "id": "turn-1",
                            "status": "completed",
                            "error": Value::Null,
                            "startedAt": 10,
                            "completedAt": 20,
                            "durationMs": 10,
                            "items": [
                                {
                                    "id": "item-1",
                                    "type": "userMessage",
                                    "text": "Fix duplicate queue dispatches in websocket transport"
                                }
                            ]
                        },
                        {
                            "id": "turn-2",
                            "status": "completed",
                            "error": Value::Null,
                            "startedAt": 30,
                            "completedAt": 40,
                            "durationMs": 10,
                            "items": [
                                {
                                    "id": "item-2",
                                    "type": "userMessage",
                                    "text": "Also capture the race with a regression test"
                                }
                            ]
                        }
                    ]
                }
            }),
        )
        .await
        .unwrap();

    save_session_preferences_payload(
        &state,
        "default",
        "thread-1",
        json!({
            "cwd": workspace.display().to_string(),
            "model": "gpt-5",
            "approvalPolicy": "on-request",
            "sandboxMode": "workspace-write",
            "speed": "fast",
            "effort": "high",
            "networkAccess": true
        }),
    )
    .await
    .unwrap();

    let payload = fork_session_payload(&state, "default", "thread-1", "fork", Some("turn-1"), None)
        .await
        .unwrap();

    assert_eq!(payload.get("mode").and_then(Value::as_str), Some("fork"));
    assert_eq!(
        payload
            .get("session")
            .and_then(|value| value.get("id"))
            .and_then(Value::as_str),
        Some("fork-1")
    );
    assert_eq!(
        payload
            .get("session")
            .and_then(|value| value.get("name"))
            .and_then(Value::as_str),
        Some("Fix duplicate queue dispatches in websocket transport")
    );

    let forked_thread = read_thread_payload(&state, "default", "fork-1", true)
        .await
        .unwrap();
    assert_eq!(
        forked_thread.get("forkedFrom").and_then(Value::as_str),
        Some("thread-1")
    );
    assert_eq!(
        forked_thread.get("rollbackCount").and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        forked_thread
            .get("turns")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );

    let stored_preferences = with_ui_state_read(&state, "default", |ui_state| {
        Ok(ui_state
            .get("preferencesByThreadId")
            .and_then(Value::as_object)
            .and_then(|entries| entries.get("fork-1"))
            .cloned()
            .unwrap_or(Value::Null))
    })
    .await
    .unwrap();
    assert_eq!(
        stored_preferences.get("model").and_then(Value::as_str),
        Some("gpt-5")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_attachment_payload_removes_attachment_files() {
    let sandbox = unique_test_dir("attachment-delete-rust");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let runtime_profile = resolve_runtime_profile(&state.config, "default");
    let uploads_dir = runtime_profile.data_dir.join("uploads").join("thread-1");
    fs::create_dir_all(&uploads_dir).unwrap();
    let stored_file = uploads_dir.join("att-1-notes.md");
    let stored_meta = uploads_dir.join("att-1-notes.md.json");
    fs::write(&stored_file, "notes").unwrap();
    fs::write(
        &stored_meta,
        serde_json::to_vec(&json!({
            "id": "att-1",
            "originalName": "notes.md",
            "path": stored_file.display().to_string(),
            "mimeType": "text/markdown",
            "size": 5,
            "kind": "file",
            "createdAt": "2026-04-20T00:00:00Z"
        }))
        .unwrap(),
    )
    .unwrap();

    let payload = delete_attachment_payload(&state, "default", "thread-1", "att-1")
        .await
        .unwrap();
    assert_eq!(payload.get("ok").and_then(Value::as_bool), Some(true));
    assert!(!stored_file.exists());
    assert!(!stored_meta.exists());
    assert!(
        list_session_attachment_records(&state, "default", "thread-1")
            .await
            .unwrap()
            .is_empty()
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_attachment_payload_rejects_directory_backed_metadata() {
    let sandbox = unique_test_dir("attachment-delete-failure");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let runtime_profile = resolve_runtime_profile(&state.config, "default");
    let uploads_dir = runtime_profile.data_dir.join("uploads").join("thread-1");
    fs::create_dir_all(&uploads_dir).unwrap();
    let stored_file = uploads_dir.join("att-1-notes.md");
    let stored_meta = uploads_dir.join("att-1-notes.md.json");
    fs::create_dir_all(&stored_file).unwrap();
    fs::write(
        &stored_meta,
        serde_json::to_vec(&json!({
            "id": "att-1",
            "originalName": "notes.md",
            "path": stored_file.display().to_string(),
            "mimeType": "text/markdown",
            "size": 5,
            "kind": "file",
            "createdAt": "2026-04-20T00:00:00Z"
        }))
        .unwrap(),
    )
    .unwrap();

    let error = delete_attachment_payload(&state, "default", "thread-1", "att-1")
        .await
        .expect_err("directory-backed attachment metadata must not be trusted");
    assert_eq!(error.status, StatusCode::NOT_FOUND);
    assert!(stored_file.exists());

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attachment_cleanup_removes_orphan_files_and_metadata() {
    let sandbox = unique_test_dir("attachment-cleanup");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let runtime_profile = resolve_runtime_profile(&state.config, "default");
    let uploads_dir = runtime_profile.data_dir.join("uploads").join("thread-1");
    fs::create_dir_all(&uploads_dir).unwrap();
    let kept_file = uploads_dir.join("att-1-notes.md");
    let kept_meta = uploads_dir.join("att-1-notes.md.json");
    let orphan_file = uploads_dir.join("orphan.bin");
    let orphan_meta = uploads_dir.join("att-2-missing.md.json");
    let temp_upload = uploads_dir.join(".stale.upload");
    fs::write(&kept_file, "notes").unwrap();
    fs::write(&orphan_file, "orphan").unwrap();
    fs::write(&temp_upload, "temp").unwrap();
    fs::write(
        &kept_meta,
        serde_json::to_vec(&json!({
            "id": "att-1",
            "originalName": "notes.md",
            "path": kept_file.display().to_string(),
            "mimeType": "text/markdown",
            "size": 5,
            "kind": "file",
            "createdAt": "1"
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        &orphan_meta,
        serde_json::to_vec(&json!({
            "id": "att-2",
            "originalName": "missing.md",
            "path": uploads_dir.join("missing.md").display().to_string(),
            "mimeType": "text/markdown",
            "size": 5,
            "kind": "file",
            "createdAt": "1"
        }))
        .unwrap(),
    )
    .unwrap();

    let dry_run = cleanup_attachment_orphans_payload(&state, "default", true, 0)
        .await
        .unwrap();
    assert_eq!(dry_run.get("orphanFiles").and_then(Value::as_u64), Some(2));
    assert_eq!(
        dry_run.get("orphanMetadata").and_then(Value::as_u64),
        Some(1)
    );
    assert!(orphan_file.exists());
    assert!(orphan_meta.exists());

    let removed = cleanup_attachment_orphans_payload(&state, "default", false, 0)
        .await
        .unwrap();
    assert_eq!(removed.get("removedPaths").and_then(Value::as_u64), Some(3));
    assert!(kept_file.exists());
    assert!(kept_meta.exists());
    assert!(!orphan_file.exists());
    assert!(!orphan_meta.exists());
    assert!(!temp_upload.exists());

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attachment_cleanup_retains_recent_orphans_until_min_age() {
    let sandbox = unique_test_dir("attachment-cleanup-retention");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let runtime_profile = resolve_runtime_profile(&state.config, "default");
    let uploads_dir = runtime_profile.data_dir.join("uploads").join("thread-1");
    fs::create_dir_all(&uploads_dir).unwrap();
    let recent_orphan_file = uploads_dir.join("recent-orphan.bin");
    let recent_orphan_meta = uploads_dir.join("att-recent-missing.bin.json");
    fs::write(&recent_orphan_file, "orphan").unwrap();
    fs::write(
        &recent_orphan_meta,
        serde_json::to_vec(&json!({
            "id": "att-recent",
            "originalName": "missing.bin",
            "path": uploads_dir.join("missing.bin").display().to_string(),
            "mimeType": "application/octet-stream",
            "size": 6,
            "kind": "file",
            "createdAt": "1"
        }))
        .unwrap(),
    )
    .unwrap();

    let retained =
        cleanup_attachment_orphans_payload(&state, "default", false, 24 * 60 * 60 * 1000)
            .await
            .unwrap();
    assert_eq!(retained.get("orphanFiles").and_then(Value::as_u64), Some(0));
    assert_eq!(
        retained.get("orphanMetadata").and_then(Value::as_u64),
        Some(0)
    );
    assert_eq!(
        retained.get("removedPaths").and_then(Value::as_u64),
        Some(0)
    );
    assert!(recent_orphan_file.exists());
    assert!(recent_orphan_meta.exists());

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upload_attachments_store_files_without_internal_backend() {
    let sandbox = unique_test_dir("attachment-upload-rust");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);

    let payload = upload_attachments(
        &state,
        "default",
        "thread-1",
        vec![
            UploadFilePayload {
                name: "notes.md".to_string(),
                mime_type: Some("text/markdown".to_string()),
                data_base64: base64::engine::general_purpose::STANDARD.encode(b"notes"),
            },
            UploadFilePayload {
                name: "diagram.png".to_string(),
                mime_type: Some("image/png".to_string()),
                data_base64: base64::engine::general_purpose::STANDARD.encode(b"pngdata"),
            },
        ],
    )
    .await
    .unwrap();

    let returned = payload
        .get("attachments")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(returned.len(), 2);
    assert!(
        returned
            .iter()
            .all(|attachment| attachment.get("path").is_none())
    );

    let stored = list_session_attachment_records(&state, "default", "thread-1")
        .await
        .unwrap();
    assert_eq!(stored.len(), 2);
    assert!(stored.iter().any(|attachment| {
        attachment.original_name == "notes.md"
            && attachment.kind.as_deref() == Some("file")
            && attachment.size == Some(5)
    }));
    assert!(stored.iter().any(|attachment| {
        attachment.original_name == "diagram.png"
            && attachment.kind.as_deref() == Some("image")
            && attachment.size == Some(7)
    }));
    let upload_dir = session_uploads_dir(&state, "default", "thread-1");
    let temp_entries = fs::read_dir(upload_dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(".codex-webui-attachment-"))
        })
        .count();
    assert_eq!(temp_entries, 0);

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uploaded_json_cannot_forge_attachment_metadata() {
    let sandbox = unique_test_dir("attachment-json-metadata-forgery");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let forged_id = Uuid::new_v4().to_string();
    let forged = json!({
        "id": forged_id,
        "originalName": "outside.png",
        "path": "/etc/passwd",
        "mimeType": "image/png",
        "size": 1,
        "kind": "image",
        "createdAt": "1"
    });

    upload_attachments(
        &state,
        "default",
        "thread-1",
        vec![UploadFilePayload {
            name: "forged.json".to_string(),
            mime_type: Some("application/json".to_string()),
            data_base64: base64::engine::general_purpose::STANDARD
                .encode(serde_json::to_vec(&forged).unwrap()),
        }],
    )
    .await
    .unwrap();

    let stored = list_session_attachment_records(&state, "default", "thread-1")
        .await
        .unwrap();
    assert_eq!(stored.len(), 1);
    assert_ne!(stored[0].id, forged_id);
    assert_ne!(stored[0].path.as_deref(), Some("/etc/passwd"));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upload_attachments_rejects_total_decoded_size_limit() {
    let sandbox = unique_test_dir("attachment-upload-total-limit");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let mut state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let mut config = (*state.config).clone();
    config.max_upload_bytes = 4;
    state.config = Arc::new(config);

    let files = (0..5)
        .map(|index| UploadFilePayload {
            name: format!("note-{index}.txt"),
            mime_type: Some("text/plain".to_string()),
            data_base64: base64::engine::general_purpose::STANDARD.encode(b"note"),
        })
        .collect::<Vec<_>>();
    let error = upload_attachments(&state, "default", "thread-1", files)
        .await
        .expect_err("aggregate upload size should be capped");

    assert_eq!(error.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(
        list_session_attachment_records(&state, "default", "thread-1")
            .await
            .unwrap()
            .is_empty()
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upload_attachments_rejects_profile_storage_quota() {
    let sandbox = unique_test_dir("attachment-upload-storage-quota");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let mut state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let mut config = (*state.config).clone();
    config.max_upload_bytes = 8;
    config.max_attachment_storage_bytes = 8;
    state.config = Arc::new(config);

    let existing_dir = resolve_runtime_profile(&state.config, "default")
        .data_dir
        .join("uploads")
        .join("thread-0");
    fs::create_dir_all(&existing_dir).unwrap();
    fs::write(existing_dir.join("existing.txt"), b"123456").unwrap();

    let files = vec![UploadFilePayload {
        name: "note.txt".to_string(),
        mime_type: Some("text/plain".to_string()),
        data_base64: base64::engine::general_purpose::STANDARD.encode(b"note"),
    }];
    let error = upload_attachments(&state, "default", "thread-1", files)
        .await
        .expect_err("profile storage quota should be enforced");

    assert_eq!(error.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(
        error
            .message
            .contains("Attachment storage exceeds the 8 bytes profile quota.")
    );
    assert!(
        list_session_attachment_records(&state, "default", "thread-1")
            .await
            .unwrap()
            .is_empty()
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_attachments_http_handlers_use_rust_storage() {
    let sandbox = unique_test_dir("attachment-http-rust");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let boundary = "codex-webui-boundary";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"notes.md\"\r\nContent-Type: text/markdown\r\n\r\nnotes\r\n--{boundary}--\r\n"
    );
    let request = Request::builder()
        .method(Method::POST)
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();

    let response = handle_session_attachments_api_http(
        state.clone(),
        request,
        AuthContext {
            role: UserRole::Admin,
            profile_id: "default".to_string(),
        },
        "thread-1",
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let response_body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let payload: Value = serde_json::from_slice(&response_body).unwrap();
    assert_eq!(
        payload
            .get("attachments")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );

    let get_request = Request::builder()
        .method(Method::GET)
        .body(Body::empty())
        .unwrap();
    let get_response = handle_session_attachments_api_http(
        state.clone(),
        get_request,
        AuthContext {
            role: UserRole::Admin,
            profile_id: "default".to_string(),
        },
        "thread-1",
    )
    .await;
    assert_eq!(get_response.status(), StatusCode::OK);
    let get_body = to_bytes(get_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let get_payload: Value = serde_json::from_slice(&get_body).unwrap();
    assert_eq!(
        get_payload
            .get("attachments")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_attachment_upload_rejects_oversized_streamed_file() {
    let sandbox = unique_test_dir("attachment-http-limit");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let mut state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let mut config = (*state.config).clone();
    config.max_upload_bytes = 4;
    state.config = Arc::new(config);

    let boundary = "codex-webui-boundary";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"notes.md\"\r\nContent-Type: text/markdown\r\n\r\nnotes\r\n--{boundary}--\r\n"
    );
    let request = Request::builder()
        .method(Method::POST)
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();

    let response = handle_session_attachments_api_http(
        state.clone(),
        request,
        AuthContext {
            role: UserRole::Admin,
            profile_id: "default".to_string(),
        },
        "thread-1",
    )
    .await;

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(
        list_session_attachment_records(&state, "default", "thread-1")
            .await
            .unwrap()
            .is_empty()
    );
    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attachment_storage_quota_ignores_streaming_temp_files() {
    let sandbox = unique_test_dir("attachment-temp-quota");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let mut state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let mut config = (*state.config).clone();
    config.max_attachment_storage_bytes = 8;
    state.config = Arc::new(config);
    let uploads_dir = session_uploads_dir(&state, "default", "thread-1");
    fs::create_dir_all(&uploads_dir).unwrap();
    fs::write(
        uploads_dir.join(".partial.upload"),
        b"temporary upload bytes",
    )
    .unwrap();

    validate_attachment_storage_quota(&state, "default", 8)
        .await
        .expect("streaming temp files should not count against profile storage quota");

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attachment_storage_usage_cache_updates_after_save_and_delete() {
    let sandbox = unique_test_dir("attachment-usage-cache");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let existing_dir = session_uploads_dir(&state, "default", "thread-0");
    fs::create_dir_all(&existing_dir).unwrap();
    fs::write(existing_dir.join("existing.txt"), b"123456").unwrap();

    assert_eq!(
        profile_attachment_storage_size(&state, "default")
            .await
            .unwrap(),
        6
    );
    assert_eq!(
        state
            .attachment_storage_usage_cache
            .lock()
            .await
            .get("default")
            .map(|entry| entry.bytes),
        Some(6)
    );

    let stored = save_uploaded_attachment_records(
        &state,
        "default",
        "thread-1",
        vec![AttachmentUploadPayload {
            name: "note.txt".to_string(),
            mime_type: Some("text/plain".to_string()),
            bytes: b"note".to_vec(),
        }],
    )
    .await
    .unwrap();
    let after_save = state
        .attachment_storage_usage_cache
        .lock()
        .await
        .get("default")
        .map(|entry| entry.bytes)
        .unwrap();
    assert!(after_save > 6);
    assert_eq!(
        profile_attachment_storage_size(&state, "default")
            .await
            .unwrap(),
        after_save
    );

    delete_attachment_payload(&state, "default", "thread-1", &stored[0].id)
        .await
        .unwrap();
    assert_eq!(
        profile_attachment_storage_size(&state, "default")
            .await
            .unwrap(),
        6
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_attachment_uploads_cannot_overbook_profile_quota() {
    let sandbox = unique_test_dir("attachment-concurrent-quota");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let mut state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let mut config = (*state.config).clone();
    config.max_attachment_storage_bytes = 1_400;
    state.config = Arc::new(config);
    let first_state = state.clone();
    let second_state = state.clone();

    let first = tokio::spawn(async move {
        save_uploaded_attachment_records(
            &first_state,
            "default",
            "thread-a",
            vec![AttachmentUploadPayload {
                name: "first.txt".to_string(),
                mime_type: Some("text/plain".to_string()),
                bytes: vec![b'a'; 700],
            }],
        )
        .await
    });
    let second = tokio::spawn(async move {
        save_uploaded_attachment_records(
            &second_state,
            "default",
            "thread-b",
            vec![AttachmentUploadPayload {
                name: "second.txt".to_string(),
                mime_type: Some("text/plain".to_string()),
                bytes: vec![b'b'; 700],
            }],
        )
        .await
    });
    let (first, second) = tokio::join!(first, second);
    let results = [first.unwrap(), second.unwrap()];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter_map(|result| result.as_ref().err())
            .filter(|error| error.status == StatusCode::PAYLOAD_TOO_LARGE)
            .count(),
        1
    );
    assert!(
        profile_attachment_storage_size(&state, "default")
            .await
            .unwrap()
            <= state.config.max_attachment_storage_bytes
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_routes_reject_invalid_session_ids_before_storage_access() {
    let sandbox = unique_test_dir("session-route-invalid-id");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let jar = issue_auth_cookie(&state.config, CookieJar::new(), false, UserRole::Admin).unwrap();
    let route_path = "/api/sessions/..%2Fescape/attachments";
    let request = Request::builder()
        .method(Method::GET)
        .uri(route_path)
        .body(Body::empty())
        .unwrap();

    let response = handle_session_route_http(
        state,
        &jar,
        &HeaderMap::new(),
        request,
        route_path,
        None,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_session_methods_reject_invalid_session_ids() {
    let sandbox = unique_test_dir("ws-invalid-session-id");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let (out_tx, _out_rx, _invalidation_rx) = WsOutbound::new(8);
    let subscriptions: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let error = execute_ws_method(
        &state,
        &out_tx,
        &subscriptions,
        &AuthContext {
            role: UserRole::Admin,
            profile_id: "default".to_string(),
        },
        "session/get",
        json!({
            "sessionId": "../escape"
        }),
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(error.contains("INVALID_SESSION_ID"));
    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_attachment_upload_rejects_large_base64_payload_before_decoding() {
    let sandbox = unique_test_dir("ws-attachment-large-payload");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let (out_tx, _out_rx, _invalidation_rx) = WsOutbound::new(8);
    let subscriptions: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let encoded_size =
        (((WS_ATTACHMENT_UPLOAD_MAX_DECODED_BYTES as usize) + 1) * 4 / 3).saturating_add(8);

    let error = execute_ws_method(
        &state,
        &out_tx,
        &subscriptions,
        &AuthContext {
            role: UserRole::Admin,
            profile_id: "default".to_string(),
        },
        "attachments/upload",
        json!({
            "sessionId": "thread-1",
            "files": [
                {
                    "name": "large.bin",
                    "mime_type": "application/octet-stream",
                    "data_base64": "A".repeat(encoded_size)
                }
            ]
        }),
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(error.contains("WebSocket attachment uploads are limited"));
    assert!(
        list_session_attachment_records(&state, "default", "thread-1")
            .await
            .unwrap()
            .is_empty()
    );
    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_recovery_ws_method_recovers_rollout_file() {
    let sandbox = unique_test_dir("session-recovery-ws");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state_with_fake_app_server(
        workspace.clone(),
        vec![workspace.clone()],
        codex_home.clone(),
    );
    let created_at = time::OffsetDateTime::now_utc().unix_timestamp();
    let created_date = time::OffsetDateTime::from_unix_timestamp(created_at)
        .unwrap()
        .date();
    let rollout_dir = codex_home
        .join("sessions")
        .join(created_date.year().to_string())
        .join(format!("{:02}", u8::from(created_date.month())))
        .join(format!("{:02}", created_date.day()));
    fs::create_dir_all(&rollout_dir).unwrap();
    let rollout_path = rollout_dir.join("2026-04-21-thread-1.jsonl");
    fs::write(&rollout_path, b"{\"step\":1}\n\xff\n{\"step\":2}\n").unwrap();

    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": "thread-1",
                    "name": "Recover rollout",
                    "preview": "Recover rollout",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": created_at,
                    "updatedAt": created_at,
                    "status": "idle",
                    "isSubagent": false,
                    "agentNickname": Value::Null,
                    "agentRole": Value::Null,
                    "turns": []
                }
            }),
        )
        .await
        .unwrap();

    let (out_tx, _out_rx, _invalidation_rx) = WsOutbound::new(8);
    let subscriptions: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let payload = execute_ws_method(
        &state,
        &out_tx,
        &subscriptions,
        &AuthContext {
            role: UserRole::Admin,
            profile_id: "default".to_string(),
        },
        "session/recovery",
        json!({
            "sessionId": "thread-1"
        }),
    )
    .await
    .unwrap();

    assert_eq!(payload.get("ok").and_then(Value::as_bool), Some(true));
    assert_eq!(
        payload.get("recoveredLines").and_then(Value::as_u64),
        Some(2)
    );
    assert_eq!(payload.get("skippedLines").and_then(Value::as_u64), Some(1));
    assert_eq!(
        fs::read_to_string(&rollout_path).unwrap(),
        "{\"step\":1}\n{\"step\":2}\n"
    );
    assert!(
        Path::new(
            payload
                .get("backupPath")
                .and_then(Value::as_str)
                .unwrap_or_default()
        )
        .exists()
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_detail_payload_surfaces_rollout_recovery_when_thread_read_fails() {
    let sandbox = unique_test_dir("session-detail-recovery-fallback");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state_with_fake_app_server(
        workspace.clone(),
        vec![workspace.clone()],
        codex_home.clone(),
    );
    let created_at = time::OffsetDateTime::now_utc().unix_timestamp();
    let created_date = time::OffsetDateTime::from_unix_timestamp(created_at)
        .unwrap()
        .date();
    let rollout_dir = codex_home
        .join("sessions")
        .join(created_date.year().to_string())
        .join(format!("{:02}", u8::from(created_date.month())))
        .join(format!("{:02}", created_date.day()));
    fs::create_dir_all(&rollout_dir).unwrap();
    let rollout_path = rollout_dir.join("2026-04-21-thread-1.jsonl");
    fs::write(&rollout_path, b"{\"step\":1}\n\xff\n{\"step\":2}\n").unwrap();

    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": "thread-1",
                    "name": "Recover rollout",
                    "preview": "Recover rollout",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": created_at,
                    "updatedAt": created_at,
                    "status": "idle",
                    "isSubagent": false,
                    "agentNickname": Value::Null,
                    "agentRole": Value::Null,
                    "turns": [],
                    "readError": {
                        "message": format!(
                            "failed to load rollout `{}` for thread thread-1: stream did not contain valid UTF-8",
                            rollout_path.display()
                        )
                    }
                }
            }),
        )
        .await
        .unwrap();

    let payload = session_detail_payload(&state, "default", "thread-1", 20)
        .await
        .unwrap();
    assert_eq!(
        payload
            .get("thread")
            .and_then(|thread| thread.get("id"))
            .and_then(Value::as_str),
        Some("thread-1")
    );
    assert_eq!(
        payload
            .get("hydration")
            .and_then(|hydration| hydration.get("state"))
            .and_then(Value::as_str),
        Some("error")
    );
    assert_eq!(
        payload
            .get("hydration")
            .and_then(|hydration| hydration.get("recovery"))
            .and_then(|recovery| recovery.get("available"))
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        payload
            .get("hydration")
            .and_then(|hydration| hydration.get("recovery"))
            .and_then(|recovery| recovery.get("issue"))
            .and_then(Value::as_str),
        Some("invalidUtf8")
    );
    assert_eq!(
        payload
            .get("hydration")
            .and_then(|hydration| hydration.get("recovery"))
            .and_then(|recovery| recovery.get("recoverableLines"))
            .and_then(Value::as_u64),
        Some(2)
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_recovery_ws_method_uses_thread_list_metadata_when_thread_read_fails() {
    let sandbox = unique_test_dir("session-recovery-ws-thread-list-fallback");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state_with_fake_app_server(
        workspace.clone(),
        vec![workspace.clone()],
        codex_home.clone(),
    );
    let created_at = time::OffsetDateTime::now_utc().unix_timestamp();
    let created_date = time::OffsetDateTime::from_unix_timestamp(created_at)
        .unwrap()
        .date();
    let rollout_dir = codex_home
        .join("sessions")
        .join(created_date.year().to_string())
        .join(format!("{:02}", u8::from(created_date.month())))
        .join(format!("{:02}", created_date.day()));
    fs::create_dir_all(&rollout_dir).unwrap();
    let rollout_path = rollout_dir.join("2026-04-21-thread-1.jsonl");
    fs::write(&rollout_path, b"{\"step\":1}\n\xff\n{\"step\":2}\n").unwrap();

    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": "thread-1",
                    "name": "Recover rollout",
                    "preview": "Recover rollout",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": created_at,
                    "updatedAt": created_at,
                    "status": "idle",
                    "isSubagent": false,
                    "agentNickname": Value::Null,
                    "agentRole": Value::Null,
                    "turns": [],
                    "readError": {
                        "message": format!(
                            "failed to load rollout `{}` for thread thread-1: stream did not contain valid UTF-8",
                            rollout_path.display()
                        )
                    }
                }
            }),
        )
        .await
        .unwrap();

    let (out_tx, _out_rx, _invalidation_rx) = WsOutbound::new(8);
    let subscriptions: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let payload = execute_ws_method(
        &state,
        &out_tx,
        &subscriptions,
        &AuthContext {
            role: UserRole::Admin,
            profile_id: "default".to_string(),
        },
        "session/recovery",
        json!({
            "sessionId": "thread-1"
        }),
    )
    .await
    .unwrap();
    assert_eq!(payload.get("ok").and_then(Value::as_bool), Some(true));
    assert_eq!(
        fs::read_to_string(&rollout_path).unwrap(),
        "{\"step\":1}\n{\"step\":2}\n"
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_config_payload_uses_rust_state_and_app_server_metadata() {
    let sandbox = unique_test_dir("config-rust");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    fs::write(
            config_toml_path(&codex_home),
            "model = \"gpt-5.4\"\nservice_tier = \"fast\"\napproval_policy = \"on-request\"\nsandbox_mode = \"workspace-write\"\n[sandbox_workspace_write]\nnetwork_access = true\n",
        )
        .unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);

    let payload = get_config_payload(&state, "default").await.unwrap();
    assert_eq!(
        payload
            .get("models")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        payload
            .get("collaborationModes")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(2)
    );
    assert_eq!(
        payload
            .get("defaults")
            .and_then(|value| value.get("model"))
            .and_then(Value::as_str),
        Some("gpt-5.4")
    );
    assert_eq!(
        payload
            .get("defaults")
            .and_then(|value| value.get("speed"))
            .and_then(Value::as_str),
        Some("fast")
    );
    assert_eq!(
        payload
            .get("git")
            .and_then(|value| value.get("discoveryDepth"))
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        payload
            .get("profiles")
            .and_then(Value::as_array)
            .and_then(|profiles| profiles.first())
            .and_then(|profile| profile.get("label"))
            .and_then(Value::as_str),
        Some("Default")
    );
    assert_eq!(
        payload
            .get("account")
            .and_then(|value| value.get("email"))
            .and_then(Value::as_str),
        Some("demo@example.com")
    );

    let _ = fs::remove_dir_all(sandbox);
}
