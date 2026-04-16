import { error, json } from "@sveltejs/kit";

import { resolveGitFileFromAbsolutePath } from "$lib/server/git";

export async function GET({ url }) {
  const filePath = url.searchParams.get("filePath");
  if (!filePath) {
    throw error(400, "filePath is required.");
  }

  return json(await resolveGitFileFromAbsolutePath(filePath));
}
