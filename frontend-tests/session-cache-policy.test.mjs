import assert from "node:assert/strict";
import test from "node:test";

import {
  SESSION_CACHE_INTERACTIVE_DELAY_MS,
  SESSION_CACHE_STREAM_IDLE_DELAY_MS,
  sessionCacheModeForStreamEvent,
  sessionCachePersistDelay
} from "../src/lib/session-cache-policy.ts";

test("defers browser snapshots until streamed deltas become idle", () => {
  assert.equal(sessionCacheModeForStreamEvent({ kind: "notification", method: "item/agentMessage/delta" }), "stream");
  assert.equal(sessionCachePersistDelay("stream"), SESSION_CACHE_STREAM_IDLE_DELAY_MS);
  assert.ok(SESSION_CACHE_STREAM_IDLE_DELAY_MS > SESSION_CACHE_INTERACTIVE_DELAY_MS);
});

test("flushes terminal and queue state without the stream idle delay", () => {
  for (const method of ["turn/completed", "thread/status/changed", "codex-webui/queueUpdated", "error"]) {
    assert.equal(sessionCacheModeForStreamEvent({ kind: "notification", method }), "terminal");
  }
  assert.equal(sessionCachePersistDelay("terminal"), SESSION_CACHE_INTERACTIVE_DELAY_MS);
});

test("keeps interactive local mutations responsive", () => {
  assert.equal(sessionCachePersistDelay("interactive"), SESSION_CACHE_INTERACTIVE_DELAY_MS);
});
