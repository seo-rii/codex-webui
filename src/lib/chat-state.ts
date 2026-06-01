import type { CodexItem, CodexTurn, SessionDetailPayload, StreamEvent } from "$lib/types";

export type ConversationState = SessionDetailPayload & {
  livePlans: Record<string, { explanation: string | null; plan: Array<{ step: string; status: string }> }>;
  liveDiffs: Record<string, string>;
};

function cloneTurn(turn: CodexTurn): CodexTurn {
  return {
    ...turn,
    items: turn.items.map((item) => ({ ...item, type: normalizeItemTypeName(item.type) }))
  };
}

function normalizeItemTypeName(itemType: string) {
  if (itemType === "agent_message" || itemType === "assistant_message" || itemType === "assistantMessage") {
    return "agentMessage";
  }
  if (itemType === "user_message") {
    return "userMessage";
  }
  if (itemType === "command_execution") {
    return "commandExecution";
  }
  if (itemType === "file_change") {
    return "fileChange";
  }
  if (itemType === "mcp_tool_call") {
    return "mcpToolCall";
  }
  if (itemType === "dynamic_tool_call") {
    return "dynamicToolCall";
  }
  if (itemType === "web_search") {
    return "webSearch";
  }
  if (itemType === "context_compaction") {
    return "contextCompaction";
  }
  if (itemType === "image_generation") {
    return "imageGeneration";
  }
  if (itemType === "collab_agent_tool_call") {
    return "collabAgentToolCall";
  }
  return itemType;
}

function isContextCompactionItem(item: CodexItem) {
  return normalizeItemTypeName(item.type) === "contextCompaction";
}

function findContextCompactionItemIndex(items: CodexItem[]) {
  return items.findIndex((item) => isContextCompactionItem(item));
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
  const normalizedItem: CodexItem = {
    ...item,
    type: normalizeItemTypeName(item.type)
  };
  const index = turn.items.findIndex((candidate) => candidate.id === normalizedItem.id);
  if (index === -1) {
    const contextCompactionIndex = isContextCompactionItem(normalizedItem) ? findContextCompactionItemIndex(turn.items) : -1;
    if (contextCompactionIndex !== -1) {
      turn.items = turn.items.map((candidate, candidateIndex) =>
        candidateIndex === contextCompactionIndex ? mergeItem(candidate, normalizedItem) : candidate
      );
      return;
    }
    turn.items = [...turn.items, normalizedItem];
    return;
  }
  turn.items = turn.items.map((candidate, candidateIndex) => (candidateIndex === index ? mergeItem(candidate, normalizedItem) : candidate));
}

function resolveConversationRunningTurn(state: ConversationState) {
  if (state.activeTurnId && state.thread.turns.some((turn) => turn.id === state.activeTurnId && String(turn.status ?? "") === "inProgress")) {
    return state.activeTurnId;
  }

  const activeTurn = [...state.thread.turns].reverse().find((turn) => String(turn.status ?? "") === "inProgress");
  return activeTurn ? activeTurn.id : null;
}

function pruneLivePlansForRunningTurns(livePlans: ConversationState["livePlans"], turns: CodexTurn[]) {
  const runningTurnIds = new Set(turns.filter((turn) => String(turn.status ?? "") === "inProgress").map((turn) => turn.id));
  const pruned = Object.fromEntries(Object.entries(livePlans).filter(([turnId]) => runningTurnIds.has(turnId)));
  return Object.keys(pruned).length === Object.keys(livePlans).length ? livePlans : pruned;
}

function isLiveThreadStatus(status: string | null | undefined) {
  return status === "running" || status === "active";
}

function realtimeTurnIdForThread(threadId: string) {
  return `realtime:${threadId}`;
}

function realtimeItemTypeForRole(role: unknown) {
  const normalized = String(role ?? "").toLowerCase();
  return normalized === "user" || normalized === "human" ? "userMessage" : "agentMessage";
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
  const normalizedIncomingItem: CodexItem = {
    ...incomingItem,
    type: normalizeItemTypeName(incomingItem.type)
  };
  if (!existingItem) {
    return normalizedIncomingItem;
  }
  const normalizedExistingItem: CodexItem = {
    ...existingItem,
    type: normalizeItemTypeName(existingItem.type)
  };

  const merged: CodexItem = {
    ...normalizedExistingItem,
    ...normalizedIncomingItem
  };
  if (normalizedExistingItem.type === "contextCompaction" && normalizedIncomingItem.type === "contextCompaction") {
    merged.id = normalizedExistingItem.id;
  }

  if (normalizedExistingItem.detailState === "loaded" && normalizedIncomingItem.detailState !== "loaded") {
    merged.detailState = "loaded";
  } else if (normalizedExistingItem.detailState === "inline" && normalizedIncomingItem.detailState === "deferred") {
    merged.detailState = "inline";
  }

  merged.detailPreview =
    typeof normalizedIncomingItem.detailPreview === "string" && normalizedIncomingItem.detailPreview.trim()
      ? normalizedIncomingItem.detailPreview
      : (normalizedExistingItem.detailPreview ?? null);
  merged.title =
    typeof normalizedIncomingItem.title === "string" && normalizedIncomingItem.title.trim()
      ? normalizedIncomingItem.title
      : (normalizedExistingItem.title ?? null);

  if ("text" in normalizedExistingItem || "text" in normalizedIncomingItem) {
    merged.text = preferProgressiveString(normalizedExistingItem.text, normalizedIncomingItem.text);
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

  const userMessageSignatures = new Set<string>();
  const deduped: CodexItem[] = [];
  for (const item of merged) {
    const itemType = normalizeItemTypeName(item.type);
    if (itemType === "contextCompaction") {
      const contextCompactionIndex = findContextCompactionItemIndex(deduped);
      if (contextCompactionIndex === -1) {
        deduped.push({ ...item, type: itemType });
      } else {
        deduped[contextCompactionIndex] = mergeItem(deduped[contextCompactionIndex], { ...item, type: itemType });
      }
      continue;
    }
    if (itemType !== "userMessage") {
      deduped.push(item);
      continue;
    }

    const fragments: string[] = [];
    for (const key of ["text", "message", "value"]) {
      const value = item[key];
      if (typeof value === "string" && value.trim()) {
        fragments.push(value);
      }
    }
    const content = Array.isArray(item.content) ? (item.content as Array<Record<string, unknown>>) : [];
    for (const entry of content) {
      for (const key of ["text", "content", "value"]) {
        const value = entry[key];
        if (typeof value === "string" && value.trim()) {
          fragments.push(value);
        }
      }
    }
    const signature = fragments.join("\n\n").replace(/\s+/gu, " ").trim();
    if (signature && userMessageSignatures.has(signature)) {
      continue;
    }
    if (signature) {
      userMessageSignatures.add(signature);
    }
    deduped.push(item);
  }

  return deduped;
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

function normalizeTurnTimestamp(value: number | null | undefined) {
  if (typeof value !== "number" || !Number.isFinite(value) || value <= 0) {
    return null;
  }
  return value < 10_000_000_000 ? value * 1000 : value;
}

function uuidV7Timestamp(id: string | null | undefined) {
  const compact = String(id ?? "").replace(/-/g, "");
  if (!/^[0-9a-f]{12}7/iu.test(compact)) {
    return null;
  }

  const value = Number.parseInt(compact.slice(0, 12), 16);
  return Number.isSafeInteger(value) && value > 0 ? value : null;
}

function turnChronologyValue(turn: CodexTurn) {
  return (
    normalizeTurnTimestamp(turn.startedAt) ??
    uuidV7Timestamp(turn.id) ??
    normalizeTurnTimestamp(turn.completedAt)
  );
}

function normalizeTurnOrder(turns: CodexTurn[]) {
  return turns
    .map((turn, index) => ({
      turn,
      index,
      order: turnChronologyValue(turn)
    }))
    .sort((left, right) => {
      if (left.order !== null && right.order !== null && left.order !== right.order) {
        return left.order - right.order;
      }
      if (left.order !== null && right.order === null) {
        return -1;
      }
      if (left.order === null && right.order !== null) {
        return 1;
      }
      return left.index - right.index;
    })
    .map((entry) => entry.turn);
}

function mergeTurns(existingTurns: CodexTurn[], incomingTurns: CodexTurn[]) {
  const existingById = new Map(existingTurns.map((turn) => [turn.id, turn] as const));
  const incomingById = new Map(incomingTurns.map((turn) => [turn.id, turn] as const));
  const merged = existingTurns.map((turn) => {
    const incomingTurn = incomingById.get(turn.id);
    return incomingTurn ? mergeTurn(turn, incomingTurn) : cloneTurn(turn);
  });
  const seenIds = new Set(merged.map((turn) => turn.id));

  for (const turn of incomingTurns) {
    if (seenIds.has(turn.id)) {
      continue;
    }
    merged.push(mergeTurn(existingById.get(turn.id), turn));
    seenIds.add(turn.id);
  }

  return normalizeTurnOrder(merged);
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
  const existingItemCount = Array.isArray(existingQueue.items) ? existingQueue.items.length : 0;
  const incomingItemCount = Array.isArray(incomingQueue.items) ? incomingQueue.items.length : 0;
  if (incomingItemCount === 0 && existingItemCount > 0 && !incomingQueue.resumeRequired) {
    return incomingQueue;
  }

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
    message: incomingHydration.message,
    recovery: incomingHydration.recovery
  } satisfies SessionDetailPayload["hydration"];
}

export function createConversationState(detail: SessionDetailPayload): ConversationState {
  return {
    ...detail,
    thread: {
      ...detail.thread,
      turns: normalizeTurnOrder(detail.thread.turns.map(cloneTurn))
    },
    livePlans: {},
    liveDiffs: {}
  };
}

export function mergeConversationState(current: ConversationState, detail: SessionDetailPayload): ConversationState {
  const incoming = createConversationState(detail);
  const incomingSettled = !isLiveThreadStatus(incoming.thread.status) && !incoming.activeTurnId;
  const mergedTurns = mergeTurns(current.thread.turns, incoming.thread.turns).map((turn) =>
    incomingSettled && String(turn.status ?? "") === "inProgress"
      ? {
          ...turn,
          status: incoming.thread.status === "failed" || incoming.thread.status === "error" ? "failed" : "completed",
          completedAt: turn.completedAt ?? Date.now()
        }
      : turn
  );
  const hasLiveTurn = mergedTurns.some((turn) => String(turn.status ?? "") === "inProgress");
  const threadStatus = hasLiveTurn
    ? (isLiveThreadStatus(incoming.thread.status) ? incoming.thread.status : current.thread.status || "running")
    : incoming.thread.status;
  const livePlans = pruneLivePlansForRunningTurns(current.livePlans, mergedTurns);

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
    goal: incoming.goal ?? current.goal ?? null,
    tokenUsage: incoming.tokenUsage ?? current.tokenUsage ?? null,
    hydration: mergeHydration(current.hydration, incoming.hydration, mergedTurns.length),
    livePlans,
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
      next.livePlans = pruneLivePlansForRunningTurns(next.livePlans, next.thread.turns);
    }
    return next;
  }

  if (method === "codex-webui/preferencesUpdated") {
    next.preferences = params.preferences as SessionDetailPayload["preferences"];
    return next;
  }

  if (method === "codex-webui/skillsUpdated") {
    next.selectedSkills = Array.isArray(params.selectedSkills) ? (params.selectedSkills as SessionDetailPayload["selectedSkills"]) : [];
    return next;
  }

  if (method === "codex-webui/attachmentsUpdated") {
    next.attachments = Array.isArray(params.attachments) ? (params.attachments as SessionDetailPayload["attachments"]) : [];
    return next;
  }

  if (method === "codex-webui/queueUpdated") {
    next.queue = mergeQueue(next.queue, (params.queue as SessionDetailPayload["queue"]) ?? next.queue);
    return next;
  }

  if (method === "codex-webui/languageBridgeResponseTranslated") {
    const turnId = String(params.turnId ?? "");
    const itemId = String(params.itemId ?? "");
    const translatedText = String(params.text ?? "");
    if (!turnId || !itemId || !translatedText) {
      return next;
    }
    const seeded = ensureTurn(next, turnId);
    const existing =
      seeded.turn.items.find((candidate) => candidate.id === itemId) ??
      ({
        id: itemId,
        type: "agentMessage",
        text: ""
      } satisfies CodexItem);
    upsertItem(seeded.turn, {
      ...existing,
      type: existing.type || "agentMessage",
      originalText: existing.originalText ?? params.originalText ?? existing.text ?? "",
      text: translatedText,
      languageBridgeTranslated: true,
      languageBridgeOutputLanguage: params.language ?? null
    });
    next.thread.turns = seeded.turns;
    return next;
  }

  if (method === "thread/tokenUsage/updated") {
    next.tokenUsage = (params.tokenUsage as SessionDetailPayload["tokenUsage"]) ?? null;
    return next;
  }

  if (method === "thread/goal/updated") {
    next.goal = (params.goal as SessionDetailPayload["goal"]) ?? null;
    return next;
  }

  if (method === "thread/goal/cleared") {
    next.goal = null;
    return next;
  }

  if (method === "codex-webui/sessionHydrationStarted") {
    next.hydration = {
      state: "loading",
      loadedTurns: Number(params.loadedTurns ?? 0),
      totalTurns: typeof params.totalTurns === "number" ? Number(params.totalTurns) : null,
      remainingTurns: typeof params.remainingTurns === "number" ? Number(params.remainingTurns) : next.hydration.remainingTurns,
      message: null,
      recovery: {
        available: false,
        issue: null,
        totalLines: null,
        recoverableLines: null,
        skippedLines: null
      }
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
      message: null,
      recovery: {
        available: false,
        issue: null,
        totalLines: null,
        recoverableLines: null,
        skippedLines: null
      }
    };
    return next;
  }

  if (method === "codex-webui/sessionHydrationCompleted") {
    next.hydration = {
      state: "complete",
      loadedTurns: Number(params.loadedTurns ?? next.thread.turns.length),
      totalTurns: typeof params.totalTurns === "number" ? Number(params.totalTurns) : next.thread.turns.length,
      remainingTurns: typeof params.remainingTurns === "number" ? Number(params.remainingTurns) : 0,
      message: null,
      recovery: {
        available: false,
        issue: null,
        totalLines: null,
        recoverableLines: null,
        skippedLines: null
      }
    };
    if (typeof params.activeTurnId === "string" || params.activeTurnId === null) {
      next.activeTurnId = (params.activeTurnId as string | null) ?? null;
    }
    if (next.thread.status !== "running" && next.thread.status !== "active") {
      next.activeTurnId = resolveConversationRunningTurn(next);
    }
    next.livePlans = pruneLivePlansForRunningTurns(next.livePlans, next.thread.turns);
    return next;
  }

  if (method === "codex-webui/sessionHydrationFailed") {
    next.hydration = {
      state: "error",
      loadedTurns: next.hydration.loadedTurns,
      totalTurns: next.hydration.totalTurns,
      remainingTurns: next.hydration.remainingTurns,
      message: typeof params.message === "string" ? params.message : "Failed to load session history.",
      recovery:
        params.recovery && typeof params.recovery === "object"
          ? {
              available: Boolean((params.recovery as Record<string, unknown>).available),
              issue:
                typeof (params.recovery as Record<string, unknown>).issue === "string"
                  ? String((params.recovery as Record<string, unknown>).issue)
                  : null,
              totalLines:
                typeof (params.recovery as Record<string, unknown>).totalLines === "number"
                  ? Number((params.recovery as Record<string, unknown>).totalLines)
                  : null,
              recoverableLines:
                typeof (params.recovery as Record<string, unknown>).recoverableLines === "number"
                  ? Number((params.recovery as Record<string, unknown>).recoverableLines)
                  : null,
              skippedLines:
                typeof (params.recovery as Record<string, unknown>).skippedLines === "number"
                  ? Number((params.recovery as Record<string, unknown>).skippedLines)
                  : null
            }
          : next.hydration.recovery
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

  if (method === "thread/realtime/started") {
    const turnId = realtimeTurnIdForThread(next.thread.id);
    const seeded = ensureTurn(next, turnId, {
      status: "inProgress",
      startedAt: Date.now(),
      completedAt: null,
      durationMs: null
    });
    next.thread.turns = seeded.turns;
    next.activeTurnId = turnId;
    next.thread.status = "running";
    return next;
  }

  if (method === "thread/realtime/transcript/delta" || method === "thread/realtime/transcript/done") {
    const turnId = realtimeTurnIdForThread(next.thread.id);
    const role = String(params.role ?? "assistant");
    const itemId = `${turnId}:${role}`;
    const seeded = ensureTurn(next, turnId, {
      status: "inProgress",
      startedAt: Date.now(),
      completedAt: null,
      durationMs: null
    });
    const existing =
      seeded.turn.items.find((candidate) => candidate.id === itemId) ??
      ({
        id: itemId,
        type: realtimeItemTypeForRole(role),
        text: ""
      } satisfies CodexItem);
    const nextText =
      method === "thread/realtime/transcript/done"
        ? String(params.text ?? existing.text ?? "")
        : `${String(existing.text ?? "")}${String(params.delta ?? "")}`;

    upsertItem(seeded.turn, {
      ...existing,
      type: realtimeItemTypeForRole(role),
      text: nextText
    });
    next.thread.turns = seeded.turns;
    next.activeTurnId = turnId;
    next.thread.status = "running";
    return next;
  }

  if (method === "thread/realtime/error") {
    const turnId = realtimeTurnIdForThread(next.thread.id);
    const seeded = ensureTurn(next, turnId, {
      status: "inProgress",
      startedAt: Date.now(),
      completedAt: null,
      durationMs: null
    });
    upsertItem(seeded.turn, {
      id: `${turnId}:error`,
      type: "agentMessage",
      text: `Realtime error: ${String(params.message ?? params.error ?? "Unknown error")}`
    });
    next.thread.turns = seeded.turns;
    next.activeTurnId = turnId;
    next.thread.status = "running";
    return next;
  }

  if (method === "thread/realtime/closed") {
    const turnId = realtimeTurnIdForThread(next.thread.id);
    const seeded = ensureTurn(next, turnId, {
      status: "completed"
    });
    seeded.turn.status = "completed";
    seeded.turn.completedAt = seeded.turn.completedAt ?? Date.now();
    next.thread.turns = seeded.turns;
    if (next.activeTurnId === turnId) {
      next.activeTurnId = null;
    }
    if (!resolveConversationRunningTurn(next)) {
      next.thread.status = "completed";
    }
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
    if (method === "turn/completed") {
      delete next.livePlans[turn.id];
    }
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
    const itemType = normalizeItemTypeName(streamItem.type);
    const lifecycleStatus =
      itemType === "contextCompaction" ? (method === "item/started" ? "inProgress" : "completed") : streamItem.lifecycleStatus;
    upsertItem(seeded.turn, {
      ...streamItem,
      type: itemType,
      lifecycleStatus
    });
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
