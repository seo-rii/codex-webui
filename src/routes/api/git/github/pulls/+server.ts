import { error, json } from "@sveltejs/kit";

import { listGitHubPullRequests } from "$lib/server/github";

export async function GET({ url }) {
  const repoPath = url.searchParams.get("repoPath");
  if (!repoPath) {
    throw error(400, "repoPath is required.");
  }

  const state = url.searchParams.get("state");
  const limit = Number(url.searchParams.get("limit") ?? "20");
  return json(
    await listGitHubPullRequests(
      repoPath,
      state === "closed" || state === "all" ? state : "open",
      Number.isFinite(limit) ? limit : 20
    )
  );
}
