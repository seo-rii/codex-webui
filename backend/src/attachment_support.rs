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
            let (expected_file_path, expected_metadata_path) = attachment_storage_paths(
                state,
                profile_id,
                session_id,
                &record.id,
                &record.original_name,
            );
            let stored_file_path = record.path.as_deref().map(PathBuf::from);
            if path == expected_metadata_path
                && stored_file_path.as_ref() == Some(&expected_file_path)
                && tokio_fs::symlink_metadata(&expected_file_path)
                    .await
                    .is_ok_and(|metadata| metadata.file_type().is_file())
            {
                attachments.push(record);
            }
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

pub(crate) fn human_readable_byte_limit(bytes: u64) -> String {
    if bytes < 1024 * 1024 {
        format!("{bytes} bytes")
    } else {
        let mb = ((bytes as f64) / (1024.0 * 1024.0)).round() as u64;
        format!("{mb}MB")
    }
}

pub(crate) fn attachment_limit_error_message(max_upload_bytes: u64) -> String {
    format!(
        "Upload exceeds the {} limit.",
        human_readable_byte_limit(max_upload_bytes)
    )
}

pub(crate) fn attachment_storage_quota_error_message(max_storage_bytes: u64) -> String {
    format!(
        "Attachment storage exceeds the {} profile quota.",
        human_readable_byte_limit(max_storage_bytes)
    )
}

pub(crate) const MAX_ATTACHMENTS_PER_REQUEST: usize = 20;
pub(crate) const MAX_TOTAL_ATTACHMENT_UPLOAD_MULTIPLIER: u64 = 4;
const ATTACHMENT_STORAGE_USAGE_CACHE_TTL: Duration = Duration::from_secs(5 * 60);

async fn attachment_storage_lock(state: &AppState, profile_id: &str) -> Arc<Mutex<()>> {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id).0;
    let mut locks = state.attachment_storage_locks.lock().await;
    if locks.len() >= 64 && !locks.contains_key(&resolved_profile_id) {
        locks.retain(|_, lock| Arc::strong_count(lock) > 1);
    }
    Arc::clone(
        locks
            .entry(resolved_profile_id)
            .or_insert_with(|| Arc::new(Mutex::new(()))),
    )
}

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

pub(crate) fn max_total_attachment_upload_bytes(config: &Config) -> u64 {
    config
        .max_upload_bytes
        .saturating_mul(MAX_TOTAL_ATTACHMENT_UPLOAD_MULTIPLIER)
        .max(config.max_upload_bytes)
}

pub(crate) fn validate_total_attachment_size(config: &Config, size: u64) -> ApiResult<()> {
    let max_total = max_total_attachment_upload_bytes(config);
    if size > max_total {
        return Err(api_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            attachment_limit_error_message(max_total),
        ));
    }
    Ok(())
}

async fn scan_profile_attachment_storage_size(
    state: &AppState,
    profile_id: &str,
) -> ApiResult<u64> {
    let uploads_root = resolve_runtime_profile(&state.config, profile_id)
        .data_dir
        .join("uploads");
    let mut pending = vec![uploads_root];
    let mut total = 0_u64;

    while let Some(path) = pending.pop() {
        let mut entries = match tokio_fs::read_dir(&path).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    error.to_string(),
                ));
            }
        };

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?
        {
            let file_type = entry
                .file_type()
                .await
                .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
            if file_type.is_dir() {
                pending.push(entry.path());
                continue;
            }
            if !file_type.is_file() {
                continue;
            }

            let file_name = entry.file_name().to_string_lossy().to_string();
            if file_name.starts_with(".upload-")
                || file_name.starts_with(".codex-webui-attachment-")
                || file_name.ends_with(".upload")
            {
                continue;
            }

            let metadata = entry
                .metadata()
                .await
                .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
            total = total.saturating_add(metadata.len());
        }
    }

    Ok(total)
}

pub(crate) async fn profile_attachment_storage_size(
    state: &AppState,
    profile_id: &str,
) -> ApiResult<u64> {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    if let Some(cached) = state
        .attachment_storage_usage_cache
        .lock()
        .await
        .get(&resolved_profile_id)
        .cloned()
        && cached.scanned_at.elapsed() < ATTACHMENT_STORAGE_USAGE_CACHE_TTL
    {
        return Ok(cached.bytes);
    }

    let bytes = scan_profile_attachment_storage_size(state, &resolved_profile_id).await?;
    state.attachment_storage_usage_cache.lock().await.insert(
        resolved_profile_id,
        CachedAttachmentStorageUsage {
            scanned_at: Instant::now(),
            bytes,
        },
    );
    Ok(bytes)
}

async fn adjust_attachment_storage_usage_cache(state: &AppState, profile_id: &str, delta: i128) {
    let resolved_profile_id = resolve_runtime_profile_entry(&state.config, profile_id)
        .0
        .to_string();
    let mut cache = state.attachment_storage_usage_cache.lock().await;
    if let Some(cached) = cache.get_mut(&resolved_profile_id) {
        if delta >= 0 {
            cached.bytes = cached.bytes.saturating_add(delta as u64);
        } else {
            cached.bytes = cached.bytes.saturating_sub(delta.unsigned_abs() as u64);
        }
        cached.scanned_at = Instant::now();
    }
}

pub(crate) async fn validate_attachment_storage_quota(
    state: &AppState,
    profile_id: &str,
    incoming_bytes: u64,
) -> ApiResult<()> {
    let used = profile_attachment_storage_size(state, profile_id).await?;
    if used.saturating_add(incoming_bytes) > state.config.max_attachment_storage_bytes {
        return Err(api_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            attachment_storage_quota_error_message(state.config.max_attachment_storage_bytes),
        ));
    }
    Ok(())
}

async fn write_attachment_bytes_atomically(path: &Path, bytes: &[u8]) -> ApiResult<()> {
    let parent = path.parent().ok_or_else(|| {
        api_error(
            StatusCode::BAD_REQUEST,
            "Attachment path has no parent directory.",
        )
    })?;
    tokio_fs::create_dir_all(parent)
        .await
        .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    let temp_path = parent.join(format!(".codex-webui-attachment-{}.tmp", Uuid::new_v4()));
    let write_result = async {
        let mut file = tokio_fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)
            .await?;
        file.write_all(bytes).await?;
        file.sync_all().await?;
        drop(file);
        tokio_fs::rename(&temp_path, path).await?;
        if let Ok(parent_dir) = tokio_fs::File::open(parent).await {
            let _ = parent_dir.sync_all().await;
        }
        std::io::Result::Ok(())
    }
    .await;

    if let Err(error) = write_result {
        let _ = tokio_fs::remove_file(&temp_path).await;
        return Err(api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            error.to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn attachment_payload_from_record(record: &StoredAttachmentRecord) -> Value {
    json!({
        "id": record.id,
        "originalName": record.original_name,
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

    let total_size = uploads
        .iter()
        .map(|upload| upload.bytes.len() as u64)
        .fold(0_u64, u64::saturating_add);
    validate_total_attachment_size(&state.config, total_size)?;
    let storage_lock = attachment_storage_lock(state, profile_id).await;
    let _storage_guard = storage_lock.lock().await;

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

        let metadata = serde_json::to_vec_pretty(&record)
            .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
        validate_attachment_storage_quota(
            state,
            profile_id,
            size.saturating_add(metadata.len() as u64),
        )
        .await?;
        write_attachment_bytes_atomically(&file_path, &upload.bytes).await?;
        write_attachment_bytes_atomically(&meta_path, &metadata).await?;
        adjust_attachment_storage_usage_cache(
            state,
            profile_id,
            (size + metadata.len() as u64) as i128,
        )
        .await;
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
    let storage_lock = attachment_storage_lock(state, profile_id).await;
    let _storage_guard = storage_lock.lock().await;
    validate_attachment_storage_quota(
        state,
        profile_id,
        size.saturating_add(metadata.len() as u64),
    )
    .await?;
    tokio_fs::rename(source_path, &file_path)
        .await
        .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    if let Ok(parent_dir) = tokio_fs::File::open(&uploads_dir).await {
        let _ = parent_dir.sync_all().await;
    }
    write_attachment_bytes_atomically(&meta_path, &metadata).await?;
    adjust_attachment_storage_usage_cache(
        state,
        profile_id,
        (size + metadata.len() as u64) as i128,
    )
    .await;
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
    let storage_lock = attachment_storage_lock(state, profile_id).await;
    let _storage_guard = storage_lock.lock().await;
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
    let removed_size = tokio_fs::metadata(&file_path)
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(0)
        .saturating_add(
            tokio_fs::metadata(&meta_path)
                .await
                .map(|metadata| metadata.len())
                .unwrap_or(0),
        );
    let (file_result, meta_result) = tokio::join!(
        tokio_fs::remove_file(&file_path),
        tokio_fs::remove_file(&meta_path),
    );
    for result in [file_result, meta_result] {
        if let Err(error) = result
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to delete attachment: {error}"),
            ));
        }
    }
    adjust_attachment_storage_usage_cache(state, profile_id, -(removed_size as i128)).await;
    emit_attachments_updated(state, profile_id, session_id).await?;
    Ok(json!({ "ok": true }))
}

async fn path_is_old_enough(path: &Path, min_age_ms: u64) -> bool {
    if min_age_ms == 0 {
        return true;
    }
    tokio_fs::metadata(path)
        .await
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age.as_millis() as u64 >= min_age_ms)
}

pub(crate) async fn cleanup_attachment_orphans_payload(
    state: &AppState,
    profile_id: &str,
    dry_run: bool,
    min_age_ms: u64,
) -> ApiResult<Value> {
    let uploads_root = resolve_runtime_profile(&state.config, profile_id)
        .data_dir
        .join("uploads");
    let mut scanned_sessions = 0_u64;
    let mut orphan_files = 0_u64;
    let mut orphan_metadata = 0_u64;
    let mut removed_paths = 0_u64;
    let mut removed_bytes = 0_u64;
    let mut session_dirs = match tokio_fs::read_dir(&uploads_root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(json!({
                "dryRun": dry_run,
                "scannedSessions": 0,
                "orphanFiles": 0,
                "orphanMetadata": 0,
                "removedPaths": 0,
                "removedBytes": 0
            }));
        }
        Err(error) => {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                error.to_string(),
            ));
        }
    };

    while let Some(session_entry) = session_dirs
        .next_entry()
        .await
        .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?
    {
        let session_path = session_entry.path();
        if !session_entry
            .file_type()
            .await
            .map(|file_type| file_type.is_dir())
            .unwrap_or(false)
        {
            continue;
        }
        scanned_sessions += 1;
        let mut metadata_paths = Vec::new();
        let mut referenced_names = HashSet::new();
        let mut file_paths = Vec::new();
        let mut entries = match tokio_fs::read_dir(&session_path).await {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?
        {
            let path = entry.path();
            if !entry
                .file_type()
                .await
                .map(|file_type| file_type.is_file())
                .unwrap_or(false)
            {
                continue;
            }
            if path.extension().and_then(|value| value.to_str()) == Some("json") {
                metadata_paths.push(path);
            } else {
                file_paths.push(path);
            }
        }

        for metadata_path in metadata_paths {
            let raw = match tokio_fs::read_to_string(&metadata_path).await {
                Ok(raw) => raw,
                Err(_) => continue,
            };
            let record = serde_json::from_str::<StoredAttachmentRecord>(&raw).ok();
            let referenced = record
                .as_ref()
                .and_then(|record| record.path.as_deref())
                .map(PathBuf::from)
                .and_then(|path| path.file_name().map(|name| name.to_os_string()));
            if let Some(name) = referenced {
                let referenced_path = session_path.join(&name);
                if tokio_fs::metadata(&referenced_path).await.is_ok() {
                    referenced_names.insert(name);
                    continue;
                }
            }

            if path_is_old_enough(&metadata_path, min_age_ms).await {
                orphan_metadata += 1;
                let size = tokio_fs::metadata(&metadata_path)
                    .await
                    .map(|metadata| metadata.len())
                    .unwrap_or(0);
                if !dry_run && tokio_fs::remove_file(&metadata_path).await.is_ok() {
                    removed_paths += 1;
                    removed_bytes = removed_bytes.saturating_add(size);
                }
            }
        }

        for file_path in file_paths {
            let Some(file_name) = file_path.file_name().map(|name| name.to_os_string()) else {
                continue;
            };
            let is_temp_upload = file_name.to_string_lossy().ends_with(".upload");
            if !is_temp_upload && referenced_names.contains(&file_name) {
                continue;
            }
            if path_is_old_enough(&file_path, min_age_ms).await {
                orphan_files += 1;
                let size = tokio_fs::metadata(&file_path)
                    .await
                    .map(|metadata| metadata.len())
                    .unwrap_or(0);
                if !dry_run && tokio_fs::remove_file(&file_path).await.is_ok() {
                    removed_paths += 1;
                    removed_bytes = removed_bytes.saturating_add(size);
                }
            }
        }
    }

    if !dry_run && removed_bytes > 0 {
        adjust_attachment_storage_usage_cache(state, profile_id, -(removed_bytes as i128)).await;
    }

    Ok(json!({
        "dryRun": dry_run,
        "scannedSessions": scanned_sessions,
        "orphanFiles": orphan_files,
        "orphanMetadata": orphan_metadata,
        "removedPaths": removed_paths,
        "removedBytes": removed_bytes
    }))
}
