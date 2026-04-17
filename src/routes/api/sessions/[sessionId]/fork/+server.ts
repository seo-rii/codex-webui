import { json } from "@sveltejs/kit";

import { codexGateway } from "$lib/server/gateway";
import type { SessionForkMode } from "$lib/types";

export async function POST({ params, request }) {
  const body = (await request.json().catch(() => ({}))) as {
    mode?: SessionForkMode;
    turnId?: string | null;
    messageText?: string | null;
  };

  return json(
    await codexGateway.forkSession(params.sessionId, {
      mode: body.mode === "handoff" ? "handoff" : "fork",
      turnId: body.turnId ?? null,
      messageText: body.messageText ?? null
    }),
    { status: 201 }
  );
}
