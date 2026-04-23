export type AppErrorCode =
  | "EMPTY_MESSAGE"
  | "FORBIDDEN_ROLE"
  | "USAGE_LIMIT_EXCEEDED"
  | "INVALID_QUEUE_MODE"
  | "QUEUE_ITEM_NOT_FOUND"
  | "QUEUE_ALREADY_DISPATCHING"
  | "NO_ACTIVE_TURN"
  | "PENDING_REQUEST_NOT_FOUND"
  | "SESSION_ALREADY_ARCHIVED"
  | "SESSION_NOT_ARCHIVED"
  | "SESSION_NOT_FOUND"
  | "SESSION_ROLLOUT_NOT_FOUND"
  | "SESSION_ROLLOUT_NOT_RECOVERABLE";

export type AppErrorPayload = {
  code: AppErrorCode | string;
  message?: string;
  status?: number;
  retryAt?: number | string | null;
  retryAfterSeconds?: number | null;
  appServerError?: unknown;
};

function asRecord(value: unknown) {
  return value && typeof value === "object" ? (value as Record<string, unknown>) : null;
}

function maybeParseJson(value: unknown): unknown {
  if (typeof value !== "string") {
    return value;
  }

  const trimmed = value.trim();
  if (!trimmed.startsWith("{") && !trimmed.startsWith("[")) {
    return value;
  }

  try {
    return JSON.parse(trimmed) as unknown;
  } catch {
    return value;
  }
}

function normalizedErrorInfo(value: unknown): string {
  if (typeof value === "string") {
    return value.replace(/[^a-z0-9]/giu, "").toLowerCase();
  }

  const record = asRecord(value);
  if (!record) {
    return "";
  }

  return Object.keys(record)
    .join("")
    .replace(/[^a-z0-9]/giu, "")
    .toLowerCase();
}

export function isUsageLimitErrorPayload(value: unknown) {
  const parsed = maybeParseJson(value);
  const stack: unknown[] = [parsed];

  while (stack.length > 0) {
    const current = stack.pop();
    if (typeof current === "string") {
      const lowered = current.toLowerCase();
      const compact = current.replace(/[^a-z0-9]/giu, "").toLowerCase();
      if (
        compact === "usagelimitexceeded" ||
        (lowered.includes("usage limit") &&
          (lowered.includes("hit") || lowered.includes("exceeded") || lowered.includes("reached")))
      ) {
        return true;
      }
      const maybeJson = maybeParseJson(current);
      if (maybeJson !== current) {
        stack.push(maybeJson);
      }
      continue;
    }

    if (Array.isArray(current)) {
      stack.push(...current);
      continue;
    }

    const record = asRecord(current);
    if (!record) {
      continue;
    }

    if (
      normalizedErrorInfo(record.code) === "usagelimitexceeded" ||
      normalizedErrorInfo(record.codexErrorInfo) === "usagelimitexceeded" ||
      normalizedErrorInfo(record.errorInfo) === "usagelimitexceeded"
    ) {
      return true;
    }

    stack.push(...Object.values(record).map(maybeParseJson));
  }

  return false;
}

export function appErrorRetryAtMs(value: unknown): number | undefined {
  const now = Date.now();
  const candidates: number[] = [];
  const stack: unknown[] = [maybeParseJson(value)];

  while (stack.length > 0) {
    const current = stack.pop();
    if (typeof current === "string") {
      const maybeJson = maybeParseJson(current);
      if (maybeJson !== current) {
        stack.push(maybeJson);
      }
      continue;
    }

    if (Array.isArray(current)) {
      stack.push(...current);
      continue;
    }

    const record = asRecord(current);
    if (!record) {
      continue;
    }

    for (const [key, rawValue] of Object.entries(record)) {
      const normalizedKey = key.replace(/[^a-z0-9]/giu, "").toLowerCase();
      const absoluteKey = normalizedKey === "retryat" || normalizedKey === "resetat" || normalizedKey === "resetsat";
      const relativeKey = normalizedKey === "retryafterseconds" || normalizedKey === "resetafterseconds";
      if (absoluteKey || relativeKey) {
        const numeric = typeof rawValue === "number" ? rawValue : typeof rawValue === "string" ? Number(rawValue.trim()) : NaN;
        if (Number.isFinite(numeric) && numeric > 0) {
          candidates.push(relativeKey ? now + numeric * 1000 : numeric >= 100_000_000_000 ? numeric : numeric * 1000);
        } else if (typeof rawValue === "string") {
          const parsedTime = Date.parse(rawValue);
          if (Number.isFinite(parsedTime)) {
            candidates.push(parsedTime);
          }
        }
      }

      const maybeJson = maybeParseJson(rawValue);
      if (Array.isArray(maybeJson) || asRecord(maybeJson)) {
        stack.push(maybeJson);
      }
    }
  }

  return (
    candidates
      .filter((candidate) => candidate >= now)
      .sort((a, b) => a - b)[0] ?? candidates.sort((a, b) => b - a)[0]
  );
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
      if (isUsageLimitErrorPayload(trimmed)) {
        return {
          code: "USAGE_LIMIT_EXCEEDED",
          message: trimmed,
          retryAt: appErrorRetryAtMs(trimmed) ?? null,
          retryAfterSeconds: null
        };
      }
      return null;
    }

    try {
      return parseAppError(JSON.parse(trimmed));
    } catch {
      return null;
    }
  }

  const record = asRecord(value);
  if (!record) {
    return null;
  }

  const nestedError = parseAppError(record.error);
  if (nestedError) {
    return nestedError;
  }

  const directUsageLimit =
    normalizedErrorInfo(record.code) === "usagelimitexceeded" ||
    normalizedErrorInfo(record.codexErrorInfo) === "usagelimitexceeded" ||
    normalizedErrorInfo(record.errorInfo) === "usagelimitexceeded";
  if (!directUsageLimit) {
    const nestedMessage = parseAppError(record.message);
    if (nestedMessage) {
      return nestedMessage;
    }
  }

  const code =
    typeof record.code === "string"
      ? record.code
      : directUsageLimit || isUsageLimitErrorPayload(record)
        ? "USAGE_LIMIT_EXCEEDED"
        : null;
  if (!code) {
    return parseAppError(record.message);
  }

  return {
    code,
    message: typeof record.message === "string" ? record.message : undefined,
    status: typeof record.status === "number" && Number.isFinite(record.status) ? record.status : undefined,
    retryAt: appErrorRetryAtMs(record) ?? null,
    retryAfterSeconds:
      typeof record.retryAfterSeconds === "number" && Number.isFinite(record.retryAfterSeconds)
        ? record.retryAfterSeconds
        : null,
    appServerError: record.appServerError
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
