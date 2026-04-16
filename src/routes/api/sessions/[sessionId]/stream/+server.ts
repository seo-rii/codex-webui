import { codexGateway } from "$lib/server/gateway";

const encoder = new TextEncoder();

function encodeEvent(event: string, payload: unknown) {
  return encoder.encode(`event: ${event}\ndata: ${JSON.stringify(payload)}\n\n`);
}

export function GET({ params }) {
  let unsubscribe = () => {};
  let keepAlive: ReturnType<typeof setInterval> | null = null;

  const stream = new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(encodeEvent("ready", { threadId: params.sessionId }));
      unsubscribe = codexGateway.subscribe(params.sessionId, (payload) => {
        controller.enqueue(encodeEvent("message", payload));
      });
      keepAlive = setInterval(() => {
        controller.enqueue(encoder.encode(": ping\n\n"));
      }, 15_000);
    },
    cancel() {
      unsubscribe();
      if (keepAlive) {
        clearInterval(keepAlive);
      }
    }
  });

  return new Response(stream, {
    headers: {
      "cache-control": "no-cache, no-transform",
      connection: "keep-alive",
      "content-type": "text/event-stream",
      "x-accel-buffering": "no"
    }
  });
}
