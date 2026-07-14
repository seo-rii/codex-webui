import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

function readPackageVersion(projectRoot) {
  try {
    const packageJson = JSON.parse(fs.readFileSync(path.join(projectRoot, "package.json"), "utf8"));
    return String(packageJson.version ?? "0.0.0").trim() || "0.0.0";
  } catch {
    return "0.0.0";
  }
}

function gitOutput(projectRoot, args) {
  const result = spawnSync("git", args, {
    cwd: projectRoot,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"]
  });
  if (result.status !== 0) {
    return null;
  }
  return result.stdout.trim() || null;
}

function isGitDirty(projectRoot) {
  return Boolean(gitOutput(projectRoot, ["status", "--porcelain"]));
}

function sanitizeVersionPart(value) {
  return String(value ?? "")
    .trim()
    .replace(/[^0-9A-Za-z._-]+/gu, "-")
    .replace(/^-+|-+$/gu, "");
}

function shortCommit(commit) {
  const normalized = sanitizeVersionPart(commit);
  if (!normalized || normalized === "unknown") {
    return "unknown";
  }
  return normalized.slice(0, 12);
}

export function createBuildMetadata(projectRoot, env = process.env) {
  const packageVersion = sanitizeVersionPart(env.CODEX_WEBUI_PACKAGE_VERSION) || readPackageVersion(projectRoot);
  const repositoryCommit = gitOutput(projectRoot, ["rev-parse", "HEAD"]);
  const inheritedCommit = sanitizeVersionPart(env.CODEX_WEBUI_BUILD_COMMIT);
  const inheritedMetadataMatches = !repositoryCommit || !inheritedCommit || inheritedCommit === repositoryCommit;
  const commit = repositoryCommit || inheritedCommit || "unknown";
  const commitShort =
    inheritedMetadataMatches && sanitizeVersionPart(env.CODEX_WEBUI_BUILD_COMMIT_SHORT)
      ? sanitizeVersionPart(env.CODEX_WEBUI_BUILD_COMMIT_SHORT)
      : shortCommit(commit);
  const dirty = repositoryCommit
    ? isGitDirty(projectRoot)
    : env.CODEX_WEBUI_BUILD_DIRTY === "true" || env.CODEX_WEBUI_BUILD_DIRTY === "false"
      ? env.CODEX_WEBUI_BUILD_DIRTY === "true"
      : isGitDirty(projectRoot);
  const epochMs =
    inheritedMetadataMatches && sanitizeVersionPart(env.CODEX_WEBUI_BUILD_EPOCH_MS)
      ? sanitizeVersionPart(env.CODEX_WEBUI_BUILD_EPOCH_MS)
      : String(Date.now());
  const timestamp = String(
    inheritedMetadataMatches && env.CODEX_WEBUI_BUILD_TIMESTAMP
      ? env.CODEX_WEBUI_BUILD_TIMESTAMP
      : new Date(Number(epochMs)).toISOString()
  );
  const requestedVersion = inheritedMetadataMatches ? sanitizeVersionPart(env.CODEX_WEBUI_BUILD_VERSION) : "";
  const version =
    requestedVersion && requestedVersion.includes(commitShort)
      ? requestedVersion
      : [requestedVersion || packageVersion, commitShort, dirty ? "dirty" : "", epochMs].filter(Boolean).join("-");

  return {
    packageVersion,
    version,
    commit,
    commitShort,
    dirty,
    epochMs,
    timestamp
  };
}

export function buildMetadataEnv(metadata) {
  return {
    CODEX_WEBUI_PACKAGE_VERSION: metadata.packageVersion,
    CODEX_WEBUI_BUILD_VERSION: metadata.version,
    CODEX_WEBUI_BUILD_COMMIT: metadata.commit,
    CODEX_WEBUI_BUILD_COMMIT_SHORT: metadata.commitShort,
    CODEX_WEBUI_BUILD_DIRTY: metadata.dirty ? "true" : "false",
    CODEX_WEBUI_BUILD_EPOCH_MS: metadata.epochMs,
    CODEX_WEBUI_BUILD_TIMESTAMP: metadata.timestamp
  };
}
