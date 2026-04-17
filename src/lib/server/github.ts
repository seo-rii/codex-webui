import { execFile } from "node:child_process";
import { promisify } from "node:util";

import { error } from "@sveltejs/kit";

import type {
  GitHubPullRequestDetailPayload,
  GitHubPullRequestFile,
  GitHubPullRequestListPayload,
  GitHubPullRequestSummary,
  GitHubRepositoryInfo,
  GitStatusPayload
} from "$lib/types";

import { getGitStatus, resolveGitRepository } from "./git";

const execFileAsync = promisify(execFile);
const PR_LIST_CACHE_TTL_MS = 15_000;
const PR_DETAIL_CACHE_TTL_MS = 10_000;

const listCache = new Map<
  string,
  {
    expiresAt: number;
    payload: GitHubPullRequestListPayload;
  }
>();
const detailCache = new Map<
  string,
  {
    expiresAt: number;
    payload: GitHubPullRequestDetailPayload;
  }
>();

function parseGitHubRemote(remoteName: string, remoteUrl: string): GitHubRepositoryInfo | null {
  const trimmed = remoteUrl.trim();
  if (!trimmed) {
    return null;
  }

  const sshMatch = trimmed.match(/^git@([^:]+):([^/]+)\/(.+?)(?:\.git)?$/u);
  if (sshMatch) {
    return {
      host: sshMatch[1],
      owner: sshMatch[2],
      name: sshMatch[3],
      remoteName,
      url: `https://${sshMatch[1]}/${sshMatch[2]}/${sshMatch[3]}`
    };
  }

  const httpsMatch = trimmed.match(/^(?:https?|ssh):\/\/(?:git@)?([^/]+)\/([^/]+)\/(.+?)(?:\.git)?\/?$/u);
  if (httpsMatch) {
    return {
      host: httpsMatch[1],
      owner: httpsMatch[2],
      name: httpsMatch[3],
      remoteName,
      url: `https://${httpsMatch[1]}/${httpsMatch[2]}/${httpsMatch[3]}`
    };
  }

  return null;
}

async function runGit(repoPath: string, args: string[]) {
  try {
    const result = await execFileAsync("git", ["-C", repoPath, ...args], {
      encoding: "utf8",
      maxBuffer: 10 * 1024 * 1024
    });
    return result.stdout.trim();
  } catch (cause) {
    const stderr =
      cause && typeof cause === "object" && "stderr" in cause ? String((cause as { stderr?: string | Buffer }).stderr ?? "") : "";
    throw error(400, stderr.trim() || `git ${args[0] ?? "command"} failed.`);
  }
}

async function runGh(repoPath: string, args: string[]) {
  try {
    const result = await execFileAsync("gh", args, {
      cwd: repoPath,
      encoding: "utf8",
      maxBuffer: 20 * 1024 * 1024,
      env: {
        ...process.env,
        GH_PAGER: "cat",
        GH_PROMPT_DISABLED: "1",
        GIT_TERMINAL_PROMPT: "0",
        PAGER: "cat"
      }
    });
    return result.stdout.trim();
  } catch (cause) {
    if (cause && typeof cause === "object" && "code" in cause && (cause as { code?: string }).code === "ENOENT") {
      throw error(400, "GitHub CLI (gh) is not installed on the server.");
    }
    const stderr =
      cause && typeof cause === "object" && "stderr" in cause ? String((cause as { stderr?: string | Buffer }).stderr ?? "") : "";
    if (stderr.includes("executable file not found") || stderr.includes("not found")) {
      throw error(400, "GitHub CLI (gh) is not installed on the server.");
    }
    throw error(400, stderr.trim() || `gh ${args[0] ?? "command"} failed.`);
  }
}

async function resolveGitHubRepository(repoPath: string): Promise<GitHubRepositoryInfo> {
  const repository = await resolveGitRepository(repoPath);
  const remoteNames = (await runGit(repository.path, ["remote"]))
    .split("\n")
    .map((entry) => entry.trim())
    .filter(Boolean);

  for (const remoteName of ["origin", ...remoteNames.filter((entry) => entry !== "origin")]) {
    const remoteUrl = await runGit(repository.path, ["config", "--get", `remote.${remoteName}.url`]).catch(() => "");
    const parsed = parseGitHubRemote(remoteName, remoteUrl);
    if (parsed) {
      return parsed;
    }
  }

  throw error(400, "No GitHub remote was found for the selected repository.");
}

function mapPullRequestSummary(pullRequest: Record<string, unknown>): GitHubPullRequestSummary {
  const labels = Array.isArray(pullRequest.labels)
    ? pullRequest.labels
        .map((entry) => (entry && typeof entry === "object" && "name" in entry ? String((entry as { name: string }).name) : ""))
        .filter(Boolean)
    : [];
  const mergedAt = typeof pullRequest.merged_at === "string" ? pullRequest.merged_at : null;

  return {
    number: Number(pullRequest.number ?? 0),
    title: String(pullRequest.title ?? "Untitled PR"),
    state: mergedAt ? "merged" : String(pullRequest.state ?? "open") === "closed" ? "closed" : "open",
    isDraft: Boolean(pullRequest.draft),
    url: String(pullRequest.html_url ?? ""),
    author:
      pullRequest.user && typeof pullRequest.user === "object" && "login" in pullRequest.user
        ? String((pullRequest.user as { login: string }).login)
        : null,
    authorUrl:
      pullRequest.user && typeof pullRequest.user === "object" && "html_url" in pullRequest.user
        ? String((pullRequest.user as { html_url: string }).html_url)
        : null,
    baseRefName:
      pullRequest.base && typeof pullRequest.base === "object" && "ref" in pullRequest.base
        ? String((pullRequest.base as { ref: string }).ref)
        : "",
    headRefName:
      pullRequest.head && typeof pullRequest.head === "object" && "ref" in pullRequest.head
        ? String((pullRequest.head as { ref: string }).ref)
        : "",
    updatedAt: typeof pullRequest.updated_at === "string" ? pullRequest.updated_at : null,
    additions: Number(pullRequest.additions ?? 0),
    deletions: Number(pullRequest.deletions ?? 0),
    changedFiles: Number(pullRequest.changed_files ?? 0),
    labels
  };
}

function mapPullRequestFile(file: Record<string, unknown>): GitHubPullRequestFile {
  return {
    path: String(file.filename ?? ""),
    previousPath: typeof file.previous_filename === "string" ? file.previous_filename : null,
    status: String(file.status ?? "modified"),
    additions: Number(file.additions ?? 0),
    deletions: Number(file.deletions ?? 0),
    patch: typeof file.patch === "string" ? file.patch : null
  };
}

function getListCacheKey(repoPath: string, state: string, limit: number) {
  return `${repoPath}::${state}::${limit}`;
}

function getDetailCacheKey(repoPath: string, number: number) {
  return `${repoPath}::${number}`;
}

function invalidateGitHubCache(repoPath: string) {
  for (const key of listCache.keys()) {
    if (key.startsWith(`${repoPath}::`)) {
      listCache.delete(key);
    }
  }
  for (const key of detailCache.keys()) {
    if (key.startsWith(`${repoPath}::`)) {
      detailCache.delete(key);
    }
  }
}

export async function listGitHubPullRequests(
  repoPath: string,
  state: "open" | "closed" | "all" = "open",
  limit = 20
): Promise<GitHubPullRequestListPayload> {
  const normalizedLimit = Math.max(1, Math.min(50, Math.round(limit)));
  const cacheKey = getListCacheKey(repoPath, state, normalizedLimit);
  const cached = listCache.get(cacheKey);
  if (cached && cached.expiresAt > Date.now()) {
    return cached.payload;
  }

  const repository = await resolveGitHubRepository(repoPath);
  const payload = {
    repository,
    pullRequests: JSON.parse(
      await runGh(
        repoPath,
        [
          "api",
          `repos/${repository.owner}/${repository.name}/pulls?state=${encodeURIComponent(state)}&per_page=${normalizedLimit}`
        ]
      )
    ).map((pullRequest: Record<string, unknown>) => mapPullRequestSummary(pullRequest))
  } satisfies GitHubPullRequestListPayload;

  listCache.set(cacheKey, {
    expiresAt: Date.now() + PR_LIST_CACHE_TTL_MS,
    payload
  });
  return payload;
}

export async function getGitHubPullRequest(repoPath: string, number: number): Promise<GitHubPullRequestDetailPayload> {
  const pullRequestNumber = Math.max(1, Math.round(number));
  const cacheKey = getDetailCacheKey(repoPath, pullRequestNumber);
  const cached = detailCache.get(cacheKey);
  if (cached && cached.expiresAt > Date.now()) {
    return cached.payload;
  }

  const repository = await resolveGitHubRepository(repoPath);
  const pullRequest = JSON.parse(
    await runGh(repoPath, ["api", `repos/${repository.owner}/${repository.name}/pulls/${pullRequestNumber}`])
  ) as Record<string, unknown>;
  const files = JSON.parse(
    await runGh(
      repoPath,
      ["api", `repos/${repository.owner}/${repository.name}/pulls/${pullRequestNumber}/files?per_page=100`]
    )
  ) as Array<Record<string, unknown>>;

  const payload = {
    repository,
    pullRequest: {
      ...mapPullRequestSummary(pullRequest),
      body: typeof pullRequest.body === "string" ? pullRequest.body : "",
      reviewDecision: typeof pullRequest.review_decision === "string" ? pullRequest.review_decision : null,
      mergeStateStatus: typeof pullRequest.mergeable_state === "string" ? pullRequest.mergeable_state : null,
      commits: Number(pullRequest.commits ?? 0),
      files: files.map((file) => mapPullRequestFile(file))
    }
  } satisfies GitHubPullRequestDetailPayload;

  detailCache.set(cacheKey, {
    expiresAt: Date.now() + PR_DETAIL_CACHE_TTL_MS,
    payload
  });
  return payload;
}

export async function checkoutGitHubPullRequest(repoPath: string, number: number): Promise<GitStatusPayload> {
  const repository = await resolveGitRepository(repoPath);
  const pullRequestNumber = Math.max(1, Math.round(number));
  await runGh(repository.path, ["pr", "checkout", String(pullRequestNumber)]);
  invalidateGitHubCache(repository.path);
  return getGitStatus(repository.path);
}
