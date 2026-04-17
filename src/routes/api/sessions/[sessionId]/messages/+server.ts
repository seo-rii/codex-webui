import { json } from "@sveltejs/kit";

import { listAttachments } from "$lib/server/attachments";
import { throwRouteError } from "$lib/server/errors";
import { codexGateway } from "$lib/server/gateway";

export async function POST({ params, request }) {
  const body = (await request.json().catch(() => ({}))) as {
    prompt?: string;
    attachmentIds?: string[];
    preferences?: Record<string, unknown>;
  };

  const prompt = body.prompt?.trim() ?? "";
  const attachmentIds = Array.isArray(body.attachmentIds) ? body.attachmentIds : [];

  if (!prompt && attachmentIds.length === 0) {
    throwRouteError(400, "EMPTY_MESSAGE");
  }

  const attachments = (await listAttachments(params.sessionId)).filter((attachment) => attachmentIds.includes(attachment.id));

  await codexGateway.sendMessage(params.sessionId, prompt, attachments, (body.preferences ?? {}) as never);
  return json({ ok: true });
}
