import { json } from "@sveltejs/kit";

import { clearAuthCookie } from "$lib/server/auth";

export function POST({ cookies }) {
  clearAuthCookie(cookies);
  return json({ ok: true });
}
