import { json } from "@sveltejs/kit";

import { codexGateway } from "$lib/server/gateway";

export async function GET({ params }) {
  return json(await codexGateway.getSessionItemDetail(params.sessionId, params.turnId, params.itemId));
}
