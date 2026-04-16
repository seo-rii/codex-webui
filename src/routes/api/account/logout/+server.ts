import { json } from "@sveltejs/kit";

import { codexGateway } from "$lib/server/gateway";

export async function POST() {
  return json(await codexGateway.logoutAccount());
}
