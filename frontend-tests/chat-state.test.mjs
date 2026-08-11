import assert from "node:assert/strict";
import test from "node:test";

import {
  applyStreamEvent,
  createConversationState,
  mergeConversationState,
  mergeConversationTurnState,
  sessionStateKey
} from "../src/lib/chat-state.ts";

test("session state keys isolate duplicate ids across profiles", () => {
  assert.notEqual(sessionStateKey("duplicate", "profile-a"), sessionStateKey("duplicate", "profile-b"));
  assert.equal(sessionStateKey("duplicate", null), sessionStateKey("duplicate", "default"));
});

test("generated thread titles accept Codex snake-case notifications", () => {
  const current = createConversationState(detailWithItems([], "completed"));
  const updated = applyStreamEvent(current, {
    kind: "notification",
    method: "thread/name/updated",
    params: {
      thread_id: "session-1",
      thread_name: "Generated conversation title"
    }
  });

  assert.equal(updated.thread.name, "Generated conversation title");
});

function detailWithItems(items, status = "running") {
  return {
    thread: {
      id: "session-1",
      preview: "",
      name: null,
      cwd: "/tmp",
      status,
      createdAt: 1,
      updatedAt: 1,
      isSubagent: false,
      agentNickname: null,
      agentRole: null,
      turns: [
        {
          id: "turn-1",
          items,
          status: status === "running" ? "inProgress" : "completed",
          error: null,
          startedAt: 1,
          completedAt: status === "running" ? null : 2,
          durationMs: status === "running" ? null : 1
        }
      ]
    },
    preferences: {},
    selectedSkills: [],
    goal: null,
    attachments: [],
    queue: {
      sessionId: "session-1",
      items: [],
      resumeRequired: false,
      updatedAt: null
    },
    pendingRequests: [],
    activeTurnId: status === "running" ? "turn-1" : null,
    tokenUsage: null,
    hydration: {
      state: "complete",
      loadedTurns: 1,
      totalTurns: 1,
      remainingTurns: 0,
      message: null,
      recovery: {
        available: false,
        issue: null,
        totalLines: null,
        recoverableLines: null,
        skippedLines: null
      }
    },
    cacheVersion: "v1",
    notModified: false
  };
}

test("a stale snapshot cannot overwrite a newer streamed delta", () => {
  const snapshot = detailWithItems([{ id: "message-1", type: "agentMessage", text: "hel" }]);
  const streamed = applyStreamEvent(createConversationState(snapshot), {
    kind: "notification",
    method: "item/agentMessage/delta",
    params: { turnId: "turn-1", itemId: "message-1", delta: "lo" }
  });

  const merged = mergeConversationState(streamed, snapshot);

  assert.equal(merged.thread.turns.length, 1);
  assert.equal(merged.thread.turns[0].items.length, 1);
  assert.equal(merged.thread.turns[0].items[0].text, "hello");
});

test("a snapshot that already contains the delta remains exactly once", () => {
  const completeSnapshot = detailWithItems([{ id: "message-1", type: "agentMessage", text: "hello" }]);
  const current = createConversationState(completeSnapshot);

  const merged = mergeConversationState(current, completeSnapshot);

  assert.equal(merged.thread.turns[0].items.length, 1);
  assert.equal(merged.thread.turns[0].items[0].text, "hello");
});

test("a stale live snapshot cannot reopen a turn completed while the request was in flight", () => {
  const staleSnapshot = detailWithItems([{ id: "message-1", type: "agentMessage", text: "hel" }]);
  let current = applyStreamEvent(createConversationState(staleSnapshot), {
    kind: "notification",
    method: "item/agentMessage/delta",
    params: { turnId: "turn-1", itemId: "message-1", delta: "lo" }
  });
  current = applyStreamEvent(current, {
    kind: "notification",
    method: "turn/completed",
    params: {
      turnId: "turn-1",
      turn: {
        id: "turn-1",
        items: [{ id: "message-1", type: "agentMessage", text: "hello" }],
        status: "completed",
        error: null,
        startedAt: 1,
        completedAt: 2,
        durationMs: 1
      }
    }
  });

  const merged = mergeConversationState(current, staleSnapshot);

  assert.equal(merged.thread.turns.length, 1);
  assert.equal(merged.thread.turns[0].items.length, 1);
  assert.equal(merged.thread.turns[0].items[0].text, "hello");
  assert.equal(merged.thread.turns[0].status, "completed");
  assert.equal(merged.thread.status, "completed");
  assert.equal(merged.activeTurnId, null);
});

test("snapshot and optimistic user echoes with different ids are deduplicated", () => {
  const current = createConversationState(
    detailWithItems([{ id: "optimistic-user", type: "userMessage", text: "same prompt" }])
  );
  const snapshot = detailWithItems([{ id: "server-user", type: "userMessage", text: "same prompt" }]);

  const merged = mergeConversationState(current, snapshot);

  assert.equal(merged.thread.turns[0].items.length, 1);
  assert.equal(merged.thread.turns[0].items[0].text, "same prompt");
});

test("a sparse item completion preserves loaded tool annotations and result", () => {
  const current = createConversationState(
    detailWithItems([
      {
        id: "tool-1",
        type: "dynamicToolCall",
        tool: "lookup",
        status: "running",
        detailState: "loaded",
        annotations: { audience: ["assistant"] },
        result: { content: [{ type: "text", text: "loaded result" }] }
      }
    ])
  );

  const completed = applyStreamEvent(current, {
    kind: "notification",
    method: "item/completed",
    params: {
      turnId: "turn-1",
      item: {
        id: "tool-1",
        type: "dynamicToolCall",
        status: "completed",
        detailState: "deferred",
        annotations: null
      }
    }
  });

  const item = completed.thread.turns[0].items[0];
  assert.equal(item.status, "completed");
  assert.equal(item.detailState, "loaded");
  assert.deepEqual(item.annotations, { audience: ["assistant"] });
  assert.deepEqual(item.result, { content: [{ type: "text", text: "loaded result" }] });
});

test("hydration chunks merge items into an already streamed turn", () => {
  const current = createConversationState(
    detailWithItems([
      {
        id: "tool-1",
        type: "mcpToolCall",
        tool: "search",
        annotations: { readOnlyHint: true }
      }
    ])
  );

  const hydrated = applyStreamEvent(current, {
    kind: "notification",
    method: "codex-webui/sessionHydrationChunk",
    params: {
      turns: [
        {
          id: "turn-1",
          items: [{ id: "message-1", type: "agentMessage", text: "finished" }],
          status: "completed",
          error: null,
          startedAt: 1,
          completedAt: 2,
          durationMs: 1
        }
      ],
      loadedTurns: 1,
      totalTurns: 1,
      remainingTurns: 0
    }
  });

  assert.deepEqual(
    hydrated.thread.turns[0].items.map((item) => item.id).sort(),
    ["message-1", "tool-1"]
  );
  assert.deepEqual(hydrated.thread.turns[0].items.find((item) => item.id === "tool-1").annotations, {
    readOnlyHint: true
  });
});

test("late turn detail cannot erase a newer streamed tool update", () => {
  const currentTurn = {
    id: "turn-1",
    items: [
      {
        id: "tool-1",
        type: "commandExecution",
        aggregatedOutput: "new output",
        annotations: { source: "stream" },
        status: "completed"
      }
    ],
    status: "completed",
    error: null,
    startedAt: 1,
    completedAt: 3,
    durationMs: 2,
    detailState: "full"
  };
  const staleDetail = {
    ...currentTurn,
    items: [
      {
        id: "tool-1",
        type: "commandExecution",
        aggregatedOutput: "new",
        status: "running"
      }
    ]
  };

  const merged = mergeConversationTurnState(currentTurn, staleDetail);

  assert.equal(merged.items[0].aggregatedOutput, "new output");
  assert.equal(merged.items[0].status, "completed");
  assert.deepEqual(merged.items[0].annotations, { source: "stream" });
});

test("an authoritative snapshot removes absent turns without erasing same-id tool metadata", () => {
  const current = createConversationState(detailWithItems([
    {
      id: "tool-1",
      type: "dynamicToolCall",
      annotations: { source: "stream" },
      result: "loaded"
    }
  ]));
  current.thread.turns.push({
    id: "rolled-back-turn",
    items: [{ id: "old-message", type: "agentMessage", text: "remove me" }],
    status: "completed",
    error: null,
    startedAt: 3,
    completedAt: 4,
    durationMs: 1
  });
  const snapshot = detailWithItems([{ id: "tool-1", type: "dynamicToolCall", detailState: "deferred" }], "completed");

  const merged = mergeConversationState(current, snapshot, false);

  assert.deepEqual(merged.thread.turns.map((turn) => turn.id), ["turn-1"]);
  assert.deepEqual(merged.thread.turns[0].items[0].annotations, { source: "stream" });
  assert.equal(merged.thread.turns[0].items[0].result, "loaded");
});

test("a fully hydrated turn clears its deferred item count", () => {
  const summary = {
    id: "turn-1",
    items: [{ id: "message-1", type: "agentMessage", text: "done" }],
    status: "completed",
    error: null,
    startedAt: 1,
    completedAt: 2,
    durationMs: 1,
    detailState: "summary",
    hiddenItemCount: 2
  };
  const full = {
    ...summary,
    items: [
      { id: "tool-1", type: "dynamicToolCall", tool: "lookup", status: "completed" },
      { id: "message-1", type: "agentMessage", text: "done" }
    ],
    detailState: "full",
    hiddenItemCount: 0
  };

  const merged = mergeConversationTurnState(summary, full);

  assert.equal(merged.detailState, "full");
  assert.equal(merged.hiddenItemCount, 0);
  assert.equal(merged.items.length, 2);
});
