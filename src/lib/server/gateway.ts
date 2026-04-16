import { execFile } from "node:child_process";
import { createReadStream } from "node:fs";
import { readdir } from "node:fs/promises";
import path from "node:path";
import readline from "node:readline";
import type {
  AppConfigPayload,
  AttachmentRecord,
  CodexItem,
  CodexTurn,
  CollaborationModeOption,
  GlobalStreamEvent,
  PendingServerRequest,
  SessionDetailPayload,
  SessionDraftPayload,
  SessionItemDetailPayload,
  SessionListPayload,
  SessionPreferences,
  SessionQueueItem,
  SessionQueuePayload,
  SessionSummary,
  StartupScheduledShutdownAlert,
  SessionTurnPayload,
  SessionTurnsPagePayload,
  StreamEvent,
  ThreadTokenUsage
} from "$lib/types";
import { stripAttachmentPreamble } from "$lib/attachments";

import { listAttachments } from "./attachments";
import { AppServerClient } from "./app-server/client";
import { configTomlPath, syncCodexTomlWithPreferences } from "./codex-config";
import { getRuntimeConfig } from "./env";
import { buildSandboxPolicy, listDirectoryPayload, resolveAllowedDirectory } from "./fs";
import { resolveGitRepository } from "./git";
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
const HYDRATION_CACHE_LIMIT = 2;
const DEFERRED_ITEM_TYPES = new Set(["commandExecution", "fileChange", "mcpToolCall", "dynamicToolCall", "webSearch"]);
const DEFAULT_THREAD_NAME = "New thread";
const LIVE_THREAD_STATUSES = new Set(["running", "active"]);

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

function coercePreferences(preferences: Partial<SessionPreferences> | null | undefined) {
  const defaults = getRuntimeConfig().defaults;
  return {
    ...defaults,
    ...preferences
  } satisfies SessionPreferences;
}

function toSummary(
  thread: Record<string, unknown>,
  preferences: SessionPreferences | null,
  archived = false,
  queueCount = 0
): SessionSummary {
  const normalized = normalizeThread(thread);
  const preview = asText(normalized.preview) ?? "";
  return {
    id: String(normalized.id),
    name: getDisplayThreadName(asText(normalized.name), preview),
    preview,
    queueCount: Math.max(0, queueCount),
    cwd: (normalized.cwd as string | null) ?? preferences?.cwd ?? getRuntimeConfig().defaults.cwd,
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
  queueCount: number
): SessionSummary {
  return {
    id: indexed.id,
    name: indexed.name,
    preview: indexed.preview,
    queueCount: Math.max(0, queueCount),
    cwd: indexed.cwd || preferences?.cwd || getRuntimeConfig().defaults.cwd,
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
  queueCounts: Record<string, number>,
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
        return indexedSessionToSummary(entry, stored ? coercePreferences(stored) : null, queueCounts[entry.id] ?? 0);
      }

      return {
        ...live,
        name: !isPlaceholderThreadName(live.name) ? live.name : entry.name,
        preview: live.preview?.trim() ? live.preview : entry.preview,
        cwd: live.cwd || entry.cwd,
        createdAt: live.createdAt || entry.createdAt,
        updatedAt: Math.max(live.updatedAt || 0, entry.updatedAt || 0)
      } satisfies SessionSummary;
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
      sessions.push(live);
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

export class CodexGateway {
  private readonly client = new AppServerClient();
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
  private recentLiveSessions = new Map<string, SessionSummary>();
  private recentLiveSessionsLoadedAt = 0;
  private recentLiveSessionsPromise: Promise<Map<string, SessionSummary>> | null = null;
  private loadedThreadIds = new Set<string>();
  private loadedThreadIdsLoadedAt = 0;
  private loadedThreadIdsPromise: Promise<Set<string>> | null = null;
  private shutdownTimer: ReturnType<typeof setTimeout> | null = null;
  private shutdownScheduledFor: number | null = null;

  constructor() {
    void uiStateStore.markQueuesPendingResume();
    void this.restorePersistedShutdownState();
    this.client.onNotification((payload) => this.handleNotification(payload.method, payload.params));
    this.client.onServerRequest((payload) => {
      void this.handleServerRequest(payload);
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
    const [modelsResponse, collaborationResponse, accountResponse] = await Promise.all([
      this.client.request("model/list", { includeHidden: false }),
      this.client.request("collaborationMode/list", {}),
      this.client.request("account/read", { refreshToken: false })
    ]);
    const [pausedQueueEntries, globalState] = await Promise.all([uiStateStore.listResumePendingQueues(), uiStateStore.getGlobal()]);
    const preferences: Record<string, SessionPreferences> = pausedQueueEntries.length > 0 ? await uiStateStore.getAll() : {};
    const indexedSessions =
      pausedQueueEntries.length > 0 ? await sessionIndexClient.list(getRuntimeConfig().codexHome).catch(() => []) : [];
    const indexedById = new Map(indexedSessions.map((session) => [session.id, session]));
    const pausedQueues = await Promise.all(
      pausedQueueEntries.map(async (entry) => {
        const indexedSession = indexedById.get(entry.threadId) ?? null;
        let name = getDisplayThreadName(indexedSession?.name ?? null, indexedSession?.preview ?? null);
        let cwd = indexedSession?.cwd || preferences[entry.threadId]?.cwd || getRuntimeConfig().defaults.cwd;

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
    const account = asRecord(asRecord(accountResponse).account);

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
      defaults: getRuntimeConfig().defaults,
      paths: {
        codexHome: getRuntimeConfig().codexHome,
        configFilePath: configTomlPath(getRuntimeConfig().codexHome)
      },
      git: {
        discoveryDepth: getRuntimeConfig().gitDiscoveryDepth
      },
      systemShutdown: {
        available: getRuntimeConfig().systemShutdownEnabled,
        delaySeconds: getRuntimeConfig().systemShutdownDelaySeconds,
        armed: getRuntimeConfig().systemShutdownEnabled && globalState.shutdownAfterQueueCompletes
      },
      startup: {
        pausedQueues,
        scheduledShutdown:
          getRuntimeConfig().systemShutdownEnabled &&
          globalState.scheduledShutdown &&
          globalState.scheduledShutdown.scheduledFor > Date.now()
            ? globalState.scheduledShutdown
            : null
      },
      account: {
        type: (account.type as "apiKey" | "chatgpt" | null) ?? null,
        email: (account.email as string | null) ?? null,
        planType: (account.planType as string | null) ?? null,
        requiresOpenaiAuth: Boolean(asRecord(accountResponse).requiresOpenaiAuth)
      }
    };
  }

  async listSessions(archived = false, cursor: string | null = null, limit = SESSION_WINDOW_SIZE): Promise<SessionListPayload> {
    if (!archived) {
      const [preferences, queueCounts, indexedPage, recentLiveSessions] = await Promise.all([
        uiStateStore.getAll(),
        uiStateStore.getQueueCounts(),
        sessionIndexClient.page(getRuntimeConfig().codexHome, cursor, limit, null),
        this.getRecentLiveSessionSummaries().catch(() => new Map<string, SessionSummary>())
      ]);

      return {
        sessions: mergeIndexedSessionsWithRecentLiveSessions(
          indexedPage.entries,
          preferences,
          queueCounts,
          recentLiveSessions,
          limit,
          null,
          cursor === null || cursor === "0"
        ),
        nextCursor: indexedPage.nextCursor
      };
    }

    const response = asRecord(await this.client.request("thread/list", { limit, archived, cursor }));
    const preferences = await uiStateStore.getAll();
    const threads = (response.data as Array<Record<string, unknown>> | undefined) ?? [];
    return {
      sessions: threads
        .filter((thread) => !isSubagentThread(thread))
        .map((thread) => {
          const stored = preferences[String(thread.id)];
          return toSummary(thread, stored ? coercePreferences(stored) : null, archived);
        })
        .filter((session) => !isSubagentSessionSummary(session))
        .sort(compareSessionSummaries),
      nextCursor: typeof response.nextCursor === "string" && response.nextCursor.trim() ? String(response.nextCursor) : null
    };
  }

  async searchSessions(
    query: string,
    scope: "summary" | "full",
    archived = false,
    cursor: string | null = null,
    limit = SESSION_WINDOW_SIZE
  ): Promise<SessionListPayload> {
    const needle = query.trim().toLowerCase();
    if (!needle) {
      return this.listSessions(archived, cursor, limit);
    }

    if (!archived && scope === "summary") {
      const [preferences, queueCounts, indexedPage, recentLiveSessions] = await Promise.all([
        uiStateStore.getAll(),
        uiStateStore.getQueueCounts(),
        sessionIndexClient.page(getRuntimeConfig().codexHome, cursor, limit, needle),
        this.getRecentLiveSessionSummaries().catch(() => new Map<string, SessionSummary>())
      ]);

      return {
        sessions: mergeIndexedSessionsWithRecentLiveSessions(
          indexedPage.entries,
          preferences,
          queueCounts,
          recentLiveSessions,
          limit,
          (session) => `${session.name ?? ""}
${session.preview ?? ""}`.toLowerCase().includes(needle),
          cursor === null || cursor === "0"
        ),
        nextCursor: indexedPage.nextCursor
      };
    }

    const sessions = await this.collectListableSessions(archived);
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

  private async collectListableSessions(archived: boolean): Promise<SessionSummary[]> {
    const preferences = await uiStateStore.getAll();
    const queueCounts = await uiStateStore.getQueueCounts();
    if (archived) {
      return this.listAllThreadSessions(true, preferences, queueCounts);
    }

    const indexedSessions = (await sessionIndexClient.list(getRuntimeConfig().codexHome).catch(() => [])).filter(
      (session) => !session.isSubagent
    );
    const liveThreads = await this.listAllThreadSessions(false, preferences, queueCounts);
    const liveById = new Map(liveThreads.map((session) => [session.id, session]));
    const mergedSessions = indexedSessions.map((session) => {
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
      return {
        id: session.id,
        name: session.name,
        preview: session.preview,
        queueCount: queueCounts[session.id] ?? 0,
        cwd: session.cwd || stored?.cwd || getRuntimeConfig().defaults.cwd,
        archived: false,
        createdAt: session.createdAt,
        updatedAt: session.updatedAt,
        status: session.status || "unknown",
        isSubagent: session.isSubagent,
        agentNickname: null,
        agentRole: null,
        preferences: stored ? coercePreferences(stored) : null
      } satisfies SessionSummary;
    });

    for (const live of liveThreads) {
      if (!mergedSessions.some((session) => session.id === live.id)) {
        mergedSessions.push(live);
      }
    }

    const dedupedSessions = mergedSessions.filter(
      (session, index, collection) => collection.findIndex((candidate) => candidate.id === session.id) === index
    );
    const visibleSessions = dedupedSessions.filter((session) => !isSubagentSessionSummary(session));
    visibleSessions.sort(compareSessionSummaries);
    return visibleSessions;
  }

  private async listAllThreadSessions(
    archived: boolean,
    preferences: Record<string, Awaited<ReturnType<typeof uiStateStore.getAll>>[string]>,
    queueCounts: Record<string, number>
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
            return toSummary(thread, stored ? coercePreferences(stored) : null, archived, queueCounts[threadId] ?? 0);
          })
          .filter((session) => !isSubagentSessionSummary(session))
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
      const [preferences, queueCounts] = await Promise.all([uiStateStore.getAll(), uiStateStore.getQueueCounts()]);
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
            const summary = toSummary(thread, stored ? coercePreferences(stored) : null, false, queueCounts[threadId] ?? 0);
            return [threadId, summary] as const;
          })
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
    await this.client.request("thread/archive", { threadId });
    sessionIndexClient.invalidate();
    this.recentLiveSessions.delete(threadId);
    this.rolloutPaths.delete(threadId);
    this.sessionHydrations.delete(threadId);
    this.itemDetailsByThread.delete(threadId);
    this.fullTurnsByThread.delete(threadId);
    return { ok: true };
  }

  async unarchiveSession(threadId: string) {
    const response = asRecord(await this.client.request("thread/unarchive", { threadId }));
    sessionIndexClient.invalidate();
    this.rolloutPaths.delete(threadId);
    this.sessionHydrations.delete(threadId);
    this.itemDetailsByThread.delete(threadId);
    this.fullTurnsByThread.delete(threadId);
    const thread = normalizeThread(asRecord(response.thread));
    const preferences = await this.getPreferences(threadId, thread);
    const summary = toSummary(thread, preferences, false);
    this.recentLiveSessions.set(threadId, summary);
    this.recentLiveSessionsLoadedAt = Date.now();
    return {
      ok: true,
      session: summary
    };
  }

  async getAccount() {
    const response = asRecord(await this.client.request("account/read", { refreshToken: false }));
    return {
      account: asRecord(response.account),
      requiresOpenaiAuth: Boolean(response.requiresOpenaiAuth)
    };
  }

  async startAccountLogin(type: "chatgpt" | "chatgptDeviceCode" | "apiKey", apiKey?: string | null) {
    if (type === "apiKey") {
      if (!apiKey?.trim()) {
        throw new Error("API key is required.");
      }
      return this.client.request("account/login/start", { type, apiKey: apiKey.trim() });
    }

    return this.client.request("account/login/start", { type });
  }

  async cancelAccountLogin(loginId: string) {
    return this.client.request("account/login/cancel", { loginId });
  }

  async logoutAccount() {
    return this.client.request("account/logout", {});
  }

  async createSession(preferences: Partial<SessionPreferences>, name: string | null) {
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

    const summary = toSummary(thread, nextPreferences);
    sessionIndexClient.invalidate();
    this.recentLiveSessions.set(String(thread.id), summary);
    this.recentLiveSessionsLoadedAt = Date.now();
    this.emitGlobal({
      kind: "notification",
      method: "codex-webui/sessionSummaryUpdated",
      params: {
        session: summary
      }
    });
    return summary;
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
    await syncCodexTomlWithPreferences(getRuntimeConfig().codexHome, nextPreferences);
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
    const runtimeConfig = getRuntimeConfig();
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
      throw new Error("Provide a prompt or at least one attachment.");
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
    await uiStateStore.removeQueueItem(threadId, queueId);
    const queue = await this.getQueue(threadId);
    this.emitQueueUpdated(threadId, queue);
    await this.maybeScheduleGlobalShutdown(null);
    return queue;
  }

  async updateQueuedMessage(threadId: string, queueId: string, prompt: string, attachments: AttachmentRecord[]): Promise<SessionQueuePayload> {
    const stored = await uiStateStore.getQueue(threadId);
    const queuedItem = stored?.items.find((item) => item.id === queueId);
    if (!queuedItem) {
      throw new Error("Queued message not found.");
    }

    const trimmedPrompt = prompt.trim();
    if (!trimmedPrompt && attachments.length === 0) {
      throw new Error("Provide a prompt or at least one attachment.");
    }

    const updated = await uiStateStore.updateQueueItem(threadId, queueId, {
      prompt: trimmedPrompt,
      attachmentIds: attachments.map((attachment) => attachment.id),
      attachmentNames: attachments.map((attachment) => attachment.originalName)
    });
    if (!updated) {
      throw new Error("Queued message not found.");
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
        throw new Error("Queued message not found.");
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
      throw new Error("Queue is already dispatching.");
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
      throw new Error("No active turn is available to steer.");
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
      throw new Error("Pending request not found.");
    }
    await this.client.respond(pending.rawId, result);
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
      cwd: String(thread.cwd ?? getRuntimeConfig().defaults.cwd)
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
        const codexHome = getRuntimeConfig().codexHome;
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

          initial.turns = [...turnsById.values()].map((turn) =>
            this.prepareSummaryTurnForClient(threadId, {
              ...turn,
              items: [...turn.items]
            })
          );
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
      void this.emitSessionSummaryUpdated(threadId, null, null, null, "running", Math.floor(Date.now() / 1000));
    } else if (method === "turn/completed") {
      const turn = asRecord(params.turn);
      if (this.activeTurns.get(threadId) === String(turn.id ?? "")) {
        this.activeTurns.delete(threadId);
      }
      void this.maybeDrainQueue(threadId);
      void this.maybeScheduleGlobalShutdown(String(turn.id ?? ""));
      this.emitGlobal({
        kind: "notification",
        method: "codex-webui/sessionAttention",
        params: {
          sessionId: threadId,
          reason: "completed"
        }
      });
      void this.emitSessionSummaryUpdated(threadId, null, null, null, "completed", Math.floor(Date.now() / 1000));
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
      this.pendingRequests.get(threadId)?.delete(requestId);
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
  }

  private async tryAutoApprove(
    threadId: string,
    payload: { id: string | number; method: string; params: Record<string, unknown> }
  ) {
    const preferences = (await uiStateStore.get(threadId)) ?? getRuntimeConfig().defaults;
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
    const runtimeConfig = getRuntimeConfig();
    const globalState = await uiStateStore.getGlobal();
    this.emitGlobal({
      kind: "notification",
      method: "codex-webui/configUpdated",
      params: {
        defaults: runtimeConfig.defaults,
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
        }
      }
    });
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
    const runtimeConfig = getRuntimeConfig();
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
    const runtimeConfig = getRuntimeConfig();
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
    const runtimeConfig = getRuntimeConfig();
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
        this.emit(threadId, {
          kind: "notification",
          method: "codex-webui/queueDispatchFailed",
          params: {
            queueId: queuedItem.id,
            message: error instanceof Error ? error.message : "Failed to dispatch queued message."
          }
        });
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
    activityAt: number | null = null
  ) {
    try {
      const resolvedThread = normalizeThread(thread ?? (await this.readThread(threadId, false)));
      const resolvedPreferences = preferences ?? (await this.getPreferences(threadId, resolvedThread));
      const resolvedQueueCount = queueCount ?? (await this.getQueue(threadId)).items.length;
      const previousSummary = this.recentLiveSessions.get(threadId);
      const summary = {
        ...toSummary(resolvedThread, resolvedPreferences, false, resolvedQueueCount),
        status: statusOverride ?? (asText(resolvedThread.status) ?? "unknown"),
        updatedAt: Math.max(
          Number(resolvedThread.updatedAt ?? 0),
          previousSummary?.updatedAt ?? 0,
          activityAt ?? 0
        )
      } satisfies SessionSummary;
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

export const codexGateway = new CodexGateway();
