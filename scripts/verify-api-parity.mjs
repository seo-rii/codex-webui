import fs from "node:fs/promises";
import path from "node:path";
import process from "node:process";

const projectRoot = process.cwd();
const frontendApiPath = path.join(projectRoot, "src", "lib", "api.ts");
const debugClientPath = path.join(projectRoot, "scripts", "ws-debug-client.mjs");
const backendMainPath = path.join(projectRoot, "backend", "src", "main.rs");

function extractAll(pattern, content) {
  return [...content.matchAll(pattern)].map((match) => match[1]);
}

async function main() {
  const [frontendApi, debugClient, backendMain] = await Promise.all([
    fs.readFile(frontendApiPath, "utf8"),
    fs.readFile(debugClientPath, "utf8"),
    fs.readFile(backendMainPath, "utf8")
  ]);

  const wsMethods = new Set([
    ...extractAll(/ws\.request(?:<[^>]+>)?\("([^"]+)"/gu, frontendApi)
  ]);

  const httpRoutes = new Set([
    ...extractAll(/apiPath\("([^"]+)"/gu, frontendApi).map((pathname) => `/api${pathname}`),
    ...extractAll(/buildUrl\("([^"]+)"/gu, debugClient).filter((pathname) => pathname.startsWith("/api/"))
  ]);

  const missingWsMethods = [...wsMethods].filter((method) => !backendMain.includes(`"${method}"`)).sort();
  const missingHttpRoutes = [...httpRoutes].filter((route) => !backendMain.includes(`"${route}"`)).sort();

  if (missingWsMethods.length || missingHttpRoutes.length) {
    const lines = [
      missingWsMethods.length
        ? `Missing WS methods:\n${missingWsMethods.map((value) => `- ${value}`).join("\n")}`
        : null,
      missingHttpRoutes.length
        ? `Missing HTTP routes:\n${missingHttpRoutes.map((value) => `- ${value}`).join("\n")}`
        : null
    ].filter(Boolean);

    throw new Error(lines.join("\n\n"));
  }

  console.log(`API parity verification passed for ${wsMethods.size} WS methods and ${httpRoutes.size} HTTP routes.`);
}

await main();
