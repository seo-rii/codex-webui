import { json } from "@sveltejs/kit";

import { listGitRepositories } from "$lib/server/git";

export async function GET() {
  return json({ repositories: await listGitRepositories(true) });
}
