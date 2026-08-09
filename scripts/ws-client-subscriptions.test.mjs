import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import ts from "typescript";

const sourceUrl = new URL("../src/lib/ws-client.ts", import.meta.url);
const source = (await readFile(sourceUrl, "utf8")).replace(
  'import { base } from "$app/paths";',
  'const base = "";'
);
const transpiled = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.ESNext,
    target: ts.ScriptTarget.ES2022
  }
}).outputText;
const { WebSocketRpcClient } = await import(
  `data:text/javascript;base64,${Buffer.from(transpiled).toString("base64")}`
);

class FakeWebSocket {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;
  static instances = [];

  readyState = FakeWebSocket.CONNECTING;
  listeners = new Map();
  sentRequests = [];

  constructor() {
    FakeWebSocket.instances.push(this);
    queueMicrotask(() => {
      if (this.readyState !== FakeWebSocket.CONNECTING) {
        return;
      }
      this.readyState = FakeWebSocket.OPEN;
      this.emit("open", {});
    });
  }

  addEventListener(type, listener) {
    const listeners = this.listeners.get(type) ?? new Set();
    listeners.add(listener);
    this.listeners.set(type, listeners);
  }

  send(encoded) {
    const message = JSON.parse(encoded);
    if (message.kind === "request") {
      this.sentRequests.push(message);
    }
  }

  respond(request, { ok = true, result = {}, error = "rejected" } = {}) {
    this.emit("message", {
      data: JSON.stringify({
        kind: "response",
        id: request.id,
        ok,
        ...(ok ? { result } : { error })
      })
    });
  }

  close() {
    if (this.readyState === FakeWebSocket.CLOSED) {
      return;
    }
    this.readyState = FakeWebSocket.CLOSED;
    this.emit("close", {});
  }

  emit(type, event) {
    for (const listener of this.listeners.get(type) ?? []) {
      listener(event);
    }
  }
}

globalThis.window = { location: { href: "http://localhost/" } };
globalThis.WebSocket = FakeWebSocket;

function requestsFor(socket, method) {
  return socket.sentRequests.filter((request) => request.method === method);
}

async function tick() {
  await new Promise((resolve) => setImmediate(resolve));
}

test("global subscriptions become active only after one acknowledged request", async () => {
  FakeWebSocket.instances = [];
  const client = new WebSocketRpcClient();
  const unsubscribeFirst = client.subscribeGlobal(() => {});
  const unsubscribeSecond = client.subscribeGlobal(() => {});
  await tick();

  const socket = FakeWebSocket.instances[0];
  assert.ok(socket);
  const subscribeRequest = requestsFor(socket, "events/subscribe");
  assert.equal(subscribeRequest.length, 1);
  assert.equal(client.auxiliarySubscriptionStates.get("global").syncedGeneration, null);

  socket.respond(subscribeRequest[0]);
  await tick();
  assert.equal(client.auxiliarySubscriptionStates.get("global").syncedGeneration, 1);

  unsubscribeFirst();
  await tick();
  assert.equal(requestsFor(socket, "events/unsubscribe").length, 0);

  unsubscribeSecond();
  await tick();
  const unsubscribeRequest = requestsFor(socket, "events/unsubscribe");
  assert.equal(unsubscribeRequest.length, 1);
  socket.respond(unsubscribeRequest[0]);
  await tick();
  assert.equal(client.auxiliarySubscriptionStates.has("global"), false);
  client.disconnect();
});

test("terminal subscriptions require a fresh ACK after reconnect without duplicates", async () => {
  FakeWebSocket.instances = [];
  const client = new WebSocketRpcClient();
  const unsubscribe = client.subscribeTerminal("terminal-1", () => {});
  await tick();

  const firstSocket = FakeWebSocket.instances[0];
  const firstRequest = requestsFor(firstSocket, "terminal/subscribe")[0];
  assert.ok(firstRequest);
  firstSocket.respond(firstRequest);
  await tick();
  assert.equal(client.auxiliarySubscriptionStates.get("terminal:terminal-1").syncedGeneration, 1);

  firstSocket.close();
  client.reconnectNow();
  await tick();

  const secondSocket = FakeWebSocket.instances[1];
  assert.ok(secondSocket);
  const secondRequests = requestsFor(secondSocket, "terminal/subscribe");
  assert.equal(secondRequests.length, 1);
  assert.equal(client.auxiliarySubscriptionStates.get("terminal:terminal-1").syncedGeneration, null);

  client.reconnectNow();
  await tick();
  assert.equal(requestsFor(secondSocket, "terminal/subscribe").length, 1);

  secondSocket.respond(secondRequests[0]);
  await tick();
  assert.equal(client.auxiliarySubscriptionStates.get("terminal:terminal-1").syncedGeneration, 2);

  unsubscribe();
  await tick();
  const unsubscribeRequest = requestsFor(secondSocket, "terminal/unsubscribe")[0];
  assert.ok(unsubscribeRequest);
  secondSocket.respond(unsubscribeRequest);
  await tick();
  client.disconnect();
});

test("a pending global subscribe recovers across connection generations", async () => {
  FakeWebSocket.instances = [];
  const client = new WebSocketRpcClient();
  const unsubscribe = client.subscribeGlobal(() => {});
  await tick();

  const firstSocket = FakeWebSocket.instances[0];
  assert.equal(requestsFor(firstSocket, "events/subscribe").length, 1);

  firstSocket.close();
  client.reconnectNow();
  await tick();
  await tick();

  const secondSocket = FakeWebSocket.instances[1];
  assert.ok(secondSocket);
  const secondRequests = requestsFor(secondSocket, "events/subscribe");
  assert.equal(secondRequests.length, 1);
  assert.equal(client.auxiliarySubscriptionStates.get("global").syncedGeneration, null);

  secondSocket.respond(secondRequests[0]);
  await tick();
  const state = client.auxiliarySubscriptionStates.get("global");
  assert.equal(state.syncedGeneration, 2);
  assert.equal(state.retryTimer, null);
  assert.equal(state.retryAttempt, 0);

  unsubscribe();
  await tick();
  const unsubscribeRequest = requestsFor(secondSocket, "events/unsubscribe")[0];
  assert.ok(unsubscribeRequest);
  secondSocket.respond(unsubscribeRequest);
  await tick();
  client.disconnect();
});

test("subscription rejection exhausts bounded retries and foreground recovery sends once", async () => {
  FakeWebSocket.instances = [];
  const client = new WebSocketRpcClient();
  const unsubscribe = client.subscribeGlobal(() => {});
  await tick();

  const socket = FakeWebSocket.instances[0];
  const firstRequest = requestsFor(socket, "events/subscribe")[0];
  const state = client.auxiliarySubscriptionStates.get("global");
  state.retryAttempt = Number.MAX_SAFE_INTEGER;
  socket.respond(firstRequest, { ok: false, error: "subscription rejected" });
  await tick();

  assert.equal(requestsFor(socket, "events/subscribe").length, 1);
  assert.equal(state.syncedGeneration, null);
  assert.equal(state.retryExhaustedGeneration, 1);

  client.reconnectNow();
  client.reconnectNow();
  await tick();
  const recoveredRequests = requestsFor(socket, "events/subscribe");
  assert.equal(recoveredRequests.length, 2);
  assert.equal(state.syncedGeneration, null);

  socket.respond(recoveredRequests[1]);
  await tick();
  assert.equal(state.syncedGeneration, 1);

  unsubscribe();
  await tick();
  const unsubscribeRequest = requestsFor(socket, "events/unsubscribe")[0];
  assert.ok(unsubscribeRequest);
  socket.respond(unsubscribeRequest);
  await tick();
  client.disconnect();
});
