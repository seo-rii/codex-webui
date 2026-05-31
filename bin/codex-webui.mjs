#!/usr/bin/env node

import { randomBytes, scryptSync } from "node:crypto";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { createInterface } from "node:readline/promises";
import { spawn, spawnSync } from "node:child_process";
import process from "node:process";

import YAML from "yaml";

const packageRoot = path.resolve(new URL("..", import.meta.url).pathname);
const stateDir = path.join(os.homedir(), ".codex", "codex-webui");
const configPath = path.join(os.homedir(), ".codex", "codex-webui.yml");
const pidPath = path.join(stateDir, "server.pid");
const serverMetaPath = path.join(stateDir, "server.json");
const logPath = path.join(stateDir, "server.log");
const tunnelPidPath = path.join(stateDir, "tunnel.pid");
const tunnelLogPath = path.join(stateDir, "tunnel.log");
const tunnelMetaPath = path.join(stateDir, "tunnel.json");

function runtimeErrorLogPath(config) {
  return path.join(config.dataDir, "logs", "runtime-errors.jsonl");
}

function expandHome(input) {
  if (!input) {
    return input;
  }
  if (input === "~") {
    return os.homedir();
  }
  if (input.startsWith("~/")) {
    return path.join(os.homedir(), input.slice(2));
  }
  return input;
}

function commandAvailable(command) {
  const probe = process.platform === "win32" ? ["where", command] : ["which", command];
  return spawnSync(probe[0], probe.slice(1), { stdio: "ignore" }).status === 0;
}

function hashPassword(password) {
  const salt = randomBytes(16);
  const key = scryptSync(password, salt, 64);
  return `scrypt$${salt.toString("base64url")}$${key.toString("base64url")}`;
}

function createSessionSecret() {
  return randomBytes(32).toString("base64url");
}

function defaultBasePath(port) {
  return `/absproxy/${port}`;
}

function currentRustTargets() {
  const candidates = [];
  if (process.platform === "linux" && process.arch === "x64") {
    candidates.push("x86_64-unknown-linux-gnu", "x86_64-unknown-linux-musl");
  } else if (process.platform === "linux" && process.arch === "arm64") {
    candidates.push("aarch64-unknown-linux-gnu", "aarch64-unknown-linux-musl");
  } else if (process.platform === "darwin" && process.arch === "x64") {
    candidates.push("x86_64-apple-darwin");
  } else if (process.platform === "darwin" && process.arch === "arm64") {
    candidates.push("aarch64-apple-darwin");
  } else if (process.platform === "win32" && process.arch === "x64") {
    candidates.push("x86_64-pc-windows-msvc");
  } else if (process.platform === "win32" && process.arch === "arm64") {
    candidates.push("aarch64-pc-windows-msvc");
  }
  return candidates;
}

function defaultTunnelConfigValues() {
  return {
    provider: "auto",
    background: true,
    hostname: "",
    name: "",
    overwriteDns: false,
    logLevel: "info",
    extraArgs: []
  };
}

function defaultConfigValues() {
  const port = 4173;
  const dataDir = path.join(os.homedir(), ".codex", "codex-webui", "data");
  const codexHome = path.join(os.homedir(), ".codex");
  return {
    host: "127.0.0.1",
    port,
    basePath: defaultBasePath(port),
    codexBin: "codex",
    codexHome,
    dataDir,
    defaultProfileId: "default",
    profiles: [
      {
        id: "default",
        label: "Default",
        codexHome,
        dataDir: path.join(dataDir, "profiles", "default")
      }
    ],
    allowedRoots: [process.cwd()],
    passwordHash: "",
    ownerPasswordHash: "",
    hcaptchaSiteKey: "",
    hcaptchaSecretKey: "",
    sessionSecret: createSessionSecret(),
    corsAllowedOrigins: [],
    backendBinaryPath: "",
    appServerHandoff: true,
    perSessionAppServers: false,
    tunnel: defaultTunnelConfigValues()
  };
}

function sanitizeProfileId(input) {
  const normalized = String(input ?? "")
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/gu, "-")
    .replace(/^-+|-+$/gu, "");
  return normalized || "default";
}

function defaultProfileDataDir(rootDataDir, profileId) {
  return path.join(rootDataDir, "profiles", profileId);
}

function normalizeTunnelProvider(input) {
  return ["auto", "cloudflared", "ngrok"].includes(String(input ?? "").trim()) ? String(input).trim() : "auto";
}

function parseBooleanInput(input, fallback) {
  const normalized = String(input ?? "").trim().toLowerCase();
  if (!normalized) {
    return fallback;
  }
  if (["y", "yes", "true", "1", "on"].includes(normalized)) {
    return true;
  }
  if (["n", "no", "false", "0", "off"].includes(normalized)) {
    return false;
  }
  return fallback;
}

function formatOptionalPrompt(value, emptyLabel = "none") {
  return value ? String(value) : emptyLabel;
}

function normalizeTunnelConfig(rawTunnel = {}) {
  const defaults = defaultTunnelConfigValues();
  return {
    ...defaults,
    ...rawTunnel,
    provider: normalizeTunnelProvider(rawTunnel.provider ?? defaults.provider),
    background: rawTunnel.background === undefined ? defaults.background : rawTunnel.background !== false,
    hostname: String(rawTunnel.hostname ?? defaults.hostname).trim(),
    name: String(rawTunnel.name ?? defaults.name).trim(),
    overwriteDns: rawTunnel.overwriteDns === true,
    logLevel: String(rawTunnel.logLevel ?? defaults.logLevel).trim() || defaults.logLevel,
    extraArgs:
      Array.isArray(rawTunnel.extraArgs)
        ? rawTunnel.extraArgs.map((entry) => String(entry).trim()).filter(Boolean)
        : defaults.extraArgs
  };
}

function normalizeProfiles(rawConfig, rootDataDir, defaultCodexHome) {
  const rawProfiles = Array.isArray(rawConfig?.profiles) ? rawConfig.profiles : [];
  const fallbackDefaultProfileId = sanitizeProfileId(rawConfig?.defaultProfileId);
  const profiles = [];
  const seenIds = new Set();

  if (rawProfiles.length > 0) {
    for (const [index, entry] of rawProfiles.entries()) {
      const id = sanitizeProfileId(entry?.id ?? (index === 0 ? fallbackDefaultProfileId : `profile-${index + 1}`));
      if (seenIds.has(id)) {
        continue;
      }
      seenIds.add(id);
      const codexHome = entry?.codexHome ?? entry?.codex_home ?? defaultCodexHome;
      const dataDir = entry?.dataDir ?? entry?.data_dir ?? defaultProfileDataDir(rootDataDir, id);
      profiles.push({
        id,
        label: String(entry?.label ?? "").trim() || (id === fallbackDefaultProfileId ? "Default" : id),
        codexHome: expandHome(String(codexHome)),
        dataDir: expandHome(String(dataDir))
      });
    }
  }

  if (profiles.length === 0) {
    const id = fallbackDefaultProfileId;
    profiles.push({
      id,
      label: "Default",
      codexHome: expandHome(String(rawConfig?.codexHome ?? defaultCodexHome)),
      dataDir: expandHome(defaultProfileDataDir(rootDataDir, id))
    });
  }

  const defaultProfileId = profiles.some((profile) => profile.id === fallbackDefaultProfileId)
    ? fallbackDefaultProfileId
    : profiles[0].id;

  return { defaultProfileId, profiles };
}

function normalizeConfig(rawConfig = {}) {
  const defaults = defaultConfigValues();
  const dataDir = expandHome(String(rawConfig.dataDir ?? defaults.dataDir));
  const codexHome = expandHome(String(rawConfig.codexHome ?? defaults.codexHome));
  const { defaultProfileId, profiles } = normalizeProfiles(rawConfig, dataDir, codexHome);
  const defaultProfile = profiles.find((profile) => profile.id === defaultProfileId) ?? profiles[0];

  return {
    ...defaults,
    ...rawConfig,
    basePath: String(rawConfig.basePath ?? defaults.basePath).startsWith("/")
      ? String(rawConfig.basePath ?? defaults.basePath)
      : `/${String(rawConfig.basePath ?? defaults.basePath)}`,
    codexHome: defaultProfile.codexHome,
    dataDir,
    defaultProfileId,
    profiles,
    hcaptchaSiteKey: String(rawConfig.hcaptchaSiteKey ?? rawConfig.hcaptcha_site_key ?? defaults.hcaptchaSiteKey).trim(),
    hcaptchaSecretKey: String(rawConfig.hcaptchaSecretKey ?? rawConfig.hcaptcha_secret_key ?? defaults.hcaptchaSecretKey).trim(),
    allowedRoots:
      Array.isArray(rawConfig.allowedRoots) && rawConfig.allowedRoots.length > 0
        ? rawConfig.allowedRoots.map((entry) => expandHome(String(entry)))
        : defaults.allowedRoots,
    corsAllowedOrigins: Array.isArray(rawConfig.corsAllowedOrigins) ? rawConfig.corsAllowedOrigins : defaults.corsAllowedOrigins,
    ownerPasswordHash: String(rawConfig.ownerPasswordHash ?? rawConfig.owner_password_hash ?? defaults.ownerPasswordHash).trim(),
    backendBinaryPath: expandHome(String(rawConfig.backendBinaryPath ?? defaults.backendBinaryPath)),
    appServerHandoff: rawConfig.appServerHandoff === undefined ? defaults.appServerHandoff : rawConfig.appServerHandoff !== false,
    perSessionAppServers:
      rawConfig.perSessionAppServers === undefined
        ? defaults.perSessionAppServers
        : rawConfig.perSessionAppServers === true,
    tunnel: normalizeTunnelConfig(rawConfig.tunnel)
  };
}

async function ensureStateDir() {
  await fs.mkdir(stateDir, { recursive: true });
}

async function writeFileAtomic(filePath, content) {
  const directory = path.dirname(filePath);
  await fs.mkdir(directory, { recursive: true });
  const tempPath = path.join(directory, `.codex-webui-${path.basename(filePath)}-${process.pid}-${Date.now()}.tmp`);
  let handle = null;
  try {
    handle = await fs.open(tempPath, "wx");
    await handle.writeFile(content, typeof content === "string" ? "utf8" : undefined);
    await handle.sync();
    await handle.close();
    handle = null;
    await fs.rename(tempPath, filePath);
    try {
      const directoryHandle = await fs.open(directory, "r");
      try {
        await directoryHandle.sync();
      } finally {
        await directoryHandle.close();
      }
    } catch {
      // Directory fsync is not portable across all supported platforms.
    }
  } catch (error) {
    if (handle) {
      await handle.close().catch(() => {});
    }
    await fs.rm(tempPath, { force: true }).catch(() => {});
    throw error;
  }
}

async function readConfig() {
  try {
    const raw = await fs.readFile(configPath, "utf8");
    return normalizeConfig(YAML.parse(raw) ?? {});
  } catch {
    return null;
  }
}

async function writeConfig(config) {
  await writeFileAtomic(configPath, YAML.stringify(config));
}

async function promptConfig(existing = null) {
  const rl = createInterface({
    input: process.stdin,
    output: process.stdout
  });
  const defaults = normalizeConfig(existing ?? {});

  try {
    const host = (await rl.question(`Host [${defaults.host}]: `)).trim() || defaults.host;
    const portInput = (await rl.question(`Port [${defaults.port}]: `)).trim();
    const port = Number.parseInt(portInput || String(defaults.port), 10);
    const basePath = (await rl.question(`Base path [${defaults.basePath || defaultBasePath(port)}]: `)).trim() || defaults.basePath || defaultBasePath(port);
    const codexBin = (await rl.question(`Codex binary [${defaults.codexBin}]: `)).trim() || defaults.codexBin;
    const dataDir = expandHome((await rl.question(`Data dir [${defaults.dataDir}]: `)).trim() || defaults.dataDir);
    const profileCountInput = (await rl.question(`Profile count [${defaults.profiles.length}]: `)).trim();
    const profileCount = Math.max(1, Number.parseInt(profileCountInput || String(defaults.profiles.length), 10) || defaults.profiles.length);
    const profiles = [];

    for (let index = 0; index < profileCount; index += 1) {
      const existingProfile =
        defaults.profiles[index] ??
        {
          id: index === 0 ? defaults.defaultProfileId : `profile-${index + 1}`,
          label: index === 0 ? "Default" : `Profile ${index + 1}`,
          codexHome: defaults.profiles[0]?.codexHome ?? defaults.codexHome,
          dataDir: defaultProfileDataDir(dataDir, index === 0 ? defaults.defaultProfileId : `profile-${index + 1}`)
        };
      const idInput = (await rl.question(`Profile ${index + 1} id [${existingProfile.id}]: `)).trim();
      const id = sanitizeProfileId(idInput || existingProfile.id);
      const label = (await rl.question(`Profile ${index + 1} label [${existingProfile.label}]: `)).trim() || existingProfile.label;
      const codexHome = expandHome((await rl.question(`Profile ${index + 1} Codex home [${existingProfile.codexHome}]: `)).trim() || existingProfile.codexHome);
      const defaultProfileDir = existingProfile.dataDir || defaultProfileDataDir(dataDir, id);
      const profileDataDir = expandHome((await rl.question(`Profile ${index + 1} data dir [${defaultProfileDir}]: `)).trim() || defaultProfileDir);
      profiles.push({
        id,
        label,
        codexHome,
        dataDir: profileDataDir
      });
    }

    const defaultProfileIds = [...new Set(profiles.map((profile) => profile.id))];
    const defaultProfilePrompt = `Default profile id [${defaults.defaultProfileId}] (${defaultProfileIds.join(", ")}): `;
    const defaultProfileInput = (await rl.question(defaultProfilePrompt)).trim();
    const requestedDefaultProfileId = sanitizeProfileId(defaultProfileInput || defaults.defaultProfileId);
    const defaultProfileId = defaultProfileIds.includes(requestedDefaultProfileId) ? requestedDefaultProfileId : defaultProfileIds[0];
    const allowedRootsRaw = (await rl.question(`Allowed roots (comma separated) [${defaults.allowedRoots.join(", ")}]: `)).trim();
    const corsRaw = (await rl.question(`CORS origins (comma separated, optional) [${defaults.corsAllowedOrigins.join(", ")}]: `)).trim();
    const backendBinaryPath = expandHome((await rl.question(`Backend binary path (optional) [${defaults.backendBinaryPath || "auto"}]: `)).trim() || defaults.backendBinaryPath || "");
    const perSessionAppServersInput = (
      await rl.question(
        `Use a separate Codex app-server per active session? [${defaults.perSessionAppServers ? "Y/n" : "y/N"}]: `
      )
    ).trim();
    const tunnelProviderInput = (await rl.question(`Tunnel provider [${defaults.tunnel.provider}]: `)).trim();
    const tunnelBackgroundInput = (await rl.question(`Tunnel runs in background by default? [${defaults.tunnel.background ? "Y/n" : "y/N"}]: `)).trim();
    const tunnelHostnameInput = (await rl.question(`Tunnel hostname (cloudflared only, optional) [${formatOptionalPrompt(defaults.tunnel.hostname)}]: `)).trim();
    const tunnelNameInput = (await rl.question(`Tunnel name (cloudflared only, optional) [${formatOptionalPrompt(defaults.tunnel.name)}]: `)).trim();
    const password = await rl.question("Password (leave blank to keep existing hash): ");
    const passwordHash = password.trim() ? hashPassword(password.trim()) : defaults.passwordHash;
    const ownerPassword = await rl.question("Owner password for terminal/runtime/shutdown actions (optional, leave blank to keep existing): ");
    const ownerPasswordHash = ownerPassword.trim() ? hashPassword(ownerPassword.trim()) : defaults.ownerPasswordHash;
    const hcaptchaSiteKeyInput = (await rl.question(`hCaptcha site key (optional) [${formatOptionalPrompt(defaults.hcaptchaSiteKey)}]: `)).trim();
    const hcaptchaSecretKey = (
      await rl.question(
        `hCaptcha secret key (optional) [${defaults.hcaptchaSecretKey ? "configured" : "none"}]: `
      )
    ).trim() || defaults.hcaptchaSecretKey;

    if (!passwordHash) {
      throw new Error("A password is required.");
    }

    const defaultProfile = profiles.find((profile) => profile.id === defaultProfileId) ?? profiles[0];
    const config = normalizeConfig({
      host,
      port: Number.isFinite(port) ? port : defaults.port,
      basePath: basePath.startsWith("/") ? basePath : `/${basePath}`,
      codexBin,
      dataDir,
      codexHome: defaultProfile.codexHome,
      defaultProfileId,
      profiles,
      allowedRoots:
        allowedRootsRaw
          ? allowedRootsRaw.split(",").map((entry) => expandHome(entry.trim())).filter(Boolean)
          : defaults.allowedRoots,
      corsAllowedOrigins:
        corsRaw
          ? corsRaw.split(",").map((entry) => entry.trim()).filter(Boolean)
          : defaults.corsAllowedOrigins,
      passwordHash,
      ownerPasswordHash,
      hcaptchaSiteKey: hcaptchaSiteKeyInput || defaults.hcaptchaSiteKey,
      hcaptchaSecretKey,
      sessionSecret: defaults.sessionSecret || createSessionSecret(),
      backendBinaryPath,
      perSessionAppServers: parseBooleanInput(perSessionAppServersInput, defaults.perSessionAppServers),
      tunnel: {
        ...defaults.tunnel,
        provider: normalizeTunnelProvider(tunnelProviderInput || defaults.tunnel.provider),
        background: parseBooleanInput(tunnelBackgroundInput, defaults.tunnel.background),
        hostname: tunnelHostnameInput || defaults.tunnel.hostname,
        name: tunnelNameInput || defaults.tunnel.name
      }
    });

    await writeConfig(config);
    return config;
  } finally {
    rl.close();
  }
}

function buildUrl(config) {
  const basePath = config.basePath === "/" ? "" : config.basePath;
  return `http://${config.host}:${config.port}${basePath}/`;
}

function buildBaseUrl(config) {
  const basePath = config.basePath === "/" ? "" : config.basePath;
  return `http://${config.host}:${config.port}${basePath}`;
}

async function readPid() {
  try {
    return Number.parseInt((await fs.readFile(pidPath, "utf8")).trim(), 10) || null;
  } catch {
    return null;
  }
}

async function readServerMeta() {
  try {
    return JSON.parse(await fs.readFile(serverMetaPath, "utf8"));
  } catch {
    return null;
  }
}

async function writeServerMeta(meta) {
  await writeFileAtomic(serverMetaPath, JSON.stringify(meta, null, 2));
}

async function clearServerStateFiles() {
  await Promise.all([
    fs.rm(pidPath, { force: true }),
    fs.rm(serverMetaPath, { force: true })
  ]);
}

async function readNumericFile(filePath) {
  try {
    return Number.parseInt((await fs.readFile(filePath, "utf8")).trim(), 10) || null;
  } catch {
    return null;
  }
}

async function writeNumericFile(filePath, value) {
  await writeFileAtomic(filePath, String(value));
}

async function readTunnelMeta() {
  try {
    return JSON.parse(await fs.readFile(tunnelMetaPath, "utf8"));
  } catch {
    return null;
  }
}

async function writeTunnelMeta(meta) {
  await writeFileAtomic(tunnelMetaPath, JSON.stringify(meta, null, 2));
}

function isRunning(pid) {
  if (!pid) {
    return false;
  }
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

async function verifyServerInstance(config, meta) {
  const token = String(meta?.instanceToken ?? "").trim();
  if (!token) {
    return false;
  }
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 1500);
  try {
    const response = await fetch(`${buildBaseUrl(config)}/healthz`, {
      credentials: "include",
      headers: {
        "x-codex-webui-instance-token": token
      },
      signal: controller.signal
    });
    if (!response.ok) {
      return false;
    }
    const payload = await response.json();
    return payload?.instanceTokenMatched === true;
  } catch {
    return false;
  } finally {
    clearTimeout(timeout);
  }
}

async function prepareRestartHandoff(config, meta) {
  const token = String(meta?.instanceToken ?? "").trim();
  if (!token) {
    return { prepared: false, error: "Missing instance token." };
  }
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 1500);
  try {
    const response = await fetch(`${buildBaseUrl(config)}/api/admin/restart-handoff/prepare`, {
      method: "POST",
      credentials: "include",
      headers: {
        "x-codex-webui-instance-token": token
      },
      signal: controller.signal
    });
    let payload = null;
    try {
      payload = await response.json();
    } catch {
      payload = null;
    }
    if (!response.ok) {
      return { prepared: false, error: payload?.error ?? payload?.message ?? `HTTP ${response.status}` };
    }
    return {
      prepared: payload?.handoffPrepared !== false || payload?.activeAppServerProcesses === 0,
      error: null
    };
  } catch (error) {
    return { prepared: false, error: error instanceof Error ? error.message : String(error) };
  } finally {
    clearTimeout(timeout);
  }
}

async function readServerStatus(config) {
  const meta = await readServerMeta();
  const rawPid = meta?.pid ?? (await readPid());
  const pid = Number.parseInt(String(rawPid ?? ""), 10) || null;
  const running = isRunning(pid);
  const verified = running && meta ? await verifyServerInstance(config, meta) : false;
  if (pid && !running) {
    await clearServerStateFiles();
  }
  return {
    pid,
    running,
    verified,
    meta
  };
}

function buildOriginUrl(config) {
  return `http://${config.host}:${config.port}`;
}

function joinPublicUrl(baseUrl, routePath) {
  const root = String(baseUrl).replace(/\/+$/u, "");
  const normalizedPath = routePath === "/" ? "/" : `/${String(routePath).replace(/^\/+/u, "")}`;
  return `${root}${normalizedPath}`;
}

function buildPublicWorkspaceUrl(baseUrl, config) {
  return joinPublicUrl(baseUrl, config.basePath || "/");
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function waitForProcessExit(pid, timeoutMs = 10000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (!isRunning(pid)) {
      return true;
    }
    await sleep(100);
  }
  return !isRunning(pid);
}

function extractTunnelPublicUrl(logText) {
  if (!logText?.trim()) {
    return null;
  }

  const matches = [...logText.matchAll(/https:\/\/[^\s"'`]+/gu)]
    .map((match) => match[0].replace(/[),.;]+$/u, ""))
    .filter((value) => !/https:\/\/(?:127\.0\.0\.1|localhost)(?::\d+)?/u.test(value));

  if (matches.length === 0) {
    return null;
  }

  const preferred = matches.find((value) => /trycloudflare\.com|ngrok/i.test(value));
  return preferred ?? matches[0];
}

async function readTunnelPublicUrlFromLog() {
  try {
    return extractTunnelPublicUrl(await fs.readFile(tunnelLogPath, "utf8"));
  } catch {
    return null;
  }
}

function resolveTunnelProvider(requestedProvider) {
  const provider = normalizeTunnelProvider(requestedProvider);
  if (provider === "cloudflared") {
    if (!commandAvailable("cloudflared")) {
      throw new Error("cloudflared is not installed.");
    }
    return "cloudflared";
  }
  if (provider === "ngrok") {
    if (!commandAvailable("ngrok")) {
      throw new Error("ngrok is not installed.");
    }
    return "ngrok";
  }
  if (commandAvailable("cloudflared")) {
    return "cloudflared";
  }
  if (commandAvailable("ngrok")) {
    return "ngrok";
  }
  throw new Error("Neither cloudflared nor ngrok is installed.");
}

function buildTunnelLaunch(config, tunnelOptions) {
  const originUrl = buildOriginUrl(config);
  const provider = resolveTunnelProvider(tunnelOptions.provider);

  if (provider === "cloudflared") {
    const args = ["tunnel", "--url", originUrl, "--no-autoupdate", "--loglevel", tunnelOptions.logLevel];
    if (tunnelOptions.hostname) {
      args.push("--hostname", tunnelOptions.hostname);
    }
    if (tunnelOptions.name) {
      args.push("--name", tunnelOptions.name);
    }
    if (tunnelOptions.overwriteDns) {
      args.push("--overwrite-dns");
    }
    args.push(...tunnelOptions.extraArgs);
    return {
      provider,
      command: "cloudflared",
      args,
      originUrl
    };
  }

  if (tunnelOptions.hostname || tunnelOptions.name || tunnelOptions.overwriteDns) {
    throw new Error("The selected tunnel provider does not support --hostname, --name, or --overwrite-dns. Use cloudflared for those options.");
  }

  const args = ["http", originUrl, ...tunnelOptions.extraArgs];
  return {
    provider,
    command: "ngrok",
    args,
    originUrl
  };
}

function isBroadAllowedRoot(rootPath) {
  const resolved = path.resolve(expandHome(String(rootPath ?? "")));
  const home = path.resolve(os.homedir());
  const parsed = path.parse(resolved);
  return resolved === parsed.root || resolved === home || home.startsWith(`${resolved}${path.sep}`);
}

function tunnelSafetyFindings(config) {
  const findings = [];
  if (!String(config.passwordHash ?? "").trim()) {
    findings.push("Password hash is not configured.");
  }
  if (!String(config.sessionSecret ?? "").trim() || String(config.sessionSecret ?? "").trim().length < 32) {
    findings.push("Session secret is missing or short.");
  }
  if (!String(config.hcaptchaSiteKey ?? "").trim() || !String(config.hcaptchaSecretKey ?? "").trim()) {
    findings.push("hCaptcha is not configured for the public login surface.");
  }
  const broadRoot = (config.allowedRoots ?? []).find(isBroadAllowedRoot);
  if (broadRoot) {
    findings.push(`Allowed root is broad: ${broadRoot}`);
  }
  if ((config.corsAllowedOrigins ?? []).some((origin) => String(origin).trim() === "*")) {
    findings.push("CORS allows every origin.");
  }
  return findings;
}

function tunnelBlockingFindings(config) {
  const findings = [];
  if (!String(config.ownerPasswordHash ?? "").trim()) {
    findings.push("Owner password hash is required before starting a public tunnel.");
  }
  return findings;
}

function printTunnelSafetyChecklist(config, launch, findings, blockingFindings = []) {
  console.log("Tunnel safety checklist:");
  console.log(`  Provider: ${launch.provider}`);
  console.log(`  Local origin: ${launch.originUrl}`);
  console.log(`  Public route: ${config.basePath || "/"}`);
  console.log(`  Allowed roots: ${(config.allowedRoots ?? []).join(", ") || "(none)"}`);
  console.log(`  Owner password: ${String(config.ownerPasswordHash ?? "").trim() ? "configured" : "not configured"}`);
  console.log(`  hCaptcha: ${String(config.hcaptchaSiteKey ?? "").trim() && String(config.hcaptchaSecretKey ?? "").trim() ? "configured" : "not configured"}`);
  if (blockingFindings.length > 0) {
    console.log("");
    console.log("Blocking issues:");
    for (const finding of blockingFindings) {
      console.log(`  - ${finding}`);
    }
  }
  if (findings.length > 0) {
    console.log("");
    console.log("Warnings:");
    for (const finding of findings) {
      console.log(`  - ${finding}`);
    }
  }
  console.log("");
  console.log("A tunnel exposes Codex chat control, Git operations, file tools, and host terminals to the public internet.");
}

async function confirmTunnelSafety(config, options, launch) {
  const findings = tunnelSafetyFindings(config);
  const blockingFindings = tunnelBlockingFindings(config);
  if (blockingFindings.length > 0) {
    if (!options.json) {
      printTunnelSafetyChecklist(config, launch, findings, blockingFindings);
    }
    throw new Error(`Refusing to start a public tunnel. ${blockingFindings.join(" ")}`);
  }

  if (options.yes || process.env.CODEX_WEBUI_TUNNEL_ASSUME_YES === "true") {
    return;
  }

  printTunnelSafetyChecklist(config, launch, findings);

  if (!process.stdin.isTTY || options.json) {
    throw new Error("Refusing to start a public tunnel without explicit confirmation. Re-run with --yes after reviewing the tunnel safety checklist.");
  }

  const rl = createInterface({
    input: process.stdin,
    output: process.stdout
  });
  try {
    const answer = (await rl.question('Type "expose" to start the tunnel: ')).trim().toLowerCase();
    if (answer !== "expose") {
      throw new Error("Tunnel start cancelled.");
    }
  } finally {
    rl.close();
  }
}

async function readTunnelStatus(config) {
  const pid = await readNumericFile(tunnelPidPath);
  const meta = await readTunnelMeta();
  const running = isRunning(pid);

  if (!running) {
    await fs.rm(tunnelPidPath, { force: true });
  }

  const publicUrl = meta?.publicUrl ?? (running ? await readTunnelPublicUrlFromLog() : null);
  const workspaceUrl = publicUrl ? buildPublicWorkspaceUrl(publicUrl, config) : null;

  if (running && meta && publicUrl && meta.publicUrl !== publicUrl) {
    const nextMeta = {
      ...meta,
      publicUrl,
      publicWorkspaceUrl: workspaceUrl
    };
    await writeTunnelMeta(nextMeta);
    return {
      running,
      pid,
      meta: nextMeta,
      publicUrl,
      workspaceUrl,
      logPath: tunnelLogPath
    };
  }

  return {
    running,
    pid,
    meta,
    publicUrl,
    workspaceUrl,
    logPath: tunnelLogPath
  };
}

async function printTunnelStatus(config, jsonOutput = false) {
  const status = await readTunnelStatus(config);
  if (jsonOutput) {
    console.log(JSON.stringify(status, null, 2));
    return;
  }

  if (!status.running) {
    console.log("No active tunnel.");
    console.log(`Log: ${status.logPath}`);
    return;
  }

  console.log("Tunnel is running.");
  console.log(`Provider: ${status.meta?.provider ?? "unknown"}`);
  console.log(`PID: ${status.pid}`);
  console.log(`Origin: ${status.meta?.originUrl ?? buildOriginUrl(config)}`);
  console.log(`Workspace: ${status.workspaceUrl ?? "waiting for public URL..."}`);
  if (status.publicUrl) {
    console.log(`Public base: ${status.publicUrl}`);
  }
  console.log(`Log: ${status.logPath}`);
}

async function stopTunnel() {
  const pid = await readNumericFile(tunnelPidPath);
  if (!pid || !isRunning(pid)) {
    await fs.rm(tunnelPidPath, { force: true });
    await fs.rm(tunnelMetaPath, { force: true });
    return { stopped: false };
  }

  process.kill(pid, "SIGTERM");
  await fs.rm(tunnelPidPath, { force: true });
  await fs.rm(tunnelMetaPath, { force: true });
  return { stopped: true, pid };
}

function parseTunnelArgs(argv) {
  const options = {
    action: "start",
    provider: null,
    foreground: null,
    hostname: null,
    name: null,
    overwriteDns: null,
    logLevel: null,
    extraArgs: [],
    json: false,
    yes: false,
    lines: 80,
    help: false
  };

  let index = 0;
  const firstToken = argv[0];
  if (["start", "status", "stop", "logs"].includes(firstToken)) {
    options.action = firstToken;
    index = 1;
  }

  while (index < argv.length) {
    const token = argv[index];
    if (token === "--help" || token === "-h") {
      options.help = true;
      index += 1;
      continue;
    }
    if (token === "--json") {
      options.json = true;
      index += 1;
      continue;
    }
    if (token === "--yes" || token === "-y") {
      options.yes = true;
      index += 1;
      continue;
    }
    if (token === "--foreground") {
      options.foreground = true;
      index += 1;
      continue;
    }
    if (token === "--background") {
      options.foreground = false;
      index += 1;
      continue;
    }
    if (token === "--overwrite-dns") {
      options.overwriteDns = true;
      index += 1;
      continue;
    }
    if (token === "--provider" || token === "--hostname" || token === "--name" || token === "--log-level" || token === "--arg" || token === "--lines") {
      const nextValue = argv[index + 1];
      if (!nextValue) {
        throw new Error(`Missing value for ${token}.`);
      }
      if (token === "--provider") {
        options.provider = nextValue;
      } else if (token === "--hostname") {
        options.hostname = nextValue;
      } else if (token === "--name") {
        options.name = nextValue;
      } else if (token === "--log-level") {
        options.logLevel = nextValue;
      } else if (token === "--arg") {
        options.extraArgs.push(nextValue);
      } else if (token === "--lines") {
        options.lines = Number.parseInt(nextValue, 10) || options.lines;
      }
      index += 2;
      continue;
    }
    throw new Error(`Unknown tunnel option: ${token}`);
  }

  return options;
}

function mergeTunnelOptions(config, cliOptions) {
  return normalizeTunnelConfig({
    ...config.tunnel,
    provider: cliOptions.provider ?? config.tunnel.provider,
    background: cliOptions.foreground === null ? config.tunnel.background : cliOptions.foreground === false ? true : false,
    hostname: cliOptions.hostname ?? config.tunnel.hostname,
    name: cliOptions.name ?? config.tunnel.name,
    overwriteDns: cliOptions.overwriteDns ?? config.tunnel.overwriteDns,
    logLevel: cliOptions.logLevel ?? config.tunnel.logLevel,
    extraArgs: [...config.tunnel.extraArgs, ...cliOptions.extraArgs]
  });
}

async function waitForTunnelPublicUrl(pid, timeoutMs = 10000) {
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    if (!isRunning(pid)) {
      return null;
    }
    const publicUrl = await readTunnelPublicUrlFromLog();
    if (publicUrl) {
      return publicUrl;
    }
    await sleep(350);
  }
  return null;
}

async function startTunnel(config, cliOptions, preparedLaunch = null) {
  await ensureStateDir();
  const existing = await readTunnelStatus(config);
  if (existing.running) {
    return existing;
  }

  const tunnelOptions = mergeTunnelOptions(config, cliOptions);
  const launch = preparedLaunch ?? buildTunnelLaunch(config, tunnelOptions);
  if (!preparedLaunch) {
    await confirmTunnelSafety(config, cliOptions, launch);
  }
  await fs.mkdir(path.dirname(tunnelLogPath), { recursive: true });
  await writeFileAtomic(tunnelLogPath, "");

  if (tunnelOptions.background) {
    const logHandle = await fs.open(tunnelLogPath, "a");
    const child = spawn(launch.command, launch.args, {
      cwd: packageRoot,
      detached: true,
      stdio: ["ignore", logHandle.fd, logHandle.fd]
    });
    child.unref();
    await logHandle.close();
    await writeNumericFile(tunnelPidPath, child.pid);
    const meta = {
      provider: launch.provider,
      pid: child.pid,
      command: launch.command,
      args: launch.args,
      startedAt: new Date().toISOString(),
      originUrl: launch.originUrl,
      localWorkspaceUrl: buildBaseUrl(config),
      publicUrl: null,
      publicWorkspaceUrl: null,
      background: true
    };
    await writeTunnelMeta(meta);
    await sleep(700);
    if (!isRunning(child.pid)) {
      throw new Error(`The ${launch.provider} tunnel exited early. Check ${tunnelLogPath}.`);
    }
    const publicUrl = await waitForTunnelPublicUrl(child.pid);
    if (publicUrl) {
      await writeTunnelMeta({
        ...meta,
        publicUrl,
        publicWorkspaceUrl: buildPublicWorkspaceUrl(publicUrl, config)
      });
    }
    return readTunnelStatus(config);
  }

  const child = spawn(launch.command, launch.args, {
    cwd: packageRoot,
    stdio: "inherit"
  });
  await writeNumericFile(tunnelPidPath, child.pid);
  await writeTunnelMeta({
    provider: launch.provider,
    pid: child.pid,
    command: launch.command,
    args: launch.args,
    startedAt: new Date().toISOString(),
    originUrl: launch.originUrl,
    localWorkspaceUrl: buildBaseUrl(config),
    publicUrl: null,
    publicWorkspaceUrl: null,
    background: false
  });

  await new Promise((resolve, reject) => {
    child.once("error", reject);
    child.once("exit", async (code) => {
      await fs.rm(tunnelPidPath, { force: true });
      await fs.rm(tunnelMetaPath, { force: true });
      if ((code ?? 0) !== 0) {
        reject(new Error(`Tunnel exited with code ${code ?? 0}.`));
        return;
      }
      resolve(null);
    });
  });

  return null;
}

async function printTunnelLogs(lines, jsonOutput = false) {
  try {
    const logText = await fs.readFile(tunnelLogPath, "utf8");
    const tail = logText.split(/\r?\n/u).filter(Boolean).slice(-Math.max(1, lines));
    if (jsonOutput) {
      console.log(JSON.stringify({ logPath: tunnelLogPath, lines: tail }, null, 2));
      return;
    }
    console.log(`Log: ${tunnelLogPath}`);
    if (tail.length === 0) {
      console.log("(log file is empty)");
      return;
    }
    console.log(tail.join("\n"));
  } catch {
    if (jsonOutput) {
      console.log(JSON.stringify({ logPath: tunnelLogPath, lines: [], exists: false }, null, 2));
      return;
    }
    console.log(`No tunnel log found at ${tunnelLogPath}.`);
  }
}

async function resolveBackendBinary(config) {
  const candidates = [
    config.backendBinaryPath,
    process.env.CODEX_WEBUI_BACKEND_BIN,
    ...currentRustTargets().map((target) =>
      path.join(packageRoot, "dist", "backend", target, process.platform === "win32" ? "backend.exe" : "backend")
    ),
    path.join(packageRoot, "backend", "target", "release", process.platform === "win32" ? "backend.exe" : "backend"),
    path.join(packageRoot, "backend", "target", "debug", process.platform === "win32" ? "backend.exe" : "backend")
  ].filter(Boolean);

  for (const candidate of candidates) {
    const resolved = expandHome(candidate);
    try {
      await fs.access(resolved);
      return resolved;
    } catch {
      // try next
    }
  }

  throw new Error("Could not find a runnable backend binary. Set backendBinaryPath in ~/.codex/codex-webui.yml or build the Rust backend first.");
}

async function startServer(config) {
  await ensureStateDir();
  const current = await readServerStatus(config);
  if (current.verified) {
    return { pid: current.pid, alreadyRunning: true };
  }
  if (current.running) {
    throw new Error(
      `Refusing to reuse PID ${current.pid}: the process is running but does not match this codex-webui instance. Remove ${pidPath} and ${serverMetaPath} only if you have verified it is stale.`
    );
  }

  const backendBinary = await resolveBackendBinary(config);
  const logHandle = await fs.open(logPath, "a");
  const defaultProfile = config.profiles.find((profile) => profile.id === config.defaultProfileId) ?? config.profiles[0];
  const instanceToken = createSessionSecret();
  const child = spawn(backendBinary, {
    cwd: packageRoot,
    detached: true,
    stdio: ["ignore", logHandle.fd, logHandle.fd],
    env: {
      ...process.env,
      HOST: String(config.host),
      PORT: String(config.port),
      CODEX_WEBUI_BASE_PATH: String(config.basePath),
      CODEX_WEBUI_CODEX_BIN: String(config.codexBin),
      CODEX_HOME: String(defaultProfile.codexHome),
      CODEX_WEBUI_DATA_DIR: String(config.dataDir),
      CODEX_WEBUI_DEFAULT_PROFILE_ID: String(config.defaultProfileId),
      CODEX_WEBUI_PROFILES_JSON: JSON.stringify(
        config.profiles.map((profile) => ({
          id: profile.id,
          label: profile.label,
          codexHome: profile.codexHome,
          dataDir: profile.dataDir
        }))
      ),
      CODEX_WEBUI_ALLOWED_ROOTS: config.allowedRoots.join(path.delimiter),
      CODEX_WEBUI_PASSWORD_HASH: String(config.passwordHash),
      CODEX_WEBUI_OWNER_PASSWORD_HASH: String(config.ownerPasswordHash ?? ""),
      CODEX_WEBUI_HCAPTCHA_SITE_KEY: String(config.hcaptchaSiteKey ?? ""),
      CODEX_WEBUI_HCAPTCHA_SECRET_KEY: String(config.hcaptchaSecretKey ?? ""),
      CODEX_WEBUI_SESSION_SECRET: String(config.sessionSecret),
      CODEX_WEBUI_INSTANCE_TOKEN: instanceToken,
      CODEX_WEBUI_APP_SERVER_HANDOFF: config.appServerHandoff === false ? "false" : "true",
      CODEX_WEBUI_PER_SESSION_APP_SERVERS: config.perSessionAppServers ? "true" : "false",
      CODEX_WEBUI_CORS_ALLOWED_ORIGINS: config.corsAllowedOrigins.join(",")
    }
  });
  child.unref();
  await writeFileAtomic(pidPath, String(child.pid));
  await writeServerMeta({
    pid: child.pid,
    instanceToken,
    startedAt: new Date().toISOString(),
    command: backendBinary,
    cwd: packageRoot,
    url: buildUrl(config)
  });
  await logHandle.close();
  return { pid: child.pid, alreadyRunning: false };
}

async function stopServer(config) {
  const status = await readServerStatus(config);
  if (!status.pid || !status.running) {
    await clearServerStateFiles();
    return { stopped: false };
  }
  if (!status.verified) {
    return { stopped: false, unsafe: true, pid: status.pid };
  }
  process.kill(status.pid, "SIGTERM");
  const exited = await waitForProcessExit(status.pid, 10000);
  if (!exited) {
    throw new Error(`Timed out waiting for codex-webui PID ${status.pid} to stop.`);
  }
  await clearServerStateFiles();
  return { stopped: true, pid: status.pid };
}

async function restartServer(config) {
  const status = await readServerStatus(config);
  if (status.pid && status.running && !status.verified) {
    throw new Error(`Refusing to restart: PID ${status.pid} could not be verified as this codex-webui instance.`);
  }
  const handoff = status.verified ? await prepareRestartHandoff(config, status.meta) : { prepared: false, error: "No verified running gateway." };
  if (status.verified && !handoff.prepared) {
    throw new Error(`Refusing to restart without Codex app-server handoff: ${handoff.error ?? "handoff was not prepared"}`);
  }
  const stopResult = await stopServer(config);
  if (stopResult.unsafe) {
    throw new Error(`Refusing to restart: PID ${stopResult.pid} could not be verified as this codex-webui instance.`);
  }
  const started = await startServer(config);
  return { ...started, handoffPrepared: handoff.prepared };
}

function printTunnelUsage() {
  console.log("Tunnel commands:");
  console.log("  codex-webui tunnel start [--provider auto|cloudflared|ngrok] [--foreground] [--hostname host] [--name tunnel] [--overwrite-dns] [--log-level level] [--arg value] [--yes] [--json]");
  console.log("  codex-webui tunnel status [--json]");
  console.log("  codex-webui tunnel stop");
  console.log("  codex-webui tunnel logs [--lines 80] [--json]");
}

async function runTunnel(config, argv) {
  const options = parseTunnelArgs(argv);
  if (options.help) {
    printTunnelUsage();
    return;
  }

  if (options.action === "status") {
    await printTunnelStatus(config, options.json);
    return;
  }

  if (options.action === "stop") {
    const result = await stopTunnel();
    console.log(result.stopped ? `Stopped tunnel (pid ${result.pid}).` : "No tunnel is running.");
    return;
  }

  if (options.action === "logs") {
    await printTunnelLogs(options.lines, options.json);
    return;
  }

  const existingTunnel = await readTunnelStatus(config);
  let launch = null;
  if (!existingTunnel.running) {
    const tunnelOptions = mergeTunnelOptions(config, options);
    launch = buildTunnelLaunch(config, tunnelOptions);
    await confirmTunnelSafety(config, options, launch);
  }

  const server = await startServer(config);
  console.log(`Server running at ${buildUrl(config)} (pid ${server.pid})`);
  const status = await startTunnel(config, options, launch);
  if (status) {
    if (options.json) {
      console.log(JSON.stringify(status, null, 2));
      return;
    }
    console.log(`Tunnel provider: ${status.meta?.provider ?? "unknown"}`);
    console.log(`Tunnel PID: ${status.pid}`);
    if (status.workspaceUrl) {
      console.log(`Public workspace: ${status.workspaceUrl}`);
    } else {
      console.log(`Public workspace: waiting for public URL, check ${status.logPath}`);
    }
    console.log(`Tunnel log: ${status.logPath}`);
    console.log("Use `codex-webui tunnel status` or `codex-webui tunnel stop` to manage it.");
  }
}

function printUsage(config, pid, alreadyRunning) {
  console.log(alreadyRunning ? "codex-webui is already running." : "codex-webui started in the background.");
  console.log(`URL: ${buildUrl(config)}`);
  console.log(`PID: ${pid}`);
  console.log(`Config: ${configPath}`);
  console.log(`Log: ${logPath}`);
  console.log(`Errors: ${runtimeErrorLogPath(config)}`);
  console.log("");
  printCommandHelp();
}

function printCommandHelp() {
  console.log("Commands:");
  console.log("  codex-webui config   Re-run the interactive setup");
  console.log("  codex-webui status   Show the background server state");
  console.log("  codex-webui restart  Restart the background server with Codex session handoff when available");
  console.log("  codex-webui stop     Stop the background server");
  console.log("  codex-webui tunnel start   Start a cloudflared/ngrok tunnel");
  console.log("  codex-webui tunnel status  Show the current tunnel state");
  console.log("  codex-webui tunnel stop    Stop the current tunnel");
  console.log("  codex-webui tunnel logs    Print recent tunnel logs");
  console.log("");
  console.log("Options:");
  console.log("  --hcaptcha-site-key <key>      Enable hCaptcha on the login screen with this site key");
  console.log("  --hcaptcha-secret-key <secret> Enable hCaptcha verification with this secret");
  console.log("  --disable-hcaptcha             Disable hCaptcha even if it is configured");
  console.log("  codex-webui tunnel start --yes Skip the public-exposure confirmation after reviewing the checklist");
}

function readOptionValue(argv, index, flagName) {
  const current = argv[index] ?? "";
  const equalsIndex = current.indexOf("=");
  if (equalsIndex >= 0) {
    return {
      value: current.slice(equalsIndex + 1),
      nextIndex: index
    };
  }

  const next = argv[index + 1];
  if (next === undefined) {
    throw new Error(`Missing value for ${flagName}.`);
  }

  return {
    value: next,
    nextIndex: index + 1
  };
}

function configInputFromCliOverrides(overrides) {
  const next = {};
  if (overrides.disableHcaptcha) {
    next.hcaptchaSiteKey = "";
    next.hcaptchaSecretKey = "";
  }
  if (overrides.hcaptchaSiteKey !== undefined) {
    next.hcaptchaSiteKey = String(overrides.hcaptchaSiteKey).trim();
  }
  if (overrides.hcaptchaSecretKey !== undefined) {
    next.hcaptchaSecretKey = String(overrides.hcaptchaSecretKey).trim();
  }
  return next;
}

function applyCliConfigOverrides(config, overrides) {
  return normalizeConfig({
    ...(config ?? {}),
    ...configInputFromCliOverrides(overrides)
  });
}

function parseCliInvocation(argv) {
  const knownCommands = new Set(["config", "status", "restart", "stop", "tunnel"]);
  let command = "";
  let restArgs = [...argv];
  if (knownCommands.has(restArgs[0] ?? "")) {
    command = restArgs[0];
    restArgs = restArgs.slice(1);
  }

  const overrides = {};
  const remainingArgs = [];

  for (let index = 0; index < restArgs.length; index += 1) {
    const argument = restArgs[index];
    if (command !== "tunnel" && (argument === "--help" || argument === "-h")) {
      return { command, restArgs: [], overrides, help: true };
    }
    if (command === "tunnel" && (argument === "--help" || argument === "-h")) {
      remainingArgs.push(argument);
      continue;
    }
    if (argument === "--disable-hcaptcha") {
      overrides.disableHcaptcha = true;
      continue;
    }
    if (argument === "--hcaptcha-site-key" || argument.startsWith("--hcaptcha-site-key=")) {
      const parsed = readOptionValue(restArgs, index, "--hcaptcha-site-key");
      overrides.hcaptchaSiteKey = parsed.value;
      index = parsed.nextIndex;
      continue;
    }
    if (argument === "--hcaptcha-secret-key" || argument.startsWith("--hcaptcha-secret-key=")) {
      const parsed = readOptionValue(restArgs, index, "--hcaptcha-secret-key");
      overrides.hcaptchaSecretKey = parsed.value;
      index = parsed.nextIndex;
      continue;
    }
    remainingArgs.push(argument);
  }

  if (command !== "tunnel" && remainingArgs.length > 0) {
    throw new Error(`Unknown option: ${remainingArgs[0]}`);
  }

  return { command, restArgs: remainingArgs, overrides, help: false };
}

async function main() {
  const { command, restArgs, overrides, help } = parseCliInvocation(process.argv.slice(2));
  let config = await readConfig();

  if (help) {
    printCommandHelp();
    return;
  }
  if (command === "tunnel" && parseTunnelArgs(restArgs).help) {
    printTunnelUsage();
    return;
  }

  if (config) {
    config = applyCliConfigOverrides(config, overrides);
  }

  if (!config || command === "config") {
    config = await promptConfig(config ? config : applyCliConfigOverrides(null, overrides));
    console.log(`Saved configuration to ${configPath}`);
    if (command === "config") {
      return;
    }
  }

  if (command === "status") {
    const status = await readServerStatus(config);
    if (!status.pid || !status.running) {
      console.log("codex-webui is not running.");
      return;
    }
    console.log(status.verified ? "codex-webui is running." : "A process is running at the recorded PID, but it could not be verified.");
    console.log(`PID: ${status.pid}`);
    console.log(`URL: ${buildUrl(config)}`);
    console.log(`Config: ${configPath}`);
    if (!status.verified) {
      console.log(`Refusing to manage this PID until ${serverMetaPath} and /healthz verification match.`);
    }
    return;
  }

  if (command === "stop") {
    const result = await stopServer(config);
    if (result.unsafe) {
      console.log(`Refusing to stop PID ${result.pid}: it could not be verified as this codex-webui instance.`);
      return;
    }
    console.log(result.stopped ? `Stopped codex-webui (pid ${result.pid}).` : "codex-webui is not running.");
    return;
  }

  if (command === "restart") {
    const result = await restartServer(config);
    if (result.handoffPrepared === false) {
      console.log("Restart handoff was not prepared; active Codex app-server sessions may restart.");
    }
    printUsage(config, result.pid, false);
    return;
  }

  if (command === "tunnel") {
    await runTunnel(config, restArgs);
    return;
  }

  const result = await startServer(config);
  printUsage(config, result.pid, result.alreadyRunning);
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
});
