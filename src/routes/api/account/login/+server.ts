import { json } from "@sveltejs/kit";

import { codexGateway } from "$lib/server/gateway";

export async function POST({ request }) {
  const body = (await request.json().catch(() => ({}))) as {
    type?: "chatgpt" | "chatgptDeviceCode" | "apiKey";
    apiKey?: string | null;
  };

  const type = body.type;
  if (type !== "chatgpt" && type !== "chatgptDeviceCode" && type !== "apiKey") {
    throw new Error("Invalid account login type.");
  }

  return json(await codexGateway.startAccountLogin(type, body.apiKey ?? null));
}
