import fsp from "node:fs/promises";

import type {
  AppNotification,
  NotificationEventType,
  NotificationSettings,
  SavedSessionFilter,
  SessionPreferences,
  SessionQueueItem,
  SessionSummaryHighlight,
  StartupScheduledShutdownAlert
} from "$lib/types";

import { ensureDataDirectories, getStoreFilePath, pathExists } from "./fs";

type UiState = {
  global: {
    shutdownAfterQueueCompletes: boolean;
    scheduledShutdown: StartupScheduledShutdownAlert | null;
  };
  notifications: {
    items: AppNotification[];
    settings: NotificationSettings;
  };
  sessionMetaByThreadId: Record<
    string,
    {
      pinned: boolean;
      tags: string[];
    }
  >;
  savedSessionFilters: SavedSessionFilter[];
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
  highlightsByThreadId: Record<string, SessionSummaryHighlight>;
};

const DEFAULT_NOTIFICATION_SETTINGS: NotificationSettings = {
  enabledEventTypes: ["sessionCompleted", "sessionAttention", "queueDispatchFailed", "shutdownScheduled"],
  slackWebhookUrl: null,
  webhookUrl: null
};

function normalizeNotificationSettings(value: NotificationSettings | null | undefined): NotificationSettings {
  return {
    enabledEventTypes: Array.isArray(value?.enabledEventTypes)
      ? value.enabledEventTypes.filter(
          (entry): entry is NotificationEventType =>
            entry === "sessionCompleted" ||
            entry === "sessionAttention" ||
            entry === "queueDispatchFailed" ||
            entry === "shutdownScheduled"
        )
      : [...DEFAULT_NOTIFICATION_SETTINGS.enabledEventTypes],
    slackWebhookUrl: typeof value?.slackWebhookUrl === "string" && value.slackWebhookUrl.trim() ? value.slackWebhookUrl.trim() : null,
    webhookUrl: typeof value?.webhookUrl === "string" && value.webhookUrl.trim() ? value.webhookUrl.trim() : null
  };
}

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
        notifications: {
          items: [],
          settings: { ...DEFAULT_NOTIFICATION_SETTINGS }
        },
        sessionMetaByThreadId: {},
        savedSessionFilters: [],
        preferencesByThreadId: {},
        draftsByThreadId: {},
        queuesByThreadId: {},
        highlightsByThreadId: {}
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
        notifications: {
          items: Array.isArray(parsed.notifications?.items) ? parsed.notifications.items : [],
          settings: normalizeNotificationSettings(parsed.notifications?.settings)
        },
        sessionMetaByThreadId: parsed.sessionMetaByThreadId ?? {},
        savedSessionFilters: Array.isArray(parsed.savedSessionFilters) ? parsed.savedSessionFilters : [],
        preferencesByThreadId: parsed.preferencesByThreadId ?? {},
        draftsByThreadId: parsed.draftsByThreadId ?? {},
        queuesByThreadId: parsed.queuesByThreadId ?? {},
        highlightsByThreadId: parsed.highlightsByThreadId ?? {}
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
        notifications: {
          items: [],
          settings: { ...DEFAULT_NOTIFICATION_SETTINGS }
        },
        sessionMetaByThreadId: {},
        savedSessionFilters: [],
        preferencesByThreadId: {},
        draftsByThreadId: {},
        queuesByThreadId: {},
        highlightsByThreadId: {}
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

  async getNotifications(limit = 80) {
    const state = await this.load();
    const items = [...state.notifications.items]
      .sort((left, right) => right.createdAt - left.createdAt)
      .slice(0, Math.max(1, limit))
      .map((entry) => ({
        ...entry,
        payload: { ...entry.payload }
      }));
    return {
      notifications: items,
      unreadCount: state.notifications.items.filter((entry) => entry.readAt === null).length
    };
  }

  async addNotification(notification: AppNotification) {
    this.writeChain = this.writeChain.then(async () => {
      const state = await this.load();
      state.notifications.items = [notification, ...state.notifications.items.filter((entry) => entry.id !== notification.id)].slice(0, 200);
      await this.flush();
    });
    return this.writeChain;
  }

  async markNotificationsRead(ids: string[] | null = null) {
    let changed = false;
    this.writeChain = this.writeChain.then(async () => {
      const state = await this.load();
      const targetIds = ids ? new Set(ids) : null;
      const markedAt = Date.now();
      state.notifications.items = state.notifications.items.map((entry) => {
        if (entry.readAt !== null) {
          return entry;
        }
        if (targetIds && !targetIds.has(entry.id)) {
          return entry;
        }
        changed = true;
        return {
          ...entry,
          readAt: markedAt
        };
      });
      if (changed) {
        await this.flush();
      }
    });
    await this.writeChain;
    return changed;
  }

  async clearNotifications() {
    let changed = false;
    this.writeChain = this.writeChain.then(async () => {
      const state = await this.load();
      if (state.notifications.items.length === 0) {
        return;
      }
      changed = true;
      state.notifications.items = [];
      await this.flush();
    });
    await this.writeChain;
    return changed;
  }

  async getNotificationSettings() {
    const state = await this.load();
    return normalizeNotificationSettings(state.notifications.settings);
  }

  async updateNotificationSettings(nextSettings: Partial<NotificationSettings>) {
    let updatedSettings = { ...DEFAULT_NOTIFICATION_SETTINGS };
    this.writeChain = this.writeChain.then(async () => {
      const state = await this.load();
      state.notifications.settings = normalizeNotificationSettings({
        ...state.notifications.settings,
        ...nextSettings
      });
      updatedSettings = { ...state.notifications.settings };
      await this.flush();
    });
    await this.writeChain;
    return updatedSettings;
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

  async getSessionMeta(threadId: string) {
    const state = await this.load();
    return state.sessionMetaByThreadId[threadId] ?? { pinned: false, tags: [] };
  }

  async getAllSessionMeta() {
    const state = await this.load();
    return { ...state.sessionMetaByThreadId };
  }

  async updateSessionMeta(threadId: string, patch: Partial<{ pinned: boolean; tags: string[] }>) {
    let nextMeta = { pinned: false, tags: [] as string[] };
    this.writeChain = this.writeChain.then(async () => {
      const state = await this.load();
      const current = state.sessionMetaByThreadId[threadId] ?? { pinned: false, tags: [] };
      nextMeta = {
        pinned: typeof patch.pinned === "boolean" ? patch.pinned : Boolean(current.pinned),
        tags: Array.isArray(patch.tags)
          ? [...new Set(patch.tags.map((entry) => entry.trim()).filter((entry) => entry.length > 0))]
          : [...current.tags]
      };
      if (!nextMeta.pinned && nextMeta.tags.length === 0) {
        delete state.sessionMetaByThreadId[threadId];
      } else {
        state.sessionMetaByThreadId[threadId] = nextMeta;
      }
      await this.flush();
    });
    await this.writeChain;
    return nextMeta;
  }

  async getKnownSessionTags() {
    const state = await this.load();
    return [...new Set(Object.values(state.sessionMetaByThreadId).flatMap((entry) => entry.tags))].sort((left, right) =>
      left.localeCompare(right)
    );
  }

  async getSavedSessionFilters() {
    const state = await this.load();
    return [...state.savedSessionFilters];
  }

  async saveSessionFilter(filter: SavedSessionFilter) {
    let savedFilters: SavedSessionFilter[] = [];
    this.writeChain = this.writeChain.then(async () => {
      const state = await this.load();
      const nextFilter: SavedSessionFilter = {
        ...filter,
        name: filter.name.trim(),
        tags: [...new Set(filter.tags.map((entry) => entry.trim()).filter((entry) => entry.length > 0))]
      };
      state.savedSessionFilters = [
        nextFilter,
        ...state.savedSessionFilters.filter((entry) => entry.id !== nextFilter.id)
      ].slice(0, 40);
      savedFilters = [...state.savedSessionFilters];
      await this.flush();
    });
    await this.writeChain;
    return savedFilters;
  }

  async deleteSessionFilter(filterId: string) {
    let savedFilters: SavedSessionFilter[] = [];
    this.writeChain = this.writeChain.then(async () => {
      const state = await this.load();
      state.savedSessionFilters = state.savedSessionFilters.filter((entry) => entry.id !== filterId);
      savedFilters = [...state.savedSessionFilters];
      await this.flush();
    });
    await this.writeChain;
    return savedFilters;
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

  async getSessionHighlight(threadId: string) {
    const state = await this.load();
    return state.highlightsByThreadId[threadId] ?? null;
  }

  async getSessionHighlights() {
    const state = await this.load();
    return { ...state.highlightsByThreadId };
  }

  async setSessionHighlight(threadId: string, highlight: SessionSummaryHighlight | null) {
    let changed = false;
    this.writeChain = this.writeChain.then(async () => {
      const state = await this.load();
      const current = state.highlightsByThreadId[threadId] ?? null;
      const isSame = current?.kind === highlight?.kind && (current?.at ?? null) === (highlight?.at ?? null);
      if (isSame) {
        return;
      }

      changed = true;
      if (highlight) {
        state.highlightsByThreadId[threadId] = {
          kind: highlight.kind,
          at: highlight.at
        };
      } else {
        delete state.highlightsByThreadId[threadId];
      }
      await this.flush();
    });
    await this.writeChain;
    return changed;
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
