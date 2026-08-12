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
