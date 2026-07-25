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

const readMethods = new Set(["session/itemDetail/get"]);

class FakeWebSocket {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;
  static instances = [];

  readyState = FakeWebSocket.CONNECTING;
  listeners = new Map();
  sentRequests = [];
  inflightMethods = new Map();
  maxInflight = 0;
  maxInflightReads = 0;

  constructor() {
    FakeWebSocket.instances.push(this);
    queueMicrotask(() => {
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
    if (message.kind !== "request") {
      return;
    }
    this.sentRequests.push(message);
    this.inflightMethods.set(message.id, message.method);
    this.maxInflight = Math.max(this.maxInflight, this.inflightMethods.size);
    this.maxInflightReads = Math.max(
      this.maxInflightReads,
      [...this.inflightMethods.values()].filter((method) => readMethods.has(method)).length
    );
  }

  respond(request) {
    this.inflightMethods.delete(request.id);
    this.emit("message", {
      data: JSON.stringify({
        kind: "response",
        id: request.id,
        ok: true,
        result: { id: request.id }
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

async function drainRequests(socket, promises) {
  let responseIndex = 0;
  while (responseIndex < promises.length) {
    const request = socket.sentRequests[responseIndex];
    assert.ok(request, `request ${responseIndex + 1} should eventually be sent`);
    socket.respond(request);
    responseIndex += 1;
    await Promise.resolve();
  }
  await Promise.all(promises);
}

test("queues bulk reads below the gateway connection limit", async () => {
  FakeWebSocket.instances = [];
  const client = new WebSocketRpcClient();
  const promises = Array.from({ length: 40 }, (_, index) =>
    client.request(
      "session/itemDetail/get",
      { sessionId: "session", turnId: "turn", itemId: `item-${index}` },
      5_000
    )
  );
  await Promise.resolve();

  const socket = FakeWebSocket.instances[0];
  assert.ok(socket);
  assert.equal(socket.sentRequests.length, 12);
  await drainRequests(socket, promises);
  assert.equal(socket.maxInflightReads, 12);
  assert.equal(socket.sentRequests.length, 40);
  client.disconnect();
});

test("reserves request capacity for control mutations during bulk reads", async () => {
  FakeWebSocket.instances = [];
  const client = new WebSocketRpcClient();
  const readPromises = Array.from({ length: 30 }, (_, index) =>
    client.request(
      "session/itemDetail/get",
      { sessionId: "session", turnId: "turn", itemId: `item-${index}` },
      5_000
    )
  );
  const controlPromises = Array.from({ length: 12 }, (_, index) =>
    client.request("config/update", { sequence: index }, 5_000)
  );
  const promises = [...readPromises, ...controlPromises];
  await Promise.resolve();

  const socket = FakeWebSocket.instances[0];
  assert.ok(socket);
  assert.equal(socket.sentRequests.length, 20);
  assert.equal(socket.sentRequests.filter((request) => readMethods.has(request.method)).length, 12);
  await drainRequests(socket, promises);
  assert.equal(socket.maxInflight, 20);
  assert.equal(socket.maxInflightReads, 12);
  client.disconnect();
});
