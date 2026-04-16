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

export function createConversationState(detail: SessionDetailPayload): ConversationState {
  return {
    ...detail,
    livePlans: {},
    liveDiffs: {}
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
    upsertItem(seeded.turn, params.item as CodexItem);
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
