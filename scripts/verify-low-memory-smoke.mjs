#!/usr/bin/env node

import fs from "node:fs/promises";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import WebSocket from "ws";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const args = process.argv.slice(2);
const options = {
  memoryMb: 8192,
  timeoutMs: 60_000,
  keepTemp: false,
  backendBin: process.env.CODEX_WEBUI_BACKEND_BIN ?? "",
  fakeCodexBin: process.env.CODEX_WEBUI_FAKE_CODEX_BIN ?? ""
};

for (let index = 0; index < args.length; index += 1) {
  const arg = args[index];
  if (arg === "--memory-mb" && args[index + 1]) {
    options.memoryMb = Number.parseInt(args[index + 1], 10) || options.memoryMb;
    index += 1;
    continue;
  }
  if (arg === "--timeout-ms" && args[index + 1]) {
    options.timeoutMs = Number.parseInt(args[index + 1], 10) || options.timeoutMs;
    index += 1;
    continue;
  }
  if (arg === "--backend-bin" && args[index + 1]) {
    options.backendBin = args[index + 1];
    index += 1;
    continue;
  }
  if (arg === "--fake-codex-bin" && args[index + 1]) {
    options.fakeCodexBin = args[index + 1];
    index += 1;
    continue;
  }
  if (arg === "--keep-temp") {
    options.keepTemp = true;
    continue;
  }
  if (arg === "--help" || arg === "-h") {
    console.log(`Usage: node scripts/verify-low-memory-smoke.mjs [options]

Options:
  --memory-mb <mb>         POSIX virtual-memory limit for the gateway child. Default: 8192.
  --timeout-ms <ms>        Whole smoke timeout. Default: 60000.
  --backend-bin <path>     Backend binary path. Defaults to CODEX_WEBUI_BACKEND_BIN or target builds.
  --fake-codex-bin <path>  Fake Codex app-server binary path. Defaults to CODEX_WEBUI_FAKE_CODEX_BIN or target builds.
  --keep-temp              Keep the temporary workspace after the run.
`);
    process.exit(0);
  }
  throw new Error(`Unknown argument: ${arg}`);
}

const executableName = process.platform === "win32" ? ".exe" : "";
const firstExistingPath = async (candidates) => {
  for (const candidate of candidates.filter(Boolean)) {
    const absolute = path.resolve(repoRoot, candidate);
    try {
      await fs.access(absolute);
      return absolute;
    } catch {
      // Try the next candidate.
    }
  }
  return null;
};

const backendBin = await firstExistingPath([
  options.backendBin,
  path.join("backend", "target", "release", `backend${executableName}`),
  path.join("backend", "target", "debug", `backend${executableName}`)
]);
const fakeCodexBin = await firstExistingPath([
  options.fakeCodexBin,
  path.join("backend", "target", "release", `fake_codex_app_server${executableName}`),
  path.join("backend", "target", "debug", `fake_codex_app_server${executableName}`)
]);

if (!backendBin || !fakeCodexBin) {
  console.error("Missing smoke binaries.");
  console.error("Run `pnpm gateway:build` and `pnpm build:e2e-fake`, or pass --backend-bin/--fake-codex-bin.");
  process.exit(1);
}

const withTimeout = async (promise, label) => {
  let timer = null;
  try {
    return await Promise.race([
      promise,
      new Promise((_, reject) => {
        timer = setTimeout(() => reject(new Error(`${label} timed out after ${options.timeoutMs}ms`)), options.timeoutMs);
      })
    ]);
  } finally {
    if (timer) {
      clearTimeout(timer);
    }
  }
};

const findFreePort = () =>
  new Promise((resolve, reject) => {
    const server = net.createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      server.close(() => resolve(address.port));
    });
  });

const waitForHttpOk = async (url) => {
  const deadline = Date.now() + options.timeoutMs;
  let lastError = null;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(url);
      if (response.ok) {
        return response;
      }
      lastError = new Error(`${url} returned ${response.status}`);
    } catch (error) {
      lastError = error;
    }
    await new Promise((resolve) => setTimeout(resolve, 150));
  }
  throw lastError ?? new Error(`${url} did not become ready`);
};

const requestWs = (socket, method, params) =>
  new Promise((resolve, reject) => {
    const id = `smoke-${Date.now()}-${Math.random().toString(36).slice(2)}`;
    const timeout = setTimeout(() => {
      socket.off("message", onMessage);
      reject(new Error(`${method} timed out`));
    }, options.timeoutMs);
    const onMessage = (payload) => {
      const envelope = JSON.parse(typeof payload === "string" ? payload : payload.toString("utf8"));
      if (envelope.kind !== "response" || envelope.id !== id) {
        return;
      }
      clearTimeout(timeout);
      socket.off("message", onMessage);
      if (envelope.ok) {
        resolve(envelope.result ?? null);
      } else {
        reject(new Error(`${method} failed: ${envelope.error ?? "unknown error"}`));
      }
    };
    socket.on("message", onMessage);
    socket.send(JSON.stringify({ kind: "request", id, method, params }));
  });

const cookieHeaderFromResponse = (response) => {
  const cookies =
    typeof response.headers.getSetCookie === "function"
      ? response.headers.getSetCookie()
      : response.headers.get("set-cookie")
        ? [response.headers.get("set-cookie")]
        : [];
  return cookies
    .map((cookie) => String(cookie).split(";")[0]?.trim())
    .filter(Boolean)
    .join("; ");
};

const tempRoot = await fs.mkdtemp(path.join(os.tmpdir(), "codex-webui-low-memory-"));
let child = null;
let childStderr = "";

try {
  const workspace = path.join(tempRoot, "workspace");
  const codexHome = path.join(tempRoot, "codex-home");
  const dataDir = path.join(tempRoot, "data");
  await fs.mkdir(workspace, { recursive: true });
  await fs.mkdir(codexHome, { recursive: true });
  await fs.mkdir(dataDir, { recursive: true });

  const port = await findFreePort();
  const childEnv = {
    ...process.env,
    HOST: "127.0.0.1",
    PORT: String(port),
    CODEX_HOME: codexHome,
    CODEX_WEBUI_BASE_PATH: "",
    CODEX_WEBUI_ALLOWED_ROOTS: workspace,
    CODEX_WEBUI_CODEX_BIN: fakeCodexBin,
    CODEX_WEBUI_DATA_DIR: dataDir,
    CODEX_WEBUI_SESSION_SECRET: "low-memory-smoke-session-secret-32-bytes",
    CODEX_WEBUI_PASSWORD: "low-memory-smoke-password",
    CODEX_WEBUI_PASSWORD_HASH: "",
    CODEX_WEBUI_OWNER_PASSWORD: "",
    CODEX_WEBUI_OWNER_PASSWORD_HASH: "",
    CODEX_WEBUI_VIEWER_PASSWORD: "",
    CODEX_WEBUI_VIEWER_PASSWORD_HASH: "",
    CODEX_WEBUI_MAX_APP_SERVERS: "1",
    CODEX_WEBUI_SERVER_THREADS: "1",
    CODEX_WEBUI_BLOCKING_THREADS: "4",
    CODEX_WEBUI_CONTROLLER_THREADS: "1",
    CODEX_WEBUI_APP_SERVER_HANDOFF: "false"
  };

  if (process.platform === "win32") {
    child = spawn(backendBin, { cwd: repoRoot, env: childEnv, stdio: ["ignore", "pipe", "pipe"] });
  } else {
    childEnv.CODEX_WEBUI_LOW_MEMORY_SMOKE_BACKEND = backendBin;
    child = spawn("bash", ["-lc", `ulimit -v ${options.memoryMb * 1024}; exec "$CODEX_WEBUI_LOW_MEMORY_SMOKE_BACKEND"`], {
      cwd: repoRoot,
      env: childEnv,
      stdio: ["ignore", "pipe", "pipe"]
    });
  }

  child.stderr.on("data", (chunk) => {
    childStderr += chunk.toString("utf8");
  });
  child.stdout.resume();

  child.once("exit", (code, signal) => {
    if (code !== null && code !== 0) {
      childStderr += `\nbackend exited with code ${code}`;
    } else if (signal) {
      childStderr += `\nbackend exited with signal ${signal}`;
    }
  });

  await withTimeout(waitForHttpOk(`http://127.0.0.1:${port}/readyz`), "gateway readiness");

  const loginResponse = await fetch(`http://127.0.0.1:${port}/api/auth/login`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ password: "low-memory-smoke-password" })
  });
  if (!loginResponse.ok) {
    throw new Error(`login failed with ${loginResponse.status}: ${await loginResponse.text()}`);
  }
  const cookieHeader = cookieHeaderFromResponse(loginResponse);
  if (!cookieHeader) {
    throw new Error("login did not return an auth cookie");
  }

  const socket = new WebSocket(`ws://127.0.0.1:${port}/ws`, {
    headers: { Cookie: cookieHeader }
  });
  await withTimeout(
    new Promise((resolve, reject) => {
      socket.once("open", resolve);
      socket.once("error", reject);
      socket.once("unexpected-response", (_request, response) => {
        reject(new Error(`websocket upgrade returned ${response.statusCode}`));
      });
    }),
    "websocket connect"
  );

  try {
    const session = await requestWs(socket, "session/create", {
      preferences: {
        cwd: workspace,
        model: "gpt-5",
        approvalPolicy: "on-request",
        sandboxMode: "workspace-write"
      },
      name: "Low memory smoke"
    });
    const sessionId = session.id;
    if (!sessionId) {
      throw new Error("session/create did not return an id");
    }
    const turn = await requestWs(socket, "turn/send", {
      sessionId,
      prompt: "Say low-memory smoke passed.",
      attachmentIds: [],
      skills: [],
      preferences: {
        cwd: workspace,
        model: "gpt-5",
        approvalPolicy: "on-request",
        sandboxMode: "workspace-write"
      },
      clientUserMessageId: "low-memory-smoke-user-message"
    });
    if (!turn?.turnId) {
      throw new Error("turn/send did not return a turn id");
    }
    await requestWs(socket, "session/get", { sessionId, limit: 20 });
  } finally {
    socket.close();
    socket.terminate();
  }

  const metrics = await (
    await fetch(`http://127.0.0.1:${port}/metrics`, {
      headers: { Cookie: cookieHeader }
    })
  ).text();
  if (!metrics.includes("codex_webui_host_memory_current_bytes")) {
    throw new Error("metrics did not expose host memory diagnostics");
  }

  console.log(`low-memory smoke passed on port ${port} with ${options.memoryMb}MB limit`);
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  if (childStderr.trim()) {
    console.error(childStderr.trim());
  }
  process.exitCode = 1;
} finally {
  if (child && child.exitCode === null && child.signalCode === null) {
    child.kill("SIGTERM");
    await Promise.race([
      new Promise((resolve) => child.once("exit", resolve)),
      new Promise((resolve) => setTimeout(resolve, 5_000))
    ]);
    if (child.exitCode === null && child.signalCode === null) {
      child.kill("SIGKILL");
    }
  }
  if (!options.keepTemp) {
    await fs.rm(tempRoot, { recursive: true, force: true });
  } else {
    console.log(`kept temp directory: ${tempRoot}`);
  }
}
