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
            if auth.role != UserRole::Admin {
                return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
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
            let mut uploads = Vec::new();

            loop {
                let field = match multipart.next_field().await {
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

                let file_name = field
                    .file_name()
                    .map(str::to_string)
                    .unwrap_or_else(|| "attachment".to_string());
                let mime_type = field.content_type().map(str::to_string);
                let bytes = match field.bytes().await {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        return json_error(
                            StatusCode::BAD_REQUEST,
                            "Failed to read attachment upload body.",
                        );
                    }
                };
                if bytes.is_empty() {
                    continue;
                }

                uploads.push(AttachmentUploadPayload {
                    name: file_name,
                    mime_type,
                    bytes: bytes.to_vec(),
                });
            }

            match save_uploaded_attachment_records(&state, &auth.profile_id, session_id, uploads)
                .await
            {
                Ok(stored) => {
                    if let Err(error) =
                        emit_attachments_updated(&state, &auth.profile_id, session_id).await
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
                Err(error) => json_error(error.status, &error.message),
            }
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
    if auth.role != UserRole::Admin {
        return json_error(StatusCode::FORBIDDEN, "This action requires an admin role.");
    }

    match delete_attachment_payload(&state, &auth.profile_id, session_id, attachment_id).await {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => json_error(error.status, &error.message),
    }
}
