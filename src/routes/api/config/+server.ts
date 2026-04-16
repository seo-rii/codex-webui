import { json } from "@sveltejs/kit";

import { codexGateway } from "$lib/server/gateway";

export async function GET() {
  return json(await codexGateway.getConfig());
}

export async function PATCH({ request }) {
  const body = (await request.json().catch(() => ({}))) as {
    systemShutdown?: {
      armed?: boolean;
    };
  };

  if (typeof body.systemShutdown?.armed !== "boolean") {
    return json(await codexGateway.getConfig());
  }

  return json(await codexGateway.saveSystemShutdownAfterQueueCompletes(body.systemShutdown.armed));
}
