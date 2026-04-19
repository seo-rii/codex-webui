import fsp from "node:fs/promises";
import path from "node:path";

import { getRuntimeConfig, type RuntimeProfileConfig } from "./env";

type RuntimeErrorLogPayload = {
  source: string;
  message: string;
  profileId?: string | null;
  details?: unknown;
};

let processErrorLoggingInstalled = false;

function getRuntimeLogsDir() {
  return path.join(getRuntimeConfig().dataDir, "logs");
}

export function getRuntimeErrorLogPath() {
  return path.join(getRuntimeLogsDir(), "runtime-errors.jsonl");
}

export function getProfileLogPath(profile: RuntimeProfileConfig, filename: string) {
  return path.join(profile.dataDir, "logs", filename);
}

async function ensureLogDirectory(filePath: string) {
  await fsp.mkdir(path.dirname(filePath), { recursive: true });
}

export async function appendLogLine(filePath: string, line: string) {
  const message = line.trim();
  if (!message) {
    return;
  }

  await ensureLogDirectory(filePath);
  await fsp.appendFile(filePath, `${new Date().toISOString()} ${message}\n`, "utf8");
}

export function serializeErrorForLog(value: unknown, depth = 0): unknown {
  if (depth > 3) {
    return "[max-depth]";
  }

  if (value instanceof Error) {
    const serialized: Record<string, unknown> = {
      name: value.name,
      message: value.message
    };
    if (value.stack) {
      serialized.stack = value.stack;
    }
    if ("cause" in value && value.cause !== undefined) {
      serialized.cause = serializeErrorForLog(value.cause, depth + 1);
    }
    return serialized;
  }

  if (Array.isArray(value)) {
    return value.map((entry) => serializeErrorForLog(entry, depth + 1));
  }

  if (value && typeof value === "object") {
    try {
      return JSON.parse(JSON.stringify(value));
    } catch {
      return String(value);
    }
  }

  if (typeof value === "string" || typeof value === "number" || typeof value === "boolean" || value === null) {
    return value;
  }

  return String(value);
}

export async function appendRuntimeErrorLog(payload: RuntimeErrorLogPayload) {
  const record = {
    at: new Date().toISOString(),
    pid: process.pid,
    source: payload.source,
    message: payload.message,
    profileId: payload.profileId ?? null,
    details: payload.details === undefined ? null : serializeErrorForLog(payload.details)
  };

  const logPath = getRuntimeErrorLogPath();
  await ensureLogDirectory(logPath);
  await fsp.appendFile(logPath, `${JSON.stringify(record)}\n`, "utf8");
}

export function installRuntimeProcessErrorLogging() {
  if (processErrorLoggingInstalled) {
    return;
  }
  processErrorLoggingInstalled = true;

  process.on("uncaughtExceptionMonitor", (error, origin) => {
    void appendRuntimeErrorLog({
      source: "node-backend",
      message: "uncaught exception",
      details: {
        origin,
        error: serializeErrorForLog(error)
      }
    }).catch(() => {});
  });

  process.on("unhandledRejection", (reason) => {
    void appendRuntimeErrorLog({
      source: "node-backend",
      message: "unhandled rejection",
      details: {
        reason: serializeErrorForLog(reason)
      }
    }).catch(() => {});
  });
}
