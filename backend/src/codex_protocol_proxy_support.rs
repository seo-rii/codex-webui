use super::*;

const CODEX_PROTOCOL_PROXY_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) async fn proxy_app_server_payload(
    state: &AppState,
    profile_id: &str,
    upstream_method: &str,
    params: Value,
) -> ApiResult<Value> {
    let client = app_server_client(state, profile_id)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to codex app-server: {error}"),
            )
        })?;

    client
        .request_with_timeout(
            upstream_method.to_string(),
            params,
            CODEX_PROTOCOL_PROXY_TIMEOUT,
            false,
        )
        .await
        .map_err(|error| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Failed to proxy Codex `{upstream_method}` request: {error}"),
            )
        })
}
