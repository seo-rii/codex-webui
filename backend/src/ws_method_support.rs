use super::*;

pub(crate) fn is_ws_method_allowed(role: UserRole, method: &str) -> bool {
    if role_has_admin_access(role) {
        return true;
    }

    matches!(
        method,
        "runtime/status"
            | "sessions/list"
            | "sessions/search"
            | "session/get"
            | "session/olderTurns/get"
            | "session/turn/get"
            | "session/itemDetail/get"
            | "session/goal/get"
            | "notifications/list"
            | "arena/list"
            | "session/subscribe"
            | "session/unsubscribe"
            | "events/subscribe"
            | "events/unsubscribe"
    )
}

pub(crate) fn ws_method_requires_owner(method: &str, params: &Value) -> bool {
    matches!(
        method,
        "config/update"
            | "codex/features/set"
            | "codex/marketplaces/add"
            | "codex/marketplaces/remove"
            | "codex/marketplaces/upgrade"
            | "codex/mcp/oauth/login"
            | "codex/mcp/refresh"
            | "codex/plugins/install"
            | "codex/plugins/uninstall"
            | "runtime/install"
            | "runtime/process/kill"
            | "runtime/resetTicket/use"
            | "runtime/update"
            | "gateway/restart"
            | "memory/reset"
            | "terminal/list"
            | "terminal/create"
            | "terminal/read"
            | "terminal/context/attach"
            | "terminal/input"
            | "terminal/close"
            | "terminal/subscribe"
            | "terminal/unsubscribe"
            | "system/shutdown/force"
    ) || (matches!(
        method,
        "session/create" | "session/savePreferences" | "turn/send" | "arena/start"
    ) && preferences_payload_requires_owner(
        params.get("preferences").unwrap_or(&Value::Null),
    )) || (method == "git/worktrees/remove"
        && params
            .get("force")
            .and_then(Value::as_bool)
            .unwrap_or(false))
}

pub(crate) fn ws_method_uses_request_replay(method: &str) -> bool {
    !matches!(
        method,
        "session/subscribe"
            | "session/unsubscribe"
            | "terminal/subscribe"
            | "terminal/unsubscribe"
            | "events/subscribe"
            | "events/unsubscribe"
            | "codex/realtime/start"
            | "codex/realtime/appendAudio"
            | "codex/realtime/appendText"
            | "codex/realtime/stop"
    )
}

pub(crate) fn authorize_ws_method(
    config: &Config,
    role: UserRole,
    method: &str,
    params: &Value,
) -> Result<()> {
    if !is_ws_method_allowed(role, method) {
        anyhow::bail!(
            "{{\"code\":\"FORBIDDEN_ROLE\",\"message\":\"This action requires an admin role.\"}}"
        );
    }
    if ws_method_requires_owner(method, params) && !role_has_owner_access(config, role) {
        anyhow::bail!(owner_required_error_value());
    }
    Ok(())
}

pub(crate) fn should_audit_ws_method(method: &str) -> bool {
    !matches!(
        method,
        "config/get"
            | "diagnostics/parser/compare"
            | "runtime/status"
            | "runtime/checkUpdate"
            | "runtime/processes/list"
            | "runtime/quota"
            | "runtime/resetTickets"
            | "memory/status"
            | "codex/features/list"
            | "codex/plugins/list"
            | "codex/plugins/read"
            | "codex/plugins/skill/read"
            | "codex/skills/list"
            | "codex/hooks/list"
            | "codex/mcp/status/list"
            | "codex/apps/list"
            | "codex/realtime/listVoices"
            | "catalog/get"
            | "directories/browse"
            | "files/search"
            | "editor/file/get"
            | "sessions/list"
            | "sessions/search"
            | "session/get"
            | "session/draft/get"
            | "session/queue/get"
            | "session/olderTurns/get"
            | "session/rollbackTargets/list"
            | "session/turn/get"
            | "session/itemDetail/get"
            | "session/goal/get"
            | "notifications/list"
            | "account/get"
            | "arena/list"
            | "git/repositories/list"
            | "git/status"
            | "git/github/pulls"
            | "git/github/pull"
            | "git/commit/diff"
            | "git/file/get"
            | "git/file/resolve"
            | "git/worktrees/list"
            | "terminal/list"
            | "terminal/read"
            | "session/subscribe"
            | "session/unsubscribe"
            | "events/subscribe"
            | "events/unsubscribe"
            | "terminal/subscribe"
            | "terminal/unsubscribe"
    )
}

pub(crate) fn summarize_audit_target(params: &Value) -> Option<String> {
    for key in [
        "sessionId",
        "threadId",
        "terminalId",
        "queueId",
        "turnId",
        "presetId",
        "filterId",
        "repoPath",
        "filePath",
    ] {
        if let Some(value) = params
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            return Some(value.trim().to_string());
        }
    }
    None
}
