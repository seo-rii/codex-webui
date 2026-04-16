import { error, json } from "@sveltejs/kit";

import { listAttachments } from "$lib/server/attachments";
import { codexGateway } from "$lib/server/gateway";

export async function POST({ params, request }) {
  const body = (await request.json().catch(() => ({}))) as {
    mode?: "message" | "steer";
  };

  const mode = body.mode === "steer" ? "steer" : body.mode === "message" ? "message" : null;
  if (!mode) {
    throw error(400, "mode is required.");
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
    throw error(404, "Queued message not found.");
  }

  const attachmentIds = Array.isArray(body.attachmentIds) ? body.attachmentIds : queuedItem.attachmentIds;
  const attachments = (await listAttachments(params.sessionId)).filter((attachment) => attachmentIds.includes(attachment.id));
  const prompt = typeof body.prompt === "string" ? body.prompt : queuedItem.prompt;
  if (!prompt.trim() && attachments.length === 0) {
    throw error(400, "Provide a prompt or at least one attachment.");
  }

  return json(await codexGateway.updateQueuedMessage(params.sessionId, params.queueId, prompt, attachments));
}

export async function DELETE({ params }) {
  return json(await codexGateway.removeQueuedMessage(params.sessionId, params.queueId));
}
