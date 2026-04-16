import { json } from "@sveltejs/kit";

import { codexGateway } from "$lib/server/gateway";

export async function POST({ request }) {
  const body = (await request.json().catch(() => ({}))) as {
    loginId?: string;
  };

  if (!body.loginId) {
    throw new Error("loginId is required.");
  }

  return json(await codexGateway.cancelAccountLogin(body.loginId));
}
