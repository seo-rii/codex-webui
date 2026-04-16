import { json } from "@sveltejs/kit";

import { createGitWorktree, listGitWorktrees, removeGitWorktree } from "$lib/server/git";

export async function GET({ url }) {
  const repoPath = url.searchParams.get("repoPath") ?? "";
  return json({
    repoPath,
    worktrees: await listGitWorktrees(repoPath)
  });
}

export async function POST({ request }) {
  const body = (await request.json().catch(() => ({}))) as {
    repoPath?: string;
    worktreePath?: string;
    branchName?: string | null;
    createBranch?: boolean;
    detach?: boolean;
  };
  return json(
    await createGitWorktree(
      body.repoPath ?? "",
      body.worktreePath ?? "",
      body.branchName ?? null,
      Boolean(body.createBranch),
      Boolean(body.detach)
    )
  );
}

export async function DELETE({ request }) {
  const body = (await request.json().catch(() => ({}))) as {
    repoPath?: string;
    worktreePath?: string;
    force?: boolean;
  };
  return json(await removeGitWorktree(body.repoPath ?? "", body.worktreePath ?? "", Boolean(body.force)));
}
