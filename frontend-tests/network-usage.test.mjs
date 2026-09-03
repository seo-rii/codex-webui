import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

test("active status fallback polling does not reload the selected transcript", async () => {
  const source = await readFile("src/routes/+page.svelte", "utf8");
  const effectStart = source.indexOf("!sessionListNeedsActiveStatusPolling");
  assert.notEqual(effectStart, -1, "active-session polling effect should exist");
  const effectEnd = source.indexOf("\n  $effect(() => {", effectStart);
  const effect = source.slice(effectStart, effectEnd === -1 ? undefined : effectEnd);

  assert.match(effect, /scheduleSessionRefresh\(0\)/u);
  assert.doesNotMatch(effect, /scheduleSelectedSessionStateRefresh/u);
  assert.doesNotMatch(effect, /refreshSelectedSessionState/u);
});

test("completion-tail refresh keeps the detail cache boundary separate", async () => {
  const source = await readFile("src/routes/+page.svelte", "utf8");
  const applyStart = source.indexOf("function applyLatestCompletedTurnPayload(");
  const applyEnd = source.indexOf("\n  async function refreshSelectedSessionCompletionTail(", applyStart);
  assert.notEqual(applyStart, -1, "completion-tail apply function should exist");
  assert.notEqual(applyEnd, -1, "completion-tail apply function should have a stable boundary");
  const applyCompletion = source.slice(applyStart, applyEnd);

  assert.match(applyCompletion, /sessionCompletionVersionsByTurnId/u);
  assert.doesNotMatch(applyCompletion, /sessionTurnVersionsById\s*=/u);
  assert.doesNotMatch(applyCompletion, /sessionDetail(?:CacheVersion|StateHash|MetadataVersion)\s*=\s*null/u);

  const selectionStart = source.indexOf("async function selectSession(");
  const selectionEnd = source.indexOf("\n  function rebindSelectedSessionProfile(", selectionStart);
  const selection = source.slice(selectionStart, selectionEnd === -1 ? undefined : selectionEnd);
  assert.doesNotMatch(
    selection,
    /if \(terminalSelection\) \{\s*knownVersion = null;/u,
    "terminal sessions should validate the cached detail instead of forcing a full reload"
  );
});

test("completion-tail refresh remains armed after the fast retry window", async () => {
  const source = await readFile("src/routes/+page.svelte", "utf8");
  const scheduleStart = source.indexOf("function scheduleSelectedSessionCompletionRefresh(");
  const scheduleEnd = source.indexOf("\n  function getRequestedSessionIdFromUrl(", scheduleStart);
  assert.notEqual(scheduleStart, -1, "completion-tail scheduler should exist");
  assert.notEqual(scheduleEnd, -1, "completion-tail scheduler should have a stable boundary");
  const scheduler = source.slice(scheduleStart, scheduleEnd);

  assert.match(scheduler, /const longRetryDelay = 30_000;/u);
  assert.match(scheduler, /retryDelays\[nextAttempt\] \?\? longRetryDelay/u);
  assert.doesNotMatch(
    scheduler,
    /if \(nextAttempt < retryDelays\.length\)[\s\S]*?delete\(jobKey\)/u,
    "exhausting fast retries must not permanently abandon final-message reconciliation"
  );
});

test("completion-tail refresh cannot settle on the final turn already loaded from cache", async () => {
  const pageSource = await readFile("src/routes/+page.svelte", "utf8");
  const apiSource = await readFile("src/lib/api.ts", "utf8");
  const anchorStart = pageSource.indexOf("function completionRefreshTurnAnchors(");
  const anchorEnd = pageSource.indexOf("\n  function getConversationDisplayTitle(", anchorStart);
  const selectionStart = pageSource.indexOf("async function selectSession(");
  const selectionEnd = pageSource.indexOf("\n  function rebindSelectedSessionProfile(", selectionStart);

  assert.notEqual(anchorStart, -1, "completion baseline helper should exist");
  assert.notEqual(anchorEnd, -1, "completion baseline helper should have a stable boundary");
  assert.notEqual(selectionStart, -1, "session selection should exist");
  assert.notEqual(selectionEnd, -1, "session selection should have a stable boundary");

  const anchors = pageSource.slice(anchorStart, anchorEnd);
  const selection = pageSource.slice(selectionStart, selectionEnd);
  assert.match(anchors, /\{ expectedTurnId: null, afterTurnId: turn\.id \}/u);
  assert.match(anchors, /\{ expectedTurnId: turn\.id, afterTurnId: null \}/u);
  assert.match(selection, /sessionDetailSourceUpdatedAt/u);
  assert.match(selection, /completionAnchors\.afterTurnId/u);
  assert.match(apiSource, /afterTurnId,/u);
});

test("stale detail patches retry conditionally instead of forcing a full response", async () => {
  const source = await readFile("src/routes/+page.svelte", "utf8");
  const refreshStart = source.indexOf("async function refreshSelectedSessionState(");
  const refreshEnd = source.indexOf("\n  async function recoverFromReconnect(", refreshStart);
  assert.notEqual(refreshStart, -1, "selected-session refresh should exist");
  assert.notEqual(refreshEnd, -1, "selected-session refresh should have a stable boundary");
  const refresh = source.slice(refreshStart, refreshEnd);
  const patchBranchStart = refresh.indexOf("if (isSessionDetailPatchResponse(detail))");
  const fallbackRequest = refresh.indexOf("await api.getSession(sessionId, turnLimit, null, null, null", patchBranchStart);
  const staleRetry = refresh.indexOf("scheduleSelectedSessionStateRefresh(sessionId, 0, replaceWithRecentWindow)", patchBranchStart);

  assert.notEqual(patchBranchStart, -1, "detail patch branch should exist");
  assert.notEqual(staleRetry, -1, "stale patch should schedule a conditional retry");
  assert.notEqual(fallbackRequest, -1, "invalid reconstructed patches should retain a full fallback");
  assert.ok(staleRetry < fallbackRequest, "base mismatch must return before the unconditional fallback request");
});

test("foreground session catch-up only reloads after a confirmed stream gap", async () => {
  const source = await readFile("src/routes/+page.svelte", "utf8");
  const streamStart = source.indexOf("function connectStream(");
  const streamEnd = source.indexOf("\n  function updateSessionDetailSyncState(", streamStart);
  assert.notEqual(streamStart, -1, "session stream handler should exist");
  assert.notEqual(streamEnd, -1, "session stream handler should have a stable boundary");
  const streamHandler = source.slice(streamStart, streamEnd);

  assert.match(streamHandler, /streamCursorResult\.gap/u);
  assert.doesNotMatch(streamHandler, /staleSessionCatchupEventThreshold/u);
  assert.doesNotMatch(
    streamHandler,
    /nextEventCount\s*>=/u,
    "ordinary streamed deltas must not trigger a full transcript refresh by event count"
  );
});
