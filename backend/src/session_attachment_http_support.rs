use super::*;

pub(crate) async fn handle_session_attachments_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    session_id: &str,
) -> Response {
    let method = request.method().clone();
    match method {
        Method::GET => {
            match list_session_attachments_payload(&state, &auth.profile_id, session_id).await {
                Ok(attachments) => Json(json!({ "attachments": attachments })).into_response(),
                Err(error) => json_error(error.status, &error.message),
            }
        }
        Method::POST => {
            if !role_has_admin_access(auth.role) {
                return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
            }
            let max_total_upload_bytes = state
                .config
                .max_upload_bytes
                .saturating_mul(MAX_ATTACHMENTS_PER_REQUEST as u64);
            if request
                .headers()
                .get(header::CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .is_some_and(|length| length > max_total_upload_bytes)
            {
                return json_error(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    &attachment_limit_error_message(max_total_upload_bytes),
                );
            }

            let multipart = match Multipart::from_request(request, &()).await {
                Ok(multipart) => multipart,
                Err(_) => {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        "Failed to read attachment upload body.",
                    );
                }
            };
            let mut multipart = multipart;
            let mut stored = Vec::new();
            let uploads_dir = session_uploads_dir(&state, &auth.profile_id, session_id);
            if let Err(error) = tokio_fs::create_dir_all(&uploads_dir).await {
                return json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string());
            }

            loop {
                let mut field = match multipart.next_field().await {
                    Ok(Some(field)) => field,
                    Ok(None) => break,
                    Err(_) => {
                        return json_error(
                            StatusCode::BAD_REQUEST,
                            "Failed to read attachment upload body.",
                        );
                    }
                };

                if field.name() != Some("files") {
                    continue;
                }
                if stored.len() >= MAX_ATTACHMENTS_PER_REQUEST {
                    let error = attachment_count_limit_error();
                    return json_error(error.status, &error.message);
                }

                let file_name = field
                    .file_name()
                    .map(str::to_string)
                    .unwrap_or_else(|| "attachment".to_string());
                let mime_type = field.content_type().map(str::to_string);
                let temp_path = uploads_dir.join(format!(".{}.upload", Uuid::new_v4()));
                let mut temp_file = match tokio_fs::File::create(&temp_path).await {
                    Ok(file) => file,
                    Err(error) => {
                        return json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string());
                    }
                };
                let mut size = 0_u64;
                loop {
                    let chunk = match field.chunk().await {
                        Ok(Some(chunk)) => chunk,
                        Ok(None) => break,
                        Err(_) => {
                            let _ = tokio_fs::remove_file(&temp_path).await;
                            return json_error(
                                StatusCode::BAD_REQUEST,
                                "Failed to read attachment upload body.",
                            );
                        }
                    };
                    size = size.saturating_add(chunk.len() as u64);
                    if let Err(error) = validate_attachment_size(&state.config, size) {
                        let _ = tokio_fs::remove_file(&temp_path).await;
                        return json_error(error.status, &error.message);
                    }
                    if let Err(error) = temp_file.write_all(&chunk).await {
                        let _ = tokio_fs::remove_file(&temp_path).await;
                        return json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string());
                    }
                }
                if let Err(error) = temp_file.flush().await {
                    let _ = tokio_fs::remove_file(&temp_path).await;
                    return json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string());
                }
                drop(temp_file);
                if size == 0 {
                    let _ = tokio_fs::remove_file(&temp_path).await;
                    continue;
                }

                match store_uploaded_attachment_file(
                    &state,
                    &auth.profile_id,
                    session_id,
                    &file_name,
                    mime_type,
                    size,
                    &temp_path,
                )
                .await
                {
                    Ok(record) => stored.push(record),
                    Err(error) => {
                        let _ = tokio_fs::remove_file(&temp_path).await;
                        return json_error(error.status, &error.message);
                    }
                }
            }

            if stored.is_empty() {
                return json_error(StatusCode::BAD_REQUEST, "Select at least one file.");
            }
            if let Err(error) = emit_attachments_updated(&state, &auth.profile_id, session_id).await
            {
                return json_error(error.status, &error.message);
            }
            let mut response = Json(json!({
                "attachments": stored
                    .iter()
                    .map(attachment_payload_from_record)
                    .collect::<Vec<_>>()
            }))
            .into_response();
            *response.status_mut() = StatusCode::CREATED;
            response
        }
        _ => json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed."),
    }
}

pub(crate) async fn handle_session_attachment_api_http(
    state: AppState,
    request: Request,
    auth: AuthContext,
    session_id: &str,
    attachment_id: &str,
) -> Response {
    if request.method() != Method::DELETE {
        return json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }
    if !role_has_admin_access(auth.role) {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    match delete_attachment_payload(&state, &auth.profile_id, session_id, attachment_id).await {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}
