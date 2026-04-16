import fs from "node:fs";
import fsp from "node:fs/promises";
import path from "node:path";

import type { ApprovalPolicy, ReasoningEffort, SandboxMode, ServiceSpeed, SessionPreferences } from "$lib/types";

type CodexTomlDefaults = {
  model: string | null;
  modelReasoningEffort: ReasoningEffort | null;
  planModeReasoningEffort: ReasoningEffort | null;
  approvalPolicy: ApprovalPolicy | null;
  sandboxMode: SandboxMode | null;
  serviceTier: ServiceSpeed;
  networkAccess: boolean | null;
};

const CONFIG_SCHEMA_HEADER = "#:schema https://developers.openai.com/codex/config-schema.json";

export function configTomlPath(codexHome: string) {
  return path.join(codexHome, "config.toml");
}

function parseSectionName(line: string) {
  const match = line.match(/^\s*\[([^\]]+)\]\s*$/u);
  return match ? match[1].trim() : null;
}

function matchesKey(line: string, key: string) {
  return new RegExp(`^\\s*${key.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&")}\\s*=`, "u").test(line);
}

function trimTomlValue(value: string) {
  let result = "";
  let escaped = false;
  let quote: '"' | "'" | null = null;

  for (const character of value) {
    if (escaped) {
      result += character;
      escaped = false;
      continue;
    }

    if (character === "\\") {
      result += character;
      escaped = true;
      continue;
    }

    if ((character === '"' || character === "'") && (!quote || quote === character)) {
      quote = quote ? null : character;
      result += character;
      continue;
    }

    if (character === "#" && !quote) {
      break;
    }

    result += character;
  }

  return result.trim();
}

function getTomlValue(raw: string, section: string | null, key: string) {
  let currentSection: string | null = null;

  for (const line of raw.split(/\r?\n/u)) {
    const nextSection = parseSectionName(line);
    if (nextSection !== null) {
      currentSection = nextSection;
      continue;
    }

    if (currentSection !== section || !matchesKey(line, key)) {
      continue;
    }

    const [, value = ""] = line.split("=", 2);
    return trimTomlValue(value);
  }

  return null;
}

function parseTomlString(value: string | null) {
  if (!value) {
    return null;
  }

  if (value.startsWith('"') && value.endsWith('"')) {
    try {
      return JSON.parse(value) as string;
    } catch {
      return value.slice(1, -1).replace(/\\"/gu, '"').replace(/\\\\/gu, "\\");
    }
  }

  if (value.startsWith("'") && value.endsWith("'")) {
    return value.slice(1, -1);
  }

  return null;
}

function parseTomlBoolean(value: string | null) {
  if (value === "true") {
    return true;
  }
  if (value === "false") {
    return false;
  }
  return null;
}

function normalizeLines(raw: string) {
  const lines = raw.split(/\r?\n/u);
  while (lines.length > 1 && lines.at(-1) === "") {
    lines.pop();
  }
  return lines;
}

function stringifyTomlString(value: string) {
  return `"${value.replace(/\\/gu, "\\\\").replace(/"/gu, '\\"')}"`;
}

function upsertTomlValue(raw: string, section: string | null, key: string, value: string | null) {
  const lines = normalizeLines(raw);
  let currentSection: string | null = null;
  let sectionStart = section === null ? 0 : -1;
  let sectionEnd = lines.length;
  let replaced = false;

  for (let index = 0; index < lines.length; index += 1) {
    const nextSection = parseSectionName(lines[index]);
    if (nextSection !== null) {
      if (currentSection === section && sectionEnd === lines.length) {
        sectionEnd = index;
      }
      currentSection = nextSection;
      if (section !== null && nextSection === section && sectionStart === -1) {
        sectionStart = index;
      }
      continue;
    }

    if (currentSection !== section || !matchesKey(lines[index], key)) {
      continue;
    }

    replaced = true;
    if (value === null) {
      lines.splice(index, 1);
      index -= 1;
      sectionEnd -= 1;
      continue;
    }

    lines[index] = `${key} = ${value}`;
  }

  if (!replaced && value !== null) {
    if (section === null) {
      const firstSectionIndex = lines.findIndex((line) => parseSectionName(line) !== null);
      const insertIndex = firstSectionIndex === -1 ? lines.length : firstSectionIndex;
      lines.splice(insertIndex, 0, `${key} = ${value}`);
    } else if (sectionStart === -1) {
      if (lines.length > 0 && lines.at(-1) !== "") {
        lines.push("");
      }
      lines.push(`[${section}]`, `${key} = ${value}`);
    } else {
      lines.splice(sectionEnd, 0, `${key} = ${value}`);
    }
  }

  while (lines.length > 1 && lines.at(-1) === "") {
    lines.pop();
  }

  return `${lines.join("\n")}\n`;
}

export function readCodexTomlDefaults(codexHome: string): CodexTomlDefaults {
  const filePath = configTomlPath(codexHome);
  if (!fs.existsSync(filePath)) {
    return {
      model: null,
      modelReasoningEffort: null,
      planModeReasoningEffort: null,
      approvalPolicy: null,
      sandboxMode: null,
      serviceTier: "auto",
      networkAccess: null
    };
  }

  const raw = fs.readFileSync(filePath, "utf8");
  const model = parseTomlString(getTomlValue(raw, null, "model"));
  const modelReasoningEffort = parseTomlString(getTomlValue(raw, null, "model_reasoning_effort")) as ReasoningEffort | null;
  const planModeReasoningEffort = parseTomlString(getTomlValue(raw, null, "plan_mode_reasoning_effort")) as ReasoningEffort | null;
  const approvalPolicy = parseTomlString(getTomlValue(raw, null, "approval_policy")) as ApprovalPolicy | null;
  const sandboxMode = parseTomlString(getTomlValue(raw, null, "sandbox_mode")) as SandboxMode | null;
  const serviceTier = (parseTomlString(getTomlValue(raw, null, "service_tier")) as ServiceSpeed | null) ?? "auto";
  const networkAccess = parseTomlBoolean(getTomlValue(raw, "sandbox_workspace_write", "network_access"));

  return {
    model,
    modelReasoningEffort,
    planModeReasoningEffort,
    approvalPolicy,
    sandboxMode,
    serviceTier: serviceTier === "fast" || serviceTier === "flex" ? serviceTier : "auto",
    networkAccess
  };
}

export async function syncCodexTomlWithPreferences(codexHome: string, preferences: SessionPreferences) {
  const filePath = configTomlPath(codexHome);
  await fsp.mkdir(path.dirname(filePath), { recursive: true });

  let raw = "";
  try {
    raw = await fsp.readFile(filePath, "utf8");
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") {
      throw error;
    }
  }

  if (!raw.trim()) {
    raw = `${CONFIG_SCHEMA_HEADER}\n`;
  }

  raw = upsertTomlValue(raw, null, "model", preferences.model ? stringifyTomlString(preferences.model) : null);
  raw = upsertTomlValue(raw, null, "approval_policy", stringifyTomlString(preferences.approvalPolicy));
  raw = upsertTomlValue(raw, null, "sandbox_mode", stringifyTomlString(preferences.sandboxMode));
  raw = upsertTomlValue(
    raw,
    null,
    "service_tier",
    preferences.speed === "auto" ? null : stringifyTomlString(preferences.speed)
  );

  if (preferences.mode === "plan") {
    raw = upsertTomlValue(
      raw,
      null,
      "plan_mode_reasoning_effort",
      preferences.effort ? stringifyTomlString(preferences.effort) : null
    );
  } else {
    raw = upsertTomlValue(
      raw,
      null,
      "model_reasoning_effort",
      preferences.effort ? stringifyTomlString(preferences.effort) : null
    );
  }

  raw = upsertTomlValue(
    raw,
    "sandbox_workspace_write",
    "network_access",
    preferences.networkAccess ? "true" : "false"
  );

  await fsp.writeFile(filePath, raw, "utf8");
}
