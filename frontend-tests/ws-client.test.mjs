import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import test from "node:test";

registerHooks({
  resolve(specifier, context, nextResolve) {
    if (specifier === "$app/paths") {
      return {
        url: "data:text/javascript,export const base = ''",
        shortCircuit: true
      };
    }
    return nextResolve(specifier, context);
  }
});

class MockWebSocket extends EventTarget {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;
  static instances = [];

  readyState = MockWebSocket.CONNECTING;
  sent = [];

  constructor() {
    super();
    MockWebSocket.instances.push(this);
  }

  open() {
    this.readyState = MockWebSocket.OPEN;
    this.dispatchEvent(new Event("open"));
  }

  send(raw) {
    this.sent.push(JSON.parse(raw));
  }

  respond(message) {
    this.dispatchEvent(
      new MessageEvent("message", {
        data: JSON.stringify({ kind: "response", id: message.id, ok: true, result: {} })
      })
    );
  }

  respondError(message, error = "subscription rejected") {
    this.dispatchEvent(
      new MessageEvent("message", {
        data: JSON.stringify({ kind: "response", id: message.id, ok: false, error })
      })
    );
  }

  close() {
    this.readyState = MockWebSocket.CLOSED;
    this.dispatchEvent(new Event("close"));
  }
}

globalThis.window = { location: new URL("http://localhost/") };
globalThis.WebSocket = MockWebSocket;

const { WebSocketRpcClient } = await import("../src/lib/ws-client.ts");

function requests(socket, method, sessionId) {
  return socket.sent.filter(
    (message) => message.kind === "request" && message.method === method && message.params.sessionId === sessionId
  );
}

async function settle() {
  await Promise.resolve();
  await Promise.resolve();
}

test("A-B-A subscription changes finish with A subscribed and B unsubscribed", async () => {
  const client = new WebSocketRpcClient();
  const releaseInitialA = client.subscribeSession("A", () => {}, { profileId: "profile-1" });
  const socket = MockWebSocket.instances.at(-1);
  socket.open();
  await settle();

  const initialSubscribeA = requests(socket, "session/subscribe", "A").at(-1);
  assert.ok(initialSubscribeA);
  socket.respond(initialSubscribeA);
  await settle();

  releaseInitialA();
  const releaseB = client.subscribeSession("B", () => {}, { profileId: "profile-1" });
  await settle();
  const unsubscribeA = requests(socket, "session/unsubscribe", "A").at(-1);
  const subscribeB = requests(socket, "session/subscribe", "B").at(-1);
  assert.ok(unsubscribeA);
  assert.ok(subscribeB);

  releaseB();
  const releaseFinalA = client.subscribeSession("A", () => {}, { profileId: "profile-1" });

  socket.respond(subscribeB);
  socket.respond(unsubscribeA);
  await settle();

  const unsubscribeB = requests(socket, "session/unsubscribe", "B").at(-1);
  const finalSubscribeA = requests(socket, "session/subscribe", "A").at(-1);
  assert.ok(unsubscribeB);
  assert.ok(finalSubscribeA);
  assert.notEqual(finalSubscribeA.id, initialSubscribeA.id);

  socket.respond(unsubscribeB);
  socket.respond(finalSubscribeA);
  await settle();

  assert.equal(requests(socket, "session/subscribe", "A").at(-1).id, finalSubscribeA.id);
  assert.equal(requests(socket, "session/unsubscribe", "B").at(-1).id, unsubscribeB.id);

  releaseFinalA();
  await settle();
  const finalUnsubscribeA = requests(socket, "session/unsubscribe", "A").at(-1);
  socket.respond(finalUnsubscribeA);
  await settle();
  client.disconnect();
});

test("changing the default profile keeps the socket and scopes later requests", async () => {
  const client = new WebSocketRpcClient();
  client.setDefaultProfileId("profile-1");
  const firstRequest = client.request("config/get");
  const socket = MockWebSocket.instances.at(-1);
  socket.open();
  await settle();

  const firstMessage = socket.sent.find((message) => message.method === "config/get");
  assert.equal(firstMessage.params.requestProfileId, "profile-1");
  socket.respond(firstMessage);
  await firstRequest;

  client.setDefaultProfileId("profile-2");
  const secondRequest = client.request("runtime/status");
  await settle();
  const secondMessage = socket.sent.find((message) => message.method === "runtime/status");
  assert.equal(secondMessage.params.requestProfileId, "profile-2");
  assert.equal(MockWebSocket.instances.at(-1), socket);
  socket.respond(secondMessage);
  await secondRequest;

  const explicitRequest = client.request("config/get", { profileId: "profile-1" });
  await settle();
  const explicitMessage = socket.sent.filter((message) => message.method === "config/get").at(-1);
  assert.equal(explicitMessage.params.profileId, "profile-1");
  assert.equal(explicitMessage.params.requestProfileId, "profile-2");
  socket.respond(explicitMessage);
  await explicitRequest;
  client.disconnect();
});

test("moving a session subscription between profiles keeps the existing socket", async () => {
  const client = new WebSocketRpcClient();
  const releaseSource = client.subscribeSession("session-1", () => {}, { profileId: "profile-1" });
  const socket = MockWebSocket.instances.at(-1);
  const socketCount = MockWebSocket.instances.length;
  socket.open();
  await settle();

  const sourceSubscribe = requests(socket, "session/subscribe", "session-1").at(-1);
  assert.equal(sourceSubscribe.params.profileId, "profile-1");
  socket.respond(sourceSubscribe);
  await settle();

  releaseSource();
  const releaseTarget = client.subscribeSession("session-1", () => {}, { profileId: "profile-2" });
  await settle();

  const sourceUnsubscribe = requests(socket, "session/unsubscribe", "session-1").at(-1);
  const targetSubscribe = requests(socket, "session/subscribe", "session-1").at(-1);
  assert.equal(sourceUnsubscribe.params.profileId, "profile-1");
  assert.equal(targetSubscribe.params.profileId, "profile-2");
  assert.equal(MockWebSocket.instances.length, socketCount);
  assert.equal(MockWebSocket.instances.at(-1), socket);

  socket.respond(sourceUnsubscribe);
  socket.respond(targetSubscribe);
  await settle();

  releaseTarget();
  await settle();
  const targetUnsubscribe = requests(socket, "session/unsubscribe", "session-1").at(-1);
  assert.equal(targetUnsubscribe.params.profileId, "profile-2");
  socket.respond(targetUnsubscribe);
  await settle();
  client.disconnect();
});

test("rejected session subscriptions retry with bounded backoff", async () => {
  const client = new WebSocketRpcClient();
  const release = client.subscribeSession("session-retry", () => {}, { profileId: "profile-1" });
  const socket = MockWebSocket.instances.at(-1);
  socket.open();
  await settle();

  const initialSubscribe = requests(socket, "session/subscribe", "session-retry").at(-1);
  assert.ok(initialSubscribe);
  socket.respondError(initialSubscribe);
  await settle();

  assert.equal(requests(socket, "session/subscribe", "session-retry").length, 1);
  await new Promise((resolve) => setTimeout(resolve, 600));

  const retryRequests = requests(socket, "session/subscribe", "session-retry");
  assert.equal(retryRequests.length, 2);
  socket.respond(retryRequests.at(-1));
  await settle();

  release();
  await settle();
  const unsubscribe = requests(socket, "session/unsubscribe", "session-retry").at(-1);
  socket.respond(unsubscribe);
  await settle();
  client.disconnect();
});

test("rejected session unsubscriptions remain pending and retry", async () => {
  const client = new WebSocketRpcClient();
  const release = client.subscribeSession("session-unsubscribe-retry", () => {}, { profileId: "profile-1" });
  const socket = MockWebSocket.instances.at(-1);
  socket.open();
  await settle();

  const subscribe = requests(socket, "session/subscribe", "session-unsubscribe-retry").at(-1);
  socket.respond(subscribe);
  await settle();

  release();
  await settle();
  const initialUnsubscribe = requests(socket, "session/unsubscribe", "session-unsubscribe-retry").at(-1);
  socket.respondError(initialUnsubscribe);
  await settle();

  assert.equal(requests(socket, "session/unsubscribe", "session-unsubscribe-retry").length, 1);
  await new Promise((resolve) => setTimeout(resolve, 600));

  const retryRequests = requests(socket, "session/unsubscribe", "session-unsubscribe-retry");
  assert.equal(retryRequests.length, 2);
  socket.respond(retryRequests.at(-1));
  await settle();
  client.disconnect();
});
