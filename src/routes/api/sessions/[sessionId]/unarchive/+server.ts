import { json } from "@sveltejs/kit";

import { codexGateway } from "$lib/server/gateway";

export async function POST({ params }) {
  return json(await codexGateway.unarchiveSession(params.sessionId));
}
