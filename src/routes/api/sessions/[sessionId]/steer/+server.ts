import { json } from "@sveltejs/kit";

import { listAttachments } from "$lib/server/attachments";
import { throwRouteError } from "$lib/server/errors";
import { codexGateway } from "$lib/server/gateway";

export async function POST({ params, request }) {
  const body = (await request.json().catch(() => ({}))) as {
    prompt?: string;
    attachmentIds?: string[];
  };

  const prompt = body.prompt?.trim() ?? "";
  if (!prompt) {
    throwRouteError(400, "EMPTY_MESSAGE");
  }

  const attachmentIds = Array.isArray(body.attachmentIds) ? body.attachmentIds : [];
  const attachments = (await listAttachments(params.sessionId)).filter((attachment) => attachmentIds.includes(attachment.id));
  await codexGateway.steer(params.sessionId, prompt, attachments);
  return json({ ok: true });
}
