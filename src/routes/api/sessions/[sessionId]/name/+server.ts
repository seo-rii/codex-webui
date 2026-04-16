import { error, json } from "@sveltejs/kit";

import { codexGateway } from "$lib/server/gateway";

export async function POST({ params, request }) {
  const body = (await request.json().catch(() => ({}))) as { name?: string };
  const name = body.name?.trim() ?? "";
  if (!name) {
    throw error(400, "Session name is required.");
  }
  await codexGateway.renameSession(params.sessionId, name);
  return json({ ok: true });
}
