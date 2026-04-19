import type { CodexItem, CodexTurn, SessionDetailPayload, StreamEvent } from "$lib/types";

export type ConversationState = SessionDetailPayload & {
  livePlans: Record<string, { explanation: string | null; plan: Array<{ step: string; status: string }> }>;
  liveDiffs: Record<string, string>;
};

function cloneTurn(turn: CodexTurn): CodexTurn {
  return {
    ...turn,
    items: [...turn.items]
  };
}

function ensureTurn(state: ConversationState, turnId: string, seed?: Partial<CodexTurn>) {
  const turns = [...state.thread.turns];
  let index = turns.findIndex((turn) => turn.id === turnId);
  if (index === -1) {
    turns.push({
      id: turnId,
      items: [],
      status: "inProgress",
      error: null,
      startedAt: null,
      completedAt: null,
      durationMs: null,
      ...seed
    });
    index = turns.length - 1;
  } else {
    turns[index] = cloneTurn(turns[index]);
  }
  return {
    turns,
    turn: turns[index],
    index
  };
}

function upsertItem(turn: CodexTurn, item: CodexItem) {
  const index = turn.items.findIndex((candidate) => candidate.id === item.id);
  if (index === -1) {
    turn.items = [...turn.items, item];
    return;
  }
  turn.items = turn.items.map((candidate, candidateIndex) => (candidateIndex === index ? { ...candidate, ...item } : candidate));
}

function resolveConversationRunningTurn(state: ConversationState) {
  if (state.activeTurnId && state.thread.turns.some((turn) => turn.id === state.activeTurnId && String(turn.status ?? "") === "inProgress")) {
    return state.activeTurnId;
  }

  const activeTurn = [...state.thread.turns].reverse().find((turn) => String(turn.status ?? "") === "inProgress");
  return activeTurn ? activeTurn.id : null;
}

function isLiveThreadStatus(status: string | null | undefined) {
  return status === "running" || status === "active";
}

function preferProgressiveString(existing: unknown, incoming: unknown) {
  if (typeof incoming !== "string") {
    return typeof existing === "string" ? existing : incoming;
  }
  if (typeof existing !== "string") {
    return incoming;
  }
  if (!incoming) {
    return existing;
  }
  if (!existing) {
    return incoming;
  }
  if (incoming.includes(existing)) {
    return incoming;
  }
  if (existing.includes(incoming)) {
    return existing;
  }
  return incoming.length >= existing.length ? incoming : existing;
}

function mergeStringArray(existing: unknown, incoming: unknown) {
  const existingEntries = Array.isArray(existing) ? existing.map((entry) => String(entry)) : [];
  const incomingEntries = Array.isArray(incoming) ? incoming.map((entry) => String(entry)) : [];
  if (existingEntries.length === 0) {
    return incomingEntries;
  }
  if (incomingEntries.length === 0) {
    return existingEntries;
  }

  const merged: string[] = [];
  const size = Math.max(existingEntries.length, incomingEntries.length);
  for (let index = 0; index < size; index += 1) {
    merged[index] = preferProgressiveString(existingEntries[index], incomingEntries[index]) as string;
  }
  return merged;
}

function preferRicherArray<T>(existing: unknown, incoming: unknown): T[] | undefined {
  const existingEntries = Array.isArray(existing) ? (existing as T[]) : null;
  const incomingEntries = Array.isArray(incoming) ? (incoming as T[]) : null;
  if (!existingEntries?.length) {
    return incomingEntries ?? undefined;
  }
  if (!incomingEntries?.length) {
    return existingEntries;
  }
  return incomingEntries.length >= existingEntries.length ? incomingEntries : existingEntries;
}

function mergeItem(existingItem: CodexItem | undefined, incomingItem: CodexItem): CodexItem {
  if (!existingItem) {
    return { ...incomingItem };
  }

  const merged: CodexItem = {
    ...existingItem,
    ...incomingItem
  };

  if (existingItem.detailState === "loaded" && incomingItem.detailState !== "loaded") {
    merged.detailState = "loaded";
  } else if (existingItem.detailState === "inline" && incomingItem.detailState === "deferred") {
    merged.detailState = "inline";
  }

  merged.detailPreview =
    typeof incomingItem.detailPreview === "string" && incomingItem.detailPreview.trim()
      ? incomingItem.detailPreview
      : (existingItem.detailPreview ?? null);
  merged.title =
    typeof incomingItem.title === "string" && incomingItem.title.trim()
      ? incomingItem.title
      : (existingItem.title ?? null);

  if ("text" in existingItem || "text" in incomingItem) {
    merged.text = preferProgressiveString(existingItem.text, incomingItem.text);
  }
  if ("aggregatedOutput" in existingItem || "aggregatedOutput" in incomingItem) {
    merged.aggregatedOutput = preferProgressiveString(existingItem.aggregatedOutput, incomingItem.aggregatedOutput);
  }
  if ("summary" in existingItem || "summary" in incomingItem) {
    merged.summary = mergeStringArray(existingItem.summary, incomingItem.summary);
  }
  if ("changes" in existingItem || "changes" in incomingItem) {
    merged.changes = preferRicherArray(existingItem.changes, incomingItem.changes);
  }
  if ("command" in existingItem || "command" in incomingItem) {
    merged.command = preferRicherArray(existingItem.command, incomingItem.command);
  }
  if ("diff" in existingItem || "diff" in incomingItem) {
    merged.diff = preferProgressiveString(existingItem.diff, incomingItem.diff);
  }
  if ("original" in existingItem || "original" in incomingItem) {
    merged.original = preferProgressiveString(existingItem.original, incomingItem.original);
  }
  if ("modified" in existingItem || "modified" in incomingItem) {
    merged.modified = preferProgressiveString(existingItem.modified, incomingItem.modified);
  }
  if ("query" in existingItem || "query" in incomingItem) {
    merged.query = preferProgressiveString(existingItem.query, incomingItem.query);
  }
  if ("prompt" in existingItem || "prompt" in incomingItem) {
    merged.prompt = preferProgressiveString(existingItem.prompt, incomingItem.prompt);
  }
  if (incomingItem.invocation !== undefined || existingItem.invocation !== undefined) {
    merged.invocation = incomingItem.invocation ?? existingItem.invocation;
  }
  if (incomingItem.result !== undefined || existingItem.result !== undefined) {
    merged.result = incomingItem.result ?? existingItem.result;
  }
  if (incomingItem.action !== undefined || existingItem.action !== undefined) {
    merged.action = incomingItem.action ?? existingItem.action;
  }
  if (incomingItem.tool !== undefined || existingItem.tool !== undefined) {
    merged.tool = incomingItem.tool ?? existingItem.tool;
  }
  if (incomingItem.lifecycleStatus !== undefined || existingItem.lifecycleStatus !== undefined) {
    const incomingLifecycle = String(incomingItem.lifecycleStatus ?? "");
    const existingLifecycle = String(existingItem.lifecycleStatus ?? "");
    merged.lifecycleStatus =
      existingLifecycle === "completed" && incomingLifecycle === "inProgress"
        ? existingItem.lifecycleStatus
        : (incomingItem.lifecycleStatus ?? existingItem.lifecycleStatus);
  }

  return merged;
}

function mergeTurnStatus(existingStatus: string | null | undefined, incomingStatus: string | null | undefined) {
  const existing = String(existingStatus ?? "");
  const incoming = String(incomingStatus ?? "");
  if (!existing) {
    return incoming;
  }
  if (!incoming || existing === incoming) {
    return existing || incoming;
  }
  if (existing === "inProgress" && incoming !== "inProgress") {
    return incoming;
  }
  if (incoming === "inProgress" && existing !== "inProgress") {
    return existing;
  }
  return incoming;
}

function mergeTurnItems(existingItems: CodexItem[], incomingItems: CodexItem[]) {
  const existingById = new Map(existingItems.map((item) => [item.id, item] as const));
  const incomingById = new Map(incomingItems.map((item) => [item.id, item] as const));
  const incomingSubsetOfExisting = incomingItems.every((item) => existingById.has(item.id));
  const orderedBase = incomingSubsetOfExisting && existingItems.length >= incomingItems.length ? existingItems : incomingItems;
  const merged = orderedBase.map((baseItem) => mergeItem(existingById.get(baseItem.id), incomingById.get(baseItem.id) ?? baseItem));
  const seenIds = new Set(merged.map((item) => item.id));

  for (const item of incomingItems) {
    if (seenIds.has(item.id)) {
      continue;
    }
    merged.push(mergeItem(existingById.get(item.id), item));
    seenIds.add(item.id);
  }

  for (const item of existingItems) {
    if (seenIds.has(item.id)) {
      continue;
    }
    merged.push({ ...item });
    seenIds.add(item.id);
  }

  return merged;
}

function mergeTurn(existingTurn: CodexTurn | undefined, incomingTurn: CodexTurn): CodexTurn {
  if (!existingTurn) {
    return cloneTurn(incomingTurn);
  }

  const status = mergeTurnStatus(existingTurn.status, incomingTurn.status);
  const completedAt = status === "inProgress" ? null : (incomingTurn.completedAt ?? existingTurn.completedAt);
  const durationMs = status === "inProgress" ? null : (incomingTurn.durationMs ?? existingTurn.durationMs);

  return {
    ...existingTurn,
    ...incomingTurn,
    items: mergeTurnItems(existingTurn.items, incomingTurn.items),
    status,
    error: incomingTurn.error ?? existingTurn.error,
    startedAt: incomingTurn.startedAt ?? existingTurn.startedAt,
    completedAt,
    durationMs,
    detailState:
      existingTurn.detailState === "full" && incomingTurn.detailState !== "full"
        ? "full"
        : (incomingTurn.detailState ?? existingTurn.detailState),
    hiddenItemCount:
      status === "inProgress"
        ? 0
        : Math.max(Number(existingTurn.hiddenItemCount ?? 0), Number(incomingTurn.hiddenItemCount ?? 0))
  };
}

function mergeTurns(existingTurns: CodexTurn[], incomingTurns: CodexTurn[]) {
  const existingById = new Map(existingTurns.map((turn) => [turn.id, turn] as const));
  const merged = incomingTurns.map((turn) => mergeTurn(existingById.get(turn.id), turn));
  const seenIds = new Set(merged.map((turn) => turn.id));

  for (const turn of existingTurns) {
    if (seenIds.has(turn.id)) {
      continue;
    }
    merged.push(cloneTurn(turn));
    seenIds.add(turn.id);
  }

  return merged;
}

function mergePendingRequests(
  existingRequests: SessionDetailPayload["pendingRequests"],
  incomingRequests: SessionDetailPayload["pendingRequests"]
) {
  const merged = new Map(existingRequests.map((request) => [request.id, request] as const));
  for (const request of incomingRequests) {
    merged.set(request.id, request);
  }
  return [...merged.values()].sort((left, right) => String(left.createdAt ?? "").localeCompare(String(right.createdAt ?? "")));
}

function mergeQueue(existingQueue: SessionDetailPayload["queue"], incomingQueue: SessionDetailPayload["queue"]) {
  const existingUpdatedAt = Number(existingQueue.updatedAt ?? 0);
  const incomingUpdatedAt = Number(incomingQueue.updatedAt ?? 0);
  return existingUpdatedAt > incomingUpdatedAt ? existingQueue : incomingQueue;
}

function mergeHydration(
  existingHydration: SessionDetailPayload["hydration"],
  incomingHydration: SessionDetailPayload["hydration"],
  loadedTurns: number
) {
  const totalTurns =
    typeof incomingHydration.totalTurns === "number"
      ? Math.max(incomingHydration.totalTurns, loadedTurns)
      : typeof existingHydration.totalTurns === "number"
        ? Math.max(existingHydration.totalTurns, loadedTurns)
        : null;

  return {
    state: incomingHydration.state,
    loadedTurns: Math.max(incomingHydration.loadedTurns, loadedTurns),
    totalTurns,
    remainingTurns:
      typeof totalTurns === "number"
        ? Math.max(totalTurns - Math.max(incomingHydration.loadedTurns, loadedTurns), 0)
        : incomingHydration.remainingTurns,
    message: incomingHydration.message
  } satisfies SessionDetailPayload["hydration"];
}

export function createConversationState(detail: SessionDetailPayload): ConversationState {
  return {
    ...detail,
    livePlans: {},
    liveDiffs: {}
  };
}

export function mergeConversationState(current: ConversationState, detail: SessionDetailPayload): ConversationState {
  const incoming = createConversationState(detail);
  const mergedTurns = mergeTurns(current.thread.turns, incoming.thread.turns);
  const hasLiveTurn = mergedTurns.some((turn) => String(turn.status ?? "") === "inProgress");
  const threadStatus = hasLiveTurn
    ? (isLiveThreadStatus(incoming.thread.status) ? incoming.thread.status : current.thread.status || "running")
    : incoming.thread.status;

  return {
    ...incoming,
    thread: {
      ...current.thread,
      ...incoming.thread,
      turns: mergedTurns,
      status: threadStatus,
      updatedAt: Math.max(Number(current.thread.updatedAt ?? 0), Number(incoming.thread.updatedAt ?? 0))
    },
    queue: mergeQueue(current.queue, incoming.queue),
    pendingRequests: mergePendingRequests(current.pendingRequests, incoming.pendingRequests),
    tokenUsage: incoming.tokenUsage ?? current.tokenUsage ?? null,
    hydration: mergeHydration(current.hydration, incoming.hydration, mergedTurns.length),
    livePlans: {
      ...current.livePlans
    },
    liveDiffs: {
      ...current.liveDiffs
    }
  };
}

export function applyStreamEvent(current: ConversationState, event: StreamEvent): ConversationState {
  const next: ConversationState = {
    ...current,
    thread: {
      ...current.thread,
      turns: [...current.thread.turns]
    },
    pendingRequests: [...current.pendingRequests],
    hydration: { ...current.hydration },
    livePlans: { ...current.livePlans },
    liveDiffs: { ...current.liveDiffs }
  };

  if (event.kind === "serverRequest") {
    if (!next.pendingRequests.some((request) => request.id === event.id)) {
      next.pendingRequests.push({
        id: event.id,
        method: event.method,
        params: event.params,
        createdAt: new Date().toISOString()
      });
    }
    return next;
  }

  const { method, params } = event;

  if (method === "serverRequest/resolved") {
    next.pendingRequests = next.pendingRequests.filter((request) => request.id !== String(params.requestId ?? ""));
    return next;
  }

  if (method === "thread/name/updated") {
    next.thread.name = (params.threadName as string | null | undefined) ?? null;
    return next;
  }

  if (method === "thread/status/changed") {
    next.thread.status = typeof params.status === "string" ? params.status : next.thread.status;
    if (next.thread.status !== "running" && next.thread.status !== "active") {
      next.activeTurnId = resolveConversationRunningTurn(next);
    }
    return next;
  }

  if (method === "codex-webui/preferencesUpdated") {
    next.preferences = params.preferences as SessionDetailPayload["preferences"];
    return next;
  }

  if (method === "codex-webui/attachmentsUpdated") {
    next.attachments = Array.isArray(params.attachments) ? (params.attachments as SessionDetailPayload["attachments"]) : [];
    return next;
  }

  if (method === "codex-webui/queueUpdated") {
    next.queue = (params.queue as SessionDetailPayload["queue"]) ?? next.queue;
    return next;
  }

  if (method === "thread/tokenUsage/updated") {
    next.tokenUsage = (params.tokenUsage as SessionDetailPayload["tokenUsage"]) ?? null;
    return next;
  }

  if (method === "codex-webui/sessionHydrationStarted") {
    next.hydration = {
      state: "loading",
      loadedTurns: Number(params.loadedTurns ?? 0),
      totalTurns: typeof params.totalTurns === "number" ? Number(params.totalTurns) : null,
      remainingTurns: typeof params.remainingTurns === "number" ? Number(params.remainingTurns) : next.hydration.remainingTurns,
      message: null
    };
    return next;
  }

  if (method === "codex-webui/sessionHydrationChunk") {
    const existingTurnIds = new Set(next.thread.turns.map((turn) => turn.id));
    const incomingTurns = Array.isArray(params.turns) ? (params.turns as CodexTurn[]) : [];
    next.thread.turns = [
      ...next.thread.turns,
      ...incomingTurns.filter((turn) => turn?.id && !existingTurnIds.has(turn.id))
    ];
    next.hydration = {
      state: "loading",
      loadedTurns: Number(params.loadedTurns ?? next.thread.turns.length),
      totalTurns: typeof params.totalTurns === "number" ? Number(params.totalTurns) : next.hydration.totalTurns,
      remainingTurns: typeof params.remainingTurns === "number" ? Number(params.remainingTurns) : next.hydration.remainingTurns,
      message: null
    };
    return next;
  }

  if (method === "codex-webui/sessionHydrationCompleted") {
    next.hydration = {
      state: "complete",
      loadedTurns: Number(params.loadedTurns ?? next.thread.turns.length),
      totalTurns: typeof params.totalTurns === "number" ? Number(params.totalTurns) : next.thread.turns.length,
      remainingTurns: typeof params.remainingTurns === "number" ? Number(params.remainingTurns) : 0,
      message: null
    };
    if (typeof params.activeTurnId === "string" || params.activeTurnId === null) {
      next.activeTurnId = (params.activeTurnId as string | null) ?? null;
    }
    if (next.thread.status !== "running" && next.thread.status !== "active") {
      next.activeTurnId = resolveConversationRunningTurn(next);
    }
    return next;
  }

  if (method === "codex-webui/sessionHydrationFailed") {
    next.hydration = {
      state: "error",
      loadedTurns: next.hydration.loadedTurns,
      totalTurns: next.hydration.totalTurns,
      remainingTurns: next.hydration.remainingTurns,
      message: typeof params.message === "string" ? params.message : "Failed to load session history."
    };
    return next;
  }

  if (method === "turn/plan/updated") {
    const turnId = String(params.turnId ?? "");
    next.livePlans[turnId] = {
      explanation: (params.explanation as string | null | undefined) ?? null,
      plan: Array.isArray(params.plan) ? (params.plan as Array<{ step: string; status: string }>) : []
    };
    return next;
  }

  if (method === "turn/diff/updated") {
    const turnId = String(params.turnId ?? "");
    next.liveDiffs[turnId] = String(params.diff ?? "");
    return next;
  }

  if (method === "turn/started" || method === "turn/completed") {
    const turn = params.turn as CodexTurn;
    const seeded = ensureTurn(next, turn.id, turn);
    seeded.turn.status = turn.status;
    seeded.turn.error = turn.error;
    seeded.turn.startedAt = turn.startedAt;
    seeded.turn.completedAt = turn.completedAt;
    seeded.turn.durationMs = turn.durationMs;
    next.thread.turns = seeded.turns;
    next.activeTurnId = method === "turn/started" ? turn.id : next.activeTurnId === turn.id ? null : next.activeTurnId;
    if (method === "turn/started") {
      next.thread.status = "running";
    }
    if (next.thread.status !== "running" && next.thread.status !== "active") {
      next.activeTurnId = resolveConversationRunningTurn(next);
    }
    if (method === "turn/completed" && !next.activeTurnId && (next.thread.status === "running" || next.thread.status === "active")) {
      next.thread.status = "completed";
    }
    return next;
  }

  if (method === "item/started" || method === "item/completed") {
    const turnId = String(params.turnId ?? "");
    const seeded = ensureTurn(next, turnId);
    const streamItem = (params.item as CodexItem) ?? ({ id: String(params.itemId ?? ""), type: "unknown" } satisfies CodexItem);
    upsertItem(
      seeded.turn,
      streamItem.type === "contextCompaction"
        ? {
            ...streamItem,
            lifecycleStatus: method === "item/started" ? "inProgress" : "completed"
          }
        : streamItem
    );
    next.thread.turns = seeded.turns;
    return next;
  }

  if (method === "item/agentMessage/delta" || method === "item/plan/delta") {
    const turnId = String(params.turnId ?? "");
    const itemId = String(params.itemId ?? "");
    const seeded = ensureTurn(next, turnId);
    const type = method === "item/agentMessage/delta" ? "agentMessage" : "plan";
    const existing =
      seeded.turn.items.find((candidate) => candidate.id === itemId) ??
      ({
        id: itemId,
        type,
        text: ""
      } satisfies CodexItem);

    upsertItem(seeded.turn, {
      ...existing,
      text: `${String(existing.text ?? "")}${String(params.delta ?? "")}`
    });
    next.thread.turns = seeded.turns;
    return next;
  }

  if (
    method === "item/reasoning/textDelta" ||
    method === "item/reasoning/summaryTextDelta" ||
    method === "item/reasoning/summaryPartAdded"
  ) {
    const turnId = String(params.turnId ?? "");
    const itemId = String(params.itemId ?? "");
    const seeded = ensureTurn(next, turnId);
    const existing =
      seeded.turn.items.find((candidate) => candidate.id === itemId) ??
      ({
        id: itemId,
        type: "reasoning",
        text: "",
        summary: []
      } satisfies CodexItem);

    const summary = Array.isArray(existing.summary) ? existing.summary.map((entry) => String(entry)) : [];
    const summaryIndex = Number(params.summaryIndex ?? 0);

    if (method === "item/reasoning/summaryPartAdded" && summary[summaryIndex] === undefined) {
      summary[summaryIndex] = "";
    }

    if (method === "item/reasoning/summaryTextDelta") {
      summary[summaryIndex] = `${summary[summaryIndex] ?? ""}${String(params.delta ?? "")}`;
    }

    upsertItem(seeded.turn, {
      ...existing,
      text:
        method === "item/reasoning/textDelta"
          ? `${String(existing.text ?? "")}${String(params.delta ?? "")}`
          : String(existing.text ?? ""),
      summary
    });
    next.thread.turns = seeded.turns;
    return next;
  }

  if (method === "item/commandExecution/outputDelta") {
    const turnId = String(params.turnId ?? "");
    const itemId = String(params.itemId ?? "");
    const seeded = ensureTurn(next, turnId);
    const existing =
      seeded.turn.items.find((candidate) => candidate.id === itemId) ??
      ({
        id: itemId,
        type: "commandExecution",
        command: "",
        cwd: next.preferences.cwd,
        aggregatedOutput: ""
      } satisfies CodexItem);

    upsertItem(seeded.turn, {
      ...existing,
      aggregatedOutput: `${String(existing.aggregatedOutput ?? "")}${String(params.delta ?? "")}`
    });
    next.thread.turns = seeded.turns;
    return next;
  }

  return next;
}
