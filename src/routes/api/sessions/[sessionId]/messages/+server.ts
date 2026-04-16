import { error, json } from "@sveltejs/kit";

import { listAttachments } from "$lib/server/attachments";
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
    throw error(400, "Provide a prompt or at least one attachment.");
  }

  const attachments = (await listAttachments(params.sessionId)).filter((attachment) => attachmentIds.includes(attachment.id));

  await codexGateway.sendMessage(params.sessionId, prompt, attachments, (body.preferences ?? {}) as never);
  return json({ ok: true });
}
