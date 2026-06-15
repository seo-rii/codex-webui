import type { SessionDetailPayload, SessionListPayload } from "$lib/types";

type CacheStoreName = "session-details" | "session-lists";

type CacheEnvelope<T> = {
  key: string;
  payload: T;
  version: string | null;
  savedAt: number;
};

const DB_NAME = "codex-webui-browser-cache";
const DB_VERSION = 1;
const BROWSER_CACHE_OPERATION_TIMEOUT_MS = 1_500;
const PERSISTENT_PRUNE_INTERVAL_MS = 30_000;
const STORE_LIMITS: Record<CacheStoreName, { maxEntries: number; ttlMs: number }> = {
  "session-details": {
    maxEntries: 80,
    ttlMs: 24 * 60 * 60 * 1000
  },
  "session-lists": {
    maxEntries: 40,
    ttlMs: 10 * 60 * 1000
  }
};
const memoryStores: Record<CacheStoreName, Map<string, CacheEnvelope<unknown>>> = {
  "session-details": new Map(),
  "session-lists": new Map()
};
const persistentPruneTimers: Record<CacheStoreName, ReturnType<typeof setTimeout> | null> = {
  "session-details": null,
  "session-lists": null
};
const lastPersistentPruneAt: Record<CacheStoreName, number> = {
  "session-details": 0,
  "session-lists": 0
};

let databasePromise: Promise<IDBDatabase> | null = null;

function isExpiredEnvelope(storeName: CacheStoreName, envelope: CacheEnvelope<unknown>) {
  return Date.now() - envelope.savedAt > STORE_LIMITS[storeName].ttlMs;
}

function pruneMemoryStore(storeName: CacheStoreName) {
  const store = memoryStores[storeName];
  for (const [key, envelope] of store) {
    if (isExpiredEnvelope(storeName, envelope)) {
      store.delete(key);
    }
  }

  const maxEntries = STORE_LIMITS[storeName].maxEntries;
  if (store.size <= maxEntries) {
    return;
  }

  const keysToDelete = [...store.entries()]
    .sort((left, right) => left[1].savedAt - right[1].savedAt)
    .slice(0, store.size - maxEntries)
    .map(([key]) => key);
  for (const key of keysToDelete) {
    store.delete(key);
  }
}

function browserSupportsIndexedDb() {
  return typeof window !== "undefined" && typeof window.indexedDB !== "undefined";
}

async function withBrowserCacheTimeout<T>(operation: Promise<T>) {
  let timer: ReturnType<typeof setTimeout> | null = null;
  try {
    return await Promise.race([
      operation,
      new Promise<T>((_, reject) => {
        timer = setTimeout(() => {
          reject(new Error("Browser cache operation timed out."));
        }, BROWSER_CACHE_OPERATION_TIMEOUT_MS);
      })
    ]);
  } finally {
    if (timer) {
      clearTimeout(timer);
    }
  }
}

function openDatabase() {
  if (!browserSupportsIndexedDb()) {
    return Promise.reject(new Error("IndexedDB is unavailable."));
  }

  if (databasePromise) {
    return databasePromise;
  }

  databasePromise = new Promise<IDBDatabase>((resolve, reject) => {
    const request = window.indexedDB.open(DB_NAME, DB_VERSION);

    request.onupgradeneeded = () => {
      const database = request.result;
      if (!database.objectStoreNames.contains("session-lists")) {
        database.createObjectStore("session-lists", { keyPath: "key" });
      }
      if (!database.objectStoreNames.contains("session-details")) {
        database.createObjectStore("session-details", { keyPath: "key" });
      }
    };

    request.onsuccess = () => {
      resolve(request.result);
    };

    request.onerror = () => {
      reject(request.error ?? new Error("Failed to open browser cache."));
    };
  }).catch((error) => {
    databasePromise = null;
    throw error;
  });

  return databasePromise;
}

async function runStoreRequest<T>(
  storeName: CacheStoreName,
  mode: IDBTransactionMode,
  operation: (store: IDBObjectStore, resolve: (value: T) => void, reject: (error: unknown) => void) => void
) {
  if (!browserSupportsIndexedDb()) {
    throw new Error("IndexedDB is unavailable.");
  }

  const database = await openDatabase();
  return new Promise<T>((resolve, reject) => {
    const transaction = database.transaction(storeName, mode);
    const store = transaction.objectStore(storeName);

    transaction.onerror = () => {
      reject(transaction.error ?? new Error("Browser cache transaction failed."));
    };

    operation(store, resolve, reject);
  });
}

async function readEnvelope<T>(storeName: CacheStoreName, key: string) {
  pruneMemoryStore(storeName);
  const memory = memoryStores[storeName].get(key) as CacheEnvelope<T> | undefined;
  if (memory && !isExpiredEnvelope(storeName, memory as CacheEnvelope<unknown>)) {
    return memory;
  }
  if (memory) {
    memoryStores[storeName].delete(key);
  }

  try {
    const value = await withBrowserCacheTimeout(
      runStoreRequest<CacheEnvelope<T> | null>(storeName, "readonly", (store, resolve, reject) => {
        const request = store.get(key);
        request.onsuccess = () => {
          resolve((request.result as CacheEnvelope<T> | undefined) ?? null);
        };
        request.onerror = () => {
          reject(request.error ?? new Error("Browser cache read failed."));
        };
      })
    );
    if (value && !isExpiredEnvelope(storeName, value as CacheEnvelope<unknown>)) {
      memoryStores[storeName].set(key, value as CacheEnvelope<unknown>);
      return value;
    }
    if (value) {
      void deleteEnvelope(storeName, key);
    }
  } catch {
    return memory && !isExpiredEnvelope(storeName, memory as CacheEnvelope<unknown>) ? memory : null;
  }

  return memory && !isExpiredEnvelope(storeName, memory as CacheEnvelope<unknown>) ? memory : null;
}

async function deleteEnvelope(storeName: CacheStoreName, key: string) {
  memoryStores[storeName].delete(key);
  if (!browserSupportsIndexedDb()) {
    return;
  }

  try {
    await withBrowserCacheTimeout(
      runStoreRequest<void>(storeName, "readwrite", (store, resolve, reject) => {
        const request = store.delete(key);
        request.onsuccess = () => {
          resolve();
        };
        request.onerror = () => {
          reject(request.error ?? new Error("Browser cache delete failed."));
        };
      })
    );
  } catch {
    // Ignore cache delete failures.
  }
}

async function prunePersistentStore(storeName: CacheStoreName) {
  lastPersistentPruneAt[storeName] = Date.now();
  pruneMemoryStore(storeName);
  if (!browserSupportsIndexedDb()) {
    return;
  }

  try {
    await withBrowserCacheTimeout(
      runStoreRequest<void>(storeName, "readwrite", (store, resolve, reject) => {
        const request = store.getAll();
        request.onsuccess = () => {
          const entries = ((request.result as CacheEnvelope<unknown>[] | undefined) ?? []).filter(Boolean);
          const expiredKeys = entries.filter((entry) => isExpiredEnvelope(storeName, entry)).map((entry) => entry.key);
          const maxEntries = STORE_LIMITS[storeName].maxEntries;
          const overflowKeys =
            entries.length > maxEntries
              ? entries
                  .filter((entry) => !expiredKeys.includes(entry.key))
                  .sort((left, right) => left.savedAt - right.savedAt)
                  .slice(0, entries.length - maxEntries)
                  .map((entry) => entry.key)
              : [];
          for (const key of new Set([...expiredKeys, ...overflowKeys])) {
            store.delete(key);
          }
          resolve();
        };
        request.onerror = () => {
          reject(request.error ?? new Error("Browser cache prune failed."));
        };
      })
    );
  } catch {
    // Ignore cache prune failures.
  }
}

function schedulePersistentPrune(storeName: CacheStoreName) {
  if (persistentPruneTimers[storeName]) {
    return;
  }

  const elapsed = Date.now() - lastPersistentPruneAt[storeName];
  const delay = Math.max(0, PERSISTENT_PRUNE_INTERVAL_MS - elapsed);
  persistentPruneTimers[storeName] = setTimeout(() => {
    persistentPruneTimers[storeName] = null;
    void prunePersistentStore(storeName);
  }, delay);
}

async function writeEnvelope<T>(storeName: CacheStoreName, envelope: CacheEnvelope<T>) {
  memoryStores[storeName].set(envelope.key, envelope as CacheEnvelope<unknown>);
  pruneMemoryStore(storeName);

  try {
    await withBrowserCacheTimeout(
      runStoreRequest<void>(storeName, "readwrite", (store, resolve, reject) => {
        const request = store.put(envelope);
        request.onsuccess = () => {
          resolve();
        };
        request.onerror = () => {
          reject(request.error ?? new Error("Browser cache write failed."));
        };
      })
    );
  } catch {
    // Memory fallback already contains the latest value.
  }
  schedulePersistentPrune(storeName);
}

export async function readSessionListCache(key: string) {
  return readEnvelope<SessionListPayload>("session-lists", key);
}

export async function writeSessionListCache(key: string, payload: SessionListPayload, version: string | null) {
  await writeEnvelope("session-lists", {
    key,
    payload,
    version,
    savedAt: Date.now()
  });
}

export async function readSessionDetailCache(key: string) {
  return readEnvelope<SessionDetailPayload>("session-details", key);
}

export async function writeSessionDetailCache(key: string, payload: SessionDetailPayload, version: string | null) {
  await writeEnvelope("session-details", {
    key,
    payload,
    version,
    savedAt: Date.now()
  });
}

export async function clearSessionBrowserCache() {
  memoryStores["session-details"].clear();
  memoryStores["session-lists"].clear();

  if (!browserSupportsIndexedDb()) {
    return;
  }

  try {
    const database = await withBrowserCacheTimeout(openDatabase());
    await Promise.all(
      (["session-details", "session-lists"] as CacheStoreName[]).map(
        (storeName) =>
          new Promise<void>((resolve, reject) => {
            void withBrowserCacheTimeout(
              new Promise<void>((resolveOperation, rejectOperation) => {
                const transaction = database.transaction(storeName, "readwrite");
                const store = transaction.objectStore(storeName);
                const request = store.clear();
                request.onsuccess = () => {
                  resolveOperation();
                };
                request.onerror = () => {
                  rejectOperation(request.error ?? new Error("Browser cache clear failed."));
                };
              })
            )
              .then(resolve)
              .catch(reject);
          })
      )
    );
  } catch {
    // Ignore cache clear failures.
  }
}
