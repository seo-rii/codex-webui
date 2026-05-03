import { base } from "$app/paths";
import { clearSessionBrowserCache } from "$lib/session-browser-cache";

const APP_VERSION_STORAGE_KEY = "codex-webui.app-version";
const APP_VERSION_FLUSH_STORAGE_KEY = "codex-webui.app-version-flush";

type AppVersionPayload = {
  version?: string;
};

function appVersionUrl() {
  const prefix = base || "";
  return `${prefix}/_app/version.json?cwui_v=${Date.now()}`;
}

function normalizeVersionPayload(payload: AppVersionPayload) {
  return String(payload.version ?? "").trim();
}

async function readCurrentAppVersion() {
  const response = await fetch(appVersionUrl(), {
    cache: "no-store",
    credentials: "include",
    headers: {
      accept: "application/json"
    }
  });
  if (!response.ok) {
    return null;
  }

  const payload = (await response.json()) as AppVersionPayload;
  const version = normalizeVersionPayload(payload);
  return version || null;
}

async function clearControlledCacheStorage() {
  if (!("caches" in window)) {
    return;
  }

  try {
    const keys = await window.caches.keys();
    await Promise.all(keys.filter((key) => key.startsWith("codex-webui-")).map((key) => window.caches.delete(key)));
  } catch {
    // CacheStorage is best-effort and may be blocked in hardened browser profiles.
  }
}

async function unregisterScopedServiceWorkers() {
  if (!("serviceWorker" in navigator)) {
    return;
  }

  try {
    const scopePrefix = new URL(base ? `${base}/` : "/", window.location.origin).toString();
    const registrations = await navigator.serviceWorker.getRegistrations();
    await Promise.all(
      registrations
        .filter((registration) => registration.scope.startsWith(scopePrefix))
        .map((registration) => registration.unregister())
    );
  } catch {
    // A failed unregister should not block the hard reload fallback.
  }
}

async function flushAppCachesForVersion(nextVersion: string) {
  window.sessionStorage.setItem(APP_VERSION_FLUSH_STORAGE_KEY, nextVersion);
  window.localStorage.setItem(APP_VERSION_STORAGE_KEY, nextVersion);
  await Promise.all([clearControlledCacheStorage(), clearSessionBrowserCache(), unregisterScopedServiceWorkers()]);
  window.location.reload();
}

async function checkAppVersionOnce() {
  const nextVersion = await readCurrentAppVersion();
  if (!nextVersion) {
    return;
  }

  const previousVersion = window.localStorage.getItem(APP_VERSION_STORAGE_KEY);
  if (!previousVersion) {
    window.localStorage.setItem(APP_VERSION_STORAGE_KEY, nextVersion);
    return;
  }

  if (previousVersion === nextVersion) {
    window.sessionStorage.removeItem(APP_VERSION_FLUSH_STORAGE_KEY);
    return;
  }

  if (window.sessionStorage.getItem(APP_VERSION_FLUSH_STORAGE_KEY) === nextVersion) {
    window.localStorage.setItem(APP_VERSION_STORAGE_KEY, nextVersion);
    return;
  }

  await flushAppCachesForVersion(nextVersion);
}

export function startAppVersionGuard() {
  if (typeof window === "undefined") {
    return () => {};
  }

  let disposed = false;
  let checking = false;
  let interval: number | undefined;

  const run = () => {
    if (disposed || checking) {
      return;
    }
    checking = true;
    void checkAppVersionOnce()
      .catch(() => {
        // Version checks should never interrupt app startup or websocket recovery.
      })
      .finally(() => {
        checking = false;
      });
  };

  run();
  interval = window.setInterval(run, 60_000);

  const handleVisibilityChange = () => {
    if (document.visibilityState === "visible") {
      run();
    }
  };
  window.addEventListener("focus", run);
  document.addEventListener("visibilitychange", handleVisibilityChange);

  return () => {
    disposed = true;
    if (interval !== undefined) {
      window.clearInterval(interval);
    }
    window.removeEventListener("focus", run);
    document.removeEventListener("visibilitychange", handleVisibilityChange);
  };
}
