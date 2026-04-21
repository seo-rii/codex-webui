import fs from "node:fs/promises";
import path from "node:path";
import process from "node:process";

const projectRoot = process.cwd();
const frontendApiPath = path.join(projectRoot, "src", "lib", "api.ts");
const debugClientPath = path.join(projectRoot, "scripts", "ws-debug-client.mjs");
const wsClientPath = path.join(projectRoot, "src", "lib", "ws-client.ts");
const backendMainPath = path.join(projectRoot, "backend", "src", "main.rs");
const INTERPOLATION_TOKEN = "__SEGMENT__";

function extractAll(pattern, content) {
  return [...content.matchAll(pattern)].map((match) => match[1]);
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
  const [frontendApi, debugClient, wsClient, backendMain] = await Promise.all([
    fs.readFile(frontendApiPath, "utf8"),
    fs.readFile(debugClientPath, "utf8"),
    fs.readFile(wsClientPath, "utf8"),
    fs.readFile(backendMainPath, "utf8")
  ]);

  const wsMethods = new Set([
    ...extractAll(/ws\.request(?:<[^>]+>)?\("([^"]+)"/gu, frontendApi)
  ]);

  const httpRoutes = new Set(
    [
    ...extractAll(/apiPath\("([^"]+)"/gu, frontendApi).map((pathname) => `/api${pathname}`),
      ...extractAll(/apiPath\(`([^`]+)`/gu, frontendApi).map((pathname) => `/api${normalizeTemplatePath(pathname)}`),
      ...extractAll(/buildUrl\("([^"]+)"/gu, debugClient).filter((pathname) => pathname.startsWith("/api/")),
      ...extractAll(/buildUrl\(`([^`]+)`/gu, debugClient)
        .map(normalizeTemplatePath)
        .filter((pathname) => pathname.startsWith("/api/"))
    ].sort()
  );

  const websocketRoutes = new Set([
    ...extractAll(/appPath\("([^"]+)"/gu, wsClient).filter((pathname) => pathname === "/ws")
  ]);

  const missingWsMethods = [...wsMethods].filter((method) => !backendMain.includes(`"${method}"`)).sort();
  const missingHttpRoutes = [...httpRoutes].filter((route) => !backendSupportsTemplate(backendMain, route)).sort();
  const missingWebSocketRoutes = [...websocketRoutes]
    .filter((route) => !backendSupportsTemplate(backendMain, route))
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
