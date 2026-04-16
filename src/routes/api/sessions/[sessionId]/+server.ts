import { json } from "@sveltejs/kit";

import { codexGateway } from "$lib/server/gateway";

export async function GET({ params, url }) {
  const limit = Math.max(1, Math.min(100, Number(url.searchParams.get("limit") ?? 20) || 20));
  return json(await codexGateway.getSession(params.sessionId, limit));
}

export async function PATCH({ params, request }) {
  const body = (await request.json().catch(() => ({}))) as {
    preferences?: Record<string, unknown>;
  };
  return json(await codexGateway.savePreferences(params.sessionId, (body.preferences ?? {}) as never));
}
