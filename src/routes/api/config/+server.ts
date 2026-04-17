import { json } from "@sveltejs/kit";

import { codexGateway } from "$lib/server/gateway";
import { normalizeThemeSettings } from "$lib/theme-customization";

export async function GET() {
  return json(await codexGateway.getConfig());
}

export async function PATCH({ request }) {
  const body = (await request.json().catch(() => ({}))) as {
    systemShutdown?: {
      armed?: boolean;
    };
    theme?: unknown;
  };

  if (body.theme) {
    return json(await codexGateway.saveThemeSettings(normalizeThemeSettings(body.theme)));
  }

  if (typeof body.systemShutdown?.armed === "boolean") {
    return json(await codexGateway.saveSystemShutdownAfterQueueCompletes(body.systemShutdown.armed));
  }

  return json(await codexGateway.getConfig());
}
