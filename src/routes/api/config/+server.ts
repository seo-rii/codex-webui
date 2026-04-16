import { json } from "@sveltejs/kit";

import { codexGateway } from "$lib/server/gateway";

export async function GET() {
  return json(await codexGateway.getConfig());
}
