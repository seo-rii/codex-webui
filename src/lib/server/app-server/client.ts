import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { EventEmitter } from "node:events";
import readline from "node:readline";
import type { Readable } from "node:stream";

import type { RuntimeProfileConfig } from "../env";
import { getRuntimeConfig } from "../env";
import { ensureDataDirectories } from "../fs";

type JsonRpcMessage = {
  id?: string | number;
  method?: string;
  params?: Record<string, unknown>;
  result?: unknown;
  error?: { code?: number; message?: string; data?: unknown };
};

type NotificationPayload = {
  method: string;
  params: Record<string, unknown>;
};

type ServerRequestPayload = {
  id: string | number;
  method: string;
  params: Record<string, unknown>;
};

const INVALID_REFRESH_TOKEN_PATTERN = /(TokenRefreshFailed|invalid_grant:\s*Invalid refresh token)/iu;

function stripAnsi(value: string) {
  return value.replace(/\u001b\[[0-9;]*m/gu, "");
}

function isInvalidRefreshTokenStderr(value: string) {
  return INVALID_REFRESH_TOKEN_PATTERN.test(stripAnsi(value));
}

export class AppServerClient {
  constructor(private readonly profile: RuntimeProfileConfig) {}

  private child: ChildProcessWithoutNullStreams | null = null;
  private lineReader: readline.Interface | null = null;
  private startPromise: Promise<void> | null = null;
  private nextRequestId = 1;
  private pending = new Map<
    string,
    {
      resolve: (value: unknown) => void;
      reject: (reason?: unknown) => void;
    }
  >();
  private readonly events = new EventEmitter();
  private stderrBuffer = "";
  private suppressedInvalidRefreshTokenWarning = false;

  onNotification(listener: (payload: NotificationPayload) => void) {
    this.events.on("notification", listener);
    return () => this.events.off("notification", listener);
  }

  onServerRequest(listener: (payload: ServerRequestPayload) => void) {
    this.events.on("serverRequest", listener);
    return () => this.events.off("serverRequest", listener);
  }

  async request(method: string, params: Record<string, unknown>) {
    await this.ensureStarted();
    return this.requestRaw(method, params);
  }

  async respond(id: string | number, result: unknown) {
    await this.ensureStarted();
    this.writeMessage({ id, result });
  }

  async reject(id: string | number, message: string) {
    await this.ensureStarted();
    this.writeMessage({
      id,
      error: {
        code: -32000,
        message
      }
    });
  }

  private async ensureStarted() {
    if (this.startPromise) {
      return this.startPromise;
    }
    this.startPromise = this.start();
    return this.startPromise;
  }

  private async start() {
    await ensureDataDirectories(this.profile);
    const { codexBin } = getRuntimeConfig();

    this.child = spawn(codexBin, ["app-server", "--listen", "stdio://"], {
      env: {
        ...process.env,
        CODEX_HOME: this.profile.codexHome
      },
      stdio: ["pipe", "pipe", "pipe"]
    });

    this.stderrBuffer = "";
    this.suppressedInvalidRefreshTokenWarning = false;
    this.child.stderr.on("data", (chunk: Buffer | string) => {
      this.handleStderrChunk(chunk.toString());
    });

    this.child.once("error", (error: Error) => {
      this.failPending(error);
    });

    this.child.once("exit", (code: number | null, signal: NodeJS.Signals | null) => {
      this.flushStderrBuffer();
      this.failPending(new Error(`codex app-server exited (${signal ?? code ?? "unknown"})`));
      this.child = null;
      this.lineReader = null;
      this.startPromise = null;
    });

    this.lineReader = readline.createInterface({
      input: this.child.stdout as Readable,
      crlfDelay: Infinity
    });
    this.lineReader.on("line", (line: string) => this.handleLine(line));

    await this.requestRaw("initialize", {
      clientInfo: {
        name: "codex_webui",
        title: "Codex Web UI",
        version: "0.1.0"
      },
      capabilities: {
        experimentalApi: true
      }
    });
    this.writeMessage({ method: "initialized", params: {} });
  }

  private requestRaw(method: string, params: Record<string, unknown>) {
    const id = this.nextRequestId++;
    const key = String(id);
    return new Promise<unknown>((resolve, reject) => {
      this.pending.set(key, { resolve, reject });
      this.writeMessage({ id, method, params });
    });
  }

  private writeMessage(message: JsonRpcMessage) {
    if (!this.child?.stdin.writable) {
      throw new Error("codex app-server is not writable.");
    }
    this.child.stdin.write(`${JSON.stringify(message)}\n`);
  }

  private handleLine(line: string) {
    if (!line.trim()) {
      return;
    }
    const message = JSON.parse(line) as JsonRpcMessage;

    if (message.method && message.id !== undefined) {
      this.events.emit("serverRequest", {
        id: message.id,
        method: message.method,
        params: (message.params ?? {}) as Record<string, unknown>
      } satisfies ServerRequestPayload);
      return;
    }

    if (message.method) {
      this.events.emit("notification", {
        method: message.method,
        params: (message.params ?? {}) as Record<string, unknown>
      } satisfies NotificationPayload);
      return;
    }

    if (message.id === undefined) {
      return;
    }

    const pending = this.pending.get(String(message.id));
    if (!pending) {
      return;
    }
    this.pending.delete(String(message.id));

    if (message.error) {
      pending.reject(new Error(message.error.message ?? "Unknown app-server error"));
      return;
    }

    pending.resolve(message.result);
  }

  private failPending(reason: unknown) {
    for (const pending of this.pending.values()) {
      pending.reject(reason);
    }
    this.pending.clear();
  }

  private handleStderrChunk(chunk: string) {
    this.stderrBuffer += chunk;
    const lines = this.stderrBuffer.split(/\r?\n/u);
    this.stderrBuffer = lines.pop() ?? "";
    for (const line of lines) {
      this.logStderrLine(line);
    }
  }

  private flushStderrBuffer() {
    if (!this.stderrBuffer.trim()) {
      this.stderrBuffer = "";
      return;
    }
    this.logStderrLine(this.stderrBuffer);
    this.stderrBuffer = "";
  }

  private logStderrLine(line: string) {
    const message = line.trim();
    if (!message) {
      return;
    }

    if (isInvalidRefreshTokenStderr(message)) {
      if (this.suppressedInvalidRefreshTokenWarning) {
        return;
      }
      this.suppressedInvalidRefreshTokenWarning = true;
      console.warn(
        `[codex app-server] ${this.profile.label}: invalid refresh token detected; suppressing repeated refresh-token errors until re-authentication.`
      );
      return;
    }

    console.error(`[codex app-server] ${message}`);
  }
}
