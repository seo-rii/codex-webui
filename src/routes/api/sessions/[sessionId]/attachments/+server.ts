import { error, json } from "@sveltejs/kit";

import { listAttachments, saveUploads } from "$lib/server/attachments";
import { codexGateway } from "$lib/server/gateway";

export async function GET({ params }) {
  return json({ attachments: await listAttachments(params.sessionId) });
}

export async function POST({ params, request }) {
  const formData = await request.formData();
  const files = formData
    .getAll("files")
    .filter((value): value is File => value instanceof File && value.size > 0);

  if (files.length === 0) {
    throw error(400, "Select at least one file.");
  }

  const attachments = await saveUploads(params.sessionId, files);
  await codexGateway.notifyAttachmentsUpdated(params.sessionId);
  return json({ attachments }, { status: 201 });
}
