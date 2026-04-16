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

type WorkerResponse = {
  id: string;
  ok: boolean;
  result?: {
    entries: IndexedSessionSummary[];
  };
  error?: string;
};

export class SessionIndexClient {
  private worker: Worker | null = null;
  private nextRequestId = 1;
  private pending = new Map<
    string,
    {
      resolve: (value: IndexedSessionSummary[]) => void;
      reject: (error: Error) => void;
    }
  >();

  async list(codexHome: string) {
    if (!codexHome) {
      return [];
    }
    const worker = this.getWorker();
    const id = `session-index-${this.nextRequestId++}`;
    return new Promise<IndexedSessionSummary[]>((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      worker.postMessage({
        id,
        method: "session-index/list",
        params: { codexHome }
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
      pending.resolve(message.result?.entries ?? []);
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
