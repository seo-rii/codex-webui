export type SessionCachePersistMode = "interactive" | "stream" | "terminal";

export const SESSION_CACHE_INTERACTIVE_DELAY_MS = 120;
export const SESSION_CACHE_STREAM_IDLE_DELAY_MS = 2_500;

export function sessionCachePersistDelay(mode: SessionCachePersistMode) {
  return mode === "stream" ? SESSION_CACHE_STREAM_IDLE_DELAY_MS : SESSION_CACHE_INTERACTIVE_DELAY_MS;
}

export function sessionCacheModeForStreamEvent(event: { kind: string; method?: string }): SessionCachePersistMode {
  if (event.kind !== "notification") {
    return "stream";
  }
  if (
    event.method === "turn/completed" ||
    event.method === "thread/realtime/closed" ||
    event.method === "thread/status/changed" ||
    event.method === "codex-webui/queueUpdated" ||
    event.method === "error"
  ) {
    return "terminal";
  }
  return "stream";
}
