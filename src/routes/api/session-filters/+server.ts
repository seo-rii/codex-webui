import { json } from "@sveltejs/kit";

import { codexGateway } from "$lib/server/gateway";
import type { SavedSessionFilter } from "$lib/types";

export async function POST({ request }) {
  const body = (await request.json().catch(() => ({}))) as {
    filter?: SavedSessionFilter;
  };

  return json(await codexGateway.saveSessionFilter(body.filter as SavedSessionFilter));
}

export async function DELETE({ url }) {
  const filterId = url.searchParams.get("filterId")?.trim() ?? "";
  return json(await codexGateway.deleteSessionFilter(filterId));
}
