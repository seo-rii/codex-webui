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
const memoryStores: Record<CacheStoreName, Map<string, CacheEnvelope<unknown>>> = {
  "session-details": new Map(),
  "session-lists": new Map()
};

let databasePromise: Promise<IDBDatabase> | null = null;

function browserSupportsIndexedDb() {
  return typeof window !== "undefined" && typeof window.indexedDB !== "undefined";
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
  const memory = memoryStores[storeName].get(key) as CacheEnvelope<T> | undefined;

  try {
    const value = await runStoreRequest<CacheEnvelope<T> | null>(storeName, "readonly", (store, resolve, reject) => {
      const request = store.get(key);
      request.onsuccess = () => {
        resolve((request.result as CacheEnvelope<T> | undefined) ?? null);
      };
      request.onerror = () => {
        reject(request.error ?? new Error("Browser cache read failed."));
      };
    });
    if (value) {
      memoryStores[storeName].set(key, value as CacheEnvelope<unknown>);
      return value;
    }
  } catch {
    return memory ?? null;
  }

  return memory ?? null;
}

async function writeEnvelope<T>(storeName: CacheStoreName, envelope: CacheEnvelope<T>) {
  memoryStores[storeName].set(envelope.key, envelope as CacheEnvelope<unknown>);

  try {
    await runStoreRequest<void>(storeName, "readwrite", (store, resolve, reject) => {
      const request = store.put(envelope);
      request.onsuccess = () => {
        resolve();
      };
      request.onerror = () => {
        reject(request.error ?? new Error("Browser cache write failed."));
      };
    });
  } catch {
    // Memory fallback already contains the latest value.
  }
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
    const database = await openDatabase();
    await Promise.all(
      (["session-details", "session-lists"] as CacheStoreName[]).map(
        (storeName) =>
          new Promise<void>((resolve, reject) => {
            const transaction = database.transaction(storeName, "readwrite");
            const store = transaction.objectStore(storeName);
            const request = store.clear();
            request.onsuccess = () => {
              resolve();
            };
            request.onerror = () => {
              reject(request.error ?? new Error("Browser cache clear failed."));
            };
          })
      )
    );
  } catch {
    // Ignore cache clear failures.
  }
}
