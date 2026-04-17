import { json } from "@sveltejs/kit";

import { codexGateway } from "$lib/server/gateway";

export async function GET({ url }) {
  const limit = Math.max(1, Math.min(200, Number(url.searchParams.get("limit") ?? 80) || 80));
  return json(await codexGateway.getNotifications(limit));
}

export async function PATCH({ request }) {
  const body = (await request.json().catch(() => ({}))) as {
    ids?: string[] | null;
  };
  return json(await codexGateway.markNotificationsRead(Array.isArray(body.ids) ? body.ids : null));
}

export async function DELETE() {
  return json(await codexGateway.clearNotifications());
}
