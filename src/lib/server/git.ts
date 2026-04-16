import { execFile } from "node:child_process";
import fsp from "node:fs/promises";
import path from "node:path";
import type { Dirent } from "node:fs";
import { promisify } from "node:util";

import { error } from "@sveltejs/kit";

import type { GitBranch, GitCommit, GitFilePayload, GitFileStatus, GitRepository, GitStatusPayload, GitWorktree } from "$lib/types";

import { getRuntimeConfig } from "./env";
import { realPathSafe } from "./fs";

const execFileAsync = promisify(execFile);
const SKIP_DIRS = new Set([".git", "node_modules", ".svelte-kit", "build", "dist", ".next", "coverage"]);
const REPO_CACHE_TTL_MS = 5_000;

let repositoryCache:
  | {
      expiresAt: number;
      repositories: GitRepository[];
    }
  | null = null;
const pinnedRepositories = new Map<string, GitRepository>();

function normalizeGitPath(filePath: string) {
  return filePath.split(path.sep).join("/");
}

function isPathInside(parentPath: string, candidatePath: string) {
  return candidatePath === parentPath || candidatePath.startsWith(`${parentPath}${path.sep}`);
}

function mergeRepositories(...collections: GitRepository[][]) {
  return [...new Map(collections.flat().map((repository) => [repository.path, repository])).values()].sort((left, right) =>
    left.relativePath.localeCompare(right.relativePath)
  );
}

function mapGitCode(code: string) {
  switch (code) {
    case "M":
      return "modified";
    case "A":
      return "added";
    case "D":
      return "deleted";
    case "R":
      return "renamed";
    case "C":
      return "copied";
    case "U":
      return "unmerged";
    case "?":
      return "untracked";
    case "!":
      return "ignored";
    default:
      return "clean";
  }
}

function inferLanguage(filePath: string) {
  const extension = path.extname(filePath).toLowerCase();
  switch (extension) {
    case ".ts":
      return "typescript";
    case ".tsx":
      return "typescript";
    case ".js":
    case ".mjs":
    case ".cjs":
      return "javascript";
    case ".jsx":
      return "javascript";
    case ".svelte":
      return "html";
    case ".json":
      return "json";
    case ".css":
      return "css";
    case ".scss":
      return "scss";
    case ".html":
      return "html";
    case ".md":
      return "markdown";
    case ".yml":
    case ".yaml":
      return "yaml";
    case ".sh":
      return "shell";
    case ".rs":
      return "rust";
    case ".py":
      return "python";
    case ".go":
      return "go";
    case ".java":
      return "java";
    case ".kt":
      return "kotlin";
    case ".swift":
      return "swift";
    default:
      return "plaintext";
  }
}

function isBinaryBuffer(buffer: Buffer) {
  return buffer.includes(0);
}

function parseStatusHeader(header: string) {
  const summary = header.replace(/^## /u, "");
  const [branchPart, trackingPart] = summary.split("...");
  const branch = branchPart?.trim() === "HEAD (no branch)" ? null : branchPart?.trim() ?? null;
  const ahead = Number(trackingPart?.match(/ahead (\d+)/u)?.[1] ?? "0");
  const behind = Number(trackingPart?.match(/behind (\d+)/u)?.[1] ?? "0");
  return { branch, ahead, behind };
}

function parseStatusLine(line: string): GitFileStatus {
  const stagedCode = line[0] ?? " ";
  const unstagedCode = line[1] ?? " ";
  const rawPath = line.slice(3);
  const renameMatch = rawPath.match(/^(.*) -> (.*)$/u);
  const originalPath = renameMatch ? renameMatch[1] : null;
  const filePath = renameMatch ? renameMatch[2] : rawPath;

  return {
    path: filePath,
    originalPath,
    stagedCode,
    unstagedCode,
    stagedLabel: mapGitCode(stagedCode),
    unstagedLabel: mapGitCode(unstagedCode),
    hasStagedChanges: stagedCode !== " " && stagedCode !== "?",
    hasUnstagedChanges: unstagedCode !== " " && unstagedCode !== "?",
    isUntracked: stagedCode === "?" && unstagedCode === "?"
  };
}

function resolveRepoFilePath(repoPath: string, filePath: string) {
  const candidate = path.resolve(repoPath, filePath);
  if (candidate !== repoPath && !candidate.startsWith(`${repoPath}${path.sep}`)) {
    throw error(403, "The selected file is outside the repository root.");
  }
  return candidate;
}

async function ensureWorktreePathAllowed(worktreePath: string) {
  const candidatePath = path.resolve(worktreePath);
  const roots = await getAllowedGitRoots();
  if (!roots.some((root) => isPathInside(root, candidatePath))) {
    throw error(403, "The selected worktree path is outside allowed roots.");
  }
  return candidatePath;
}

async function runGitText(repoPath: string, args: string[]) {
  try {
    const result = await execFileAsync("git", ["-C", repoPath, ...args], {
      maxBuffer: 20 * 1024 * 1024,
      encoding: "utf8"
    });
    return result.stdout;
  } catch (cause) {
    const stderr =
      cause && typeof cause === "object" && "stderr" in cause ? String((cause as { stderr?: string | Buffer }).stderr ?? "") : "";
    throw error(400, stderr.trim() || `git ${args[0] ?? "command"} failed.`);
  }
}

async function runGitBuffer(repoPath: string, args: string[]) {
  try {
    const result = await execFileAsync("git", ["-C", repoPath, ...args], {
      maxBuffer: 20 * 1024 * 1024,
      encoding: "buffer"
    });
    return result.stdout;
  } catch (cause) {
    const stderr =
      cause && typeof cause === "object" && "stderr" in cause ? String((cause as { stderr?: string | Buffer }).stderr ?? "") : "";
    throw error(400, stderr.trim() || `git ${args[0] ?? "command"} failed.`);
  }
}

async function readFileContent(targetPath: string) {
  try {
    const buffer = await fsp.readFile(targetPath);
    return {
      content: isBinaryBuffer(buffer) ? "" : buffer.toString("utf8"),
      isBinary: isBinaryBuffer(buffer)
    };
  } catch {
    return {
      content: "",
      isBinary: false
    };
  }
}

async function readHeadContent(repoPath: string, filePath: string) {
  try {
    const buffer = await runGitBuffer(repoPath, ["show", `HEAD:${normalizeGitPath(filePath)}`]);
    return {
      content: isBinaryBuffer(buffer) ? "" : buffer.toString("utf8"),
      isBinary: isBinaryBuffer(buffer)
    };
  } catch {
    return {
      content: "",
      isBinary: false
    };
  }
}

async function readCurrentBranch(repoPath: string) {
  const branch = (await runGitText(repoPath, ["branch", "--show-current"])).trim();
  return branch || null;
}

async function getAllowedGitRoots() {
  return Promise.all(getRuntimeConfig().allowedRoots.map((root: string) => realPathSafe(root)));
}

async function buildRepositoryRecord(repoPath: string, roots: string[]) {
  const normalizedRepoPath = await realPathSafe(repoPath);
  const rootPath = roots.find((candidate) => isPathInside(candidate, normalizedRepoPath));
  if (!rootPath) {
    throw error(404, "The selected repository was not found within allowed roots.");
  }

  return {
    path: normalizedRepoPath,
    name: path.basename(normalizedRepoPath),
    rootPath,
    relativePath: path.relative(rootPath, normalizedRepoPath) || ".",
    currentBranch: await readCurrentBranch(normalizedRepoPath).catch(() => null)
  } satisfies GitRepository;
}

async function rememberRepository(repoPath: string, roots: string[]) {
  const repository = await buildRepositoryRecord(repoPath, roots);
  pinnedRepositories.set(repository.path, repository);
  if (repositoryCache) {
    repositoryCache = {
      ...repositoryCache,
      repositories: mergeRepositories(repositoryCache.repositories, [repository])
    };
  }
  return repository;
}

async function hasGitMarker(candidatePath: string) {
  const gitMarker = await fsp.stat(path.join(candidatePath, ".git")).catch(() => null);
  return gitMarker?.isDirectory() || gitMarker?.isFile();
}

async function findAncestorRepository(targetPath: string, roots: string[]) {
  let currentPath = await realPathSafe(targetPath);
  const stats = await fsp.stat(currentPath).catch(() => null);
  if (!stats?.isDirectory()) {
    currentPath = path.dirname(currentPath);
  }

  while (roots.some((root) => isPathInside(root, currentPath))) {
    if (await hasGitMarker(currentPath)) {
      return rememberRepository(currentPath, roots);
    }

    const parentPath = path.dirname(currentPath);
    if (parentPath === currentPath) {
      break;
    }
    currentPath = parentPath;
  }

  return null;
}

async function findDescendantRepository(startPath: string, roots: string[]) {
  const stats = await fsp.stat(startPath).catch(() => null);
  if (!stats?.isDirectory()) {
    return null;
  }

  const queue = [{ currentPath: startPath, depth: 0 }];
  const maxDepth = Math.max(getRuntimeConfig().gitDiscoveryDepth + 2, 3);

  while (queue.length > 0) {
    const nextEntry = queue.shift();
    if (!nextEntry) {
      continue;
    }

    if (nextEntry.depth > 0 && (await hasGitMarker(nextEntry.currentPath))) {
      return rememberRepository(nextEntry.currentPath, roots);
    }

    if (nextEntry.depth >= maxDepth) {
      continue;
    }

    const entries = await fsp.readdir(nextEntry.currentPath, { withFileTypes: true }).catch(() => [] as Dirent[]);
    for (const entry of entries) {
      if (!entry.isDirectory() || SKIP_DIRS.has(entry.name)) {
        continue;
      }
      queue.push({
        currentPath: path.join(nextEntry.currentPath, entry.name),
        depth: nextEntry.depth + 1
      });
    }
  }

  return null;
}

async function walkRepositories(rootPath: string, currentPath: string, depth: number, repositories: GitRepository[]) {
  const gitMarker = await fsp.stat(path.join(currentPath, ".git")).catch(() => null);
  if (gitMarker?.isDirectory() || gitMarker?.isFile()) {
    repositories.push({
      path: currentPath,
      name: path.basename(currentPath),
      rootPath,
      relativePath: path.relative(rootPath, currentPath) || ".",
      currentBranch: null
    });
  }

  if (depth >= getRuntimeConfig().gitDiscoveryDepth) {
    return;
  }

  const entries = await fsp.readdir(currentPath, { withFileTypes: true }).catch(() => [] as Dirent[]);
  await Promise.all(
    entries
      .filter((entry: Dirent) => entry.isDirectory() && !SKIP_DIRS.has(entry.name))
      .map((entry: Dirent) => walkRepositories(rootPath, path.join(currentPath, entry.name), depth + 1, repositories))
  );
}

export async function listGitRepositories(forceRefresh = false): Promise<GitRepository[]> {
  if (!forceRefresh && repositoryCache && repositoryCache.expiresAt > Date.now()) {
    return mergeRepositories(repositoryCache.repositories, [...pinnedRepositories.values()]);
  }

  const repositories: GitRepository[] = [];
  const roots = await getAllowedGitRoots();

  for (const rootPath of roots) {
    await walkRepositories(rootPath, rootPath, 0, repositories);
  }

  const deduped = [...new Map(repositories.map((repository) => [repository.path, repository])).values()];
  const withBranches = await Promise.all(
    deduped.map(async (repository) => ({
      ...repository,
      currentBranch: await readCurrentBranch(repository.path).catch(() => null)
    }))
  );

  const merged = mergeRepositories(withBranches, [...pinnedRepositories.values()]);
  repositoryCache = {
    expiresAt: Date.now() + REPO_CACHE_TTL_MS,
    repositories: merged
  };
  return merged;
}

export async function resolveGitRepository(repoPath: string) {
  if (!repoPath) {
    throw error(400, "repoPath is required.");
  }

  const normalized = await realPathSafe(repoPath);
  const repository = (await listGitRepositories()).find((candidate) => candidate.path === normalized);
  if (!repository) {
    throw error(404, "The selected repository was not found within allowed roots.");
  }
  return repository;
}

export async function listGitBranches(repoPath: string): Promise<GitBranch[]> {
  const repository = await resolveGitRepository(repoPath);
  const output = await runGitText(repository.path, ["for-each-ref", "refs/heads", "--format=%(refname:short)\t%(HEAD)\t%(upstream:short)"]);
  return output
    .split("\n")
    .map((line: string) => line.trim())
    .filter(Boolean)
    .map((line: string) => {
      const [name, current, upstream] = line.split("\t");
      return {
        name,
        current: current === "*",
        upstream: upstream || null
      } satisfies GitBranch;
    });
}

export async function listGitCommits(repoPath: string, limit = 12): Promise<GitCommit[]> {
  const repository = await resolveGitRepository(repoPath);
  const output = await runGitText(repository.path, [
    "log",
    `--max-count=${limit}`,
    "--pretty=format:%H%x09%h%x09%an%x09%aI%x09%s"
  ]);
  return output
    .split("\n")
    .map((line: string) => line.trim())
    .filter(Boolean)
    .map((line: string) => {
      const [hash, shortHash, author, authoredAt, subject] = line.split("\t");
      return {
        hash,
        shortHash,
        author,
        authoredAt,
        subject
      } satisfies GitCommit;
    });
}

export async function getGitStatus(repoPath: string): Promise<GitStatusPayload> {
  const repository = await resolveGitRepository(repoPath);
  const output = await runGitText(repository.path, ["status", "--porcelain=v1", "--branch"]);
  const lines = output
    .split("\n")
    .map((line: string) => line.replace(/\r/u, ""))
    .filter(Boolean);
  const header = lines.find((line: string) => line.startsWith("## ")) ?? "## HEAD";
  const files = lines.filter((line: string) => !line.startsWith("## ")).map(parseStatusLine);
  const { branch, ahead, behind } = parseStatusHeader(header);

  return {
    repo: {
      ...repository,
      currentBranch: branch
    },
    branch,
    ahead,
    behind,
    clean: files.length === 0,
    files,
    branches: await listGitBranches(repository.path),
    commits: await listGitCommits(repository.path)
  };
}

export async function listGitWorktrees(repoPath: string) {
  const repository = await resolveGitRepository(repoPath);
  const output = await runGitText(repository.path, ["worktree", "list", "--porcelain"]);
  const worktrees: GitWorktree[] = [];
  let current: GitWorktree | null = null;

  for (const rawLine of output.split("\n")) {
    const line = rawLine.trimEnd();
    if (!line) {
      if (current) {
        worktrees.push(current);
        current = null;
      }
      continue;
    }

    if (line.startsWith("worktree ")) {
      if (current) {
        worktrees.push(current);
      }
      current = {
        path: line.slice("worktree ".length),
        branch: null,
        head: null,
        bare: false,
        detached: false,
        locked: false,
        prunable: false,
        current: line.slice("worktree ".length) === repository.path
      };
      continue;
    }

    if (!current) {
      continue;
    }

    if (line.startsWith("HEAD ")) {
      current.head = line.slice("HEAD ".length);
    } else if (line.startsWith("branch ")) {
      current.branch = line.slice("branch refs/heads/".length);
    } else if (line === "bare") {
      current.bare = true;
    } else if (line === "detached") {
      current.detached = true;
    } else if (line.startsWith("locked")) {
      current.locked = true;
    } else if (line.startsWith("prunable")) {
      current.prunable = true;
    }
  }

  if (current) {
    worktrees.push(current);
  }

  return worktrees;
}

export async function createGitWorktree(
  repoPath: string,
  worktreePath: string,
  branchName: string | null,
  createBranch: boolean,
  detach: boolean
) {
  const repository = await resolveGitRepository(repoPath);
  const resolvedWorktreePath = await ensureWorktreePathAllowed(worktreePath);
  const trimmedBranchName = branchName?.trim() || null;
  if (!detach && !trimmedBranchName) {
    throw error(400, "Provide a branch name or create a detached worktree.");
  }
  const args = ["worktree", "add"];

  if (detach) {
    args.push("--detach");
  } else if (createBranch && trimmedBranchName) {
    args.push("-b", trimmedBranchName);
  }

  args.push(resolvedWorktreePath);

  if (!detach && trimmedBranchName && !createBranch) {
    args.push(trimmedBranchName);
  }

  await runGitText(repository.path, args);
  repositoryCache = null;
  const roots = await getAllowedGitRoots();
  await rememberRepository(resolvedWorktreePath, roots).catch(() => null);
  return {
    repoPath: repository.path,
    worktrees: await listGitWorktrees(repository.path)
  };
}

export async function removeGitWorktree(repoPath: string, worktreePath: string, force = false) {
  const repository = await resolveGitRepository(repoPath);
  const resolvedWorktreePath = await ensureWorktreePathAllowed(worktreePath);
  await runGitText(repository.path, force ? ["worktree", "remove", "--force", resolvedWorktreePath] : ["worktree", "remove", resolvedWorktreePath]);
  repositoryCache = null;
  pinnedRepositories.delete(resolvedWorktreePath);
  return {
    repoPath: repository.path,
    worktrees: await listGitWorktrees(repository.path)
  };
}

export async function getGitFile(repoPath: string, filePath: string): Promise<GitFilePayload> {
  const repository = await resolveGitRepository(repoPath);
  const statusPayload = await getGitStatus(repository.path);
  const status = statusPayload.files.find((entry: GitFileStatus) => entry.path === filePath) ?? null;
  const targetPath = resolveRepoFilePath(repository.path, filePath);
  const modified = await readFileContent(targetPath);
  const original = await readHeadContent(repository.path, status?.originalPath ?? filePath);

  return {
    repoPath: repository.path,
    filePath,
    originalPath: status?.originalPath ?? null,
    originalContent: original.content,
    modifiedContent: modified.content,
    language: inferLanguage(filePath),
    isBinary: original.isBinary || modified.isBinary,
    status
  };
}

export async function resolveGitFileFromAbsolutePath(filePath: string) {
  const normalized = await realPathSafe(filePath);
  const targetStats = await fsp.stat(normalized).catch(() => null);
  const roots = await getAllowedGitRoots();
  const repositories = await listGitRepositories();
  const repository = [...repositories]
    .sort((left, right) => right.path.length - left.path.length)
    .find((candidate) => isPathInside(candidate.path, normalized));

  const resolvedRepository = repository ?? (await findAncestorRepository(normalized, roots)) ?? (await findDescendantRepository(normalized, roots));

  if (!resolvedRepository) {
    throw error(404, "The selected path could not be mapped to a Git repository within allowed roots.");
  }

  const relativePath = path.relative(resolvedRepository.path, normalized);

  return {
    repoPath: resolvedRepository.path,
    filePath: !targetStats?.isDirectory() && relativePath && !relativePath.startsWith("..") ? normalizeGitPath(relativePath) : null
  };
}

export async function saveGitFile(repoPath: string, filePath: string, content: string) {
  const repository = await resolveGitRepository(repoPath);
  const targetPath = resolveRepoFilePath(repository.path, filePath);
  await fsp.mkdir(path.dirname(targetPath), { recursive: true });
  await fsp.writeFile(targetPath, content, "utf8");
  return getGitFile(repository.path, filePath);
}

export async function stageGitChanges(repoPath: string, filePath: string | null) {
  const repository = await resolveGitRepository(repoPath);
  await runGitText(repository.path, filePath ? ["add", "--", filePath] : ["add", "-A"]);
  return getGitStatus(repository.path);
}

export async function unstageGitChanges(repoPath: string, filePath: string | null) {
  const repository = await resolveGitRepository(repoPath);
  await runGitText(repository.path, filePath ? ["restore", "--staged", "--", filePath] : ["restore", "--staged", "."]);
  return getGitStatus(repository.path);
}

export async function commitGitChanges(repoPath: string, message: string) {
  const repository = await resolveGitRepository(repoPath);
  if (!message.trim()) {
    throw error(400, "Commit message is required.");
  }
  await runGitText(repository.path, ["commit", "-m", message.trim()]);
  return getGitStatus(repository.path);
}

export async function checkoutGitBranch(repoPath: string, branchName: string, create = false) {
  const repository = await resolveGitRepository(repoPath);
  if (!branchName.trim()) {
    throw error(400, "branchName is required.");
  }
  await runGitText(repository.path, create ? ["switch", "-c", branchName.trim()] : ["switch", branchName.trim()]);
  repositoryCache = null;
  return getGitStatus(repository.path);
}
