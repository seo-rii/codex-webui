import { error, json } from "@sveltejs/kit";

import { codexGateway } from "$lib/server/gateway";

export async function POST({ params, request }) {
  const body = (await request.json().catch(() => ({}))) as {
    requestId?: string;
    result?: unknown;
  };

  if (!body.requestId) {
    throw error(400, "requestId is required.");
  }

  await codexGateway.resolveServerRequest(params.sessionId, body.requestId, body.result ?? {});
  return json({ ok: true });
}
