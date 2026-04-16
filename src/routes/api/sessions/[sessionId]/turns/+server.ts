import { json } from "@sveltejs/kit";

import { codexGateway } from "$lib/server/gateway";

export async function GET({ params, url }) {
  const beforeTurnId = url.searchParams.get("beforeTurnId")?.trim();
  if (!beforeTurnId) {
    return json({ message: "beforeTurnId is required." }, { status: 400 });
  }

  const limit = Math.max(1, Math.min(100, Number(url.searchParams.get("limit") ?? 20) || 20));
  return json(await codexGateway.getSessionOlderTurns(params.sessionId, beforeTurnId, limit));
}
