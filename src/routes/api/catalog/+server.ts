import { json } from "@sveltejs/kit";

import { getCatalog } from "$lib/server/catalog";

export async function GET() {
  return json(await getCatalog());
}
