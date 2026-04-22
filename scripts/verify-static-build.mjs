import fs from "node:fs/promises";
import path from "node:path";
import process from "node:process";

const projectRoot = process.cwd();
const buildStaticDir = path.join(projectRoot, "build", "static");
const buildInternalEntry = path.join(projectRoot, "build", "internal", "index.js");
const svelteEndpointDir = path.join(projectRoot, ".svelte-kit", "output", "server", "entries", "endpoints");

async function exists(targetPath) {
  try {
    await fs.access(targetPath);
    return true;
  } catch {
    return false;
  }
}

async function assertFileContainsNoPlaceholders(targetPath) {
  const content = await fs.readFile(targetPath, "utf8");
  if (content.includes("%lang%") || content.includes("%dir%")) {
    throw new Error(`${path.relative(projectRoot, targetPath)} still contains unresolved html placeholders`);
  }
}

async function main() {
  if (!(await exists(buildStaticDir))) {
    throw new Error("build/static is missing. Run `pnpm build` first.");
  }

  for (const htmlName of ["index.html", "200.html"]) {
    const htmlPath = path.join(buildStaticDir, htmlName);
    if (await exists(htmlPath)) {
      await assertFileContainsNoPlaceholders(htmlPath);
    }
  }

  if (await exists(path.join(buildStaticDir, "login.html"))) {
    throw new Error("deprecated build/static/login.html still exists");
  }

  if (await exists(buildInternalEntry)) {
    throw new Error("legacy build/internal/index.js still exists");
  }

  if (await exists(svelteEndpointDir)) {
    const entries = await fs.readdir(svelteEndpointDir);
    if (entries.length > 0) {
      throw new Error("SvelteKit endpoint output still exists under .svelte-kit/output/server/entries/endpoints");
    }
  }

  console.log("Static build verification passed.");
}

await main();
