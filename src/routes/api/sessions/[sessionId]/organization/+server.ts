import { json } from "@sveltejs/kit";

import { codexGateway } from "$lib/server/gateway";

export async function PATCH({ params, request }) {
  const body = (await request.json().catch(() => ({}))) as {
    pinned?: boolean;
    tags?: string[];
  };

  return json(
    await codexGateway.updateSessionOrganization(params.sessionId, {
      pinned: typeof body.pinned === "boolean" ? body.pinned : undefined,
      tags: Array.isArray(body.tags) ? body.tags : undefined
    })
  );
}
