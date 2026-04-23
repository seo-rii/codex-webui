const CACHE_NAME = "codex-webui-shell-__CODEX_WEBUI_APP_VERSION__";

function getScopePath() {
  const pathname = new URL(self.registration.scope).pathname.replace(/\/+$/u, "");
  return pathname ? `${pathname}/` : "/";
}

function getAppShellUrl() {
  return new URL(getScopePath(), self.location.origin).toString();
}

function isNavigationRequest(request) {
  if (request.mode === "navigate") {
    return true;
  }

  const accept = request.headers.get("accept") ?? "";
  return request.method === "GET" && request.destination === "" && accept.includes("text/html");
}

function isStaticAssetRequest(request) {
  return ["script", "style", "image", "font", "manifest"].includes(request.destination);
}

function isDynamicEndpoint(url) {
  return url.pathname.includes("/api/") || url.pathname.endsWith("/ws") || url.pathname.endsWith("/_app/version.json");
}

self.addEventListener("install", (event) => {
  self.skipWaiting();
  event.waitUntil(
    caches.open(CACHE_NAME).then((cache) =>
      cache.addAll([
        getAppShellUrl(),
        new URL("manifest.webmanifest", self.registration.scope).toString(),
        new URL("apple-touch-icon.png", self.registration.scope).toString(),
        new URL("icon-192.png", self.registration.scope).toString(),
        new URL("icon-512.png", self.registration.scope).toString()
      ]).catch(() => {})
    )
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches.keys().then((keys) =>
      Promise.all(
        keys.map((key) => {
          if (key === CACHE_NAME) {
            return Promise.resolve();
          }
          return caches.delete(key);
        })
      )
    ).then(() => self.clients.claim())
  );
});

self.addEventListener("fetch", (event) => {
  const { request } = event;
  if (request.method !== "GET") {
    return;
  }

  const url = new URL(request.url);
  if (url.origin !== self.location.origin) {
    return;
  }

  if (!url.pathname.startsWith(getScopePath()) || isDynamicEndpoint(url)) {
    return;
  }

  if (isNavigationRequest(request)) {
    event.respondWith(
      caches.open(CACHE_NAME).then(async (cache) => {
        try {
          const response = await fetch(request, { cache: "no-store" });
          if (response.ok) {
            await cache.put(getAppShellUrl(), response.clone());
          }
          return response;
        } catch {
          return (await cache.match(request)) || (await cache.match(getAppShellUrl())) || Response.error();
        }
      })
    );
    return;
  }

  if (!isStaticAssetRequest(request)) {
    return;
  }

  event.respondWith(
    caches.open(CACHE_NAME).then(async (cache) => {
      const isShellMetadata =
        url.pathname.endsWith("/service-worker.js") ||
        url.pathname.endsWith("/manifest.webmanifest") ||
        url.pathname.endsWith("/_app/env.js");
      const cached = isShellMetadata ? null : await cache.match(request);
      const networkPromise = fetch(request)
        .then(async (response) => {
          if (response.ok && !isShellMetadata) {
            await cache.put(request, response.clone());
          }
          return response;
        })
        .catch(() => null);

      if (cached) {
        void networkPromise;
        return cached;
      }

      return (await networkPromise) || Response.error();
    })
  );
});
