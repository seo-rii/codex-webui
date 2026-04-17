import fsp from "node:fs/promises";
import path from "node:path";

import { error } from "@sveltejs/kit";

import type { EditableFilePayload } from "$lib/types";

import { getCurrentRuntimeProfile, getRuntimeConfig } from "./env";
import { realPathSafe } from "./fs";

function inferLanguage(filePath: string) {
  const extension = path.extname(filePath).toLowerCase();
  switch (extension) {
    case ".ts":
    case ".tsx":
      return "typescript";
    case ".js":
    case ".mjs":
    case ".cjs":
    case ".jsx":
      return "javascript";
    case ".json":
      return "json";
    case ".toml":
      return "ini";
    case ".md":
      return "markdown";
    case ".yml":
    case ".yaml":
      return "yaml";
    case ".svelte":
      return "html";
    case ".rs":
      return "rust";
    case ".py":
      return "python";
    case ".css":
      return "css";
    case ".sh":
      return "shell";
    default:
      return "plaintext";
  }
}

function isPathInside(parentPath: string, candidatePath: string) {
  return candidatePath === parentPath || candidatePath.startsWith(`${parentPath}${path.sep}`);
}

async function resolveWritablePath(filePath: string) {
  if (!filePath.trim()) {
    throw error(400, "filePath is required.");
  }
  const config = getRuntimeConfig();
  const profile = getCurrentRuntimeProfile();
  const candidatePath = path.resolve(filePath);
  const roots = [...config.allowedRoots.map((root) => path.resolve(root)), path.resolve(profile.codexHome)];
  const existingPath = await realPathSafe(candidatePath).catch(() => null);
  const pathToCheck = existingPath ?? candidatePath;

  if (!roots.some((root) => isPathInside(root, pathToCheck))) {
    throw error(403, "This file is outside editable roots.");
  }

  return candidatePath;
}

export async function readEditableFile(filePath: string): Promise<EditableFilePayload> {
  const resolvedPath = await resolveWritablePath(filePath);
  const content = await fsp.readFile(resolvedPath, "utf8").catch((cause: NodeJS.ErrnoException) => {
    if (cause.code === "ENOENT") {
      return "";
    }
    throw cause;
  });

  return {
    path: resolvedPath,
    displayName: path.basename(resolvedPath),
    content,
    language: inferLanguage(resolvedPath),
    writable: true
  };
}

export async function writeEditableFile(filePath: string, content: string): Promise<EditableFilePayload> {
  const resolvedPath = await resolveWritablePath(filePath);
  await fsp.mkdir(path.dirname(resolvedPath), { recursive: true });
  await fsp.writeFile(resolvedPath, content, "utf8");
  return readEditableFile(resolvedPath);
}
