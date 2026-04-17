export type AppErrorCode =
  | "EMPTY_MESSAGE"
  | "FORBIDDEN_ROLE"
  | "INVALID_QUEUE_MODE"
  | "QUEUE_ITEM_NOT_FOUND"
  | "QUEUE_ALREADY_DISPATCHING"
  | "NO_ACTIVE_TURN"
  | "PENDING_REQUEST_NOT_FOUND"
  | "SESSION_ALREADY_ARCHIVED"
  | "SESSION_NOT_ARCHIVED"
  | "SESSION_NOT_FOUND";

export type AppErrorPayload = {
  code: AppErrorCode | string;
  message?: string;
  status?: number;
};

function asRecord(value: unknown) {
  return value && typeof value === "object" ? (value as Record<string, unknown>) : null;
}

export function serializeAppError(payload: AppErrorPayload) {
  return JSON.stringify(payload);
}

export function parseAppError(value: unknown): AppErrorPayload | null {
  if (value instanceof Error) {
    return parseAppError(value.message);
  }

  if (typeof value === "string") {
    const trimmed = value.trim();
    if (!trimmed.startsWith("{")) {
      return null;
    }

    try {
      return parseAppError(JSON.parse(trimmed));
    } catch {
      return null;
    }
  }

  const record = asRecord(value);
  if (!record || typeof record.code !== "string") {
    return null;
  }

  return {
    code: record.code,
    message: typeof record.message === "string" ? record.message : undefined,
    status: typeof record.status === "number" && Number.isFinite(record.status) ? record.status : undefined
  };
}

export function createAppError(code: AppErrorCode, message?: string) {
  return new Error(
    serializeAppError({
      code,
      message
    })
  );
}
