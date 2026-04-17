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
import { getCurrentProfileId } from "./profile-context";

export type RuntimeProfileConfig = {
  id: string;
  label: string;
  codexHome: string;
  dataDir: string;
  defaults: SessionPreferences;
};

type RuntimeConfig = {
  password: string | null;
  passwordHash: string | null;
  sessionSecret: string | null;
  corsAllowedOrigins: string[];
  allowedRoots: string[];
  codexBin: string;
  maxUploadBytes: number;
  gitDiscoveryDepth: number;
  systemShutdownEnabled: boolean;
  systemShutdownDelaySeconds: number;
  systemShutdownCommandOverride: string | null;
  defaults: SessionPreferences;
  codexHome: string;
  dataDir: string;
  defaultProfileId: string;
  profiles: RuntimeProfileConfig[];
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

function sanitizeProfileId(value: string | null | undefined) {
  const normalized = String(value ?? "")
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/gu, "-")
    .replace(/^-+|-+$/gu, "");
  return normalized || "default";
}

function buildProfileDefaults(
  codexHome: string,
  allowedRoots: string[],
  overrides: Partial<{
    model: string | null;
    effort: ReasoningEffort | null;
    speed: ServiceSpeed;
    mode: CollaborationMode;
    sendOnEnter: boolean;
    sandboxMode: SandboxMode;
    approvalPolicy: ApprovalPolicy;
    networkAccess: boolean;
    autoApproveMode: AutoApproveMode;
    steeringResumeMode: SteeringResumeMode;
  }> = {}
) {
  const codexDefaults = readCodexTomlDefaults(codexHome);
  const mode = parseEnum<CollaborationMode>(
    process.env.CODEX_WEBUI_DEFAULT_MODE,
    ["default", "plan"],
    overrides.mode ?? "default"
  );
  const speed = parseEnum<ServiceSpeed>(
    process.env.CODEX_WEBUI_DEFAULT_SPEED ?? (codexDefaults.serviceTier !== "auto" ? codexDefaults.serviceTier : undefined),
    ["auto", "fast", "flex"],
    overrides.speed ?? "auto"
  );
  const sandboxMode = parseEnum<SandboxMode>(
    process.env.CODEX_WEBUI_DEFAULT_SANDBOX ?? codexDefaults.sandboxMode ?? undefined,
    ["read-only", "workspace-write", "danger-full-access"],
    overrides.sandboxMode ?? "workspace-write"
  );
  const approvalPolicy = parseEnum<ApprovalPolicy>(
    process.env.CODEX_WEBUI_DEFAULT_APPROVAL_POLICY ?? codexDefaults.approvalPolicy ?? undefined,
    ["never", "on-request", "on-failure", "untrusted"],
    overrides.approvalPolicy ?? "on-request"
  );
  const effort = parseEnum<ReasoningEffort>(
    process.env.CODEX_WEBUI_DEFAULT_EFFORT ??
      (mode === "plan" ? codexDefaults.planModeReasoningEffort ?? undefined : codexDefaults.modelReasoningEffort ?? undefined),
    ["minimal", "low", "medium", "high", "xhigh"],
    overrides.effort ?? "medium"
  );

  return {
    cwd: allowedRoots[0],
    model: overrides.model ?? process.env.CODEX_WEBUI_DEFAULT_MODEL ?? codexDefaults.model ?? null,
    effort,
    speed,
    mode,
    sendOnEnter:
      process.env.CODEX_WEBUI_DEFAULT_SEND_ON_ENTER === undefined
        ? Boolean(overrides.sendOnEnter ?? false)
        : process.env.CODEX_WEBUI_DEFAULT_SEND_ON_ENTER === "true",
    sandboxMode,
    approvalPolicy,
    networkAccess:
      process.env.CODEX_WEBUI_DEFAULT_NETWORK === undefined
        ? overrides.networkAccess ?? codexDefaults.networkAccess ?? false
        : process.env.CODEX_WEBUI_DEFAULT_NETWORK === "true",
    autoApproveMode: parseEnum<AutoApproveMode>(process.env.CODEX_WEBUI_DEFAULT_AUTO_APPROVE, ["manual", "turn", "session"], overrides.autoApproveMode ?? "manual"),
    steeringResumeMode: parseEnum<SteeringResumeMode>(process.env.CODEX_WEBUI_DEFAULT_STEERING_RESUME, ["ask", "auto"], overrides.steeringResumeMode ?? "ask"),
    shutdownOnCompletion: false,
    gitRepoPath: null
  } satisfies SessionPreferences;
}

function parseProfiles(
  allowedRoots: string[],
  defaultCodexHome: string,
  defaultDataDir: string
): { defaultProfileId: string; profiles: RuntimeProfileConfig[] } {
  const rawProfiles = process.env.CODEX_WEBUI_PROFILES_JSON?.trim();
  const defaultProfileId = sanitizeProfileId(process.env.CODEX_WEBUI_DEFAULT_PROFILE_ID);

  if (!rawProfiles) {
    return {
      defaultProfileId,
      profiles: [
        {
          id: defaultProfileId,
          label: "Default",
          codexHome: defaultCodexHome,
          dataDir: path.join(defaultDataDir, "profiles", defaultProfileId),
          defaults: buildProfileDefaults(defaultCodexHome, allowedRoots)
        }
      ]
    };
  }

  const parsed = JSON.parse(rawProfiles) as Array<Record<string, unknown>>;
  const seenIds = new Set<string>();
  const profiles: RuntimeProfileConfig[] = [];

  for (const entry of parsed) {
    const id = sanitizeProfileId(typeof entry.id === "string" ? entry.id : null);
    if (seenIds.has(id)) {
      continue;
    }
    seenIds.add(id);
    const rawCodexHome = typeof entry.codexHome === "string" ? entry.codexHome : typeof entry.codex_home === "string" ? entry.codex_home : null;
    const rawDataDir = typeof entry.dataDir === "string" ? entry.dataDir : typeof entry.data_dir === "string" ? entry.data_dir : null;
    const codexHome =
      typeof rawCodexHome === "string" && rawCodexHome.trim()
        ? path.resolve(rawCodexHome)
        : defaultCodexHome;
    const dataDir =
      typeof rawDataDir === "string" && rawDataDir.trim()
        ? path.resolve(rawDataDir)
        : path.join(defaultDataDir, "profiles", id);
    const label =
      typeof entry.label === "string" && entry.label.trim()
        ? entry.label.trim()
        : id === defaultProfileId
          ? "Default"
          : id;

    profiles.push({
      id,
      label,
      codexHome,
      dataDir,
      defaults: buildProfileDefaults(codexHome, allowedRoots)
    });
  }

  if (profiles.length === 0) {
    profiles.push({
      id: defaultProfileId,
      label: "Default",
      codexHome: defaultCodexHome,
      dataDir: path.join(defaultDataDir, "profiles", defaultProfileId),
      defaults: buildProfileDefaults(defaultCodexHome, allowedRoots)
    });
  }

  const hasDefault = profiles.some((profile) => profile.id === defaultProfileId);
  if (!hasDefault) {
    return {
      defaultProfileId: profiles[0].id,
      profiles
    };
  }

  return {
    defaultProfileId,
    profiles
  };
}

export function getRuntimeConfig(): RuntimeConfig {
  const allowedRoots = parseAllowedRoots();
  const codexHome = path.resolve(process.env.CODEX_WEBUI_CODEX_HOME ?? process.env.CODEX_HOME ?? path.join(os.homedir(), ".codex"));
  const dataDir = path.resolve(process.env.CODEX_WEBUI_DATA_DIR ?? path.join(process.cwd(), ".data"));
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
  const { defaultProfileId, profiles } = parseProfiles(allowedRoots, codexHome, dataDir);
  const defaultProfile = profiles.find((profile) => profile.id === defaultProfileId) ?? profiles[0];

  const maxUploadMb = Number(process.env.CODEX_WEBUI_MAX_UPLOAD_MB ?? "20");
  const gitDiscoveryDepth = Number(process.env.CODEX_WEBUI_GIT_DISCOVERY_DEPTH ?? "1");
  const systemShutdownDelaySeconds = Number(process.env.CODEX_WEBUI_SHUTDOWN_DELAY_SECONDS ?? "30");

  return {
    password: process.env.CODEX_WEBUI_PASSWORD ?? null,
    passwordHash: process.env.CODEX_WEBUI_PASSWORD_HASH ?? null,
    sessionSecret: process.env.CODEX_WEBUI_SESSION_SECRET ?? null,
    corsAllowedOrigins: parseCorsAllowedOrigins(),
    allowedRoots,
    dataDir,
    codexHome,
    codexBin: process.env.CODEX_WEBUI_CODEX_BIN ?? "codex",
    maxUploadBytes: Number.isFinite(maxUploadMb) && maxUploadMb > 0 ? maxUploadMb * 1024 * 1024 : 20 * 1024 * 1024,
    gitDiscoveryDepth: Number.isInteger(gitDiscoveryDepth) && gitDiscoveryDepth >= 0 ? gitDiscoveryDepth : 1,
    systemShutdownEnabled: process.env.CODEX_WEBUI_ENABLE_SYSTEM_SHUTDOWN === "true",
    systemShutdownDelaySeconds:
      Number.isFinite(systemShutdownDelaySeconds) && systemShutdownDelaySeconds >= 0 ? systemShutdownDelaySeconds : 30,
    systemShutdownCommandOverride: process.env.CODEX_WEBUI_SHUTDOWN_COMMAND?.trim() || null,
    defaults: defaultProfile.defaults,
    defaultProfileId: defaultProfile.id,
    profiles,
    cookieSameSite,
    cookieSecureMode
  };
}

export function getRuntimeProfile(profileId: string | null | undefined): RuntimeProfileConfig {
  const config = getRuntimeConfig();
  return config.profiles.find((profile) => profile.id === profileId) ?? config.profiles.find((profile) => profile.id === config.defaultProfileId) ?? config.profiles[0];
}

export function getCurrentRuntimeProfile(): RuntimeProfileConfig {
  return getRuntimeProfile(getCurrentProfileId());
}
