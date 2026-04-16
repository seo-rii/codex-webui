import { error, json } from "@sveltejs/kit";

import { stageGitChanges } from "$lib/server/git";

export async function POST({ request }) {
  const body = (await request.json().catch(() => ({}))) as {
    repoPath?: string;
    filePath?: string | null;
  };

  if (!body.repoPath) {
    throw error(400, "repoPath is required.");
  }

  return json(await stageGitChanges(body.repoPath, body.filePath ?? null));
}
