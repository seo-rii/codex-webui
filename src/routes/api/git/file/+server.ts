import { error, json } from "@sveltejs/kit";

import { getGitFile, saveGitFile } from "$lib/server/git";

export async function GET({ url }) {
  const repoPath = url.searchParams.get("repoPath");
  const filePath = url.searchParams.get("filePath");
  if (!repoPath || !filePath) {
    throw error(400, "repoPath and filePath are required.");
  }

  return json(await getGitFile(repoPath, filePath));
}

export async function PUT({ request }) {
  const body = (await request.json().catch(() => ({}))) as {
    repoPath?: string;
    filePath?: string;
    content?: string;
  };

  if (!body.repoPath || !body.filePath || typeof body.content !== "string") {
    throw error(400, "repoPath, filePath, and content are required.");
  }

  return json(await saveGitFile(body.repoPath, body.filePath, body.content));
}
