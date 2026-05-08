import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const codexRoot = path.resolve(process.env.CODEX_REPO_PATH ?? path.join(repoRoot, "..", "codex"));
const slashCommandPath = path.join(codexRoot, "codex-rs", "tui", "src", "slash_command.rs");
const protocolPath = path.join(codexRoot, "codex-rs", "app-server-protocol", "src", "protocol", "common.rs");
const manifestPath = path.join(repoRoot, "src", "lib", "codex-commands.ts");

function fail(message) {
  console.error(message);
  process.exitCode = 1;
}

function readIfExists(filePath) {
  if (!fs.existsSync(filePath)) {
    return null;
  }
  return fs.readFileSync(filePath, "utf8");
}

function kebabCase(value) {
  return value
    .replace(/([a-z0-9])([A-Z])/g, "$1-$2")
    .replace(/_/g, "-")
    .toLowerCase();
}

function parseUpstreamSlashCommands(source) {
  const enumBody = source.match(/pub enum SlashCommand \{([\s\S]*?)\n\}/)?.[1] ?? "";
  const commands = [];
  let pendingAttr = "";
  for (const rawLine of enumBody.split(/\r?\n/u)) {
    const line = rawLine.trim();
    if (!line || line.startsWith("//")) {
      continue;
    }
    if (line.startsWith("#[strum(")) {
      pendingAttr = line;
      continue;
    }
    const variant = line.match(/^([A-Za-z][A-Za-z0-9_]*)\s*,/u)?.[1];
    if (!variant) {
      continue;
    }
    const toStringValue = pendingAttr.match(/to_string\s*=\s*"([^"]+)"/u)?.[1];
    const serializeValue = pendingAttr.match(/serialize\s*=\s*"([^"]+)"/u)?.[1];
    commands.push(toStringValue ?? serializeValue ?? kebabCase(variant));
    pendingAttr = "";
  }
  return commands;
}

function parseManifestCommands(source) {
  const entries = [...source.matchAll(/\{\s*command:\s*"([^"]+)"[\s\S]*?source:\s*"([^"]+)"/gu)].map((match) => ({
    command: match[1],
    source: match[2]
  }));
  return entries;
}

function parseProtocolMethods(source) {
  const requests = new Set();
  const notifications = new Set();
  for (const match of source.matchAll(/=>\s+"([^"]+)"\s*([({])/gu)) {
    if (match[2] === "{") {
      requests.add(match[1]);
    } else {
      notifications.add(match[1]);
    }
  }
  return {
    requests: [...requests].sort(),
    notifications: [...notifications].sort()
  };
}

const supportedRequests = new Set([
  "account/chatgptAuthTokens/refresh",
  "account/login/cancel",
  "account/login/start",
  "account/logout",
  "account/rateLimits/read",
  "account/read",
  "collaborationMode/list",
  "config/read",
  "model/list",
  "plugin/list",
  "plugin/read",
  "plugin/skill/read",
  "review/start",
  "skills/list",
  "thread/archive",
  "thread/fork",
  "thread/goal/clear",
  "thread/goal/get",
  "thread/goal/set",
  "thread/list",
  "thread/loaded/list",
  "thread/metadata/update",
  "thread/name/set",
  "thread/read",
  "thread/resume",
  "thread/rollback",
  "thread/start",
  "thread/turns/list",
  "thread/unarchive",
  "turn/interrupt",
  "turn/start",
  "turn/steer"
]);

const plannedRequests = new Set([
  "app/list",
  "config/mcpServer/reload",
  "configRequirements/read",
  "experimentalFeature/enablement/set",
  "experimentalFeature/list",
  "externalAgentConfig/detect",
  "externalAgentConfig/import",
  "feedback/upload",
  "fuzzyFileSearch/sessionStart",
  "fuzzyFileSearch/sessionStop",
  "fuzzyFileSearch/sessionUpdate",
  "hooks/list",
  "mcpServer/oauth/login",
  "mcpServer/resource/read",
  "mcpServerStatus/list",
  "memory/reset",
  "modelProvider/capabilities/read",
  "thread/approveGuardianDeniedAction",
  "thread/compact/start",
  "thread/memoryMode/set",
  "thread/realtime/appendAudio",
  "thread/realtime/appendText",
  "thread/realtime/listVoices",
  "thread/realtime/start",
  "thread/realtime/stop",
  "thread/turns/items/list",
  "windowsSandbox/readiness"
]);

const blockedRequests = new Set([
  "account/sendAddCreditsNudgeEmail",
  "command/exec",
  "command/exec/resize",
  "command/exec/terminate",
  "command/exec/write",
  "config/batchWrite",
  "config/value/write",
  "device/key/create",
  "device/key/public",
  "device/key/sign",
  "fs/copy",
  "fs/createDirectory",
  "fs/getMetadata",
  "fs/readDirectory",
  "fs/readFile",
  "fs/remove",
  "fs/unwatch",
  "fs/watch",
  "fs/writeFile",
  "item/commandExecution/requestApproval",
  "item/fileChange/requestApproval",
  "item/permissions/requestApproval",
  "item/tool/call",
  "item/tool/requestUserInput",
  "marketplace/add",
  "marketplace/remove",
  "marketplace/upgrade",
  "mcpServer/elicitation/request",
  "mcpServer/tool/call",
  "mock/experimentalMethod",
  "plugin/install",
  "plugin/share/delete",
  "plugin/share/list",
  "plugin/share/save",
  "plugin/share/updateTargets",
  "plugin/uninstall",
  "process/kill",
  "process/resizePty",
  "process/spawn",
  "process/writeStdin",
  "skills/config/write",
  "thread/backgroundTerminals/clean",
  "thread/decrement_elicitation",
  "thread/increment_elicitation",
  "thread/inject_items",
  "thread/shellCommand",
  "thread/unsubscribe",
  "windowsSandbox/setupStart"
]);

function classifyRequest(method) {
  if (supportedRequests.has(method)) {
    return "supported";
  }
  if (plannedRequests.has(method)) {
    return "planned";
  }
  if (blockedRequests.has(method)) {
    return "blocked";
  }
  return null;
}

function classifyNotification(method) {
  if (
    method === "thread/goal/updated" ||
    method === "thread/goal/cleared" ||
    method.startsWith("thread/") ||
    method.startsWith("turn/") ||
    method.startsWith("item/") ||
    method.startsWith("rawResponseItem/") ||
    method.startsWith("mcpServer/") ||
    method.startsWith("account/") ||
    method.startsWith("app/") ||
    method.startsWith("remoteControl/") ||
    method.startsWith("externalAgentConfig/") ||
    method.startsWith("fuzzyFileSearch/") ||
    method.startsWith("model/") ||
    method.startsWith("process/") ||
    method.startsWith("command/") ||
    method.startsWith("hook/") ||
    method.startsWith("fs/") ||
    method.startsWith("windows") ||
    ["error", "warning", "guardianWarning", "deprecationNotice", "configWarning", "skills/changed", "serverRequest/resolved"].includes(method)
  ) {
    return "relay-or-known";
  }
  return null;
}

const slashSource = readIfExists(slashCommandPath);
const protocolSource = readIfExists(protocolPath);
if (!slashSource || !protocolSource) {
  console.warn(`Skipping Codex compatibility verification; Codex repo was not found at ${codexRoot}.`);
  process.exit(0);
}

const manifestSource = fs.readFileSync(manifestPath, "utf8");
const upstreamCommands = parseUpstreamSlashCommands(slashSource);
const manifestEntries = parseManifestCommands(manifestSource);
const upstreamManifestCommands = manifestEntries
  .filter((entry) => entry.source === "upstream")
  .map((entry) => entry.command);

const duplicates = upstreamManifestCommands.filter((command, index) => upstreamManifestCommands.indexOf(command) !== index);
if (duplicates.length > 0) {
  fail(`Duplicate upstream command classifications: ${[...new Set(duplicates)].join(", ")}`);
}

const missingCommands = upstreamCommands.filter((command) => !upstreamManifestCommands.includes(command));
const staleCommands = upstreamManifestCommands.filter((command) => !upstreamCommands.includes(command));
if (missingCommands.length > 0) {
  fail(`Missing upstream slash command classifications: ${missingCommands.join(", ")}`);
}
if (staleCommands.length > 0) {
  fail(`Stale upstream slash command classifications: ${staleCommands.join(", ")}`);
}

const { requests, notifications } = parseProtocolMethods(protocolSource);
const unclassifiedRequests = requests.filter((method) => !classifyRequest(method));
const unclassifiedNotifications = notifications.filter((method) => !classifyNotification(method));
if (unclassifiedRequests.length > 0) {
  fail(`Unclassified Codex app-server requests: ${unclassifiedRequests.join(", ")}`);
}
if (unclassifiedNotifications.length > 0) {
  fail(`Unclassified Codex app-server notifications: ${unclassifiedNotifications.join(", ")}`);
}

if (process.exitCode) {
  process.exit(process.exitCode);
}

console.log(
  `Codex compatibility verified: ${upstreamCommands.length} slash commands, ${requests.length} requests, ${notifications.length} notifications.`
);
