use super::*;

pub(crate) fn is_ws_method_allowed(role: UserRole, method: &str) -> bool {
    if role == UserRole::Admin {
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
            | "notifications/list"
            | "arena/list"
            | "session/subscribe"
            | "session/unsubscribe"
            | "events/subscribe"
            | "events/unsubscribe"
    )
}

pub(crate) fn should_audit_ws_method(method: &str) -> bool {
    !matches!(
        method,
        "config/get"
            | "runtime/status"
            | "runtime/checkUpdate"
            | "runtime/quota"
            | "catalog/get"
            | "directories/browse"
            | "editor/file/get"
            | "sessions/list"
            | "sessions/search"
            | "session/get"
            | "session/draft/get"
            | "session/queue/get"
            | "session/olderTurns/get"
            | "session/turn/get"
            | "session/itemDetail/get"
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
