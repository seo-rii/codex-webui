import { json } from "@sveltejs/kit";

import { codexGateway } from "$lib/server/gateway";
import type { PromptPreset } from "$lib/types";

export async function POST({ request }) {
  const body = (await request.json().catch(() => ({}))) as {
    preset?: PromptPreset;
  };

  return json(await codexGateway.savePromptPreset(body.preset as PromptPreset));
}

export async function DELETE({ url }) {
  const presetId = url.searchParams.get("presetId")?.trim() ?? "";
  return json(await codexGateway.deletePromptPreset(presetId));
}
