import { base } from "$app/paths";

import type { GlobalStreamEvent, StreamEvent, TerminalEvent, WsConnectionState } from "$lib/types";

type ClientEnvelope =
  | {
      kind: "request";
      id: string;
      method: string;
      params: unknown;
    }
  | {
      kind: "ping";
      nonce?: string;
    };

type ServerEnvelope =
  | {
      kind: "ready";
      connectionId: string;
    }
  | {
      kind: "response";
      id: string;
      ok: boolean;
      result?: unknown;
      error?: string;
    }
  | {
      kind: "event";
      sessionId: string;
      profileId?: string | null;
      event: StreamEvent;
    }
  | {
      kind: "terminalEvent";
      terminalId: string;
      event: TerminalEvent;
    }
  | {
      kind: "globalEvent";
      event: GlobalStreamEvent;
    }
  | {
      kind: "resyncRequired";
      reason: string;
    }
  | {
      kind: "pong";
      nonce?: string;
    };

type PendingRequest = {
  message: Extract<ClientEnvelope, { kind: "request" }>;
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
  replayable: boolean;
  sentGeneration: number | null;
  timeoutTimer: ReturnType<typeof setTimeout>;
};

type SessionEventHandler = (event: StreamEvent) => void;
type TerminalEventHandler = (event: TerminalEvent) => void;
type SessionSubscriptionOptions = {
  includeInitialQueue?: boolean;
  profileId?: string | null;
};
type SessionSubscriptionState = {
  sessionId: string;
  options: Required<SessionSubscriptionOptions>;
  desired: boolean;
  syncedGeneration: number | null;
  syncing: boolean;
  retryAttempt: number;
  retryTimer: ReturnType<typeof setTimeout> | null;
};

const HEARTBEAT_MS = 20_000;
const CONNECT_TIMEOUT_MS = 12_000;
const PONG_TIMEOUT_MS = 10_000;
const FOREGROUND_STALE_MS = HEARTBEAT_MS + PONG_TIMEOUT_MS;
const REQUEST_TIMEOUT_MS = 720_000;
const SUBSCRIPTION_REQUEST_TIMEOUT_MS = 15_000;
const RECONNECT_DELAYS = [250, 500, 1000, 2000, 4000];
const SUBSCRIPTION_RETRY_DELAYS = [500, 1000, 2000, 5000, 10_000, 30_000];
const DEDUPED_READ_METHODS = new Set([
  "arena/list",
  "audit/list",
  "catalog/get",
  "codex/apps/list",
  "codex/features/list",
  "codex/hooks/list",
  "codex/mcp/status/list",
  "codex/plugins/list",
  "codex/plugins/read",
  "codex/realtime/listVoices",
  "codex/skills/list",
  "config/get",
  "diagnostics/parser/compare",
  "directories/browse",
  "editor/file/get",
  "files/search",
  "git/commit/diff",
  "git/file/get",
  "git/file/resolve",
  "git/github/pull",
  "git/github/pulls",
  "git/repositories/list",
  "git/status",
  "git/worktrees/list",
  "memory/status",
  "notifications/list",
  "runtime/checkUpdate",
  "runtime/processes/list",
  "runtime/quota",
  "runtime/status",
  "session/draft/get",
  "session/get",
  "session/goal/get",
  "session/itemDetail/get",
  "session/olderTurns/get",
  "session/queue/get",
  "session/rollbackTargets/list",
  "session/search",
  "session/turn/get",
  "sessions/list",
  "sessions/search",
  "terminal/list",
  "terminal/read"
]);

function appPath(pathname: string) {
  const normalized = pathname.startsWith("/") ? pathname : `/${pathname}`;
  return `${base}${normalized}` || "/";
}

function nextDelay(attempt: number) {
  return RECONNECT_DELAYS[Math.min(attempt, RECONNECT_DELAYS.length - 1)];
}

function makeRequestId() {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }

  return `ws-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;
}

function normalizeJsonForKey(value: unknown): unknown {
  if (value === null || typeof value === "string" || typeof value === "number" || typeof value === "boolean") {
    return value;
  }
  if (Array.isArray(value)) {
    return value.map((item) => (typeof item === "undefined" ? null : normalizeJsonForKey(item)));
  }
  if (typeof value === "object") {
    const input = value as Record<string, unknown>;
    const output: Record<string, unknown> = {};
    for (const key of Object.keys(input).sort()) {
      const item = input[key];
      if (typeof item === "undefined" || typeof item === "function" || typeof item === "symbol") {
        continue;
      }
      output[key] = normalizeJsonForKey(item);
    }
    return output;
  }
  return null;
}

function dedupeKeyForRequest(method: string, params: unknown) {
  if (!DEDUPED_READ_METHODS.has(method)) {
    return null;
  }
  try {
    return `${method}:${JSON.stringify(normalizeJsonForKey(params))}`;
  } catch {
    return null;
  }
}

function requestCanReplayAfterReconnect(method: string, params: unknown) {
  if (!params || typeof params !== "object") {
    return false;
  }
  const payload = params as Record<string, unknown>;
  if (method === "turn/send" || method === "turn/steer") {
    return typeof payload.clientUserMessageId === "string" && payload.clientUserMessageId.trim().length > 0;
  }
  if (method === "session/queue/enqueue") {
    return typeof payload.clientRequestId === "string" && payload.clientRequestId.trim().length > 0;
  }
  return false;
}

function normalizeSubscriptionProfileId(profileId: string | null | undefined) {
  const value = typeof profileId === "string" ? profileId.trim() : "";
  return value || "default";
}

function sessionSubscriptionKey(sessionId: string, profileId: string | null | undefined) {
  return `${normalizeSubscriptionProfileId(profileId)}:${sessionId}`;
}

function profileIdFromSessionEvent(event: StreamEvent) {
  const params = event && typeof event === "object" && "params" in event ? event.params : null;
  if (!params || typeof params !== "object") {
    return null;
  }
  const profileId = (params as { profileId?: unknown; profile_id?: unknown }).profileId ?? (params as { profile_id?: unknown }).profile_id;
  return typeof profileId === "string" && profileId.trim() ? profileId.trim() : null;
}

export class WebSocketRpcClient {
  private socket: WebSocket | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private heartbeatTimer: ReturnType<typeof setInterval> | null = null;
  private connectTimeoutTimer: ReturnType<typeof setTimeout> | null = null;
  private pongTimeoutTimer: ReturnType<typeof setTimeout> | null = null;
  private reconnectAttempt = 0;
  private connectionGeneration = 0;
  private manualClose = false;
  private hasConnectedOnce = false;
  private lastActivityAt = 0;
  private pending = new Map<string, PendingRequest>();
  private inflightReadRequests = new Map<string, Promise<unknown>>();
  private sessionHandlers = new Map<string, Set<SessionEventHandler>>();
  private sessionSubscriptionOptions = new Map<string, Required<SessionSubscriptionOptions>>();
  private sessionSubscriptionStates = new Map<string, SessionSubscriptionState>();
  private terminalHandlers = new Map<string, Set<TerminalEventHandler>>();
  private globalHandlers = new Set<(event: GlobalStreamEvent) => void>();
  private reconnectListeners = new Set<() => void>();
  private resyncRequiredListeners = new Set<(reason: string) => void>();
  private connectionState: WsConnectionState = "idle";
  private connectionStateListeners = new Set<(state: WsConnectionState) => void>();
  private defaultProfileId: string | null = null;

  setDefaultProfileId(profileId: string | null | undefined) {
    const normalized = typeof profileId === "string" && profileId.trim() ? profileId.trim() : null;
    this.defaultProfileId = normalized;
  }

  request<T>(method: string, params: unknown = {}, timeoutMs = REQUEST_TIMEOUT_MS): Promise<T> {
    if (typeof window === "undefined") {
      return Promise.reject(new Error("WebSocket requests are only available in the browser."));
    }

    const scopedParams =
      this.defaultProfileId && params && typeof params === "object" && !Array.isArray(params) && !("requestProfileId" in params)
        ? { ...(params as Record<string, unknown>), requestProfileId: this.defaultProfileId }
        : params;
    const dedupeKey = dedupeKeyForRequest(method, scopedParams);
    if (dedupeKey) {
      const existing = this.inflightReadRequests.get(dedupeKey);
      if (existing) {
        return existing as Promise<T>;
      }
    }

    const id = makeRequestId();
    const message: Extract<ClientEnvelope, { kind: "request" }> = {
      kind: "request",
      id,
      method,
      params: scopedParams
    };

    const promise = new Promise<T>((resolve, reject) => {
      const timeoutTimer = setTimeout(() => {
        const pending = this.pending.get(id);
        if (!pending) {
          return;
        }

        this.pending.delete(id);
        pending.reject(new Error("WebSocket request timed out."));
        this.sendPing(true);
      }, timeoutMs);

      this.pending.set(id, {
        message,
        resolve: resolve as (value: unknown) => void,
        reject,
        replayable: requestCanReplayAfterReconnect(method, scopedParams),
        sentGeneration: null,
        timeoutTimer
      });
      this.ensureConnected();
      this.flushPending();
    });
    if (dedupeKey) {
      this.inflightReadRequests.set(dedupeKey, promise);
      const cleanup = () => {
        if (this.inflightReadRequests.get(dedupeKey) === promise) {
          this.inflightReadRequests.delete(dedupeKey);
        }
      };
      void promise.then(cleanup, cleanup);
    }
    return promise;
  }

  subscribeSession(sessionId: string, handler: SessionEventHandler, options: SessionSubscriptionOptions = {}) {
    const subscriptionKey = sessionSubscriptionKey(sessionId, options.profileId ?? null);
    const current = this.sessionHandlers.get(subscriptionKey) ?? new Set<SessionEventHandler>();
    const shouldSubscribe = current.size === 0;
    current.add(handler);
    this.sessionHandlers.set(subscriptionKey, current);
    if (shouldSubscribe) {
      const normalizedOptions = {
        includeInitialQueue: options.includeInitialQueue ?? true,
        profileId: normalizeSubscriptionProfileId(options.profileId ?? null)
      };
      this.sessionSubscriptionOptions.set(subscriptionKey, normalizedOptions);
      const state = this.sessionSubscriptionStates.get(subscriptionKey) ?? {
        sessionId,
        options: normalizedOptions,
        desired: true,
        syncedGeneration: null,
        syncing: false,
        retryAttempt: 0,
        retryTimer: null
      };
      this.clearSessionSubscriptionRetry(state);
      state.options = normalizedOptions;
      state.desired = true;
      this.sessionSubscriptionStates.set(subscriptionKey, state);
    }
    this.ensureConnected();
    if (shouldSubscribe) {
      void this.syncSessionSubscription(subscriptionKey);
    }

    return () => {
      const handlers = this.sessionHandlers.get(subscriptionKey);
      if (!handlers) {
        return;
      }

      handlers.delete(handler);
      if (handlers.size > 0) {
        return;
      }

      const options = this.sessionSubscriptionOptions.get(subscriptionKey);
      this.sessionHandlers.delete(subscriptionKey);
      const state = this.sessionSubscriptionStates.get(subscriptionKey) ?? {
        sessionId,
        options: options ?? {
          includeInitialQueue: true,
          profileId: normalizeSubscriptionProfileId(null)
        },
        desired: false,
        syncedGeneration: null,
        syncing: false,
        retryAttempt: 0,
        retryTimer: null
      };
      this.clearSessionSubscriptionRetry(state);
      state.desired = false;
      this.sessionSubscriptionStates.set(subscriptionKey, state);
      void this.syncSessionSubscription(subscriptionKey);
    };
  }

  subscribeGlobal(handler: (event: GlobalStreamEvent) => void) {
    const shouldSubscribe = this.globalHandlers.size === 0;
    this.globalHandlers.add(handler);
    this.ensureConnected();
    if (shouldSubscribe) {
      this.sendTransient("events/subscribe", {});
    }

    return () => {
      this.globalHandlers.delete(handler);
      if (this.globalHandlers.size === 0) {
        this.sendTransient("events/unsubscribe", {});
      }
    };
  }

  subscribeTerminal(terminalId: string, handler: TerminalEventHandler) {
    const current = this.terminalHandlers.get(terminalId) ?? new Set<TerminalEventHandler>();
    const shouldSubscribe = current.size === 0;
    current.add(handler);
    this.terminalHandlers.set(terminalId, current);
    this.ensureConnected();
    if (shouldSubscribe) {
      this.sendTransient("terminal/subscribe", { terminalId });
    }

    return () => {
      const handlers = this.terminalHandlers.get(terminalId);
      if (!handlers) {
        return;
      }

      handlers.delete(handler);
      if (handlers.size > 0) {
        return;
      }

      this.terminalHandlers.delete(terminalId);
      this.sendTransient("terminal/unsubscribe", { terminalId });
    };
  }

  onReconnect(listener: () => void) {
    this.reconnectListeners.add(listener);
    return () => {
      this.reconnectListeners.delete(listener);
    };
  }

  onResyncRequired(listener: (reason: string) => void) {
    this.resyncRequiredListeners.add(listener);
    return () => {
      this.resyncRequiredListeners.delete(listener);
    };
  }

  onConnectionState(listener: (state: WsConnectionState) => void) {
    this.connectionStateListeners.add(listener);
    listener(this.connectionState);
    return () => {
      this.connectionStateListeners.delete(listener);
    };
  }

  reconnectNow() {
    if (typeof window === "undefined" || !this.hasConnectionDemand()) {
      return;
    }

    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    this.reconnectAttempt = 0;

    if (this.socket?.readyState === WebSocket.OPEN) {
      const staleOpenSocket = this.lastActivityAt > 0 && Date.now() - this.lastActivityAt > FOREGROUND_STALE_MS;
      if (staleOpenSocket) {
        this.forceReconnect();
        return;
      }
      this.sendPing(true);
      this.flushPending();
      return;
    }

    if (this.socket?.readyState === WebSocket.CONNECTING) {
      this.forceReconnect();
      return;
    }

    this.socket = null;
    this.manualClose = false;
    this.ensureConnected();
  }

  disconnect(message = "WebSocket connection closed.") {
    this.manualClose = true;
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    if (this.heartbeatTimer) {
      clearInterval(this.heartbeatTimer);
      this.heartbeatTimer = null;
    }
    this.clearConnectTimeout();
    this.clearPongTimeout();

    if (this.socket) {
      this.socket.close();
      this.socket = null;
    }
    this.setConnectionState("disconnected");

    for (const pending of this.pending.values()) {
      clearTimeout(pending.timeoutTimer);
      pending.reject(new Error(message));
    }
    this.pending.clear();
    this.inflightReadRequests.clear();
    for (const state of this.sessionSubscriptionStates.values()) {
      this.clearSessionSubscriptionRetry(state);
    }
  }

  private ensureConnected() {
    if (typeof window === "undefined") {
      return;
    }

    if (this.socket && (this.socket.readyState === WebSocket.OPEN || this.socket.readyState === WebSocket.CONNECTING)) {
      return;
    }

    this.manualClose = false;
    this.setConnectionState(this.hasConnectedOnce ? "reconnecting" : "connecting");
    const socket = new WebSocket(this.buildWebSocketUrl());
    this.socket = socket;
    this.startConnectTimeout(socket);

    socket.addEventListener("open", () => {
      if (this.socket !== socket) {
        return;
      }

      this.clearConnectTimeout();
      this.clearPongTimeout();
      this.lastActivityAt = Date.now();
      const isReconnect = this.hasConnectedOnce;
      this.hasConnectedOnce = true;
      this.reconnectAttempt = 0;
      this.connectionGeneration += 1;
      this.startHeartbeat();
      this.restoreSubscriptions();
      this.flushPending();
      this.setConnectionState("connected");

      if (isReconnect) {
        for (const listener of this.reconnectListeners) {
          listener();
        }
      }
    });

    socket.addEventListener("message", (event) => {
      if (this.socket !== socket) {
        return;
      }
      if (typeof event.data !== "string") {
        return;
      }

      let payload: ServerEnvelope;
      try {
        payload = JSON.parse(event.data) as ServerEnvelope;
      } catch {
        return;
      }

      this.lastActivityAt = Date.now();
      this.clearPongTimeout();

      if (payload.kind === "pong") {
        return;
      }

      if (payload.kind === "resyncRequired") {
        for (const listener of this.resyncRequiredListeners) {
          listener(payload.reason);
        }
        this.forceReconnect();
        return;
      }

      if (payload.kind === "response") {
        const pending = this.pending.get(payload.id);
        if (!pending) {
          return;
        }

        this.pending.delete(payload.id);
        clearTimeout(pending.timeoutTimer);
        if (payload.ok) {
          pending.resolve(payload.result);
        } else {
          pending.reject(new Error(payload.error || "WebSocket request failed."));
        }
        return;
      }

      if (payload.kind === "event") {
        const eventProfileId =
          normalizeSubscriptionProfileId(payload.profileId) !== "default" || payload.profileId
            ? normalizeSubscriptionProfileId(payload.profileId)
            : profileIdFromSessionEvent(payload.event);
        const handlerSets = eventProfileId
          ? [this.sessionHandlers.get(sessionSubscriptionKey(payload.sessionId, eventProfileId))]
          : Array.from(this.sessionHandlers.entries())
              .filter(([key]) => key.endsWith(`:${payload.sessionId}`))
              .map(([, handlers]) => handlers);
        if (handlerSets.every((handlers) => !handlers)) {
          return;
        }
        for (const handlers of handlerSets) {
          if (!handlers) {
            continue;
          }
          for (const handler of handlers) {
            handler(payload.event);
          }
        }
        return;
      }

      if (payload.kind === "terminalEvent") {
        const handlers = this.terminalHandlers.get(payload.terminalId);
        if (!handlers) {
          return;
        }
        for (const handler of handlers) {
          handler(payload.event);
        }
        return;
      }

      if (payload.kind === "globalEvent") {
        for (const handler of this.globalHandlers) {
          handler(payload.event);
        }
      }
    });

    socket.addEventListener("close", () => {
      if (this.socket !== socket) {
        return;
      }
      this.socket = null;
      this.stopHeartbeat();
      this.clearConnectTimeout();
      this.clearPongTimeout();

      if (!this.manualClose && this.hasConnectionDemand()) {
        this.setConnectionState("reconnecting");
        this.scheduleReconnect();
      } else {
        this.setConnectionState("disconnected");
      }
    });

    socket.addEventListener("error", () => {
      socket.close();
    });
  }

  private scheduleReconnect() {
    if (this.reconnectTimer || !this.hasConnectionDemand()) {
      return;
    }

    const delay = nextDelay(this.reconnectAttempt);
    this.reconnectAttempt += 1;
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.ensureConnected();
    }, delay);
  }

  private hasConnectionDemand() {
    return this.pending.size > 0 || this.sessionHandlers.size > 0 || this.terminalHandlers.size > 0 || this.globalHandlers.size > 0;
  }

  private flushPending() {
    if (!this.socket || this.socket.readyState !== WebSocket.OPEN) {
      return;
    }

    for (const [id, pending] of [...this.pending.entries()]) {
      if (pending.sentGeneration === this.connectionGeneration) {
        continue;
      }
      if (pending.sentGeneration !== null && !pending.replayable) {
        this.pending.delete(id);
        clearTimeout(pending.timeoutTimer);
        const skippedReadReplay = DEDUPED_READ_METHODS.has(pending.message.method);
        pending.reject(
          new Error(
            skippedReadReplay
              ? "WebSocket reconnected before the read completed. Refreshing server state."
              : "WebSocket reconnected before the mutation completed. Refreshing server state."
          )
        );
        for (const listener of this.resyncRequiredListeners) {
          listener(skippedReadReplay ? "readReplaySkipped" : "mutationReplaySkipped");
        }
        continue;
      }

      this.socket.send(JSON.stringify(pending.message));
      pending.sentGeneration = this.connectionGeneration;
    }
  }

  private restoreSubscriptions() {
    if (this.globalHandlers.size > 0) {
      this.sendTransient("events/subscribe", {});
    }

    for (const [subscriptionKey, state] of this.sessionSubscriptionStates.entries()) {
      this.clearSessionSubscriptionRetry(state);
      state.syncedGeneration = null;
      if (state.desired) {
        void this.syncSessionSubscription(subscriptionKey);
      }
    }

    for (const terminalId of this.terminalHandlers.keys()) {
      this.sendTransient("terminal/subscribe", { terminalId });
    }
  }

  private sendTransient(method: string, params: unknown) {
    if (!this.socket || this.socket.readyState !== WebSocket.OPEN) {
      return;
    }

    const scopedParams =
      this.defaultProfileId && params && typeof params === "object" && !Array.isArray(params) && !("requestProfileId" in params)
        ? { ...(params as Record<string, unknown>), requestProfileId: this.defaultProfileId }
        : params;
    const payload: ClientEnvelope = {
      kind: "request",
      id: makeRequestId(),
      method,
      params: scopedParams
    };
    this.socket.send(JSON.stringify(payload));
  }

  private async syncSessionSubscription(subscriptionKey: string) {
    const state = this.sessionSubscriptionStates.get(subscriptionKey);
    if (!state || state.syncing) {
      return;
    }

    state.syncing = true;
    try {
      while (this.sessionSubscriptionStates.get(subscriptionKey) === state) {
        const socketOpen = this.socket?.readyState === WebSocket.OPEN;
        const subscribed = state.syncedGeneration === this.connectionGeneration;
        if (!socketOpen) {
          if (!state.desired) {
            this.sessionSubscriptionStates.delete(subscriptionKey);
            this.sessionSubscriptionOptions.delete(subscriptionKey);
          }
          return;
        }
        if (state.desired === subscribed) {
          if (!state.desired) {
            this.sessionSubscriptionStates.delete(subscriptionKey);
            this.sessionSubscriptionOptions.delete(subscriptionKey);
          }
          return;
        }

        const generation = this.connectionGeneration;
        const subscribing = state.desired;
        try {
          await this.request(
            subscribing ? "session/subscribe" : "session/unsubscribe",
            subscribing
              ? {
                  sessionId: state.sessionId,
                  includeInitialQueue: state.options.includeInitialQueue,
                  profileId: state.options.profileId
                }
              : {
                  sessionId: state.sessionId,
                  profileId: state.options.profileId
                },
            SUBSCRIPTION_REQUEST_TIMEOUT_MS
          );
        } catch {
          state.syncedGeneration = subscribing ? null : generation;
          this.scheduleSessionSubscriptionRetry(subscriptionKey, state, generation);
          return;
        }

        if (generation !== this.connectionGeneration) {
          state.syncedGeneration = null;
          continue;
        }
        state.retryAttempt = 0;
        state.syncedGeneration = subscribing ? generation : null;
      }
    } finally {
      state.syncing = false;
      const latest = this.sessionSubscriptionStates.get(subscriptionKey);
      const subscribed = latest?.syncedGeneration === this.connectionGeneration;
      if (
        latest &&
        !latest.retryTimer &&
        this.socket?.readyState === WebSocket.OPEN &&
        latest.desired !== subscribed
      ) {
        queueMicrotask(() => {
          void this.syncSessionSubscription(subscriptionKey);
        });
      }
    }
  }

  private clearSessionSubscriptionRetry(state: SessionSubscriptionState) {
    if (state.retryTimer) {
      clearTimeout(state.retryTimer);
      state.retryTimer = null;
    }
    state.retryAttempt = 0;
  }

  private scheduleSessionSubscriptionRetry(
    subscriptionKey: string,
    state: SessionSubscriptionState,
    generation: number
  ) {
    if (state.retryTimer || this.sessionSubscriptionStates.get(subscriptionKey) !== state) {
      return;
    }
    const delay = SUBSCRIPTION_RETRY_DELAYS[Math.min(state.retryAttempt, SUBSCRIPTION_RETRY_DELAYS.length - 1)];
    state.retryAttempt += 1;
    state.retryTimer = setTimeout(() => {
      state.retryTimer = null;
      if (
        this.sessionSubscriptionStates.get(subscriptionKey) !== state ||
        generation !== this.connectionGeneration ||
        this.socket?.readyState !== WebSocket.OPEN
      ) {
        return;
      }
      void this.syncSessionSubscription(subscriptionKey);
    }, delay);
  }

  private startHeartbeat() {
    this.stopHeartbeat();
    this.heartbeatTimer = setInterval(() => {
      this.sendPing(true);
    }, HEARTBEAT_MS);
  }

  private stopHeartbeat() {
    if (this.heartbeatTimer) {
      clearInterval(this.heartbeatTimer);
      this.heartbeatTimer = null;
    }
  }

  private sendPing(expectPong = false) {
    const socket = this.socket;
    if (!socket || socket.readyState !== WebSocket.OPEN) {
      return false;
    }

    const payload: ClientEnvelope = {
      kind: "ping",
      nonce: makeRequestId()
    };
    try {
      socket.send(JSON.stringify(payload));
      if (expectPong) {
        this.startPongTimeout(socket);
      }
      return true;
    } catch {
      socket.close();
      return false;
    }
  }

  private startConnectTimeout(socket: WebSocket) {
    this.clearConnectTimeout();
    this.connectTimeoutTimer = setTimeout(() => {
      if (this.socket !== socket || socket.readyState !== WebSocket.CONNECTING) {
        return;
      }
      this.forceReconnect();
    }, CONNECT_TIMEOUT_MS);
  }

  private clearConnectTimeout() {
    if (this.connectTimeoutTimer) {
      clearTimeout(this.connectTimeoutTimer);
      this.connectTimeoutTimer = null;
    }
  }

  private startPongTimeout(socket: WebSocket) {
    this.clearPongTimeout();
    this.pongTimeoutTimer = setTimeout(() => {
      if (this.socket !== socket || socket.readyState !== WebSocket.OPEN) {
        return;
      }
      this.forceReconnect();
    }, PONG_TIMEOUT_MS);
  }

  private clearPongTimeout() {
    if (this.pongTimeoutTimer) {
      clearTimeout(this.pongTimeoutTimer);
      this.pongTimeoutTimer = null;
    }
  }

  private forceReconnect() {
    if (typeof window === "undefined" || !this.hasConnectionDemand()) {
      return;
    }

    const socket = this.socket;
    this.socket = null;
    this.manualClose = false;
    this.stopHeartbeat();
    this.clearConnectTimeout();
    this.clearPongTimeout();
    this.setConnectionState(this.hasConnectedOnce ? "reconnecting" : "connecting");

    if (socket && socket.readyState !== WebSocket.CLOSING && socket.readyState !== WebSocket.CLOSED) {
      try {
        socket.close();
      } catch {
        // The replacement connection below is the recovery path.
      }
    }

    this.ensureConnected();
  }

  private buildWebSocketUrl() {
    const url = new URL(appPath("/ws"), window.location.href);
    url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
    return url.toString();
  }

  private setConnectionState(state: WsConnectionState) {
    if (this.connectionState === state) {
      return;
    }

    this.connectionState = state;
    for (const listener of this.connectionStateListeners) {
      listener(state);
    }
  }
}
