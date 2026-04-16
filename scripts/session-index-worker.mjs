import fs from "node:fs/promises";
import path from "node:path";
import readline from "node:readline";
import { createReadStream } from "node:fs";
import { parentPort } from "node:worker_threads";

function asObject(value) {
  return value && typeof value === "object" ? value : {};
}

function normalizeText(value) {
  if (typeof value !== "string") {
    return "";
  }
  return value.replace(/\s+/g, " ").trim();
}

function isPlaceholderTitle(value) {
  const normalized = normalizeText(value);
  return !normalized || normalized === "New thread";
}

function inferTitle(text) {
  const normalized = normalizeText(text);
  if (!normalized) {
    return null;
  }

  let candidate =
    normalized.split(/\r?\n/u, 1)[0]?.split(/(?<=[.?!])\s+/u, 1)[0]?.split(/\s[-:|]\s/u, 1)[0]?.trim() ?? normalized;

  candidate = candidate
    .replace(/^[#>*`\-.\d()\[\]\s]+/u, "")
    .replace(/\s+/g, " ")
    .replace(
      /(해줘|해주세요|해 줘|고쳐줘|고쳐 줘|수정해줘|수정해 줘|추가해줘|추가해 줘|구현해줘|구현해 줘|만들어줘|만들어 줘|계속 작업해|계속 진행해|계속해|부탁해|please|can you|could you|help me)\s*$/iu,
      ""
    )
    .replace(/[.?!…。]+$/u, "")
    .trim();

  if (!candidate) {
    candidate = normalized;
  }

  return candidate.length > 60 ? `${candidate.slice(0, 60).trimEnd()}...` : candidate;
}

async function listSessionFiles(rootDir) {
  const results = [];
  const queue = [rootDir];
  while (queue.length > 0) {
    const current = queue.shift();
    if (!current) {
      continue;
    }
    let entries;
    try {
      entries = await fs.readdir(current, { withFileTypes: true });
    } catch {
      continue;
    }
    for (const entry of entries) {
      const nextPath = path.join(current, entry.name);
      if (entry.isDirectory()) {
        queue.push(nextPath);
        continue;
      }
      if (entry.isFile() && entry.name.endsWith(".jsonl")) {
        results.push(nextPath);
      }
    }
  }
  return results;
}

async function parseSessionFile(filePath, stat = null) {
  const resolvedStat = stat ?? (await fs.stat(filePath));
  const stream = createReadStream(filePath, { encoding: "utf8" });
  const rl = readline.createInterface({ input: stream, crlfDelay: Infinity });

  let sessionId = null;
  let cwd = null;
  let createdAt = null;
  let preview = "";
  let explicitName = null;
  let isSubagent = false;
  let lineCount = 0;

  try {
    for await (const line of rl) {
      lineCount += 1;
      if (!line.trim()) {
        continue;
      }

      let entry;
      try {
        entry = JSON.parse(line);
      } catch {
        continue;
      }

      if (entry?.type === "session_meta") {
        const payload = asObject(entry.payload);
        sessionId = typeof payload.id === "string" ? payload.id : sessionId;
        cwd = typeof payload.cwd === "string" ? payload.cwd : cwd;
        const source = asObject(payload.source);
        const subagent = asObject(source.subagent);
        isSubagent =
          isSubagent ||
          Object.keys(subagent).length > 0 ||
          normalizeText(payload.agent_nickname).length > 0 ||
          normalizeText(payload.agent_role).length > 0;
        createdAt =
          typeof payload.timestamp === "string" && payload.timestamp
            ? Math.floor(new Date(payload.timestamp).getTime() / 1000)
            : createdAt;
      }

      const payload = asObject(entry?.payload);
      if (!preview && payload.type === "user_message") {
        preview = normalizeText(payload.message);
      }

      if (!explicitName && entry?.type === "event_msg" && payload.type === "thread_name_updated") {
        explicitName = normalizeText(payload.thread_name || payload.threadName);
      }

      if (sessionId && preview && lineCount >= 32) {
        break;
      }
      if (lineCount >= 256) {
        break;
      }
    }
  } finally {
    rl.close();
    stream.destroy();
  }

  if (!sessionId) {
    const match = filePath.match(/([0-9a-f]{8}-[0-9a-f-]{27,})/i);
    sessionId = match ? match[1] : path.basename(filePath, ".jsonl");
  }

  return {
    id: sessionId,
    name: !isPlaceholderTitle(explicitName) ? explicitName : inferTitle(preview),
    preview,
    cwd: cwd || "",
    isSubagent,
    createdAt: createdAt || Math.floor(resolvedStat.birthtimeMs / 1000) || Math.floor(resolvedStat.mtimeMs / 1000),
    updatedAt: Math.floor(resolvedStat.mtimeMs / 1000),
    status: "unknown"
  };
}

let cache = {
  key: "",
  loadedAt: 0,
  entries: []
};
let sessionFileCache = new Map();

async function loadIndex(codexHome) {
  const cacheKey = codexHome;
  if (cache.key === cacheKey && Date.now() - cache.loadedAt < 5000) {
    return cache.entries;
  }

  const rootDir = path.join(codexHome, "sessions");
  const files = await listSessionFiles(rootDir);
  const activeFiles = new Set(files);
  for (const cachedPath of sessionFileCache.keys()) {
    if (!activeFiles.has(cachedPath)) {
      sessionFileCache.delete(cachedPath);
    }
  }

  const parsedEntries = await Promise.all(
    files.map(async (filePath) => {
      let stat;
      try {
        stat = await fs.stat(filePath);
      } catch {
        sessionFileCache.delete(filePath);
        return null;
      }

      const cached = sessionFileCache.get(filePath);
      if (cached && cached.mtimeMs === stat.mtimeMs && cached.size === stat.size) {
        return cached.entry;
      }

      const entry = await parseSessionFile(filePath, stat);
      sessionFileCache.set(filePath, {
        mtimeMs: stat.mtimeMs,
        size: stat.size,
        entry
      });
      return entry;
    })
  );
  const entryMap = new Map();
  for (const entry of parsedEntries) {
    if (!entry) {
      continue;
    }
    const current = entryMap.get(entry.id);
    if (!current) {
      entryMap.set(entry.id, entry);
      continue;
    }

    entryMap.set(entry.id, {
      id: entry.id,
      name:
        !isPlaceholderTitle(current.name)
          ? current.name
          : !isPlaceholderTitle(entry.name)
            ? entry.name
            : null,
      preview:
        current.preview && current.preview.trim()
          ? current.preview
          : entry.preview && entry.preview.trim()
            ? entry.preview
            : "",
      cwd: current.cwd || entry.cwd || "",
      isSubagent: Boolean(current.isSubagent || entry.isSubagent),
      createdAt:
        current.createdAt && entry.createdAt
          ? Math.min(current.createdAt, entry.createdAt)
          : current.createdAt || entry.createdAt,
      updatedAt: Math.max(current.updatedAt || 0, entry.updatedAt || 0),
      status:
        current.status && current.status !== "unknown"
          ? current.status
          : entry.status || "unknown"
    });
  }

  const entries = [...entryMap.values()]
    .filter((entry) => !entry.isSubagent)
    .sort((left, right) => right.updatedAt - left.updatedAt);

  cache = {
    key: cacheKey,
    loadedAt: Date.now(),
    entries
  };
  return entries;
}

function sliceEntries(entries, cursor, limit) {
  const start = Math.max(0, Number.parseInt(String(cursor ?? "0"), 10) || 0);
  const numericLimit = Number(limit);
  const pageSize = Number.isFinite(numericLimit) && numericLimit > 0 ? Math.max(1, Math.floor(numericLimit)) : 20;
  const nextIndex = start + pageSize;
  return {
    entries: entries.slice(start, nextIndex),
    nextCursor: nextIndex < entries.length ? String(nextIndex) : null
  };
}

parentPort?.on("message", async (message) => {
  const { id, method, params } = message ?? {};
  if (!id || (method !== "session-index/list" && method !== "session-index/page")) {
    return;
  }

  try {
    const entries = await loadIndex(String(params?.codexHome || ""));
    const query = normalizeText(String(params?.query || "")).toLowerCase();
    const filteredEntries = query
      ? entries.filter((entry) => `${normalizeText(entry.name)}\n${normalizeText(entry.preview)}`.toLowerCase().includes(query))
      : entries;
    const page = method === "session-index/page" ? sliceEntries(filteredEntries, params?.cursor, params?.limit) : {
      entries: filteredEntries,
      nextCursor: null
    };
    parentPort?.postMessage({
      id,
      ok: true,
      result: {
        entries: page.entries,
        nextCursor: page.nextCursor
      }
    });
  } catch (error) {
    parentPort?.postMessage({
      id,
      ok: false,
      error: error instanceof Error ? error.message : "Failed to build session index."
    });
  }
});
