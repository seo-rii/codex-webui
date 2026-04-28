use super::*;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct DirectoryEntryPayload {
    pub(crate) name: String,
    pub(crate) path: String,
    #[serde(rename = "isDirectory")]
    pub(crate) is_directory: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct DirectoryPayload {
    #[serde(rename = "allowedRoots")]
    pub(crate) allowed_roots: Vec<DirectoryEntryPayload>,
    #[serde(rename = "currentPath")]
    pub(crate) current_path: Option<String>,
    #[serde(rename = "parentPath")]
    pub(crate) parent_path: Option<String>,
    pub(crate) entries: Vec<DirectoryEntryPayload>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct EditableFilePayload {
    pub(crate) path: String,
    #[serde(rename = "displayName")]
    pub(crate) display_name: String,
    pub(crate) content: String,
    pub(crate) language: String,
    pub(crate) writable: bool,
}

fn directory_entry_payload(path: &Path) -> DirectoryEntryPayload {
    DirectoryEntryPayload {
        name: path
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| path.display().to_string()),
        path: path.display().to_string(),
        is_directory: true,
    }
}

fn infer_editor_language(file_path: &Path) -> String {
    match file_path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .as_deref()
    {
        Some("ts" | "tsx") => "typescript",
        Some("js" | "mjs" | "cjs" | "jsx") => "javascript",
        Some("json") => "json",
        Some("toml") => "ini",
        Some("md") => "markdown",
        Some("yml" | "yaml") => "yaml",
        Some("svelte") => "html",
        Some("rs") => "rust",
        Some("py") => "python",
        Some("css") => "css",
        Some("sh") => "shell",
        _ => "plaintext",
    }
    .to_string()
}

pub(crate) fn ensure_not_sensitive_file_path(path: &Path) -> ApiResult<()> {
    for component in path.components() {
        let Some(part) = component.as_os_str().to_str() else {
            continue;
        };
        let lowered = part.to_ascii_lowercase();
        if lowered == ".ssh"
            || lowered == ".git"
            || lowered == ".git-credentials"
            || lowered == ".npmrc"
            || lowered == "auth.json"
            || lowered == "id_rsa"
            || lowered == "id_ed25519"
            || lowered.starts_with(".env")
            || lowered.ends_with(".pem")
            || lowered.ends_with(".key")
            || lowered.contains("session_secret")
            || lowered.contains("password_hash")
        {
            return Err(api_error(
                StatusCode::FORBIDDEN,
                "This file is blocked by the sensitive file policy.",
            ));
        }
    }
    Ok(())
}

pub(crate) async fn list_directories_payload(
    state: &AppState,
    current_path: Option<&str>,
) -> ApiResult<Value> {
    let resolved_roots = resolved_allowed_roots(&state.config).await;
    let root_entries = resolved_roots
        .iter()
        .map(|root| directory_entry_payload(root))
        .collect::<Vec<_>>();

    let Some(current_path) = current_path.filter(|value| !value.trim().is_empty()) else {
        return Ok(serde_json::to_value(DirectoryPayload {
            allowed_roots: root_entries.clone(),
            current_path: None,
            parent_path: None,
            entries: root_entries,
        })
        .expect("directory payload should serialize"));
    };

    let candidate = resolve_input_path(&state.config.project_root, current_path);
    let resolved = real_path_safe(&candidate).await;
    if !resolved_roots
        .iter()
        .any(|root| path_is_within(root, &resolved))
    {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "The selected path is outside the allowed roots.",
        ));
    }

    let metadata = tokio_fs::metadata(&resolved).await.map_err(|_| {
        api_error(
            StatusCode::BAD_REQUEST,
            "The selected path is not a directory.",
        )
    })?;
    if !metadata.is_dir() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "The selected path is not a directory.",
        ));
    }

    let mut reader = tokio_fs::read_dir(&resolved).await.map_err(|_| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to read directory.",
        )
    })?;
    let mut entries = Vec::new();
    while let Some(entry) = reader.next_entry().await.map_err(|_| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to read directory.",
        )
    })? {
        let file_type = entry.file_type().await.map_err(|_| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to inspect directory entry.",
            )
        })?;
        if file_type.is_dir() {
            entries.push(directory_entry_payload(&entry.path()));
        }
    }
    entries.sort_by(|left, right| left.name.cmp(&right.name));

    let parent_path = if resolved_roots.iter().any(|root| root == &resolved) {
        None
    } else {
        resolved.parent().map(|parent| parent.display().to_string())
    };

    Ok(serde_json::to_value(DirectoryPayload {
        allowed_roots: root_entries,
        current_path: Some(resolved.display().to_string()),
        parent_path,
        entries,
    })
    .expect("directory payload should serialize"))
}

pub(crate) async fn resolve_editable_file_path(
    state: &AppState,
    profile_id: &str,
    file_path: &str,
) -> ApiResult<PathBuf> {
    if file_path.trim().is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "filePath is required."));
    }

    let candidate = resolve_input_path(&state.config.project_root, file_path);
    ensure_not_sensitive_file_path(&candidate)?;
    let existing = tokio_fs::canonicalize(&candidate).await.ok();
    let path_to_check = existing.unwrap_or_else(|| candidate.clone());
    ensure_not_sensitive_file_path(&path_to_check)?;

    let roots = editable_file_roots(state, profile_id).await;

    if !roots
        .iter()
        .any(|root| path_is_within(root, &path_to_check))
    {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "This file is outside editable roots.",
        ));
    }

    Ok(candidate)
}

pub(crate) async fn editable_file_roots(state: &AppState, profile_id: &str) -> Vec<PathBuf> {
    let mut roots = resolved_allowed_roots(&state.config).await;
    let profile_root =
        real_path_safe(&resolve_runtime_profile(&state.config, profile_id).codex_home).await;
    roots.push(profile_root);
    roots
}

pub(crate) async fn write_text_file_safely(
    target_path: &Path,
    content: &str,
    allowed_roots: &[PathBuf],
) -> ApiResult<()> {
    ensure_not_sensitive_file_path(target_path)?;
    let parent_path = target_path.parent().ok_or_else(|| {
        api_error(
            StatusCode::BAD_REQUEST,
            "The selected file path has no parent directory.",
        )
    })?;
    let file_name = target_path.file_name().ok_or_else(|| {
        api_error(
            StatusCode::BAD_REQUEST,
            "The selected file path has no file name.",
        )
    })?;

    let mut ancestor = PathBuf::new();
    for component in parent_path.components() {
        ancestor.push(component.as_os_str());
        if let Ok(metadata) = tokio_fs::symlink_metadata(&ancestor).await {
            if metadata.file_type().is_symlink() {
                return Err(api_error(
                    StatusCode::FORBIDDEN,
                    "Refusing to write through a symlinked parent directory.",
                ));
            }
        }
    }

    tokio_fs::create_dir_all(parent_path).await.map_err(|_| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to create parent directories for the file.",
        )
    })?;

    let canonical_parent = tokio_fs::canonicalize(parent_path).await.map_err(|_| {
        api_error(
            StatusCode::BAD_REQUEST,
            "The selected file parent directory is invalid.",
        )
    })?;
    if !allowed_roots
        .iter()
        .any(|root| path_is_within(root, &canonical_parent))
    {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "The selected file parent directory is outside editable roots.",
        ));
    }

    let final_path = canonical_parent.join(file_name);
    ensure_not_sensitive_file_path(&final_path)?;
    if let Ok(metadata) = tokio_fs::symlink_metadata(&final_path).await {
        if metadata.file_type().is_symlink() {
            return Err(api_error(
                StatusCode::FORBIDDEN,
                "Refusing to replace a symlinked file.",
            ));
        }
    }

    let temp_path = canonical_parent.join(format!(
        ".codex-webui-write-{}-{}.tmp",
        file_name.to_string_lossy(),
        Uuid::new_v4()
    ));
    let write_result = async {
        let mut temp_file = tokio_fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)
            .await?;
        temp_file.write_all(content.as_bytes()).await?;
        temp_file.sync_all().await?;
        drop(temp_file);
        tokio_fs::rename(&temp_path, &final_path).await?;
        if let Ok(parent_dir) = tokio_fs::File::open(&canonical_parent).await {
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

    let final_check = tokio_fs::canonicalize(&final_path)
        .await
        .unwrap_or_else(|_| final_path.clone());
    if !allowed_roots
        .iter()
        .any(|root| path_is_within(root, &final_check))
    {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "The selected file escaped editable roots during save.",
        ));
    }

    Ok(())
}

pub(crate) async fn ensure_text_file_preview_size(path: &Path) -> ApiResult<()> {
    match tokio_fs::metadata(path).await {
        Ok(metadata) => {
            if metadata.len() > TEXT_FILE_PREVIEW_LIMIT_BYTES {
                return Err(api_error(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "The selected file is too large to preview.",
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to inspect the selected file.",
            ));
        }
    }
    Ok(())
}

pub(crate) async fn read_editable_file_payload(
    state: &AppState,
    profile_id: &str,
    file_path: &str,
) -> ApiResult<Value> {
    let resolved_path = resolve_editable_file_path(state, profile_id, file_path).await?;
    ensure_text_file_preview_size(&resolved_path).await?;
    let content = match tokio_fs::read_to_string(&resolved_path).await {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(_) => {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to read the selected file.",
            ));
        }
    };

    Ok(serde_json::to_value(EditableFilePayload {
        path: resolved_path.display().to_string(),
        display_name: resolved_path
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| resolved_path.display().to_string()),
        content,
        language: infer_editor_language(&resolved_path),
        writable: true,
    })
    .expect("editable file payload should serialize"))
}

pub(crate) async fn write_editable_file_payload(
    state: &AppState,
    profile_id: &str,
    file_path: &str,
    content: &str,
) -> ApiResult<Value> {
    let resolved_path = resolve_editable_file_path(state, profile_id, file_path).await?;
    let roots = editable_file_roots(state, profile_id).await;
    write_text_file_safely(&resolved_path, content, &roots).await?;
    read_editable_file_payload(state, profile_id, &resolved_path.display().to_string()).await
}

pub(crate) async fn resolve_allowed_directory(
    state: &AppState,
    requested_path: &str,
) -> ApiResult<String> {
    if requested_path.trim().is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "A working directory is required.",
        ));
    }

    let candidate = resolve_input_path(&state.config.project_root, requested_path);
    let resolved = tokio_fs::canonicalize(&candidate).await.map_err(|_| {
        api_error(
            StatusCode::BAD_REQUEST,
            "The selected working directory does not exist.",
        )
    })?;
    let metadata = tokio_fs::metadata(&resolved).await.map_err(|_| {
        api_error(
            StatusCode::BAD_REQUEST,
            "The selected working directory is invalid.",
        )
    })?;
    if !metadata.is_dir() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "The selected working directory must be a directory.",
        ));
    }

    let allowed_roots = resolved_allowed_roots(&state.config).await;
    if !allowed_roots
        .iter()
        .any(|root| path_is_within(root, &resolved))
    {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "The selected working directory is outside the allowed roots.",
        ));
    }

    Ok(resolved.display().to_string())
}
