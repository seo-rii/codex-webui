import { json } from "@sveltejs/kit";

import { readEditableFile, writeEditableFile } from "$lib/server/editor";

export async function GET({ url }) {
  return json(await readEditableFile(url.searchParams.get("filePath") ?? ""));
}

export async function PUT({ request }) {
  const body = (await request.json().catch(() => ({}))) as {
    filePath?: string;
    content?: string;
  };
  return json(await writeEditableFile(body.filePath ?? "", body.content ?? ""));
}
