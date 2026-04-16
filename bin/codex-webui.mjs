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
const logPath = path.join(stateDir, "server.log");

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

function defaultConfigValues() {
  const port = 4173;
  return {
    host: "127.0.0.1",
    port,
    basePath: defaultBasePath(port),
    codexBin: "codex",
    codexHome: path.join(os.homedir(), ".codex"),
    dataDir: path.join(os.homedir(), ".codex", "codex-webui", "data"),
    allowedRoots: [process.cwd()],
    passwordHash: "",
    sessionSecret: createSessionSecret(),
    corsAllowedOrigins: [],
    backendBinaryPath: ""
  };
}

async function ensureStateDir() {
  await fs.mkdir(stateDir, { recursive: true });
}

async function readConfig() {
  try {
    const raw = await fs.readFile(configPath, "utf8");
    const parsed = YAML.parse(raw) ?? {};
    const defaults = defaultConfigValues();
    return {
      ...defaults,
      ...parsed,
      allowedRoots: Array.isArray(parsed.allowedRoots) && parsed.allowedRoots.length > 0 ? parsed.allowedRoots.map(expandHome) : defaults.allowedRoots,
      corsAllowedOrigins: Array.isArray(parsed.corsAllowedOrigins) ? parsed.corsAllowedOrigins : defaults.corsAllowedOrigins
    };
  } catch {
    return null;
  }
}

async function writeConfig(config) {
  await fs.mkdir(path.dirname(configPath), { recursive: true });
  await fs.writeFile(configPath, YAML.stringify(config), "utf8");
}

async function promptConfig(existing = null) {
  const rl = createInterface({
    input: process.stdin,
    output: process.stdout
  });
  const defaults = {
    ...defaultConfigValues(),
    ...(existing ?? {})
  };

  try {
    const host = (await rl.question(`Host [${defaults.host}]: `)).trim() || defaults.host;
    const portInput = (await rl.question(`Port [${defaults.port}]: `)).trim();
    const port = Number.parseInt(portInput || String(defaults.port), 10);
    const basePath = (await rl.question(`Base path [${defaults.basePath || defaultBasePath(port)}]: `)).trim() || defaults.basePath || defaultBasePath(port);
    const codexBin = (await rl.question(`Codex binary [${defaults.codexBin}]: `)).trim() || defaults.codexBin;
    const codexHome = expandHome((await rl.question(`Codex home [${defaults.codexHome}]: `)).trim() || defaults.codexHome);
    const dataDir = expandHome((await rl.question(`Data dir [${defaults.dataDir}]: `)).trim() || defaults.dataDir);
    const allowedRootsRaw = (await rl.question(`Allowed roots (comma separated) [${defaults.allowedRoots.join(", ")}]: `)).trim();
    const corsRaw = (await rl.question(`CORS origins (comma separated, optional) [${defaults.corsAllowedOrigins.join(", ")}]: `)).trim();
    const backendBinaryPath = expandHome((await rl.question(`Backend binary path (optional) [${defaults.backendBinaryPath || "auto"}]: `)).trim() || defaults.backendBinaryPath || "");
    const password = await rl.question("Password (leave blank to keep existing hash): ");
    const passwordHash = password.trim() ? hashPassword(password.trim()) : defaults.passwordHash;

    if (!passwordHash) {
      throw new Error("A password is required.");
    }

    const config = {
      host,
      port: Number.isFinite(port) ? port : defaults.port,
      basePath: basePath.startsWith("/") ? basePath : `/${basePath}`,
      codexBin,
      codexHome,
      dataDir,
      allowedRoots:
        allowedRootsRaw
          ? allowedRootsRaw.split(",").map((entry) => expandHome(entry.trim())).filter(Boolean)
          : defaults.allowedRoots,
      corsAllowedOrigins:
        corsRaw
          ? corsRaw.split(",").map((entry) => entry.trim()).filter(Boolean)
          : defaults.corsAllowedOrigins,
      passwordHash,
      sessionSecret: defaults.sessionSecret || createSessionSecret(),
      backendBinaryPath
    };

    await writeConfig(config);
    return config;
  } finally {
    rl.close();
  }
}

function buildUrl(config) {
  const basePath = config.basePath === "/" ? "" : config.basePath;
  return `http://${config.host}:${config.port}${basePath}/login`;
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
  const currentPid = await readPid();
  if (isRunning(currentPid)) {
    return { pid: currentPid, alreadyRunning: true };
  }

  const backendBinary = await resolveBackendBinary(config);
  const logHandle = await fs.open(logPath, "a");
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
      CODEX_WEBUI_CODEX_HOME: String(config.codexHome),
      CODEX_WEBUI_DATA_DIR: String(config.dataDir),
      CODEX_WEBUI_ALLOWED_ROOTS: config.allowedRoots.join(path.delimiter),
      CODEX_WEBUI_PASSWORD_HASH: String(config.passwordHash),
      CODEX_WEBUI_SESSION_SECRET: String(config.sessionSecret),
      CODEX_WEBUI_CORS_ALLOWED_ORIGINS: config.corsAllowedOrigins.join(",")
    }
  });
  child.unref();
  await fs.writeFile(pidPath, String(child.pid), "utf8");
  await logHandle.close();
  return { pid: child.pid, alreadyRunning: false };
}

async function stopServer() {
  const pid = await readPid();
  if (!pid || !isRunning(pid)) {
    await fs.rm(pidPath, { force: true });
    return { stopped: false };
  }
  process.kill(pid, "SIGTERM");
  await fs.rm(pidPath, { force: true });
  return { stopped: true, pid };
}

async function restartServer(config) {
  await stopServer();
  return startServer(config);
}

async function runTunnel(config) {
  const { pid } = await startServer(config);
  const targetUrl = buildBaseUrl(config);
  console.log(`Server running at ${buildUrl(config)} (pid ${pid})`);

  if (commandAvailable("cloudflared")) {
    console.log("Starting cloudflared tunnel...");
    const child = spawn("cloudflared", ["tunnel", "--url", targetUrl], {
      cwd: packageRoot,
      stdio: "inherit"
    });
    child.on("exit", (code) => process.exit(code ?? 0));
    return;
  }

  if (commandAvailable("ngrok")) {
    console.log("Starting ngrok tunnel...");
    const child = spawn("ngrok", ["http", `${config.host}:${config.port}`], {
      cwd: packageRoot,
      stdio: "inherit"
    });
    child.on("exit", (code) => process.exit(code ?? 0));
    return;
  }

  throw new Error("Neither cloudflared nor ngrok is installed.");
}

function printUsage(config, pid, alreadyRunning) {
  console.log(alreadyRunning ? "codex-webui is already running." : "codex-webui started in the background.");
  console.log(`URL: ${buildUrl(config)}`);
  console.log(`PID: ${pid}`);
  console.log(`Config: ${configPath}`);
  console.log(`Log: ${logPath}`);
  console.log("");
  console.log("Commands:");
  console.log("  codex-webui config   Re-run the interactive setup");
  console.log("  codex-webui restart  Restart the background server");
  console.log("  codex-webui stop     Stop the background server");
  console.log("  codex-webui tunnel   Expose the running UI through cloudflared or ngrok");
}

async function main() {
  const command = process.argv[2] ?? "";
  let config = await readConfig();

  if (!config || command === "config") {
    config = await promptConfig(config);
    console.log(`Saved configuration to ${configPath}`);
    if (command === "config") {
      return;
    }
  }

  if (command === "stop") {
    const result = await stopServer();
    console.log(result.stopped ? `Stopped codex-webui (pid ${result.pid}).` : "codex-webui is not running.");
    return;
  }

  if (command === "restart") {
    const result = await restartServer(config);
    printUsage(config, result.pid, false);
    return;
  }

  if (command === "tunnel") {
    await runTunnel(config);
    return;
  }

  const result = await startServer(config);
  printUsage(config, result.pid, result.alreadyRunning);
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
});
