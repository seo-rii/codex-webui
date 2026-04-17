import { json } from "@sveltejs/kit";

import type { NotificationSettings } from "$lib/types";
import { codexGateway } from "$lib/server/gateway";

export async function PATCH({ request }) {
  const body = (await request.json().catch(() => ({}))) as Partial<NotificationSettings>;
  return json(await codexGateway.updateNotificationSettings(body));
}
