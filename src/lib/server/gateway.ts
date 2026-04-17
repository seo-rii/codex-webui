import { execFile } from "node:child_process";
import { randomUUID } from "node:crypto";
import { createReadStream } from "node:fs";
import { readdir } from "node:fs/promises";
import path from "node:path";
import readline from "node:readline";
import type { ArenaContestant, ArenaListPayload, ArenaRun } from "$lib/arena-types";
import type {
  AutomationDefinition,
  AutomationRun,
  AppNotification,
  AppConfigPayload,
  AttachmentRecord,
  CodexItem,
  CodexTurn,
  CollaborationModeOption,
  GlobalStreamEvent,
  NotificationEventType,
  NotificationListPayload,
  NotificationSettings,
  PendingServerRequest,
  PromptPreset,
  SavedSessionFilter,
  SessionDetailPayload,
  SessionDraftPayload,
  SessionForkMode,
  SessionForkPayload,
  SessionItemDetailPayload,
  SessionListPayload,
  SessionPreferences,
  SessionQueueItem,
  SessionQueuePayload,
  SessionSummaryFilter,
  SessionSummary,
  SessionSummaryHighlight,
  StartupScheduledShutdownAlert,
  SessionTurnSearchPayload,
  SessionTurnPayload,
  SessionTurnsPagePayload,
  StreamEvent,
  ThreadTokenUsage
} from "$lib/types";
import { stripAttachmentPreamble } from "$lib/attachments";
import { createAppError, parseAppError } from "$lib/errors";

import { listAttachments } from "./attachments";
import { arenaStore } from "./arena-store";
import { AppServerClient } from "./app-server/client";
import { configTomlPath, syncCodexTomlWithPreferences } from "./codex-config";
import { getCurrentRuntimeProfile, getRuntimeConfig, getRuntimeProfile, type RuntimeProfileConfig } from "./env";
import { buildSandboxPolicy, listDirectoryPayload, resolveAllowedDirectory, sanitizeFileName } from "./fs";
import { createGitWorktree, resolveGitRepository } from "./git";
import { runWithProfile } from "./profile-context";
import { sessionIndexClient, type IndexedSessionSummary } from "./session-index";
import { uiStateStore } from "./store";

type InternalPendingRequest = PendingServerRequest & {
  rawId: string | number;
};

type SessionHydrationState = SessionDetailPayload["hydration"] & {
  turns: SessionDetailPayload["thread"]["turns"];
};

const SESSION_WINDOW_SIZE = 20;
const RECENT_LIVE_THREAD_WINDOW_SIZE = 120;
const RECENT_LIVE_THREAD_CACHE_TTL_MS = 2500;
const LOADED_THREAD_CACHE_TTL_MS = 1500;
const ACCOUNT_STATE_CACHE_TTL_MS = 30_000;
const INVALID_REFRESH_TOKEN_PATTERN = /(TokenRefreshFailed|invalid_grant:\s*Invalid refresh token)/iu;
const HYDRATION_CACHE_LIMIT = 2;
const DEFERRED_ITEM_TYPES = new Set(["commandExecution", "fileChange", "mcpToolCall", "dynamicToolCall", "webSearch"]);
const DEFAULT_THREAD_NAME = "New thread";
const LIVE_THREAD_STATUSES = new Set(["running", "active"]);
const DEFAULT_NOTIFICATION_LIMIT = 80;
const DEFAULT_AUTOMATION_RUN_HISTORY_LIMIT = 40;

type GatewayAccountState = {
  account: Record<string, unknown>;
  requiresOpenaiAuth: boolean;
};

function asRecord(value: unknown) {
  return (value ?? {}) as Record<string, unknown>;
}

function asText(value: unknown) {
  if (typeof value === "string") {
    return value;
  }

  if (!value || typeof value !== "object") {
    return null;
  }

  const record = asRecord(value);
  for (const key of ["text", "title", "value", "name", "status", "state"]) {
    if (typeof record[key] === "string") {
      return String(record[key]);
    }
  }

  return null;
}

function normalizeThreadStatus(value: unknown) {
  if (typeof value === "string") {
    return value;
  }

  if (!value || typeof value !== "object") {
    return null;
  }

  const record = asRecord(value);
  if (typeof record.type === "string") {
    return String(record.type);
  }

  return asText(value);
}

function isLiveThreadStatus(value: unknown) {
  return LIVE_THREAD_STATUSES.has(String(normalizeThreadStatus(value) ?? ""));
}

function getThreadSpawnMetadata(source: unknown) {
  return asRecord(asRecord(asRecord(source).subagent).thread_spawn);
}

function getThreadAgentNickname(thread: Record<string, unknown>) {
  return (
    asText(thread.agentNickname) ??
    asText(thread.agent_nickname) ??
    asText(getThreadSpawnMetadata(thread.source).agent_nickname)
  );
}

function getThreadAgentRole(thread: Record<string, unknown>) {
  return asText(thread.agentRole) ?? asText(thread.agent_role) ?? asText(getThreadSpawnMetadata(thread.source).agent_role);
}

function isSubagentThread(thread: Record<string, unknown>) {
  const subagentSource = asRecord(asRecord(thread.source).subagent);
  return Boolean(
    Object.keys(subagentSource).length > 0 || (getThreadAgentNickname(thread) ?? "").trim() || (getThreadAgentRole(thread) ?? "").trim()
  );
}

function normalizeThread(thread: Record<string, unknown>): Record<string, unknown> {
  return {
    ...thread,
    name: asText(thread.name),
    preview: stripAttachmentPreamble(asText(thread.preview) ?? ""),
    status: normalizeThreadStatus(thread.status),
    isSubagent: isSubagentThread(thread),
    agentNickname: getThreadAgentNickname(thread),
    agentRole: getThreadAgentRole(thread),
    turns: Array.isArray(thread.turns) ? thread.turns : []
  };
}

function asNumber(value: unknown, fallback = 0) {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

function normalizeTokenUsage(value: unknown): ThreadTokenUsage | null {
  if (!value || typeof value !== "object") {
    return null;
  }

  const record = asRecord(value);
  const normalizeBreakdown = (input: unknown) => {
    const breakdown = asRecord(input);
    return {
      totalTokens: asNumber(breakdown.totalTokens),
      inputTokens: asNumber(breakdown.inputTokens),
      cachedInputTokens: asNumber(breakdown.cachedInputTokens),
      outputTokens: asNumber(breakdown.outputTokens),
      reasoningOutputTokens: asNumber(breakdown.reasoningOutputTokens)
    };
  };

  return {
    total: normalizeBreakdown(record.total),
    last: normalizeBreakdown(record.last),
    modelContextWindow:
      typeof record.modelContextWindow === "number" && Number.isFinite(record.modelContextWindow)
        ? record.modelContextWindow
        : null
  };
}

function isUnmaterializedThreadError(error: unknown) {
  return error instanceof Error && /not materialized yet|includeTurns is unavailable before first user message/iu.test(error.message);
}

function isInvalidRefreshTokenError(error: unknown) {
  const message = error instanceof Error ? error.message : String(error ?? "");
  return INVALID_REFRESH_TOKEN_PATTERN.test(message);
}

function coercePreferences(preferences: Partial<SessionPreferences> | null | undefined) {
  const defaults = getCurrentRuntimeProfile().defaults;
  return {
    ...defaults,
    ...preferences
  } satisfies SessionPreferences;
}

function toSummary(
  thread: Record<string, unknown>,
  preferences: SessionPreferences | null,
  archived = false,
  queueCount = 0,
  highlight: SessionSummaryHighlight | null = null
): SessionSummary {
  const normalized = normalizeThread(thread);
  const preview = asText(normalized.preview) ?? "";
  return {
    id: String(normalized.id),
    name: getDisplayThreadName(asText(normalized.name), preview),
    preview,
    queueCount: Math.max(0, queueCount),
    highlight,
    pinned: false,
    tags: [],
    cwd: (normalized.cwd as string | null) ?? preferences?.cwd ?? getCurrentRuntimeProfile().defaults.cwd,
    archived,
    createdAt: Number(normalized.createdAt ?? 0),
    updatedAt: Number(normalized.updatedAt ?? 0),
    status: asText(normalized.status) ?? "unknown",
    isSubagent: Boolean(normalized.isSubagent),
    agentNickname: asText(normalized.agentNickname),
    agentRole: asText(normalized.agentRole),
    preferences
  };
}

function indexedSessionToSummary(
  indexed: IndexedSessionSummary,
  preferences: SessionPreferences | null,
  queueCount: number,
  highlight: SessionSummaryHighlight | null = null
): SessionSummary {
  return {
    id: indexed.id,
    name: indexed.name,
    preview: indexed.preview,
    queueCount: Math.max(0, queueCount),
    highlight,
    pinned: false,
    tags: [],
    cwd: indexed.cwd || preferences?.cwd || getCurrentRuntimeProfile().defaults.cwd,
    archived: false,
    createdAt: indexed.createdAt,
    updatedAt: indexed.updatedAt,
    status: indexed.status || "unknown",
    isSubagent: indexed.isSubagent,
    agentNickname: null,
    agentRole: null,
    preferences
  };
}

function isSubagentSessionSummary(
  session: Pick<SessionSummary, "isSubagent" | "agentNickname" | "agentRole"> | null | undefined
) {
  return Boolean(session?.isSubagent || (session?.agentNickname ?? "").trim() || (session?.agentRole ?? "").trim());
}

function getSessionSortPriority(status: string | null | undefined) {
  return status === "running" || status === "active" ? 1 : 0;
}

function compareSessionSummaries(left: SessionSummary, right: SessionSummary) {
  const pinnedDifference = Number(Boolean(right.pinned)) - Number(Boolean(left.pinned));
  if (pinnedDifference !== 0) {
    return pinnedDifference;
  }

  const priorityDifference = getSessionSortPriority(right.status) - getSessionSortPriority(left.status);
  if (priorityDifference !== 0) {
    return priorityDifference;
  }

  const updatedDifference = (right.updatedAt || 0) - (left.updatedAt || 0);
  if (updatedDifference !== 0) {
    return updatedDifference;
  }

  const createdDifference = (right.createdAt || 0) - (left.createdAt || 0);
  if (createdDifference !== 0) {
    return createdDifference;
  }

  return 0;
}

function mergeIndexedSessionsWithRecentLiveSessions(
  indexedEntries: IndexedSessionSummary[],
  preferences: Record<string, SessionPreferences>,
  sessionMetaByThreadId: Record<string, { pinned: boolean; tags: string[] }>,
  queueCounts: Record<string, number>,
  highlightsByThreadId: Record<string, SessionSummaryHighlight>,
  recentLiveSessions: Map<string, SessionSummary>,
  limit: number,
  matcher: ((session: SessionSummary) => boolean) | null = null,
  includeUnindexedLiveSessions = false
) {
  const sessions = indexedEntries
    .filter((entry) => !entry.isSubagent)
    .map((entry) => {
      const stored = preferences[entry.id];
      const live = recentLiveSessions.get(entry.id);
      if (!live) {
        return applySessionMeta(
          indexedSessionToSummary(
            entry,
            stored ? coercePreferences(stored) : null,
            queueCounts[entry.id] ?? 0,
            highlightsByThreadId[entry.id] ?? null
          ),
          sessionMetaByThreadId[entry.id] ?? null
        );
      }

      return applySessionMeta(
        {
          ...live,
          name: !isPlaceholderThreadName(live.name) ? live.name : entry.name,
          preview: live.preview?.trim() ? live.preview : entry.preview,
          cwd: live.cwd || entry.cwd,
          createdAt: live.createdAt || entry.createdAt,
          updatedAt: Math.max(live.updatedAt || 0, entry.updatedAt || 0)
        } satisfies SessionSummary,
        sessionMetaByThreadId[entry.id] ?? null
      );
    })
    .filter((session) => !isSubagentSessionSummary(session));

  if (includeUnindexedLiveSessions) {
    const seen = new Set(sessions.map((session) => session.id));
    for (const live of recentLiveSessions.values()) {
      if (seen.has(live.id) || isSubagentSessionSummary(live)) {
        continue;
      }
      if (matcher && !matcher(live)) {
        continue;
      }
      sessions.push(applySessionMeta(live, sessionMetaByThreadId[live.id] ?? null));
      seen.add(live.id);
    }
  }

  return sessions.sort(compareSessionSummaries).slice(0, limit);
}

function findActiveTurnId(thread: Record<string, unknown>) {
  const turns = Array.isArray(thread.turns) ? (thread.turns as Array<Record<string, unknown>>) : [];
  const activeTurn = [...turns].reverse().find((turn) => String(turn.status ?? "") === "inProgress");
  return activeTurn ? String(activeTurn.id ?? "") : null;
}

function resolveTrackedActiveTurnId(thread: Record<string, unknown>, fallback: string | null = null) {
  const activeTurnId = findActiveTurnId(thread);
  if (activeTurnId) {
    return activeTurnId;
  }

  const turns = Array.isArray(thread.turns) ? (thread.turns as Array<Record<string, unknown>>) : [];
  if (turns.length > 0) {
    return null;
  }

  return fallback;
}

function getThreadActiveTurnId(thread: Record<string, unknown>, fallback: string | null = null) {
  const status = String(normalizeThreadStatus(thread.status) ?? "");
  if (!LIVE_THREAD_STATUSES.has(status)) {
    return null;
  }

  return resolveTrackedActiveTurnId(thread, fallback);
}

function normalizeStoppedTurn(turn: CodexTurn): CodexTurn {
  if (String(turn.status ?? "") !== "inProgress") {
    return turn;
  }

  return {
    ...turn,
    status: "stopped"
  };
}

function itemCacheKey(turnId: string, itemId: string) {
  return `${turnId}:${itemId}`;
}

function summarizeCommand(command: unknown) {
  if (Array.isArray(command)) {
    return command.map((entry) => String(entry)).join(" ").trim();
  }
  return asText(command) ?? "";
}

function summarizeToolInvocation(invocation: unknown) {
  const record = asRecord(invocation);
  const toolName =
    asText(record.toolName) ??
    asText(record.name) ??
    asText(record.tool) ??
    asText(record.method) ??
    asText(record.displayName);
  const serverName = asText(record.serverName) ?? asText(record.server);
  if (toolName && serverName) {
    return `${serverName} · ${toolName}`;
  }
  return toolName ?? serverName ?? "";
}

function normalizeSessionTitleSource(prompt: string) {
  return stripAttachmentPreamble(prompt).replace(/\s+/g, " ").trim();
}

function buildAutomationThreadName(name: string) {
  return `Automation · ${name.trim()}`;
}

function buildAutomationWorktreeName(name: string) {
  return sanitizeFileName(name.trim().toLowerCase()).slice(0, 48) || "automation";
}

function isPlaceholderThreadName(name: string | null | undefined) {
  const normalized = (name ?? "").trim();
  return !normalized || normalized === DEFAULT_THREAD_NAME;
}

function inferSessionDisplayTitle(prompt: string) {
  const normalized = normalizeSessionTitleSource(prompt);
  if (!normalized) {
    return null;
  }

  let candidate =
    normalized.split(/\r?\n/u, 1)[0]?.split(/(?<=[.?!])\s+/u, 1)[0]?.split(/\s[-:|]\s/u, 1)[0]?.trim() ?? normalized;

  candidate = candidate
    .replace(/^[#>*`\-.\d()\[\]\s]+/u, "")
    .replace(/\s+/g, " ")
    .replace(
      /(해줘|해주세요|해 줘|고쳐줘|고쳐 줘|수정해줘|수정해 줘|추가해줘|추가해 줘|구현해줘|구현해 줘|만들어줘|만들어 줘|계속 작업해|계속 진행해|계속해|부탁해|please|can you|could you|help me)\s*$/iu,
      ""
    )
    .replace(/[.?!…。]+$/u, "")
    .trim();

  if (!candidate) {
    candidate = normalized;
  }

  return candidate.length > 60 ? `${candidate.slice(0, 60).trimEnd()}...` : candidate;
}

function extractDraftTextFromItem(item: CodexItem) {
  const flatten = (value: unknown): string => {
    if (typeof value === "string") {
      return value;
    }
    if (Array.isArray(value)) {
      return value.map((entry) => flatten(entry)).filter((entry) => entry.trim().length > 0).join("\n\n");
    }
    if (!value || typeof value !== "object") {
      return "";
    }

    const record = asRecord(value);
    return (
      flatten(record.text) ||
      flatten(record.content) ||
      flatten(record.value) ||
      flatten(record.message) ||
      flatten(record.title) ||
      flatten(record.path) ||
      ""
    );
  };

  const content = Array.isArray(item.content) ? (item.content as Array<Record<string, unknown>>) : [];
  const fragments = content.map((entry) => flatten(entry)).filter((entry) => entry.trim().length > 0);
  const contentText = stripAttachmentPreamble(fragments.join("\n\n")).trim();
  if (contentText) {
    return contentText;
  }

  return stripAttachmentPreamble(flatten(item.text) || flatten(item.message) || flatten(item.value) || flatten(item)).trim();
}

function inferPersistedSessionTitle(prompt: string) {
  const normalized = normalizeSessionTitleSource(prompt);
  const title = inferSessionDisplayTitle(prompt);
  if (!title) {
    return null;
  }
  return title === normalized ? null : title;
}

function getDisplayThreadName(name: string | null | undefined, preview: string | null | undefined) {
  if (!isPlaceholderThreadName(name)) {
    return name?.trim() ?? null;
  }
  return inferSessionDisplayTitle(preview ?? "");
}

function applySessionMeta(summary: SessionSummary, meta: { pinned: boolean; tags: string[] } | null | undefined) {
  return {
    ...summary,
    pinned: Boolean(meta?.pinned),
    tags: Array.isArray(meta?.tags) ? [...meta.tags] : []
  } satisfies SessionSummary;
}

function buildArenaThreadName(prompt: string, label: string) {
  const title = inferSessionDisplayTitle(prompt) ?? "Arena run";
  return `Arena · ${title} · ${label}`.slice(0, 120);
}

function extractArenaResponse(turns: CodexTurn[]) {
  for (const turn of [...turns].reverse()) {
    for (const item of [...turn.items].reverse()) {
      if (item.type !== "agentMessage") {
        continue;
      }
      const text = asText(item.text)?.trim();
      if (text) {
        return text;
      }
    }
  }

  return null;
}

function matchesSessionFilter(session: SessionSummary, filter: SessionSummaryFilter | null) {
  if (!filter) {
    return true;
  }

  if (filter.pinnedOnly && !session.pinned) {
    return false;
  }
  if (filter.runningOnly && getSessionSortPriority(session.status) === 0) {
    return false;
  }
  if (filter.queuedOnly && session.queueCount <= 0) {
    return false;
  }
  if (filter.highlight !== "all" && session.highlight?.kind !== filter.highlight) {
    return false;
  }

  const requiredTags = filter.tags.map((entry) => entry.trim()).filter((entry) => entry.length > 0);
  if (requiredTags.length > 0) {
    const sessionTags = new Set(session.tags.map((entry) => entry.trim()));
    if (!requiredTags.every((tag) => sessionTags.has(tag))) {
      return false;
    }
  }

  return true;
}

function normalizeSessionFilter(filter: Partial<SessionSummaryFilter> | null | undefined): SessionSummaryFilter | null {
  if (!filter) {
    return null;
  }

  return {
    pinnedOnly: Boolean(filter.pinnedOnly),
    runningOnly: Boolean(filter.runningOnly),
    queuedOnly: Boolean(filter.queuedOnly),
    highlight: filter.highlight === "attention" || filter.highlight === "completed" ? filter.highlight : "all",
    tags: Array.isArray(filter.tags) ? [...new Set(filter.tags.map((entry) => entry.trim()).filter((entry) => entry.length > 0))] : []
  };
}

function buildNotificationPayload(
  type: NotificationEventType,
  sessionId: string | null,
  sessionName: string | null,
  payload: Record<string, unknown> = {}
) {
  return {
    id: randomUUID(),
    type,
    createdAt: Date.now(),
    readAt: null,
    sessionId,
    sessionName,
    payload
  } satisfies AppNotification;
}

function describeNotification(notification: AppNotification) {
  if (notification.type === "sessionCompleted") {
    const sessionLabel = notification.sessionName?.trim() || "Codex session";
    return {
      title: "Codex task completed",
      body: `${sessionLabel} finished and is ready for review.`
    };
  }

  if (notification.type === "sessionAttention") {
    const sessionLabel = notification.sessionName?.trim() || "Codex session";
    return {
      title: "Codex needs input",
      body: `${sessionLabel} is waiting for a decision or input.`
    };
  }

  if (notification.type === "queueDispatchFailed") {
    const sessionLabel = notification.sessionName?.trim() || "Codex session";
    return {
      title: "Queued follow-up failed",
      body: `${sessionLabel} could not send the next queued message automatically.`
    };
  }

  return {
    title: "Shutdown scheduled",
    body: `The server will shut down in ${String(notification.payload.delaySeconds ?? 0)} seconds once work remains idle.`
  };
}

export class CodexGateway {
  private readonly client: AppServerClient;
  private readonly runtimeConfig = getRuntimeConfig();
  private readonly streamSubscribers = new Map<string, Set<(event: StreamEvent) => void>>();
  private readonly globalSubscribers = new Set<(event: GlobalStreamEvent) => void>();
  private readonly pendingRequests = new Map<string, Map<string, InternalPendingRequest>>();
  private readonly activeTurns = new Map<string, string>();
  private readonly tokenUsageByThread = new Map<string, ThreadTokenUsage>();
  private readonly sessionHydrations = new Map<string, SessionHydrationState>();
  private readonly hydrationJobs = new Map<string, Promise<void>>();
  private readonly rolloutPaths = new Map<string, string | null>();
  private readonly itemDetailsByThread = new Map<string, Map<string, CodexItem>>();
  private readonly fullTurnsByThread = new Map<string, Map<string, CodexTurn>>();
  private readonly drainingQueues = new Set<string>();
  private readonly automationTimers = new Map<string, ReturnType<typeof setTimeout>>();
  private recentLiveSessions = new Map<string, SessionSummary>();
  private recentLiveSessionsLoadedAt = 0;
  private recentLiveSessionsPromise: Promise<Map<string, SessionSummary>> | null = null;
  private loadedThreadIds = new Set<string>();
  private loadedThreadIdsLoadedAt = 0;
  private loadedThreadIdsPromise: Promise<Set<string>> | null = null;
  private accountStateCache: { value: GatewayAccountState; expiresAt: number } | null = null;
  private shutdownTimer: ReturnType<typeof setTimeout> | null = null;
  private shutdownScheduledFor: number | null = null;

  constructor(private readonly profile: RuntimeProfileConfig) {
    this.client = new AppServerClient(profile);
    void uiStateStore.markQueuesPendingResume();
    void this.restorePersistedShutdownState();
    void this.restoreAutomationSchedules();
    this.client.onNotification((payload) => {
      void runWithProfile(this.profile.id, () => this.handleNotification(payload.method, payload.params));
    });
    this.client.onServerRequest((payload) => {
      void runWithProfile(this.profile.id, () => this.handleServerRequest(payload));
    });
  }

  subscribe(threadId: string, listener: (event: StreamEvent) => void) {
    const subscribers = this.streamSubscribers.get(threadId) ?? new Set();
    subscribers.add(listener);
    this.streamSubscribers.set(threadId, subscribers);
    return () => {
      const current = this.streamSubscribers.get(threadId);
      current?.delete(listener);
      if (current?.size === 0) {
        this.streamSubscribers.delete(threadId);
      }
    };
  }

  subscribeGlobal(listener: (event: GlobalStreamEvent) => void) {
    this.globalSubscribers.add(listener);
    return () => {
      this.globalSubscribers.delete(listener);
    };
  }

  async getConfig(): Promise<AppConfigPayload> {
    const [modelsResponse, collaborationResponse, accountState] = await Promise.all([
      this.client.request("model/list", { includeHidden: false }),
      this.client.request("collaborationMode/list", {}),
      this.readAccountState()
    ]);
    const [pausedQueueEntries, globalState, notifications, savedFilters, knownTags, promptPresets, automations, automationRuns] = await Promise.all([
      uiStateStore.listResumePendingQueues(),
      uiStateStore.getGlobal(),
      uiStateStore.getNotifications(DEFAULT_NOTIFICATION_LIMIT),
      uiStateStore.getSavedSessionFilters(),
      uiStateStore.getKnownSessionTags(),
      uiStateStore.getPromptPresets(),
      uiStateStore.getAutomations(),
      uiStateStore.getAutomationRuns(DEFAULT_AUTOMATION_RUN_HISTORY_LIMIT)
    ]);
    const preferences: Record<string, SessionPreferences> = pausedQueueEntries.length > 0 ? await uiStateStore.getAll() : {};
    const indexedSessions = pausedQueueEntries.length > 0 ? await sessionIndexClient.list(this.profile.codexHome).catch(() => []) : [];
    const indexedById = new Map(indexedSessions.map((session) => [session.id, session]));
    const pausedQueues = await Promise.all(
      pausedQueueEntries.map(async (entry) => {
        const indexedSession = indexedById.get(entry.threadId) ?? null;
        let name = getDisplayThreadName(indexedSession?.name ?? null, indexedSession?.preview ?? null);
        let cwd = indexedSession?.cwd || preferences[entry.threadId]?.cwd || this.profile.defaults.cwd;

        if (!name || !indexedSession?.cwd) {
          try {
            const thread = await this.readThread(entry.threadId, false);
            name = getDisplayThreadName(asText(thread.name), asText(thread.preview)) ?? name;
            cwd = String(thread.cwd ?? cwd);
          } catch {
            // Keep whatever indexed metadata was available.
          }
        }

        return {
          sessionId: entry.threadId,
          name,
          cwd,
          pendingCount: entry.pendingCount,
          updatedAt: entry.updatedAt
        };
      })
    );

    const models = asRecord(modelsResponse).data as Array<Record<string, unknown>> | undefined;
    const collabModes = asRecord(collaborationResponse).data as Array<Record<string, unknown>> | undefined;
    const account = accountState.account;

    return {
      models:
        models?.map((model) => ({
          id: String(model.id),
          displayName: String(model.displayName ?? model.model ?? model.id),
          description: String(model.description ?? ""),
          defaultReasoningEffort: String(model.defaultReasoningEffort ?? "medium"),
          supportedReasoningEfforts: Array.isArray(model.supportedReasoningEfforts)
            ? model.supportedReasoningEfforts
                .map((entry) => asRecord(entry).reasoningEffort ?? asRecord(entry).effort ?? entry)
                .map((entry) => String(entry))
            : [],
          additionalSpeedTiers: Array.isArray(model.additionalSpeedTiers)
            ? model.additionalSpeedTiers.map((entry) => String(entry))
            : [],
          inputModalities: Array.isArray(model.inputModalities) ? model.inputModalities.map((entry) => String(entry)) : [],
          isDefault: Boolean(model.isDefault)
        })) ?? [],
      collaborationModes:
        collabModes?.map((mode) => ({
          name: String(mode.name ?? ""),
          mode: (mode.mode as CollaborationModeOption["mode"]) ?? null,
          model: (mode.model as string | null) ?? null,
          reasoning_effort: (mode.reasoning_effort as string | null) ?? null
        })) ?? [],
      allowedRoots: (await listDirectoryPayload(null)).allowedRoots,
      defaults: this.profile.defaults,
      paths: {
        codexHome: this.profile.codexHome,
        configFilePath: configTomlPath(this.profile.codexHome)
      },
      git: {
        discoveryDepth: this.runtimeConfig.gitDiscoveryDepth
      },
      systemShutdown: {
        available: this.runtimeConfig.systemShutdownEnabled,
        delaySeconds: this.runtimeConfig.systemShutdownDelaySeconds,
        armed: this.runtimeConfig.systemShutdownEnabled && globalState.shutdownAfterQueueCompletes
      },
      startup: {
        pausedQueues,
        scheduledShutdown:
          this.runtimeConfig.systemShutdownEnabled &&
          globalState.scheduledShutdown &&
          globalState.scheduledShutdown.scheduledFor > Date.now()
            ? globalState.scheduledShutdown
            : null
      },
      notifications: {
        unreadCount: notifications.unreadCount,
        settings: await uiStateStore.getNotificationSettings()
      },
      sessionOrganization: {
        savedFilters,
        knownTags
      },
      promptPresets,
      automations: {
        items: automations,
        recentRuns: automationRuns
      },
      account: {
        type: (account.type as "apiKey" | "chatgpt" | null) ?? null,
        email: (account.email as string | null) ?? null,
        planType: (account.planType as string | null) ?? null,
        requiresOpenaiAuth: accountState.requiresOpenaiAuth
      },
      profiles: this.runtimeConfig.profiles.map((profile) => ({
        id: profile.id,
        label: profile.label,
        codexHome: profile.codexHome,
        active: profile.id === this.profile.id
      }))
    };
  }

  async getNotifications(limit = DEFAULT_NOTIFICATION_LIMIT): Promise<NotificationListPayload> {
    return uiStateStore.getNotifications(limit);
  }

  async markNotificationsRead(ids: string[] | null = null) {
    const changed = await uiStateStore.markNotificationsRead(ids);
    const payload = await uiStateStore.getNotifications(DEFAULT_NOTIFICATION_LIMIT);
    if (changed) {
      this.emitGlobal({
        kind: "notification",
        method: "codex-webui/notificationStateUpdated",
        params: {
          unreadCount: payload.unreadCount
        }
      });
      void this.emitConfigUpdated();
    }
    return payload;
  }

  async clearNotifications() {
    const changed = await uiStateStore.clearNotifications();
    const payload = await uiStateStore.getNotifications(DEFAULT_NOTIFICATION_LIMIT);
    if (changed) {
      this.emitGlobal({
        kind: "notification",
        method: "codex-webui/notificationStateUpdated",
        params: {
          unreadCount: payload.unreadCount
        }
      });
      void this.emitConfigUpdated();
    }
    return payload;
  }

  async updateNotificationSettings(settings: Partial<NotificationSettings>) {
    const nextSettings = await uiStateStore.updateNotificationSettings(settings);
    const unreadCount = (await uiStateStore.getNotifications(1)).unreadCount;
    this.emitGlobal({
      kind: "notification",
      method: "codex-webui/notificationSettingsUpdated",
      params: {
        settings: nextSettings,
        unreadCount
      }
    });
    await this.emitConfigUpdated();
    return {
      settings: nextSettings,
      unreadCount
    };
  }

  async updateSessionOrganization(threadId: string, patch: Partial<{ pinned: boolean; tags: string[] }>) {
    const nextMeta = await uiStateStore.updateSessionMeta(threadId, patch);
    await this.emitSessionSummaryUpdated(threadId);
    await this.emitConfigUpdated();
    return {
      meta: nextMeta,
      knownTags: await uiStateStore.getKnownSessionTags()
    };
  }

  async saveSessionFilter(filter: SavedSessionFilter) {
    if (!filter.name.trim()) {
      throw new Error("Filter name is required.");
    }
    const savedFilters = await uiStateStore.saveSessionFilter({
      ...filter,
      name: filter.name.trim()
    });
    await this.emitConfigUpdated();
    return {
      savedFilters,
      knownTags: await uiStateStore.getKnownSessionTags()
    };
  }

  async deleteSessionFilter(filterId: string) {
    const savedFilters = await uiStateStore.deleteSessionFilter(filterId);
    await this.emitConfigUpdated();
    return {
      savedFilters,
      knownTags: await uiStateStore.getKnownSessionTags()
    };
  }

  async savePromptPreset(preset: PromptPreset) {
    if (!preset.name.trim()) {
      throw new Error("Preset name is required.");
    }
    if (!preset.prompt.trim()) {
      throw new Error("Preset prompt is required.");
    }

    const promptPresets = await uiStateStore.savePromptPreset({
      ...preset,
      name: preset.name.trim()
    });
    await this.emitConfigUpdated();
    return {
      promptPresets
    };
  }

  async deletePromptPreset(presetId: string) {
    const promptPresets = await uiStateStore.deletePromptPreset(presetId);
    await this.emitConfigUpdated();
    return {
      promptPresets
    };
  }

  async saveAutomation(automation: AutomationDefinition) {
    if (!automation.name.trim()) {
      throw new Error("Automation name is required.");
    }
    if (!automation.prompt.trim()) {
      throw new Error("Automation prompt is required.");
    }
    if (automation.target === "worktree" && !automation.repoPath?.trim()) {
      throw new Error("Worktree automations require a repository.");
    }

    const normalizedInterval =
      automation.scheduleMode === "interval" ? Math.max(1, Math.round(Number(automation.intervalMinutes ?? 0) || 0)) : null;
    if (automation.scheduleMode === "interval" && !normalizedInterval) {
      throw new Error("Automation interval must be at least 1 minute.");
    }

    const now = Date.now();
    const nextAutomation = {
      ...automation,
      name: automation.name.trim(),
      prompt: automation.prompt,
      repoPath: automation.repoPath?.trim() || null,
      cwd: automation.cwd?.trim() || null,
      intervalMinutes: normalizedInterval,
      nextRunAt:
        automation.enabled && automation.scheduleMode === "interval" && normalizedInterval
          ? now + normalizedInterval * 60_000
          : null
    } satisfies AutomationDefinition;

    const automations = await uiStateStore.saveAutomation(nextAutomation);
    this.scheduleAutomation(nextAutomation);
    await this.emitConfigUpdated();
    return {
      automations
    };
  }

  async deleteAutomation(automationId: string) {
    this.clearAutomationTimer(automationId);
    const automations = await uiStateStore.deleteAutomation(automationId);
    await this.emitConfigUpdated();
    return {
      automations
    };
  }

  async runAutomation(automationId: string, trigger: "manual" | "schedule" = "manual") {
    const automation = (await uiStateStore.getAutomations()).find((entry) => entry.id === automationId);
    if (!automation) {
      throw new Error("Automation not found.");
    }

    const runId = randomUUID();
    const now = Date.now();
    let worktreePath: string | null = null;
    let cwd = automation.cwd?.trim() || automation.repoPath?.trim() || this.profile.defaults.cwd;
    let gitRepoPath = automation.repoPath?.trim() || null;

    const run: AutomationRun = {
      id: runId,
      automationId: automation.id,
      automationName: automation.name,
      status: "running",
      trigger,
      sessionId: null,
      repoPath: gitRepoPath,
      cwd,
      worktreePath: null,
      startedAt: now,
      completedAt: null,
      error: null
    };
    await uiStateStore.saveAutomationRun(run);
    await this.emitConfigUpdated();

    try {
      if (automation.target === "worktree" && automation.repoPath) {
        const repo = await resolveGitRepository(automation.repoPath);
        const timeSuffix = new Date(now).toISOString().replace(/[:.]/gu, "-");
        worktreePath = path.join(
          path.dirname(repo.path),
          ".codex-webui-worktrees",
          buildAutomationWorktreeName(automation.name),
          timeSuffix
        );
        const branchName = `automation/${buildAutomationWorktreeName(automation.name)}-${timeSuffix.toLowerCase()}`;
        await createGitWorktree(repo.path, worktreePath, branchName, true, false);
        cwd = worktreePath;
        gitRepoPath = worktreePath;
      }

      const preferences = await this.preparePreferences({
        cwd,
        gitRepoPath,
        model: automation.model,
        effort: automation.effort,
        speed: automation.speed ?? undefined,
        mode: automation.mode ?? undefined
      });

      const session = await this.createSession(preferences, buildAutomationThreadName(automation.name));
      await uiStateStore.updateAutomationRun(runId, {
        sessionId: session.id,
        repoPath: gitRepoPath,
        cwd,
        worktreePath,
        status: "started"
      });
      await this.sendMessage(session.id, automation.prompt, [], preferences);

      const patch: Partial<AutomationDefinition> = {
        lastRunAt: now
      };
      if (automation.enabled && automation.scheduleMode === "interval" && automation.intervalMinutes) {
        patch.nextRunAt = now + automation.intervalMinutes * 60_000;
      }
      const updatedAutomation = await uiStateStore.updateAutomation(automation.id, patch);
      if (updatedAutomation) {
        this.scheduleAutomation(updatedAutomation);
      }

      await this.emitConfigUpdated();
      return {
        ok: true,
        session,
        run: await uiStateStore.updateAutomationRun(runId, {})
      };
    } catch (error) {
      await uiStateStore.updateAutomationRun(runId, {
        status: "failed",
        completedAt: Date.now(),
        error: error instanceof Error ? error.message : "Failed to run automation.",
        worktreePath
      });
      await this.emitConfigUpdated();
      throw error;
    }
  }

  async listArenaRuns(): Promise<ArenaListPayload> {
    const runs = await arenaStore.getRuns();
    const hydratedRuns = await Promise.all(
      runs.map(async (run) => {
        let changed = false;
        const contestants = await Promise.all(
          run.contestants.map(async (contestant) => {
            try {
              const thread = await this.readThread(contestant.sessionId, false);
              const status = String(normalizeThreadStatus(asRecord(thread).status) ?? contestant.status ?? "unknown");
              let response = contestant.response;
              if (!response && !isLiveThreadStatus(status)) {
                const hydration = await this.ensureSessionHistory(contestant.sessionId, thread);
                response = extractArenaResponse(hydration.turns);
              }
              const updatedAt = Math.max(contestant.updatedAt, Number(asRecord(thread).updatedAt ?? 0), Date.now());
              if (status !== contestant.status || response !== contestant.response || updatedAt !== contestant.updatedAt) {
                changed = true;
              }
              return {
                ...contestant,
                status,
                response,
                updatedAt
              } satisfies ArenaContestant;
            } catch {
              return contestant;
            }
          })
        );
        const status = contestants.some((contestant) => isLiveThreadStatus(contestant.status)) ? "running" : "completed";
        const updatedRun = {
          ...run,
          contestants,
          status,
          updatedAt: Math.max(run.updatedAt, ...contestants.map((contestant) => contestant.updatedAt))
        } satisfies ArenaRun;
        if (changed || updatedRun.status !== run.status || updatedRun.updatedAt !== run.updatedAt) {
          await arenaStore.updateRun(run.id, () => updatedRun);
        }
        return updatedRun;
      })
    );

    return {
      runs: hydratedRuns.sort((left, right) => right.updatedAt - left.updatedAt)
    };
  }

  async startArenaRun(
    prompt: string,
    contestants: Array<{
      model?: string;
      label?: string;
    }>,
    preferences: Partial<SessionPreferences>
  ) {
    const trimmedPrompt = prompt.trim();
    if (!trimmedPrompt) {
      throw new Error("Prompt is required.");
    }

    const normalizedContestants = contestants
      .map((contestant) => ({
        model: contestant.model?.trim() ?? "",
        label: contestant.label?.trim() ?? contestant.model?.trim() ?? ""
      }))
      .filter((contestant) => contestant.model.length > 0 && contestant.label.length > 0)
      .filter((contestant, index, collection) => collection.findIndex((entry) => entry.model === contestant.model) === index)
      .slice(0, 4);

    if (normalizedContestants.length < 2) {
      throw new Error("Choose at least two models for an arena run.");
    }

    const nextPreferences = await this.preparePreferences(preferences);
    const createdAt = Date.now();
    const arenaContestants: ArenaContestant[] = [];

    for (const contestant of normalizedContestants) {
      const session = await this.createSession(
        {
          ...nextPreferences,
          model: contestant.model
        },
        buildArenaThreadName(trimmedPrompt, contestant.label),
        { hiddenFromSidebar: true }
      );
      arenaContestants.push({
        id: randomUUID(),
        sessionId: session.id,
        model: contestant.model,
        label: contestant.label,
        status: "running",
        response: null,
        createdAt,
        updatedAt: createdAt
      });
    }

    const run = {
      id: randomUUID(),
      prompt: trimmedPrompt,
      cwd: nextPreferences.cwd,
      status: "running",
      createdAt,
      updatedAt: createdAt,
      contestants: arenaContestants
    } satisfies ArenaRun;
    await arenaStore.saveRun(run);

    await Promise.all(
      arenaContestants.map(async (contestant) => {
        try {
          await this.sendMessage(contestant.sessionId, trimmedPrompt, [], {
            ...nextPreferences,
            model: contestant.model
          });
        } catch (error) {
          await arenaStore.updateRun(run.id, (current) => {
            if (!current) {
              return current;
            }

            return {
              ...current,
              updatedAt: Date.now(),
              contestants: current.contestants.map((entry) =>
                entry.id === contestant.id
                  ? {
                      ...entry,
                      status: "failed",
                      response: error instanceof Error ? error.message : "Arena run failed.",
                      updatedAt: Date.now()
                    }
                  : entry
              )
            };
          });
        }
      })
    );

    return (await this.listArenaRuns()).runs.find((entry) => entry.id === run.id) ?? run;
  }

  async listSessions(
    archived = false,
    cursor: string | null = null,
    limit = SESSION_WINDOW_SIZE,
    filter: SessionSummaryFilter | null = null
  ): Promise<SessionListPayload> {
    if (!archived) {
      const [preferences, sessionMetaByThreadId, queueCounts, highlightsByThreadId, indexedPage, recentLiveSessions, hiddenSessionIds] = await Promise.all([
        uiStateStore.getAll(),
        uiStateStore.getAllSessionMeta(),
        uiStateStore.getQueueCounts(),
        uiStateStore.getSessionHighlights(),
        sessionIndexClient.page(this.profile.codexHome, cursor, limit, null),
        this.getRecentLiveSessionSummaries().catch(() => new Map<string, SessionSummary>()),
        arenaStore.getHiddenSessionIds()
      ]);

      return {
        sessions: mergeIndexedSessionsWithRecentLiveSessions(
          indexedPage.entries,
          preferences,
          sessionMetaByThreadId,
          queueCounts,
          highlightsByThreadId,
          recentLiveSessions,
          limit,
          null,
          cursor === null || cursor === "0"
        )
          .filter((session) => !hiddenSessionIds.has(session.id))
          .filter((session) => matchesSessionFilter(session, filter)),
        nextCursor: indexedPage.nextCursor
      };
    }

    const response = asRecord(await this.client.request("thread/list", { limit, archived, cursor }));
    const [preferences, sessionMetaByThreadId, highlightsByThreadId, hiddenSessionIds] = await Promise.all([
      uiStateStore.getAll(),
      uiStateStore.getAllSessionMeta(),
      uiStateStore.getSessionHighlights(),
      arenaStore.getHiddenSessionIds()
    ]);
    const threads = (response.data as Array<Record<string, unknown>> | undefined) ?? [];
    return {
      sessions: threads
        .filter((thread) => !isSubagentThread(thread))
        .map((thread) => {
          const threadId = String(thread.id);
          const stored = preferences[threadId];
          return applySessionMeta(
            toSummary(thread, stored ? coercePreferences(stored) : null, archived, 0, highlightsByThreadId[threadId] ?? null),
            sessionMetaByThreadId[threadId] ?? null
          );
        })
        .filter((session) => !isSubagentSessionSummary(session))
        .filter((session) => !hiddenSessionIds.has(session.id))
        .filter((session) => matchesSessionFilter(session, filter))
        .sort(compareSessionSummaries),
      nextCursor: typeof response.nextCursor === "string" && response.nextCursor.trim() ? String(response.nextCursor) : null
    };
  }

  async searchSessions(
    query: string,
    scope: "summary" | "full",
    archived = false,
    cursor: string | null = null,
    limit = SESSION_WINDOW_SIZE,
    filter: SessionSummaryFilter | null = null
  ): Promise<SessionListPayload> {
    const needle = query.trim().toLowerCase();
    if (!needle) {
      return this.listSessions(archived, cursor, limit, filter);
    }

    if (!archived && scope === "summary") {
      const [preferences, sessionMetaByThreadId, queueCounts, highlightsByThreadId, indexedPage, recentLiveSessions, hiddenSessionIds] = await Promise.all([
        uiStateStore.getAll(),
        uiStateStore.getAllSessionMeta(),
        uiStateStore.getQueueCounts(),
        uiStateStore.getSessionHighlights(),
        sessionIndexClient.page(this.profile.codexHome, cursor, limit, needle),
        this.getRecentLiveSessionSummaries().catch(() => new Map<string, SessionSummary>()),
        arenaStore.getHiddenSessionIds()
      ]);

      return {
        sessions: mergeIndexedSessionsWithRecentLiveSessions(
          indexedPage.entries,
          preferences,
          sessionMetaByThreadId,
          queueCounts,
          highlightsByThreadId,
          recentLiveSessions,
          limit,
          (session) => `${session.name ?? ""}
${session.preview ?? ""}`.toLowerCase().includes(needle),
          cursor === null || cursor === "0"
        )
          .filter((session) => !hiddenSessionIds.has(session.id))
          .filter((session) => matchesSessionFilter(session, filter)),
        nextCursor: indexedPage.nextCursor
      };
    }

    const sessions = await this.collectListableSessions(archived, filter);
    sessions.sort(compareSessionSummaries);
    const matches: SessionSummary[] = [];
    for (const session of sessions) {
      const summaryHaystack = `${session.name ?? ""}\n${session.preview ?? ""}`.toLowerCase();
      if (summaryHaystack.includes(needle)) {
        matches.push(session);
        continue;
      }

      if (scope !== "full") {
        continue;
      }

      try {
        const thread = await this.readThread(session.id, true);
        if (JSON.stringify(thread.turns ?? []).toLowerCase().includes(needle)) {
          matches.push(session);
        }
      } catch {
        // Ignore full-text search misses for threads that cannot be materialized.
      }
    }

    const start = Math.max(0, Number.parseInt(cursor ?? "0", 10) || 0);
    const nextIndex = start + limit;
    return {
      sessions: matches.slice(start, nextIndex),
      nextCursor: nextIndex < matches.length ? String(nextIndex) : null
    };
  }

  private async collectListableSessions(archived: boolean, filter: SessionSummaryFilter | null = null): Promise<SessionSummary[]> {
    const [preferences, sessionMetaByThreadId, queueCounts, highlightsByThreadId, hiddenSessionIds] = await Promise.all([
      uiStateStore.getAll(),
      uiStateStore.getAllSessionMeta(),
      uiStateStore.getQueueCounts(),
      uiStateStore.getSessionHighlights(),
      arenaStore.getHiddenSessionIds()
    ]);
    if (archived) {
      return this.listAllThreadSessions(true, preferences, sessionMetaByThreadId, queueCounts, highlightsByThreadId, hiddenSessionIds, filter);
    }

    const indexedSessions = (await sessionIndexClient.list(this.profile.codexHome).catch(() => [])).filter(
      (session) => !session.isSubagent
    );
    const liveThreads = await this.listAllThreadSessions(
      false,
      preferences,
      sessionMetaByThreadId,
      queueCounts,
      highlightsByThreadId,
      hiddenSessionIds,
      filter
    );
    const liveById = new Map(liveThreads.map((session) => [session.id, session]));
    const mergedSessions = indexedSessions.filter((session) => !hiddenSessionIds.has(session.id)).map((session) => {
      const live = liveById.get(session.id);
      if (live) {
        return {
          ...live,
          name: !isPlaceholderThreadName(live.name) ? live.name : session.name,
          preview: live.preview?.trim() ? live.preview : session.preview,
          cwd: live.cwd || session.cwd,
          createdAt: live.createdAt || session.createdAt,
          updatedAt: Math.max(live.updatedAt || 0, session.updatedAt || 0)
        };
      }

      const stored = preferences[session.id];
      const summary = {
        id: session.id,
        name: session.name,
        preview: session.preview,
        queueCount: queueCounts[session.id] ?? 0,
        highlight: highlightsByThreadId[session.id] ?? null,
        pinned: false,
        tags: [],
        cwd: session.cwd || stored?.cwd || this.profile.defaults.cwd,
        archived: false,
        createdAt: session.createdAt,
        updatedAt: session.updatedAt,
        status: session.status || "unknown",
        isSubagent: session.isSubagent,
        agentNickname: null,
        agentRole: null,
        preferences: stored ? coercePreferences(stored) : null
      } satisfies SessionSummary;
      return applySessionMeta(summary, sessionMetaByThreadId[session.id] ?? null);
    });

    for (const live of liveThreads) {
      if (!mergedSessions.some((session) => session.id === live.id)) {
        mergedSessions.push(live);
      }
    }

    const dedupedSessions = mergedSessions.filter(
      (session, index, collection) => collection.findIndex((candidate) => candidate.id === session.id) === index
    );
    const visibleSessions = dedupedSessions
      .filter((session) => !hiddenSessionIds.has(session.id))
      .filter((session) => !isSubagentSessionSummary(session))
      .filter((session) => matchesSessionFilter(session, filter));
    visibleSessions.sort(compareSessionSummaries);
    return visibleSessions;
  }

  private async listAllThreadSessions(
    archived: boolean,
    preferences: Record<string, Awaited<ReturnType<typeof uiStateStore.getAll>>[string]>,
    sessionMetaByThreadId: Record<string, { pinned: boolean; tags: string[] }>,
    queueCounts: Record<string, number>,
    highlightsByThreadId: Record<string, SessionSummaryHighlight>,
    hiddenSessionIds: Set<string>,
    filter: SessionSummaryFilter | null = null
  ): Promise<SessionSummary[]> {
    const sessions: SessionSummary[] = [];
    let cursor: string | null = null;

    do {
      const response = asRecord(await this.client.request("thread/list", { limit: 200, archived, cursor }));
      const threads = (response.data as Array<Record<string, unknown>> | undefined) ?? [];
      sessions.push(
        ...threads
          .filter((thread) => !isSubagentThread(thread))
          .map((thread) => {
            const threadId = String(thread.id);
            const stored = preferences[threadId];
            return applySessionMeta(
              toSummary(
                thread,
                stored ? coercePreferences(stored) : null,
                archived,
                queueCounts[threadId] ?? 0,
                highlightsByThreadId[threadId] ?? null
              ),
              sessionMetaByThreadId[threadId] ?? null
            );
          })
          .filter((session) => !isSubagentSessionSummary(session))
          .filter((session) => !hiddenSessionIds.has(session.id))
          .filter((session) => matchesSessionFilter(session, filter))
      );
      cursor = typeof response.nextCursor === "string" && response.nextCursor.trim() ? String(response.nextCursor) : null;
    } while (cursor);

    return sessions;
  }

  private async getRecentLiveSessionSummaries() {
    const now = Date.now();
    if (this.recentLiveSessions.size > 0 && now - this.recentLiveSessionsLoadedAt < RECENT_LIVE_THREAD_CACHE_TTL_MS) {
      return this.recentLiveSessions;
    }

    if (this.recentLiveSessionsPromise) {
      return this.recentLiveSessionsPromise;
    }

    this.recentLiveSessionsPromise = (async () => {
      const [preferences, sessionMetaByThreadId, queueCounts, highlightsByThreadId, hiddenSessionIds] = await Promise.all([
        uiStateStore.getAll(),
        uiStateStore.getAllSessionMeta(),
        uiStateStore.getQueueCounts(),
        uiStateStore.getSessionHighlights(),
        arenaStore.getHiddenSessionIds()
      ]);
      const response = asRecord(
        await this.client.request("thread/list", { limit: RECENT_LIVE_THREAD_WINDOW_SIZE, archived: false, cursor: null })
      );
      const threads = (response.data as Array<Record<string, unknown>> | undefined) ?? [];
      const nextRecentLiveSessions = new Map(
        threads
          .filter((thread) => !isSubagentThread(thread))
          .map((thread) => {
            const threadId = String(thread.id);
            const stored = preferences[threadId];
            const summary = applySessionMeta(
              toSummary(
                thread,
                stored ? coercePreferences(stored) : null,
                false,
                queueCounts[threadId] ?? 0,
                highlightsByThreadId[threadId] ?? null
              ),
              sessionMetaByThreadId[threadId] ?? null
            );
            return [threadId, summary] as const;
          })
          .filter(([threadId]) => !hiddenSessionIds.has(threadId))
      );
      this.recentLiveSessions = nextRecentLiveSessions;
      this.recentLiveSessionsLoadedAt = Date.now();
      return nextRecentLiveSessions;
    })().finally(() => {
      this.recentLiveSessionsPromise = null;
    });

    return this.recentLiveSessionsPromise;
  }

  async archiveSession(threadId: string) {
    if (await this.findSessionSummaryById(threadId, true)) {
      throw createAppError("SESSION_ALREADY_ARCHIVED");
    }
    if (!(await this.findSessionSummaryById(threadId, false))) {
      throw createAppError("SESSION_NOT_FOUND");
    }
    try {
      await this.client.request("thread/archive", { threadId });
    } catch (error) {
      if (await this.findSessionSummaryById(threadId, true)) {
        throw createAppError("SESSION_ALREADY_ARCHIVED");
      }
      if (!(await this.findSessionSummaryById(threadId, false))) {
        throw createAppError("SESSION_NOT_FOUND");
      }
      throw error;
    }
    sessionIndexClient.invalidate();
    this.recentLiveSessions.delete(threadId);
    this.rolloutPaths.delete(threadId);
    this.sessionHydrations.delete(threadId);
    this.itemDetailsByThread.delete(threadId);
    this.fullTurnsByThread.delete(threadId);
    return { ok: true };
  }

  async unarchiveSession(threadId: string) {
    if (await this.findSessionSummaryById(threadId, false)) {
      throw createAppError("SESSION_NOT_ARCHIVED");
    }
    if (!(await this.findSessionSummaryById(threadId, true))) {
      throw createAppError("SESSION_NOT_FOUND");
    }
    let response: Record<string, unknown>;
    try {
      response = asRecord(await this.client.request("thread/unarchive", { threadId }));
    } catch (error) {
      if (await this.findSessionSummaryById(threadId, false)) {
        throw createAppError("SESSION_NOT_ARCHIVED");
      }
      if (!(await this.findSessionSummaryById(threadId, true))) {
        throw createAppError("SESSION_NOT_FOUND");
      }
      throw error;
    }
    sessionIndexClient.invalidate();
    this.rolloutPaths.delete(threadId);
    this.sessionHydrations.delete(threadId);
    this.itemDetailsByThread.delete(threadId);
    this.fullTurnsByThread.delete(threadId);
    const thread = normalizeThread(asRecord(response.thread));
    const preferences = await this.getPreferences(threadId, thread);
    const summary = applySessionMeta(toSummary(thread, preferences, false), await uiStateStore.getSessionMeta(threadId));
    this.recentLiveSessions.set(threadId, summary);
    this.recentLiveSessionsLoadedAt = Date.now();
    return {
      ok: true,
      session: summary
    };
  }

  async getAccount() {
    return this.readAccountState(true);
  }

  async startAccountLogin(type: "chatgpt" | "chatgptDeviceCode" | "apiKey", apiKey?: string | null) {
    this.invalidateAccountStateCache();
    if (type === "apiKey") {
      if (!apiKey?.trim()) {
        throw new Error("API key is required.");
      }
      return this.client.request("account/login/start", { type, apiKey: apiKey.trim() });
    }

    return this.client.request("account/login/start", { type });
  }

  async cancelAccountLogin(loginId: string) {
    this.invalidateAccountStateCache();
    return this.client.request("account/login/cancel", { loginId });
  }

  async logoutAccount() {
    this.invalidateAccountStateCache();
    return this.client.request("account/logout", {});
  }

  async createSession(
    preferences: Partial<SessionPreferences>,
    name: string | null,
    options: {
      hiddenFromSidebar?: boolean;
    } = {}
  ) {
    const nextPreferences = await this.preparePreferences(preferences);
    const response = asRecord(
      await this.client.request("thread/start", {
        model: nextPreferences.model,
        cwd: nextPreferences.cwd,
        approvalPolicy: nextPreferences.approvalPolicy,
        sandbox: nextPreferences.sandboxMode,
        serviceTier: nextPreferences.speed === "auto" ? null : nextPreferences.speed,
        experimentalRawEvents: false,
        persistExtendedHistory: true
      })
    );
    const thread = asRecord(response.thread);

    await uiStateStore.set(String(thread.id), nextPreferences);
    const nextName = name?.trim() || null;
    if (nextName && !isPlaceholderThreadName(nextName)) {
      await this.renameSession(String(thread.id), nextName);
      thread.name = nextName;
    }

    const summary = applySessionMeta(toSummary(thread, nextPreferences), await uiStateStore.getSessionMeta(String(thread.id)));
    sessionIndexClient.invalidate();
    if (!options.hiddenFromSidebar) {
      this.recentLiveSessions.set(String(thread.id), summary);
      this.recentLiveSessionsLoadedAt = Date.now();
      this.emitGlobal({
        kind: "notification",
        method: "codex-webui/sessionSummaryUpdated",
        params: {
          session: summary
        }
      });
    }
    return summary;
  }

  async forkSession(
    sourceThreadId: string,
    options: {
      mode: SessionForkMode;
      turnId?: string | null;
      messageText?: string | null;
    }
  ): Promise<SessionForkPayload> {
    const thread = await this.readThread(sourceThreadId, false);
    const preferences = await this.getPreferences(sourceThreadId, thread);
    const hydration = await this.ensureSessionHistory(sourceThreadId, thread);
    const anchorIndex = options.turnId ? hydration.turns.findIndex((turn) => turn.id === options.turnId) : hydration.turns.length - 1;
    const turns = anchorIndex >= 0 ? hydration.turns.slice(0, anchorIndex + 1) : hydration.turns;

    let selectedMessageText = options.messageText?.trim() || null;
    if (!selectedMessageText) {
      for (let turnIndex = turns.length - 1; turnIndex >= 0 && !selectedMessageText; turnIndex -= 1) {
        const turn = turns[turnIndex];
        for (let itemIndex = turn.items.length - 1; itemIndex >= 0; itemIndex -= 1) {
          const item = turn.items[itemIndex];
          if (item.type !== "userMessage") {
            continue;
          }
          const text = extractDraftTextFromItem(item);
          if (text) {
            selectedMessageText = text;
            break;
          }
        }
      }
    }

    if (!selectedMessageText) {
      selectedMessageText = stripAttachmentPreamble(asText(thread.preview) ?? "").trim() || null;
    }

    let draft = stripAttachmentPreamble(selectedMessageText ?? "").trim();
    if (options.mode === "handoff") {
      const sourceName = getDisplayThreadName(asText(thread.name), asText(thread.preview)) ?? "Source thread";
      const preview = stripAttachmentPreamble(asText(thread.preview) ?? "").trim();
      const entries: Array<{ role: "User" | "Assistant"; text: string }> = [];
      for (const turn of turns) {
        for (const item of turn.items ?? []) {
          if (item.type !== "userMessage" && item.type !== "agentMessage") {
            continue;
          }

          const text = extractDraftTextFromItem(item);
          if (!text) {
            continue;
          }

          entries.push({
            role: item.type === "userMessage" ? "User" : "Assistant",
            text
          });
        }
      }

      const sections = [
        `Continue this task in a fresh thread.\n\nSource thread: ${sourceName}\nWorking directory: ${String(thread.cwd ?? "")}`.trim()
      ];
      if (preview) {
        sections.push(`Current goal:\n${preview}`);
      }
      if (selectedMessageText?.trim()) {
        sections.push(`Focus request:\n${selectedMessageText.trim()}`);
      }
      if (entries.length > 0) {
        sections.push(
          `Recent context:\n${entries
            .slice(-8)
            .map((entry) => `- ${entry.role}: ${entry.text.replace(/\s+/gu, " ").trim()}`)
            .join("\n")}`
        );
      }
      sections.push("Continue from this handoff, preserve any existing constraints, and begin with the most sensible next step.");
      draft = sections.filter((section) => section.trim().length > 0).join("\n\n").trim();
    }
    if (!draft) {
      throw new Error(options.mode === "handoff" ? "There is no thread context to hand off yet." : "There is no message to fork yet.");
    }

    const sourceName = getDisplayThreadName(asText(thread.name), asText(thread.preview));
    const nextName =
      options.mode === "handoff"
        ? `${sourceName && !isPlaceholderThreadName(sourceName) ? sourceName : inferSessionDisplayTitle(draft) ?? "Thread"} · Handoff`
        : inferSessionDisplayTitle(selectedMessageText ?? draft) ?? sourceName ?? null;

    const session = await this.createSession(preferences, nextName);
    const savedDraft = await this.saveDraft(session.id, draft, "message");
    return {
      session,
      draft: savedDraft.draft,
      mode: options.mode
    };
  }

  async getSession(threadId: string, limit = SESSION_WINDOW_SIZE): Promise<SessionDetailPayload> {
    const thread = await this.readThread(threadId, false);
    const preferences = await this.getPreferences(threadId, thread);
    const attachments = await listAttachments(threadId);
    const pending = [...(this.pendingRequests.get(threadId)?.values() ?? [])].map(({ rawId: _rawId, ...request }) => request);
    const hydration = await this.ensureSessionHistory(threadId, thread);
    const windowSize = Math.max(1, limit);
    const runtimeState = await this.resolveRuntimeSessionState(threadId, thread, hydration.turns, this.activeTurns.get(threadId) ?? null);
    const visibleTurns = runtimeState.turns.slice(-windowSize).map((turn) => structuredClone(turn));
    const totalTurns = hydration.totalTurns ?? hydration.turns.length;
    const activeTurnId = runtimeState.activeTurnId;

    let queue = await this.getQueue(threadId);
    if (queue.resumeRequired && !activeTurnId && preferences.steeringResumeMode === "auto") {
      await uiStateStore.setQueueResumePending(threadId, false);
      queue = await this.getQueue(threadId);
      void this.maybeDrainQueue(threadId);
    }

    void this.setSessionHighlight(threadId, null, thread, preferences, queue.items.length, runtimeState.status, null);

    return {
      thread: {
        ...(thread as SessionDetailPayload["thread"]),
        status: runtimeState.status,
        turns: visibleTurns
      },
      preferences,
      attachments,
      queue,
      pendingRequests: pending,
      activeTurnId,
      tokenUsage: this.tokenUsageByThread.get(threadId) ?? null,
      hydration: {
        state: hydration.state,
        loadedTurns: visibleTurns.length,
        totalTurns,
        remainingTurns: Math.max(totalTurns - visibleTurns.length, 0),
        message: hydration.message
      }
    };
  }

  async getSessionOlderTurns(threadId: string, beforeTurnId: string, limit = SESSION_WINDOW_SIZE): Promise<SessionTurnsPagePayload> {
    const hydration = await this.ensureSessionHistory(threadId);
    const thread = await this.readThread(threadId, false);
    const runtimeState = await this.resolveRuntimeSessionState(threadId, thread, hydration.turns, this.activeTurns.get(threadId) ?? null);
    const beforeIndex = hydration.turns.findIndex((turn) => turn.id === beforeTurnId);
    if (beforeIndex <= 0) {
      return {
        turns: [],
        loadedTurns: hydration.turns.length,
        totalTurns: hydration.totalTurns,
        remainingTurns: 0
      };
    }

    const windowSize = Math.max(1, limit);
    const start = Math.max(0, beforeIndex - windowSize);
    const turns = runtimeState.turns.slice(start, beforeIndex).map((turn) => structuredClone(turn));
    return {
      turns,
      loadedTurns: beforeIndex,
      totalTurns: hydration.totalTurns,
      remainingTurns: start
    };
  }

  async getSessionTurn(threadId: string, turnId: string): Promise<SessionTurnPayload> {
    const cached = this.fullTurnsByThread.get(threadId)?.get(turnId);
    if (cached) {
      return {
        turn: structuredClone(cached)
      };
    }

    const thread = await this.readThread(threadId, true);
    const runtimeState = await this.resolveRuntimeSessionState(
      threadId,
      thread,
      Array.isArray(thread.turns) ? (thread.turns as CodexTurn[]) : [],
      this.activeTurns.get(threadId) ?? null
    );
    const rawTurn = runtimeState.turns.find((candidate) => candidate.id === turnId);
    if (!rawTurn) {
      throw new Error("Turn not found.");
    }

    const turn = this.prepareTurnForClient(threadId, rawTurn);
    this.getFullTurnMap(threadId).set(turnId, structuredClone(turn));
    return {
      turn: structuredClone(turn)
    };
  }

  async searchSessionTurns(
    threadId: string,
    query: string,
    cursor: string | null = null,
    limit = SESSION_WINDOW_SIZE
  ): Promise<SessionTurnSearchPayload> {
    const needle = query.trim().toLowerCase();
    if (!needle) {
      return {
        matches: [],
        nextCursor: null,
        totalMatches: 0
      };
    }

    const hydration = await this.ensureSessionHistory(threadId);
    const thread = await this.readThread(threadId, false);
    const runtimeState = await this.resolveRuntimeSessionState(threadId, thread, hydration.turns, this.activeTurns.get(threadId) ?? null);
    const fullTurnMap = this.fullTurnsByThread.get(threadId) ?? new Map<string, CodexTurn>();
    const summaryItemIdsByTurn = new Map(runtimeState.turns.map((turn) => [turn.id, new Set(turn.items.map((item) => item.id))]));
    const matches: SessionTurnSearchPayload["matches"] = [];

    for (const [turnIndex, summaryTurn] of runtimeState.turns.entries()) {
      const turn = fullTurnMap.get(summaryTurn.id) ?? summaryTurn;
      const visibleItemIds = summaryItemIdsByTurn.get(turn.id) ?? new Set<string>();

      for (const item of turn.items) {
        const fragments: string[] = [];

        if (item.type === "userMessage" || item.type === "agentMessage" || item.type === "plan") {
          const text = asText(item.text) ?? "";
          if (text.trim()) {
            fragments.push(text);
          }
        } else if (item.type === "reasoning") {
          const text = asText(item.text) ?? "";
          if (text.trim()) {
            fragments.push(text);
          }
          if (Array.isArray(item.summary)) {
            for (const summaryEntry of item.summary) {
              const text = asText(summaryEntry) ?? "";
              if (text.trim()) {
                fragments.push(text);
              }
            }
          }
        } else if (item.type === "commandExecution") {
          const command = summarizeCommand(item.command);
          if (command.trim()) {
            fragments.push(command);
          }
          const aggregatedOutput = asText(item.aggregatedOutput) ?? "";
          if (aggregatedOutput.trim()) {
            fragments.push(aggregatedOutput);
          }
        } else if (item.type === "fileChange") {
          if (Array.isArray(item.changes)) {
            for (const change of item.changes) {
              const normalizedChange = asRecord(change);
              const changePath = asText(normalizedChange.path) ?? "";
              if (changePath.trim()) {
                fragments.push(changePath);
              }
              const diff = asText(normalizedChange.diff) ?? "";
              if (diff.trim()) {
                fragments.push(diff);
              }
            }
          }
        } else if (item.type === "mcpToolCall" || item.type === "dynamicToolCall") {
          const invocation = summarizeToolInvocation(item.invocation);
          if (invocation.trim()) {
            fragments.push(invocation);
          }
          const result = asText(item.result) ?? "";
          if (result.trim()) {
            fragments.push(result);
          }
        } else if (item.type === "webSearch") {
          const searchQuery = asText(item.query) ?? "";
          if (searchQuery.trim()) {
            fragments.push(searchQuery);
          }
        } else if (item.type === "contextCompaction") {
          const text = asText(item.text) ?? asText(item.detailPreview) ?? "";
          if (text.trim()) {
            fragments.push(text);
          }
        } else {
          const text = asText(item.text) ?? "";
          if (text.trim()) {
            fragments.push(text);
          }
        }

        let preview: string | null = null;
        for (const fragment of fragments) {
          const normalized = fragment.replace(/\s+/g, " ").trim();
          if (!normalized) {
            continue;
          }

          const lower = normalized.toLowerCase();
          const matchIndex = lower.indexOf(needle);
          if (matchIndex < 0) {
            continue;
          }

          const snippetStart = Math.max(0, matchIndex - 54);
          const snippetEnd = Math.min(normalized.length, matchIndex + needle.length + 54);
          preview = `${snippetStart > 0 ? "..." : ""}${normalized.slice(snippetStart, snippetEnd).trim()}${snippetEnd < normalized.length ? "..." : ""}`;
          break;
        }

        if (!preview) {
          continue;
        }

        matches.push({
          turnId: turn.id,
          turnIndex,
          itemId: item.id,
          itemType: item.type,
          preview,
          startedAt: turn.startedAt ?? null,
          requiresFullTurn: !visibleItemIds.has(item.id),
          requiresItemDetail: DEFERRED_ITEM_TYPES.has(item.type)
        });
      }
    }

    const start = Math.max(0, Number.parseInt(cursor ?? "0", 10) || 0);
    const windowSize = Math.max(1, limit);
    const nextIndex = start + windowSize;
    return {
      matches: matches.slice(start, nextIndex),
      nextCursor: nextIndex < matches.length ? String(nextIndex) : null,
      totalMatches: matches.length
    };
  }

  async getSessionItemDetail(threadId: string, turnId: string, itemId: string): Promise<SessionItemDetailPayload> {
    const cached = this.itemDetailsByThread.get(threadId)?.get(itemCacheKey(turnId, itemId));
    if (cached) {
      return {
        item: {
          ...structuredClone(cached),
          detailState: "loaded"
        }
      };
    }

    const turn = (await this.getSessionTurn(threadId, turnId)).turn;
    const item = turn.items.find((candidate) => candidate.id === itemId);
    if (!item) {
      throw new Error("Transcript item detail not found.");
    }

    const resolved = this.itemDetailsByThread.get(threadId)?.get(itemCacheKey(turnId, itemId));
    if (!resolved) {
      throw new Error("Transcript item detail not found.");
    }

    return {
      item: {
        ...structuredClone(resolved),
        detailState: "loaded"
      }
    };
  }

  async savePreferences(threadId: string, preferences: Partial<SessionPreferences>) {
    const nextPreferences = await this.preparePreferences(preferences);
    await uiStateStore.set(threadId, nextPreferences);
    await syncCodexTomlWithPreferences(this.profile.codexHome, nextPreferences);
    this.emit(threadId, {
      kind: "notification",
      method: "codex-webui/preferencesUpdated",
      params: {
        preferences: nextPreferences
      }
    });
    void this.emitConfigUpdated();
    void this.emitSessionSummaryUpdated(threadId, null, nextPreferences);
    return nextPreferences;
  }

  async saveSystemShutdownAfterQueueCompletes(enabled: boolean) {
    const runtimeConfig = this.runtimeConfig;
    if (!runtimeConfig.systemShutdownEnabled && enabled) {
      throw new Error("System shutdown support is disabled.");
    }

    await uiStateStore.setGlobalShutdownAfterQueueCompletes(Boolean(enabled));

    if (!enabled) {
      await this.clearScheduledShutdown();
    } else {
      await this.maybeScheduleGlobalShutdown(null);
    }

    return this.getConfig();
  }

  async getDraft(threadId: string): Promise<SessionDraftPayload> {
    const stored = await uiStateStore.getDraft(threadId);
    return {
      sessionId: threadId,
      draft: stored?.draft ?? "",
      intent: stored?.intent ?? null,
      updatedAt: stored?.updatedAt ?? null
    };
  }

  async saveDraft(threadId: string, draft: string, intent: "message" | "steer" | "queue"): Promise<SessionDraftPayload> {
    const trimmed = draft.trim();
    if (!trimmed) {
      await uiStateStore.clearDraft(threadId);
      return {
        sessionId: threadId,
        draft: "",
        intent: null,
        updatedAt: null
      };
    }

    await uiStateStore.setDraft(threadId, draft, intent);
    const stored = await uiStateStore.getDraft(threadId);
    return {
      sessionId: threadId,
      draft: stored?.draft ?? draft,
      intent: stored?.intent ?? intent,
      updatedAt: stored?.updatedAt ?? Date.now()
    };
  }

  async clearDraft(threadId: string): Promise<SessionDraftPayload> {
    await uiStateStore.clearDraft(threadId);
    return {
      sessionId: threadId,
      draft: "",
      intent: null,
      updatedAt: null
    };
  }

  async getQueue(threadId: string): Promise<SessionQueuePayload> {
    const stored = await uiStateStore.getQueue(threadId);
    return {
      sessionId: threadId,
      items: stored?.items ?? [],
      resumeRequired: Boolean(stored?.resumePending && (stored?.items.length ?? 0) > 0),
      updatedAt: stored?.updatedAt ?? null
    };
  }

  async enqueueMessage(threadId: string, prompt: string, attachments: AttachmentRecord[]): Promise<SessionQueuePayload> {
    const trimmedPrompt = prompt.trim();
    if (!trimmedPrompt && attachments.length === 0) {
      throw createAppError("EMPTY_MESSAGE");
    }

    await this.cancelScheduledShutdownForActivity();

    const nextItem = {
      id: crypto.randomUUID(),
      prompt: trimmedPrompt,
      attachmentIds: attachments.map((attachment) => attachment.id),
      attachmentNames: attachments.map((attachment) => attachment.originalName),
      createdAt: Date.now()
    } satisfies SessionQueueItem;

    await uiStateStore.enqueueQueueItem(threadId, {
      ...nextItem
    });
    await uiStateStore.setQueueResumePending(threadId, false);
    const queue = await this.getQueue(threadId);
    const enqueueAccepted = queue.items.some((item) => item.id === nextItem.id);
    this.emitQueueUpdated(threadId, queue);
    return {
      ...queue,
      enqueueAccepted,
      enqueueItemId: nextItem.id
    };
  }

  async removeQueuedMessage(threadId: string, queueId: string): Promise<SessionQueuePayload> {
    const removed = await uiStateStore.removeQueueItem(threadId, queueId);
    if (!removed) {
      throw createAppError("QUEUE_ITEM_NOT_FOUND");
    }
    const queue = await this.getQueue(threadId);
    this.emitQueueUpdated(threadId, queue);
    await this.maybeScheduleGlobalShutdown(null);
    return queue;
  }

  async updateQueuedMessage(threadId: string, queueId: string, prompt: string, attachments: AttachmentRecord[]): Promise<SessionQueuePayload> {
    const stored = await uiStateStore.getQueue(threadId);
    const queuedItem = stored?.items.find((item) => item.id === queueId);
    if (!queuedItem) {
      throw createAppError("QUEUE_ITEM_NOT_FOUND");
    }

    const trimmedPrompt = prompt.trim();
    if (!trimmedPrompt && attachments.length === 0) {
      throw createAppError("EMPTY_MESSAGE");
    }

    const updated = await uiStateStore.updateQueueItem(threadId, queueId, {
      prompt: trimmedPrompt,
      attachmentIds: attachments.map((attachment) => attachment.id),
      attachmentNames: attachments.map((attachment) => attachment.originalName)
    });
    if (!updated) {
      throw createAppError("QUEUE_ITEM_NOT_FOUND");
    }

    const queue = await this.getQueue(threadId);
    this.emitQueueUpdated(threadId, queue);
    return queue;
  }

  async reorderQueuedMessages(threadId: string, orderedIds: string[]): Promise<SessionQueuePayload> {
    const stored = await uiStateStore.getQueue(threadId);
    if (!stored || stored.items.length === 0) {
      throw createAppError("QUEUE_ITEM_NOT_FOUND");
    }

    const reordered = await uiStateStore.reorderQueueItems(threadId, orderedIds);
    if (!reordered) {
      throw createAppError("QUEUE_ITEM_NOT_FOUND");
    }

    const queue = await this.getQueue(threadId);
    this.emitQueueUpdated(threadId, queue);
    return queue;
  }

  async dispatchQueuedMessage(threadId: string, queueId: string, mode: "message" | "steer"): Promise<SessionQueuePayload> {
    const queue = await this.withQueueDispatchLock(threadId, async () => {
      const stored = await uiStateStore.getQueue(threadId);
      const queuedItem = stored?.items.find((item) => item.id === queueId);
      if (!queuedItem) {
        throw createAppError("QUEUE_ITEM_NOT_FOUND");
      }

      const attachments = (await listAttachments(threadId)).filter((attachment) => queuedItem.attachmentIds.includes(attachment.id));
      if (mode === "steer") {
        await this.steer(threadId, queuedItem.prompt, attachments);
      } else {
        await this.sendMessage(threadId, queuedItem.prompt, attachments, {});
      }

      await uiStateStore.removeQueueItem(threadId, queueId);
      await uiStateStore.setQueueResumePending(threadId, false);
      const nextQueue = await this.getQueue(threadId);
      this.emitQueueUpdated(threadId, nextQueue);
      return nextQueue;
    });

    if (!queue) {
      throw createAppError("QUEUE_ALREADY_DISPATCHING");
    }

    return queue;
  }

  async resumeQueue(threadId: string): Promise<SessionQueuePayload> {
    await uiStateStore.setQueueResumePending(threadId, false);
    const queue = await this.getQueue(threadId);
    this.emitQueueUpdated(threadId, queue);
    void this.maybeDrainQueue(threadId);
    return queue;
  }

  async renameSession(threadId: string, name: string) {
    await this.client.request("thread/name/set", { threadId, name });
    sessionIndexClient.invalidate();
  }

  async notifyAttachmentsUpdated(threadId: string) {
    this.emit(threadId, {
      kind: "notification",
      method: "codex-webui/attachmentsUpdated",
      params: {
        attachments: await listAttachments(threadId)
      }
    });
  }

  private buildTurnInput(prompt: string, attachments: AttachmentRecord[]) {
    const textAttachments = attachments.filter((attachment) => attachment.kind === "file").map((attachment) => attachment.path);
    return [
      {
        type: "text",
        text: textAttachments.length > 0 ? `[[codex-webui-attachments]]\n${textAttachments.join("\n")}\n[[/codex-webui-attachments]]\n\n${prompt}` : prompt,
        text_elements: []
      },
      ...attachments
        .filter((attachment) => attachment.kind === "image")
        .map((attachment) => ({ type: "localImage", path: attachment.path }))
    ];
  }

  async sendMessage(threadId: string, prompt: string, attachments: AttachmentRecord[], preferences: Partial<SessionPreferences>) {
    await this.cancelScheduledShutdownForActivity();
    const inferredTitle = inferPersistedSessionTitle(prompt);
    const nextPreferences = await this.preparePreferences(preferences);
    const defaultModel = nextPreferences.model ?? (await this.getDefaultModel());
    const readableRoots = [...new Set(attachments.map((attachment) => attachment.path).map((filePath) => filePath.replace(/\/[^/]+$/, "")))];
    const thread = await this.readThread(threadId, false);
    const shouldBackfillTitle = isPlaceholderThreadName(asText(asRecord(thread).name));

    if (thread.status === "notLoaded") {
      await this.client.request("thread/resume", {
        threadId,
        persistExtendedHistory: true
      });
    }

    await uiStateStore.set(threadId, {
      ...nextPreferences,
      model: defaultModel
    });

    const response = await this.client.request("turn/start", {
      threadId,
      input: this.buildTurnInput(prompt, attachments),
      cwd: nextPreferences.cwd,
      approvalPolicy: nextPreferences.approvalPolicy,
      sandboxPolicy: buildSandboxPolicy(nextPreferences, readableRoots),
      model: defaultModel,
      serviceTier: nextPreferences.speed === "auto" ? null : nextPreferences.speed,
      effort: nextPreferences.mode === "plan" ? null : nextPreferences.effort,
      collaborationMode:
        nextPreferences.mode === "plan"
          ? {
              mode: "plan",
              settings: {
                model: defaultModel,
                reasoning_effort: nextPreferences.effort,
                developer_instructions: null
              }
            }
          : null
    });
    await uiStateStore.clearDraft(threadId);
    if (shouldBackfillTitle && inferredTitle) {
      await this.renameSession(threadId, inferredTitle).catch(() => {});
    }
    void this.emitSessionSummaryUpdated(threadId, null, {
      ...nextPreferences,
      model: defaultModel
    });
    return response;
  }

  async steer(threadId: string, prompt: string, attachments: AttachmentRecord[] = []) {
    const turnId = await this.resolveActiveTurnId(threadId);
    if (!turnId) {
      throw createAppError("NO_ACTIVE_TURN");
    }
    const response = await this.client.request("turn/steer", {
      threadId,
      expectedTurnId: turnId,
      input: this.buildTurnInput(prompt, attachments)
    });
    await uiStateStore.clearDraft(threadId);
    return response;
  }

  async interrupt(threadId: string) {
    const turnId = await this.resolveActiveTurnId(threadId);
    if (!turnId) {
      return { interrupted: false };
    }
    await this.client.request("turn/interrupt", { threadId, turnId });
    return { interrupted: true };
  }

  async resolveServerRequest(threadId: string, requestId: string, result: unknown) {
    const pending = this.pendingRequests.get(threadId)?.get(requestId);
    if (!pending) {
      throw createAppError("PENDING_REQUEST_NOT_FOUND");
    }
    await this.client.respond(pending.rawId, result);
  }

  private async findSessionSummaryById(threadId: string, archived: boolean) {
    const [preferences, queueCounts, highlightsByThreadId] = await Promise.all([
      uiStateStore.getAll(),
      archived ? Promise.resolve({} as Record<string, number>) : uiStateStore.getQueueCounts(),
      uiStateStore.getSessionHighlights()
    ]);
    let cursor: string | null = null;

    do {
      const response = asRecord(await this.client.request("thread/list", { limit: 200, archived, cursor }));
      const threads = (response.data as Array<Record<string, unknown>> | undefined) ?? [];
      const matched = threads.find((thread) => String(thread.id ?? "") === threadId && !isSubagentThread(thread));
      if (matched) {
        const stored = preferences[threadId];
        return toSummary(
          matched,
          stored ? coercePreferences(stored) : null,
          archived,
          queueCounts[threadId] ?? 0,
          highlightsByThreadId[threadId] ?? null
        );
      }
      cursor = typeof response.nextCursor === "string" && response.nextCursor.trim() ? String(response.nextCursor) : null;
    } while (cursor);

    return null;
  }

  async listDirectories(currentPath: string | null) {
    return listDirectoryPayload(currentPath);
  }

  private async getPreferences(threadId: string, thread: Record<string, unknown>) {
    const stored = await uiStateStore.get(threadId);
    if (stored) {
      return coercePreferences(stored);
    }
    return this.preparePreferences({
      cwd: String(thread.cwd ?? this.profile.defaults.cwd)
    });
  }

  private async preparePreferences(preferences: Partial<SessionPreferences>) {
    const nextPreferences = coercePreferences(preferences);
    nextPreferences.cwd = await resolveAllowedDirectory(nextPreferences.cwd);
    if (nextPreferences.gitRepoPath) {
      nextPreferences.gitRepoPath = (await resolveGitRepository(nextPreferences.gitRepoPath)).path;
    }
    return nextPreferences;
  }

  private async getDefaultModel() {
    const config = await this.getConfig();
    return config.models.find((model) => model.isDefault)?.id ?? config.models[0]?.id ?? null;
  }

  private async getLoadedThreadIds() {
    const now = Date.now();
    if (this.loadedThreadIdsLoadedAt > 0 && now - this.loadedThreadIdsLoadedAt < LOADED_THREAD_CACHE_TTL_MS) {
      return this.loadedThreadIds;
    }

    if (this.loadedThreadIdsPromise) {
      return this.loadedThreadIdsPromise;
    }

    this.loadedThreadIdsPromise = (async () => {
      const response = asRecord(await this.client.request("thread/loaded/list", {}));
      const ids = new Set(
        ((response.data as unknown[] | undefined) ?? [])
          .filter((value) => typeof value === "string" && value.trim())
          .map((value) => String(value))
      );
      this.loadedThreadIds = ids;
      this.loadedThreadIdsLoadedAt = Date.now();
      return ids;
    })().finally(() => {
      this.loadedThreadIdsPromise = null;
    });

    return this.loadedThreadIdsPromise;
  }

  private async readThread(threadId: string, includeTurns: boolean) {
    try {
      const response = asRecord(await this.client.request("thread/read", { threadId, includeTurns }));
      return normalizeThread(asRecord(response.thread));
    } catch (error) {
      if (!includeTurns || !isUnmaterializedThreadError(error)) {
        throw error;
      }

      const response = asRecord(await this.client.request("thread/read", { threadId, includeTurns: false }));
      return normalizeThread(asRecord(response.thread));
    }
  }

  private async resolveRuntimeSessionState(
    threadId: string,
    thread: Record<string, unknown>,
    turns: CodexTurn[],
    fallbackActiveTurnId: string | null = null
  ) {
    const persistedActiveTurnId = findActiveTurnId({ turns });
    let loadedThreadIds: Set<string> | null = null;
    try {
      loadedThreadIds = await this.getLoadedThreadIds();
    } catch {
      loadedThreadIds = null;
    }

    const loadedThreadIdsAvailable = loadedThreadIds !== null;
    const loadedInMemory = loadedThreadIdsAvailable
      ? loadedThreadIds!.has(threadId) || this.activeTurns.has(threadId)
      : this.activeTurns.has(threadId) || isLiveThreadStatus(thread.status);
    const staleRunning =
      loadedThreadIdsAvailable && !loadedInMemory && (isLiveThreadStatus(thread.status) || Boolean(persistedActiveTurnId));
    const status = staleRunning ? "stopped" : String(normalizeThreadStatus(thread.status) ?? "unknown");
    const normalizedTurns = staleRunning ? turns.map((turn) => normalizeStoppedTurn(turn)) : turns;
    const activeTurnId = loadedInMemory
      ? getThreadActiveTurnId(
          {
            ...thread,
            status,
            turns: normalizedTurns
          },
          persistedActiveTurnId ?? fallbackActiveTurnId ?? this.activeTurns.get(threadId) ?? null
        )
      : null;

    if (activeTurnId) {
      this.activeTurns.set(threadId, activeTurnId);
    } else if (staleRunning || !LIVE_THREAD_STATUSES.has(status)) {
      this.activeTurns.delete(threadId);
    }

    return {
      loadedInMemory,
      staleRunning,
      status,
      turns: normalizedTurns,
      activeTurnId
    };
  }

  private async resolveActiveTurnId(threadId: string) {
    const thread = await this.readThread(threadId, true);
    const turns = Array.isArray(thread.turns) ? (thread.turns as CodexTurn[]) : [];
    const runtimeState = await this.resolveRuntimeSessionState(threadId, thread, turns, this.activeTurns.get(threadId) ?? null);
    return runtimeState.activeTurnId;
  }

  private getHydrationState(threadId: string): SessionHydrationState {
    const existing = this.sessionHydrations.get(threadId);
    if (existing) {
      return existing;
    }

    const fresh = {
      state: "idle",
      loadedTurns: 0,
      totalTurns: null,
      remainingTurns: 0,
      message: null,
      turns: []
    } satisfies SessionHydrationState;
    this.sessionHydrations.set(threadId, fresh);
    return fresh;
  }

  private pruneHydrationCache() {
    const completed = [...this.sessionHydrations.entries()].filter(([, hydration]) => hydration.state === "complete");
    while (completed.length > HYDRATION_CACHE_LIMIT) {
      const oldest = completed.shift();
      if (!oldest) {
        break;
      }
      this.sessionHydrations.delete(oldest[0]);
    }
  }

  private getItemDetailMap(threadId: string) {
    const existing = this.itemDetailsByThread.get(threadId);
    if (existing) {
      return existing;
    }

    const created = new Map<string, CodexItem>();
    this.itemDetailsByThread.set(threadId, created);
    return created;
  }

  private getFullTurnMap(threadId: string) {
    const existing = this.fullTurnsByThread.get(threadId);
    if (existing) {
      return existing;
    }

    const created = new Map<string, CodexTurn>();
    this.fullTurnsByThread.set(threadId, created);
    return created;
  }

  private cacheFullTurns(threadId: string, turns: CodexTurn[]) {
    const fullTurns = this.getFullTurnMap(threadId);
    fullTurns.clear();

    for (const turn of turns) {
      const prepared = this.prepareTurnForClient(threadId, {
        ...turn,
        items: [...turn.items]
      });
      fullTurns.set(turn.id, structuredClone(prepared));
    }
  }

  private cacheItemDetail(threadId: string, turnId: string, item: CodexItem) {
    this.getItemDetailMap(threadId).set(itemCacheKey(turnId, item.id), structuredClone(item));
  }

  private appendCommandExecutionDetail(threadId: string, turnId: string, itemId: string, delta: string) {
    const details = this.itemDetailsByThread.get(threadId);
    if (!details) {
      return;
    }

    const key = itemCacheKey(turnId, itemId);
    const cached = details.get(key);
    if (!cached) {
      return;
    }

    details.set(key, {
      ...cached,
      aggregatedOutput: `${String(cached.aggregatedOutput ?? "")}${delta}`
    });
  }

  private prepareItemForClient(threadId: string, turnId: string, rawItem: unknown): CodexItem {
    const normalized = structuredClone(asRecord(rawItem)) as CodexItem;
    normalized.id = String(normalized.id ?? `${turnId}:${Math.random().toString(36).slice(2, 8)}`);
    normalized.type = String(normalized.type ?? "unknown");

    if (normalized.type === "contextCompaction") {
      return {
        id: normalized.id,
        type: normalized.type,
        title: "Context compression",
        detailState: "inline",
        detailPreview: "Compressing conversation context"
      };
    }

    if (!DEFERRED_ITEM_TYPES.has(normalized.type)) {
      return {
        ...normalized,
        detailState: normalized.detailState ?? "inline"
      };
    }

    this.cacheItemDetail(threadId, turnId, normalized);

    if (normalized.type === "commandExecution") {
      return {
        id: normalized.id,
        type: normalized.type,
        title: "Command",
        detailState: "deferred",
        detailPreview: summarizeCommand(normalized.command) || null,
        command: normalized.command,
        parsed_cmd: Array.isArray(normalized.parsed_cmd)
          ? normalized.parsed_cmd
          : Array.isArray(normalized.parsedCmd)
            ? normalized.parsedCmd
            : [],
        cwd: normalized.cwd ?? null,
        status: normalized.status ?? null,
        exitCode: normalized.exitCode ?? null
      };
    }

    if (normalized.type === "fileChange") {
      const changes = Array.isArray(normalized.changes) ? normalized.changes.map((entry) => asRecord(entry)) : [];
      const firstPath = changes.length > 0 ? asText(changes[0].path) ?? null : null;
      return {
        id: normalized.id,
        type: normalized.type,
        title: "Files changed",
        detailState: "deferred",
        detailPreview: firstPath ?? (changes.length > 0 ? `${changes.length} files` : null),
        changeCount: changes.length,
        firstChangePath: firstPath,
        changes: changes.map((change) => ({
          path: asText(change.path) ?? "Code edit",
          kind: change.kind ?? "update"
        }))
      };
    }

    if (normalized.type === "webSearch") {
      return {
        id: normalized.id,
        type: normalized.type,
        title: "Web search",
        detailState: "deferred",
        detailPreview: (asText(normalized.query) ?? summarizeToolInvocation(normalized.action)) || null,
        query: normalized.query ?? null
      };
    }

    const invocationSummary = summarizeToolInvocation(normalized.invocation);
    return {
      id: normalized.id,
      type: normalized.type,
      title: normalized.type === "mcpToolCall" ? "MCP call" : "Tool call",
      detailState: "deferred",
      detailPreview: invocationSummary || asText(normalized.tool) || null
    };
  }

  private prepareTurnForClient(threadId: string, rawTurn: CodexTurn): CodexTurn {
    return {
      ...rawTurn,
      items: rawTurn.items.map((item) => this.prepareItemForClient(threadId, rawTurn.id, item)),
      detailState: "full",
      hiddenItemCount: 0
    };
  }

  private prepareSummaryTurnForClient(threadId: string, rawTurn: CodexTurn): CodexTurn {
    const fullTurn = this.prepareTurnForClient(threadId, rawTurn);
    if (String(fullTurn.status ?? "") === "inProgress") {
      return fullTurn;
    }

    const finalAgentIndex = [...fullTurn.items]
      .map((item, index) => ({ item, index }))
      .reverse()
      .find((entry) => entry.item.type === "agentMessage")?.index;

    if (typeof finalAgentIndex !== "number") {
      return fullTurn;
    }

    const hiddenItemCount = fullTurn.items.filter(
      (item, index) => item.type !== "userMessage" && item.type !== "fileChange" && index !== finalAgentIndex
    ).length;
    if (hiddenItemCount <= 0) {
      return fullTurn;
    }

    return {
      ...fullTurn,
      items: fullTurn.items.filter((item, index) => item.type === "userMessage" || item.type === "fileChange" || index === finalAgentIndex),
      detailState: "summary",
      hiddenItemCount
    };
  }

  private async ensureSessionHistory(threadId: string, threadSummary?: Record<string, unknown>) {
    const existing = this.getHydrationState(threadId);
    if (existing.state === "complete") {
      return existing;
    }

    const activeJob = this.hydrationJobs.get(threadId);
    if (activeJob) {
      await activeJob;
      return this.getHydrationState(threadId);
    }

    const job = (async () => {
      const initial = this.getHydrationState(threadId);
      initial.state = "loading";
      initial.loadedTurns = 0;
      initial.totalTurns = null;
      initial.remainingTurns = 0;
      initial.message = null;
      initial.turns = [];

      try {
        const codexHome = this.profile.codexHome;
        const liveThread = String(threadSummary?.status ?? "") === "active" || String(threadSummary?.status ?? "") === "running" || this.activeTurns.has(threadId);
        let rolloutPath = this.rolloutPaths.get(threadId) ?? null;
        if (rolloutPath === null && !this.rolloutPaths.has(threadId)) {
          const createdAt = Number(threadSummary?.createdAt ?? 0);
          const candidateDates =
            createdAt > 0
              ? [0, -1, 1].map((offset) => {
                  const value = new Date(createdAt * 1000);
                  value.setDate(value.getDate() + offset);
                  return value;
                })
              : [];

          for (const date of candidateDates) {
            const dayDirectory = path.join(
              codexHome,
              "sessions",
              String(date.getFullYear()),
              String(date.getMonth() + 1).padStart(2, "0"),
              String(date.getDate()).padStart(2, "0")
            );

            try {
              const names = await readdir(dayDirectory);
              const match = names.find((name) => name.endsWith(`${threadId}.jsonl`));
              if (match) {
                rolloutPath = path.join(dayDirectory, match);
                break;
              }
            } catch {
              continue;
            }
          }

          if (!rolloutPath) {
            try {
              const archived = await readdir(path.join(codexHome, "archived_sessions"));
              const match = archived.find((name) => name.endsWith(`${threadId}.jsonl`));
              if (match) {
                rolloutPath = path.join(codexHome, "archived_sessions", match);
              }
            } catch {
              rolloutPath = null;
            }
          }

          this.rolloutPaths.set(threadId, rolloutPath);
        }

        if (rolloutPath) {
          const turnsById = new Map<string, SessionDetailPayload["thread"]["turns"][number]>();
          let currentTurnId: string | null = null;
          const stream = createReadStream(rolloutPath, { encoding: "utf8" });
          const lines = readline.createInterface({
            input: stream,
            crlfDelay: Infinity
          });

          for await (const line of lines) {
            if (!line.trim()) {
              continue;
            }

            let record: Record<string, unknown>;
            try {
              record = JSON.parse(line) as Record<string, unknown>;
            } catch {
              continue;
            }

            if (record.type !== "event_msg" && record.type !== "response_item") {
              continue;
            }

            const payload = asRecord(record.payload);
            const eventType = String(payload.type ?? "");
            const rawTimestamp = typeof record.timestamp === "string" ? Date.parse(record.timestamp) : NaN;
            const timestamp = Number.isNaN(rawTimestamp) ? null : rawTimestamp;

            if (eventType === "task_started") {
              currentTurnId = String(payload.turn_id ?? `turn-${turnsById.size + 1}`);
              if (!turnsById.has(currentTurnId)) {
                turnsById.set(currentTurnId, {
                  id: currentTurnId,
                  items: [],
                  status: "inProgress",
                  error: null,
                  startedAt: timestamp,
                  completedAt: null,
                  durationMs: null
                });
              }
              continue;
            }

            if (eventType === "thread_rolled_back") {
              const numTurns = Math.max(0, Number(payload.num_turns ?? 0));
              if (numTurns > 0) {
                const retainedTurns = [...turnsById.values()].slice(0, Math.max(turnsById.size - numTurns, 0));
                turnsById.clear();
                for (const retainedTurn of retainedTurns) {
                  turnsById.set(retainedTurn.id, retainedTurn);
                }
              }
              currentTurnId = null;
              continue;
            }

            const activeTurnId: string | null = typeof payload.turn_id === "string" ? String(payload.turn_id) : currentTurnId;
            if (!activeTurnId) {
              continue;
            }

            const turn: SessionDetailPayload["thread"]["turns"][number] =
              turnsById.get(activeTurnId) ??
              ({
                id: activeTurnId,
                items: [],
                status: "inProgress",
                error: null,
                startedAt: timestamp,
                completedAt: null,
                durationMs: null
              } satisfies SessionDetailPayload["thread"]["turns"][number]);
            if (!turnsById.has(activeTurnId)) {
              turnsById.set(activeTurnId, turn);
            }

            if (record.type === "response_item" && eventType === "reasoning") {
              const summaryEntries = Array.isArray(payload.summary)
                ? payload.summary
                    .map((entry) => asRecord(entry))
                    .map((entry) => asText(entry.text) ?? "")
                    .filter((entry) => entry.trim().length > 0)
                : [];
              const existingReasoning = [...turn.items].reverse().find((item) => item.type === "reasoning");
              const mergedSummary = Array.isArray(existingReasoning?.summary)
                ? existingReasoning.summary.map((entry) => String(entry))
                : [];

              for (const summaryEntry of summaryEntries) {
                if (mergedSummary[mergedSummary.length - 1] !== summaryEntry) {
                  mergedSummary.push(summaryEntry);
                }
              }

              const nextReasoningItem = {
                id: existingReasoning?.id ?? `${activeTurnId}:reasoning`,
                type: "reasoning",
                text: typeof payload.content === "string" ? payload.content : String(existingReasoning?.text ?? ""),
                summary: mergedSummary
              } satisfies CodexItem;
              const existingReasoningIndex = turn.items.findIndex((item) => item.id === nextReasoningItem.id);
              if (existingReasoningIndex >= 0) {
                turn.items[existingReasoningIndex] = nextReasoningItem;
              } else if (summaryEntries.length > 0 || nextReasoningItem.text.trim()) {
                turn.items.push(nextReasoningItem);
              }
              continue;
            }

            if (eventType === "agent_reasoning") {
              const summaryEntry = String(payload.text ?? "").trim();
              if (!summaryEntry) {
                continue;
              }

              const existingReasoning = [...turn.items].reverse().find((item) => item.type === "reasoning");
              const mergedSummary = Array.isArray(existingReasoning?.summary)
                ? existingReasoning.summary.map((entry) => String(entry))
                : [];
              if (mergedSummary[mergedSummary.length - 1] !== summaryEntry) {
                mergedSummary.push(summaryEntry);
              }

              const nextReasoningItem = {
                id: existingReasoning?.id ?? `${activeTurnId}:reasoning`,
                type: "reasoning",
                text: String(existingReasoning?.text ?? ""),
                summary: mergedSummary
              } satisfies CodexItem;
              const existingReasoningIndex = turn.items.findIndex((item) => item.id === nextReasoningItem.id);
              if (existingReasoningIndex >= 0) {
                turn.items[existingReasoningIndex] = nextReasoningItem;
              } else {
                turn.items.push(nextReasoningItem);
              }
              continue;
            }

            if (eventType === "user_message") {
              turn.items.push({
                id: `${activeTurnId}:user:${turn.items.length}`,
                type: "userMessage",
                text: String(payload.message ?? ""),
                attachments: []
              });
              continue;
            }

            if (eventType === "agent_message") {
              turn.items.push({
                id: `${activeTurnId}:agent:${turn.items.length}`,
                type: "agentMessage",
                text: String(payload.message ?? ""),
                phase: typeof payload.phase === "string" ? payload.phase : null
              });
              continue;
            }

            if (eventType === "exec_command_end") {
              turn.items.push({
                id: String(payload.call_id ?? `${activeTurnId}:command:${turn.items.length}`),
                type: "commandExecution",
                command: Array.isArray(payload.command) ? payload.command : [],
                aggregatedOutput: String(payload.aggregated_output ?? ""),
                exitCode: Number(payload.exit_code ?? 0),
                status: String(payload.status ?? "completed"),
                cwd: String(payload.cwd ?? "")
              });
              continue;
            }

            if (eventType === "patch_apply_end") {
              const changes = Object.entries(asRecord(payload.changes)).map(([changePath, change]) => {
                const normalizedChange = asRecord(change);
                const changeType = String(normalizedChange.type ?? "update");
                const movePath = asText(normalizedChange.move_path) ?? asText(normalizedChange.movePath) ?? null;
                const diff =
                  changeType === "update"
                    ? (asText(normalizedChange.unified_diff) ?? asText(normalizedChange.unifiedDiff) ?? "")
                    : asText(normalizedChange.content) ?? "";

                return {
                  path: changePath,
                  kind:
                    changeType === "update"
                      ? {
                          type: "update",
                          movePath,
                          move_path: movePath
                        }
                      : {
                          type: changeType
                        },
                  diff
                };
              });
              turn.items.push({
                id: String(payload.call_id ?? `${activeTurnId}:patch:${turn.items.length}`),
                type: "fileChange",
                changes,
                status: String(payload.status ?? (payload.success ? "completed" : "failed"))
              });
              continue;
            }

            if (eventType === "mcp_tool_call_end") {
              turn.items.push({
                id: String(payload.call_id ?? `${activeTurnId}:mcp:${turn.items.length}`),
                type: "mcpToolCall",
                invocation: payload.invocation ?? null,
                result: payload.result ?? null
              });
              continue;
            }

            if (eventType === "web_search_end") {
              turn.items.push({
                id: String(payload.call_id ?? `${activeTurnId}:web:${turn.items.length}`),
                type: "webSearch",
                query: String(payload.query ?? ""),
                action: payload.action ?? null
              });
              continue;
            }

            if (eventType !== "task_complete") {
              continue;
            }

            turn.status = "completed";
            turn.completedAt = timestamp;
            turn.durationMs =
              turn.startedAt !== null && turn.completedAt !== null ? Math.max(turn.completedAt - turn.startedAt, 0) : null;

            const lastAgentMessage = typeof payload.last_agent_message === "string" ? payload.last_agent_message : null;
            const lastAgentItem = [...turn.items].reverse().find((item) => item.type === "agentMessage");
            if (lastAgentMessage && String(lastAgentItem?.text ?? "") !== lastAgentMessage) {
              turn.items.push({
                id: `${activeTurnId}:agent:final`,
                type: "agentMessage",
                text: lastAgentMessage
              });
            }

            if (currentTurnId === activeTurnId) {
              currentTurnId = null;
            }
          }

          if (liveThread) {
            try {
              const thread = await this.readThread(threadId, true);
              const liveTurns = Array.isArray(thread.turns) ? (thread.turns as SessionDetailPayload["thread"]["turns"]) : [];
              for (const liveTurn of liveTurns) {
                const normalizedLiveTurn = {
                  ...liveTurn,
                  items: [...liveTurn.items]
                };
                const existingTurn = turnsById.get(normalizedLiveTurn.id);
                if (!existingTurn) {
                  turnsById.set(normalizedLiveTurn.id, normalizedLiveTurn);
                  continue;
                }

                const mergedItems = [...existingTurn.items];
                for (const item of normalizedLiveTurn.items) {
                  const itemIndex = mergedItems.findIndex((candidate) => candidate.id === item.id);
                  if (itemIndex >= 0) {
                    mergedItems[itemIndex] = item;
                    continue;
                  }
                  mergedItems.push(item);
                }

                turnsById.set(normalizedLiveTurn.id, {
                  ...existingTurn,
                  ...normalizedLiveTurn,
                  items: mergedItems,
                  status:
                    String(normalizedLiveTurn.status ?? "") === "inProgress" || String(existingTurn.status ?? "") !== "inProgress"
                      ? normalizedLiveTurn.status
                      : existingTurn.status,
                  startedAt: normalizedLiveTurn.startedAt ?? existingTurn.startedAt,
                  completedAt: normalizedLiveTurn.completedAt ?? existingTurn.completedAt,
                  durationMs: normalizedLiveTurn.durationMs ?? existingTurn.durationMs,
                  error: normalizedLiveTurn.error ?? existingTurn.error
                });
              }
            } catch {
              // Keep rollout-derived history even if the live thread snapshot is temporarily unavailable.
            }
          }

          const fullTurns = [...turnsById.values()].map((turn) => ({
            ...turn,
            items: [...turn.items]
          }));
          this.cacheFullTurns(threadId, fullTurns);
          initial.turns = fullTurns.map((turn) => this.prepareSummaryTurnForClient(threadId, turn));
          initial.state = "complete";
          initial.totalTurns = turnsById.size;
          initial.loadedTurns = initial.turns.length;
          initial.remainingTurns = 0;
          initial.message = null;
          const activeTurnId = findActiveTurnId({ turns: initial.turns });
          if (activeTurnId) {
            this.activeTurns.set(threadId, activeTurnId);
          } else {
            this.activeTurns.delete(threadId);
          }
          this.pruneHydrationCache();
          return;
        }

        const thread = await this.readThread(threadId, true);
        const turns = Array.isArray(thread.turns) ? (thread.turns as SessionDetailPayload["thread"]["turns"]) : [];
        const totalTurns = turns.length;
        this.cacheFullTurns(threadId, turns);
        initial.turns = turns.map((turn) => this.prepareSummaryTurnForClient(threadId, turn));
        initial.state = "complete";
        initial.totalTurns = totalTurns;
        initial.loadedTurns = initial.turns.length;
        initial.remainingTurns = 0;
        initial.message = null;
        const activeTurnId = findActiveTurnId(thread);
        if (activeTurnId) {
          this.activeTurns.set(threadId, activeTurnId);
        } else {
          this.activeTurns.delete(threadId);
        }
        this.pruneHydrationCache();
      } catch (error) {
        initial.state = "error";
        initial.message = error instanceof Error ? error.message : "Failed to load session history.";
      } finally {
        this.hydrationJobs.delete(threadId);
      }
    })();

    this.hydrationJobs.set(threadId, job);
    await job;
    return this.getHydrationState(threadId);
  }

  private handleNotification(method: string, params: Record<string, unknown>) {
    if (method === "account/updated") {
      this.emitGlobal({
        kind: "notification",
        method: "codex-webui/accountUpdated",
        params
      });
      return;
    }

    if (method === "account/login/completed") {
      this.emitGlobal({
        kind: "notification",
        method: "codex-webui/accountLoginCompleted",
        params: {
          loginId: typeof params.loginId === "string" ? params.loginId : null,
          success: Boolean(params.success),
          error: typeof params.error === "string" ? params.error : null
        }
      });
      return;
    }

    if (method === "account/rateLimits/updated") {
      this.emitGlobal({
        kind: "notification",
        method: "codex-webui/accountRateLimitsUpdated",
        params
      });
      return;
    }

    const threadId = this.extractThreadId(method, params);
    if (!threadId) {
      return;
    }

    if (method.startsWith("turn/") || method.startsWith("item/")) {
      this.sessionHydrations.delete(threadId);
      this.fullTurnsByThread.delete(threadId);
    }

    if (method === "turn/started") {
      const turn = asRecord(params.turn);
      this.activeTurns.set(threadId, String(turn.id ?? ""));
      this.loadedThreadIds.add(threadId);
      this.loadedThreadIdsLoadedAt = Date.now();
      void this.cancelScheduledShutdownForActivity();
      void this.setSessionHighlight(threadId, null, null, null, null, "running", Math.floor(Date.now() / 1000));
      void this.finalizeAutomationRunForSession(threadId, "running");
    } else if (method === "turn/completed") {
      const turn = asRecord(params.turn);
      if (this.activeTurns.get(threadId) === String(turn.id ?? "")) {
        this.activeTurns.delete(threadId);
      }
      void this.maybeDrainQueue(threadId);
      void this.maybeScheduleGlobalShutdown(String(turn.id ?? ""));
      void this.finalizeAutomationRunForSession(threadId, "completed");
      void this.enqueueAppNotification(
        buildNotificationPayload(
          "sessionCompleted",
          threadId,
          this.recentLiveSessions.get(threadId)?.name ?? null,
          {
            turnId: String(turn.id ?? "")
          }
        )
      );
      this.emitGlobal({
        kind: "notification",
        method: "codex-webui/sessionAttention",
        params: {
          sessionId: threadId,
          reason: "completed"
        }
      });
      void this.setSessionHighlight(
        threadId,
        {
          kind: "completed",
          at: Date.now()
        },
        null,
        null,
        null,
        "completed",
        Math.floor(Date.now() / 1000)
      );
    } else if (method === "thread/status/changed") {
      const nextStatus = normalizeThreadStatus(params.status) ?? "unknown";
      if (nextStatus !== "running" && nextStatus !== "active") {
        this.activeTurns.delete(threadId);
        void this.maybeDrainQueue(threadId);
        void this.maybeScheduleGlobalShutdown(null);
      } else {
        void this.cancelScheduledShutdownForActivity();
      }
      if (nextStatus === "notLoaded") {
        this.loadedThreadIds.delete(threadId);
      } else {
        this.loadedThreadIds.add(threadId);
      }
      this.loadedThreadIdsLoadedAt = Date.now();
    } else if (method === "serverRequest/resolved") {
      const requestId = String(params.requestId ?? "");
      const pendingRequests = this.pendingRequests.get(threadId);
      pendingRequests?.delete(requestId);
      if (!pendingRequests || pendingRequests.size === 0) {
        void this.setSessionHighlight(threadId, null);
      }
    } else if (method === "thread/tokenUsage/updated") {
      const tokenUsage = normalizeTokenUsage(params.tokenUsage);
      if (tokenUsage) {
        this.tokenUsageByThread.set(threadId, tokenUsage);
      }
    }

    if (method === "item/commandExecution/outputDelta") {
      const turnId = String(params.turnId ?? "");
      const itemId = String(params.itemId ?? "");
      const delta = String(params.delta ?? "");
      if (turnId && itemId && delta) {
        this.appendCommandExecutionDetail(threadId, turnId, itemId, delta);
      }
      this.emit(threadId, {
        kind: "notification",
        method,
        params: {
          ...params,
          delta,
          deltaLength: delta.length
        }
      });
      return;
    }

    this.emit(threadId, {
      kind: "notification",
      method,
      params:
        method === "item/started" || method === "item/completed"
          ? {
              ...params,
              item: this.prepareItemForClient(threadId, String(params.turnId ?? ""), params.item)
            }
        : method === "thread/name/updated"
          ? {
              ...params,
              threadName: asText(params.threadName)
            }
        : method === "thread/status/changed"
          ? {
              ...params,
              status: normalizeThreadStatus(params.status) ?? "unknown"
              }
        : method === "thread/tokenUsage/updated"
            ? {
                ...params,
                tokenUsage: normalizeTokenUsage(params.tokenUsage)
              }
            : params
    });

    if (method === "thread/name/updated") {
      void this.emitSessionSummaryUpdated(threadId);
    }

    if (method === "thread/status/changed") {
      void this.emitSessionSummaryUpdated(
        threadId,
        null,
        null,
        null,
        normalizeThreadStatus(params.status) ?? "unknown",
        Math.floor(Date.now() / 1000)
      );
    }

    if (method === "thread/archived" || method === "thread/unarchived") {
      this.rolloutPaths.delete(threadId);
      this.sessionHydrations.delete(threadId);
      this.itemDetailsByThread.delete(threadId);
      this.fullTurnsByThread.delete(threadId);
      this.emitGlobal({
        kind: "notification",
        method: "codex-webui/sessionListsInvalidated",
        params: {
          threadId,
          archived: method === "thread/archived"
        }
      });
    }
  }

  private async handleServerRequest(payload: { id: string | number; method: string; params: Record<string, unknown> }) {
    const threadId = String(payload.params.threadId ?? "");
    if (!threadId) {
      return;
    }

    if (await this.tryAutoApprove(threadId, payload)) {
      return;
    }

    const pending = {
      id: String(payload.id),
      rawId: payload.id,
      method: payload.method,
      params: payload.params,
      createdAt: new Date().toISOString()
    } satisfies InternalPendingRequest;

    const requests = this.pendingRequests.get(threadId) ?? new Map<string, InternalPendingRequest>();
    requests.set(pending.id, pending);
    this.pendingRequests.set(threadId, requests);

    this.emit(threadId, {
      kind: "serverRequest",
      id: pending.id,
      method: pending.method,
      params: pending.params
    });
    this.emitGlobal({
      kind: "notification",
      method: "codex-webui/sessionAttention",
      params: {
        sessionId: threadId,
        reason: "approval"
      }
    });
    void this.enqueueAppNotification(
      buildNotificationPayload("sessionAttention", threadId, this.recentLiveSessions.get(threadId)?.name ?? null, {
        reason: "approval",
        requestId: pending.id,
        requestMethod: pending.method
      })
    );
    void this.setSessionHighlight(threadId, {
      kind: "attention",
      at: Date.now()
    });
  }

  private async tryAutoApprove(
    threadId: string,
    payload: { id: string | number; method: string; params: Record<string, unknown> }
  ) {
    const preferences = (await uiStateStore.get(threadId)) ?? this.profile.defaults;
    if (preferences.autoApproveMode === "manual") {
      return false;
    }

    let result: Record<string, unknown> | null = null;

    if (payload.method === "item/commandExecution/requestApproval" || payload.method === "item/fileChange/requestApproval") {
      result = {
        decision: preferences.autoApproveMode === "session" ? "acceptForSession" : "accept"
      };
    } else if (payload.method === "item/permissions/requestApproval") {
      result = {
        scope: preferences.autoApproveMode,
        permissions: payload.params.permissions ?? {}
      };
    }

    if (!result) {
      return false;
    }

    await this.client.respond(payload.id, result);
    this.emit(threadId, {
      kind: "notification",
      method: "codex-webui/autoApproved",
      params: {
        requestId: String(payload.id),
        requestMethod: payload.method,
        autoApproveMode: preferences.autoApproveMode
      }
    });
    return true;
  }

  private emitQueueUpdated(threadId: string, queue?: SessionQueuePayload) {
    void (async () => {
      const nextQueue = queue ?? (await this.getQueue(threadId));
      this.emit(threadId, {
        kind: "notification",
        method: "codex-webui/queueUpdated",
        params: {
          queue: nextQueue
        }
      });
      void this.emitSessionSummaryUpdated(threadId, null, null, nextQueue.items.length);
    })();
  }

  private async withQueueDispatchLock<T>(threadId: string, work: () => Promise<T>) {
    if (this.drainingQueues.has(threadId)) {
      return null;
    }

    this.drainingQueues.add(threadId);
    try {
      return await work();
    } finally {
      this.drainingQueues.delete(threadId);
    }
  }

  private async emitConfigUpdated() {
    const runtimeConfig = this.runtimeConfig;
    const [globalState, notifications, savedFilters, knownTags, promptPresets, automations, automationRuns] = await Promise.all([
      uiStateStore.getGlobal(),
      uiStateStore.getNotifications(1),
      uiStateStore.getSavedSessionFilters(),
      uiStateStore.getKnownSessionTags(),
      uiStateStore.getPromptPresets(),
      uiStateStore.getAutomations(),
      uiStateStore.getAutomationRuns(DEFAULT_AUTOMATION_RUN_HISTORY_LIMIT)
    ]);
    this.emitGlobal({
      kind: "notification",
      method: "codex-webui/configUpdated",
      params: {
        defaults: this.profile.defaults,
        systemShutdown: {
          available: runtimeConfig.systemShutdownEnabled,
          delaySeconds: runtimeConfig.systemShutdownDelaySeconds,
          armed: runtimeConfig.systemShutdownEnabled && globalState.shutdownAfterQueueCompletes
        },
        startup: {
          scheduledShutdown:
            runtimeConfig.systemShutdownEnabled &&
            globalState.scheduledShutdown &&
            globalState.scheduledShutdown.scheduledFor > Date.now()
              ? globalState.scheduledShutdown
              : null
        },
        notifications: {
          unreadCount: notifications.unreadCount,
          settings: await uiStateStore.getNotificationSettings()
        },
        sessionOrganization: {
          savedFilters,
          knownTags
        },
        promptPresets,
        automations: {
          items: automations,
          recentRuns: automationRuns
        }
      }
    });
  }

  private invalidateAccountStateCache() {
    this.accountStateCache = null;
  }

  private async readAccountState(force = false): Promise<GatewayAccountState> {
    const now = Date.now();
    const cached = this.accountStateCache;
    if (!force && cached && cached.expiresAt > now) {
      return {
        account: { ...cached.value.account },
        requiresOpenaiAuth: cached.value.requiresOpenaiAuth
      };
    }

    try {
      const response = asRecord(await this.client.request("account/read", { refreshToken: false }));
      const value = {
        account: asRecord(response.account),
        requiresOpenaiAuth: Boolean(response.requiresOpenaiAuth)
      } satisfies GatewayAccountState;
      this.accountStateCache = {
        value,
        expiresAt: now + ACCOUNT_STATE_CACHE_TTL_MS
      };
      return {
        account: { ...value.account },
        requiresOpenaiAuth: value.requiresOpenaiAuth
      };
    } catch (error) {
      if (!isInvalidRefreshTokenError(error)) {
        throw error;
      }

      const fallback = {
        account: {},
        requiresOpenaiAuth: true
      } satisfies GatewayAccountState;
      this.accountStateCache = {
        value: fallback,
        expiresAt: now + 5_000
      };
      return {
        account: {},
        requiresOpenaiAuth: true
      };
    }
  }

  private clearAutomationTimer(automationId: string) {
    const timer = this.automationTimers.get(automationId);
    if (timer) {
      clearTimeout(timer);
      this.automationTimers.delete(automationId);
    }
  }

  private async restoreAutomationSchedules() {
    const automations = await uiStateStore.getAutomations();
    for (const automation of automations) {
      this.scheduleAutomation(automation);
    }
  }

  private scheduleAutomation(automation: AutomationDefinition) {
    this.clearAutomationTimer(automation.id);
    if (!automation.enabled || automation.scheduleMode !== "interval" || !automation.intervalMinutes) {
      return;
    }

    const nextRunAt = automation.nextRunAt ?? Date.now() + automation.intervalMinutes * 60_000;
    const delayMs = Math.max(0, nextRunAt - Date.now());
    const timer = setTimeout(() => {
      void this.runAutomation(automation.id, "schedule").catch(async () => {
        const updatedAutomation = await uiStateStore.updateAutomation(automation.id, {
          nextRunAt: Date.now() + automation.intervalMinutes! * 60_000
        });
        if (updatedAutomation) {
          this.scheduleAutomation(updatedAutomation);
        }
        await this.emitConfigUpdated();
      });
    }, delayMs);
    this.automationTimers.set(automation.id, timer);
  }

  private async finalizeAutomationRunForSession(
    sessionId: string,
    status: Extract<AutomationRun["status"], "running" | "started" | "completed" | "failed">,
    error: string | null = null
  ) {
    const run = (await uiStateStore.getAutomationRuns(200)).find(
      (entry) => entry.sessionId === sessionId && (entry.status === "running" || entry.status === "started")
    );
    if (!run) {
      return;
    }

    await uiStateStore.updateAutomationRun(run.id, {
      status,
      completedAt: status === "completed" || status === "failed" ? Date.now() : run.completedAt,
      error
    });
    await this.emitConfigUpdated();
  }

  private async enqueueAppNotification(notification: AppNotification) {
    const settings = await uiStateStore.getNotificationSettings();
    if (!settings.enabledEventTypes.includes(notification.type)) {
      return;
    }

    await uiStateStore.addNotification(notification);
    const unreadCount = (await uiStateStore.getNotifications(1)).unreadCount;
    this.emitGlobal({
      kind: "notification",
      method: "codex-webui/notificationAdded",
      params: {
        notification,
        unreadCount
      }
    });
    void this.emitConfigUpdated();
    void this.deliverNotificationHooks(notification, settings);
  }

  private async deliverNotificationHooks(notification: AppNotification, settings: NotificationSettings) {
    const { title, body } = describeNotification(notification);
    const deliveries: Array<Promise<unknown>> = [];

    if (settings.slackWebhookUrl) {
      deliveries.push(
        fetch(settings.slackWebhookUrl, {
          method: "POST",
          headers: {
            "content-type": "application/json"
          },
          body: JSON.stringify({
            text: `*${title}*\n${body}`
          })
        }).catch(() => null)
      );
    }

    if (settings.webhookUrl) {
      deliveries.push(
        fetch(settings.webhookUrl, {
          method: "POST",
          headers: {
            "content-type": "application/json"
          },
          body: JSON.stringify({
            notification
          })
        }).catch(() => null)
      );
    }

    if (deliveries.length > 0) {
      await Promise.allSettled(deliveries);
    }
  }

  private armScheduledShutdown(shutdown: StartupScheduledShutdownAlert) {
    if (this.shutdownTimer) {
      clearTimeout(this.shutdownTimer);
      this.shutdownTimer = null;
    }

    this.shutdownScheduledFor = shutdown.scheduledFor;

    const delayMs = Math.max(0, shutdown.scheduledFor - Date.now());
    this.shutdownTimer = setTimeout(() => {
      void this.executeScheduledShutdown();
    }, delayMs);
  }

  private async clearScheduledShutdown() {
    if (this.shutdownTimer) {
      clearTimeout(this.shutdownTimer);
      this.shutdownTimer = null;
    }

    this.shutdownScheduledFor = null;
    await uiStateStore.setScheduledShutdown(null);
    await this.emitConfigUpdated();
  }

  private async executeScheduledShutdown() {
    const runtimeConfig = this.runtimeConfig;
    this.shutdownTimer = null;
    this.shutdownScheduledFor = null;
    await uiStateStore.setScheduledShutdown(null);
    await this.emitConfigUpdated();

    const command =
      runtimeConfig.systemShutdownCommandOverride ??
      (process.platform === "darwin"
        ? "osascript"
        : process.platform === "win32"
          ? "shutdown"
          : "shutdown");
    const args =
      runtimeConfig.systemShutdownCommandOverride
        ? []
        : process.platform === "darwin"
          ? ["-e", 'tell app "System Events" to shut down']
          : process.platform === "win32"
            ? ["/s", "/t", "0"]
            : ["-h", "now"];

    execFile(command, args, (error) => {
      if (!error) {
        return;
      }

      this.emitGlobal({
        kind: "notification",
        method: "codex-webui/shutdownFailed",
        params: {
          message: error.message
        }
      });
    });
  }

  private async restorePersistedShutdownState() {
    const runtimeConfig = this.runtimeConfig;
    const globalState = await uiStateStore.getGlobal();

    if (!runtimeConfig.systemShutdownEnabled) {
      if (globalState.scheduledShutdown || globalState.shutdownAfterQueueCompletes) {
        await uiStateStore.setGlobalShutdownAfterQueueCompletes(false);
        await uiStateStore.setScheduledShutdown(null);
      }
      return;
    }

    if (globalState.scheduledShutdown) {
      this.armScheduledShutdown(globalState.scheduledShutdown);
    } else if (globalState.shutdownAfterQueueCompletes) {
      await this.maybeScheduleGlobalShutdown(null);
    }
  }

  private async hasOutstandingQueuedWork() {
    const queueCounts = await uiStateStore.getQueueCounts();
    return Object.values(queueCounts).some((count) => count > 0);
  }

  private async hasActiveWorkAcrossThreads() {
    if (this.activeTurns.size > 0) {
      return true;
    }

    let cursor: string | null = null;
    do {
      const response = asRecord(await this.client.request("thread/list", { limit: 200, archived: false, cursor }));
      const threads = (response.data as Array<Record<string, unknown>> | undefined) ?? [];
      if (threads.some((thread) => LIVE_THREAD_STATUSES.has(String(normalizeThreadStatus(thread.status) ?? "unknown")))) {
        return true;
      }
      cursor = typeof response.nextCursor === "string" && response.nextCursor.trim() ? String(response.nextCursor) : null;
    } while (cursor);

    return false;
  }

  private async maybeScheduleGlobalShutdown(completedTurnId: string | null) {
    const runtimeConfig = this.runtimeConfig;
    if (!runtimeConfig.systemShutdownEnabled || this.shutdownTimer) {
      return;
    }

    const globalState = await uiStateStore.getGlobal();
    if (!globalState.shutdownAfterQueueCompletes) {
      return;
    }

    if (await this.hasOutstandingQueuedWork()) {
      return;
    }

    if (await this.hasActiveWorkAcrossThreads()) {
      return;
    }

    const scheduledShutdown = {
      sessionId: null,
      scheduledFor: Date.now() + runtimeConfig.systemShutdownDelaySeconds * 1000,
      delaySeconds: runtimeConfig.systemShutdownDelaySeconds
    } satisfies StartupScheduledShutdownAlert;

    await uiStateStore.setScheduledShutdown(scheduledShutdown);
    this.armScheduledShutdown(scheduledShutdown);
    this.emitGlobal({
      kind: "notification",
      method: "codex-webui/shutdownScheduled",
      params: {
        delaySeconds: runtimeConfig.systemShutdownDelaySeconds,
        turnId: completedTurnId,
        scheduledFor: scheduledShutdown.scheduledFor,
        sessionId: null
      }
    });
    void this.enqueueAppNotification(
      buildNotificationPayload("shutdownScheduled", null, null, {
        delaySeconds: runtimeConfig.systemShutdownDelaySeconds,
        scheduledFor: scheduledShutdown.scheduledFor,
        turnId: completedTurnId
      })
    );
    await this.emitConfigUpdated();
  }

  private async cancelScheduledShutdownForActivity() {
    const globalState = await uiStateStore.getGlobal();
    if (!globalState.shutdownAfterQueueCompletes || !globalState.scheduledShutdown) {
      return;
    }

    await this.clearScheduledShutdown();
  }

  private async maybeDrainQueue(threadId: string) {
    await this.withQueueDispatchLock(threadId, async () => {
      const queue = await this.getQueue(threadId);
      if (queue.items.length === 0) {
        await this.maybeScheduleGlobalShutdown(null);
        return;
      }
      if (queue.resumeRequired) {
        return;
      }

      const activeTurnId = await this.resolveActiveTurnId(threadId);
      if (activeTurnId) {
        return;
      }

      const queuedItem = queue.items[0];
      if (!queuedItem) {
        return;
      }

      try {
        const attachments = (await listAttachments(threadId)).filter((attachment) => queuedItem.attachmentIds.includes(attachment.id));
        await this.sendMessage(threadId, queuedItem.prompt, attachments, {});
        await uiStateStore.removeQueueItem(threadId, queuedItem.id);
        this.emitQueueUpdated(threadId);
      } catch (error) {
        const parsedError = parseAppError(error);
        this.emit(threadId, {
          kind: "notification",
          method: "codex-webui/queueDispatchFailed",
          params: {
            queueId: queuedItem.id,
            code: parsedError?.code ?? null,
            message: parsedError?.message ?? (error instanceof Error ? error.message : "Failed to dispatch queued message.")
          }
        });
        void this.enqueueAppNotification(
          buildNotificationPayload("queueDispatchFailed", threadId, this.recentLiveSessions.get(threadId)?.name ?? null, {
            queueId: queuedItem.id,
            code: parsedError?.code ?? null,
            message: parsedError?.message ?? (error instanceof Error ? error.message : "Failed to dispatch queued message.")
          })
        );
      }
    });
  }

  private emit(threadId: string, event: StreamEvent) {
    const subscribers = this.streamSubscribers.get(threadId);
    if (!subscribers) {
      return;
    }
    for (const listener of subscribers) {
      listener(event);
    }
  }

  private emitGlobal(event: GlobalStreamEvent) {
    for (const listener of this.globalSubscribers) {
      listener(event);
    }
  }

  private async emitSessionSummaryUpdated(
    threadId: string,
    thread: Record<string, unknown> | null = null,
    preferences: SessionPreferences | null = null,
    queueCount: number | null = null,
    statusOverride: string | null = null,
    activityAt: number | null = null,
    highlightOverride: SessionSummaryHighlight | null | undefined = undefined
  ) {
    try {
      const resolvedThread = normalizeThread(thread ?? (await this.readThread(threadId, false)));
      const resolvedPreferences = preferences ?? (await this.getPreferences(threadId, resolvedThread));
      const resolvedQueueCount = queueCount ?? (await this.getQueue(threadId)).items.length;
      const resolvedHighlight =
        highlightOverride === undefined ? await uiStateStore.getSessionHighlight(threadId) : highlightOverride;
      const previousSummary = this.recentLiveSessions.get(threadId);
      const summary = applySessionMeta({
        ...toSummary(resolvedThread, resolvedPreferences, false, resolvedQueueCount, resolvedHighlight),
        status: statusOverride ?? (asText(resolvedThread.status) ?? "unknown"),
        updatedAt: Math.max(
          Number(resolvedThread.updatedAt ?? 0),
          previousSummary?.updatedAt ?? 0,
          activityAt ?? 0
        )
      } satisfies SessionSummary, await uiStateStore.getSessionMeta(threadId));
      if ((await arenaStore.getHiddenSessionIds()).has(threadId)) {
        this.recentLiveSessions.delete(threadId);
        return;
      }
      if (isSubagentSessionSummary(summary)) {
        return;
      }
      this.recentLiveSessions.set(threadId, summary);
      this.recentLiveSessionsLoadedAt = Date.now();
      this.emitGlobal({
        kind: "notification",
        method: "codex-webui/sessionSummaryUpdated",
        params: {
          session: summary
        }
      });
    } catch {
      // Ignore non-fatal summary refresh failures so live turn streaming can continue.
    }
  }

  private async setSessionHighlight(
    threadId: string,
    highlight: SessionSummaryHighlight | null,
    thread: Record<string, unknown> | null = null,
    preferences: SessionPreferences | null = null,
    queueCount: number | null = null,
    statusOverride: string | null = null,
    activityAt: number | null = null
  ) {
    const changed = await uiStateStore.setSessionHighlight(threadId, highlight);
    if (!changed && highlight === null && statusOverride === null && activityAt === null) {
      return;
    }

    await this.emitSessionSummaryUpdated(
      threadId,
      thread,
      preferences,
      queueCount,
      statusOverride,
      activityAt,
      highlight
    );
  }

  private extractThreadId(method: string, params: Record<string, unknown>) {
    if (typeof params.threadId === "string") {
      return params.threadId;
    }
    if (method.startsWith("thread/") && typeof asRecord(params.thread).id === "string") {
      return String(asRecord(params.thread).id);
    }
    return null;
  }
}

const gatewayInstances = new Map<string, CodexGateway>();

function getGatewayForProfile(profileId: string | null = null) {
  const resolvedProfile = getRuntimeProfile(profileId ?? getCurrentRuntimeProfile().id);
  let gateway = gatewayInstances.get(resolvedProfile.id);
  if (!gateway) {
    gateway = new CodexGateway(resolvedProfile);
    gatewayInstances.set(resolvedProfile.id, gateway);
  }
  return {
    gateway,
    profile: resolvedProfile
  };
}

export const codexGateway = new Proxy(
  {} as CodexGateway,
  {
    get(_target, property) {
      const { gateway, profile } = getGatewayForProfile();
      const value = Reflect.get(gateway, property);
      if (typeof value !== "function") {
        return value;
      }
      return (...args: unknown[]) => runWithProfile(profile.id, () => Reflect.apply(value, gateway, args));
    }
  }
);
