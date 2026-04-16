import { error, json } from "@sveltejs/kit";

import { getGitCommitDiff } from "$lib/server/git";

export async function GET({ url }) {
  const repoPath = url.searchParams.get("repoPath");
  const commitHash = url.searchParams.get("commitHash");
  if (!repoPath || !commitHash) {
    throw error(400, "repoPath and commitHash are required.");
  }

  return json(await getGitCommitDiff(repoPath, commitHash));
}
