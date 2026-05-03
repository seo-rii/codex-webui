#!/usr/bin/env node

import fs from "node:fs/promises";
import process from "node:process";
import readline from "node:readline";
import WebSocket from "ws";

const args = process.argv.slice(2);
const options = {
  url: process.env.CODEX_WEBUI_URL ?? "http://127.0.0.1:4173/absproxy/4173",
  password: process.env.CODEX_WEBUI_PASSWORD ?? "",
  cookie: process.env.CODEX_WEBUI_COOKIE ?? "",
  profile: process.env.CODEX_WEBUI_PROFILE ?? "",
  method: "",
  params: "",
  paramsFile: "",
  subscribeGlobal: false,
  sessions: [],
  terminals: [],
  raw: false,
  noInteractive: false,
  timeoutMs: 15_000,
  insecure: false,
  printCookie: false
};

for (let index = 0; index < args.length; index += 1) {
  const argument = args[index];
  if (argument === "--url" && args[index + 1]) {
    options.url = args[index + 1];
    index += 1;
    continue;
  }
  if (argument === "--password" && args[index + 1]) {
    options.password = args[index + 1];
    index += 1;
    continue;
  }
  if (argument === "--cookie" && args[index + 1]) {
    options.cookie = args[index + 1];
    index += 1;
    continue;
  }
  if (argument === "--profile" && args[index + 1]) {
    options.profile = args[index + 1];
    index += 1;
    continue;
  }
  if (argument === "--method" && args[index + 1]) {
    options.method = args[index + 1];
    index += 1;
    continue;
  }
  if (argument === "--params" && args[index + 1]) {
    options.params = args[index + 1];
    index += 1;
    continue;
  }
  if (argument === "--params-file" && args[index + 1]) {
    options.paramsFile = args[index + 1];
    index += 1;
    continue;
  }
  if (argument === "--session" && args[index + 1]) {
    options.sessions.push(args[index + 1]);
    index += 1;
    continue;
  }
  if (argument === "--terminal" && args[index + 1]) {
    options.terminals.push(args[index + 1]);
    index += 1;
    continue;
  }
  if (argument === "--global") {
    options.subscribeGlobal = true;
    continue;
  }
  if (argument === "--raw") {
    options.raw = true;
    continue;
  }
  if (argument === "--no-interactive") {
    options.noInteractive = true;
    continue;
  }
  if (argument === "--timeout-ms" && args[index + 1]) {
    options.timeoutMs = Number.parseInt(args[index + 1], 10) || options.timeoutMs;
    index += 1;
    continue;
  }
  if (argument === "--insecure") {
    options.insecure = true;
    continue;
  }
  if (argument === "--print-cookie") {
    options.printCookie = true;
    continue;
  }
  if (argument === "--help" || argument === "-h") {
    console.log(`Usage: node scripts/ws-debug-client.mjs [options]

Options:
  --url <http(s)://host[:port][/base-path]>   codex-webui base URL
  --password <password>                       login first and reuse the auth cookie
  --cookie <cookie-header>                    reuse an existing Cookie header directly
  --profile <profile-id>                      switch the active auth profile before opening WS
  --method <rpc/method>                       send one RPC request after connect
  --params <json>                             JSON params for --method
  --params-file <path>                        read JSON params from a file
  --global                                    subscribe to global events
  --session <session-id>                      subscribe to a session event stream
  --terminal <terminal-id>                    subscribe to a terminal stream
  --raw                                       print raw envelopes instead of pretty output
  --no-interactive                            do not open the stdin REPL
  --timeout-ms <ms>                           request timeout for one-shot requests
  --insecure                                  allow invalid TLS certificates for HTTPS/WSS
  --print-cookie                              print the final Cookie header and exit if nothing else is requested

Interactive commands:
  /req <method> <json>
  /global
  /session <session-id>
  /terminal <terminal-id>
  /unsub-global
  /unsub-session <session-id>
  /unsub-terminal <terminal-id>
  /ping
  /raw on|off
  /quit

Examples:
  node scripts/ws-debug-client.mjs --password test --method sessions/list --params '{"limit":2}' --no-interactive
  node scripts/ws-debug-client.mjs --password test --session 019d... --global
`);
    process.exit(0);
  }

  console.error(`Unknown argument: ${argument}`);
  process.exit(1);
}

const baseUrl = new URL(options.url);
const normalizedBasePath = `${baseUrl.pathname.replace(/\/+$/u, "") || ""}`;
const cookieJar = new Map();
const pendingRequests = new Map();
let requestCounter = 0;
let heartbeatTimer = null;
let rl = null;

if (options.cookie.trim()) {
  for (const part of options.cookie.split(";")) {
    const cookie = part.trim();
    if (!cookie) {
      continue;
    }
    const separatorIndex = cookie.indexOf("=");
    if (separatorIndex <= 0) {
      continue;
    }
    cookieJar.set(cookie.slice(0, separatorIndex).trim(), cookie.slice(separatorIndex + 1).trim());
  }
}

if (options.params && options.paramsFile) {
  console.error("Use either --params or --params-file, not both.");
  process.exit(1);
}

let initialParams = {};
if (options.paramsFile) {
  try {
    initialParams = JSON.parse(await fs.readFile(options.paramsFile, "utf8"));
  } catch (error) {
    console.error(`Failed to read params file: ${error instanceof Error ? error.message : String(error)}`);
    process.exit(1);
  }
} else if (options.params.trim()) {
  try {
    initialParams = JSON.parse(options.params);
  } catch (error) {
    console.error(`Failed to parse --params JSON: ${error instanceof Error ? error.message : String(error)}`);
    process.exit(1);
  }
}

const buildUrl = (suffix) => {
  const nextUrl = new URL(baseUrl);
  nextUrl.pathname = `${normalizedBasePath}${suffix}` || suffix;
  nextUrl.search = "";
  nextUrl.hash = "";
  return nextUrl;
};

const getCookieHeader = () =>
  [...cookieJar.entries()]
    .map(([name, value]) => `${name}=${value}`)
    .join("; ");

const mergeResponseCookies = (response) => {
  const setCookies =
    typeof response.headers.getSetCookie === "function"
      ? response.headers.getSetCookie()
      : response.headers.get("set-cookie")
        ? [response.headers.get("set-cookie")]
        : [];

  for (const rawCookie of setCookies) {
    const parts = String(rawCookie)
      .split(";")
      .map((part) => part.trim())
      .filter(Boolean);
    const firstPart = parts[0];
    if (!firstPart) {
      continue;
    }
    const separatorIndex = firstPart.indexOf("=");
    if (separatorIndex <= 0) {
      continue;
    }
    const name = firstPart.slice(0, separatorIndex);
    const value = firstPart.slice(separatorIndex + 1);
    const expires = parts
      .find((part) => part.toLowerCase().startsWith("expires="))
      ?.slice("expires=".length);
    const maxAge = parts
      .find((part) => part.toLowerCase().startsWith("max-age="))
      ?.slice("max-age=".length);
    const expired =
      value === "" ||
      maxAge === "0" ||
      (expires ? Number.isFinite(Date.parse(expires)) && Date.parse(expires) <= Date.now() : false);
    if (expired) {
      continue;
    }
    cookieJar.set(name, value);
  }
};

if (options.password.trim()) {
  const loginResponse = await fetch(buildUrl("/api/auth/login"), {
    method: "POST",
    credentials: "include",
    headers: {
      "content-type": "application/json",
      ...(getCookieHeader() ? { cookie: getCookieHeader() } : {})
    },
    body: JSON.stringify({ password: options.password })
  }).catch((error) => {
    console.error(`Login request failed: ${error instanceof Error ? error.message : String(error)}`);
    process.exit(1);
  });

  if (!loginResponse.ok) {
    console.error(`Login failed: ${loginResponse.status} ${await loginResponse.text()}`);
    process.exit(1);
  }

  mergeResponseCookies(loginResponse);
}

if (options.profile.trim()) {
  const profileResponse = await fetch(buildUrl("/api/auth/profile"), {
    method: "POST",
    credentials: "include",
    headers: {
      "content-type": "application/json",
      ...(getCookieHeader() ? { cookie: getCookieHeader() } : {})
    },
    body: JSON.stringify({ profileId: options.profile })
  }).catch((error) => {
    console.error(`Profile switch failed: ${error instanceof Error ? error.message : String(error)}`);
    process.exit(1);
  });

  if (!profileResponse.ok) {
    console.error(`Profile switch failed: ${profileResponse.status} ${await profileResponse.text()}`);
    process.exit(1);
  }

  mergeResponseCookies(profileResponse);
}

if (options.printCookie) {
  console.log(getCookieHeader());
  if (!options.method && !options.subscribeGlobal && options.sessions.length === 0 && options.terminals.length === 0 && options.noInteractive) {
    process.exit(0);
  }
}

class DebugSocketClient {
  constructor(url, cookieHeader, insecure, raw) {
    this.url = url;
    this.cookieHeader = cookieHeader;
    this.insecure = insecure;
    this.raw = raw;
    this.socket = null;
    this.connected = false;
    this.closed = false;
    this.connectionId = null;
    this.onEnvelope = null;
    this.onClosed = null;
  }

  async connect() {
    await new Promise((resolve, reject) => {
      const socket = new WebSocket(this.url, {
        headers: this.cookieHeader ? { Cookie: this.cookieHeader } : undefined,
        rejectUnauthorized: !this.insecure
      });
      this.socket = socket;

      socket.once("open", () => {
        this.connected = true;
        resolve();
      });

      socket.on("message", (payload) => {
        if (!this.connected) {
          return;
        }
        try {
          const envelope = JSON.parse(typeof payload === "string" ? payload : payload.toString("utf8"));
          if (envelope.kind === "ready" && typeof envelope.connectionId === "string") {
            this.connectionId = envelope.connectionId;
          }
          if (typeof this.onEnvelope === "function") {
            this.onEnvelope(envelope);
          }
        } catch (error) {
          console.error(`Failed to parse text frame: ${error instanceof Error ? error.message : String(error)}`);
        }
      });

      socket.on("ping", (payload) => {
        if (socket.readyState === WebSocket.OPEN) {
          socket.pong(payload);
        }
      });

      socket.once("error", (error) => {
        if (!this.connected) {
          reject(error);
          return;
        }
        console.error(`Socket error: ${error.message}`);
      });

      socket.on("close", () => {
        this.connected = false;
        this.closed = true;
        if (typeof this.onClosed === "function") {
          this.onClosed();
        }
      });
    });
  }

  sendJson(payload) {
    if (!this.socket || !this.connected) {
      throw new Error("WebSocket is not connected.");
    }
    this.socket.send(JSON.stringify(payload));
  }

  close() {
    if (this.closed) {
      return;
    }
    this.closed = true;
    this.connected = false;
    this.socket?.close();
    this.socket?.terminate();
  }
}

const wsUrl = buildUrl("/ws");
wsUrl.protocol = wsUrl.protocol === "https:" ? "wss:" : "ws:";
const client = new DebugSocketClient(wsUrl, getCookieHeader(), options.insecure, options.raw);

const logEnvelope = (envelope) => {
  if (options.raw) {
    console.log(JSON.stringify(envelope));
    return;
  }

  const timestamp = new Date().toISOString();
  if (envelope.kind === "ready") {
    console.log(`[${timestamp}] ready ${envelope.connectionId}`);
    return;
  }

  if (envelope.kind === "response") {
    const pending = pendingRequests.get(envelope.id) ?? null;
    const label = pending ? `${pending.method} (${envelope.id})` : envelope.id;
    console.log(`[${timestamp}] response ${envelope.ok ? "ok" : "error"} ${label}`);
    if (envelope.ok) {
      console.dir(envelope.result ?? null, { depth: null, colors: process.stdout.isTTY });
    } else {
      console.error(envelope.error ?? "Unknown websocket request error.");
    }
    return;
  }

  if (envelope.kind === "event") {
    console.log(`[${timestamp}] session event ${envelope.sessionId}`);
    console.dir(envelope.event, { depth: null, colors: process.stdout.isTTY });
    return;
  }

  if (envelope.kind === "globalEvent") {
    console.log(`[${timestamp}] global event`);
    console.dir(envelope.event, { depth: null, colors: process.stdout.isTTY });
    return;
  }

  if (envelope.kind === "terminalEvent") {
    console.log(`[${timestamp}] terminal event ${envelope.terminalId}`);
    console.dir(envelope.event, { depth: null, colors: process.stdout.isTTY });
    return;
  }

  if (envelope.kind === "pong") {
    console.log(`[${timestamp}] pong ${envelope.nonce ?? ""}`.trimEnd());
    return;
  }

  console.dir(envelope, { depth: null, colors: process.stdout.isTTY });
};

client.onEnvelope = (envelope) => {
  logEnvelope(envelope);
  if (envelope.kind === "response" && pendingRequests.has(envelope.id)) {
    const pending = pendingRequests.get(envelope.id);
    pendingRequests.delete(envelope.id);
    if (envelope.ok) {
      pending?.resolve(envelope.result ?? null);
      return;
    }
    pending?.reject(new Error(envelope.error ?? "WebSocket request failed."));
  }
};

client.onClosed = () => {
  if (heartbeatTimer) {
    clearInterval(heartbeatTimer);
    heartbeatTimer = null;
  }
  if (rl) {
    rl.close();
    rl = null;
  }
};

const sendRequest = (method, params = {}) =>
  new Promise((resolve, reject) => {
    const id = `debug-${Date.now()}-${requestCounter += 1}`;
    pendingRequests.set(id, { method, resolve, reject });
    try {
      client.sendJson({
        kind: "request",
        id,
        method,
        params
      });
    } catch (error) {
      pendingRequests.delete(id);
      reject(error);
    }
  });

await client.connect().catch((error) => {
  console.error(`WebSocket connection failed: ${error instanceof Error ? error.message : String(error)}`);
  process.exit(1);
});

heartbeatTimer = setInterval(() => {
  if (!client.connected) {
    return;
  }
  try {
    client.sendJson({
      kind: "ping",
      nonce: `heartbeat-${Date.now()}`
    });
  } catch (error) {
    console.error(`Heartbeat failed: ${error instanceof Error ? error.message : String(error)}`);
  }
}, 20_000);

if (options.subscribeGlobal) {
  await sendRequest("events/subscribe", {}).catch((error) => {
    console.error(`Global subscribe failed: ${error instanceof Error ? error.message : String(error)}`);
  });
}

for (const sessionId of options.sessions) {
  await sendRequest("session/subscribe", { sessionId }).catch((error) => {
    console.error(`Session subscribe failed for ${sessionId}: ${error instanceof Error ? error.message : String(error)}`);
  });
}

for (const terminalId of options.terminals) {
  await sendRequest("terminal/subscribe", { terminalId }).catch((error) => {
    console.error(`Terminal subscribe failed for ${terminalId}: ${error instanceof Error ? error.message : String(error)}`);
  });
}

let shouldExitAfterInitialRequest = false;
if (options.method.trim()) {
  shouldExitAfterInitialRequest = options.noInteractive && !options.subscribeGlobal && options.sessions.length === 0 && options.terminals.length === 0;
  const timeout = setTimeout(() => {
    console.error(`Timed out waiting for response to ${options.method}.`);
    client.close();
    process.exit(1);
  }, options.timeoutMs);

  await sendRequest(options.method, initialParams)
    .catch((error) => {
      console.error(`Initial request failed: ${error instanceof Error ? error.message : String(error)}`);
      client.close();
      process.exit(1);
    })
    .finally(() => {
      clearTimeout(timeout);
    });
}

if (shouldExitAfterInitialRequest) {
  client.close();
  process.exit(0);
}

if (options.noInteractive) {
  if (!options.method && !options.subscribeGlobal && options.sessions.length === 0 && options.terminals.length === 0) {
    console.error("Nothing to do. Pass --method or a subscription flag, or omit --no-interactive.");
    client.close();
    process.exit(1);
  }
  await new Promise(() => {});
}

console.log("Interactive mode. Type /help-like commands such as /req sessions/list {\"limit\":2} or /quit.");
rl = readline.createInterface({
  input: process.stdin,
  output: process.stdout,
  terminal: true
});

rl.on("line", async (line) => {
  const trimmed = line.trim();
  if (!trimmed) {
    return;
  }

  if (trimmed === "/quit" || trimmed === "/exit") {
    client.close();
    process.exit(0);
  }

  if (trimmed === "/help") {
    console.log("/req <method> <json>");
    console.log("/global");
    console.log("/session <session-id>");
    console.log("/terminal <terminal-id>");
    console.log("/unsub-global");
    console.log("/unsub-session <session-id>");
    console.log("/unsub-terminal <terminal-id>");
    console.log("/ping");
    console.log("/raw on|off");
    console.log("/quit");
    return;
  }

  if (trimmed === "/global") {
    await sendRequest("events/subscribe", {}).catch((error) => {
      console.error(error instanceof Error ? error.message : String(error));
    });
    return;
  }

  if (trimmed === "/unsub-global") {
    await sendRequest("events/unsubscribe", {}).catch((error) => {
      console.error(error instanceof Error ? error.message : String(error));
    });
    return;
  }

  if (trimmed === "/ping") {
    client.sendJson({
      kind: "ping",
      nonce: `manual-${Date.now()}`
    });
    return;
  }

  if (trimmed.startsWith("/raw ")) {
    const mode = trimmed.slice("/raw ".length).trim().toLowerCase();
    options.raw = mode === "on";
    client.raw = options.raw;
    console.log(`raw output ${options.raw ? "enabled" : "disabled"}`);
    return;
  }

  if (trimmed.startsWith("/session ")) {
    const sessionId = trimmed.slice("/session ".length).trim();
    await sendRequest("session/subscribe", { sessionId }).catch((error) => {
      console.error(error instanceof Error ? error.message : String(error));
    });
    return;
  }

  if (trimmed.startsWith("/unsub-session ")) {
    const sessionId = trimmed.slice("/unsub-session ".length).trim();
    await sendRequest("session/unsubscribe", { sessionId }).catch((error) => {
      console.error(error instanceof Error ? error.message : String(error));
    });
    return;
  }

  if (trimmed.startsWith("/terminal ")) {
    const terminalId = trimmed.slice("/terminal ".length).trim();
    await sendRequest("terminal/subscribe", { terminalId }).catch((error) => {
      console.error(error instanceof Error ? error.message : String(error));
    });
    return;
  }

  if (trimmed.startsWith("/unsub-terminal ")) {
    const terminalId = trimmed.slice("/unsub-terminal ".length).trim();
    await sendRequest("terminal/unsubscribe", { terminalId }).catch((error) => {
      console.error(error instanceof Error ? error.message : String(error));
    });
    return;
  }

  if (trimmed.startsWith("/req ")) {
    const requestText = trimmed.slice("/req ".length).trim();
    const separatorIndex = requestText.indexOf(" ");
    const method = separatorIndex < 0 ? requestText : requestText.slice(0, separatorIndex);
    const paramsText = separatorIndex < 0 ? "{}" : requestText.slice(separatorIndex + 1).trim();
    let params = {};
    try {
      params = paramsText ? JSON.parse(paramsText) : {};
    } catch (error) {
      console.error(`Invalid JSON params: ${error instanceof Error ? error.message : String(error)}`);
      return;
    }

    await sendRequest(method, params).catch((error) => {
      console.error(error instanceof Error ? error.message : String(error));
    });
    return;
  }

  console.error("Unknown command. Use /help.");
});

process.on("SIGINT", () => {
  client.close();
  process.exit(0);
});

process.on("SIGTERM", () => {
  client.close();
  process.exit(0);
});
