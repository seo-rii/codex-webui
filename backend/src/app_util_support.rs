use super::*;
use sha2::Digest;

pub(crate) fn session_relay_key(profile_id: &str, session_id: &str) -> String {
    format!("profile::{profile_id}::session::{session_id}")
}

pub(crate) fn global_relay_key(profile_id: &str) -> String {
    format!("profile::{profile_id}::{GLOBAL_RELAY_KEY}")
}

pub(crate) fn request_cache_key(profile_id: &str, request_id: &str) -> String {
    format!("profile::{profile_id}::request::{request_id}")
}

pub(crate) fn runtime_session_key(profile_id: &str, session_id: &str) -> String {
    format!("profile::{profile_id}::session-runtime::{session_id}")
}

pub(crate) fn api_error(status: StatusCode, message: impl Into<String>) -> ApiError {
    ApiError {
        status,
        message: message.into(),
    }
}

pub(crate) fn trim_terminal_buffer(buffer: &mut String) {
    if buffer.len() <= TERMINAL_BUFFER_LIMIT {
        return;
    }

    let target = buffer.len().saturating_sub(TERMINAL_BUFFER_LIMIT);
    let trim_index = buffer
        .char_indices()
        .find(|(index, _)| *index >= target)
        .map(|(index, _)| index)
        .unwrap_or(0);
    buffer.replace_range(..trim_index, "");
}

pub(crate) async fn terminate_process(pid: u32) -> Result<()> {
    if cfg!(windows) {
        let output = run_command_with_timeout(
            "taskkill",
            vec![
                "/PID".to_string(),
                pid.to_string(),
                "/T".to_string(),
                "/F".to_string(),
            ],
            Duration::from_secs(4),
        )
        .await?;
        if !output.status.success() {
            anyhow::bail!("failed to stop terminal process.");
        }
        return Ok(());
    }

    let output = run_command_with_timeout(
        "kill",
        vec!["-TERM".to_string(), pid.to_string()],
        Duration::from_secs(4),
    )
    .await?;
    if !output.status.success() {
        anyhow::bail!("failed to stop terminal process.");
    }
    Ok(())
}

pub(crate) fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub(crate) fn now_rfc3339() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| String::new())
}

pub(crate) async fn upload_attachments(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    files: Vec<UploadFilePayload>,
) -> ApiResult<Value> {
    let mut uploads = Vec::new();
    for file in files {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(file.data_base64)
            .map_err(|error| api_error(StatusCode::BAD_REQUEST, error.to_string()))?;
        uploads.push(AttachmentUploadPayload {
            name: file.name,
            mime_type: file.mime_type,
            bytes,
        });
    }
    let stored = save_uploaded_attachment_records(state, profile_id, session_id, uploads).await?;
    emit_attachments_updated(state, profile_id, session_id).await?;
    Ok(json!({
        "attachments": stored
            .iter()
            .map(attachment_payload_from_record)
            .collect::<Vec<_>>()
    }))
}

pub(crate) fn json_error(status: StatusCode, message: &str) -> Response {
    let mut response = Json(json!({ "message": message })).into_response();
    *response.status_mut() = status;
    response
}

pub(crate) fn normalize_request_path(base_path: &str, path: &str) -> NormalizedPath {
    if base_path.is_empty() {
        return NormalizedPath::Route(path.to_string());
    }

    if path == "/" {
        return NormalizedPath::Redirect(format!("{base_path}/"));
    }

    if path == base_path {
        return NormalizedPath::Redirect(format!("{base_path}/"));
    }

    if let Some(stripped) = path.strip_prefix(base_path) {
        if stripped.is_empty() {
            return NormalizedPath::Route("/".to_string());
        }
        if stripped.starts_with('/') {
            return NormalizedPath::Route(stripped.to_string());
        }
    }

    NormalizedPath::OutsideBase
}

pub(crate) fn with_base(base_path: &str, route_path: &str) -> String {
    if base_path.is_empty() {
        return route_path.to_string();
    }
    if route_path == "/" {
        return format!("{base_path}/");
    }
    format!("{base_path}{route_path}")
}

pub(crate) fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

pub(crate) fn payload_cache_version(payload: &Value) -> String {
    let encoded = serde_json::to_vec(payload).unwrap_or_else(|_| payload.to_string().into_bytes());
    let mut hasher = Sha256::new();
    hasher.update(encoded);
    URL_SAFE_NO_PAD.encode(hasher.finalize())
}

pub(crate) fn cacheable_payload_response(payload: Value, known_version: Option<&str>) -> Value {
    let version = payload_cache_version(&payload);
    if known_version
        .map(str::trim)
        .is_some_and(|candidate| !candidate.is_empty() && candidate == version)
    {
        return json!({
            "cacheVersion": version,
            "notModified": true
        });
    }

    let mut next_payload = payload;
    if let Some(payload_object) = next_payload.as_object_mut() {
        payload_object.insert("cacheVersion".to_string(), Value::String(version));
        payload_object.insert("notModified".to_string(), Value::Bool(false));
    }
    next_payload
}

pub(crate) fn require_string(params: &Value, key: &str) -> Result<String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("{key} is required"))
}

pub(crate) fn query_param_value(query: Option<&str>, key: &str) -> Option<String> {
    query?.split('&').find_map(|entry| {
        let (raw_key, raw_value) = entry.split_once('=').unwrap_or((entry, ""));
        if raw_key != key {
            return None;
        }
        let decoded = raw_value.replace('+', "%20");
        urlencoding::decode(&decoded)
            .ok()
            .map(|value| value.into_owned())
    })
}

pub(crate) fn query_param_values(query: Option<&str>, key: &str) -> Vec<String> {
    query
        .unwrap_or_default()
        .split('&')
        .filter_map(|entry| {
            let (raw_key, raw_value) = entry.split_once('=').unwrap_or((entry, ""));
            if raw_key != key {
                return None;
            }
            let decoded = raw_value.replace('+', "%20");
            urlencoding::decode(&decoded)
                .ok()
                .map(|value| value.into_owned())
        })
        .collect()
}

pub(crate) fn selected_skills_from_value(value: Option<&Value>) -> Vec<Value> {
    let Some(entries) = value.and_then(Value::as_array) else {
        return Vec::new();
    };

    let mut seen = HashSet::new();
    entries
        .iter()
        .filter_map(|entry| {
            let object = entry.as_object()?;
            let name = object.get("name").and_then(Value::as_str)?.trim();
            let path = object.get("path").and_then(Value::as_str)?.trim();
            if name.is_empty() || path.is_empty() {
                return None;
            }
            let id = object
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(path);
            let key = format!("{name}\u{0}{path}");
            if !seen.insert(key) {
                return None;
            }
            Some(json!({
                "id": id,
                "name": name,
                "path": path
            }))
        })
        .collect()
}

pub(crate) enum NormalizedPath {
    Redirect(String),
    OutsideBase,
    Route(String),
}
