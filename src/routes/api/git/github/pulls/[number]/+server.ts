import { error, json } from "@sveltejs/kit";

import { getGitHubPullRequest } from "$lib/server/github";

export async function GET({ params, url }) {
  const repoPath = url.searchParams.get("repoPath");
  const number = Number(params.number);
  if (!repoPath || !Number.isFinite(number)) {
    throw error(400, "repoPath and pull request number are required.");
  }

  return json(await getGitHubPullRequest(repoPath, number));
}
