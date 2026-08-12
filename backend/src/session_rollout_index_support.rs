use super::*;
use rusqlite::{Connection, OpenFlags, params_from_iter};

const SESSION_ROLLOUT_INDEX_CACHE_TTL: Duration = Duration::from_secs(20);
const SESSION_ROLLOUT_PREVIEW_SCAN_LIMIT: usize = 160;
const SESSION_ROLLOUT_TITLE_SCAN_LIMIT: usize = 1024;
const SESSION_ROLLOUT_SCAN_MAX_CANDIDATES: usize = 50_000;
const SESSION_ROLLOUT_SCAN_MAX_DIRECTORIES: usize = 10_000;

fn session_index_path(codex_home: &Path) -> PathBuf {
    codex_home.join("session_index.jsonl")
}

fn state_database_path(codex_home: &Path) -> PathBuf {
    codex_home.join("state_5.sqlite")
}

fn session_rollout_index_cache_key(profile_id: &str, archived: bool) -> String {
    format!("{profile_id}:rollout-index:archived={archived}")
}

fn candidate_session_id_from_path(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?.trim();
    if stem.len() >= 36 {
        let candidate = &stem[stem.len() - 36..];
        if candidate
            .chars()
            .all(|character| character.is_ascii_hexdigit() || character == '-')
        {
            return Some(candidate.to_string());
        }
    }
    None
}

fn system_time_to_unix_ms(system_time: SystemTime) -> i64 {
    system_time
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or_default()
}

fn parse_timestamp_to_unix_ms(value: &str) -> Option<i64> {
    time::OffsetDateTime::parse(value.trim(), &time::format_description::well_known::Rfc3339)
        .ok()
        .map(|timestamp| timestamp.unix_timestamp_nanos() / 1_000_000)
        .and_then(|value| i64::try_from(value).ok())
}

pub(crate) fn candidate_effective_updated_at(candidate: &Value) -> i64 {
    let indexed_updated_at = candidate
        .get("indexedUpdatedAt")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let rollout_updated_at = candidate
        .get("updatedAt")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    indexed_updated_at.max(rollout_updated_at)
}

fn candidate_indexed_name(candidate: &Value) -> Option<String> {
    candidate
        .get("indexedName")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub(crate) fn candidate_matches_indexed_query(candidate: &Value, needle: &str) -> bool {
    candidate_indexed_name(candidate)
        .map(|name| name.to_lowercase().contains(needle))
        .unwrap_or(false)
}

fn read_session_index_entries(codex_home: &Path) -> HashMap<String, Value> {
    let mut entries = HashMap::new();
    let Ok(file) = fs::File::open(session_index_path(codex_home)) else {
        return entries;
    };
    let reader = std::io::BufReader::new(file);
    for raw_line in std::io::BufRead::lines(reader) {
        let Ok(raw_line) = raw_line else {
            continue;
        };
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(parsed) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        let Some(session_id) = parsed
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let indexed_name = parsed
            .get("thread_name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let indexed_updated_at = parsed
            .get("updated_at")
            .and_then(Value::as_str)
            .and_then(parse_timestamp_to_unix_ms);
        entries.insert(
            session_id.to_string(),
            json!({
                "indexedName": indexed_name,
                "indexedUpdatedAt": indexed_updated_at
            }),
        );
    }
    entries
}

fn normalize_rollout_preview_text(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty()
        || trimmed.starts_with("<environment_context>")
        || trimmed.starts_with("# AGENTS.md instructions")
        || trimmed.starts_with("<permissions instructions>")
        || trimmed.starts_with("<skills_instructions>")
        || trimmed.starts_with("<apps_instructions>")
        || trimmed.starts_with("<plugins_instructions>")
    {
        return None;
    }

    let without_attachments =
        if let Some(rest) = trimmed.strip_prefix(&format!("{ATTACHMENT_PREAMBLE_START}\n")) {
            if let Some((_, tail)) = rest.split_once(&format!("\n{ATTACHMENT_PREAMBLE_END}")) {
                tail.trim_start_matches('\n').trim()
            } else {
                trimmed
            }
        } else {
            trimmed
        };

    (!without_attachments.is_empty()).then_some(without_attachments.to_string())
}

fn rollout_preview_from_response_item(payload: &Value) -> Option<String> {
    payload
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            (entry.get("type").and_then(Value::as_str) == Some("input_text"))
                .then(|| entry.get("text").and_then(Value::as_str))
                .flatten()
        })
        .find_map(normalize_rollout_preview_text)
}

fn merge_candidate_metadata_into_thread(thread: &mut Value, candidate: &Value, archived: bool) {
    let Some(thread_object) = thread.as_object_mut() else {
        return;
    };
    let session_id = candidate
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_default();
    if thread_object
        .get("id")
        .and_then(Value::as_str)
        .is_none_or(|value| value.trim().is_empty())
        && !session_id.is_empty()
    {
        thread_object.insert("id".to_string(), Value::String(session_id));
    }
    if let Some(path) = candidate
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        thread_object.insert("rolloutPath".to_string(), Value::String(path.to_string()));
    }
    if thread_name_is_preview_fallback(
        thread_object.get("name").and_then(Value::as_str),
        thread_object
            .get("preview")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    ) {
        if let Some(indexed_name) = candidate_indexed_name(candidate) {
            thread_object.insert("name".to_string(), Value::String(indexed_name));
        }
    }

    let effective_updated_at = candidate_effective_updated_at(candidate);
    let current_updated_at = thread_object
        .get("updatedAt")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    if current_updated_at <= 0 {
        thread_object.insert("updatedAt".to_string(), json!(effective_updated_at));
    }

    if thread_object
        .get("createdAt")
        .and_then(Value::as_i64)
        .unwrap_or_default()
        <= 0
    {
        thread_object.insert(
            "createdAt".to_string(),
            json!(
                effective_updated_at.max(
                    candidate
                        .get("updatedAt")
                        .and_then(Value::as_i64)
                        .unwrap_or_default()
                )
            ),
        );
    }

    thread_object.insert("archived".to_string(), Value::Bool(archived));
    if thread_object
        .get("status")
        .and_then(Value::as_str)
        .is_none_or(|value| value.trim().is_empty())
    {
        thread_object.insert("status".to_string(), Value::String("completed".to_string()));
    }

    let has_agent_nickname = thread_object
        .get("agentNickname")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty());
    let has_agent_role = thread_object
        .get("agentRole")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty());
    if !thread_object
        .get("isSubagent")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && (has_agent_nickname || has_agent_role)
    {
        thread_object.insert("isSubagent".to_string(), Value::Bool(true));
    }
}

fn read_state_thread_metadata_rows_from_codex_home(
    codex_home: &Path,
    session_ids: &[String],
) -> Result<HashMap<String, Value>, String> {
    if session_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let database_path = state_database_path(codex_home);
    if !database_path.is_file() {
        return Ok(HashMap::new());
    }

    let connection = Connection::open_with_flags(database_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("failed to open state database: {error}"))?;
    let placeholders = std::iter::repeat_n("?", session_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let query = format!(
        "SELECT \
            t.id, \
            t.title, \
            t.first_user_message, \
            t.cwd, \
            t.archived, \
            COALESCE(NULLIF(t.created_at_ms, 0), t.created_at * 1000), \
            COALESCE(NULLIF(t.updated_at_ms, 0), t.updated_at * 1000), \
            t.agent_nickname, \
            t.agent_role, \
            t.source, \
            CASE WHEN EXISTS(SELECT 1 FROM thread_spawn_edges edge WHERE edge.child_thread_id = t.id) THEN 1 ELSE 0 END \
        FROM threads t \
        WHERE t.id IN ({placeholders})"
    );
    let mut statement = connection
        .prepare(&query)
        .map_err(|error| format!("failed to prepare thread metadata query: {error}"))?;
    let rows = statement
        .query_map(params_from_iter(session_ids.iter()), |row| {
            let session_id: String = row.get(0)?;
            let title: String = row.get(1)?;
            let first_user_message: String = row.get(2)?;
            let cwd: String = row.get(3)?;
            let archived: i64 = row.get(4)?;
            let created_at: i64 = row.get(5)?;
            let updated_at: i64 = row.get(6)?;
            let agent_nickname: Option<String> = row.get(7)?;
            let agent_role: Option<String> = row.get(8)?;
            let source_raw: String = row.get(9)?;
            let spawned_subagent: i64 = row.get(10)?;
            let source = serde_json::from_str::<Value>(&source_raw)
                .unwrap_or_else(|_| Value::String(source_raw));
            let source_has_subagent = thread_source_marks_subagent(&source);
            Ok((
                session_id.clone(),
                json!({
                    "id": session_id,
                    "name": title,
                    "preview": first_user_message,
                    "cwd": cwd,
                    "archived": archived != 0,
                    "createdAt": created_at,
                    "updatedAt": updated_at,
                    "status": "completed",
                    "source": source,
                    "isSubagent": spawned_subagent != 0
                        || source_has_subagent
                        || agent_nickname.as_deref().is_some_and(|value| !value.trim().is_empty())
                        || agent_role.as_deref().is_some_and(|value| !value.trim().is_empty()),
                    "agentNickname": agent_nickname,
                    "agentRole": agent_role
                }),
            ))
        })
        .map_err(|error| format!("failed to read thread metadata rows: {error}"))?;

    let mut metadata_by_id = HashMap::new();
    for row in rows {
        let (session_id, payload) =
            row.map_err(|error| format!("failed to decode thread metadata row: {error}"))?;
        metadata_by_id.insert(session_id, payload);
    }
    Ok(metadata_by_id)
}

fn read_rollout_thread_metadata_from_path(
    path: &Path,
    archived: bool,
    modified_at: i64,
) -> Option<Value> {
    let file = fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);
    let mut session_id = candidate_session_id_from_path(path);
    let mut created_at = modified_at;
    let mut cwd = Value::Null;
    let mut preview = None;
    let mut name = None;
    let mut is_subagent = false;
    let mut agent_nickname = Value::Null;
    let mut agent_role = Value::Null;
    let mut preview_scan_count = 0usize;
    let mut title_scan_count = 0usize;

    for raw_line in std::io::BufRead::lines(reader) {
        if preview_scan_count >= SESSION_ROLLOUT_PREVIEW_SCAN_LIMIT
            && title_scan_count >= SESSION_ROLLOUT_TITLE_SCAN_LIMIT
        {
            break;
        }
        let Ok(raw_line) = raw_line else {
            continue;
        };
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }
        preview_scan_count += 1;
        title_scan_count += 1;
        let Ok(parsed) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        let record_type = parsed
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();

        if record_type == "session_meta" {
            let payload = parsed.get("payload").unwrap_or(&Value::Null);
            session_id = payload
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .or(session_id);
            created_at = payload
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(parse_timestamp_to_unix_ms)
                .or_else(|| {
                    parsed
                        .get("timestamp")
                        .and_then(Value::as_str)
                        .and_then(parse_timestamp_to_unix_ms)
                })
                .unwrap_or(modified_at);
            cwd = payload.get("cwd").cloned().unwrap_or(Value::Null);
            name = payload
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .or(name);
            is_subagent = payload
                .get("source")
                .is_some_and(thread_source_marks_subagent)
                || payload
                    .get("agent_nickname")
                    .and_then(Value::as_str)
                    .is_some();
            agent_nickname = payload
                .get("agent_nickname")
                .cloned()
                .or_else(|| payload.get("agentNickname").cloned())
                .unwrap_or(Value::Null);
            agent_role = payload
                .get("agent_role")
                .cloned()
                .or_else(|| payload.get("agentRole").cloned())
                .unwrap_or(Value::Null);
        } else if record_type == "event_msg" {
            let payload = parsed.get("payload").unwrap_or(&Value::Null);
            if payload.get("type").and_then(Value::as_str) == Some("thread_name_updated") {
                if let Some(thread_name) = payload
                    .get("thread_name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    name = Some(thread_name.to_string());
                }
            } else if preview.is_none()
                && preview_scan_count <= SESSION_ROLLOUT_PREVIEW_SCAN_LIMIT
                && payload.get("type").and_then(Value::as_str) == Some("user_message")
            {
                preview = payload
                    .get("message")
                    .and_then(Value::as_str)
                    .and_then(normalize_rollout_preview_text);
            }
        } else if preview.is_none()
            && preview_scan_count <= SESSION_ROLLOUT_PREVIEW_SCAN_LIMIT
            && record_type == "response_item"
        {
            let payload = parsed.get("payload").unwrap_or(&Value::Null);
            if payload.get("type").and_then(Value::as_str) == Some("message")
                && payload.get("role").and_then(Value::as_str) == Some("user")
            {
                preview = rollout_preview_from_response_item(payload);
            }
        } else if parsed.get("method").and_then(Value::as_str) == Some("thread/name/updated") {
            if let Some(thread_name) = parsed
                .get("params")
                .and_then(|value| {
                    value
                        .get("threadName")
                        .or_else(|| value.get("thread_name"))
                        .or_else(|| value.get("name"))
                        .or_else(|| value.get("title"))
                })
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                name = Some(thread_name.to_string());
            }
        }

        if preview_scan_count >= SESSION_ROLLOUT_PREVIEW_SCAN_LIMIT
            && title_scan_count >= SESSION_ROLLOUT_TITLE_SCAN_LIMIT
        {
            break;
        }
        if session_id.is_some()
            && !cwd.is_null()
            && preview.is_some()
            && (name.is_some() || title_scan_count >= SESSION_ROLLOUT_TITLE_SCAN_LIMIT)
        {
            break;
        }
    }

    let session_id = session_id?;
    Some(json!({
        "id": session_id,
        "name": name,
        "preview": preview.unwrap_or_default(),
        "cwd": cwd,
        "archived": archived,
        "createdAt": created_at,
        "updatedAt": modified_at,
        "status": "completed",
        "isSubagent": is_subagent,
        "agentNickname": agent_nickname,
        "agentRole": agent_role
    }))
}

fn rollout_file_contains_query(path: &Path, needle: &str) -> bool {
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    let reader = std::io::BufReader::new(file);
    for raw_line in std::io::BufRead::lines(reader) {
        let Ok(raw_line) = raw_line else {
            continue;
        };
        if raw_line.to_lowercase().contains(needle) {
            return true;
        }
    }
    false
}

fn collect_rollout_candidates(codex_home: &Path, archived: bool) -> Vec<Value> {
    let session_index_entries = read_session_index_entries(codex_home);
    let root = if archived {
        codex_home.join("archived_sessions")
    } else {
        codex_home.join("sessions")
    };
    let mut candidates = Vec::new();
    let mut pending = vec![root];
    let mut scanned_directories = 0usize;

    while let Some(directory) = pending.pop() {
        scanned_directories = scanned_directories.saturating_add(1);
        if scanned_directories > SESSION_ROLLOUT_SCAN_MAX_DIRECTORIES {
            break;
        }
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                pending.push(path);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
                continue;
            }
            let modified_at = entry
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .map(system_time_to_unix_ms)
                .unwrap_or_default();
            let session_id = candidate_session_id_from_path(&path);
            let indexed_entry = session_id
                .as_ref()
                .and_then(|session_id| session_index_entries.get(session_id))
                .cloned()
                .unwrap_or(Value::Null);
            candidates.push(json!({
                "id": session_id,
                "path": path.display().to_string(),
                "updatedAt": modified_at,
                "indexedName": indexed_entry.get("indexedName").cloned().unwrap_or(Value::Null),
                "indexedUpdatedAt": indexed_entry.get("indexedUpdatedAt").cloned().unwrap_or(Value::Null)
            }));
            if candidates.len() >= SESSION_ROLLOUT_SCAN_MAX_CANDIDATES {
                break;
            }
        }
        if candidates.len() >= SESSION_ROLLOUT_SCAN_MAX_CANDIDATES {
            break;
        }
    }

    candidates.sort_by(|left, right| {
        candidate_effective_updated_at(right)
            .cmp(&candidate_effective_updated_at(left))
            .then_with(|| {
                right
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .cmp(left.get("id").and_then(Value::as_str).unwrap_or_default())
            })
    });
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rollout_activity_time_wins_over_older_title_index_time() {
        let candidate = json!({
            "indexedUpdatedAt": 100,
            "updatedAt": 200
        });

        assert_eq!(candidate_effective_updated_at(&candidate), 200);
    }

    #[cfg(unix)]
    #[test]
    fn collect_rollout_candidates_skips_symlink_directories() {
        let sandbox =
            std::env::temp_dir().join(format!("codex-webui-rollout-symlink-{}", Uuid::new_v4()));
        let sessions_dir = sandbox.join("sessions").join("2026").join("05").join("06");
        fs::create_dir_all(&sessions_dir).expect("test should create sessions directory");
        fs::write(
            sessions_dir.join("rollout-2026-05-06T00-00-00-019df000-0000-7000-8000-000000000001.jsonl"),
            br#"{"type":"session_meta","payload":{"id":"019df000-0000-7000-8000-000000000001","cwd":"/tmp"}}"#,
        )
        .expect("test should write rollout");
        std::os::unix::fs::symlink(&sandbox, sandbox.join("sessions").join("loop"))
            .expect("test should create symlink loop");

        let candidates = collect_rollout_candidates(&sandbox, false);

        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates
                .first()
                .and_then(|candidate| candidate.get("id"))
                .and_then(Value::as_str),
            Some("019df000-0000-7000-8000-000000000001")
        );
        let _ = fs::remove_dir_all(sandbox);
    }
}

pub(crate) async fn list_rollout_candidates_shared_payload(
    state: &AppState,
    profile_id: &str,
    archived: bool,
) -> ApiResult<Arc<Vec<Value>>> {
    let cache_key = session_rollout_index_cache_key(profile_id, archived);
    {
        let mut cache = state.session_thread_cache.lock().await;
        cache.retain(|_, entry| entry.created_at.elapsed() < SESSION_ROLLOUT_INDEX_CACHE_TTL);
        if let Some(cached) = cache.get(&cache_key) {
            return Ok(cached.threads.clone());
        }
    }

    let scan_lock = {
        let mut locks = state.session_thread_cache_locks.lock().await;
        locks
            .entry(cache_key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    };
    let _scan_guard = scan_lock.lock().await;
    {
        let mut cache = state.session_thread_cache.lock().await;
        cache.retain(|_, entry| entry.created_at.elapsed() < SESSION_ROLLOUT_INDEX_CACHE_TTL);
        if let Some(cached) = cache.get(&cache_key) {
            return Ok(cached.threads.clone());
        }
    }

    let codex_home = resolve_runtime_profile(&state.config, profile_id)
        .codex_home
        .clone();
    let candidates =
        tokio::task::spawn_blocking(move || collect_rollout_candidates(&codex_home, archived))
            .await
            .map_err(|error| {
                api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to scan session rollout files: {error}"),
                )
            })?;

    let candidates = Arc::new(candidates);
    state.session_thread_cache.lock().await.insert(
        cache_key,
        CachedSessionThreads {
            created_at: Instant::now(),
            threads: candidates.clone(),
            next_cursor: String::new(),
        },
    );
    Ok(candidates)
}

pub(crate) async fn list_rollout_candidates_payload(
    state: &AppState,
    profile_id: &str,
    archived: bool,
) -> ApiResult<Vec<Value>> {
    Ok(
        list_rollout_candidates_shared_payload(state, profile_id, archived)
            .await?
            .as_ref()
            .clone(),
    )
}

pub(crate) async fn read_rollout_thread_metadata_from_candidate(
    candidate: &Value,
    archived: bool,
) -> ApiResult<Option<Value>> {
    let Some(path) = candidate.get("path").and_then(Value::as_str) else {
        return Ok(None);
    };
    let modified_at = candidate
        .get("updatedAt")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let path_buf = PathBuf::from(path);
    let candidate = candidate.clone();
    let thread = tokio::task::spawn_blocking(move || {
        read_rollout_thread_metadata_from_path(&path_buf, archived, modified_at)
    })
    .await
    .map_err(|error| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read session rollout metadata: {error}"),
        )
    })?;
    Ok(thread.map(|mut thread| {
        merge_candidate_metadata_into_thread(&mut thread, &candidate, archived);
        thread
    }))
}

pub(crate) async fn rollout_candidate_contains_query_payload(
    candidate: &Value,
    needle: &str,
) -> ApiResult<bool> {
    let Some(path) = candidate.get("path").and_then(Value::as_str) else {
        return Ok(false);
    };
    let path_buf = PathBuf::from(path);
    let needle = needle.to_string();
    tokio::task::spawn_blocking(move || rollout_file_contains_query(&path_buf, &needle))
        .await
        .map_err(|error| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to search the session rollout file: {error}"),
            )
        })
}

pub(crate) async fn read_rollout_thread_metadata_by_session_id(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<Option<Value>> {
    for archived in [false, true] {
        let candidates = list_rollout_candidates_payload(state, profile_id, archived).await?;
        for candidate in candidates {
            if candidate.get("id").and_then(Value::as_str) != Some(session_id) {
                continue;
            }
            if let Some(thread) =
                read_rollout_thread_metadata_from_candidate(&candidate, archived).await?
            {
                return Ok(Some(thread));
            }
        }
    }
    Ok(None)
}

pub(crate) async fn read_state_thread_metadata_by_session_id(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    archived_hint: Option<bool>,
) -> ApiResult<Option<Value>> {
    let codex_home = resolve_runtime_profile(&state.config, profile_id)
        .codex_home
        .clone();
    let requested_session_id = session_id.to_string();
    let worker_session_id = requested_session_id.clone();
    let metadata = tokio::task::spawn_blocking(move || {
        read_state_thread_metadata_rows_from_codex_home(
            &codex_home,
            std::slice::from_ref(&worker_session_id),
        )
    })
    .await
    .map_err(|error| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read session state database metadata: {error}"),
        )
    })?;
    let mut thread = match metadata {
        Ok(mut metadata) => metadata.remove(requested_session_id.as_str()),
        Err(error) => {
            warn!(
                profile_id,
                session_id = requested_session_id.as_str(),
                "{error}"
            );
            None
        }
    };
    if let Some(thread) = thread.as_mut() {
        merge_candidate_metadata_into_thread(
            thread,
            &json!({
                "id": requested_session_id,
                "updatedAt": thread.get("updatedAt").cloned().unwrap_or_else(|| json!(0)),
                "indexedUpdatedAt": thread.get("updatedAt").cloned().unwrap_or_else(|| json!(0)),
                "indexedName": thread.get("name").cloned().unwrap_or(Value::Null)
            }),
            archived_hint.unwrap_or_else(|| {
                thread
                    .get("archived")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            }),
        );
    }
    Ok(thread)
}

pub(crate) async fn hydrate_rollout_candidates_to_threads_payload(
    state: &AppState,
    profile_id: &str,
    archived: bool,
    candidates: &[Value],
) -> ApiResult<Vec<Value>> {
    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    let codex_home = resolve_runtime_profile(&state.config, profile_id)
        .codex_home
        .clone();
    let session_ids = candidates
        .iter()
        .filter_map(|candidate| candidate.get("id").and_then(Value::as_str))
        .map(str::to_string)
        .collect::<Vec<_>>();
    let state_metadata = tokio::task::spawn_blocking(move || {
        read_state_thread_metadata_rows_from_codex_home(&codex_home, &session_ids)
    })
    .await
    .map_err(|error| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to hydrate session metadata from state database: {error}"),
        )
    })?;
    let mut metadata_by_id = match state_metadata {
        Ok(metadata_by_id) => metadata_by_id,
        Err(error) => {
            warn!(profile_id, "{error}");
            HashMap::new()
        }
    };

    let mut threads_by_id = HashMap::new();
    let mut fallback_candidates = Vec::new();
    for candidate in candidates {
        let session_id = candidate
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut thread = metadata_by_id.remove(session_id);
        if let Some(payload) = thread.as_mut() {
            merge_candidate_metadata_into_thread(payload, candidate, archived);
        } else {
            fallback_candidates.push(candidate.clone());
            continue;
        }
        if let Some(mut payload) = thread {
            merge_candidate_metadata_into_thread(&mut payload, candidate, archived);
            threads_by_id.insert(session_id.to_string(), payload);
        }
    }

    let fallback_threads = tokio::task::spawn_blocking(move || {
        let mut metadata_by_id = HashMap::new();
        for candidate in fallback_candidates {
            let Some(path) = candidate.get("path").and_then(Value::as_str) else {
                continue;
            };
            let modified_at = candidate
                .get("updatedAt")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            let Some(mut thread) =
                read_rollout_thread_metadata_from_path(&PathBuf::from(path), archived, modified_at)
            else {
                continue;
            };
            merge_candidate_metadata_into_thread(&mut thread, &candidate, archived);
            let Some(session_id) = thread.get("id").and_then(Value::as_str).map(str::to_string)
            else {
                continue;
            };
            metadata_by_id.insert(session_id, thread);
        }
        metadata_by_id
    })
    .await
    .map_err(|error| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to batch hydrate session rollout metadata: {error}"),
        )
    })?;
    threads_by_id.extend(fallback_threads);

    let mut threads = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let Some(session_id) = candidate.get("id").and_then(Value::as_str) else {
            continue;
        };
        if let Some(thread) = threads_by_id.remove(session_id) {
            threads.push(thread);
        }
    }
    Ok(threads)
}

pub(crate) async fn search_state_thread_ids_payload(
    state: &AppState,
    profile_id: &str,
    archived: bool,
    needle: &str,
) -> ApiResult<Option<HashSet<String>>> {
    let codex_home = resolve_runtime_profile(&state.config, profile_id)
        .codex_home
        .clone();
    let like_pattern = format!("%{}%", needle.trim().to_lowercase());
    let query_result =
        tokio::task::spawn_blocking(move || -> Result<Option<HashSet<String>>, String> {
            let database_path = state_database_path(&codex_home);
            if !database_path.is_file() {
                return Ok(None);
            }
            let connection =
                Connection::open_with_flags(database_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                    .map_err(|error| format!("failed to open state database: {error}"))?;
            let mut statement = connection
                .prepare(
                    "SELECT id FROM threads \
                WHERE archived = ?1 AND (\
                    lower(title) LIKE ?2 \
                    OR lower(first_user_message) LIKE ?2 \
                    OR lower(cwd) LIKE ?2 \
                    OR lower(COALESCE(agent_nickname, '')) LIKE ?2 \
                    OR lower(COALESCE(agent_role, '')) LIKE ?2\
                )",
                )
                .map_err(|error| format!("failed to prepare search query: {error}"))?;
            let rows = statement
                .query_map(
                    (if archived { 1_i64 } else { 0_i64 }, like_pattern),
                    |row| row.get::<_, String>(0),
                )
                .map_err(|error| format!("failed to execute search query: {error}"))?;
            let mut matches = HashSet::new();
            for row in rows {
                matches
                    .insert(row.map_err(|error| format!("failed to decode search row: {error}"))?);
            }
            Ok(Some(matches))
        })
        .await
        .map_err(|error| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to search session metadata from state database: {error}"),
            )
        })?;

    match query_result {
        Ok(matched_ids) => Ok(matched_ids),
        Err(error) => {
            warn!(profile_id, "{error}");
            Ok(None)
        }
    }
}
