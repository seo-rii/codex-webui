import fsp from "node:fs/promises";
import path from "node:path";

import type { CatalogPayload, PluginCatalogEntry, SkillCatalogEntry } from "$lib/types";

import { getCurrentRuntimeProfile } from "./env";

const catalogCache = new Map<
  string,
  {
    expiresAt: number;
    payload: CatalogPayload;
  }
>();

function parseFrontMatter(raw: string) {
  const match = raw.match(/^---\n([\s\S]*?)\n---/u);
  if (!match) {
    return { name: null, description: null };
  }

  const lines = match[1].split(/\r?\n/u);
  let name: string | null = null;
  let description: string | null = null;
  for (const line of lines) {
    const [key, ...rest] = line.split(":");
    if (!key || rest.length === 0) {
      continue;
    }
    const value = rest.join(":").trim();
    if (key.trim() === "name") {
      name = value || null;
    } else if (key.trim() === "description") {
      description = value || null;
    }
  }

  return { name, description };
}

async function walkFiles(rootPath: string, matcher: (filePath: string) => boolean, results: string[]) {
  const entries = await fsp.readdir(rootPath, { withFileTypes: true }).catch(() => []);
  for (const entry of entries) {
    const nextPath = path.join(rootPath, entry.name);
    if (entry.isDirectory()) {
      await walkFiles(nextPath, matcher, results);
      continue;
    }
    if (entry.isFile() && matcher(nextPath)) {
      results.push(nextPath);
    }
  }
}

async function listSkillEntries(codexHome: string): Promise<SkillCatalogEntry[]> {
  const skillsRoot = path.join(codexHome, "skills");
  const pluginRoot = path.join(codexHome, "plugins");
  const skillFiles: string[] = [];
  const pluginSkillFiles: string[] = [];

  await walkFiles(skillsRoot, (filePath) => path.basename(filePath) === "SKILL.md", skillFiles).catch(() => {});
  await walkFiles(pluginRoot, (filePath) => path.basename(filePath) === "SKILL.md", pluginSkillFiles).catch(() => {});

  const entries = await Promise.all(
    [...skillFiles, ...pluginSkillFiles].map(async (filePath) => {
      const raw = await fsp.readFile(filePath, "utf8").catch(() => "");
      const metadata = parseFrontMatter(raw);
      const relativePath = filePath.startsWith(`${skillsRoot}${path.sep}`)
        ? path.relative(skillsRoot, filePath)
        : filePath.startsWith(`${pluginRoot}${path.sep}`)
          ? path.relative(pluginRoot, filePath)
          : path.basename(path.dirname(filePath));
      const normalizedRelative = relativePath.replace(/\\/gu, "/");
      const pluginMatch = normalizedRelative.match(/([^/]+)\/[^/]+\/[^/]+\/[^/]+\/skills\//u);
      const pluginName = pluginMatch?.[1] ?? null;
      const source = normalizedRelative.startsWith(".system/")
        ? "system"
        : filePath.startsWith(`${pluginRoot}${path.sep}`)
          ? "plugin"
          : "local";

      return {
        id: normalizedRelative.replace(/\/SKILL\.md$/u, ""),
        name: metadata.name ?? path.basename(path.dirname(filePath)),
        description: metadata.description ?? "",
        path: filePath,
        source,
        pluginName
      } satisfies SkillCatalogEntry;
    })
  );

  return entries.sort((left, right) => left.name.localeCompare(right.name));
}

async function listPluginEntries(codexHome: string): Promise<PluginCatalogEntry[]> {
  const pluginRoot = path.join(codexHome, "plugins");
  const pluginFiles: string[] = [];
  await walkFiles(pluginRoot, (filePath) => filePath.endsWith(`${path.sep}.codex-plugin${path.sep}plugin.json`), pluginFiles).catch(() => {});

  const entries = await Promise.all(
    pluginFiles.map(async (filePath) => {
      const raw = await fsp.readFile(filePath, "utf8").catch(() => "{}");
      const parsed = JSON.parse(raw) as Record<string, unknown>;
      const pluginBase = path.dirname(path.dirname(filePath));
      const display = (parsed.interface as Record<string, unknown> | undefined)?.displayName;
      const category = (parsed.interface as Record<string, unknown> | undefined)?.category;
      const developerName = (parsed.interface as Record<string, unknown> | undefined)?.developerName;
      const skillsDir = typeof parsed.skills === "string" ? path.resolve(pluginBase, parsed.skills) : null;
      const skillFiles: string[] = [];
      if (skillsDir) {
        await walkFiles(skillsDir, (candidate) => path.basename(candidate) === "SKILL.md", skillFiles).catch(() => {});
      }

      return {
        name: String(parsed.name ?? path.basename(pluginBase)),
        displayName: typeof display === "string" && display.trim() ? display : String(parsed.name ?? path.basename(pluginBase)),
        description: typeof parsed.description === "string" ? parsed.description : "",
        version: typeof parsed.version === "string" ? parsed.version : null,
        developerName: typeof developerName === "string" ? developerName : null,
        category: typeof category === "string" ? category : null,
        path: pluginBase,
        skills: skillFiles.map((skillPath) => path.basename(path.dirname(skillPath))).sort((left, right) => left.localeCompare(right))
      } satisfies PluginCatalogEntry;
    })
  );

  return entries.sort((left, right) => left.displayName.localeCompare(right.displayName));
}

export async function getCatalogForCodexHome(codexHome: string): Promise<CatalogPayload> {
  const cached = catalogCache.get(codexHome);
  if (cached && cached.expiresAt > Date.now()) {
    return cached.payload;
  }
  const [plugins, skills] = await Promise.all([listPluginEntries(codexHome), listSkillEntries(codexHome)]);
  const payload = { plugins, skills };
  catalogCache.set(codexHome, {
    expiresAt: Date.now() + 10_000,
    payload
  });
  return payload;
}

export async function getCatalog(): Promise<CatalogPayload> {
  return getCatalogForCodexHome(getCurrentRuntimeProfile().codexHome);
}
