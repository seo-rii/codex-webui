export type SessionStreamMetadata = {
  streamEpoch?: unknown;
  streamSequenceStart?: unknown;
  streamSequence?: unknown;
};

export type SessionStreamCursor = {
  epoch: string;
  firstSequence: number;
  sequence: number;
};

export type SessionStreamCursorResult = {
  cursor: SessionStreamCursor | null;
  gap: boolean;
  epochChanged: boolean;
};

function positiveInteger(value: unknown) {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : null;
}

function streamRange(metadata: SessionStreamMetadata) {
  const epoch = typeof metadata.streamEpoch === "string" ? metadata.streamEpoch.trim() : "";
  const sequence = positiveInteger(metadata.streamSequence);
  if (!epoch || sequence === null) {
    return null;
  }
  const requestedStart = positiveInteger(metadata.streamSequenceStart);
  const start = requestedStart === null ? sequence : Math.min(requestedStart, sequence);
  return { epoch, start, sequence };
}

export function observeSessionStreamEvent(
  current: SessionStreamCursor | null,
  metadata: SessionStreamMetadata
): SessionStreamCursorResult {
  const range = streamRange(metadata);
  if (!range) {
    return { cursor: current, gap: false, epochChanged: false };
  }
  if (!current) {
    return {
      cursor: { epoch: range.epoch, firstSequence: range.start, sequence: range.sequence },
      gap: false,
      epochChanged: false
    };
  }
  if (current.epoch !== range.epoch) {
    return {
      cursor: { epoch: range.epoch, firstSequence: range.start, sequence: range.sequence },
      gap: true,
      epochChanged: true
    };
  }

  return {
    cursor: {
      ...current,
      sequence: Math.max(current.sequence, range.sequence)
    },
    gap: range.start > current.sequence + 1,
    epochChanged: false
  };
}

export function reconcileSessionStreamBoundary(
  current: SessionStreamCursor | null,
  requestCursor: SessionStreamCursor | null,
  metadata: SessionStreamMetadata
): SessionStreamCursorResult {
  const range = streamRange(metadata);
  if (!range) {
    return { cursor: current, gap: false, epochChanged: false };
  }
  if (!current) {
    return {
      cursor: {
        epoch: range.epoch,
        firstSequence: range.sequence + 1,
        sequence: range.sequence
      },
      gap: false,
      epochChanged: false
    };
  }
  if (current.epoch !== range.epoch) {
    return {
      cursor: {
        epoch: range.epoch,
        firstSequence: range.sequence + 1,
        sequence: range.sequence
      },
      gap: true,
      epochChanged: true
    };
  }

  const requestAlreadyTrackedEpoch = requestCursor?.epoch === range.epoch;
  return {
    cursor: {
      ...current,
      firstSequence: Math.min(current.firstSequence, range.sequence + 1),
      sequence: Math.max(current.sequence, range.sequence)
    },
    gap: !requestAlreadyTrackedEpoch && current.firstSequence > range.sequence + 1,
    epochChanged: false
  };
}
