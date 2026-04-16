import { json } from "@sveltejs/kit";

import { codexGateway } from "$lib/server/gateway";

export async function GET({ params }) {
  return json(await codexGateway.getDraft(params.sessionId));
}

export async function PATCH({ params, request }) {
  const body = (await request.json().catch(() => ({}))) as {
    draft?: string;
    intent?: "message" | "steer";
  };

  return json(await codexGateway.saveDraft(params.sessionId, body.draft ?? "", body.intent ?? "message"));
}

export async function DELETE({ params }) {
  return json(await codexGateway.clearDraft(params.sessionId));
}
