import { json } from "@sveltejs/kit";

import { codexGateway } from "$lib/server/gateway";

export async function GET({ url }) {
  const archived = url.searchParams.get("archived") === "true";
  const query = url.searchParams.get("query")?.trim() ?? "";
  const scope = url.searchParams.get("scope") === "full" ? "full" : "summary";
  const cursor = url.searchParams.get("cursor");
  const limit = Math.max(1, Math.min(100, Number(url.searchParams.get("limit") ?? 20) || 20));
  const payload = query
    ? await codexGateway.searchSessions(query, scope, archived, cursor, limit)
    : await codexGateway.listSessions(archived, cursor, limit);
  payload.sessions.sort((left, right) => right.updatedAt - left.updatedAt);
  return json(payload);
}

export async function POST({ request }) {
  const body = (await request.json().catch(() => ({}))) as {
    name?: string | null;
    preferences?: Record<string, unknown>;
  };
  const session = await codexGateway.createSession((body.preferences ?? {}) as never, body.name ?? null);
  return json(session, { status: 201 });
}
