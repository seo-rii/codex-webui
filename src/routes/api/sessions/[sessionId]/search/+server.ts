import { json } from "@sveltejs/kit";

import { codexGateway } from "$lib/server/gateway";

export async function GET({ params, url }) {
  const query = url.searchParams.get("query")?.trim();
  if (!query) {
    return json({ message: "query is required." }, { status: 400 });
  }

  const cursor = url.searchParams.get("cursor")?.trim() || null;
  const limit = Math.max(1, Math.min(100, Number(url.searchParams.get("limit") ?? 20) || 20));
  return json(await codexGateway.searchSessionTurns(params.sessionId, query, cursor, limit));
}
