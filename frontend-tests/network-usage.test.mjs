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
