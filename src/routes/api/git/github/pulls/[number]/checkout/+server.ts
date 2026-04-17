import { error, json } from "@sveltejs/kit";

import { checkoutGitHubPullRequest } from "$lib/server/github";

export async function POST({ params, request }) {
  const body = (await request.json().catch(() => ({}))) as {
    repoPath?: string;
  };
  const number = Number(params.number);

  if (!body.repoPath || !Number.isFinite(number)) {
    throw error(400, "repoPath and pull request number are required.");
  }

  return json(await checkoutGitHubPullRequest(body.repoPath, number));
}
