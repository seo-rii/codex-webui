use super::*;

fn ui_state_notification_items(ui_state: &Value) -> Vec<Value> {
    ui_state
        .get("notifications")
        .and_then(|value| value.get("items"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn ui_state_webhook_failures(ui_state: &Value) -> Vec<Value> {
    ui_state
        .get("notifications")
        .and_then(|value| value.get("webhookFailures"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

pub(crate) fn unread_notification_count(items: &[Value]) -> usize {
    items
        .iter()
        .filter(|entry| entry.get("readAt").is_none_or(Value::is_null))
        .count()
}

fn notifications_payload_from_items(mut items: Vec<Value>, limit: usize) -> Value {
    items.sort_by(|left, right| {
        let left_created = left.get("createdAt").and_then(Value::as_i64).unwrap_or(0);
        let right_created = right.get("createdAt").and_then(Value::as_i64).unwrap_or(0);
        right_created.cmp(&left_created)
    });
    let unread_count = unread_notification_count(&items);
    let limited = items.into_iter().take(limit.max(1)).collect::<Vec<_>>();
    json!({
        "notifications": limited,
        "unreadCount": unread_count
    })
}

fn redacted_notification_item(item: Value) -> Value {
    json!({
        "id": item.get("id").cloned().unwrap_or(Value::Null),
        "type": item.get("type").cloned().unwrap_or(Value::Null),
        "createdAt": item.get("createdAt").cloned().unwrap_or(Value::Null),
        "readAt": item.get("readAt").cloned().unwrap_or(Value::Null),
        "sessionId": item.get("sessionId").cloned().unwrap_or(Value::Null)
    })
}

fn redact_notifications_payload_for_viewer(mut payload: Value) -> Value {
    if let Some(items) = payload
        .get_mut("notifications")
        .and_then(Value::as_array_mut)
    {
        *items = std::mem::take(items)
            .into_iter()
            .map(redacted_notification_item)
            .collect();
    }
    if let Some(object) = payload.as_object_mut() {
        object.remove("webhookFailures");
    }
    payload
}

pub(crate) async fn get_notifications_payload(
    state: &AppState,
    profile_id: &str,
    limit: usize,
) -> ApiResult<Value> {
    with_ui_state_read(state, profile_id, |ui_state| {
        let mut payload =
            notifications_payload_from_items(ui_state_notification_items(ui_state), limit);
        payload["webhookFailures"] = Value::Array(
            ui_state_webhook_failures(ui_state)
                .into_iter()
                .take(40)
                .collect(),
        );
        Ok(payload)
    })
    .await
}

pub(crate) async fn get_notifications_payload_for_role(
    state: &AppState,
    profile_id: &str,
    limit: usize,
    role: UserRole,
) -> ApiResult<Value> {
    let payload = get_notifications_payload(state, profile_id, limit).await?;
    if role_has_admin_access(role) {
        Ok(payload)
    } else {
        Ok(redact_notifications_payload_for_viewer(payload))
    }
}

pub(crate) async fn record_notification_webhook_failure(
    state: &AppState,
    profile_id: &str,
    notification: &Value,
    field: &str,
    error: &str,
) {
    let entry = json!({
        "id": Uuid::new_v4().to_string(),
        "createdAt": now_unix_ms(),
        "notificationId": notification.get("id").cloned().unwrap_or(Value::Null),
        "eventType": notification.get("type").cloned().unwrap_or(Value::Null),
        "field": field,
        "error": redact_user_facing_error(error),
    });
    let payload = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(failures) = ui_state
            .get_mut("notifications")
            .and_then(Value::as_object_mut)
            .and_then(|notifications| notifications.get_mut("webhookFailures"))
            .and_then(Value::as_array_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "notification webhook failure state is missing",
            ));
        };
        failures.insert(0, entry.clone());
        if failures.len() > 40 {
            failures.truncate(40);
        }
        Ok(json!({
            "webhookFailures": failures.clone()
        }))
    })
    .await;

    if let Ok(payload) = payload {
        emit_profile_config_updated(
            state,
            profile_id,
            json!({
                "notifications": payload
            }),
        )
        .await;
    }
}

pub(crate) async fn mark_notifications_read_payload(
    state: &AppState,
    profile_id: &str,
    ids: Option<Vec<String>>,
) -> ApiResult<Value> {
    let target_ids = ids.map(|items| {
        items
            .into_iter()
            .filter_map(|item| {
                let trimmed = item.trim().to_string();
                (!trimmed.is_empty()).then_some(trimmed)
            })
            .collect::<Vec<_>>()
    });

    let (payload, changed) = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(items) = ui_state
            .get_mut("notifications")
            .and_then(Value::as_object_mut)
            .and_then(|value| value.get_mut("items"))
            .and_then(Value::as_array_mut)
        else {
            return Ok((json!({ "notifications": [], "unreadCount": 0 }), false));
        };

        let targets = target_ids.as_ref().map(|entries| {
            entries
                .iter()
                .cloned()
                .collect::<std::collections::HashSet<_>>()
        });
        let marked_at = now_unix_ms() as i64;
        let mut changed = false;

        for entry in items.iter_mut() {
            let read_at = entry.get("readAt");
            let entry_id = entry.get("id").and_then(Value::as_str);
            let should_mark = read_at.is_none_or(Value::is_null)
                && targets
                    .as_ref()
                    .is_none_or(|ids| entry_id.is_some_and(|candidate| ids.contains(candidate)));
            if should_mark {
                if let Some(object) = entry.as_object_mut() {
                    object.insert("readAt".to_string(), json!(marked_at));
                    changed = true;
                }
            }
        }

        Ok((
            notifications_payload_from_items(items.clone(), DEFAULT_NOTIFICATION_LIMIT),
            changed,
        ))
    })
    .await?;

    if changed {
        emit_profile_global_notification(
            state,
            profile_id,
            json!({
                "kind": "notification",
                "method": "codex-webui/notificationStateUpdated",
                "params": {
                    "unreadCount": payload.get("unreadCount").cloned().unwrap_or_else(|| json!(0))
                }
            }),
        )
        .await;
        emit_profile_config_updated(
            state,
            profile_id,
            json!({
                "notifications": {
                    "unreadCount": payload.get("unreadCount").cloned().unwrap_or_else(|| json!(0))
                }
            }),
        )
        .await;
    }

    Ok(payload)
}

pub(crate) async fn clear_notifications_payload(
    state: &AppState,
    profile_id: &str,
) -> ApiResult<Value> {
    let (payload, changed) = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(items) = ui_state
            .get_mut("notifications")
            .and_then(Value::as_object_mut)
            .and_then(|value| value.get_mut("items"))
            .and_then(Value::as_array_mut)
        else {
            return Ok((json!({ "notifications": [], "unreadCount": 0 }), false));
        };

        let changed = !items.is_empty();
        items.clear();
        Ok((
            notifications_payload_from_items(Vec::new(), DEFAULT_NOTIFICATION_LIMIT),
            changed,
        ))
    })
    .await?;

    if changed {
        emit_profile_global_notification(
            state,
            profile_id,
            json!({
                "kind": "notification",
                "method": "codex-webui/notificationStateUpdated",
                "params": {
                    "unreadCount": 0
                }
            }),
        )
        .await;
        emit_profile_config_updated(
            state,
            profile_id,
            json!({
                "notifications": {
                    "unreadCount": 0
                }
            }),
        )
        .await;
    }

    Ok(payload)
}

pub(crate) async fn update_notification_settings_payload(
    state: &AppState,
    profile_id: &str,
    patch: Value,
) -> ApiResult<Value> {
    validate_notification_webhook_url(
        &state.config,
        patch.get("slackWebhookUrl"),
        "slackWebhookUrl",
    )?;
    validate_notification_webhook_url(&state.config, patch.get("webhookUrl"), "webhookUrl")?;

    let payload = with_ui_state_write(state, profile_id, |ui_state| {
        let notifications = ui_state
            .get_mut("notifications")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "notifications state is missing",
                )
            })?;

        let current_settings = notifications.get("settings");
        let merged_settings = normalize_notification_settings_value(Some(&json!({
            "enabledEventTypes": patch.get("enabledEventTypes").cloned().unwrap_or_else(|| {
                current_settings
                    .and_then(|value| value.get("enabledEventTypes"))
                    .cloned()
                    .unwrap_or_else(|| default_notification_settings_value()["enabledEventTypes"].clone())
            }),
            "slackWebhookUrl": patch.get("slackWebhookUrl").cloned().unwrap_or_else(|| {
                current_settings
                    .and_then(|value| value.get("slackWebhookUrl"))
                    .cloned()
                    .unwrap_or(Value::Null)
            }),
            "webhookUrl": patch.get("webhookUrl").cloned().unwrap_or_else(|| {
                current_settings
                    .and_then(|value| value.get("webhookUrl"))
                    .cloned()
                    .unwrap_or(Value::Null)
            })
        })));

        notifications.insert("settings".to_string(), merged_settings.clone());
        let unread_count = notifications
            .get("items")
            .and_then(Value::as_array)
            .map(|items| unread_notification_count(items))
            .unwrap_or(0);

        Ok(json!({
            "settings": merged_settings,
            "unreadCount": unread_count
        }))
    })
    .await?;

    emit_profile_global_notification(
        state,
        profile_id,
        json!({
            "kind": "notification",
            "method": "codex-webui/notificationSettingsUpdated",
            "params": payload.clone()
        }),
    )
    .await;
    emit_profile_config_updated(
        state,
        profile_id,
        json!({
            "notifications": payload.clone()
        }),
    )
    .await;

    Ok(payload)
}

pub(crate) fn validate_notification_webhook_url_str(raw: &str, field: &str) -> ApiResult<()> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(());
    }
    let url = reqwest::Url::parse(raw).map_err(|_| {
        api_error(
            StatusCode::BAD_REQUEST,
            format!("{field} must be a valid URL."),
        )
    })?;
    if url.scheme() != "https" {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            format!("{field} must use https."),
        ));
    }
    let host = url.host_str().unwrap_or_default();
    let lowered_host = host.to_ascii_lowercase();
    if lowered_host == "localhost"
        || lowered_host.ends_with(".localhost")
        || lowered_host.ends_with(".local")
    {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            format!("{field} cannot target a local address."),
        ));
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if notification_webhook_ip_is_private_or_local(ip) {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                format!("{field} cannot target a private or local address."),
            ));
        }
    }
    Ok(())
}

fn validate_notification_webhook_url(
    config: &Config,
    candidate: Option<&Value>,
    field: &str,
) -> ApiResult<()> {
    let Some(raw) = candidate
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };
    validate_notification_webhook_url_str(raw, field)?;
    validate_notification_webhook_host_allowlist(config, raw, field)
}

fn notification_webhook_host_allowed(allowed_hosts: &[String], host: &str) -> bool {
    if allowed_hosts.is_empty() {
        return true;
    }
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    allowed_hosts.iter().any(|allowed| {
        if let Some(suffix) = allowed.strip_prefix("*.") {
            host.ends_with(&format!(".{suffix}"))
        } else {
            host == *allowed
        }
    })
}

pub(crate) fn validate_notification_webhook_host_allowlist(
    config: &Config,
    raw: &str,
    field: &str,
) -> ApiResult<()> {
    if config.webhook_allowed_hosts.is_empty() {
        return Ok(());
    }
    let url = reqwest::Url::parse(raw).map_err(|_| {
        api_error(
            StatusCode::BAD_REQUEST,
            format!("{field} must be a valid URL."),
        )
    })?;
    let Some(host) = url.host_str() else {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            format!("{field} must include a host."),
        ));
    };
    if notification_webhook_host_allowed(&config.webhook_allowed_hosts, host) {
        return Ok(());
    }
    Err(api_error(
        StatusCode::BAD_REQUEST,
        format!("{field} host is not allowed."),
    ))
}

pub(crate) fn notification_webhook_ip_is_private_or_local(ip: std::net::IpAddr) -> bool {
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || match ip {
            std::net::IpAddr::V4(value) => value.is_private() || value.is_link_local(),
            std::net::IpAddr::V6(value) => value.is_unique_local() || value.is_unicast_link_local(),
        }
}

pub(crate) async fn resolve_notification_webhook_public_addrs(
    config: &Config,
    raw: &str,
    field: &str,
) -> ApiResult<Option<(String, Vec<std::net::SocketAddr>)>> {
    validate_notification_webhook_url_str(raw, field)?;
    validate_notification_webhook_host_allowlist(config, raw, field)?;
    let url = reqwest::Url::parse(raw).map_err(|_| {
        api_error(
            StatusCode::BAD_REQUEST,
            format!("{field} must be a valid URL."),
        )
    })?;
    let Some(host) = url.host_str() else {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            format!("{field} must include a host."),
        ));
    };
    if host.parse::<std::net::IpAddr>().is_ok() {
        return Ok(None);
    }
    let port = url.port_or_known_default().unwrap_or(443);
    let resolved = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::net::lookup_host((host, port)),
    )
    .await
    .map_err(|_| {
        api_error(
            StatusCode::BAD_GATEWAY,
            format!("{field} DNS lookup timed out."),
        )
    })?
    .map_err(|_| {
        api_error(
            StatusCode::BAD_GATEWAY,
            format!("{field} host could not be resolved."),
        )
    })?;
    let mut saw_address = false;
    let mut public_addrs = Vec::new();
    for socket_addr in resolved {
        saw_address = true;
        if notification_webhook_ip_is_private_or_local(socket_addr.ip()) {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                format!("{field} resolves to a private or local address."),
            ));
        }
        public_addrs.push(socket_addr);
    }
    if !saw_address {
        return Err(api_error(
            StatusCode::BAD_GATEWAY,
            format!("{field} host could not be resolved."),
        ));
    }
    Ok(Some((host.to_string(), public_addrs)))
}
