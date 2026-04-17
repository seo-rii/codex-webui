import { error, json } from "@sveltejs/kit";

import { pullGitRepository } from "$lib/server/git";

export async function POST({ request }) {
  const body = (await request.json().catch(() => ({}))) as {
    repoPath?: string;
  };

  if (!body.repoPath) {
    throw error(400, "repoPath is required.");
  }

  return json(await pullGitRepository(body.repoPath));
}
