import { error, json } from "@sveltejs/kit";

import { unstageGitChanges } from "$lib/server/git";

export async function POST({ request }) {
  const body = (await request.json().catch(() => ({}))) as {
    repoPath?: string;
    filePath?: string | null;
  };

  if (!body.repoPath) {
    throw error(400, "repoPath is required.");
  }

  return json(await unstageGitChanges(body.repoPath, body.filePath ?? null));
}
