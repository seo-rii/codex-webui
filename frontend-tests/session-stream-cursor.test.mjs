import assert from "node:assert/strict";
import test from "node:test";

import {
  observeSessionStreamEvent,
  reconcileSessionStreamBoundary
} from "../src/lib/session-stream-cursor.ts";

test("coalesced session event ranges remain contiguous", () => {
  const initial = reconcileSessionStreamBoundary(null, null, {
    streamEpoch: "epoch-a",
    streamSequence: 4
  });
  const observed = observeSessionStreamEvent(initial.cursor, {
    streamEpoch: "epoch-a",
    streamSequenceStart: 5,
    streamSequence: 8
  });

  assert.equal(observed.gap, false);
  assert.equal(observed.cursor?.sequence, 8);
});

test("missing session event sequences request a resync", () => {
  const observed = observeSessionStreamEvent(
    { epoch: "epoch-a", firstSequence: 5, sequence: 8 },
    {
      streamEpoch: "epoch-a",
      streamSequenceStart: 10,
      streamSequence: 10
    }
  );

  assert.equal(observed.gap, true);
  assert.equal(observed.epochChanged, false);
  assert.equal(observed.cursor?.sequence, 10);
});

test("a snapshot boundary detects events missed before the first observed event", () => {
  const firstEvent = observeSessionStreamEvent(null, {
    streamEpoch: "epoch-a",
    streamSequence: 12
  });
  const reconciled = reconcileSessionStreamBoundary(firstEvent.cursor, null, {
    streamEpoch: "epoch-a",
    streamSequence: 9
  });

  assert.equal(reconciled.gap, true);
});

test("a snapshot boundary accepts a contiguous first event", () => {
  const firstEvent = observeSessionStreamEvent(null, {
    streamEpoch: "epoch-a",
    streamSequence: 10
  });
  const reconciled = reconcileSessionStreamBoundary(firstEvent.cursor, null, {
    streamEpoch: "epoch-a",
    streamSequence: 9
  });

  assert.equal(reconciled.gap, false);
});

test("a gateway epoch change requests a resync", () => {
  const observed = observeSessionStreamEvent(
    { epoch: "epoch-a", firstSequence: 1, sequence: 7 },
    {
      streamEpoch: "epoch-b",
      streamSequence: 1
    }
  );

  assert.equal(observed.gap, true);
  assert.equal(observed.epochChanged, true);
  assert.equal(observed.cursor?.epoch, "epoch-b");
});
