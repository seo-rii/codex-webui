import { json } from "@sveltejs/kit";

import { codexGateway } from "$lib/server/gateway";

export async function POST({ params, request }) {
  const body = (await request.json().catch(() => ({}))) as {
    trigger?: "manual" | "schedule";
  };

  return json(await codexGateway.runAutomation(params.automationId, body.trigger === "schedule" ? "schedule" : "manual"));
}
