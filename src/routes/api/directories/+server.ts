import { json } from "@sveltejs/kit";

import { codexGateway } from "$lib/server/gateway";

export async function GET({ url }) {
  const currentPath = url.searchParams.get("path");
  return json(await codexGateway.listDirectories(currentPath));
}
