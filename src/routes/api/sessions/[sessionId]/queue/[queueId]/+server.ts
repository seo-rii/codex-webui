import { json } from "@sveltejs/kit";

import { listAttachments } from "$lib/server/attachments";
import { throwRouteError } from "$lib/server/errors";
import { codexGateway } from "$lib/server/gateway";

export async function POST({ params, request }) {
  const body = (await request.json().catch(() => ({}))) as {
    mode?: "message" | "steer";
  };

  const mode = body.mode === "steer" ? "steer" : body.mode === "message" ? "message" : null;
  if (!mode) {
    throwRouteError(400, "INVALID_QUEUE_MODE");
  }

  return json(await codexGateway.dispatchQueuedMessage(params.sessionId, params.queueId, mode));
}

export async function PATCH({ params, request }) {
  const body = (await request.json().catch(() => ({}))) as {
    prompt?: string;
    attachmentIds?: string[];
  };

  const queue = await codexGateway.getQueue(params.sessionId);
  const queuedItem = queue.items.find((item) => item.id === params.queueId);
  if (!queuedItem) {
    throwRouteError(404, "QUEUE_ITEM_NOT_FOUND");
  }

  const attachmentIds = Array.isArray(body.attachmentIds) ? body.attachmentIds : queuedItem.attachmentIds;
  const attachments = (await listAttachments(params.sessionId)).filter((attachment) => attachmentIds.includes(attachment.id));
  const prompt = typeof body.prompt === "string" ? body.prompt : queuedItem.prompt;
  if (!prompt.trim() && attachments.length === 0) {
    throwRouteError(400, "EMPTY_MESSAGE");
  }

  return json(await codexGateway.updateQueuedMessage(params.sessionId, params.queueId, prompt, attachments));
}

export async function DELETE({ params }) {
  return json(await codexGateway.removeQueuedMessage(params.sessionId, params.queueId));
}
