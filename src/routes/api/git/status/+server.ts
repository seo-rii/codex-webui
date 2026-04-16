import { error, json } from "@sveltejs/kit";

import { getGitStatus } from "$lib/server/git";

export async function GET({ url }) {
  const repoPath = url.searchParams.get("repoPath");
  if (!repoPath) {
    throw error(400, "repoPath is required.");
  }

  return json(await getGitStatus(repoPath));
}
