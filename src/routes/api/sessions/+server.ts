import { json } from "@sveltejs/kit";

import { codexGateway } from "$lib/server/gateway";
import type { SessionSummaryFilter } from "$lib/types";

export async function GET({ url }) {
  const archived = url.searchParams.get("archived") === "true";
  const query = url.searchParams.get("query")?.trim() ?? "";
  const scope = url.searchParams.get("scope") === "full" ? "full" : "summary";
  const cursor = url.searchParams.get("cursor");
  const limit = Math.max(1, Math.min(100, Number(url.searchParams.get("limit") ?? 20) || 20));
  const filter: SessionSummaryFilter | null =
    url.searchParams.has("filterPinned") ||
    url.searchParams.has("filterRunning") ||
    url.searchParams.has("filterQueued") ||
    url.searchParams.has("filterHighlight") ||
    url.searchParams.has("filterTag")
      ? {
          pinnedOnly: url.searchParams.get("filterPinned") === "true",
          runningOnly: url.searchParams.get("filterRunning") === "true",
          queuedOnly: url.searchParams.get("filterQueued") === "true",
          highlight:
            url.searchParams.get("filterHighlight") === "attention" || url.searchParams.get("filterHighlight") === "completed"
              ? (url.searchParams.get("filterHighlight") as SessionSummaryFilter["highlight"])
              : "all",
          tags: url.searchParams
            .getAll("filterTag")
            .map((entry) => entry.trim())
            .filter((entry) => entry.length > 0)
        }
      : null;
  const payload = query
    ? await codexGateway.searchSessions(query, scope, archived, cursor, limit, filter)
    : await codexGateway.listSessions(archived, cursor, limit, filter);
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
