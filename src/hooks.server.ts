import { base } from "$app/paths";
import { error, type Handle } from "@sveltejs/kit";

import { getTextDirection } from "$lib/paraglide/runtime.js";
import { paraglideMiddleware } from "$lib/paraglide/server.js";
import { isAuthenticated } from "$lib/server/auth";
import { getRuntimeConfig, getRuntimeProfile } from "$lib/server/env";
import { runWithProfile } from "$lib/server/profile-context";

const PUBLIC_API_PATHS = new Set(["/api/auth/login", "/api/auth/logout", "/api/auth/session"]);
const SAFE_METHODS = new Set(["GET", "HEAD", "OPTIONS"]);
const LOOPBACK_HOSTS = new Set(["localhost", "127.0.0.1", "::1", "[::1]"]);
const CORS_METHODS = "GET,HEAD,POST,PATCH,PUT,DELETE,OPTIONS";
const PROFILE_COOKIE = "codex_webui_profile";
const PROFILE_HEADER = "x-codex-webui-profile-id";

function normalizeHostname(hostname: string) {
  return hostname.replace(/^\[|\]$/gu, "").replace(/^::ffff:/u, "");
}

function isSameOrigin(candidate: string, target: string) {
  try {
    const candidateUrl = new URL(candidate);
    const targetUrl = new URL(target);
    const candidateHostname = normalizeHostname(candidateUrl.hostname);
    const targetHostname = normalizeHostname(targetUrl.hostname);

    if (
      candidateUrl.protocol === targetUrl.protocol &&
      candidateUrl.port === targetUrl.port &&
      candidateHostname === targetHostname
    ) {
      return true;
    }

    return (
      candidateUrl.protocol === targetUrl.protocol &&
      candidateUrl.port === targetUrl.port &&
      LOOPBACK_HOSTS.has(candidateHostname) &&
      LOOPBACK_HOSTS.has(targetHostname)
    );
  } catch {
    return false;
  }
}

function normalizeOrigin(value: string) {
  try {
    return new URL(value).origin;
  } catch {
    return null;
  }
}

function isCorsAllowedOrigin(origin: string | null) {
  if (!origin) {
    return false;
  }

  return getRuntimeConfig().corsAllowedOrigins.includes(origin);
}

function isTrustedRequestSource(candidate: string, target: string) {
  return isSameOrigin(candidate, target) || isCorsAllowedOrigin(normalizeOrigin(candidate));
}

function stripBase(pathname: string) {
  if (!base) {
    return pathname;
  }

  if (pathname === base) {
    return "/";
  }

  if (pathname.startsWith(`${base}/`)) {
    return pathname.slice(base.length) || "/";
  }

  return pathname;
}

function appendVaryHeader(headers: Headers, value: string) {
  const current = headers.get("vary");
  if (!current) {
    headers.set("vary", value);
    return;
  }

  const values = new Set(
    current
      .split(",")
      .map((entry) => entry.trim())
      .filter(Boolean)
  );
  values.add(value);
  headers.set("vary", [...values].join(", "));
}

function applyCorsHeaders(headers: Headers, origin: string, requestHeaders: string | null) {
  headers.set("access-control-allow-origin", origin);
  headers.set("access-control-allow-credentials", "true");
  headers.set("access-control-allow-methods", CORS_METHODS);
  headers.set("access-control-max-age", "600");
  if (requestHeaders?.trim()) {
    headers.set("access-control-allow-headers", requestHeaders);
    appendVaryHeader(headers, "Access-Control-Request-Headers");
  }
  appendVaryHeader(headers, "Origin");
}

export const handle: Handle = async ({ event, resolve }) => {
  return paraglideMiddleware(
    event.request,
    async ({ locale }) => {
      event.locals.locale = locale;
      event.locals.textDirection = getTextDirection(locale);

      const internalProxyToken = process.env.CODEX_WEBUI_INTERNAL_PROXY_TOKEN;
      const isInternalRequest =
        Boolean(internalProxyToken) && event.request.headers.get("x-codex-webui-internal-token") === internalProxyToken;
      const requestedProfileId =
        event.request.headers.get(PROFILE_HEADER) ?? event.cookies.get(PROFILE_COOKIE) ?? getRuntimeConfig().defaultProfileId;
      const activeProfile = getRuntimeProfile(requestedProfileId);
      event.locals.profileId = activeProfile.id;

      event.locals.authenticated = isInternalRequest || isAuthenticated(event.cookies);

      const resolveWithProfile = () =>
        runWithProfile(activeProfile.id, () =>
          resolve(event, {
            transformPageChunk: ({ html }) =>
              html.replace("%lang%", locale).replace("%dir%", event.locals.textDirection)
          })
        );

      if (isInternalRequest) {
        return resolveWithProfile();
      }

      const routePath = stripBase(event.url.pathname);
      const isApiRoute = routePath.startsWith("/api/");
      const isPublicApiPath = PUBLIC_API_PATHS.has(routePath);
      const requestOrigin = normalizeOrigin(event.request.headers.get("origin") ?? "");
      const corsOrigin = isCorsAllowedOrigin(requestOrigin) ? requestOrigin : null;
      const requestedCorsHeaders = event.request.headers.get("access-control-request-headers");

      if (isApiRoute && event.request.method === "OPTIONS" && event.request.headers.get("access-control-request-method")) {
        if (!corsOrigin) {
          return new Response("CORS origin is not allowed.", { status: 403 });
        }

        const response = new Response(null, { status: 204 });
        applyCorsHeaders(response.headers, corsOrigin, requestedCorsHeaders);
        return response;
      }

      if (!SAFE_METHODS.has(event.request.method) && isApiRoute) {
        const origin = event.request.headers.get("origin");
        const referer = event.request.headers.get("referer");
        const allowedByOrigin = origin ? isTrustedRequestSource(origin, event.url.origin) : false;
        const allowedByReferer = referer ? isTrustedRequestSource(referer, event.url.origin) : false;

        if ((origin || referer) && !allowedByOrigin && !allowedByReferer) {
          throw error(403, "Cross-origin requests are not allowed.");
        }
      }

      if (isApiRoute && !isPublicApiPath && !event.locals.authenticated) {
        const response = new Response("Authentication required.", { status: 401 });
        if (corsOrigin) {
          applyCorsHeaders(response.headers, corsOrigin, requestedCorsHeaders);
        }
        return response;
      }

      const response = await resolveWithProfile();
      if (isApiRoute && corsOrigin) {
        applyCorsHeaders(response.headers, corsOrigin, requestedCorsHeaders);
      }
      return response;
    },
    {
      effectiveRequestUrl: event.url
    }
  );
};
