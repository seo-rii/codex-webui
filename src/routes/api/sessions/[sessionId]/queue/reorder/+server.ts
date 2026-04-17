import { json } from "@sveltejs/kit";

import { throwRouteError } from "$lib/server/errors";
import { codexGateway } from "$lib/server/gateway";

export async function POST({ params, request }) {
  const body = (await request.json().catch(() => ({}))) as {
    queueIds?: string[];
  };

  const queueIds = Array.isArray(body.queueIds) ? body.queueIds.filter((entry): entry is string => typeof entry === "string") : [];
  if (queueIds.length === 0) {
    throwRouteError(400, "QUEUE_ITEM_NOT_FOUND");
  }

  return json(await codexGateway.reorderQueuedMessages(params.sessionId, queueIds));
}
