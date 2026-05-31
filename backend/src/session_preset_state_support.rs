use super::*;

pub(crate) async fn save_session_filter_payload(
    state: &AppState,
    profile_id: &str,
    filter: Value,
) -> ApiResult<Value> {
    let name = filter
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "Filter name is required."))?;
    let filter_id = filter
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "filter.id is required."))?;

    let payload = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(saved_filters) = ui_state
            .get_mut("savedSessionFilters")
            .and_then(Value::as_array_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "saved filters state is missing",
            ));
        };

        let normalized_tags = filter
            .get("tags")
            .and_then(Value::as_array)
            .map(|tags| {
                let mut values = tags
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|tag| !tag.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                values.sort();
                values.dedup();
                values
            })
            .unwrap_or_default();

        let highlight = match filter.get("highlight").and_then(Value::as_str) {
            Some("attention") => "attention",
            Some("completed") => "completed",
            _ => "all",
        };

        let next_filter = json!({
            "id": filter_id,
            "name": name,
            "pinnedOnly": filter.get("pinnedOnly").and_then(Value::as_bool).unwrap_or(false),
            "runningOnly": filter.get("runningOnly").and_then(Value::as_bool).unwrap_or(false),
            "queuedOnly": filter.get("queuedOnly").and_then(Value::as_bool).unwrap_or(false),
            "untaggedOnly": filter.get("untaggedOnly").and_then(Value::as_bool).unwrap_or(false),
            "highlight": highlight,
            "tags": normalized_tags
        });

        let mut next_saved_filters = vec![next_filter];
        next_saved_filters.extend(
            saved_filters
                .iter()
                .filter(|entry| entry.get("id").and_then(Value::as_str) != Some(filter_id))
                .cloned(),
        );
        next_saved_filters.truncate(40);
        *saved_filters = next_saved_filters;

        Ok(json!({
            "savedFilters": saved_filters.clone(),
            "knownTags": known_tags_from_ui_state(ui_state)
        }))
    })
    .await?;

    emit_profile_config_updated(
        state,
        profile_id,
        json!({
            "sessionOrganization": {
                "savedFilters": payload.get("savedFilters").cloned().unwrap_or_else(|| json!([])),
                "knownTags": payload.get("knownTags").cloned().unwrap_or_else(|| json!([]))
            }
        }),
    )
    .await;

    Ok(payload)
}

pub(crate) async fn delete_session_filter_payload(
    state: &AppState,
    profile_id: &str,
    filter_id: &str,
) -> ApiResult<Value> {
    let trimmed_filter_id = filter_id.trim();
    let payload = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(saved_filters) = ui_state
            .get_mut("savedSessionFilters")
            .and_then(Value::as_array_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "saved filters state is missing",
            ));
        };

        *saved_filters = saved_filters
            .iter()
            .filter(|entry| entry.get("id").and_then(Value::as_str) != Some(trimmed_filter_id))
            .cloned()
            .collect::<Vec<_>>();

        Ok(json!({
            "savedFilters": saved_filters.clone(),
            "knownTags": known_tags_from_ui_state(ui_state)
        }))
    })
    .await?;

    emit_profile_config_updated(
        state,
        profile_id,
        json!({
            "sessionOrganization": {
                "savedFilters": payload.get("savedFilters").cloned().unwrap_or_else(|| json!([])),
                "knownTags": payload.get("knownTags").cloned().unwrap_or_else(|| json!([]))
            }
        }),
    )
    .await;

    Ok(payload)
}

pub(crate) async fn save_prompt_preset_payload(
    state: &AppState,
    profile_id: &str,
    preset: Value,
) -> ApiResult<Value> {
    let preset_id = preset
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "preset.id is required."))?;
    let preset_name = preset
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "Preset name is required."))?;
    let preset_prompt = preset
        .get("prompt")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "Preset prompt is required."))?;

    let payload = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(prompt_presets) = ui_state
            .get_mut("promptPresets")
            .and_then(Value::as_array_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "prompt presets state is missing",
            ));
        };

        let now = now_unix_ms() as i64;
        let created_at = prompt_presets
            .iter()
            .find(|entry| entry.get("id").and_then(Value::as_str) == Some(preset_id))
            .and_then(|entry| entry.get("createdAt").and_then(Value::as_i64))
            .or_else(|| preset.get("createdAt").and_then(Value::as_i64))
            .unwrap_or(now);

        let next_preset = json!({
            "id": preset_id,
            "name": preset_name,
            "prompt": preset_prompt,
            "createdAt": created_at,
            "updatedAt": now
        });

        let mut next_prompt_presets = vec![next_preset];
        next_prompt_presets.extend(
            prompt_presets
                .iter()
                .filter(|entry| entry.get("id").and_then(Value::as_str) != Some(preset_id))
                .cloned(),
        );
        next_prompt_presets.truncate(80);
        next_prompt_presets.sort_by(|left, right| {
            let left_updated = left.get("updatedAt").and_then(Value::as_i64).unwrap_or(0);
            let right_updated = right.get("updatedAt").and_then(Value::as_i64).unwrap_or(0);
            right_updated.cmp(&left_updated)
        });
        *prompt_presets = next_prompt_presets;

        Ok(json!({
            "promptPresets": prompt_presets.clone()
        }))
    })
    .await?;

    emit_profile_config_updated(
        state,
        profile_id,
        json!({
            "promptPresets": payload.get("promptPresets").cloned().unwrap_or_else(|| json!([]))
        }),
    )
    .await;

    Ok(payload)
}

pub(crate) async fn delete_prompt_preset_payload(
    state: &AppState,
    profile_id: &str,
    preset_id: &str,
) -> ApiResult<Value> {
    let trimmed_preset_id = preset_id.trim();
    let payload = with_ui_state_write(state, profile_id, |ui_state| {
        let Some(prompt_presets) = ui_state
            .get_mut("promptPresets")
            .and_then(Value::as_array_mut)
        else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "prompt presets state is missing",
            ));
        };

        *prompt_presets = prompt_presets
            .iter()
            .filter(|entry| entry.get("id").and_then(Value::as_str) != Some(trimmed_preset_id))
            .cloned()
            .collect::<Vec<_>>();
        prompt_presets.sort_by(|left, right| {
            let left_updated = left.get("updatedAt").and_then(Value::as_i64).unwrap_or(0);
            let right_updated = right.get("updatedAt").and_then(Value::as_i64).unwrap_or(0);
            right_updated.cmp(&left_updated)
        });

        Ok(json!({
            "promptPresets": prompt_presets.clone()
        }))
    })
    .await?;

    emit_profile_config_updated(
        state,
        profile_id,
        json!({
            "promptPresets": payload.get("promptPresets").cloned().unwrap_or_else(|| json!([]))
        }),
    )
    .await;

    Ok(payload)
}
