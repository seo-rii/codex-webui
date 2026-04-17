import fsp from "node:fs/promises";

import type { SessionPreferences, SessionQueueItem, StartupScheduledShutdownAlert } from "$lib/types";

import { ensureDataDirectories, getStoreFilePath, pathExists } from "./fs";

type UiState = {
  global: {
    shutdownAfterQueueCompletes: boolean;
    scheduledShutdown: StartupScheduledShutdownAlert | null;
  };
  preferencesByThreadId: Record<string, SessionPreferences>;
  draftsByThreadId: Record<
    string,
    {
      draft: string;
      intent: "message" | "steer" | "queue";
      updatedAt: number;
    }
  >;
  queuesByThreadId: Record<
    string,
    {
      items: SessionQueueItem[];
      resumePending: boolean;
      updatedAt: number;
    }
  >;
};

class UiStateStore {
  private state: UiState | null = null;
  private writeChain = Promise.resolve();

  private async load() {
    if (this.state) {
      return this.state;
    }

    await ensureDataDirectories();
    const storePath = getStoreFilePath();
    if (!(await pathExists(storePath))) {
      this.state = {
        global: {
          shutdownAfterQueueCompletes: false,
          scheduledShutdown: null
        },
        preferencesByThreadId: {},
        draftsByThreadId: {},
        queuesByThreadId: {}
      };
      return this.state;
    }

    try {
      const raw = await fsp.readFile(storePath, "utf8");
      const parsed = JSON.parse(raw) as UiState;
      this.state = {
        global: {
          shutdownAfterQueueCompletes: Boolean(parsed.global?.shutdownAfterQueueCompletes),
          scheduledShutdown: parsed.global?.scheduledShutdown ?? null
        },
        preferencesByThreadId: parsed.preferencesByThreadId ?? {},
        draftsByThreadId: parsed.draftsByThreadId ?? {},
        queuesByThreadId: parsed.queuesByThreadId ?? {}
      };
    } catch {
      try {
        await fsp.rename(storePath, `${storePath}.corrupt-${Date.now()}`);
      } catch {
        // Ignore rename failures and continue with a clean in-memory state.
      }

      this.state = {
        global: {
          shutdownAfterQueueCompletes: false,
          scheduledShutdown: null
        },
        preferencesByThreadId: {},
        draftsByThreadId: {},
        queuesByThreadId: {}
      };
      await this.flush();
    }

    return this.state;
  }

  private async flush() {
    await ensureDataDirectories();
    await fsp.writeFile(getStoreFilePath(), JSON.stringify(this.state, null, 2), "utf8");
  }

  async get(threadId: string) {
    const state = await this.load();
    return state.preferencesByThreadId[threadId] ?? null;
  }

  async getGlobal() {
    const state = await this.load();
    return {
      shutdownAfterQueueCompletes: Boolean(state.global.shutdownAfterQueueCompletes),
      scheduledShutdown: state.global.scheduledShutdown ?? null
    };
  }

  async setGlobalShutdownAfterQueueCompletes(enabled: boolean) {
    this.writeChain = this.writeChain.then(async () => {
      const state = await this.load();
      state.global.shutdownAfterQueueCompletes = enabled;
      await this.flush();
    });
    return this.writeChain;
  }

  async setScheduledShutdown(scheduledShutdown: StartupScheduledShutdownAlert | null) {
    this.writeChain = this.writeChain.then(async () => {
      const state = await this.load();
      state.global.scheduledShutdown = scheduledShutdown;
      await this.flush();
    });
    return this.writeChain;
  }

  async getAll() {
    const state = await this.load();
    return { ...state.preferencesByThreadId };
  }

  async set(threadId: string, preferences: SessionPreferences) {
    this.writeChain = this.writeChain.then(async () => {
      const state = await this.load();
      state.preferencesByThreadId[threadId] = preferences;
      await this.flush();
    });
    return this.writeChain;
  }

  async getDraft(threadId: string) {
    const state = await this.load();
    return state.draftsByThreadId[threadId] ?? null;
  }

  async setDraft(threadId: string, draft: string, intent: "message" | "steer" | "queue") {
    this.writeChain = this.writeChain.then(async () => {
      const state = await this.load();
      state.draftsByThreadId[threadId] = {
        draft,
        intent,
        updatedAt: Date.now()
      };
      await this.flush();
    });
    return this.writeChain;
  }

  async clearDraft(threadId: string) {
    this.writeChain = this.writeChain.then(async () => {
      const state = await this.load();
      delete state.draftsByThreadId[threadId];
      await this.flush();
    });
    return this.writeChain;
  }

  async markQueuesPendingResume() {
    this.writeChain = this.writeChain.then(async () => {
      const state = await this.load();
      let touched = false;
      for (const queue of Object.values(state.queuesByThreadId)) {
        if (queue.items.length === 0 || queue.resumePending) {
          continue;
        }
        queue.resumePending = true;
        queue.updatedAt = Date.now();
        touched = true;
      }
      if (touched) {
        await this.flush();
      }
    });
    return this.writeChain;
  }

  async getQueue(threadId: string) {
    const state = await this.load();
    return state.queuesByThreadId[threadId] ?? null;
  }

  async getQueueCounts() {
    const state = await this.load();
    return Object.fromEntries(
      Object.entries(state.queuesByThreadId).map(([threadId, queue]) => [threadId, queue.items.length])
    );
  }

  async listResumePendingQueues() {
    const state = await this.load();
    return Object.entries(state.queuesByThreadId)
      .filter(([, queue]) => queue.resumePending && queue.items.length > 0)
      .map(([threadId, queue]) => ({
        threadId,
        pendingCount: queue.items.length,
        updatedAt: queue.updatedAt
      }));
  }

  async enqueueQueueItem(threadId: string, item: SessionQueueItem) {
    this.writeChain = this.writeChain.then(async () => {
      const state = await this.load();
      const existing = state.queuesByThreadId[threadId] ?? {
        items: [],
        resumePending: false,
        updatedAt: Date.now()
      };
      existing.items = [...existing.items, item];
      existing.updatedAt = Date.now();
      state.queuesByThreadId[threadId] = existing;
      await this.flush();
    });
    return this.writeChain;
  }

  async removeQueueItem(threadId: string, itemId: string) {
    let removed = false;
    this.writeChain = this.writeChain.then(async () => {
      const state = await this.load();
      const existing = state.queuesByThreadId[threadId];
      if (!existing) {
        return;
      }
      const nextItems = existing.items.filter((item) => item.id !== itemId);
      if (nextItems.length === existing.items.length) {
        return;
      }
      removed = true;
      existing.items = nextItems;
      existing.updatedAt = Date.now();
      if (existing.items.length === 0) {
        delete state.queuesByThreadId[threadId];
      }
      await this.flush();
    });
    await this.writeChain;
    return removed;
  }

  async updateQueueItem(
    threadId: string,
    itemId: string,
    nextItem: {
      prompt: string;
      attachmentIds: string[];
      attachmentNames: string[];
    }
  ) {
    let updated = false;
    this.writeChain = this.writeChain.then(async () => {
      const state = await this.load();
      const existing = state.queuesByThreadId[threadId];
      if (!existing) {
        return;
      }
      const itemIndex = existing.items.findIndex((item) => item.id === itemId);
      if (itemIndex < 0) {
        return;
      }
      const currentItem = existing.items[itemIndex];
      existing.items[itemIndex] = {
        ...currentItem,
        prompt: nextItem.prompt,
        attachmentIds: [...nextItem.attachmentIds],
        attachmentNames: [...nextItem.attachmentNames]
      };
      existing.updatedAt = Date.now();
      updated = true;
      await this.flush();
    });
    await this.writeChain;
    return updated;
  }

  async setQueueResumePending(threadId: string, resumePending: boolean) {
    this.writeChain = this.writeChain.then(async () => {
      const state = await this.load();
      const existing = state.queuesByThreadId[threadId];
      if (!existing) {
        return;
      }
      existing.resumePending = resumePending;
      existing.updatedAt = Date.now();
      await this.flush();
    });
    return this.writeChain;
  }
}

export const uiStateStore = new UiStateStore();
