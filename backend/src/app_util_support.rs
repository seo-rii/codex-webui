use super::*;
use sha2::Digest;

pub(crate) fn session_relay_key(profile_id: &str, session_id: &str) -> String {
    format!("profile::{profile_id}::session::{session_id}")
}

pub(crate) fn global_relay_key(profile_id: &str) -> String {
    format!("profile::{profile_id}::{GLOBAL_RELAY_KEY}")
}

pub(crate) fn request_params_hash(params: &Value) -> String {
    let bytes = serde_json::to_vec(params).unwrap_or_else(|_| params.to_string().into_bytes());
    URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(bytes))
}

pub(crate) fn request_cache_key(
    profile_id: &str,
    request_id: &str,
    role: UserRole,
    method: &str,
    params_hash: &str,
) -> String {
    let role = match role {
        UserRole::Admin => "admin",
        UserRole::Viewer => "viewer",
    };
    format!(
        "profile::{profile_id}::role::{role}::method::{method}::params::{params_hash}::request::{request_id}"
    )
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

pub(crate) const SMALL_JSON_BODY_LIMIT: usize = 256 * 1024;
pub(crate) const LARGE_JSON_BODY_LIMIT: usize = 1024 * 1024;

pub(crate) async fn read_limited_body(
    request: Request,
    limit: usize,
    label: &str,
) -> ApiResult<Bytes> {
    if request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > limit)
    {
        return Err(api_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("{label} exceeds the {limit} byte limit."),
        ));
    }

    to_bytes(request.into_body(), limit).await.map_err(|error| {
        let message = error.to_string();
        if message.to_ascii_lowercase().contains("length limit") {
            api_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("{label} exceeds the {limit} byte limit."),
            )
        } else {
            api_error(StatusCode::BAD_REQUEST, format!("Failed to read {label}."))
        }
    })
}

pub(crate) async fn read_json_body(
    request: Request,
    limit: usize,
    label: &str,
) -> ApiResult<Value> {
    let body = read_limited_body(request, limit, label).await?;
    Ok(serde_json::from_slice(&body).unwrap_or_else(|_| json!({})))
}

pub(crate) const USAGE_LIMIT_EXCEEDED_CODE: &str = "USAGE_LIMIT_EXCEEDED";

pub(crate) fn structured_error_value(message: &str) -> Option<Value> {
    let trimmed = message.trim();
    if !trimmed.starts_with('{') {
        return None;
    }

    serde_json::from_str::<Value>(trimmed)
        .ok()
        .filter(Value::is_object)
}

pub(crate) fn structured_error_message(message: &str) -> Option<String> {
    let value = structured_error_value(message)?;
    let mut stack = vec![value];
    while let Some(current) = stack.pop() {
        match current {
            Value::Object(object) => {
                if let Some(message) = object.get("message").and_then(value_text) {
                    return Some(message);
                }

                for value in object.into_values() {
                    if matches!(value, Value::Object(_) | Value::Array(_)) {
                        stack.push(value);
                    }
                }
            }
            Value::Array(entries) => {
                stack.extend(entries);
            }
            _ => {}
        }
    }
    None
}

pub(crate) fn usage_limit_error_message(message: &str) -> bool {
    let lowered = message.to_ascii_lowercase();
    if lowered.contains("usagelimitexceeded")
        || (lowered.contains("usage limit")
            && (lowered.contains("hit")
                || lowered.contains("exceeded")
                || lowered.contains("reached")))
    {
        return true;
    }

    let Some(value) = structured_error_value(message) else {
        return false;
    };
    let mut stack = vec![value];
    while let Some(current) = stack.pop() {
        match current {
            Value::String(text) => {
                let lowered = text.to_ascii_lowercase();
                let compact = lowered
                    .chars()
                    .filter(|ch| ch.is_ascii_alphanumeric())
                    .collect::<String>();
                if compact == "usagelimitexceeded"
                    || (lowered.contains("usage limit")
                        && (lowered.contains("hit")
                            || lowered.contains("exceeded")
                            || lowered.contains("reached")))
                {
                    return true;
                }
            }
            Value::Object(object) => stack.extend(object.into_values()),
            Value::Array(entries) => stack.extend(entries),
            _ => {}
        }
    }
    false
}

pub(crate) fn retry_at_ms_from_value(value: &Value) -> Option<u64> {
    let now = now_unix_ms();
    let mut stack = vec![value.clone()];
    let mut candidates = Vec::new();

    while let Some(current) = stack.pop() {
        match current {
            Value::Object(object) => {
                for (key, value) in object {
                    let normalized_key = key
                        .chars()
                        .filter(|ch| ch.is_ascii_alphanumeric())
                        .map(|ch| ch.to_ascii_lowercase())
                        .collect::<String>();
                    let absolute_retry_key =
                        matches!(normalized_key.as_str(), "retryat" | "resetat" | "resetsat");
                    let relative_retry_key = matches!(
                        normalized_key.as_str(),
                        "retryafterseconds" | "resetafterseconds"
                    );

                    if absolute_retry_key || relative_retry_key {
                        let numeric = match &value {
                            Value::Number(number) => number.as_f64(),
                            Value::String(text) => text.trim().parse::<f64>().ok(),
                            _ => None,
                        };
                        if let Some(numeric) =
                            numeric.filter(|value| value.is_finite() && *value > 0.0)
                        {
                            let candidate = if relative_retry_key {
                                now.saturating_add((numeric * 1000.0).round() as u64)
                            } else if numeric >= 100_000_000_000.0 {
                                numeric.round() as u64
                            } else {
                                (numeric * 1000.0).round() as u64
                            };
                            candidates.push(candidate);
                        }
                    }

                    match value {
                        Value::String(text) if text.trim_start().starts_with('{') => {
                            if let Ok(parsed) = serde_json::from_str::<Value>(text.trim()) {
                                stack.push(parsed);
                            }
                        }
                        Value::Object(_) | Value::Array(_) => stack.push(value),
                        _ => {}
                    }
                }
            }
            Value::Array(entries) => stack.extend(entries),
            _ => {}
        }
    }

    candidates
        .iter()
        .copied()
        .filter(|candidate| *candidate >= now)
        .min()
        .or_else(|| candidates.into_iter().max())
}

pub(crate) fn usage_limit_error_payload(message: &str, retry_at_ms: Option<u64>) -> String {
    let display_message = structured_error_message(message)
        .unwrap_or_else(|| message.trim().to_string())
        .trim()
        .to_string();
    let mut payload = serde_json::Map::new();
    payload.insert(
        "code".to_string(),
        Value::String(USAGE_LIMIT_EXCEEDED_CODE.to_string()),
    );
    payload.insert(
        "status".to_string(),
        json!(StatusCode::TOO_MANY_REQUESTS.as_u16()),
    );
    payload.insert(
        "message".to_string(),
        Value::String(if display_message.is_empty() {
            "You've hit your usage limit.".to_string()
        } else {
            display_message
        }),
    );

    if let Some(retry_at_ms) = retry_at_ms {
        payload.insert("retryAt".to_string(), json!(retry_at_ms));
        let now = now_unix_ms();
        if retry_at_ms > now {
            payload.insert(
                "retryAfterSeconds".to_string(),
                json!((retry_at_ms - now).div_ceil(1000)),
            );
        }
    }
    if let Some(source) = structured_error_value(message) {
        payload.insert("appServerError".to_string(), source);
    }

    Value::Object(payload).to_string()
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

pub(crate) fn fnv1a32_hex(bytes: &[u8]) -> String {
    let mut hash = 0x811c9dc5_u32;
    for byte in bytes {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x01000193);
    }
    format!("{hash:08x}")
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
