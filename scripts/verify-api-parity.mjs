import fs from "node:fs/promises";
import path from "node:path";
import process from "node:process";

const projectRoot = process.cwd();
const frontendSourceRoot = path.join(projectRoot, "src");
const debugClientPath = path.join(projectRoot, "scripts", "ws-debug-client.mjs");
const backendSourceRoot = path.join(projectRoot, "backend", "src");
const INTERPOLATION_TOKEN = "__SEGMENT__";
const FRONTEND_SOURCE_FILE_EXTENSIONS = new Set([".ts", ".js", ".mjs", ".svelte"]);
const BACKEND_SOURCE_FILE_EXTENSIONS = new Set([".rs"]);

function extractAll(pattern, content) {
  return [...content.matchAll(pattern)].map((match) => match[1]);
}

async function collectSourceFiles(rootPath, extensions) {
  const entries = await fs.readdir(rootPath, { withFileTypes: true });
  const files = [];

  for (const entry of entries) {
    const entryPath = path.join(rootPath, entry.name);
    if (entry.isDirectory()) {
      files.push(...(await collectSourceFiles(entryPath, extensions)));
      continue;
    }

    if (entry.isFile() && extensions.has(path.extname(entry.name))) {
      files.push(entryPath);
    }
  }

  return files;
}

function normalizeTemplatePath(value) {
  return value.replace(/\$\{[^}]+\}/gu, INTERPOLATION_TOKEN);
}

function escapeRegex(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&");
}

function backendSupportsTemplate(content, template) {
  if (!template.includes(INTERPOLATION_TOKEN)) {
    return content.includes(`"${template}"`);
  }

  const parts = template
    .split(INTERPOLATION_TOKEN)
    .map((part) => part.trim())
    .filter(Boolean);

  if (parts.length === 0) {
    return false;
  }

  const pattern = new RegExp(parts.map((part) => escapeRegex(part)).join("[\\s\\S]*"), "u");
  return pattern.test(content);
}

async function main() {
  const frontendFiles = await collectSourceFiles(frontendSourceRoot, FRONTEND_SOURCE_FILE_EXTENSIONS);
  const backendFiles = await collectSourceFiles(backendSourceRoot, BACKEND_SOURCE_FILE_EXTENSIONS);
  const frontendSources = await Promise.all(frontendFiles.map((filePath) => fs.readFile(filePath, "utf8")));
  const backendSources = await Promise.all(backendFiles.map((filePath) => fs.readFile(filePath, "utf8")));
  const debugClient = await fs.readFile(debugClientPath, "utf8");
  const backendContent = backendSources.join("\n");
  const allFrontendContent = frontendSources.join("\n");
  const allSourceContent = `${allFrontendContent}\n${debugClient}`;

  const wsMethods = new Set([
    ...extractAll(/ws\.request(?:<[^>]+>)?\(\s*["']([^"']+)["']/gu, allSourceContent),
    ...extractAll(/ws\.request(?:<[^>]+>)?\(\s*`([^`]+)`/gu, allSourceContent)
  ]);

  const httpRoutes = new Set([
    ...extractAll(/apiPath\(\s*["']([^"']+)["']/gu, allSourceContent).map((pathname) => `/api${pathname}`),
    ...extractAll(/apiPath\(\s*`([^`]+)`/gu, allSourceContent).map((pathname) => `/api${normalizeTemplatePath(pathname)}`),
    ...extractAll(/buildUrl\(\s*["']([^"']+)["']/gu, allSourceContent).filter((pathname) => pathname.startsWith("/api/")),
    ...extractAll(/buildUrl\(\s*`([^`]+)`/gu, allSourceContent)
      .map(normalizeTemplatePath)
      .filter((pathname) => pathname.startsWith("/api/"))
  ]);

  const websocketRoutes = new Set([
    ...extractAll(/appPath\(\s*["']([^"']+)["']/gu, allSourceContent).filter((pathname) => pathname === "/ws"),
    ...extractAll(/appPath\(\s*`([^`]+)`/gu, allSourceContent)
      .map(normalizeTemplatePath)
      .filter((pathname) => pathname === "/ws")
  ]);

  const missingWsMethods = [...wsMethods].filter((method) => !backendContent.includes(`"${method}"`)).sort();
  const missingHttpRoutes = [...httpRoutes].filter((route) => !backendSupportsTemplate(backendContent, route)).sort();
  const missingWebSocketRoutes = [...websocketRoutes]
    .filter((route) => !backendSupportsTemplate(backendContent, route))
    .sort();

  if (missingWsMethods.length || missingHttpRoutes.length || missingWebSocketRoutes.length) {
    const lines = [
      missingWsMethods.length
        ? `Missing WS methods:\n${missingWsMethods.map((value) => `- ${value}`).join("\n")}`
        : null,
      missingHttpRoutes.length
        ? `Missing HTTP routes:\n${missingHttpRoutes.map((value) => `- ${value}`).join("\n")}`
        : null,
      missingWebSocketRoutes.length
        ? `Missing WebSocket routes:\n${missingWebSocketRoutes.map((value) => `- ${value}`).join("\n")}`
        : null
    ].filter(Boolean);

    throw new Error(lines.join("\n\n"));
  }

  console.log(
    `API parity verification passed for ${wsMethods.size} WS methods, ${httpRoutes.size} HTTP routes, and ${websocketRoutes.size} WebSocket routes.`
  );
}

await main();
