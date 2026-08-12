export type TranscriptWindow = {
  start: number;
  end: number;
  topSpacer: number;
  bottomSpacer: number;
  totalHeight: number;
};

export type TranscriptWindowAlignment = "start" | "center" | "end";

export type TranscriptLayout = {
  turnIds: string[];
  offsets: number[];
  extents: number[];
  totalHeight: number;
};

type TranscriptLayoutInput = {
  turnIds: string[];
  measuredHeights: ReadonlyMap<string, number>;
  estimatedHeight: number;
  gap: number;
};

type TranscriptViewportInput = {
  layout: TranscriptLayout;
  scrollOffset: number;
  viewportHeight: number;
  overscan: number;
  maxItems: number;
  anchorIndex?: number;
  anchorAlignment?: TranscriptWindowAlignment;
};

type TranscriptWindowInput = TranscriptLayoutInput & Omit<TranscriptViewportInput, "layout">;

export const EMPTY_TRANSCRIPT_WINDOW: TranscriptWindow = {
  start: 0,
  end: 0,
  topSpacer: 0,
  bottomSpacer: 0,
  totalHeight: 0
};

export const EMPTY_TRANSCRIPT_LAYOUT: TranscriptLayout = {
  turnIds: [],
  offsets: [0],
  extents: [],
  totalHeight: 0
};

function clamp(value: number, minimum: number, maximum: number) {
  return Math.min(maximum, Math.max(minimum, value));
}

export function buildTranscriptLayout(input: TranscriptLayoutInput): TranscriptLayout {
  const count = input.turnIds.length;
  if (count === 0) {
    return EMPTY_TRANSCRIPT_LAYOUT;
  }

  const estimatedHeight = Math.max(1, input.estimatedHeight);
  const gap = Math.max(0, input.gap);
  const extents = input.turnIds.map((turnId, index) => {
    const measured = input.measuredHeights.get(turnId);
    const height = typeof measured === "number" && Number.isFinite(measured) && measured > 0 ? measured : estimatedHeight;
    return height + (index < count - 1 ? gap : 0);
  });
  const offsets = new Array<number>(count + 1);
  offsets[0] = 0;
  for (let index = 0; index < count; index += 1) {
    offsets[index + 1] = offsets[index] + extents[index];
  }

  return {
    turnIds: input.turnIds,
    offsets,
    extents,
    totalHeight: offsets[count]
  };
}

function firstOffsetAtLeast(offsets: number[], target: number) {
  let lower = 0;
  let upper = offsets.length;
  while (lower < upper) {
    const middle = lower + Math.floor((upper - lower) / 2);
    if (offsets[middle] < target) {
      lower = middle + 1;
    } else {
      upper = middle;
    }
  }
  return lower;
}

export function computeTranscriptWindowFromLayout(input: TranscriptViewportInput): TranscriptWindow {
  const { layout } = input;
  const count = layout.turnIds.length;
  if (count === 0) {
    return EMPTY_TRANSCRIPT_WINDOW;
  }

  const viewportHeight = Math.max(1, input.viewportHeight);
  const maxItems = Math.max(1, Math.floor(input.maxItems));
  const totalHeight = layout.totalHeight;
  let scrollOffset = clamp(input.scrollOffset, 0, Math.max(0, totalHeight - viewportHeight));
  if (typeof input.anchorIndex === "number" && Number.isFinite(input.anchorIndex)) {
    const anchorIndex = clamp(Math.floor(input.anchorIndex), 0, count - 1);
    const anchorStart = layout.offsets[anchorIndex];
    const anchorHeight = layout.extents[anchorIndex];
    const alignment = input.anchorAlignment ?? "center";
    if (alignment === "start") {
      scrollOffset = anchorStart;
    } else if (alignment === "end") {
      scrollOffset = anchorStart + anchorHeight - viewportHeight;
    } else {
      scrollOffset = anchorStart + anchorHeight / 2 - viewportHeight / 2;
    }
    scrollOffset = clamp(scrollOffset, 0, Math.max(0, totalHeight - viewportHeight));
  }

  const rangeStart = Math.max(0, scrollOffset - Math.max(0, input.overscan));
  const rangeEnd = Math.min(totalHeight, scrollOffset + viewportHeight + Math.max(0, input.overscan));
  let start = clamp(firstOffsetAtLeast(layout.offsets, rangeStart) - 1, 0, count - 1);
  let end = Math.min(count, Math.max(start + 1, firstOffsetAtLeast(layout.offsets, rangeEnd)));
  end = Math.min(end, start + maxItems);
  if (end === start) {
    end = Math.min(count, start + 1);
  }

  if (typeof input.anchorIndex === "number" && Number.isFinite(input.anchorIndex)) {
    const anchorIndex = clamp(Math.floor(input.anchorIndex), 0, count - 1);
    if (anchorIndex < start || anchorIndex >= end) {
      start = clamp(anchorIndex - Math.floor(maxItems / 2), 0, Math.max(0, count - maxItems));
      end = Math.min(count, start + maxItems);
    }
  }

  return {
    start,
    end,
    topSpacer: layout.offsets[start],
    bottomSpacer: totalHeight - layout.offsets[end],
    totalHeight
  };
}

export function computeTranscriptWindow(input: TranscriptWindowInput): TranscriptWindow {
  const layout = buildTranscriptLayout(input);
  return computeTranscriptWindowFromLayout({
    layout,
    scrollOffset: input.scrollOffset,
    viewportHeight: input.viewportHeight,
    overscan: input.overscan,
    maxItems: input.maxItems,
    anchorIndex: input.anchorIndex,
    anchorAlignment: input.anchorAlignment
  });
}
