import fs from "node:fs";
import fsp from "node:fs/promises";
import path from "node:path";
import type { Dirent } from "node:fs";

import { error } from "@sveltejs/kit";

import type { DirectoryEntry, SessionPreferences } from "$lib/types";

import { getRuntimeConfig } from "./env";

function normalizePath(target: string) {
  return path.resolve(target);
}

export async function realPathSafe(target: string) {
  const resolved = normalizePath(target);
  try {
    return await fsp.realpath(resolved);
  } catch {
    return resolved;
  }
}

export function sanitizeFileName(name: string) {
  return name.replace(/[^a-zA-Z0-9._-]+/g, "-").replace(/^-+|-+$/g, "") || "attachment";
}

export async function ensureDataDirectories() {
  const { dataDir } = getRuntimeConfig();
  await fsp.mkdir(dataDir, { recursive: true });
  await fsp.mkdir(path.join(dataDir, "uploads"), { recursive: true });
}

export async function resolveAllowedDirectory(candidate: string) {
  const normalized = await realPathSafe(candidate);
  const roots = await Promise.all(getRuntimeConfig().allowedRoots.map((root: string) => realPathSafe(root)));
  const allowed = roots.some((root: string) => normalized === root || normalized.startsWith(`${root}${path.sep}`));
  if (!allowed) {
    throw error(403, "The selected path is outside the allowed roots.");
  }
  const stats = await fsp.stat(normalized).catch(() => null);
  if (!stats?.isDirectory()) {
    throw error(400, "The selected path is not a directory.");
  }
  return normalized;
}

export async function listDirectoryPayload(currentPath: string | null) {
  const roots = await Promise.all(
    getRuntimeConfig().allowedRoots.map(async (root) => {
      const resolved = await realPathSafe(root);
      return {
        name: path.basename(resolved) || resolved,
        path: resolved,
        isDirectory: true
      } satisfies DirectoryEntry;
    })
  );

  if (!currentPath) {
    return {
      allowedRoots: roots,
      currentPath: null,
      parentPath: null,
      entries: roots
    };
  }

  const resolved = await resolveAllowedDirectory(currentPath);
  const dirents = await fsp.readdir(resolved, { withFileTypes: true });
  const entries = dirents
    .filter((entry: Dirent) => entry.isDirectory())
    .map((entry: Dirent) => ({
      name: entry.name,
      path: path.join(resolved, entry.name),
      isDirectory: true
    }) satisfies DirectoryEntry)
    .sort((left: DirectoryEntry, right: DirectoryEntry) => left.name.localeCompare(right.name));

  const parentPath = roots.some((root) => root.path === resolved) ? null : path.dirname(resolved);

  return {
    allowedRoots: roots,
    currentPath: resolved,
    parentPath,
    entries
  };
}

export function getStoreFilePath() {
  return path.join(getRuntimeConfig().dataDir, "ui-state.json");
}

export function getUploadsRoot() {
  return path.join(getRuntimeConfig().dataDir, "uploads");
}

export function getThreadUploadsDir(threadId: string) {
  return path.join(getUploadsRoot(), threadId);
}

export function buildSandboxPolicy(preferences: SessionPreferences, additionalReadableRoots: string[]) {
  const readableRoots = [...new Set([preferences.cwd, ...additionalReadableRoots])];
  const readOnlyAccess = {
    type: "restricted",
    includePlatformDefaults: true,
    readableRoots
  };

  if (preferences.sandboxMode === "danger-full-access") {
    return { type: "dangerFullAccess" };
  }

  if (preferences.sandboxMode === "read-only") {
    return {
      type: "readOnly",
      access: readOnlyAccess,
      networkAccess: preferences.networkAccess
    };
  }

  return {
    type: "workspaceWrite",
    writableRoots: [preferences.cwd],
    readOnlyAccess,
    networkAccess: preferences.networkAccess,
    excludeTmpdirEnvVar: false,
    excludeSlashTmp: false
  };
}

export async function pathExists(target: string) {
  try {
    await fsp.access(target, fs.constants.F_OK);
    return true;
  } catch {
    return false;
  }
}
