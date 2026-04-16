import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { EventEmitter } from "node:events";
import readline from "node:readline";
import type { Readable } from "node:stream";

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

export class AppServerClient {
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
    await ensureDataDirectories();
    const { codexBin, codexHome } = getRuntimeConfig();

    this.child = spawn(codexBin, ["app-server", "--listen", "stdio://"], {
      env: {
        ...process.env,
        CODEX_HOME: codexHome
      },
      stdio: ["pipe", "pipe", "pipe"]
    });

    this.child.stderr.on("data", (chunk: Buffer | string) => {
      const message = chunk.toString().trim();
      if (message) {
        console.error(`[codex app-server] ${message}`);
      }
    });

    this.child.once("error", (error: Error) => {
      this.failPending(error);
    });

    this.child.once("exit", (code: number | null, signal: NodeJS.Signals | null) => {
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
}
