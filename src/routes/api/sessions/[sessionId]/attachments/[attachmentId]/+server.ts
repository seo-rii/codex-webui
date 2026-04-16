import { json } from "@sveltejs/kit";

import { removeAttachment } from "$lib/server/attachments";
import { codexGateway } from "$lib/server/gateway";

export async function DELETE({ params }) {
  await removeAttachment(params.sessionId, params.attachmentId);
  await codexGateway.notifyAttachmentsUpdated(params.sessionId);
  return json({ ok: true });
}
