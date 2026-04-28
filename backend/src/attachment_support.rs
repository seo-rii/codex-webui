use super::*;

pub(crate) fn string_array_from_value(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

pub(crate) async fn list_session_attachment_records(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> Result<Vec<StoredAttachmentRecord>> {
    let uploads_dir = resolve_runtime_profile(&state.config, profile_id)
        .data_dir
        .join("uploads")
        .join(session_id);
    let mut entries = match tokio_fs::read_dir(&uploads_dir).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "failed to read session uploads directory {}",
                    uploads_dir.display()
                )
            });
        }
    };

    let mut attachments = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let raw = match tokio_fs::read_to_string(&path).await {
            Ok(raw) => raw,
            Err(_) => continue,
        };
        if let Ok(record) = serde_json::from_str::<StoredAttachmentRecord>(&raw) {
            attachments.push(record);
        }
    }

    attachments.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    Ok(attachments)
}

fn sanitize_attachment_file_name(name: &str) -> String {
    let mut sanitized = String::new();
    let mut last_was_dash = false;
    for ch in name.chars() {
        let next = if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            ch
        } else {
            '-'
        };
        if next == '-' {
            if last_was_dash {
                continue;
            }
            last_was_dash = true;
        } else {
            last_was_dash = false;
        }
        sanitized.push(next);
    }
    let trimmed = sanitized.trim_matches('-');
    if trimmed.is_empty() {
        "attachment".to_string()
    } else {
        trimmed.to_string()
    }
}

fn attachment_storage_paths(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    attachment_id: &str,
    original_name: &str,
) -> (PathBuf, PathBuf) {
    let uploads_dir = resolve_runtime_profile(&state.config, profile_id)
        .data_dir
        .join("uploads")
        .join(session_id);
    let base = format!(
        "{attachment_id}-{}",
        sanitize_attachment_file_name(original_name)
    );
    (
        uploads_dir.join(&base),
        uploads_dir.join(format!("{base}.json")),
    )
}

pub(crate) fn session_uploads_dir(state: &AppState, profile_id: &str, session_id: &str) -> PathBuf {
    resolve_runtime_profile(&state.config, profile_id)
        .data_dir
        .join("uploads")
        .join(session_id)
}

fn attachment_kind_for_mime(mime_type: &str) -> &'static str {
    match mime_type {
        "image/png" | "image/jpeg" | "image/webp" | "image/gif" => "image",
        _ => "file",
    }
}

pub(crate) fn attachment_limit_error_message(max_upload_bytes: u64) -> String {
    let max_upload_mb = ((max_upload_bytes as f64) / (1024.0 * 1024.0)).round() as u64;
    format!("Upload exceeds the {max_upload_mb}MB limit.")
}

pub(crate) const MAX_ATTACHMENTS_PER_REQUEST: usize = 20;

pub(crate) fn attachment_count_limit_error() -> ApiError {
    api_error(
        StatusCode::PAYLOAD_TOO_LARGE,
        format!("Upload is limited to {MAX_ATTACHMENTS_PER_REQUEST} files."),
    )
}

pub(crate) fn validate_attachment_size(config: &Config, size: u64) -> ApiResult<()> {
    if size > config.max_upload_bytes {
        return Err(api_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            attachment_limit_error_message(config.max_upload_bytes),
        ));
    }
    Ok(())
}

pub(crate) fn attachment_payload_from_record(record: &StoredAttachmentRecord) -> Value {
    json!({
        "id": record.id,
        "originalName": record.original_name,
        "path": record.path.clone().unwrap_or_default(),
        "mimeType": record
            .mime_type
            .clone()
            .unwrap_or_else(|| "application/octet-stream".to_string()),
        "size": record.size.unwrap_or(0),
        "kind": record.kind.clone().unwrap_or_else(|| "file".to_string()),
        "createdAt": record.created_at.clone().unwrap_or_default()
    })
}

pub(crate) async fn save_uploaded_attachment_records(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    uploads: Vec<AttachmentUploadPayload>,
) -> ApiResult<Vec<StoredAttachmentRecord>> {
    let mut stored = Vec::new();
    if uploads.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Select at least one file.",
        ));
    }

    let uploads_dir = resolve_runtime_profile(&state.config, profile_id)
        .data_dir
        .join("uploads")
        .join(session_id);
    tokio_fs::create_dir_all(&uploads_dir)
        .await
        .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;

    for upload in uploads {
        if upload.bytes.is_empty() {
            continue;
        }

        let size = upload.bytes.len() as u64;
        if size > state.config.max_upload_bytes {
            return Err(api_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                attachment_limit_error_message(state.config.max_upload_bytes),
            ));
        }

        let attachment_id = Uuid::new_v4().to_string();
        let original_name = if upload.name.trim().is_empty() {
            "attachment".to_string()
        } else {
            upload.name.trim().to_string()
        };
        let mime_type = upload
            .mime_type
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("application/octet-stream")
            .to_string();
        let (file_path, meta_path) = attachment_storage_paths(
            state,
            profile_id,
            session_id,
            &attachment_id,
            &original_name,
        );
        let record = StoredAttachmentRecord {
            id: attachment_id,
            original_name,
            path: Some(file_path.display().to_string()),
            mime_type: Some(mime_type.clone()),
            size: Some(size),
            kind: Some(attachment_kind_for_mime(&mime_type).to_string()),
            created_at: Some(now_unix_ms().to_string()),
        };

        tokio_fs::write(&file_path, &upload.bytes)
            .await
            .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
        let metadata = serde_json::to_vec_pretty(&record)
            .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
        tokio_fs::write(&meta_path, metadata)
            .await
            .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
        stored.push(record);
    }

    if stored.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Select at least one file.",
        ));
    }

    Ok(stored)
}

pub(crate) async fn store_uploaded_attachment_file(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    original_name: &str,
    mime_type: Option<String>,
    size: u64,
    source_path: &Path,
) -> ApiResult<StoredAttachmentRecord> {
    validate_attachment_size(&state.config, size)?;
    let uploads_dir = resolve_runtime_profile(&state.config, profile_id)
        .data_dir
        .join("uploads")
        .join(session_id);
    tokio_fs::create_dir_all(&uploads_dir)
        .await
        .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;

    let attachment_id = Uuid::new_v4().to_string();
    let original_name = if original_name.trim().is_empty() {
        "attachment".to_string()
    } else {
        original_name.trim().to_string()
    };
    let mime_type = mime_type
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("application/octet-stream")
        .to_string();
    let (file_path, meta_path) = attachment_storage_paths(
        state,
        profile_id,
        session_id,
        &attachment_id,
        &original_name,
    );
    tokio_fs::rename(source_path, &file_path)
        .await
        .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;

    let record = StoredAttachmentRecord {
        id: attachment_id,
        original_name,
        path: Some(file_path.display().to_string()),
        mime_type: Some(mime_type.clone()),
        size: Some(size),
        kind: Some(attachment_kind_for_mime(&mime_type).to_string()),
        created_at: Some(now_unix_ms().to_string()),
    };
    let metadata = serde_json::to_vec_pretty(&record)
        .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    tokio_fs::write(&meta_path, metadata)
        .await
        .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    Ok(record)
}

pub(crate) async fn emit_attachments_updated(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<()> {
    emit_session_notification(
        state,
        profile_id,
        session_id,
        json!({
            "kind": "notification",
            "method": "codex-webui/attachmentsUpdated",
            "params": {
                "attachments": list_session_attachments_payload(state, profile_id, session_id).await?
            }
        }),
    )
    .await;
    Ok(())
}

pub(crate) async fn resolve_queue_attachment_metadata(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    attachment_ids: Option<&Value>,
) -> ApiResult<(Vec<String>, Vec<String>)> {
    let requested_ids = string_array_from_value(attachment_ids);
    if requested_ids.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }

    let requested = requested_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let attachments = list_session_attachment_records(state, profile_id, session_id)
        .await
        .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    let filtered = attachments
        .into_iter()
        .filter(|attachment| requested.contains(attachment.id.as_str()))
        .collect::<Vec<_>>();

    Ok((
        filtered
            .iter()
            .map(|attachment| attachment.id.clone())
            .collect(),
        filtered
            .iter()
            .map(|attachment| attachment.original_name.clone())
            .collect(),
    ))
}

pub(crate) async fn delete_attachment_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    attachment_id: &str,
) -> ApiResult<Value> {
    let attachments = list_session_attachment_records(state, profile_id, session_id)
        .await
        .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    let Some(target) = attachments
        .iter()
        .find(|attachment| attachment.id == attachment_id)
    else {
        return Err(api_error(StatusCode::NOT_FOUND, "Attachment not found."));
    };
    let (file_path, meta_path) = attachment_storage_paths(
        state,
        profile_id,
        session_id,
        attachment_id,
        &target.original_name,
    );
    let _ = tokio::join!(
        tokio_fs::remove_file(file_path),
        tokio_fs::remove_file(meta_path),
    );
    emit_attachments_updated(state, profile_id, session_id).await?;
    Ok(json!({ "ok": true }))
}
