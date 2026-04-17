import { json } from "@sveltejs/kit";

import { codexGateway } from "$lib/server/gateway";
import type { AutomationDefinition } from "$lib/types";

export async function POST({ request }) {
  const body = (await request.json().catch(() => ({}))) as {
    automation?: AutomationDefinition;
  };

  return json(await codexGateway.saveAutomation(body.automation as AutomationDefinition));
}

export async function DELETE({ url }) {
  const automationId = url.searchParams.get("automationId")?.trim() ?? "";
  return json(await codexGateway.deleteAutomation(automationId));
}
