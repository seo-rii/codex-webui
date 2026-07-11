use super::*;
use crate::thread_listing_support::{
    emit_session_status_summary_updated, emit_session_summary_updated,
};
use crate::thread_read_support::{
    active_turn_id_from_turns, emit_session_notification, is_unmaterialized_thread_error_message,
    read_thread_payload,
};

const DUPLICATE_TURN_START_ACTIVE_GRACE_MS: u64 = 30_000;
const RECENT_CLIENT_USER_MESSAGE_TTL: Duration = Duration::from_secs(60 * 30);
const LANGUAGE_BRIDGE_TRANSLATION_TIMEOUT: Duration = Duration::from_secs(180);
const LANGUAGE_BRIDGE_TRANSLATION_POLL_INTERVAL: Duration = Duration::from_millis(500);
const CODEX_PERMISSION_PROFILE_READ_ONLY: &str = ":read-only";
const CODEX_PERMISSION_PROFILE_WORKSPACE: &str = ":workspace";
const CODEX_PERMISSION_PROFILE_DANGER_FULL_ACCESS: &str = ":danger-full-access";
const CODEX_DEPRECATED_RESTRICTED_READ_ERROR: &str = "readOnly.access is no longer supported";

fn permission_profile_for_sandbox_mode(sandbox_mode: &str) -> &'static str {
    match sandbox_mode {
        "danger-full-access" => CODEX_PERMISSION_PROFILE_DANGER_FULL_ACCESS,
        "read-only" => CODEX_PERMISSION_PROFILE_READ_ONLY,
        _ => CODEX_PERMISSION_PROFILE_WORKSPACE,
    }
}

fn language_bridge_start_error(context: &str, error: impl std::fmt::Display) -> ApiError {
    let message = error.to_string();
    let details = if message.contains(CODEX_DEPRECATED_RESTRICTED_READ_ERROR)
        || message.contains("workspaceWrite.readOnlyAccess is no longer supported")
    {
        format!(
            "{context}: Codex rejected a deprecated sandboxPolicy payload. This WebUI build sends language bridge requests with permissions={CODEX_PERMISSION_PROFILE_READ_ONLY} and no sandboxPolicy; rebuild or safe-restart the gateway if an older backend process is still serving requests. Original error: {message}"
        )
    } else {
        format!("{context}: {message}")
    };
    api_error(StatusCode::BAD_GATEWAY, details)
}

pub(crate) fn normalize_client_user_message_id(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(160).collect::<String>())
}

fn responsesapi_client_metadata_for_user_message(client_user_message_id: Option<&str>) -> Value {
    client_user_message_id
        .map(|client_user_message_id| {
            json!({
                "clientUserMessageId": client_user_message_id
            })
        })
        .unwrap_or(Value::Null)
}

fn client_user_message_key(runtime_key: &str, client_user_message_id: &str) -> String {
    format!("{runtime_key}::client-user-message::{client_user_message_id}")
}

async fn recently_sent_client_user_message(
    state: &AppState,
    runtime_key: &str,
    client_user_message_id: &str,
) -> bool {
    let key = client_user_message_key(runtime_key, client_user_message_id);
    let now = Instant::now();
    let mut recent = state.recent_client_user_messages.lock().await;
    recent.retain(|_, sent_at| now.duration_since(*sent_at) <= RECENT_CLIENT_USER_MESSAGE_TTL);
    recent.contains_key(&key)
}

async fn remember_sent_client_user_message(
    state: &AppState,
    runtime_key: &str,
    client_user_message_id: &str,
) {
    let key = client_user_message_key(runtime_key, client_user_message_id);
    let now = Instant::now();
    let mut recent = state.recent_client_user_messages.lock().await;
    recent.retain(|_, sent_at| now.duration_since(*sent_at) <= RECENT_CLIENT_USER_MESSAGE_TTL);
    recent.insert(key, now);
}

async fn resolve_selected_attachment_records(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    attachment_ids: Option<&Value>,
) -> ApiResult<Vec<StoredAttachmentRecord>> {
    let requested_attachment_ids = string_array_from_value(attachment_ids);
    let requested_attachment_set = requested_attachment_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let attachments = list_session_attachment_records(state, profile_id, session_id)
        .await
        .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;

    if requested_attachment_set.is_empty() {
        return Ok(Vec::new());
    }

    Ok(attachments
        .into_iter()
        .filter(|attachment| requested_attachment_set.contains(attachment.id.as_str()))
        .collect())
}

pub(crate) fn build_turn_input_payload(
    prompt: &str,
    attachments: &[StoredAttachmentRecord],
    selected_skills: &[Value],
) -> (Vec<Value>, Vec<String>) {
    let mut additional_readable_roots = Vec::new();
    let mut readable_roots_seen = HashSet::new();
    let mut text_attachment_paths = Vec::new();
    let mut image_attachment_paths = Vec::new();

    for attachment in attachments {
        if let Some(path) = attachment.path.as_deref() {
            let readable_root = Path::new(path)
                .parent()
                .unwrap_or_else(|| Path::new(path))
                .display()
                .to_string();
            if readable_roots_seen.insert(readable_root.clone()) {
                additional_readable_roots.push(readable_root);
            }
            if attachment.kind.as_deref() == Some("image") {
                image_attachment_paths.push(path.to_string());
            } else {
                text_attachment_paths.push(path.to_string());
            }
        }
    }

    let mut reference_markers = Vec::new();
    let mut reference_items = Vec::new();
    for selected in selected_skills {
        let name = selected
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let path = selected
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if name.is_empty() || path.is_empty() {
            continue;
        }

        if path.starts_with("plugin://") || path.starts_with("app://") {
            let marker = if let Some(plugin_id) = path
                .strip_prefix("plugin://")
                .and_then(|value| value.split('@').next())
                .filter(|value| !value.is_empty())
            {
                format!("@{plugin_id}")
            } else {
                let app_slug_source = name
                    .chars()
                    .map(|ch| {
                        if ch.is_ascii_alphanumeric() {
                            ch.to_ascii_lowercase()
                        } else {
                            '-'
                        }
                    })
                    .collect::<String>();
                let app_slug = app_slug_source
                    .split('-')
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join("-");
                format!("${}", if app_slug.is_empty() { name } else { &app_slug })
            };
            if !prompt.contains(&marker) {
                reference_markers.push(marker);
            }
            reference_items.push(json!({
                "type": "mention",
                "name": name,
                "path": path
            }));
            continue;
        }

        let marker = format!("${name}");
        if !prompt.contains(&marker) {
            reference_markers.push(marker);
        }
        reference_items.push(json!({
            "type": "skill",
            "name": name,
            "path": path
        }));
    }

    let text_body = if reference_markers.is_empty() {
        prompt.to_string()
    } else if prompt.trim().is_empty() {
        reference_markers.join("\n")
    } else {
        format!("{}\n\n{prompt}", reference_markers.join("\n"))
    };

    let mut input = vec![json!({
        "type": "text",
        "text": if text_attachment_paths.is_empty() {
            text_body.clone()
        } else {
            format!(
                "{ATTACHMENT_PREAMBLE_START}\n{}\n{ATTACHMENT_PREAMBLE_END}\n\n{text_body}",
                text_attachment_paths.join("\n")
            )
        },
        "text_elements": []
    })];
    input.extend(reference_items);
    for image_path in image_attachment_paths {
        input.push(json!({
            "type": "localImage",
            "path": image_path
        }));
    }

    (input, additional_readable_roots)
}

fn language_bridge_enabled(preferences: &Value) -> bool {
    preferences
        .get("languageBridgeEnabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn language_bridge_output_language(preferences: &Value) -> String {
    normalize_language_bridge_output_language(
        preferences
            .get("languageBridgeOutputLanguage")
            .and_then(Value::as_str)
            .unwrap_or("auto"),
    )
}

fn language_bridge_developer_instructions(
    preferences: &Value,
    resolved_output_language: Option<&str>,
) -> Option<String> {
    if !preferences
        .get("languageBridgeEnabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return None;
    }

    let output_language = resolved_output_language
        .map(normalize_language_bridge_output_language)
        .unwrap_or_else(|| language_bridge_output_language(preferences));
    let output_language_instruction = if output_language.eq_ignore_ascii_case("auto") {
        "the same natural language as the user's latest message".to_string()
    } else {
        output_language
    };

    Some(format!(
        "Language bridge is enabled for this session.\n\
         - Internally translate non-English user requests into English before planning or tool use.\n\
         - Do not show translation notes or the translated prompt to the user.\n\
         - Keep code, commands, file paths, identifiers, logs, errors, and quoted text in their original language unless explicitly asked to translate them.\n\
         - Write the working answer in English; Codex Web UI will ask a private response translation subagent to produce the final user-facing answer in {output_language_instruction}.\n\
         - Continue following the selected collaboration mode's normal behavior."
    ))
}

fn collaboration_mode_payload(
    preferences: &Value,
    model: &Value,
    resolved_output_language: Option<&str>,
) -> Value {
    let mode = preferences
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("default");
    let language_bridge_instructions =
        language_bridge_developer_instructions(preferences, resolved_output_language);
    if mode != "plan" && language_bridge_instructions.is_none() {
        return Value::Null;
    }

    let model = model
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("gpt-5")
        .to_string();
    let reasoning_effort = preferences.get("effort").cloned().unwrap_or(Value::Null);
    json!({
        "mode": if mode == "plan" { "plan" } else { "default" },
        "settings": {
            "model": model,
            "reasoning_effort": reasoning_effort,
            "developer_instructions": language_bridge_instructions
                .map(Value::String)
                .unwrap_or(Value::Null)
        }
    })
}

fn detect_prompt_language(prompt: &str) -> String {
    if prompt
        .chars()
        .any(|ch| ('\u{ac00}'..='\u{d7af}').contains(&ch))
    {
        return "Korean".to_string();
    }
    if prompt.chars().any(|ch| {
        ('\u{3040}'..='\u{30ff}').contains(&ch) || ('\u{31f0}'..='\u{31ff}').contains(&ch)
    }) {
        return "Japanese".to_string();
    }
    if prompt
        .chars()
        .any(|ch| ('\u{4e00}'..='\u{9fff}').contains(&ch))
    {
        return "Chinese".to_string();
    }
    if prompt
        .chars()
        .any(|ch| ('\u{0400}'..='\u{04ff}').contains(&ch))
    {
        return "Russian".to_string();
    }
    "English".to_string()
}

fn language_bridge_translation_prompt(prompt: &str) -> String {
    format!(
        "Translate the following user request into clear, concise English for a coding agent.\n\
         Do not answer the request. Preserve code blocks, commands, file paths, identifiers, quoted strings, logs, and error messages exactly unless translating surrounding natural language is necessary.\n\
         Return compact JSON only with this shape: {{\"english\":\"...\",\"language\":\"...\"}}. The language field must be the original user's natural language in English, such as Korean, Japanese, Chinese, French, Spanish, or English.\n\n\
         <user_request>\n{prompt}\n</user_request>"
    )
}

fn strip_json_code_fence(value: &str) -> &str {
    let trimmed = value.trim();
    let Some(stripped) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let Some((_, body)) = stripped.split_once('\n') else {
        return trimmed;
    };
    body.rsplit_once("```")
        .map(|(body, _)| body.trim())
        .unwrap_or(body.trim())
}

fn parse_language_bridge_translation_response(
    response: &str,
    fallback_language: &str,
) -> Option<(String, String)> {
    let parsed = serde_json::from_str::<Value>(strip_json_code_fence(response)).ok()?;
    let english = parsed
        .get("english")
        .or_else(|| parsed.get("text"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    let language = parsed
        .get("language")
        .or_else(|| parsed.get("sourceLanguage"))
        .and_then(Value::as_str)
        .map(normalize_language_bridge_output_language)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| fallback_language.to_string());
    Some((english, language))
}

fn extract_agent_message_item_from_turn(turn: &Value) -> Option<(String, String)> {
    let turn_id = turn
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("turn")
        .to_string();
    let items = turn.get("items").and_then(Value::as_array)?;
    for (item_index, item) in items.iter().enumerate().rev() {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
        if !matches!(
            item_type,
            "agentMessage" | "assistantMessage" | "agent_message" | "assistant_message"
        ) {
            continue;
        }
        let Some(text) = item
            .get("text")
            .or_else(|| item.get("message"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let item_id = item
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("{turn_id}:item:{item_index}"));
        return Some((item_id, text.to_string()));
    }
    None
}

fn extract_agent_message_text(thread: &Value) -> Option<String> {
    let turns = thread
        .get("turns")
        .and_then(Value::as_array)
        .map(Vec::as_slice)?;
    for turn in turns.iter().rev() {
        if let Some((_, text)) = extract_agent_message_item_from_turn(turn) {
            return Some(text);
        }
    }
    None
}

fn ensure_language_bridge_thread_state<'a>(
    ui_state: &'a mut Value,
    session_id: &str,
) -> ApiResult<&'a mut serde_json::Map<String, Value>> {
    let root = ui_state.as_object_mut().ok_or_else(|| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "language bridge state root is missing",
        )
    })?;
    if !root
        .get("languageBridgeByThreadId")
        .is_some_and(Value::is_object)
    {
        root.insert("languageBridgeByThreadId".to_string(), json!({}));
    }
    let bridge_by_thread_id = root
        .get_mut("languageBridgeByThreadId")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "language bridge state is missing",
            )
        })?;
    if !bridge_by_thread_id
        .get(session_id)
        .is_some_and(Value::is_object)
    {
        bridge_by_thread_id.insert(session_id.to_string(), json!({}));
    }
    bridge_by_thread_id
        .get_mut(session_id)
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "session language bridge state is missing",
            )
        })
}

async fn remember_language_bridge_output_language(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    output_language: &str,
) -> ApiResult<()> {
    let normalized = normalize_language_bridge_output_language(output_language);
    with_ui_state_write(state, profile_id, |ui_state| {
        let thread_state = ensure_language_bridge_thread_state(ui_state, session_id)?;
        thread_state.insert("outputLanguage".to_string(), Value::String(normalized));
        thread_state.insert("updatedAt".to_string(), json!(now_unix_ms()));
        Ok(())
    })
    .await
}

async fn translate_prompt_with_language_bridge(
    client: &AppServerClient,
    preferences: &Value,
    cwd: &str,
    prompt: &str,
) -> ApiResult<(String, String)> {
    let fallback_language = detect_prompt_language(prompt);
    let requested_output_language = language_bridge_output_language(preferences);
    let start_response = client
        .request(
            "thread/start",
            json!({
                "cwd": cwd,
                "model": preferences.get("model").cloned().unwrap_or(Value::Null),
                "permissions": CODEX_PERMISSION_PROFILE_READ_ONLY,
                "developerInstructions": "You are a private translation preprocessor for Codex Web UI. Return only the requested JSON. Do not call tools and do not solve the user's task.",
                "ephemeral": true,
                "personality": "none"
            }),
        )
        .await
        .map_err(|error| language_bridge_start_error("Failed to start language bridge session", error))?;
    let temp_thread_id = start_response
        .get("thread")
        .and_then(|thread| thread.get("id"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            api_error(
                StatusCode::BAD_GATEWAY,
                "Language bridge session did not return a thread id.",
            )
        })?;
    let translation_prompt = language_bridge_translation_prompt(prompt);
    let (input, _) = build_turn_input_payload(&translation_prompt, &[], &[]);
    client
        .request(
            "turn/start",
            json!({
                "threadId": temp_thread_id,
                "input": input,
                "cwd": cwd,
                "model": preferences.get("model").cloned().unwrap_or(Value::Null),
                "approvalPolicy": "never",
                "permissions": CODEX_PERMISSION_PROFILE_READ_ONLY,
                "runtimeWorkspaceRoots": [cwd]
            }),
        )
        .await
        .map_err(|error| {
            language_bridge_start_error("Failed to start language bridge translation", error)
        })?;

    let deadline = tokio::time::Instant::now() + LANGUAGE_BRIDGE_TRANSLATION_TIMEOUT;
    loop {
        let response = client
            .request(
                "thread/read",
                json!({
                    "threadId": temp_thread_id,
                    "includeTurns": true
                }),
            )
            .await
            .map_err(|error| {
                api_error(
                    StatusCode::BAD_GATEWAY,
                    format!("Failed to read language bridge translation: {error}"),
                )
            })?;
        if let Some(raw_text) = response.get("thread").and_then(extract_agent_message_text) {
            let (english, detected_language) =
                parse_language_bridge_translation_response(&raw_text, &fallback_language)
                    .unwrap_or_else(|| (raw_text, fallback_language.clone()));
            let output_language = if requested_output_language.eq_ignore_ascii_case("auto") {
                detected_language
            } else {
                requested_output_language
            };
            return Ok((english, output_language));
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(api_error(
                StatusCode::GATEWAY_TIMEOUT,
                "Timed out while translating the prompt with language bridge.",
            ));
        }
        tokio::time::sleep(LANGUAGE_BRIDGE_TRANSLATION_POLL_INTERVAL).await;
    }
}

pub(crate) async fn apply_language_bridge_translations_to_turns(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    turns: &mut [Value],
) -> ApiResult<()> {
    let translations = with_ui_state_read(state, profile_id, |ui_state| {
        Ok(ui_state
            .get("languageBridgeByThreadId")
            .and_then(Value::as_object)
            .and_then(|entries| entries.get(session_id))
            .and_then(|entry| entry.get("translationsByItemId"))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default())
    })
    .await?;
    if translations.is_empty() {
        return Ok(());
    }

    for turn in turns {
        let Some(items) = turn.get_mut("items").and_then(Value::as_array_mut) else {
            continue;
        };
        for item in items {
            let Some(item_id) = item.get("id").and_then(Value::as_str).map(str::to_string) else {
                continue;
            };
            let Some(translation) = translations.get(&item_id).and_then(Value::as_object) else {
                continue;
            };
            let Some(translated_text) = translation.get("text").and_then(Value::as_str) else {
                continue;
            };
            let Some(item_object) = item.as_object_mut() else {
                continue;
            };
            let current_text = item_object
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if !current_text.is_empty() && current_text != translated_text {
                item_object
                    .entry("originalText".to_string())
                    .or_insert_with(|| Value::String(current_text));
            }
            item_object.insert(
                "text".to_string(),
                Value::String(translated_text.to_string()),
            );
            item_object.insert("languageBridgeTranslated".to_string(), Value::Bool(true));
            if let Some(language) = translation.get("language").cloned() {
                item_object.insert("languageBridgeOutputLanguage".to_string(), language);
            }
        }
    }
    Ok(())
}

pub(crate) async fn spawn_language_bridge_response_translation_for_completed_turn(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    turn: &Value,
) {
    let turn_id = turn
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if turn_id.is_empty() {
        return;
    }
    let Some((item_id, answer_text)) = extract_agent_message_item_from_turn(turn) else {
        return;
    };
    let Ok((preferences, output_language, already_translated)) =
        with_ui_state_read(state, profile_id, |ui_state| {
            let preferences = ui_state
                .get("preferencesByThreadId")
                .and_then(Value::as_object)
                .and_then(|entries| entries.get(session_id))
                .cloned()
                .unwrap_or(Value::Null);
            let bridge_state = ui_state
                .get("languageBridgeByThreadId")
                .and_then(Value::as_object)
                .and_then(|entries| entries.get(session_id));
            let output_language = bridge_state
                .and_then(|entry| entry.get("outputLanguage"))
                .and_then(Value::as_str)
                .map(normalize_language_bridge_output_language)
                .unwrap_or_else(|| language_bridge_output_language(&preferences));
            let already_translated = bridge_state
                .and_then(|entry| entry.get("translationsByItemId"))
                .and_then(Value::as_object)
                .and_then(|translations| translations.get(&item_id))
                .and_then(Value::as_object)
                .is_some_and(|translation| {
                    translation
                        .get("originalText")
                        .and_then(Value::as_str)
                        .is_some_and(|value| value == answer_text)
                        && translation
                            .get("language")
                            .and_then(Value::as_str)
                            .is_some_and(|value| value.eq_ignore_ascii_case(&output_language))
                });
            Ok((preferences, output_language, already_translated))
        })
        .await
    else {
        return;
    };
    if !language_bridge_enabled(&preferences)
        || already_translated
        || output_language.eq_ignore_ascii_case("auto")
        || output_language.eq_ignore_ascii_case("English")
    {
        return;
    }

    let state = state.clone();
    let profile_id = profile_id.to_string();
    let session_id = session_id.to_string();
    tokio::spawn(async move {
        let cwd = preferences
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let client = match app_server_client_for_session(&state, &profile_id, &session_id).await {
            Ok(client) => client,
            Err(error) => {
                warn!(
                    profile_id,
                    session_id,
                    error = %error,
                    "failed to start language bridge response translation worker"
                );
                return;
            }
        };
        let start_response = match client
            .request(
                "thread/start",
                json!({
                    "cwd": cwd,
                    "model": preferences.get("model").cloned().unwrap_or(Value::Null),
                    "permissions": CODEX_PERMISSION_PROFILE_READ_ONLY,
                    "developerInstructions": "You are a private response translation worker for Codex Web UI. Translate only the final answer. Do not call tools and do not add commentary.",
                    "ephemeral": true,
                    "personality": "none"
                }),
            )
            .await
        {
            Ok(response) => response,
            Err(error) => {
                warn!(
                    profile_id,
                    session_id,
                    error = %error,
                    "failed to create language bridge response translation worker"
                );
                return;
            }
        };
        let Some(temp_thread_id) = start_response
            .get("thread")
            .and_then(|thread| thread.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            warn!(
                profile_id,
                session_id, "language bridge response translation worker returned no thread id"
            );
            return;
        };
        let prompt = format!(
            "Translate the following Codex answer into {output_language}.\n\
             Return only the translated answer. Preserve code blocks, commands, file paths, identifiers, logs, errors, JSON, Markdown table formatting, and quoted text exactly unless explicitly translated in the source answer.\n\n\
             <codex_answer>\n{answer_text}\n</codex_answer>"
        );
        let (input, _) = build_turn_input_payload(&prompt, &[], &[]);
        if let Err(error) = client
            .request(
                "turn/start",
                json!({
                    "threadId": temp_thread_id,
                    "input": input,
                    "cwd": cwd,
                    "model": preferences.get("model").cloned().unwrap_or(Value::Null),
                    "approvalPolicy": "never",
                    "permissions": CODEX_PERMISSION_PROFILE_READ_ONLY,
                    "runtimeWorkspaceRoots": [cwd]
                }),
            )
            .await
        {
            warn!(
                profile_id,
                session_id,
                error = %error,
                "failed to start language bridge response translation turn"
            );
            return;
        }

        let deadline = tokio::time::Instant::now() + LANGUAGE_BRIDGE_TRANSLATION_TIMEOUT;
        let translated_text = loop {
            let response = match client
                .request(
                    "thread/read",
                    json!({
                        "threadId": temp_thread_id,
                        "includeTurns": true
                    }),
                )
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    warn!(
                        profile_id,
                        session_id,
                        error = %error,
                        "failed to read language bridge response translation"
                    );
                    return;
                }
            };
            if let Some(raw_text) = response.get("thread").and_then(extract_agent_message_text) {
                let stripped = strip_json_code_fence(&raw_text).trim().to_string();
                if !stripped.is_empty() {
                    break stripped;
                }
            }
            if tokio::time::Instant::now() >= deadline {
                warn!(
                    profile_id,
                    session_id, "timed out while translating language bridge response"
                );
                return;
            }
            tokio::time::sleep(LANGUAGE_BRIDGE_TRANSLATION_POLL_INTERVAL).await;
        };
        let translated_at = now_unix_ms();
        if let Err(error) = with_ui_state_write(&state, &profile_id, |ui_state| {
            let thread_state = ensure_language_bridge_thread_state(ui_state, &session_id)?;
            let translations = thread_state
                .entry("translationsByItemId".to_string())
                .or_insert_with(|| json!({}));
            if !translations.is_object() {
                *translations = json!({});
            }
            let translations = translations.as_object_mut().ok_or_else(|| {
                api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "language bridge translations state is missing",
                )
            })?;
            translations.insert(
                item_id.clone(),
                json!({
                    "turnId": turn_id.clone(),
                    "itemId": item_id.clone(),
                    "text": translated_text.clone(),
                    "originalText": answer_text.clone(),
                    "language": output_language.clone(),
                    "updatedAt": translated_at
                }),
            );
            thread_state.insert(
                "outputLanguage".to_string(),
                Value::String(output_language.clone()),
            );
            thread_state.insert("updatedAt".to_string(), json!(translated_at));
            Ok(())
        })
        .await
        {
            warn!(
                profile_id,
                session_id,
                error = %error,
                "failed to persist language bridge response translation"
            );
            return;
        }
        emit_session_notification(
            &state,
            &profile_id,
            &session_id,
            json!({
                "kind": "notification",
                "method": "codex-webui/languageBridgeResponseTranslated",
                "params": {
                    "sessionId": session_id,
                    "turnId": turn_id,
                    "itemId": item_id,
                    "text": translated_text,
                    "originalText": answer_text,
                    "language": output_language,
                    "translatedAt": translated_at
                }
            }),
        )
        .await;
    });
}

async fn turn_app_server_error(
    state: &AppState,
    profile_id: &str,
    client: &AppServerClient,
    context: &str,
    error: anyhow::Error,
) -> ApiError {
    let raw_message = error.to_string();
    if !usage_limit_error_message(&raw_message) {
        return api_error(StatusCode::BAD_GATEWAY, format!("{context}: {raw_message}"));
    }

    let mut retry_at_ms = structured_error_value(&raw_message)
        .as_ref()
        .and_then(retry_at_ms_from_value);
    if retry_at_ms.is_none() {
        retry_at_ms = client
            .request("account/rateLimits/read", json!({}))
            .await
            .ok()
            .as_ref()
            .and_then(retry_at_ms_from_value);
    }
    if retry_at_ms.is_none() {
        retry_at_ms = codex_quota_status(state, true, profile_id)
            .await
            .ok()
            .as_ref()
            .and_then(retry_at_ms_from_value);
    }

    api_error(
        StatusCode::TOO_MANY_REQUESTS,
        usage_limit_error_payload(&raw_message, retry_at_ms),
    )
}

async fn resume_thread_before_app_server_turn(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    client: &AppServerClient,
    context: &str,
) -> ApiResult<()> {
    let mut attempts = 0usize;
    loop {
        attempts += 1;
        #[cfg(not(test))]
        let resume_timeout = client.request_timeout();
        #[cfg(test)]
        let resume_timeout = Duration::from_millis(100);
        let result = client
            .request_with_timeout(
                "thread/resume",
                json!({
                    "threadId": session_id,
                    "excludeTurns": true
                }),
                resume_timeout,
                true,
            )
            .await;

        match result {
            Ok(_) => return Ok(()),
            Err(error) => {
                let message = format!("{context}: {error}");
                if message.contains("is closing; retry thread/resume") && attempts < 4 {
                    tokio::time::sleep(Duration::from_millis(250 * attempts as u64)).await;
                    continue;
                }
                if let Some(recovery_error) =
                    session_rollout_recovery_required_error(state, profile_id, session_id, &message)
                        .await
                {
                    return Err(recovery_error);
                }
                if is_unmaterialized_thread_error_message(&message) {
                    return Ok(());
                }
                if app_server_request_timed_out(&error)
                    || app_server_request_interrupted(&error)
                    || message
                        .to_ascii_lowercase()
                        .contains("codex app-server request timed out")
                {
                    warn!(
                        profile_id,
                        session_id,
                        error = %error,
                        "thread resume did not complete; falling back to direct turn start"
                    );
                    return Ok(());
                }
                return Err(turn_app_server_error(state, profile_id, client, context, error).await);
            }
        }
    }
}

async fn session_rollout_recovery_required_error(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    message: &str,
) -> Option<ApiError> {
    if !is_rollout_history_corruption_error(message) {
        return None;
    }

    let recovery = match inspect_session_rollout_recovery_payload(
        state,
        profile_id,
        session_id,
        Some(message),
    )
    .await
    {
        Some(recovery) => recovery,
        None => {
            let mut recovered = None;
            for token in message.split_whitespace() {
                let trimmed = token.trim().trim_matches(['`', '\'', '"', ':', ',', ';']);
                if !trimmed.contains(".jsonl") {
                    continue;
                }
                let Some(jsonl_end) = trimmed.find(".jsonl").map(|index| index + ".jsonl".len())
                else {
                    continue;
                };
                let path = normalize_path(PathBuf::from(&trimmed[..jsonl_end]));
                if path.extension().and_then(|extension| extension.to_str()) != Some("jsonl")
                    || path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_none_or(|name| !name.ends_with(&format!("{session_id}.jsonl")))
                {
                    continue;
                }
                let Ok(buffer) = tokio_fs::read(path).await else {
                    continue;
                };
                let plan = inspect_rollout_recovery_content(&buffer);
                if plan.info.available
                    && plan.info.recoverable_lines > 0
                    && !plan.recovered_content.trim().is_empty()
                {
                    recovered = Some(json!(plan.info));
                    break;
                }
            }
            recovered?
        }
    };
    Some(api_error(
        StatusCode::CONFLICT,
        json!({
            "code": "SESSION_ROLLOUT_RECOVERY_REQUIRED",
            "message": message,
            "status": StatusCode::CONFLICT.as_u16(),
            "sessionId": session_id,
            "recovery": recovery
        })
        .to_string(),
    ))
}

pub(crate) async fn resolve_active_turn_id_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<Option<String>> {
    resolve_active_turn_id_payload_with_hint(state, profile_id, session_id, None).await
}

async fn resolve_active_turn_id_payload_with_hint(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    expected_turn_id: Option<&str>,
) -> ApiResult<Option<String>> {
    if let Some(turn_id) = expected_turn_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(Some(turn_id.to_string()));
    }

    let runtime_key = runtime_session_key(
        &resolve_runtime_profile_entry(&state.config, profile_id).0,
        session_id,
    );
    if let Some(turn_id) = state.active_turns.lock().await.get(&runtime_key).cloned() {
        return Ok(Some(turn_id));
    }

    let thread = read_thread_payload(state, profile_id, session_id, true).await?;
    let active_turn_id = thread
        .get("turns")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .and_then(active_turn_id_from_turns);
    if let Some(turn_id) = active_turn_id.clone() {
        state.active_turns.lock().await.insert(runtime_key, turn_id);
    }
    Ok(active_turn_id)
}

pub(crate) async fn send_turn_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    prompt: &str,
    attachment_ids: Option<&Value>,
    selected_skills: Option<&Value>,
    preferences: Value,
    client_user_message_id: Option<&str>,
) -> ApiResult<Value> {
    let trimmed_prompt = prompt.trim();
    let client_user_message_id = normalize_client_user_message_id(client_user_message_id);
    let runtime_key = runtime_session_key(
        &resolve_runtime_profile_entry(&state.config, profile_id).0,
        session_id,
    );
    if let Some(client_id) = client_user_message_id.as_deref() {
        if recently_sent_client_user_message(state, &runtime_key, client_id).await {
            return Ok(json!({
                "ok": true,
                "duplicate": true,
                "clientUserMessageId": client_id
            }));
        }
    }
    clear_stale_session_runtime_activity_if_app_server_missing(
        state,
        profile_id,
        session_id,
        DUPLICATE_TURN_START_ACTIVE_GRACE_MS,
        "codex app-server is not running",
    )
    .await;
    {
        let mut pending_turn_starts = state.pending_turn_starts.lock().await;
        if !pending_turn_starts.insert(runtime_key.clone()) {
            return Err(api_error(
                StatusCode::CONFLICT,
                json!({
                    "code": "TURN_ALREADY_STARTING",
                    "message": "A response is already starting for this session."
                })
                .to_string(),
            ));
        }
    }

    let result = async {
        let attachments =
            resolve_selected_attachment_records(state, profile_id, session_id, attachment_ids)
                .await?;
        let requested_selected_skills = selected_skills_from_value(selected_skills);

        if trimmed_prompt.is_empty() && attachments.is_empty() {
            return Err(api_error(StatusCode::BAD_REQUEST, "EMPTY_MESSAGE"));
        }

        if state.active_turns.lock().await.contains_key(&runtime_key) {
            let thread = read_thread_payload(state, profile_id, session_id, true).await?;
            let active_turn_id = thread
                .get("turns")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .and_then(active_turn_id_from_turns);
            if let Some(active_turn_id) = active_turn_id {
                state
                    .active_turns
                    .lock()
                    .await
                    .insert(runtime_key.clone(), active_turn_id.clone());
                return Err(api_error(
                    StatusCode::CONFLICT,
                    json!({
                        "code": "TURN_ALREADY_RUNNING",
                        "message": "A response is already running for this session.",
                        "turnId": active_turn_id
                    })
                    .to_string(),
                ));
            }
            state.active_turns.lock().await.remove(&runtime_key);
        }

        set_runtime_session_status(state, profile_id, session_id, "starting").await;
        emit_session_status_summary_updated(state, profile_id, session_id, None, "starting").await;
        emit_session_notification(
            state,
            profile_id,
            session_id,
            json!({
                "kind": "notification",
                "method": "thread/status/changed",
                "params": {
                    "threadId": session_id,
                    "status": "starting"
                }
            }),
        )
        .await;

        cancel_scheduled_shutdown_for_activity(state, profile_id).await;

    let next_preferences =
        normalize_session_preferences_payload(state, profile_id, preferences).await?;
    let cwd = next_preferences
        .get("cwd")
        .and_then(Value::as_str)
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "A working directory is required."))?
        .to_string();
    let next_selected_skills = if requested_selected_skills.is_empty() {
        with_ui_state_read(state, profile_id, |ui_state| {
            Ok(session_selected_skills_from_ui_state(ui_state, session_id))
        })
        .await?
    } else {
        requested_selected_skills
    };

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

    let client = app_server_client_for_session_turn(state, profile_id, session_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to codex app-server: {error}"),
            )
        })?;
    resume_thread_before_app_server_turn(
        state,
        profile_id,
        session_id,
        &client,
        "Failed to resume the session before sending",
    )
    .await?;
    let mut effective_prompt = trimmed_prompt.to_string();
    let mut resolved_output_language = None;
    if language_bridge_enabled(&next_preferences) && !trimmed_prompt.is_empty() {
        let (translated_prompt, output_language) = translate_prompt_with_language_bridge(
            &client,
            &next_preferences,
            next_preferences
                .get("cwd")
                .and_then(Value::as_str)
                .unwrap_or(&cwd),
            trimmed_prompt,
        )
        .await?;
        effective_prompt = translated_prompt;
        resolved_output_language = Some(output_language);
    }

    let (input, attachment_readable_roots) =
        build_turn_input_payload(&effective_prompt, &attachments, &next_selected_skills);
    let mut readable_roots = vec![cwd.clone()];
    for readable_root in attachment_readable_roots {
        if !readable_roots.contains(&readable_root) {
            readable_roots.push(readable_root);
        }
    }

    let sandbox_mode = next_preferences
        .get("sandboxMode")
        .and_then(Value::as_str)
        .unwrap_or("workspace-write");
    let network_access = next_preferences
        .get("networkAccess")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let runtime_workspace_roots = if sandbox_mode == "read-only" {
        readable_roots.clone()
    } else {
        vec![cwd.clone()]
    };
    let permission_profile = permission_profile_for_sandbox_mode(sandbox_mode);
    let sandbox_policy = if network_access {
        match sandbox_mode {
            "danger-full-access" => json!({
                "type": "dangerFullAccess"
            }),
            "read-only" => json!({
                "type": "readOnly",
                "networkAccess": true
            }),
            _ => json!({
                "type": "workspaceWrite",
                "writableRoots": [cwd],
                "networkAccess": true,
                "excludeTmpdirEnvVar": false,
                "excludeSlashTmp": false
            }),
        }
    } else {
        Value::Null
    };
    let model = next_preferences
        .get("model")
        .cloned()
        .unwrap_or(Value::Null);
    let collaboration_mode =
        collaboration_mode_payload(&next_preferences, &model, resolved_output_language.as_deref());
    let client_user_message_id_value = client_user_message_id
        .as_ref()
        .map(|value| Value::String(value.clone()))
        .unwrap_or(Value::Null);
    let responsesapi_client_metadata =
        responsesapi_client_metadata_for_user_message(client_user_message_id.as_deref());
    let response = match client
        .request(
            "turn/start",
            json!({
                "threadId": session_id,
                "clientUserMessageId": client_user_message_id_value,
                "responsesapiClientMetadata": responsesapi_client_metadata,
                "input": input,
                "cwd": next_preferences.get("cwd").cloned().unwrap_or(Value::Null),
                "approvalPolicy": next_preferences.get("approvalPolicy").cloned().unwrap_or_else(|| json!("on-request")),
                "sandboxPolicy": sandbox_policy,
                "permissions": if network_access { Value::Null } else { Value::String(permission_profile.to_string()) },
                "runtimeWorkspaceRoots": runtime_workspace_roots,
                "model": model.clone(),
                "personality": next_preferences.get("personality").cloned().unwrap_or(Value::Null),
                "serviceTier": match next_preferences.get("speed").and_then(Value::as_str) {
                    Some("fast") => Value::String("fast".to_string()),
                    Some("flex") => Value::String("flex".to_string()),
                    _ => Value::Null
                },
                "effort": if next_preferences.get("mode").and_then(Value::as_str) == Some("plan") {
                    Value::Null
                } else {
                    next_preferences.get("effort").cloned().unwrap_or(Value::Null)
                },
                "collaborationMode": collaboration_mode
            }),
        )
        .await
    {
        Ok(response) => response,
        Err(error) => {
            let message = format!("Failed to start the turn: {error}");
            if let Some(recovery_error) =
                session_rollout_recovery_required_error(state, profile_id, session_id, &message)
                    .await
            {
                return Err(recovery_error);
            }
            return Err(
                turn_app_server_error(state, profile_id, &client, "Failed to start the turn", error)
                    .await,
            );
        }
    };

    if let Some(turn_id) = response
        .get("turn")
        .and_then(|value| value.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
    {
        state
            .active_turns
            .lock()
            .await
            .insert(runtime_key.clone(), turn_id);
    }
    if let Some(client_id) = client_user_message_id.as_deref() {
        remember_sent_client_user_message(state, &runtime_key, client_id).await;
    }
    if let Some(output_language) = resolved_output_language.as_deref() {
        remember_language_bridge_output_language(state, profile_id, session_id, output_language)
            .await?;
    }
    set_runtime_session_status(state, profile_id, session_id, "running").await;
    set_session_highlight(state, profile_id, session_id, None).await;
    emit_session_notification(
        state,
        profile_id,
        session_id,
        json!({
            "kind": "notification",
            "method": "thread/status/changed",
            "params": {
                "threadId": session_id,
                "status": "running"
            }
        }),
    )
    .await;
    state.pending_turn_starts.lock().await.remove(&runtime_key);

    clear_session_draft_payload(state, profile_id, session_id).await?;
    emit_session_status_summary_updated(
        state,
        profile_id,
        session_id,
        Some(next_preferences.clone()),
        "running",
    )
    .await;

    Ok(json!({
        "ok": true,
        "turnId": response
            .get("turn")
            .and_then(|value| value.get("id"))
            .cloned()
            .unwrap_or(Value::Null)
    }))
    }
    .await;

    if result.is_err() {
        let was_pending = state.pending_turn_starts.lock().await.remove(&runtime_key);
        if was_pending {
            let was_starting = with_ui_state_read(state, profile_id, |ui_state| {
                Ok(ui_state
                    .get("runtimeStatusByThreadId")
                    .and_then(Value::as_object)
                    .and_then(|entries| entries.get(session_id))
                    .and_then(|status| normalized_thread_status(Some(status)))
                    .as_deref()
                    == Some("starting"))
            })
            .await
            .unwrap_or(false);
            if was_starting {
                set_runtime_session_status(state, profile_id, session_id, "failed").await;
                emit_session_notification(
                    state,
                    profile_id,
                    session_id,
                    json!({
                        "kind": "notification",
                        "method": "thread/status/changed",
                        "params": {
                            "threadId": session_id,
                            "status": "failed"
                        }
                    }),
                )
                .await;
                emit_session_summary_updated(state, profile_id, session_id, None, Some("failed"))
                    .await;
            } else {
                emit_session_summary_updated(state, profile_id, session_id, None, None).await;
            }
        }
    }
    result
}

pub(crate) async fn start_session_compaction_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
) -> ApiResult<Value> {
    let runtime_key = runtime_session_key(
        &resolve_runtime_profile_entry(&state.config, profile_id).0,
        session_id,
    );
    clear_stale_session_runtime_activity_if_app_server_missing(
        state,
        profile_id,
        session_id,
        DUPLICATE_TURN_START_ACTIVE_GRACE_MS,
        "codex app-server is not running",
    )
    .await;
    {
        let mut pending_turn_starts = state.pending_turn_starts.lock().await;
        if !pending_turn_starts.insert(runtime_key.clone()) {
            return Err(api_error(
                StatusCode::CONFLICT,
                json!({
                    "code": "TURN_ALREADY_STARTING",
                    "message": "A response is already starting for this session."
                })
                .to_string(),
            ));
        }
    }

    let result = async {
        if state.active_turns.lock().await.contains_key(&runtime_key) {
            let thread = read_thread_payload(state, profile_id, session_id, true).await?;
            let active_turn_id = thread
                .get("turns")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .and_then(active_turn_id_from_turns);
            if let Some(active_turn_id) = active_turn_id {
                state
                    .active_turns
                    .lock()
                    .await
                    .insert(runtime_key.clone(), active_turn_id.clone());
                return Err(api_error(
                    StatusCode::CONFLICT,
                    json!({
                        "code": "TURN_ALREADY_RUNNING",
                        "message": "A response is already running for this session.",
                        "turnId": active_turn_id
                    })
                    .to_string(),
                ));
            }
            state.active_turns.lock().await.remove(&runtime_key);
        }

        set_runtime_session_status(state, profile_id, session_id, "starting").await;
        emit_session_summary_updated(state, profile_id, session_id, None, Some("starting")).await;
        emit_session_notification(
            state,
            profile_id,
            session_id,
            json!({
                "kind": "notification",
                "method": "thread/status/changed",
                "params": {
                    "threadId": session_id,
                    "status": "starting"
                }
            }),
        )
        .await;

        cancel_scheduled_shutdown_for_activity(state, profile_id).await;

        let client = app_server_client_for_session_turn(state, profile_id, session_id)
            .await
            .map_err(|error| {
                api_error(
                    StatusCode::BAD_GATEWAY,
                    format!("Failed to connect to codex app-server: {error}"),
                )
            })?;
        resume_thread_before_app_server_turn(
            state,
            profile_id,
            session_id,
            &client,
            "Failed to resume the session before compacting",
        )
        .await?;
        let response = match client
            .request("thread/compact/start", json!({ "threadId": session_id }))
            .await
        {
            Ok(response) => response,
            Err(error) => {
                return Err(turn_app_server_error(
                    state,
                    profile_id,
                    &client,
                    "Failed to start context compression",
                    error,
                )
                .await);
            }
        };

        let mut turn_id = response
            .get("turn")
            .and_then(|value| value.get("id"))
            .and_then(Value::as_str)
            .or_else(|| response.get("turnId").and_then(Value::as_str))
            .map(str::to_string);
        let mut observed_thread_status = None;
        if turn_id.is_none() {
            if let Ok(thread) = read_thread_payload(state, profile_id, session_id, true).await {
                turn_id = thread
                    .get("turns")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .and_then(active_turn_id_from_turns);
                observed_thread_status = normalized_thread_status(thread.get("status"));
            }
        }
        if let Some(turn_id) = turn_id.as_deref() {
            state
                .active_turns
                .lock()
                .await
                .insert(runtime_key.clone(), turn_id.to_string());
        }
        state.pending_turn_starts.lock().await.remove(&runtime_key);
        let next_status = if turn_id.is_some() {
            "running"
        } else {
            observed_thread_status
                .as_deref()
                .map(runtime_status_from_codex_thread_status)
                .unwrap_or("completed")
        };
        set_runtime_session_status(state, profile_id, session_id, next_status).await;
        set_session_highlight(state, profile_id, session_id, None).await;
        emit_session_notification(
            state,
            profile_id,
            session_id,
            json!({
                "kind": "notification",
                "method": "thread/status/changed",
                "params": {
                    "threadId": session_id,
                    "status": next_status
                }
            }),
        )
        .await;
        emit_session_summary_updated(state, profile_id, session_id, None, Some(next_status)).await;

        Ok(json!({
            "ok": true,
            "turnId": turn_id
        }))
    }
    .await;

    if result.is_err() {
        let was_pending = state.pending_turn_starts.lock().await.remove(&runtime_key);
        if was_pending {
            let was_starting = with_ui_state_read(state, profile_id, |ui_state| {
                Ok(ui_state
                    .get("runtimeStatusByThreadId")
                    .and_then(Value::as_object)
                    .and_then(|entries| entries.get(session_id))
                    .and_then(|status| normalized_thread_status(Some(status)))
                    .as_deref()
                    == Some("starting"))
            })
            .await
            .unwrap_or(false);
            if was_starting {
                set_runtime_session_status(state, profile_id, session_id, "failed").await;
                emit_session_notification(
                    state,
                    profile_id,
                    session_id,
                    json!({
                        "kind": "notification",
                        "method": "thread/status/changed",
                        "params": {
                            "threadId": session_id,
                            "status": "failed"
                        }
                    }),
                )
                .await;
                emit_session_summary_updated(state, profile_id, session_id, None, Some("failed"))
                    .await;
            } else {
                emit_session_summary_updated(state, profile_id, session_id, None, None).await;
            }
        }
    }
    result
}

pub(crate) async fn steer_turn_payload(
    state: &AppState,
    profile_id: &str,
    session_id: &str,
    prompt: &str,
    attachment_ids: Option<&Value>,
    selected_skills: Option<&Value>,
    expected_turn_id: Option<&str>,
    client_user_message_id: Option<&str>,
) -> ApiResult<Value> {
    let trimmed_prompt = prompt.trim();
    let client_user_message_id = normalize_client_user_message_id(client_user_message_id);
    if trimmed_prompt.is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "EMPTY_MESSAGE"));
    }

    let active_turn_id =
        resolve_active_turn_id_payload_with_hint(state, profile_id, session_id, expected_turn_id)
            .await?;
    let Some(active_turn_id) = active_turn_id else {
        return Err(api_error(StatusCode::CONFLICT, "NO_ACTIVE_TURN"));
    };

    let attachments =
        resolve_selected_attachment_records(state, profile_id, session_id, attachment_ids).await?;
    let requested_selected_skills = selected_skills_from_value(selected_skills);
    let next_selected_skills = if requested_selected_skills.is_empty() {
        with_ui_state_read(state, profile_id, |ui_state| {
            Ok(session_selected_skills_from_ui_state(ui_state, session_id))
        })
        .await?
    } else {
        requested_selected_skills
    };
    let client = app_server_client_for_session(state, profile_id, session_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to codex app-server: {error}"),
            )
        })?;
    resume_thread_before_app_server_turn(
        state,
        profile_id,
        session_id,
        &client,
        "Failed to resume the session before steering",
    )
    .await?;
    let preferences = with_ui_state_read(state, profile_id, |ui_state| {
        Ok(ui_state
            .get("preferencesByThreadId")
            .and_then(Value::as_object)
            .and_then(|entries| entries.get(session_id))
            .cloned()
            .unwrap_or(Value::Null))
    })
    .await?;
    let mut effective_prompt = trimmed_prompt.to_string();
    let mut resolved_output_language = None;
    if language_bridge_enabled(&preferences) {
        let cwd = preferences
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let (translated_prompt, output_language) =
            translate_prompt_with_language_bridge(&client, &preferences, cwd, trimmed_prompt)
                .await?;
        effective_prompt = translated_prompt;
        resolved_output_language = Some(output_language);
    }
    let (input, _) =
        build_turn_input_payload(&effective_prompt, &attachments, &next_selected_skills);
    let client_user_message_id_value = client_user_message_id
        .as_ref()
        .map(|value| Value::String(value.clone()))
        .unwrap_or(Value::Null);
    let responsesapi_client_metadata =
        responsesapi_client_metadata_for_user_message(client_user_message_id.as_deref());
    if let Err(error) = client
        .request(
            "turn/steer",
            json!({
                "threadId": session_id,
                "expectedTurnId": active_turn_id,
                "clientUserMessageId": client_user_message_id_value,
                "responsesapiClientMetadata": responsesapi_client_metadata,
                "input": input
            }),
        )
        .await
    {
        return Err(turn_app_server_error(
            state,
            profile_id,
            &client,
            "Failed to steer the active turn",
            error,
        )
        .await);
    }

    if let Some(output_language) = resolved_output_language.as_deref() {
        remember_language_bridge_output_language(state, profile_id, session_id, output_language)
            .await?;
    }
    let runtime_key = runtime_session_key(
        &resolve_runtime_profile_entry(&state.config, profile_id).0,
        session_id,
    );
    state
        .active_turns
        .lock()
        .await
        .insert(runtime_key, active_turn_id.clone());
    clear_session_draft_payload(state, profile_id, session_id).await?;
    Ok(json!({
        "ok": true,
        "turnId": active_turn_id
    }))
}
