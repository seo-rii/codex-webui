import { json } from "@sveltejs/kit";

import { parseAppError } from "$lib/errors";
import { throwRouteError } from "$lib/server/errors";
import { codexGateway } from "$lib/server/gateway";

export async function POST({ params }) {
  try {
    return json(await codexGateway.recoverSessionHistory(params.sessionId));
  } catch (error) {
    const parsed = parseAppError(error);
    if (parsed?.code === "SESSION_ROLLOUT_NOT_FOUND") {
      throwRouteError(404, "SESSION_ROLLOUT_NOT_FOUND", parsed.message);
    }
    if (parsed?.code === "SESSION_ROLLOUT_NOT_RECOVERABLE") {
      throwRouteError(409, "SESSION_ROLLOUT_NOT_RECOVERABLE", parsed.message);
    }
    throw error;
  }
}
