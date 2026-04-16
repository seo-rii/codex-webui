import { error, json } from "@sveltejs/kit";

import { commitGitChanges } from "$lib/server/git";

export async function POST({ request }) {
  const body = (await request.json().catch(() => ({}))) as {
    repoPath?: string;
    message?: string;
  };

  if (!body.repoPath || typeof body.message !== "string") {
    throw error(400, "repoPath and message are required.");
  }

  return json(await commitGitChanges(body.repoPath, body.message));
}
