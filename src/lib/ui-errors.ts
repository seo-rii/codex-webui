import { m } from "$lib/paraglide/messages.js";

import { appErrorRetryAtMs, isUsageLimitErrorPayload, parseAppError } from "$lib/errors";

function formatUsageRetryTime(retryAtMs: number) {
  const retryAt = new Date(retryAtMs);
  const now = new Date();
  const sameDay =
    retryAt.getFullYear() === now.getFullYear() &&
    retryAt.getMonth() === now.getMonth() &&
    retryAt.getDate() === now.getDate();

  return new Intl.DateTimeFormat(undefined, {
    month: sameDay ? undefined : "short",
    day: sameDay ? undefined : "numeric",
    year: sameDay ? undefined : "numeric",
    hour: "numeric",
    minute: "2-digit"
  }).format(retryAt);
}

function describeUsageLimitError(value: unknown, message?: string) {
  const retryAtMs = appErrorRetryAtMs(value);
  const trimmedMessage = message?.trim() ?? "";
  const hasRetrySuffix = /try again (at|later)/iu.test(trimmedMessage) || /다시.*(보낼|시도)/u.test(trimmedMessage);
  if (trimmedMessage && hasRetrySuffix) {
    return trimmedMessage;
  }

  if (retryAtMs) {
    const time = formatUsageRetryTime(retryAtMs);
    if (trimmedMessage && trimmedMessage !== "You've hit your usage limit.") {
      return `${trimmedMessage.replace(/\s+$/u, "").replace(/[.。]$/u, "")}. ${m.error_usage_retry_at_suffix({ time })}`;
    }
    return m.error_usage_limit_exceeded_retry_at({ time });
  }

  return trimmedMessage || m.error_usage_limit_exceeded();
}

export function describeUiError(value: unknown) {
  const parsed = parseAppError(value);
  if (parsed) {
    switch (parsed.code) {
      case "USAGE_LIMIT_EXCEEDED":
        return describeUsageLimitError(parsed, parsed.message);
      case "EMPTY_MESSAGE":
        return m.error_empty_message();
      case "FORBIDDEN_ROLE":
        return m.error_forbidden_role();
      case "INVALID_QUEUE_MODE":
        return m.error_invalid_queue_mode();
      case "QUEUE_ITEM_NOT_FOUND":
        return m.error_queue_item_not_found();
      case "QUEUE_ALREADY_DISPATCHING":
        return m.error_queue_already_dispatching();
      case "NO_ACTIVE_TURN":
        return m.error_no_active_turn();
      case "PENDING_REQUEST_NOT_FOUND":
        return m.error_pending_request_not_found();
      case "SESSION_ALREADY_ARCHIVED":
        return m.error_session_already_archived();
      case "SESSION_NOT_ARCHIVED":
        return m.error_session_not_archived();
      case "SESSION_NOT_FOUND":
        return m.error_session_not_found();
      case "SESSION_ROLLOUT_NOT_FOUND":
        return m.error_session_rollout_not_found();
      case "SESSION_ROLLOUT_NOT_RECOVERABLE":
        return m.error_session_rollout_not_recoverable();
      default:
        if (parsed.message?.trim()) {
          return parsed.message.trim();
        }
    }
  }

  if (isUsageLimitErrorPayload(value)) {
    return describeUsageLimitError(value, typeof value === "string" ? value : undefined);
  }

  if (value instanceof Error) {
    const message = value.message.trim();
    if (message) {
      return message;
    }
  }

  if (typeof value === "string" && value.trim()) {
    return value.trim();
  }

  return m.unknown_error();
}
