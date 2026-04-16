import os from "node:os";
import path from "node:path";

import type {
  ApprovalPolicy,
  SandboxMode,
  SessionPreferences,
  ServiceSpeed,
  CollaborationMode,
  ReasoningEffort,
  AutoApproveMode,
  SteeringResumeMode
} from "$lib/types";

import { readCodexTomlDefaults } from "./codex-config";

type RuntimeConfig = {
  password: string | null;
  passwordHash: string | null;
  sessionSecret: string | null;
  corsAllowedOrigins: string[];
  allowedRoots: string[];
  dataDir: string;
  codexHome: string;
  codexBin: string;
  maxUploadBytes: number;
  gitDiscoveryDepth: number;
  systemShutdownEnabled: boolean;
  systemShutdownDelaySeconds: number;
  systemShutdownCommandOverride: string | null;
  defaults: SessionPreferences;
  cookieSameSite: "strict" | "lax" | "none";
  cookieSecureMode: "auto" | "always" | "never";
};

const DEFAULT_ALLOWED_ROOT = (() => {
  const parent = path.resolve(process.cwd(), "..");
  return parent === path.parse(parent).root ? process.cwd() : parent;
})();

function parseEnum<T extends string>(value: string | undefined, allowed: readonly T[], fallback: T): T {
  return allowed.includes(value as T) ? (value as T) : fallback;
}

function parseAllowedRoots(): string[] {
  const raw = process.env.CODEX_WEBUI_ALLOWED_ROOTS;
  const roots = raw
    ? raw
        .split(path.delimiter)
        .map((entry: string) => entry.trim())
        .filter((entry: string) => entry.length > 0)
        .map((entry: string) => path.resolve(entry))
    : [DEFAULT_ALLOWED_ROOT];
  return [...new Set(roots)];
}

function parseCorsAllowedOrigins(): string[] {
  const raw = process.env.CODEX_WEBUI_CORS_ALLOWED_ORIGINS;
  if (!raw?.trim()) {
    return [];
  }

  const origins = raw
    .split(/[,\n]/u)
    .map((entry: string) => entry.trim())
    .filter((entry: string) => entry.length > 0)
    .map((entry: string) => {
      try {
        return new URL(entry).origin;
      } catch {
        throw new Error(`Invalid CODEX_WEBUI_CORS_ALLOWED_ORIGINS entry: ${entry}`);
      }
    });

  return [...new Set(origins)];
}

export function getRuntimeConfig(): RuntimeConfig {
  const codexHome = path.resolve(process.env.CODEX_WEBUI_CODEX_HOME ?? process.env.CODEX_HOME ?? path.join(os.homedir(), ".codex"));
  const codexDefaults = readCodexTomlDefaults(codexHome);
  const mode = parseEnum<CollaborationMode>(process.env.CODEX_WEBUI_DEFAULT_MODE, ["default", "plan"], "default");
  const speed = parseEnum<ServiceSpeed>(
    process.env.CODEX_WEBUI_DEFAULT_SPEED ?? (codexDefaults.serviceTier !== "auto" ? codexDefaults.serviceTier : undefined),
    ["auto", "fast", "flex"],
    "auto"
  );
  const sandboxMode = parseEnum<SandboxMode>(
    process.env.CODEX_WEBUI_DEFAULT_SANDBOX ?? codexDefaults.sandboxMode ?? undefined,
    ["read-only", "workspace-write", "danger-full-access"],
    "workspace-write"
  );
  const approvalPolicy = parseEnum<ApprovalPolicy>(
    process.env.CODEX_WEBUI_DEFAULT_APPROVAL_POLICY ?? codexDefaults.approvalPolicy ?? undefined,
    ["never", "on-request", "on-failure", "untrusted"],
    "on-request"
  );
  const effort = parseEnum<ReasoningEffort>(
    process.env.CODEX_WEBUI_DEFAULT_EFFORT ??
      (mode === "plan" ? codexDefaults.planModeReasoningEffort ?? undefined : codexDefaults.modelReasoningEffort ?? undefined),
    ["minimal", "low", "medium", "high", "xhigh"],
    "medium"
  );
  const cookieSameSite = parseEnum<"strict" | "lax" | "none">(
    process.env.CODEX_WEBUI_COOKIE_SAMESITE,
    ["strict", "lax", "none"],
    "strict"
  );
  const cookieSecureMode = parseEnum<"auto" | "always" | "never">(
    process.env.CODEX_WEBUI_COOKIE_SECURE,
    ["auto", "always", "never"],
    "auto"
  );
  const defaults: SessionPreferences = {
    cwd: parseAllowedRoots()[0],
    model: process.env.CODEX_WEBUI_DEFAULT_MODEL ?? codexDefaults.model ?? null,
    effort,
    speed,
    mode,
    sandboxMode,
    approvalPolicy,
    networkAccess:
      process.env.CODEX_WEBUI_DEFAULT_NETWORK === undefined
        ? codexDefaults.networkAccess ?? false
        : process.env.CODEX_WEBUI_DEFAULT_NETWORK === "true",
    autoApproveMode: parseEnum<AutoApproveMode>(process.env.CODEX_WEBUI_DEFAULT_AUTO_APPROVE, ["manual", "turn", "session"], "manual"),
    steeringResumeMode: parseEnum<SteeringResumeMode>(process.env.CODEX_WEBUI_DEFAULT_STEERING_RESUME, ["ask", "auto"], "ask"),
    shutdownOnCompletion: false,
    gitRepoPath: null
  };

  const maxUploadMb = Number(process.env.CODEX_WEBUI_MAX_UPLOAD_MB ?? "20");
  const gitDiscoveryDepth = Number(process.env.CODEX_WEBUI_GIT_DISCOVERY_DEPTH ?? "1");
  const systemShutdownDelaySeconds = Number(process.env.CODEX_WEBUI_SHUTDOWN_DELAY_SECONDS ?? "30");

  return {
    password: process.env.CODEX_WEBUI_PASSWORD ?? null,
    passwordHash: process.env.CODEX_WEBUI_PASSWORD_HASH ?? null,
    sessionSecret: process.env.CODEX_WEBUI_SESSION_SECRET ?? null,
    corsAllowedOrigins: parseCorsAllowedOrigins(),
    allowedRoots: parseAllowedRoots(),
    dataDir: path.resolve(process.env.CODEX_WEBUI_DATA_DIR ?? path.join(process.cwd(), ".data")),
    codexHome,
    codexBin: process.env.CODEX_WEBUI_CODEX_BIN ?? "codex",
    maxUploadBytes: Number.isFinite(maxUploadMb) && maxUploadMb > 0 ? maxUploadMb * 1024 * 1024 : 20 * 1024 * 1024,
    gitDiscoveryDepth: Number.isInteger(gitDiscoveryDepth) && gitDiscoveryDepth >= 0 ? gitDiscoveryDepth : 1,
    systemShutdownEnabled: process.env.CODEX_WEBUI_ENABLE_SYSTEM_SHUTDOWN === "true",
    systemShutdownDelaySeconds:
      Number.isFinite(systemShutdownDelaySeconds) && systemShutdownDelaySeconds >= 0 ? systemShutdownDelaySeconds : 30,
    systemShutdownCommandOverride: process.env.CODEX_WEBUI_SHUTDOWN_COMMAND?.trim() || null,
    defaults,
    cookieSameSite,
    cookieSecureMode
  };
}
