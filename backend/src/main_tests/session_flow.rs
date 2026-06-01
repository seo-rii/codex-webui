use super::*;
use rusqlite::{Connection, params};

fn write_rollout_fixture(
    codex_home: &Path,
    archived: bool,
    date_path: &str,
    timestamp_prefix: &str,
    session_id: &str,
    cwd: &Path,
    prompt: &str,
    extra_lines: &[&str],
    session_meta_extra: Option<&str>,
) {
    let directory = if archived {
        codex_home.join("archived_sessions")
    } else {
        codex_home.join("sessions").join(date_path)
    };
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("rollout-{timestamp_prefix}-{session_id}.jsonl"));
    let session_meta_extra = session_meta_extra.unwrap_or("");
    let content = format!(
        "{{\"timestamp\":\"2026-04-24T01:00:00.000Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"{session_id}\",\"timestamp\":\"2026-04-24T01:00:00.000Z\",\"cwd\":\"{}\",\"originator\":\"codex_webui\",\"cli_version\":\"0.121.0\",\"source\":\"vscode\"{session_meta_extra}}}}}\n{{\"timestamp\":\"2026-04-24T01:00:01.000Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"<environment_context>\\n  <cwd>{}</cwd>\\n</environment_context>\"}}]}}}}\n{{\"timestamp\":\"2026-04-24T01:00:02.000Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":{prompt:?},\"kind\":\"plain\"}}}}\n{}\n",
        cwd.display(),
        cwd.display(),
        extra_lines.join("\n")
    );
    fs::write(path, content).unwrap();
    std::thread::sleep(Duration::from_millis(8));
}

fn append_session_index_fixture(
    codex_home: &Path,
    session_id: &str,
    title: &str,
    updated_at: &str,
) {
    let path = codex_home.join("session_index.jsonl");
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    use std::io::Write;
    writeln!(
        file,
        "{}",
        json!({
            "id": session_id,
            "thread_name": title,
            "updated_at": updated_at
        })
    )
    .unwrap();
}

fn write_state_thread_fixture(
    codex_home: &Path,
    session_id: &str,
    title: &str,
    preview: &str,
    cwd: &Path,
    archived: bool,
    created_at_ms: i64,
    updated_at_ms: i64,
    agent_nickname: Option<&str>,
    agent_role: Option<&str>,
    is_subagent: bool,
) {
    let database_path = codex_home.join("state_5.sqlite");
    let connection = Connection::open(database_path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS threads (
                id TEXT PRIMARY KEY,
                rollout_path TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                source TEXT NOT NULL,
                model_provider TEXT NOT NULL,
                cwd TEXT NOT NULL,
                title TEXT NOT NULL,
                sandbox_policy TEXT NOT NULL,
                approval_mode TEXT NOT NULL,
                tokens_used INTEGER NOT NULL DEFAULT 0,
                has_user_event INTEGER NOT NULL DEFAULT 0,
                archived INTEGER NOT NULL DEFAULT 0,
                archived_at INTEGER,
                git_sha TEXT,
                git_branch TEXT,
                git_origin_url TEXT,
                cli_version TEXT NOT NULL DEFAULT '',
                first_user_message TEXT NOT NULL DEFAULT '',
                agent_nickname TEXT,
                agent_role TEXT,
                memory_mode TEXT NOT NULL DEFAULT 'enabled',
                model TEXT,
                reasoning_effort TEXT,
                agent_path TEXT,
                created_at_ms INTEGER,
                updated_at_ms INTEGER
            );
            CREATE TABLE IF NOT EXISTS thread_spawn_edges (
                parent_thread_id TEXT NOT NULL,
                child_thread_id TEXT NOT NULL PRIMARY KEY,
                status TEXT NOT NULL
            );",
        )
        .unwrap();
    connection
        .execute(
            "INSERT OR REPLACE INTO threads (
                id, rollout_path, created_at, updated_at, source, model_provider, cwd, title,
                sandbox_policy, approval_mode, tokens_used, has_user_event, archived, archived_at,
                git_sha, git_branch, git_origin_url, cli_version, first_user_message,
                agent_nickname, agent_role, memory_mode, model, reasoning_effort, agent_path,
                created_at_ms, updated_at_ms
            ) VALUES (
                ?1, ?2, ?3, ?4, 'vscode', 'openai', ?5, ?6, ?7,
                'on-request', 0, 1, ?8, NULL, NULL, NULL, NULL, '0.121.0', ?9, ?10, ?11,
                'enabled', NULL, NULL, NULL, ?12, ?13
            )",
            params![
                session_id,
                format!("/tmp/{session_id}.jsonl"),
                created_at_ms / 1000,
                updated_at_ms / 1000,
                cwd.display().to_string(),
                title,
                r#"{"type":"workspace-write"}"#,
                if archived { 1 } else { 0 },
                preview,
                agent_nickname,
                agent_role,
                created_at_ms,
                updated_at_ms
            ],
        )
        .unwrap();
    connection
        .execute(
            "DELETE FROM thread_spawn_edges WHERE child_thread_id = ?1",
            params![session_id],
        )
        .unwrap();
    if is_subagent {
        connection
            .execute(
                "INSERT OR REPLACE INTO thread_spawn_edges (parent_thread_id, child_thread_id, status)
                VALUES ('parent-thread', ?1, 'running')",
                params![session_id],
            )
            .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parser_diagnostics_reports_native_mismatches_without_hot_path_listing() {
    let sandbox = unique_test_dir("parser-diagnostics-mismatch");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state_with_fake_app_server(
        workspace.clone(),
        vec![workspace.clone()],
        codex_home.clone(),
    );
    let session_id = "019e0000-0000-7000-8000-00000000d1a6";
    write_rollout_fixture(
        &codex_home,
        false,
        "2026/04/24",
        "2026-04-24T01-11-00",
        session_id,
        &workspace,
        "local parser prompt",
        &[],
        None,
    );
    write_state_thread_fixture(
        &codex_home,
        session_id,
        "Local parser title",
        "local parser prompt",
        &workspace,
        false,
        1_777_000_000_000,
        1_777_000_001_000,
        None,
        None,
        false,
    );

    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": session_id,
                    "name": "Native Codex title",
                    "preview": "native preview",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1_777_000_002_000_i64,
                    "updatedAt": 1_777_000_003_000_i64,
                    "status": "idle",
                    "isSubagent": false,
                    "turns": [
                        {
                            "id": "native-turn-1",
                            "status": "completed",
                            "items": [
                                {
                                    "id": "native-turn-1:user:0",
                                    "type": "userMessage",
                                    "text": "native prompt"
                                },
                                {
                                    "id": "native-turn-1:agent:0",
                                    "type": "agentMessage",
                                    "text": "native answer"
                                }
                            ]
                        }
                    ],
                    "goal": {
                        "threadId": session_id,
                        "objective": "native goal",
                        "status": "active",
                        "tokensUsed": 7
                    }
                }
            }),
        )
        .await
        .unwrap();

    let payload = compare_parser_with_native_session_payload(&state, "default", session_id, 5)
        .await
        .unwrap();

    assert_eq!(payload.get("ok").and_then(Value::as_bool), Some(false));
    assert!(
        payload
            .get("mismatchCount")
            .and_then(Value::as_u64)
            .is_some_and(|count| count > 0)
    );
    let mismatches = payload
        .get("mismatches")
        .and_then(Value::as_array)
        .expect("mismatches should be an array");
    assert!(mismatches.iter().any(|entry| {
        entry.get("category").and_then(Value::as_str) == Some("summary")
            && entry.get("field").and_then(Value::as_str) == Some("name")
    }));
    assert!(mismatches.iter().any(|entry| {
        entry.get("category").and_then(Value::as_str) == Some("recentTurns")
            && entry.get("field").and_then(Value::as_str) == Some("turns")
    }));
    assert_eq!(
        payload
            .get("local")
            .and_then(|local| local.get("available"))
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        payload
            .get("native")
            .and_then(|native| native.get("available"))
            .and_then(Value::as_bool),
        Some(true)
    );

    let thread_list = app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "debug/requestCount",
            json!({
                "target": "thread/list"
            }),
        )
        .await
        .unwrap();
    assert_eq!(thread_list.get("count").and_then(Value::as_u64), Some(0));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_session_payload_uses_app_server_and_persists_preferences() {
    let sandbox = unique_test_dir("session-create-rust");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let created = create_session_payload(
        &state,
        "default",
        json!({
            "cwd": workspace.display().to_string(),
            "model": "gpt-5.4",
            "approvalPolicy": "on-request",
            "sandboxMode": "workspace-write"
        }),
        None,
        Some("Review docs"),
    )
    .await
    .unwrap();

    assert_eq!(
        created.get("name").and_then(Value::as_str),
        Some("Review docs")
    );
    let session_id = created
        .get("id")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();

    let stored_preferences = with_ui_state_read(&state, "default", |ui_state| {
        Ok(ui_state
            .get("preferencesByThreadId")
            .and_then(Value::as_object)
            .and_then(|entries| entries.get(&session_id))
            .cloned()
            .unwrap_or(Value::Null))
    })
    .await
    .unwrap();
    assert_eq!(
        stored_preferences.get("model").and_then(Value::as_str),
        Some("gpt-5.4")
    );

    let thread = app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/read",
            json!({
                "threadId": session_id,
                "includeTurns": false
            }),
        )
        .await
        .unwrap();
    assert_eq!(
        thread
            .get("thread")
            .and_then(|value| value.get("name"))
            .and_then(Value::as_str),
        Some("Review docs")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rollout_file_listing_and_update_avoid_app_server_thread_lists() {
    let sandbox = unique_test_dir("session-rollout-listing");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state_with_fake_app_server(
        workspace.clone(),
        vec![workspace.clone()],
        codex_home.clone(),
    );
    let visible_id = "019e0000-0000-7000-8000-000000000001";
    let subagent_id = "019e0000-0000-7000-8000-000000000002";
    let updated_id = "019e0000-0000-7000-8000-000000000003";

    write_rollout_fixture(
        &codex_home,
        false,
        "2026/04/24",
        "2026-04-24T01-00-00",
        visible_id,
        &workspace,
        "Implement compact session indexing",
        &[],
        None,
    );
    write_rollout_fixture(
        &codex_home,
        false,
        "2026/04/24",
        "2026-04-24T01-00-10",
        subagent_id,
        &workspace,
        "Hidden subagent prompt",
        &[],
        Some(
            ",\"source\":{\"subagent\":{\"thread_spawn\":{\"parent_thread_id\":\"parent\",\"depth\":1,\"agent_nickname\":\"Turing\",\"agent_role\":\"explorer\"}}},\"agent_nickname\":\"Turing\",\"agent_role\":\"explorer\"",
        ),
    );

    let payload = list_sessions_payload(
        &state,
        "default",
        false,
        None,
        20,
        &SessionFilterCriteria::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        payload
            .get("sessions")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        payload
            .get("sessions")
            .and_then(Value::as_array)
            .and_then(|sessions| sessions.first())
            .and_then(|session| session.get("id"))
            .and_then(Value::as_str),
        Some(visible_id)
    );

    let client = app_server_client(&state, "default").await.unwrap();
    let thread_list_count = client
        .request("debug/requestCount", json!({ "target": "thread/list" }))
        .await
        .unwrap();
    assert_eq!(
        thread_list_count.get("count").and_then(Value::as_i64),
        Some(0)
    );

    write_rollout_fixture(
        &codex_home,
        false,
        "2026/04/24",
        "2026-04-24T01-00-20",
        updated_id,
        &workspace,
        "Newest session after refresh",
        &[],
        None,
    );
    invalidate_session_lists(&state, "default").await;

    let refreshed = list_sessions_payload(
        &state,
        "default",
        false,
        None,
        20,
        &SessionFilterCriteria::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        refreshed
            .get("sessions")
            .and_then(Value::as_array)
            .and_then(|sessions| sessions.first())
            .and_then(|session| session.get("id"))
            .and_then(Value::as_str),
        Some(updated_id)
    );

    let thread_list_count_after = client
        .request("debug/requestCount", json!({ "target": "thread/list" }))
        .await
        .unwrap();
    assert_eq!(
        thread_list_count_after.get("count").and_then(Value::as_i64),
        Some(0)
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn app_server_listing_filters_projected_state_db_subagents() {
    let sandbox = unique_test_dir("session-app-server-subagent-listing");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state_with_fake_app_server(
        workspace.clone(),
        vec![workspace.clone()],
        codex_home.clone(),
    );
    let visible_id = "019e0000-0000-7000-8000-000000000011";
    let subagent_id = "019e0000-0000-7000-8000-000000000012";
    let exec_id = "019e0000-0000-7000-8000-000000000013";
    let workspace_path = workspace.display().to_string();
    let client = app_server_client(&state, "default").await.unwrap();
    client
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": visible_id,
                    "name": "Visible parent session",
                    "preview": "parent prompt",
                    "cwd": workspace_path,
                    "archived": false,
                    "createdAt": 1_713_920_000_000i64,
                    "updatedAt": 1_713_920_010_000i64,
                    "status": "completed"
                }
            }),
        )
        .await
        .unwrap();
    client
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": subagent_id,
                    "name": "Hidden worker session",
                    "preview": "subagent prompt",
                    "cwd": workspace_path,
                    "archived": false,
                    "createdAt": 1_713_920_001_000i64,
                    "updatedAt": 1_713_920_020_000i64,
                    "status": "completed",
                    "spawned_subagent": 1,
                    "agent_nickname": "Turing",
                    "agent_role": "explorer"
                }
            }),
        )
        .await
        .unwrap();
    client
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": exec_id,
                    "name": "Hidden exec worker session",
                    "preview": "exec prompt",
                    "source": "exec",
                    "cwd": workspace_path,
                    "archived": false,
                    "createdAt": 1_713_920_002_000i64,
                    "updatedAt": 1_713_920_030_000i64,
                    "status": "completed"
                }
            }),
        )
        .await
        .unwrap();

    let projected = project_thread_listing_payload(&json!({
        "id": subagent_id,
        "spawned_subagent": 1,
        "agent_nickname": "Turing",
        "agent_role": "explorer"
    }));
    assert!(thread_is_subagent(&projected));
    let exec_projected = project_thread_listing_payload(&json!({
        "id": exec_id,
        "source": "exec"
    }));
    assert!(thread_is_subagent(&exec_projected));

    let payload = list_sessions_payload(
        &state,
        "default",
        false,
        None,
        20,
        &SessionFilterCriteria::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        payload
            .get("sessions")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        payload
            .get("sessions")
            .and_then(Value::as_array)
            .and_then(|sessions| sessions.first())
            .and_then(|session| session.get("id"))
            .and_then(Value::as_str),
        Some(visible_id)
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rollout_file_listing_promotes_old_pinned_and_running_sessions() {
    let sandbox = unique_test_dir("session-rollout-priority");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state_with_fake_app_server(
        workspace.clone(),
        vec![workspace.clone()],
        codex_home.clone(),
    );
    app_server_client(&state, "default")
        .await
        .unwrap()
        .request("model/list", json!({}))
        .await
        .unwrap();
    let pinned_id = "019e0000-0000-7000-8000-000000000011";
    let running_id = "019e0000-0000-7000-8000-000000000012";
    let newest_id = "019e0000-0000-7000-8000-000000000013";

    write_rollout_fixture(
        &codex_home,
        false,
        "2026/04/24",
        "2026-04-24T01-05-00",
        pinned_id,
        &workspace,
        "Old pinned session",
        &[],
        None,
    );
    write_rollout_fixture(
        &codex_home,
        false,
        "2026/04/24",
        "2026-04-24T01-05-10",
        running_id,
        &workspace,
        "Old running session",
        &[],
        None,
    );
    write_rollout_fixture(
        &codex_home,
        false,
        "2026/04/24",
        "2026-04-24T01-05-20",
        newest_id,
        &workspace,
        "Newest normal session",
        &[],
        None,
    );

    with_ui_state_write(&state, "default", |ui_state| {
        let Some(session_meta) = ui_state
            .get_mut("sessionMetaByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "session meta state is missing",
            ));
        };
        session_meta.insert(
            pinned_id.to_string(),
            json!({
                "pinned": true,
                "tags": []
            }),
        );
        Ok(())
    })
    .await
    .unwrap();
    state.active_turns.lock().await.insert(
        runtime_session_key("default", running_id),
        "turn-running".to_string(),
    );

    let payload = list_sessions_payload(
        &state,
        "default",
        false,
        None,
        2,
        &SessionFilterCriteria::default(),
    )
    .await
    .unwrap();
    let session_ids = payload
        .get("sessions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|session| {
            session
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    assert_eq!(session_ids, vec![pinned_id, running_id]);

    let next_page = list_sessions_payload(
        &state,
        "default",
        false,
        payload.get("nextCursor").and_then(Value::as_str),
        2,
        &SessionFilterCriteria::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        next_page
            .get("sessions")
            .and_then(Value::as_array)
            .and_then(|sessions| sessions.first())
            .and_then(|session| session.get("id"))
            .and_then(Value::as_str),
        Some(newest_id)
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rollout_file_listing_prefers_recorded_thread_name_metadata() {
    let sandbox = unique_test_dir("session-rollout-title");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state_with_fake_app_server(
        workspace.clone(),
        vec![workspace.clone()],
        codex_home.clone(),
    );
    let titled_id = "019e0000-0000-7000-8000-000000000021";
    let mut extra_lines = (0..220)
        .map(|index| {
            format!(
                "{{\"timestamp\":\"2026-04-24T01:10:{:02}.000Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"agent_message\",\"message\":\"filler {index}\",\"phase\":\"commentary\"}}}}",
                index % 60
            )
        })
        .collect::<Vec<_>>();
    extra_lines.push(format!(
        "{{\"timestamp\":\"2026-04-24T01:14:30.000Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"thread_name_updated\",\"thread_id\":\"{titled_id}\",\"thread_name\":\"AI generated rollout title\"}}}}"
    ));
    let extra_line_refs = extra_lines.iter().map(String::as_str).collect::<Vec<_>>();

    write_rollout_fixture(
        &codex_home,
        false,
        "2026/04/24",
        "2026-04-24T01-10-00",
        titled_id,
        &workspace,
        "Preview fallback title that should not win",
        &extra_line_refs,
        None,
    );

    let payload = list_sessions_payload(
        &state,
        "default",
        false,
        None,
        20,
        &SessionFilterCriteria::default(),
    )
    .await
    .unwrap();
    let session = payload
        .get("sessions")
        .and_then(Value::as_array)
        .and_then(|sessions| sessions.first())
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        session.get("name").and_then(Value::as_str),
        Some("AI generated rollout title")
    );
    assert_eq!(session.get("id").and_then(Value::as_str), Some(titled_id));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rollout_file_listing_hydrates_visible_entries_from_state_metadata() {
    let sandbox = unique_test_dir("session-rollout-hydration");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state_with_fake_app_server(
        workspace.clone(),
        vec![workspace.clone()],
        codex_home.clone(),
    );
    let session_id = "019e0000-0000-7000-8000-000000000031";

    write_rollout_fixture(
        &codex_home,
        false,
        "2026/04/24",
        "2026-04-24T01-20-00",
        session_id,
        &workspace,
        "rollout preview fallback",
        &[],
        None,
    );
    append_session_index_fixture(
        &codex_home,
        session_id,
        "Indexed title should be replaced by state title",
        "2026-04-24T01:20:05.000Z",
    );
    write_state_thread_fixture(
        &codex_home,
        session_id,
        "Hydrated state title",
        "Hydrated preview from state database",
        &workspace.join("nested-project"),
        false,
        1_713_920_000_000,
        1_713_920_005_000,
        None,
        None,
        false,
    );

    let payload = list_sessions_payload(
        &state,
        "default",
        false,
        None,
        20,
        &SessionFilterCriteria::default(),
    )
    .await
    .unwrap();
    let session = payload
        .get("sessions")
        .and_then(Value::as_array)
        .and_then(|sessions| sessions.first())
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        session.get("name").and_then(Value::as_str),
        Some("Hydrated state title")
    );
    assert_eq!(
        session.get("preview").and_then(Value::as_str),
        Some("Hydrated preview from state database")
    );
    assert_eq!(
        session.get("cwd").and_then(Value::as_str),
        Some(workspace.join("nested-project").to_str().unwrap())
    );
    assert_eq!(
        session.get("isSubagent").and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(session.get("agentNickname"), Some(&Value::Null));
    assert_eq!(session.get("agentRole"), Some(&Value::Null));
    assert_eq!(
        session.get("updatedAt").and_then(Value::as_i64),
        Some(1_713_920_005_000)
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rollout_file_listing_batch_hydrates_visible_entries_without_state_metadata() {
    let sandbox = unique_test_dir("session-rollout-batch-hydration");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state_with_fake_app_server(
        workspace.clone(),
        vec![workspace.clone()],
        codex_home.clone(),
    );
    let sessions = [
        (
            "019e0000-0000-7000-8000-000000000032",
            "Batch title one",
            "first fallback prompt",
            "2026-04-24T01:21:01.000Z",
        ),
        (
            "019e0000-0000-7000-8000-000000000033",
            "Batch title two",
            "second fallback prompt",
            "2026-04-24T01:21:02.000Z",
        ),
        (
            "019e0000-0000-7000-8000-000000000034",
            "Batch title three",
            "third fallback prompt",
            "2026-04-24T01:21:03.000Z",
        ),
    ];
    for (index, (session_id, title, prompt, updated_at)) in sessions.iter().enumerate() {
        write_rollout_fixture(
            &codex_home,
            false,
            "2026/04/24",
            &format!("2026-04-24T01-21-0{index}"),
            session_id,
            &workspace,
            prompt,
            &[],
            None,
        );
        append_session_index_fixture(&codex_home, session_id, title, updated_at);
    }

    let payload = list_sessions_payload(
        &state,
        "default",
        false,
        None,
        20,
        &SessionFilterCriteria::default(),
    )
    .await
    .unwrap();
    let listed = payload
        .get("sessions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let listed_ids = listed
        .iter()
        .map(|session| {
            session
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        listed_ids,
        vec![
            "019e0000-0000-7000-8000-000000000034",
            "019e0000-0000-7000-8000-000000000033",
            "019e0000-0000-7000-8000-000000000032"
        ]
    );
    assert_eq!(
        listed
            .get(1)
            .and_then(|session| session.get("name"))
            .and_then(Value::as_str),
        Some("Batch title two")
    );
    assert_eq!(
        listed
            .get(1)
            .and_then(|session| session.get("preview"))
            .and_then(Value::as_str),
        Some("second fallback prompt")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rollout_file_listing_filters_state_db_source_only_subagents() {
    let sandbox = unique_test_dir("session-state-source-subagent");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state_with_fake_app_server(
        workspace.clone(),
        vec![workspace.clone()],
        codex_home.clone(),
    );
    let visible_id = "019e0000-0000-7000-8000-000000000035";
    let subagent_id = "019e0000-0000-7000-8000-000000000036";
    let exec_state_id = "019e0000-0000-7000-8000-000000000037";
    let exec_rollout_id = "019e0000-0000-7000-8000-000000000038";

    write_rollout_fixture(
        &codex_home,
        false,
        "2026/04/24",
        "2026-04-24T01-22-00",
        visible_id,
        &workspace,
        "visible rollout prompt",
        &[],
        None,
    );
    write_state_thread_fixture(
        &codex_home,
        visible_id,
        "Visible state title",
        "Visible state preview",
        &workspace,
        false,
        1_713_920_020_000,
        1_713_920_020_000,
        None,
        None,
        false,
    );
    write_rollout_fixture(
        &codex_home,
        false,
        "2026/04/24",
        "2026-04-24T01-22-10",
        subagent_id,
        &workspace,
        "source-only subagent prompt",
        &[],
        None,
    );
    write_state_thread_fixture(
        &codex_home,
        subagent_id,
        "Hidden source subagent",
        "Hidden source preview",
        &workspace,
        false,
        1_713_920_021_000,
        1_713_920_030_000,
        None,
        None,
        false,
    );
    let connection = Connection::open(codex_home.join("state_5.sqlite")).unwrap();
    connection
        .execute(
            "UPDATE threads SET source = ?2 WHERE id = ?1",
            params![
                subagent_id,
                json!({
                    "subagent": {
                        "thread_spawn": {
                            "parent_thread_id": visible_id,
                            "depth": 1,
                            "agent_nickname": "Turing",
                            "agent_role": "explorer"
                        }
                    }
                })
                .to_string()
            ],
        )
        .unwrap();
    write_rollout_fixture(
        &codex_home,
        false,
        "2026/04/24",
        "2026-04-24T01-22-20",
        exec_state_id,
        &workspace,
        "exec source state prompt",
        &[],
        None,
    );
    write_state_thread_fixture(
        &codex_home,
        exec_state_id,
        "Hidden exec state session",
        "Hidden exec state preview",
        &workspace,
        false,
        1_713_920_022_000,
        1_713_920_031_000,
        None,
        None,
        false,
    );
    connection
        .execute(
            "UPDATE threads SET source = 'exec' WHERE id = ?1",
            params![exec_state_id],
        )
        .unwrap();
    write_rollout_fixture(
        &codex_home,
        false,
        "2026/04/24",
        "2026-04-24T01-22-30",
        exec_rollout_id,
        &workspace,
        "exec source rollout prompt",
        &[],
        Some(",\"source\":\"exec\""),
    );

    let payload = list_sessions_payload(
        &state,
        "default",
        false,
        None,
        20,
        &SessionFilterCriteria::default(),
    )
    .await
    .unwrap();
    let sessions = payload
        .get("sessions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(sessions.len(), 1);
    assert_eq!(
        sessions
            .first()
            .and_then(|session| session.get("id"))
            .and_then(Value::as_str),
        Some(visible_id)
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_detail_hydrates_visible_title_from_state_metadata() {
    let sandbox = unique_test_dir("session-detail-state-title");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state_with_fake_app_server(
        workspace.clone(),
        vec![workspace.clone()],
        codex_home.clone(),
    );
    let session_id = "019e0000-0000-7000-8000-000000000032";

    write_rollout_fixture(
        &codex_home,
        false,
        "2026/04/24",
        "2026-04-24T01-21-00",
        session_id,
        &workspace,
        "first prompt fallback title",
        &[],
        None,
    );
    write_state_thread_fixture(
        &codex_home,
        session_id,
        "AI generated state title",
        "State preview",
        &workspace,
        false,
        1_713_920_000_000,
        1_713_920_006_000,
        None,
        None,
        false,
    );

    let detail = session_detail_payload(&state, "default", session_id, 20)
        .await
        .unwrap();
    assert_eq!(
        detail
            .get("thread")
            .and_then(|thread| thread.get("name"))
            .and_then(Value::as_str),
        Some("AI generated state title")
    );
    assert_eq!(
        detail
            .get("thread")
            .and_then(|thread| thread.get("preview"))
            .and_then(Value::as_str),
        Some("State preview")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rollout_file_search_and_archived_listing_use_file_index() {
    let sandbox = unique_test_dir("session-rollout-search");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state_with_fake_app_server(
        workspace.clone(),
        vec![workspace.clone()],
        codex_home.clone(),
    );
    let active_id = "019e0000-0000-7000-8000-000000000011";
    let archived_id = "019e0000-0000-7000-8000-000000000012";

    write_rollout_fixture(
        &codex_home,
        false,
        "2026/04/24",
        "2026-04-24T02-00-00",
        active_id,
        &workspace,
        "Investigate fallback behavior",
        &[
            r#"{"timestamp":"2026-04-24T02:00:03.000Z","type":"event_msg","payload":{"type":"agent_message","message":"The zebra-token regression reproduces in rollout indexing.","phase":"commentary"}}"#,
        ],
        None,
    );
    write_rollout_fixture(
        &codex_home,
        true,
        "",
        "2026-04-23T23-30-00",
        archived_id,
        &workspace,
        "Archived rollout thread",
        &[],
        None,
    );

    let matched = search_sessions_payload(
        &state,
        "default",
        "zebra-token",
        "full",
        false,
        None,
        20,
        &SessionFilterCriteria::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        matched
            .get("sessions")
            .and_then(Value::as_array)
            .and_then(|sessions| sessions.first())
            .and_then(|session| session.get("id"))
            .and_then(Value::as_str),
        Some(active_id)
    );

    let archived = list_sessions_payload(
        &state,
        "default",
        true,
        None,
        20,
        &SessionFilterCriteria::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        archived
            .get("sessions")
            .and_then(Value::as_array)
            .and_then(|sessions| sessions.first())
            .and_then(|session| session.get("id"))
            .and_then(Value::as_str),
        Some(archived_id)
    );

    let client = app_server_client(&state, "default").await.unwrap();
    let thread_list_count = client
        .request("debug/requestCount", json!({ "target": "thread/list" }))
        .await
        .unwrap();
    assert_eq!(
        thread_list_count.get("count").and_then(Value::as_i64),
        Some(0)
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rollout_file_search_uses_state_metadata_before_full_rollout_scan() {
    let sandbox = unique_test_dir("session-rollout-search-state");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state_with_fake_app_server(
        workspace.clone(),
        vec![workspace.clone()],
        codex_home.clone(),
    );
    let session_id = "019e0000-0000-7000-8000-000000000041";
    write_rollout_fixture(
        &codex_home,
        false,
        "2026/04/24",
        "2026-04-24T02-30-00",
        session_id,
        &workspace,
        "rollout text without target phrase",
        &[],
        None,
    );
    write_state_thread_fixture(
        &codex_home,
        session_id,
        "State indexed title",
        "Needle phrase from the state database only",
        &workspace,
        false,
        1_713_920_100_000,
        1_713_920_110_000,
        None,
        None,
        false,
    );

    let matched = search_sessions_payload(
        &state,
        "default",
        "needle phrase",
        "summary",
        false,
        None,
        20,
        &SessionFilterCriteria::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        matched
            .get("sessions")
            .and_then(Value::as_array)
            .and_then(|sessions| sessions.first())
            .and_then(|session| session.get("id"))
            .and_then(Value::as_str),
        Some(session_id)
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rust_session_list_and_search_use_app_server_threads() {
    let sandbox = unique_test_dir("session-list-rust");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let first = create_session_payload(
        &state,
        "default",
        json!({ "cwd": workspace.display().to_string() }),
        None,
        Some("Build Docs"),
    )
    .await
    .unwrap();
    let second = create_session_payload(
        &state,
        "default",
        json!({ "cwd": workspace.display().to_string() }),
        None,
        Some("Fix Queue"),
    )
    .await
    .unwrap();
    let first_id = first.get("id").and_then(Value::as_str).unwrap().to_string();
    let second_id = second
        .get("id")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();

    with_ui_state_write(&state, "default", |ui_state| {
        let Some(session_meta) = ui_state
            .get_mut("sessionMetaByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "session meta state is missing",
            ));
        };
        session_meta.insert(
            first_id.clone(),
            json!({
                "pinned": true,
                "tags": ["docs"]
            }),
        );
        let Some(queues) = ui_state
            .get_mut("queuesByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "queue state is missing",
            ));
        };
        queues.insert(
            second_id.clone(),
            json!({
                "items": [
                    {
                        "id": "queue-1",
                        "prompt": "follow up"
                    }
                ],
                "resumePending": false,
                "updatedAt": 10
            }),
        );
        Ok(())
    })
    .await
    .unwrap();

    let pinned_only = list_sessions_payload(
        &state,
        "default",
        false,
        None,
        20,
        &SessionFilterCriteria {
            pinned_only: true,
            ..SessionFilterCriteria::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        pinned_only
            .get("sessions")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        pinned_only
            .get("sessions")
            .and_then(Value::as_array)
            .and_then(|entries| entries.first())
            .and_then(|entry| entry.get("id"))
            .and_then(Value::as_str),
        Some(first_id.as_str())
    );

    let queued_only = list_sessions_payload(
        &state,
        "default",
        false,
        None,
        20,
        &SessionFilterCriteria {
            queued_only: true,
            ..SessionFilterCriteria::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        queued_only
            .get("sessions")
            .and_then(Value::as_array)
            .and_then(|entries| entries.first())
            .and_then(|entry| entry.get("id"))
            .and_then(Value::as_str),
        Some(second_id.as_str())
    );

    let untagged_only = list_sessions_payload(
        &state,
        "default",
        false,
        None,
        20,
        &SessionFilterCriteria {
            untagged_only: true,
            ..SessionFilterCriteria::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        untagged_only
            .get("sessions")
            .and_then(Value::as_array)
            .and_then(|entries| entries.first())
            .and_then(|entry| entry.get("id"))
            .and_then(Value::as_str),
        Some(second_id.as_str())
    );
    assert!(
        untagged_only
            .get("sessions")
            .and_then(Value::as_array)
            .is_some_and(|entries| entries.iter().all(|entry| entry
                .get("tags")
                .and_then(Value::as_array)
                .is_none_or(Vec::is_empty)))
    );

    let matched = search_sessions_payload(
        &state,
        "default",
        "queue",
        "summary",
        false,
        None,
        20,
        &SessionFilterCriteria::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        matched
            .get("sessions")
            .and_then(Value::as_array)
            .and_then(|entries| entries.first())
            .and_then(|entry| entry.get("name"))
            .and_then(Value::as_str),
        Some("Fix Queue")
    );

    let uppercase = search_sessions_payload(
        &state,
        "default",
        "BUILD",
        "summary",
        false,
        None,
        20,
        &SessionFilterCriteria::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        uppercase
            .get("sessions")
            .and_then(Value::as_array)
            .and_then(|entries| entries.first())
            .and_then(|entry| entry.get("name"))
            .and_then(Value::as_str),
        Some("Build Docs")
    );

    app_server_client(&state, "default")
            .await
            .unwrap()
            .request(
                "thread/seed",
                json!({
                    "thread": {
                        "id": "thread-full",
                        "name": "Research notes",
                        "preview": "Unrelated summary",
                        "cwd": workspace.display().to_string(),
                        "archived": false,
                        "createdAt": 3,
                        "updatedAt": 4,
                        "status": "idle",
                        "isSubagent": false,
                        "agentNickname": Value::Null,
                        "agentRole": Value::Null,
                        "turns": [
                            {
                                "id": "turn-1",
                                "status": "completed",
                                "error": Value::Null,
                                "startedAt": 30,
                                "completedAt": 40,
                                "durationMs": 10,
                                "items": [
                                    {
                                        "id": "item-1",
                                        "type": "assistantMessage",
                                        "text": "The websocket duplicate send race originates from optimistic queue replay."
                                    },
                                    {
                                        "id": "item-2",
                                        "type": "agent_message",
                                        "message": "Snake case assistant messages are normalized."
                                    },
                                    {
                                        "id": "item-3",
                                        "type": "turn_aborted",
                                        "message": "Internal abort marker should not render as a transcript item."
                                    }
                                ]
                            }
                        ]
                    }
                }),
            )
            .await
            .unwrap();
    invalidate_session_lists(&state, "default").await;

    let full_text = search_sessions_payload(
        &state,
        "default",
        "optimistic queue replay",
        "full",
        false,
        None,
        20,
        &SessionFilterCriteria::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        full_text
            .get("sessions")
            .and_then(Value::as_array)
            .and_then(|entries| entries.first())
            .and_then(|entry| entry.get("id"))
            .and_then(Value::as_str),
        Some("thread-full")
    );
    let normalized_thread = read_thread_payload(&state, "default", "thread-full", true)
        .await
        .unwrap();
    let normalized_items = normalized_thread
        .get("turns")
        .and_then(Value::as_array)
        .and_then(|turns| turns.first())
        .and_then(|turn| turn.get("items"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        normalized_items
            .iter()
            .filter_map(|item| item.get("type").and_then(Value::as_str))
            .collect::<Vec<_>>(),
        vec!["agentMessage", "agentMessage"]
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_list_normalizes_mixed_timestamp_units() {
    let sandbox = unique_test_dir("session-list-timestamp-units");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let client = app_server_client(&state, "default").await.unwrap();

    client
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": "thread-seconds",
                    "name": "Seconds timestamp",
                    "preview": "",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1_713_920_000,
                    "updatedAt": 1_713_920_005,
                    "status": "idle",
                    "isSubagent": false,
                    "agentNickname": null,
                    "agentRole": null,
                    "turns": []
                }
            }),
        )
        .await
        .unwrap();
    client
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": "thread-millis",
                    "name": "Millis timestamp",
                    "preview": "",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1_713_919_999_000_i64,
                    "updatedAt": 1_713_920_004_000_i64,
                    "status": "idle",
                    "isSubagent": false,
                    "agentNickname": null,
                    "agentRole": null,
                    "turns": []
                }
            }),
        )
        .await
        .unwrap();

    let payload = list_sessions_payload(
        &state,
        "default",
        false,
        None,
        20,
        &SessionFilterCriteria::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        payload
            .get("sessions")
            .and_then(Value::as_array)
            .and_then(|sessions| sessions.first())
            .and_then(|session| session.get("id"))
            .and_then(Value::as_str),
        Some("thread-seconds")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn ws_review_start_proxies_to_session_app_server() {
    let sandbox = unique_test_dir("session-review-start-proxy");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let client = app_server_client(&state, "default").await.unwrap();
    client
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": "thread-review",
                    "name": "Review target",
                    "preview": "",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 2,
                    "status": "idle",
                    "isSubagent": false,
                    "agentNickname": null,
                    "agentRole": null,
                    "turns": []
                }
            }),
        )
        .await
        .unwrap();

    let (out_tx, _out_rx) = mpsc::channel(8);
    let subscriptions: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let auth = AuthContext {
        role: UserRole::Admin,
        profile_id: "default".to_string(),
    };

    let payload = execute_ws_method(
        &state,
        &out_tx,
        &subscriptions,
        &auth,
        "session/review/start",
        json!({
            "sessionId": "thread-review",
            "target": {
                "type": "custom",
                "instructions": "Review the current diff for regressions."
            },
            "delivery": "detached"
        }),
    )
    .await
    .unwrap();

    let review_thread_id = payload
        .get("reviewThreadId")
        .and_then(Value::as_str)
        .expect("review start should return a review thread id");
    assert_ne!(review_thread_id, "thread-review");
    assert_eq!(
        payload
            .get("turn")
            .and_then(|turn| turn.get("status"))
            .and_then(Value::as_str),
        Some("inProgress")
    );

    let source_thread = client
        .request("thread/read", json!({ "threadId": "thread-review" }))
        .await
        .unwrap();
    let last_review = source_thread
        .get("thread")
        .and_then(|thread| thread.get("lastReviewStart"))
        .expect("fake app-server should record review/start params");
    assert_eq!(
        last_review.get("threadId").and_then(Value::as_str),
        Some("thread-review")
    );
    assert_eq!(
        last_review
            .get("target")
            .and_then(|target| target.get("type"))
            .and_then(Value::as_str),
        Some("custom")
    );

    let request_count = client
        .request("debug/requestCount", json!({ "target": "review/start" }))
        .await
        .unwrap();
    assert_eq!(request_count.get("count").and_then(Value::as_u64), Some(1));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn ws_session_rollback_proxies_to_session_app_server() {
    let sandbox = unique_test_dir("session-rollback-proxy");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let client = app_server_client(&state, "default").await.unwrap();
    client
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": "thread-rollback",
                    "name": "Rollback target",
                    "preview": "",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 2,
                    "status": "idle",
                    "isSubagent": false,
                    "agentNickname": null,
                    "agentRole": null,
                    "turns": [
                        {
                            "id": "turn-1",
                            "status": "completed",
                            "error": null,
                            "startedAt": 1,
                            "completedAt": 2,
                            "durationMs": 1,
                            "items": []
                        },
                        {
                            "id": "turn-2",
                            "status": "completed",
                            "error": null,
                            "startedAt": 3,
                            "completedAt": 4,
                            "durationMs": 1,
                            "items": []
                        }
                    ]
                }
            }),
        )
        .await
        .unwrap();

    let (out_tx, _out_rx) = mpsc::channel(8);
    let subscriptions: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let auth = AuthContext {
        role: UserRole::Admin,
        profile_id: "default".to_string(),
    };

    let payload = execute_ws_method(
        &state,
        &out_tx,
        &subscriptions,
        &auth,
        "session/rollback",
        json!({
            "sessionId": "thread-rollback",
            "numTurns": 1
        }),
    )
    .await
    .unwrap();

    assert_eq!(
        payload
            .get("thread")
            .and_then(|thread| thread.get("turns"))
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );
    let request_count = client
        .request("debug/requestCount", json!({ "target": "thread/rollback" }))
        .await
        .unwrap();
    assert_eq!(request_count.get("count").and_then(Value::as_u64), Some(1));

    let _ = fs::remove_dir_all(sandbox);
}

// This cache-flow test builds a large async state machine; poll it on a larger
// stack so default `cargo test` runs do not abort on 2 MiB test stacks.
#[test]
#[rustfmt::skip]
fn ws_session_cache_validation_returns_not_modified_for_matching_versions() {
    std::thread::Builder::new()
        .name("session-cache-validation".to_string())
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
    let sandbox = unique_test_dir("session-cache-validation-rust");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let created = create_session_payload(
        &state,
        "default",
        json!({ "cwd": workspace.display().to_string() }),
        None,
        Some("Cached thread"),
    )
    .await
    .unwrap();
    let session_id = created
        .get("id")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();

    let (out_tx, _out_rx) = mpsc::channel(8);
    let subscriptions: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let auth = AuthContext {
        role: UserRole::Admin,
        profile_id: "default".to_string(),
    };

    let list_payload = execute_ws_method(
        &state,
        &out_tx,
        &subscriptions,
        &auth,
        "sessions/list",
        json!({
            "archived": false,
            "limit": 20
        }),
    )
    .await
    .unwrap();
    let list_version = list_payload
        .get("cacheVersion")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();
    let list_summary_versions = list_payload.get("summaryVersions").cloned().unwrap();
    let list_state_hash = list_payload
        .get("stateHash")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();
    let list_not_modified = execute_ws_method(
        &state,
        &out_tx,
        &subscriptions,
        &auth,
        "sessions/list",
        json!({
            "archived": false,
            "limit": 20,
            "knownVersion": list_version
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        list_not_modified
            .get("notModified")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert!(list_not_modified.get("sessions").is_none());

    let patched_session = create_session_payload(
        &state,
        "default",
        json!({ "cwd": workspace.display().to_string() }),
        None,
        Some("Patched thread"),
    )
    .await
    .unwrap();
    let patched_session_id = patched_session
        .get("id")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();
    let list_patch = execute_ws_method(
        &state,
        &out_tx,
        &subscriptions,
        &auth,
        "sessions/list",
        json!({
            "archived": false,
            "limit": 20,
            "knownVersion": list_version,
            "knownSummaryVersions": list_summary_versions,
            "knownStateHash": list_state_hash
        }),
    )
    .await
    .unwrap();
    let patch = list_patch.get("patch").unwrap();
    assert_eq!(
        patch
            .get("upserts")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .filter_map(|summary| summary.get("id").and_then(Value::as_str))
            .collect::<Vec<_>>(),
        vec![patched_session_id]
    );
    assert!(
        patch
            .get("finalStateHash")
            .and_then(Value::as_str)
            .is_some()
    );
    assert!(list_patch.get("sessions").is_none());

    let detail_payload = execute_ws_method(
        &state,
        &out_tx,
        &subscriptions,
        &auth,
        "session/get",
        json!({
            "sessionId": session_id,
            "limit": 20
        }),
    )
    .await
    .unwrap();
    let detail_version = detail_payload
        .get("cacheVersion")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();
    let detail_turn_versions = detail_payload.get("turnVersions").cloned().unwrap();
    let detail_state_hash = detail_payload
        .get("stateHash")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();
    let detail_not_modified = execute_ws_method(
        &state,
        &out_tx,
        &subscriptions,
        &auth,
        "session/get",
        json!({
            "sessionId": session_id,
            "limit": 20,
            "knownVersion": detail_version
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        detail_not_modified
            .get("notModified")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert!(detail_not_modified.get("thread").is_none());

    with_ui_state_write(&state, "default", |ui_state| {
        let Some(queues) = ui_state
            .get_mut("queuesByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "queue state is missing",
            ));
        };
        queues.insert(
            session_id.clone(),
            json!({
                "items": [{
                    "id": "queue-item-1",
                    "prompt": "queued while detail is cached",
                    "skills": [],
                    "attachmentIds": [],
                    "attachmentNames": [],
                    "createdAt": now_unix_ms()
                }],
                "resumePending": false,
                "updatedAt": now_unix_ms()
            }),
        );
        Ok(())
    })
    .await
    .unwrap();
    let queue_detail_patch = execute_ws_method(
        &state,
        &out_tx,
        &subscriptions,
        &auth,
        "session/get",
        json!({
            "sessionId": session_id,
            "limit": 20,
            "knownVersion": detail_version,
            "knownTurnVersions": detail_turn_versions.clone(),
            "knownStateHash": detail_state_hash.clone()
        }),
    )
    .await
    .unwrap();
    let queue_patch = queue_detail_patch.get("patch").unwrap();
    assert_eq!(
        queue_patch
            .get("queue")
            .and_then(|queue| queue.get("items"))
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        queue_patch
            .get("turnUpserts")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );

    execute_ws_method(
        &state,
        &out_tx,
        &subscriptions,
        &auth,
        "session/rename",
        json!({
            "sessionId": session_id,
            "name": "Renamed detail thread"
        }),
    )
    .await
    .unwrap();
    let detail_patch = execute_ws_method(
        &state,
        &out_tx,
        &subscriptions,
        &auth,
        "session/get",
        json!({
            "sessionId": session_id,
            "limit": 20,
            "knownVersion": detail_version,
            "knownTurnVersions": detail_turn_versions,
            "knownStateHash": detail_state_hash
        }),
    )
    .await
    .unwrap();
    let patch = detail_patch.get("patch").unwrap();
    assert_eq!(
        patch
            .get("thread")
            .and_then(|thread| thread.get("name"))
            .and_then(Value::as_str),
        Some("Renamed detail thread")
    );
    assert_eq!(
        patch
            .get("turnUpserts")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );
    assert!(
        patch
            .get("finalStateHash")
            .and_then(Value::as_str)
            .is_some()
    );
    assert!(detail_patch.get("thread").is_none());

    let _ = fs::remove_dir_all(sandbox);
                });
        })
        .unwrap()
        .join()
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abort_turn_payload_uses_known_active_turn() {
    let sandbox = unique_test_dir("abort-turn-rust");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    state.active_turns.lock().await.insert(
        runtime_session_key("default", "thread-1"),
        "turn-123".to_string(),
    );

    let payload = abort_turn_payload(&state, "default", "thread-1")
        .await
        .unwrap();
    assert_eq!(
        payload.get("interrupted").and_then(Value::as_bool),
        Some(true)
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_detail_and_turn_search_payloads_use_rust_thread_reads() {
    let sandbox = unique_test_dir("session-detail-rust");
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
                        "name": "Investigate bug",
                        "preview": "Investigate websocket bug",
                        "cwd": workspace.display().to_string(),
                        "archived": false,
                        "createdAt": 1,
                        "updatedAt": 2,
                        "status": "running",
                        "isSubagent": false,
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
                                        "text": "Find the websocket bug"
                                    },
                                    {
                                        "id": "item-2",
                                        "type": "agentMessage",
                                        "text": "Investigating the websocket bug now"
                                    },
                                    {
                                        "id": "item-command-1",
                                        "type": "commandExecution",
                                        "command": ["bash", "-lc", "printf done"],
                                        "aggregatedOutput": "x".repeat(1024 * 128),
                                        "exitCode": 0
                                    }
                                ]
                            },
                            {
                                "id": "turn-2",
                                "status": "inProgress",
                                "error": Value::Null,
                                "startedAt": 30,
                                "completedAt": Value::Null,
                                "durationMs": Value::Null,
                                "items": [
                                    {
                                        "id": "item-3",
                                        "type": "reasoning",
                                        "text": "Need to inspect websocket state handling"
                                    }
                                ]
                            }
                        ],
                        "tokenUsage": {
                            "total": { "totalTokens": 12, "inputTokens": 6, "cachedInputTokens": 0, "outputTokens": 6, "reasoningOutputTokens": 2 },
                            "last": { "totalTokens": 7, "inputTokens": 3, "cachedInputTokens": 0, "outputTokens": 4, "reasoningOutputTokens": 1 },
                            "modelContextWindow": 1000
                        }
                    }
                }),
            )
            .await
            .unwrap();

    let detail = session_detail_payload(&state, "default", "thread-1", 1)
        .await
        .unwrap();
    assert_eq!(
        detail
            .get("thread")
            .and_then(|value| value.get("turns"))
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        detail.get("activeTurnId").and_then(Value::as_str),
        Some("turn-2")
    );
    assert_eq!(
        detail
            .get("hydration")
            .and_then(|value| value.get("remainingTurns"))
            .and_then(Value::as_u64),
        Some(1)
    );
    let older = session_older_turns_payload(&state, "default", "thread-1", "turn-2", 5)
        .await
        .unwrap();
    let older_command = older
        .get("turns")
        .and_then(Value::as_array)
        .and_then(|turns| turns.first())
        .and_then(|turn| turn.get("items"))
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.get("id").and_then(Value::as_str) == Some("item-command-1"))
        })
        .unwrap();
    assert_eq!(
        older_command.get("detailState").and_then(Value::as_str),
        Some("deferred")
    );
    assert!(older_command.get("aggregatedOutput").is_none());

    assert_eq!(
        older.get("turns").and_then(Value::as_array).map(Vec::len),
        Some(1)
    );

    let turn = session_turn_payload(&state, "default", "thread-1", "turn-1")
        .await
        .unwrap();
    assert_eq!(
        turn.get("turn")
            .and_then(|value| value.get("id"))
            .and_then(Value::as_str),
        Some("turn-1")
    );
    let full_command = turn
        .get("turn")
        .and_then(|value| value.get("items"))
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.get("id").and_then(Value::as_str) == Some("item-command-1"))
        })
        .unwrap();
    assert!(full_command.get("aggregatedOutput").is_some());

    let item = session_item_detail_payload(&state, "default", "thread-1", "turn-1", "item-2")
        .await
        .unwrap();
    assert_eq!(
        item.get("item")
            .and_then(|value| value.get("detailState"))
            .and_then(Value::as_str),
        Some("loaded")
    );

    let search = search_session_turns_payload(&state, "default", "thread-1", "websocket", None, 20)
        .await
        .unwrap();
    assert_eq!(
        search
            .get("matches")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(3)
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_detail_uses_local_rollout_tail_when_thread_read_is_slow() {
    let sandbox = unique_test_dir("session-detail-local-tail");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let session_id = "019df000-0000-7000-8000-000000000111";
    let rollout_dir = codex_home
        .join("sessions")
        .join("2026")
        .join("04")
        .join("24");
    fs::create_dir_all(&rollout_dir).unwrap();
    fs::write(
        rollout_dir.join(format!("rollout-2026-04-24T01-00-00-{session_id}.jsonl")),
        format!(
            "{}\n{}\n{}\n{}\n{}\n",
            json!({
                "timestamp": "2026-04-24T01:00:00.000Z",
                "type": "session_meta",
                "payload": {
                    "id": session_id,
                    "timestamp": "2026-04-24T01:00:00.000Z",
                    "cwd": workspace.display().to_string()
                }
            }),
            json!({
                "timestamp": "2026-04-24T01:00:01.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "task_started",
                    "turn_id": "turn-local"
                }
            }),
            json!({
                "timestamp": "2026-04-24T01:00:02.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "user_message",
                    "message": "open the slow local session"
                }
            }),
            json!({
                "timestamp": "2026-04-24T01:00:03.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "agent_message",
                    "message": "loaded from rollout tail",
                    "phase": "final_answer"
                }
            }),
            json!({
                "timestamp": "2026-04-24T01:00:04.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "task_complete",
                    "turn_id": "turn-local",
                    "last_agent_message": "loaded from rollout tail",
                    "completed_at": 1_776_969_604_000_i64,
                    "duration_ms": 3_000
                }
            })
        ),
    )
    .unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let client = app_server_client(&state, "default").await.unwrap();
    client
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": session_id,
                    "name": "Slow app-server session",
                    "preview": "slow",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1_776_969_600_000_i64,
                    "updatedAt": 1_776_969_604_000_i64,
                    "status": "running",
                    "readDelayMs": 5_000,
                    "isSubagent": false,
                    "agentNickname": Value::Null,
                    "agentRole": Value::Null,
                    "turns": []
                }
            }),
        )
        .await
        .unwrap();
    client
        .request(
            "debug/setDelay",
            json!({
                "method": "thread/goal/get",
                "delayMs": 5_000
            }),
        )
        .await
        .unwrap();

    let detail = tokio::time::timeout(
        Duration::from_secs(2),
        session_detail_payload(&state, "default", session_id, 20),
    )
    .await
    .expect("session detail should not wait for slow app-server thread/read or goal reads")
    .unwrap();

    let turns = detail
        .get("thread")
        .and_then(|value| value.get("turns"))
        .and_then(Value::as_array)
        .unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(
        turns[0]
            .get("items")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|item| item.get("text"))
            .and_then(Value::as_str),
        Some("open the slow local session")
    );
    assert_eq!(
        turns[0]
            .get("items")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(2)
    );
    assert_eq!(
        turns[0]
            .get("items")
            .and_then(Value::as_array)
            .and_then(|items| items.get(1))
            .and_then(|item| item.get("text"))
            .and_then(Value::as_str),
        Some("loaded from rollout tail")
    );
    assert!(detail.get("goal").is_some_and(Value::is_null));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_detail_does_not_resurrect_active_rollout_after_app_server_exit() {
    let sandbox = unique_test_dir("session-detail-stale-active-rollout");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let session_id = "019df000-0000-7000-8000-000000000999";
    write_rollout_fixture(
        &codex_home,
        false,
        "2026/04/24",
        "2026-04-24T01-10-00",
        session_id,
        &workspace,
        "start and crash before completion",
        &[&json!({
            "timestamp": "2026-04-24T01:10:03.000Z",
            "type": "event_msg",
            "payload": {
                "type": "task_started",
                "turn_id": "turn-stale"
            }
        })
        .to_string()],
        None,
    );

    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    with_ui_state_write(&state, "default", |ui_state| {
        let Some(runtime_status_by_thread_id) = ui_state
            .get_mut("runtimeStatusByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "runtime status state is missing",
            ));
        };
        runtime_status_by_thread_id.insert(
            session_id.to_string(),
            json!({
                "status": "failed",
                "updatedAt": now_unix_ms(),
                "reason": "codex app-server exited"
            }),
        );
        Ok(())
    })
    .await
    .unwrap();

    let detail = session_detail_payload(&state, "default", session_id, 20)
        .await
        .unwrap();

    assert_eq!(
        detail
            .get("thread")
            .and_then(|thread| thread.get("status"))
            .and_then(Value::as_str),
        Some("failed")
    );
    assert!(detail.get("activeTurnId").is_some_and(Value::is_null));
    assert!(
        state
            .active_turns
            .lock()
            .await
            .get(&runtime_session_key("default", session_id))
            .is_none()
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_detail_clears_orphaned_active_rollout_after_restart() {
    let sandbox = unique_test_dir("session-detail-orphaned-active-after-restart");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let session_id = "019df000-0000-7000-8000-000000000998";
    let rollout_dir = codex_home
        .join("sessions")
        .join("2026")
        .join("04")
        .join("24");
    fs::create_dir_all(&rollout_dir).unwrap();
    fs::write(
        rollout_dir.join(format!("rollout-2026-04-24T01-09-00-{session_id}.jsonl")),
        format!(
            "{}\n{}\n{}\n{}\n",
            json!({
                "timestamp": "2026-04-24T01:09:00.000Z",
                "type": "session_meta",
                "payload": {
                    "id": session_id,
                    "timestamp": "2026-04-24T01:09:00.000Z",
                    "cwd": workspace.display().to_string()
                }
            }),
            json!({
                "timestamp": "2026-04-24T01:09:01.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "task_started",
                    "turn_id": "turn-orphaned"
                }
            }),
            json!({
                "timestamp": "2026-04-24T01:09:02.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "user_message",
                    "message": "start and lose the host process"
                }
            }),
            json!({
                "timestamp": "2026-04-24T01:09:03.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "agent_message",
                    "message": "work was interrupted before completion",
                    "phase": "commentary"
                }
            })
        ),
    )
    .unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    with_ui_state_write(&state, "default", |ui_state| {
        let Some(runtime_status_by_thread_id) = ui_state
            .get_mut("runtimeStatusByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "runtime status state is missing",
            ));
        };
        runtime_status_by_thread_id.insert(
            session_id.to_string(),
            json!({
                "status": "running",
                "updatedAt": now_unix_ms().saturating_sub(60_000)
            }),
        );
        Ok(())
    })
    .await
    .unwrap();

    let detail = session_detail_payload(&state, "default", session_id, 20)
        .await
        .unwrap();

    assert_eq!(
        detail
            .get("thread")
            .and_then(|thread| thread.get("status"))
            .and_then(Value::as_str),
        Some("completed")
    );
    assert!(detail.get("activeTurnId").is_some_and(Value::is_null));
    assert!(
        state
            .active_turns
            .lock()
            .await
            .get(&runtime_session_key("default", session_id))
            .is_none()
    );
    let runtime_status = with_ui_state_read(&state, "default", |ui_state| {
        Ok(ui_state["runtimeStatusByThreadId"][session_id].clone())
    })
    .await
    .unwrap();
    assert_eq!(
        runtime_status.get("status").and_then(Value::as_str),
        Some("completed")
    );
    assert_eq!(
        detail
            .get("thread")
            .and_then(|thread| thread.get("turns"))
            .and_then(Value::as_array)
            .and_then(|turns| turns.first())
            .and_then(|turn| turn.get("status"))
            .and_then(Value::as_str),
        Some("completed")
    );
    assert!(
        runtime_status
            .get("reason")
            .and_then(Value::as_str)
            .is_some_and(|reason| reason.contains("did not report an active turn"))
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn app_server_exit_clears_cached_running_session_state() {
    let sandbox = unique_test_dir("app-server-exit-clears-running");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    let session_id = "thread-crashed";
    let runtime_key = runtime_session_key("default", session_id);
    state
        .active_turns
        .lock()
        .await
        .insert(runtime_key.clone(), "turn-crashed".to_string());
    state
        .pending_turn_starts
        .lock()
        .await
        .insert(runtime_key.clone());
    {
        let mut pending = state.pending_server_requests.lock().await;
        pending.entry(runtime_key.clone()).or_default().insert(
            "request-crashed".to_string(),
            PendingServerRequestEntry {
                raw_id: json!("request-crashed"),
                method: "input/request".to_string(),
                params: json!({ "threadId": session_id }),
                created_at: "2026-05-20T00:00:00Z".to_string(),
                created_at_ms: 1,
            },
        );
    }
    with_ui_state_write(&state, "default", |ui_state| {
        let Some(runtime_status_by_thread_id) = ui_state
            .get_mut("runtimeStatusByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "runtime status state is missing",
            ));
        };
        runtime_status_by_thread_id.insert(
            session_id.to_string(),
            json!({
                "status": "running",
                "updatedAt": now_unix_ms()
            }),
        );
        ui_state["highlightsByThreadId"][session_id] = json!({
            "kind": "attention",
            "at": 1,
            "reason": "approval"
        });
        ui_state["notifications"]["items"] = json!([{
            "id": "notification-crashed",
            "type": "sessionAttention",
            "createdAt": 1,
            "readAt": Value::Null,
            "sessionId": session_id,
            "sessionName": "Crashed session",
            "payload": {
                "reason": "approval",
                "requestId": "request-crashed"
            }
        }]);
        ui_state["automationRuns"] = json!([{
            "id": "run-crashed",
            "automationId": "auto-crashed",
            "automationName": "Crashed automation",
            "status": "started",
            "trigger": "schedule",
            "sessionId": session_id,
            "repoPath": Value::Null,
            "cwd": Value::Null,
            "worktreePath": Value::Null,
            "startedAt": 1,
            "completedAt": Value::Null,
            "error": Value::Null
        }]);
        Ok(())
    })
    .await
    .unwrap();

    clear_profile_runtime_activity_after_app_server_exit(
        &state,
        "default",
        Some("codex app-server exited"),
    )
    .await;
    let snapshot = read_session_summary_ui_snapshot(&state, "default")
        .await
        .unwrap();

    assert!(snapshot.active_thread_ids.is_empty());
    assert!(
        state
            .pending_server_requests
            .lock()
            .await
            .get(&runtime_key)
            .is_none()
    );
    assert_eq!(
        snapshot
            .runtime_status_by_thread_id
            .get(session_id)
            .and_then(|status| status.get("status"))
            .and_then(Value::as_str),
        Some("failed")
    );
    assert_eq!(
        snapshot
            .highlights_by_thread_id
            .get(session_id)
            .and_then(|highlight| highlight.get("kind"))
            .and_then(Value::as_str),
        Some("attention")
    );
    assert_eq!(
        snapshot
            .highlights_by_thread_id
            .get(session_id)
            .and_then(|highlight| highlight.get("reason"))
            .and_then(Value::as_str),
        Some("stopped")
    );
    let ui_state = with_ui_state_read(&state, "default", |ui_state| Ok(ui_state.clone()))
        .await
        .unwrap();
    let notifications = ui_state["notifications"]["items"]
        .as_array()
        .expect("notifications should be available");
    assert!(!notifications.iter().any(|notification| {
        notification.get("type").and_then(Value::as_str) == Some("sessionAttention")
            && notification
                .get("payload")
                .and_then(|payload| payload.get("reason"))
                .and_then(Value::as_str)
                == Some("approval")
    }));
    assert!(notifications.iter().any(|notification| {
        notification.get("type").and_then(Value::as_str) == Some("sessionAttention")
            && notification
                .get("payload")
                .and_then(|payload| payload.get("reason"))
                .and_then(Value::as_str)
                == Some("stopped")
    }));
    let runs = with_ui_state_read(&state, "default", |ui_state| {
        Ok(recent_automation_runs_from_ui_state(ui_state, 10))
    })
    .await
    .unwrap();
    let run = runs
        .first()
        .expect("automation run should remain available");
    assert_eq!(run.get("status").and_then(Value::as_str), Some("failed"));
    assert!(
        run.get("error")
            .and_then(Value::as_str)
            .is_some_and(|error| error.contains("codex app-server exited"))
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_reconcile_marks_lost_active_session_failed_without_app_server() {
    let sandbox = unique_test_dir("runtime-reconcile-no-app-server");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state = test_state(workspace.clone(), vec![workspace], codex_home);
    let session_id = "thread-lost-no-app-server";
    let runtime_key = runtime_session_key("default", session_id);
    state
        .active_turns
        .lock()
        .await
        .insert(runtime_key.clone(), "turn-lost".to_string());
    with_ui_state_write(&state, "default", |ui_state| {
        ui_state["runtimeStatusByThreadId"][session_id] = json!({
            "status": "running",
            "updatedAt": now_unix_ms().saturating_sub(60_000)
        });
        Ok(())
    })
    .await
    .unwrap();

    let reconciled = reconcile_lost_runtime_activity_for_profile(&state, "default").await;

    assert_eq!(reconciled, vec![session_id.to_string()]);
    assert!(state.active_turns.lock().await.get(&runtime_key).is_none());
    let ui_state = with_ui_state_read(&state, "default", |ui_state| Ok(ui_state.clone()))
        .await
        .unwrap();
    assert_eq!(
        ui_state["runtimeStatusByThreadId"][session_id]["status"].as_str(),
        Some("failed")
    );
    assert_eq!(
        ui_state["highlightsByThreadId"][session_id]["kind"].as_str(),
        Some("attention")
    );
    assert_eq!(
        ui_state["highlightsByThreadId"][session_id]["reason"].as_str(),
        Some("stopped")
    );
    assert_eq!(
        ui_state["notifications"]["items"][0]["type"].as_str(),
        Some("sessionAttention")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_reconcile_marks_session_failed_when_app_server_forgets_active_turn() {
    let sandbox = unique_test_dir("runtime-reconcile-forgotten-turn");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let session_id = "thread-forgotten-active-turn";
    let runtime_key = runtime_session_key("default", session_id);
    state
        .active_turns
        .lock()
        .await
        .insert(runtime_key.clone(), "turn-forgotten".to_string());
    with_ui_state_write(&state, "default", |ui_state| {
        ui_state["runtimeStatusByThreadId"][session_id] = json!({
            "status": "running",
            "updatedAt": now_unix_ms().saturating_sub(60_000)
        });
        Ok(())
    })
    .await
    .unwrap();
    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": session_id,
                    "name": "Forgotten active turn",
                    "preview": "",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 2,
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

    assert!(
        state.active_turns.lock().await.contains_key(&runtime_key),
        "test setup should keep the cached active turn"
    );
    assert!(
        state.app_servers.active_process_count().await > 0,
        "fake app-server should be running before reconciliation"
    );

    let reconciled = reconcile_lost_runtime_activity_for_profile(&state, "default").await;

    assert_eq!(reconciled, vec![session_id.to_string()]);
    assert!(state.active_turns.lock().await.get(&runtime_key).is_none());
    let ui_state = with_ui_state_read(&state, "default", |ui_state| Ok(ui_state.clone()))
        .await
        .unwrap();
    assert_eq!(
        ui_state["runtimeStatusByThreadId"][session_id]["status"].as_str(),
        Some("failed")
    );
    assert!(
        ui_state["runtimeStatusByThreadId"][session_id]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("no longer has an active turn"))
    );
    assert_eq!(
        ui_state["highlightsByThreadId"][session_id]["kind"].as_str(),
        Some("attention")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn truncated_local_session_detail_exposes_idle_history_state() {
    let sandbox = unique_test_dir("session-detail-truncated-idle");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let session_id = "019df000-0000-7000-8000-000000000112";
    let rollout_dir = codex_home
        .join("sessions")
        .join("2026")
        .join("04")
        .join("24");
    fs::create_dir_all(&rollout_dir).unwrap();
    let rollout_path = rollout_dir.join(format!("rollout-2026-04-24T01-05-00-{session_id}.jsonl"));
    let mut file = fs::File::create(&rollout_path).unwrap();
    use std::io::Write as _;
    writeln!(
        file,
        "{}",
        json!({
            "timestamp": "2026-04-24T01:05:00.000Z",
            "type": "session_meta",
            "payload": {
                "id": session_id,
                "timestamp": "2026-04-24T01:05:00.000Z",
                "cwd": workspace.display().to_string()
            }
        })
    )
    .unwrap();
    writeln!(
        file,
        "{}",
        json!({
            "timestamp": "2026-04-24T01:05:00.500Z",
            "type": "filler",
            "payload": {
                "text": "x".repeat((8 * 1024 * 1024) + 1024)
            }
        })
    )
    .unwrap();
    for (timestamp, payload) in [
        (
            "2026-04-24T01:05:01.000Z",
            json!({
                "type": "task_started",
                "turn_id": "turn-truncated"
            }),
        ),
        (
            "2026-04-24T01:05:02.000Z",
            json!({
                "type": "user_message",
                "message": "open a very large local session"
            }),
        ),
        (
            "2026-04-24T01:05:03.000Z",
            json!({
                "type": "agent_message",
                "message": "loaded from the truncated tail",
                "phase": "final_answer"
            }),
        ),
        (
            "2026-04-24T01:05:04.000Z",
            json!({
                "type": "task_complete",
                "turn_id": "turn-truncated",
                "completed_at": 1_776_969_904_000_i64,
                "duration_ms": 3_000
            }),
        ),
    ] {
        writeln!(
            file,
            "{}",
            json!({
                "timestamp": timestamp,
                "type": "event_msg",
                "payload": payload
            })
        )
        .unwrap();
    }
    drop(file);

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let detail = session_detail_payload(&state, "default", session_id, 20)
        .await
        .unwrap();

    assert_eq!(
        detail
            .get("hydration")
            .and_then(|value| value.get("state"))
            .and_then(Value::as_str),
        Some("idle")
    );
    assert_eq!(
        detail
            .get("hydration")
            .and_then(|value| value.get("remainingTurns"))
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        detail
            .get("thread")
            .and_then(|value| value.get("turns"))
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_detail_uses_task_complete_last_agent_message_fallback() {
    let sandbox = unique_test_dir("session-detail-complete-message");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let session_id = "019df000-0000-7000-8000-000000000114";
    let rollout_dir = codex_home
        .join("sessions")
        .join("2026")
        .join("04")
        .join("24");
    fs::create_dir_all(&rollout_dir).unwrap();
    fs::write(
        rollout_dir.join(format!("rollout-2026-04-24T01-07-00-{session_id}.jsonl")),
        format!(
            "{}\n{}\n{}\n{}\n",
            json!({
                "timestamp": "2026-04-24T01:07:00.000Z",
                "type": "session_meta",
                "payload": {
                    "id": session_id,
                    "timestamp": "2026-04-24T01:07:00.000Z",
                    "cwd": workspace.display().to_string()
                }
            }),
            json!({
                "timestamp": "2026-04-24T01:07:01.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "task_started",
                    "turn_id": "turn-completed"
                }
            }),
            json!({
                "timestamp": "2026-04-24T01:07:02.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "user_message",
                    "message": "finish in the background"
                }
            }),
            json!({
                "timestamp": "2026-04-24T01:07:04.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "task_complete",
                    "turn_id": "turn-completed",
                    "last_agent_message": "final answer from task completion",
                    "completed_at": 1_776_970_024_000_i64,
                    "duration_ms": 3_000
                }
            })
        ),
    )
    .unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let detail = session_detail_payload(&state, "default", session_id, 20)
        .await
        .unwrap();

    let turns = detail
        .get("thread")
        .and_then(|value| value.get("turns"))
        .and_then(Value::as_array)
        .unwrap();
    assert_eq!(turns.len(), 1);
    let items = turns[0].get("items").and_then(Value::as_array).unwrap();
    assert_eq!(
        items
            .iter()
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("agentMessage"))
            .count(),
        1
    );
    assert_eq!(
        items
            .iter()
            .find(|item| item.get("type").and_then(Value::as_str) == Some("agentMessage"))
            .and_then(|item| item.get("text"))
            .and_then(Value::as_str),
        Some("final answer from task completion")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_session_detail_marks_orphaned_active_tail_completed_without_runtime_evidence() {
    let sandbox = unique_test_dir("session-detail-local-orphaned-active");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let session_id = "019df000-0000-7000-8000-000000000113";
    let rollout_dir = codex_home
        .join("sessions")
        .join("2026")
        .join("04")
        .join("24");
    fs::create_dir_all(&rollout_dir).unwrap();
    fs::write(
        rollout_dir.join(format!("rollout-2026-04-24T01-06-00-{session_id}.jsonl")),
        format!(
            "{}\n{}\n{}\n{}\n",
            json!({
                "timestamp": "2026-04-24T01:06:00.000Z",
                "type": "session_meta",
                "payload": {
                    "id": session_id,
                    "timestamp": "2026-04-24T01:06:00.000Z",
                    "cwd": workspace.display().to_string()
                }
            }),
            json!({
                "timestamp": "2026-04-24T01:06:01.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "task_started",
                    "turn_id": "turn-running"
                }
            }),
            json!({
                "timestamp": "2026-04-24T01:06:02.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "user_message",
                    "message": "keep this local session running"
                }
            }),
            json!({
                "timestamp": "2026-04-24T01:06:03.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "agent_message",
                    "message": "still working",
                    "phase": "commentary"
                }
            })
        ),
    )
    .unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let detail = session_detail_payload(&state, "default", session_id, 20)
        .await
        .unwrap();

    assert_eq!(
        detail
            .get("thread")
            .and_then(|thread| thread.get("status"))
            .and_then(Value::as_str),
        Some("completed")
    );
    assert_eq!(
        detail
            .get("thread")
            .and_then(|thread| thread.get("turns"))
            .and_then(Value::as_array)
            .and_then(|turns| turns.first())
            .and_then(|turn| turn.get("status"))
            .and_then(Value::as_str),
        Some("completed")
    );
    assert!(detail.get("activeTurnId").is_some_and(Value::is_null));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_session_detail_marks_failed_context_compaction_tail_terminal() {
    let sandbox = unique_test_dir("session-detail-context-compaction-failed");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let session_id = "019df000-0000-7000-8000-000000000115";
    let rollout_dir = codex_home
        .join("sessions")
        .join("2026")
        .join("04")
        .join("24");
    fs::create_dir_all(&rollout_dir).unwrap();
    fs::write(
        rollout_dir.join(format!("rollout-2026-04-24T01-08-00-{session_id}.jsonl")),
        format!(
            "{}\n{}\n{}\n{}\n{}\n",
            json!({
                "timestamp": "2026-04-24T01:08:00.000Z",
                "type": "session_meta",
                "payload": {
                    "id": session_id,
                    "timestamp": "2026-04-24T01:08:00.000Z",
                    "cwd": workspace.display().to_string()
                }
            }),
            json!({
                "timestamp": "2026-04-24T01:08:01.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "task_started",
                    "turn_id": "turn-compacting"
                }
            }),
            json!({
                "timestamp": "2026-04-24T01:08:02.000Z",
                "type": "response_item",
                "payload": {
                    "type": "context_compaction",
                    "id": "compact-1"
                }
            }),
            json!({
                "timestamp": "2026-04-24T01:08:03.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "error",
                    "message": "You've hit your usage limit.",
                    "codex_error_info": "usage_limit_exceeded"
                }
            }),
            json!({
                "timestamp": "2026-04-24T01:08:04.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "task_complete",
                    "turn_id": "turn-compacting",
                    "last_agent_message": null
                }
            })
        ),
    )
    .unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let detail = session_detail_payload(&state, "default", session_id, 20)
        .await
        .unwrap();

    assert_eq!(
        detail
            .get("thread")
            .and_then(|thread| thread.get("status"))
            .and_then(Value::as_str),
        Some("completed")
    );
    let turn = detail
        .get("thread")
        .and_then(|thread| thread.get("turns"))
        .and_then(Value::as_array)
        .and_then(|turns| turns.first())
        .expect("failed compaction turn should be visible");
    assert_eq!(turn.get("status").and_then(Value::as_str), Some("failed"));
    assert_eq!(
        turn.get("items")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|item| item.get("type"))
            .and_then(Value::as_str),
        Some("contextCompaction")
    );
    assert_eq!(
        turn.get("items")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|item| item.get("status"))
            .and_then(Value::as_str),
        Some("failed")
    );
    assert!(detail.get("activeTurnId").is_some_and(Value::is_null));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_session_detail_does_not_resurrect_active_tail_behind_terminal_cache() {
    let sandbox = unique_test_dir("session-detail-active-terminal-cache");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let session_id = "019df000-0000-7000-8000-000000000114";
    let rollout_dir = codex_home
        .join("sessions")
        .join("2026")
        .join("04")
        .join("24");
    fs::create_dir_all(&rollout_dir).unwrap();
    fs::write(
        rollout_dir.join(format!("rollout-2026-04-24T01-07-00-{session_id}.jsonl")),
        format!(
            "{}\n{}\n{}\n{}\n",
            json!({
                "timestamp": "2026-04-24T01:07:00.000Z",
                "type": "session_meta",
                "payload": {
                    "id": session_id,
                    "timestamp": "2026-04-24T01:07:00.000Z",
                    "cwd": workspace.display().to_string()
                }
            }),
            json!({
                "timestamp": "2026-04-24T01:07:01.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "task_started",
                    "turn_id": "turn-running"
                }
            }),
            json!({
                "timestamp": "2026-04-24T01:07:02.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "user_message",
                    "message": "keep this terminal cache from hiding active work"
                }
            }),
            json!({
                "timestamp": "2026-04-24T01:07:03.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "agent_message",
                    "message": "still working",
                    "phase": "commentary"
                }
            })
        ),
    )
    .unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let _client = app_server_client(&state, "default").await.unwrap();
    with_ui_state_write(&state, "default", |ui_state| {
        let Some(runtime_status_by_thread_id) = ui_state
            .get_mut("runtimeStatusByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "runtime status state is missing",
            ));
        };
        runtime_status_by_thread_id.insert(
            session_id.to_string(),
            json!({
                "status": "completed",
                "updatedAt": now_unix_ms()
            }),
        );
        Ok(())
    })
    .await
    .unwrap();

    let detail = session_detail_payload(&state, "default", session_id, 20)
        .await
        .unwrap();

    assert_eq!(
        detail
            .get("thread")
            .and_then(|thread| thread.get("status"))
            .and_then(Value::as_str),
        Some("completed")
    );
    assert!(detail.get("activeTurnId").is_some_and(Value::is_null));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_session_detail_parses_rich_transcript_items_and_rollbacks() {
    let sandbox = unique_test_dir("session-detail-rich-items");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let session_id = "019df000-0000-7000-8000-000000000221";
    let rollout_dir = codex_home
        .join("sessions")
        .join("2026")
        .join("04")
        .join("24");
    fs::create_dir_all(&rollout_dir).unwrap();
    fs::write(
        rollout_dir.join(format!("rollout-2026-04-24T01-09-00-{session_id}.jsonl")),
        format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n",
            json!({
                "timestamp": "2026-04-24T01:09:00.000Z",
                "type": "session_meta",
                "payload": {
                    "id": session_id,
                    "timestamp": "2026-04-24T01:09:00.000Z",
                    "cwd": workspace.display().to_string()
                }
            }),
            json!({
                "timestamp": "2026-04-24T01:09:01.000Z",
                "type": "event_msg",
                "payload": { "type": "task_started", "turn_id": "turn-rich" }
            }),
            json!({
                "timestamp": "2026-04-24T01:09:02.000Z",
                "type": "event_msg",
                "payload": { "type": "user_message", "message": "show rich transcript items" }
            }),
            json!({
                "timestamp": "2026-04-24T01:09:03.000Z",
                "type": "event_msg",
                "payload": { "type": "web_search_begin", "call_id": "search-1" }
            }),
            json!({
                "timestamp": "2026-04-24T01:09:04.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "web_search_end",
                    "call_id": "search-1",
                    "query": "codex web search",
                    "action": { "type": "search", "query": "codex web search" },
                    "summary": "Found Codex web UI notes.",
                    "results": [
                        {
                            "title": "Codex Web Search",
                            "url": "https://example.com/codex-web-search",
                            "snippet": "Search result summary"
                        }
                    ],
                    "citations": [
                        {
                            "title": "Codex docs",
                            "url": "https://example.com/docs"
                        }
                    ]
                }
            }),
            json!({
                "timestamp": "2026-04-24T01:09:05.000Z",
                "type": "event_msg",
                "payload": { "type": "image_generation_begin", "call_id": "image-1" }
            }),
            json!({
                "timestamp": "2026-04-24T01:09:06.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "image_generation_end",
                    "call_id": "image-1",
                    "status": "completed",
                    "revised_prompt": "small diagram",
                    "result": "iVBORw0KGgo=",
                    "saved_path": workspace.join("diagram.png").display().to_string()
                }
            }),
            json!({
                "timestamp": "2026-04-24T01:09:07.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "view_image_tool_call",
                    "call_id": "view-image-1",
                    "path": workspace.join("diagram.png").display().to_string()
                }
            }),
            json!({
                "timestamp": "2026-04-24T01:09:08.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "entered_review_mode",
                    "user_facing_hint": "Review requested."
                }
            }),
            json!({
                "timestamp": "2026-04-24T01:09:09.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "exited_review_mode",
                    "review_output": {
                        "findings": [],
                        "overall_correctness": "patch is correct",
                        "overall_explanation": "No findings.",
                        "overall_confidence_score": 0.88
                    }
                }
            }),
            json!({
                "timestamp": "2026-04-24T01:09:10.000Z",
                "type": "event_msg",
                "payload": { "type": "task_complete", "turn_id": "turn-rich" }
            }),
            json!({
                "timestamp": "2026-04-24T01:09:11.000Z",
                "type": "event_msg",
                "payload": { "type": "task_started", "turn_id": "turn-rolled" }
            }),
            json!({
                "timestamp": "2026-04-24T01:09:12.000Z",
                "type": "event_msg",
                "payload": { "type": "user_message", "message": "this turn should be removed" }
            }),
            json!({
                "timestamp": "2026-04-24T01:09:13.000Z",
                "type": "event_msg",
                "payload": { "type": "thread_rolled_back", "num_turns": 1 }
            })
        ),
    )
    .unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let detail = session_detail_payload(&state, "default", session_id, 20)
        .await
        .unwrap();
    let turns = detail
        .get("thread")
        .and_then(|thread| thread.get("turns"))
        .and_then(Value::as_array)
        .expect("rich rollout turns should be visible");
    assert_eq!(turns.len(), 1);
    let turn = session_turn_payload(&state, "default", session_id, "turn-rich")
        .await
        .unwrap()
        .get("turn")
        .cloned()
        .unwrap_or(Value::Null);
    let item_types = turn
        .get("items")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .filter_map(|item| item.get("type").and_then(Value::as_str))
        .collect::<Vec<_>>();
    assert!(item_types.contains(&"webSearch"));
    assert!(item_types.contains(&"imageGeneration"));
    assert!(item_types.contains(&"imageView"));
    assert!(item_types.contains(&"enteredReviewMode"));
    assert!(item_types.contains(&"exitedReviewMode"));
    let web_search = turn
        .get("items")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .find(|item| item.get("type").and_then(Value::as_str) == Some("webSearch"))
        .expect("web search item should be parsed");
    assert_eq!(
        web_search.get("summary").and_then(Value::as_str),
        Some("Found Codex web UI notes.")
    );
    assert_eq!(
        web_search
            .get("results")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        web_search
            .get("citations")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );
    assert!(!turns.iter().any(|turn| {
        serde_json::to_string(turn)
            .unwrap()
            .contains("this turn should be removed")
    }));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_session_detail_defers_large_image_generation_results() {
    let sandbox = unique_test_dir("session-detail-large-image");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let session_id = "019df000-0000-7000-8000-000000000223";
    let rollout_dir = codex_home
        .join("sessions")
        .join("2026")
        .join("04")
        .join("24");
    fs::create_dir_all(&rollout_dir).unwrap();
    let large_result = "a".repeat(300 * 1024);
    fs::write(
        rollout_dir.join(format!("rollout-2026-04-24T01-09-30-{session_id}.jsonl")),
        format!(
            "{}\n{}\n{}\n{}\n",
            json!({
                "timestamp": "2026-04-24T01:09:30.000Z",
                "type": "session_meta",
                "payload": {
                    "id": session_id,
                    "timestamp": "2026-04-24T01:09:30.000Z",
                    "cwd": workspace.display().to_string()
                }
            }),
            json!({
                "timestamp": "2026-04-24T01:09:31.000Z",
                "type": "event_msg",
                "payload": { "type": "task_started", "turn_id": "turn-large-image" }
            }),
            json!({
                "timestamp": "2026-04-24T01:09:32.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "image_generation_end",
                    "call_id": "image-large",
                    "status": "completed",
                    "result": large_result,
                    "saved_path": workspace.join("large.png").display().to_string()
                }
            }),
            json!({
                "timestamp": "2026-04-24T01:09:33.000Z",
                "type": "event_msg",
                "payload": { "type": "agent_message", "message": "image ready" }
            })
        ),
    )
    .unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let detail = session_detail_payload(&state, "default", session_id, 20)
        .await
        .unwrap();
    let summarized_image = detail
        .get("thread")
        .and_then(|thread| thread.get("turns"))
        .and_then(Value::as_array)
        .and_then(|turns| turns.first())
        .and_then(|turn| turn.get("items"))
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.get("type").and_then(Value::as_str) == Some("imageGeneration"))
        })
        .expect("active large image generation item should be summarized");
    assert!(summarized_image.get("result").is_some_and(Value::is_null));
    assert_eq!(
        summarized_image.get("detailState").and_then(Value::as_str),
        Some("deferred")
    );
    assert_eq!(
        summarized_image
            .get("resultOmitted")
            .and_then(Value::as_bool),
        Some(true)
    );

    let detail_item = session_item_detail_payload(
        &state,
        "default",
        session_id,
        "turn-large-image",
        "image-large",
    )
    .await
    .unwrap();
    assert_eq!(
        detail_item
            .get("item")
            .and_then(|item| item.get("result"))
            .and_then(Value::as_str)
            .map(str::len),
        Some(300 * 1024)
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_summary_uses_local_rollout_metadata_when_thread_read_is_slow() {
    let sandbox = unique_test_dir("session-summary-local-metadata");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let session_id = "019df000-0000-7000-8000-000000000222";
    write_rollout_fixture(
        &codex_home,
        false,
        "2026/04/24",
        "2026-04-24T01-10-00",
        session_id,
        &workspace,
        "open the local summary session",
        &[],
        Some(",\"name\":\"Local rollout summary\""),
    );

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": session_id,
                    "name": "Slow app-server summary",
                    "preview": "slow summary",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1_776_970_200_000_i64,
                    "updatedAt": 1_776_970_201_000_i64,
                    "status": "running",
                    "readDelayMs": 5_000,
                    "isSubagent": false,
                    "turns": []
                }
            }),
        )
        .await
        .unwrap();

    let summary = tokio::time::timeout(
        Duration::from_secs(1),
        build_session_summary_payload(&state, "default", session_id, None, None),
    )
    .await
    .expect("session summary should not wait for slow app-server thread/read")
    .unwrap();

    assert_eq!(
        summary.get("name").and_then(Value::as_str),
        Some("Local rollout summary")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_summary_confirms_cached_activity_with_app_server_before_clearing() {
    let sandbox = unique_test_dir("session-summary-confirm-active-before-clear");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let session_id = "019df000-0000-7000-8000-000000000223";
    write_rollout_fixture(
        &codex_home,
        false,
        "2026/04/24",
        "2026-04-24T01-11-00",
        session_id,
        &workspace,
        "local rollout still looks completed",
        &[],
        Some(",\"name\":\"Locally completed but app-server active\""),
    );

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": session_id,
                    "name": "Active app-server session",
                    "preview": "active app-server session",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1_776_970_260_000_i64,
                    "updatedAt": 1_776_970_261_000_i64,
                    "status": "running",
                    "isSubagent": false,
                    "agentNickname": Value::Null,
                    "agentRole": Value::Null,
                    "turns": [
                        {
                            "id": "turn-app-server-active",
                            "status": "inProgress",
                            "error": Value::Null,
                            "startedAt": 10,
                            "completedAt": Value::Null,
                            "durationMs": Value::Null,
                            "items": []
                        }
                    ]
                }
            }),
        )
        .await
        .unwrap();
    let runtime_key = runtime_session_key("default", session_id);
    state
        .active_turns
        .lock()
        .await
        .insert(runtime_key.clone(), "turn-cached".to_string());
    with_ui_state_write(&state, "default", |ui_state| {
        let Some(runtime_status_by_thread_id) = ui_state
            .get_mut("runtimeStatusByThreadId")
            .and_then(Value::as_object_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "runtime status state is missing",
            ));
        };
        runtime_status_by_thread_id.insert(
            session_id.to_string(),
            json!({
                "status": "running",
                "updatedAt": now_unix_ms().saturating_sub(10_000)
            }),
        );
        Ok(())
    })
    .await
    .unwrap();

    let payload = list_sessions_payload(
        &state,
        "default",
        false,
        None,
        20,
        &SessionFilterCriteria::default(),
    )
    .await
    .unwrap();
    let first = payload
        .get("sessions")
        .and_then(Value::as_array)
        .and_then(|sessions| sessions.first())
        .cloned()
        .expect("expected seeded session");

    assert_eq!(first.get("id").and_then(Value::as_str), Some(session_id));
    assert_eq!(first.get("status").and_then(Value::as_str), Some("running"));
    assert!(state.active_turns.lock().await.contains_key(&runtime_key));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_summary_snapshot_reuses_loaded_thread_ids_probe() {
    let sandbox = unique_test_dir("session-summary-loaded-cache");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let client = app_server_client(&state, "default").await.unwrap();
    client
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": "thread-loaded-cache",
                    "name": "Loaded cache",
                    "preview": "",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 1,
                    "status": "running",
                    "isSubagent": false,
                    "agentNickname": Value::Null,
                    "agentRole": Value::Null,
                    "turns": []
                }
            }),
        )
        .await
        .unwrap();

    read_session_summary_ui_snapshot(&state, "default")
        .await
        .unwrap();
    read_session_summary_ui_snapshot(&state, "default")
        .await
        .unwrap();

    let count = client
        .request(
            "debug/requestCount",
            json!({ "target": "thread/loaded/list" }),
        )
        .await
        .unwrap();
    assert_eq!(count.get("count").and_then(Value::as_u64), Some(1));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_detail_payload_clears_completed_highlight_on_open() {
    let sandbox = unique_test_dir("session-detail-highlight-clear");
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
                    "name": "Investigate bug",
                    "preview": "Investigate websocket bug",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 2,
                    "status": "completed",
                    "isSubagent": false,
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
                                    "type": "agentMessage",
                                    "text": "Done"
                                }
                            ]
                        }
                    ]
                }
            }),
        )
        .await
        .unwrap();

    with_ui_state_write(&state, "default", |ui_state| {
        let highlights = ui_state
            .get_mut("highlightsByThreadId")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                api_error(StatusCode::INTERNAL_SERVER_ERROR, "missing highlight state")
            })?;
        highlights.insert(
            "thread-1".to_string(),
            json!({
                "kind": "completed",
                "at": now_unix_ms()
            }),
        );
        Ok(())
    })
    .await
    .unwrap();

    session_detail_payload(&state, "default", "thread-1", 20)
        .await
        .unwrap();

    let highlight_after = with_ui_state_read(&state, "default", |ui_state| {
        Ok(ui_state
            .get("highlightsByThreadId")
            .and_then(Value::as_object)
            .and_then(|entries| entries.get("thread-1"))
            .cloned()
            .unwrap_or(Value::Null))
    })
    .await
    .unwrap();
    assert!(highlight_after.is_null());

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_turn_payload_uses_app_server_and_updates_session_state() {
    let sandbox = unique_test_dir("turn-send-rust");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let runtime_profile = resolve_runtime_profile(&state.config, "default");
    let uploads_dir = runtime_profile.data_dir.join("uploads").join("thread-1");
    fs::create_dir_all(&uploads_dir).unwrap();

    let text_attachment_path = workspace.join("notes.md");
    let image_attachment_path = workspace.join("diagram.png");
    fs::write(&text_attachment_path, "attachment").unwrap();
    fs::write(&image_attachment_path, "png").unwrap();
    fs::write(
        uploads_dir.join("att-file.json"),
        serde_json::to_vec(&json!({
            "id": "att-file",
            "originalName": "notes.md",
            "path": text_attachment_path.display().to_string(),
            "mimeType": "text/markdown",
            "size": 10,
            "kind": "file",
            "createdAt": "2026-04-20T00:00:00Z"
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        uploads_dir.join("att-image.json"),
        serde_json::to_vec(&json!({
            "id": "att-image",
            "originalName": "diagram.png",
            "path": image_attachment_path.display().to_string(),
            "mimeType": "image/png",
            "size": 12,
            "kind": "image",
            "createdAt": "2026-04-20T00:00:01Z"
        }))
        .unwrap(),
    )
    .unwrap();

    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": "thread-1",
                    "name": "New thread",
                    "preview": "",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 1,
                    "status": "notLoaded",
                    "isSubagent": false,
                    "agentNickname": Value::Null,
                    "agentRole": Value::Null,
                    "turns": []
                }
            }),
        )
        .await
        .unwrap();

    save_session_draft_payload(&state, "default", "thread-1", "Draft to clear", "message")
        .await
        .unwrap();

    let prompt = "Inspect the duplicated websocket send behaviour and capture the root cause before patching it.";
    let selected_skills = json!([
        {
            "id": "skill-1",
            "name": "imagegen",
            "path": "/skills/imagegen/SKILL.md"
        }
    ]);
    let payload = send_turn_payload(
        &state,
        "default",
        "thread-1",
        prompt,
        Some(&json!(["att-file", "att-image"])),
        Some(&selected_skills),
        json!({
            "cwd": workspace.display().to_string(),
            "model": "gpt-5",
            "approvalPolicy": "on-request",
            "sandboxMode": "workspace-write",
            "speed": "fast",
            "effort": "high",
            "networkAccess": true
        }),
        Some("client-send-1"),
    )
    .await
    .unwrap();

    assert_eq!(payload.get("ok").and_then(Value::as_bool), Some(true));
    assert_eq!(
        payload.get("turnId").and_then(Value::as_str),
        Some("turn-1")
    );

    let thread = read_thread_payload(&state, "default", "thread-1", true)
        .await
        .unwrap();
    assert_eq!(
        thread.get("status").and_then(Value::as_str),
        Some("running")
    );
    assert_eq!(thread.get("resumeCount").and_then(Value::as_u64), Some(1));
    assert_eq!(
        thread.get("name").and_then(Value::as_str),
        Some("New thread")
    );
    assert_eq!(
        thread.get("turns").and_then(Value::as_array).map(Vec::len),
        Some(1)
    );

    let provisional_summary =
        build_session_summary_payload(&state, "default", "thread-1", None, None)
            .await
            .unwrap();
    assert_eq!(
        provisional_summary.get("name").and_then(Value::as_str),
        infer_session_display_title(prompt).as_deref()
    );

    let last_turn_start = thread.get("lastTurnStart").cloned().unwrap_or(Value::Null);
    assert_eq!(
        last_turn_start.get("serviceTier").and_then(Value::as_str),
        Some("fast")
    );
    assert_eq!(
        last_turn_start
            .get("clientUserMessageId")
            .and_then(Value::as_str),
        Some("client-send-1")
    );
    assert_eq!(
        last_turn_start
            .get("responsesapiClientMetadata")
            .and_then(|metadata| metadata.get("clientUserMessageId"))
            .and_then(Value::as_str),
        Some("client-send-1")
    );
    let input = last_turn_start
        .get("input")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(input.len(), 3);
    let first_text = input
        .first()
        .and_then(|value| value.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(first_text.contains("$imagegen"));
    assert!(first_text.contains(ATTACHMENT_PREAMBLE_START));
    assert!(first_text.contains(ATTACHMENT_PREAMBLE_END));
    assert!(first_text.contains(text_attachment_path.to_str().unwrap()));
    assert!(first_text.contains(prompt.trim()));
    assert_eq!(
        input
            .get(2)
            .and_then(|value| value.get("path"))
            .and_then(Value::as_str),
        Some(image_attachment_path.to_str().unwrap())
    );
    assert_eq!(
        input
            .get(1)
            .and_then(|value| value.get("type"))
            .and_then(Value::as_str),
        Some("skill")
    );
    assert_eq!(
        input
            .get(1)
            .and_then(|value| value.get("name"))
            .and_then(Value::as_str),
        Some("imagegen")
    );
    let user_message = thread
        .get("turns")
        .and_then(Value::as_array)
        .and_then(|turns| turns.first())
        .and_then(|turn| turn.get("items"))
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        user_message.get("clientId").and_then(Value::as_str),
        Some("client-send-1")
    );

    handle_profile_runtime_notification(
        &state,
        "default",
        &AppServerNotification {
            method: "thread/name/updated".to_string(),
            params: json!({
                "threadId": "thread-1",
                "threadName": "Investigate duplicate websocket sends"
            }),
        },
    )
    .await;

    let ai_summary = build_session_summary_payload(&state, "default", "thread-1", None, None)
        .await
        .unwrap();
    assert_eq!(
        ai_summary.get("name").and_then(Value::as_str),
        Some("Investigate duplicate websocket sends")
    );

    handle_profile_runtime_notification(
        &state,
        "default",
        &AppServerNotification {
            method: "thread/name/updated".to_string(),
            params: json!({
                "thread_id": "thread-1",
                "thread_name": "AI title from snake case event"
            }),
        },
    )
    .await;

    let snake_case_summary =
        build_session_summary_payload(&state, "default", "thread-1", None, None)
            .await
            .unwrap();
    assert_eq!(
        snake_case_summary.get("name").and_then(Value::as_str),
        Some("AI title from snake case event")
    );

    let stored_preferences = with_ui_state_read(&state, "default", |ui_state| {
        Ok(ui_state
            .get("preferencesByThreadId")
            .and_then(Value::as_object)
            .and_then(|entries| entries.get("thread-1"))
            .cloned()
            .unwrap_or(Value::Null))
    })
    .await
    .unwrap();
    assert_eq!(
        stored_preferences.get("model").and_then(Value::as_str),
        Some("gpt-5")
    );

    let draft = get_session_draft_payload(&state, "default", "thread-1")
        .await
        .unwrap();
    assert_eq!(draft.get("draft").and_then(Value::as_str), Some(""));

    let runtime_key = runtime_session_key(
        resolve_runtime_profile_entry(&state.config, "default").0,
        "thread-1",
    );
    assert_eq!(
        state.active_turns.lock().await.get(&runtime_key).cloned(),
        Some("turn-1".to_string())
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_turn_payload_rejects_concurrent_turn_starts() {
    let sandbox = unique_test_dir("turn-send-duplicate-start");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let created = create_session_payload(
        &state,
        "default",
        json!({
            "cwd": workspace.display().to_string(),
            "model": "gpt-5"
        }),
        None,
        Some("Duplicate start guard"),
    )
    .await
    .unwrap();
    let session_id = created
        .get("id")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();
    let client = app_server_client(&state, "default").await.unwrap();
    client
        .request(
            "debug/setDelay",
            json!({
                "method": "turn/start",
                "delayMs": 200
            }),
        )
        .await
        .unwrap();

    let preferences = json!({
        "cwd": workspace.display().to_string(),
        "model": "gpt-5"
    });
    let first = send_turn_payload(
        &state,
        "default",
        &session_id,
        "Start a turn once.",
        None,
        None,
        preferences.clone(),
        None,
    );
    let second = async {
        tokio::time::sleep(Duration::from_millis(25)).await;
        send_turn_payload(
            &state,
            "default",
            &session_id,
            "Start a turn twice.",
            None,
            None,
            preferences.clone(),
            None,
        )
        .await
    };
    let (first_result, second_result) = tokio::join!(first, second);

    assert_eq!(
        first_result.unwrap().get("turnId").and_then(Value::as_str),
        Some("turn-1")
    );
    let second_error = second_result.unwrap_err();
    assert_eq!(second_error.status, StatusCode::CONFLICT);
    assert!(second_error.message.contains("TURN_ALREADY_STARTING"));

    let request_count = client
        .request("debug/requestCount", json!({ "target": "turn/start" }))
        .await
        .unwrap();
    assert_eq!(request_count.get("count").and_then(Value::as_u64), Some(1));

    let thread = read_thread_payload(&state, "default", &session_id, true)
        .await
        .unwrap();
    assert_eq!(
        thread.get("turns").and_then(Value::as_array).map(Vec::len),
        Some(1)
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test]
async fn send_turn_payload_translates_language_bridge_prompt_with_temp_session() {
    let sandbox = unique_test_dir("turn-language-bridge");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let created = create_session_payload(
        &state,
        "default",
        json!({
            "cwd": workspace.display().to_string(),
            "model": "gpt-5",
            "languageBridgeEnabled": true,
            "languageBridgeOutputLanguage": "Korean"
        }),
        None,
        Some("Language bridge"),
    )
    .await
    .unwrap();
    let session_id = created
        .get("id")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();

    send_turn_payload(
        &state,
        "default",
        &session_id,
        "요약해줘.",
        None,
        None,
        json!({
            "cwd": workspace.display().to_string(),
            "model": "gpt-5",
            "languageBridgeEnabled": true,
            "languageBridgeOutputLanguage": "Korean"
        }),
        None,
    )
    .await
    .unwrap();

    let client = app_server_client(&state, "default").await.unwrap();
    let raw_thread = client
        .request(
            "thread/read",
            json!({
                "threadId": session_id,
                "includeTurns": true
            }),
        )
        .await
        .unwrap();
    let last_turn_start = raw_thread
        .get("thread")
        .and_then(|thread| thread.get("lastTurnStart"))
        .expect("fake app-server should record last turn/start params");
    assert_eq!(
        last_turn_start
            .get("input")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|item| item.get("text"))
            .and_then(Value::as_str),
        Some("Summarize it.")
    );
    let instructions = last_turn_start
        .get("collaborationMode")
        .and_then(|mode| mode.get("settings"))
        .and_then(|settings| settings.get("developer_instructions"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(instructions.contains("Language bridge is enabled"));
    assert!(instructions.contains("Korean"));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completed_turn_uses_language_bridge_response_translation_subagent() {
    let sandbox = unique_test_dir("turn-language-bridge-response-translation");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let created = create_session_payload(
        &state,
        "default",
        json!({
            "cwd": workspace.display().to_string(),
            "model": "gpt-5",
            "languageBridgeEnabled": true,
            "languageBridgeOutputLanguage": "Korean"
        }),
        None,
        Some("Language bridge response"),
    )
    .await
    .unwrap();
    let session_id = created
        .get("id")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();

    handle_profile_runtime_notification(
        &state,
        "default",
        &AppServerNotification {
            method: "turn/completed".to_string(),
            params: json!({
                "threadId": session_id,
                "turn": {
                    "id": "turn-response",
                    "status": "completed",
                    "items": [
                        {
                            "id": "turn-response:agent:0",
                            "type": "agentMessage",
                            "text": "This is the final English answer."
                        }
                    ]
                }
            }),
        },
    )
    .await;

    let mut translated = None;
    for _ in 0..40 {
        translated = with_ui_state_read(&state, "default", |ui_state| {
            Ok(ui_state
                .get("languageBridgeByThreadId")
                .and_then(Value::as_object)
                .and_then(|entries| entries.get(&session_id))
                .and_then(|entry| entry.get("translationsByItemId"))
                .and_then(Value::as_object)
                .and_then(|translations| translations.get("turn-response:agent:0"))
                .and_then(|translation| translation.get("text"))
                .and_then(Value::as_str)
                .map(str::to_string))
        })
        .await
        .unwrap();
        if translated.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(translated.as_deref(), Some("번역된 응답입니다."));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_turn_payload_rejects_recent_active_turn_starts() {
    let sandbox = unique_test_dir("turn-send-active-duplicate");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let created = create_session_payload(
        &state,
        "default",
        json!({
            "cwd": workspace.display().to_string(),
            "model": "gpt-5"
        }),
        None,
        Some("Recent active guard"),
    )
    .await
    .unwrap();
    let session_id = created
        .get("id")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();
    let preferences = json!({
        "cwd": workspace.display().to_string(),
        "model": "gpt-5"
    });
    let first_result = send_turn_payload(
        &state,
        "default",
        &session_id,
        "Start a turn and keep it active.",
        None,
        None,
        preferences.clone(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        first_result.get("turnId").and_then(Value::as_str),
        Some("turn-1")
    );

    let second_error = send_turn_payload(
        &state,
        "default",
        &session_id,
        "This should not become a second active turn.",
        None,
        None,
        preferences,
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(second_error.status, StatusCode::CONFLICT);
    assert!(second_error.message.contains("TURN_ALREADY_RUNNING"));

    let request_count = app_server_client(&state, "default")
        .await
        .unwrap()
        .request("debug/requestCount", json!({ "target": "turn/start" }))
        .await
        .unwrap();
    assert_eq!(request_count.get("count").and_then(Value::as_u64), Some(1));

    let thread = read_thread_payload(&state, "default", &session_id, true)
        .await
        .unwrap();
    assert_eq!(
        thread.get("turns").and_then(Value::as_array).map(Vec::len),
        Some(1)
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steer_turn_payload_uses_active_turn_from_thread_reads() {
    let sandbox = unique_test_dir("turn-steer-rust");
    let workspace = sandbox.join("workspace");
    let codex_home = sandbox.join("codex-home");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&codex_home).unwrap();

    let state =
        test_state_with_fake_app_server(workspace.clone(), vec![workspace.clone()], codex_home);
    let runtime_profile = resolve_runtime_profile(&state.config, "default");
    let uploads_dir = runtime_profile.data_dir.join("uploads").join("thread-1");
    fs::create_dir_all(&uploads_dir).unwrap();

    let text_attachment_path = workspace.join("handoff.md");
    fs::write(&text_attachment_path, "handoff").unwrap();
    fs::write(
        uploads_dir.join("att-file.json"),
        serde_json::to_vec(&json!({
            "id": "att-file",
            "originalName": "handoff.md",
            "path": text_attachment_path.display().to_string(),
            "mimeType": "text/markdown",
            "size": 7,
            "kind": "file",
            "createdAt": "2026-04-20T00:00:00Z"
        }))
        .unwrap(),
    )
    .unwrap();

    app_server_client(&state, "default")
        .await
        .unwrap()
        .request(
            "thread/seed",
            json!({
                "thread": {
                    "id": "thread-1",
                    "name": "Investigate queue",
                    "preview": "Investigate queue",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 2,
                    "status": "running",
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
                            "items": []
                        },
                        {
                            "id": "turn-2",
                            "status": "inProgress",
                            "error": Value::Null,
                            "startedAt": 30,
                            "completedAt": Value::Null,
                            "durationMs": Value::Null,
                            "items": []
                        }
                    ]
                }
            }),
        )
        .await
        .unwrap();

    save_session_draft_payload(&state, "default", "thread-1", "Steer draft", "steer")
        .await
        .unwrap();

    let payload = steer_turn_payload(
        &state,
        "default",
        "thread-1",
        "Focus on the queue deduplication race first.",
        Some(&json!(["att-file"])),
        None,
        None,
        Some("client-steer-1"),
    )
    .await
    .unwrap();

    assert_eq!(payload.get("ok").and_then(Value::as_bool), Some(true));
    assert_eq!(
        payload.get("turnId").and_then(Value::as_str),
        Some("turn-2")
    );

    let thread = read_thread_payload(&state, "default", "thread-1", true)
        .await
        .unwrap();
    let last_turn_steer = thread.get("lastTurnSteer").cloned().unwrap_or(Value::Null);
    assert_eq!(
        last_turn_steer
            .get("expectedTurnId")
            .and_then(Value::as_str),
        Some("turn-2")
    );
    assert_eq!(
        last_turn_steer
            .get("clientUserMessageId")
            .and_then(Value::as_str),
        Some("client-steer-1")
    );
    assert_eq!(
        last_turn_steer
            .get("responsesapiClientMetadata")
            .and_then(|metadata| metadata.get("clientUserMessageId"))
            .and_then(Value::as_str),
        Some("client-steer-1")
    );
    let first_text = last_turn_steer
        .get("input")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|value| value.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(first_text.contains(ATTACHMENT_PREAMBLE_START));
    assert!(first_text.contains(text_attachment_path.to_str().unwrap()));
    assert!(first_text.contains("Focus on the queue deduplication race first."));

    let runtime_key = runtime_session_key(
        resolve_runtime_profile_entry(&state.config, "default").0,
        "thread-1",
    );
    assert_eq!(
        state.active_turns.lock().await.get(&runtime_key).cloned(),
        Some("turn-2".to_string())
    );

    let draft = get_session_draft_payload(&state, "default", "thread-1")
        .await
        .unwrap();
    assert_eq!(draft.get("draft").and_then(Value::as_str), Some(""));

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steer_turn_payload_uses_expected_turn_id_without_thread_read() {
    let sandbox = unique_test_dir("turn-steer-expected-turn-rust");
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
                    "id": "thread-expected",
                    "name": "Expected steer",
                    "preview": "Expected steer",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 2,
                    "status": "running",
                    "turns": []
                }
            }),
        )
        .await
        .unwrap();

    let payload = steer_turn_payload(
        &state,
        "default",
        "thread-expected",
        "Apply this immediately.",
        None,
        None,
        Some("turn-known"),
        None,
    )
    .await
    .unwrap();

    assert_eq!(
        payload.get("turnId").and_then(Value::as_str),
        Some("turn-known")
    );
    let thread_read_count = app_server_client(&state, "default")
        .await
        .unwrap()
        .request("debug/requestCount", json!({ "target": "thread/read" }))
        .await
        .unwrap();
    assert_eq!(
        thread_read_count.get("count").and_then(Value::as_u64),
        Some(0)
    );
    let thread = read_thread_payload(&state, "default", "thread-expected", false)
        .await
        .unwrap();
    assert_eq!(
        thread
            .get("lastTurnSteer")
            .and_then(|steer| steer.get("expectedTurnId"))
            .and_then(Value::as_str),
        Some("turn-known")
    );

    let _ = fs::remove_dir_all(sandbox);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_detail_preserves_cached_active_turn_when_thread_payload_lags() {
    let sandbox = unique_test_dir("session-detail-active-turn-lag");
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
                    "name": "Lagging thread",
                    "preview": "Lagging thread",
                    "cwd": workspace.display().to_string(),
                    "archived": false,
                    "createdAt": 1,
                    "updatedAt": 2,
                    "status": "running",
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
                            "items": []
                        }
                    ]
                }
            }),
        )
        .await
        .unwrap();
    let runtime_key = runtime_session_key(
        resolve_runtime_profile_entry(&state.config, "default").0,
        "thread-1",
    );
    state
        .active_turns
        .lock()
        .await
        .insert(runtime_key.clone(), "turn-2".to_string());

    let detail = session_detail_payload(&state, "default", "thread-1", 20)
        .await
        .unwrap();

    assert_eq!(
        detail.get("activeTurnId").and_then(Value::as_str),
        Some("turn-2")
    );
    assert_eq!(
        state.active_turns.lock().await.get(&runtime_key).cloned(),
        Some("turn-2".to_string())
    );

    let _ = fs::remove_dir_all(sandbox);
}
