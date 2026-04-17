import { error, json } from "@sveltejs/kit";

import { codexGateway } from "$lib/server/gateway";

export async function GET() {
  return json(await codexGateway.listArenaRuns());
}

export async function POST({ request }) {
  const body = (await request.json().catch(() => ({}))) as {
    prompt?: string;
    contestants?: Array<{
      model?: string;
      label?: string;
    }>;
    preferences?: Record<string, unknown>;
  };

  if (!body.prompt?.trim() || !Array.isArray(body.contestants)) {
    throw error(400, "prompt and contestants are required.");
  }

  return json(await codexGateway.startArenaRun(body.prompt, body.contestants, (body.preferences ?? {}) as never), { status: 201 });
}
