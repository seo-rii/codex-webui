import path from "node:path";
import { Worker } from "node:worker_threads";

export type IndexedSessionSummary = {
  id: string;
  name: string | null;
  preview: string;
  cwd: string;
  isSubagent: boolean;
  createdAt: number;
  updatedAt: number;
  status: string;
};

export type IndexedSessionPage = {
  entries: IndexedSessionSummary[];
  nextCursor: string | null;
};

type WorkerResponse = {
  id: string;
  ok: boolean;
  result?: {
    entries: IndexedSessionSummary[];
    nextCursor?: string | null;
  };
  error?: string;
};

export class SessionIndexClient {
  private worker: Worker | null = null;
  private nextRequestId = 1;
  private pending = new Map<
    string,
    {
      resolve: (value: IndexedSessionPage) => void;
      reject: (error: Error) => void;
    }
  >();

  async list(codexHome: string) {
    const page = await this.page(codexHome, null, Number.MAX_SAFE_INTEGER, null);
    return page.entries;
  }

  async page(codexHome: string, cursor: string | null = null, limit = 20, query: string | null = null) {
    if (!codexHome) {
      return {
        entries: [],
        nextCursor: null
      } satisfies IndexedSessionPage;
    }
    const worker = this.getWorker();
    const id = `session-index-${this.nextRequestId++}`;
    return new Promise<IndexedSessionPage>((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      worker.postMessage({
        id,
        method: "session-index/page",
        params: {
          codexHome,
          cursor,
          limit,
          query
        }
      });
    });
  }

  invalidate() {
    if (!this.worker) {
      return;
    }
    this.worker.terminate();
    this.worker = null;
    for (const pending of this.pending.values()) {
      pending.reject(new Error("Session index worker restarted."));
    }
    this.pending.clear();
  }

  private getWorker() {
    if (this.worker) {
      return this.worker;
    }

    const workerPath = path.join(process.cwd(), "scripts", "session-index-worker.mjs");
    this.worker = new Worker(workerPath);
    this.worker.on("message", (message: WorkerResponse) => {
      const pending = this.pending.get(message.id);
      if (!pending) {
        return;
      }
      this.pending.delete(message.id);
      if (!message.ok) {
        pending.reject(new Error(message.error || "Session index worker failed."));
        return;
      }
      pending.resolve({
        entries: message.result?.entries ?? [],
        nextCursor:
          typeof message.result?.nextCursor === "string" && message.result.nextCursor.trim() ? message.result.nextCursor : null
      });
    });
    this.worker.on("error", (error: Error) => {
      for (const pending of this.pending.values()) {
        pending.reject(error);
      }
      this.pending.clear();
      this.worker = null;
    });
    this.worker.on("exit", () => {
      this.worker = null;
    });
    return this.worker;
  }
}

export const sessionIndexClient = new SessionIndexClient();
