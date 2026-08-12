import assert from "node:assert/strict";
import test from "node:test";

import { computeTranscriptWindow } from "../src/lib/transcript-window.ts";

function compute(overrides = {}) {
  return computeTranscriptWindow({
    turnIds: Array.from({ length: 100 }, (_, index) => `turn-${index}`),
    measuredHeights: new Map(),
    scrollOffset: 0,
    viewportHeight: 300,
    estimatedHeight: 100,
    gap: 10,
    overscan: 100,
    maxItems: 12,
    ...overrides
  });
}

test("mounts only the viewport and overscan range", () => {
  const window = compute();

  assert.deepEqual({ start: window.start, end: window.end }, { start: 0, end: 4 });
  assert.equal(window.topSpacer, 0);
  assert.equal(window.totalHeight, 10_990);
  assert.equal(window.bottomSpacer, 10_550);
});

test("keeps mounted turns bounded in a long transcript", () => {
  const window = compute({ scrollOffset: 5_000, overscan: 1_000, maxItems: 8 });

  assert.equal(window.end - window.start, 8);
  assert.ok(window.start > 0);
  assert.ok(window.bottomSpacer > 0);
});

test("uses measured heights when preserving virtual spacers", () => {
  const window = compute({
    measuredHeights: new Map([
      ["turn-0", 250],
      ["turn-1", 50]
    ]),
    scrollOffset: 500,
    overscan: 0
  });

  assert.equal(window.totalHeight, 11_090);
  assert.equal(window.start, 3);
  assert.equal(window.topSpacer, 430);
});

test("anchors an offscreen search result inside the mounted range", () => {
  const window = compute({ anchorIndex: 72, anchorAlignment: "center", maxItems: 10 });

  assert.ok(window.start <= 72);
  assert.ok(window.end > 72);
  assert.ok(window.end - window.start <= 10);
});

test("anchors the newest turn at the end for initial bottom scrolling", () => {
  const window = compute({ anchorIndex: 99, anchorAlignment: "end", maxItems: 10 });

  assert.equal(window.end, 100);
  assert.ok(window.start >= 90);
  assert.equal(window.bottomSpacer, 0);
});
