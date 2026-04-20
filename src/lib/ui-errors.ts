import { m } from "$lib/paraglide/messages.js";

import { parseAppError } from "$lib/errors";

export function describeUiError(value: unknown) {
  const parsed = parseAppError(value);
  if (parsed) {
    switch (parsed.code) {
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
