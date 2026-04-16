import { error, json } from "@sveltejs/kit";

import { checkoutGitBranch } from "$lib/server/git";

export async function POST({ request }) {
  const body = (await request.json().catch(() => ({}))) as {
    repoPath?: string;
    branchName?: string;
    create?: boolean;
  };

  if (!body.repoPath || !body.branchName) {
    throw error(400, "repoPath and branchName are required.");
  }

  return json(await checkoutGitBranch(body.repoPath, body.branchName, body.create === true));
}
