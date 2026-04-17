import { error, json } from "@sveltejs/kit";

import { fetchGitRepository } from "$lib/server/git";

export async function POST({ request }) {
  const body = (await request.json().catch(() => ({}))) as {
    repoPath?: string;
  };

  if (!body.repoPath) {
    throw error(400, "repoPath is required.");
  }

  return json(await fetchGitRepository(body.repoPath));
}
