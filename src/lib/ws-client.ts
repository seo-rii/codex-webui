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
      kind: "pong";
      nonce?: string;
    };

type PendingRequest = {
  message: Extract<ClientEnvelope, { kind: "request" }>;
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
  sentGeneration: number | null;
};

type SessionEventHandler = (event: StreamEvent) => void;
type TerminalEventHandler = (event: TerminalEvent) => void;

const HEARTBEAT_MS = 20_000;
const CONNECT_TIMEOUT_MS = 12_000;
const PONG_TIMEOUT_MS = 10_000;
const FOREGROUND_STALE_MS = HEARTBEAT_MS + PONG_TIMEOUT_MS;
const RECONNECT_DELAYS = [250, 500, 1000, 2000, 4000];

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
  private sessionHandlers = new Map<string, Set<SessionEventHandler>>();
  private terminalHandlers = new Map<string, Set<TerminalEventHandler>>();
  private globalHandlers = new Set<(event: GlobalStreamEvent) => void>();
  private reconnectListeners = new Set<() => void>();
  private connectionState: WsConnectionState = "idle";
  private connectionStateListeners = new Set<(state: WsConnectionState) => void>();

  request<T>(method: string, params: unknown = {}): Promise<T> {
    if (typeof window === "undefined") {
      return Promise.reject(new Error("WebSocket requests are only available in the browser."));
    }

    const id = makeRequestId();
    const message: Extract<ClientEnvelope, { kind: "request" }> = {
      kind: "request",
      id,
      method,
      params
    };

    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, {
        message,
        resolve: resolve as (value: unknown) => void,
        reject,
        sentGeneration: null
      });
      this.ensureConnected();
      this.flushPending();
    });
  }

  subscribeSession(sessionId: string, handler: SessionEventHandler) {
    const current = this.sessionHandlers.get(sessionId) ?? new Set<SessionEventHandler>();
    current.add(handler);
    this.sessionHandlers.set(sessionId, current);
    this.ensureConnected();
    this.sendTransient("session/subscribe", { sessionId });

    return () => {
      const handlers = this.sessionHandlers.get(sessionId);
      if (!handlers) {
        return;
      }

      handlers.delete(handler);
      if (handlers.size > 0) {
        return;
      }

      this.sessionHandlers.delete(sessionId);
      this.sendTransient("session/unsubscribe", { sessionId });
    };
  }

  subscribeGlobal(handler: (event: GlobalStreamEvent) => void) {
    this.globalHandlers.add(handler);
    this.ensureConnected();
    this.sendTransient("events/subscribe", {});

    return () => {
      this.globalHandlers.delete(handler);
      if (this.globalHandlers.size === 0) {
        this.sendTransient("events/unsubscribe", {});
      }
    };
  }

  subscribeTerminal(terminalId: string, handler: TerminalEventHandler) {
    const current = this.terminalHandlers.get(terminalId) ?? new Set<TerminalEventHandler>();
    current.add(handler);
    this.terminalHandlers.set(terminalId, current);
    this.ensureConnected();
    this.sendTransient("terminal/subscribe", { terminalId });

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
      pending.reject(new Error(message));
    }
    this.pending.clear();
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

      if (payload.kind === "response") {
        const pending = this.pending.get(payload.id);
        if (!pending) {
          return;
        }

        this.pending.delete(payload.id);
        if (payload.ok) {
          pending.resolve(payload.result);
        } else {
          pending.reject(new Error(payload.error || "WebSocket request failed."));
        }
        return;
      }

      if (payload.kind === "event") {
        const handlers = this.sessionHandlers.get(payload.sessionId);
        if (!handlers) {
          return;
        }
        for (const handler of handlers) {
          handler(payload.event);
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

    for (const pending of this.pending.values()) {
      if (pending.sentGeneration === this.connectionGeneration) {
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

    for (const sessionId of this.sessionHandlers.keys()) {
      this.sendTransient("session/subscribe", { sessionId });
    }

    for (const terminalId of this.terminalHandlers.keys()) {
      this.sendTransient("terminal/subscribe", { terminalId });
    }
  }

  private sendTransient(method: string, params: unknown) {
    if (!this.socket || this.socket.readyState !== WebSocket.OPEN) {
      return;
    }

    const payload: ClientEnvelope = {
      kind: "request",
      id: makeRequestId(),
      method,
      params
    };
    this.socket.send(JSON.stringify(payload));
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
