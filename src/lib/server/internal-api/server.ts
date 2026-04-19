import { createServer } from "node:http";
import { Readable } from "node:stream";

import { parseAppError } from "$lib/errors";

import { getRuntimeConfig, getRuntimeProfile } from "../env";
import { runWithProfile } from "../profile-context";
import { appendRuntimeErrorLog, installRuntimeProcessErrorLogging, serializeErrorForLog } from "../runtime-log";
import { HttpError } from "./kit-shim";
import { routes } from "virtual:internal-api-routes";

type CookieSameSite = "strict" | "lax" | "none" | boolean | undefined;

type CookieOptions = {
  path?: string;
  httpOnly?: boolean;
  secure?: boolean;
  sameSite?: CookieSameSite;
  maxAge?: number;
  expires?: Date;
};

type InternalRouteHandler = (event: {
  params: Record<string, string>;
  request: Request;
  url: URL;
  locals: {
    authenticated: boolean;
    profileId: string | null;
  };
  cookies: InternalCookies;
  getClientAddress?: () => string;
}) => Response | Promise<Response>;

type InternalRouteModule = Partial<Record<"GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "OPTIONS" | "HEAD", InternalRouteHandler>>;

type InternalRouteEntry = {
  path: string;
  pattern: RegExp;
  paramNames: string[];
  module: InternalRouteModule;
};

installRuntimeProcessErrorLogging();

const INTERNAL_PROXY_HEADER = "x-codex-webui-internal-token";
const PROFILE_HEADER = "x-codex-webui-profile-id";

class InternalCookies {
  private readonly values = new Map<string, string>();
  private readonly outgoing: string[] = [];

  constructor(cookieHeader: string | null) {
    for (const entry of (cookieHeader ?? "").split(";")) {
      const separator = entry.indexOf("=");
      if (separator <= 0) {
        continue;
      }

      const name = entry.slice(0, separator).trim();
      const value = entry.slice(separator + 1).trim();
      if (!name) {
        continue;
      }

      this.values.set(name, decodeURIComponent(value));
    }
  }

  get(name: string) {
    return this.values.get(name);
  }

  set(name: string, value: string, options: CookieOptions = {}) {
    this.values.set(name, value);
    this.outgoing.push(serializeCookie(name, value, options));
  }

  delete(name: string, options: CookieOptions = {}) {
    this.values.delete(name);
    this.outgoing.push(
      serializeCookie(name, "", {
        ...options,
        maxAge: 0,
        expires: new Date(0)
      })
    );
  }

  apply(headers: Headers) {
    for (const value of this.outgoing) {
      headers.append("set-cookie", value);
    }
  }
}

function serializeCookie(name: string, value: string, options: CookieOptions) {
  const parts = [`${name}=${encodeURIComponent(value)}`];

  if (options.path) {
    parts.push(`Path=${options.path}`);
  }
  if (typeof options.maxAge === "number" && Number.isFinite(options.maxAge)) {
    parts.push(`Max-Age=${Math.max(0, Math.floor(options.maxAge))}`);
  }
  if (options.expires instanceof Date) {
    parts.push(`Expires=${options.expires.toUTCString()}`);
  }
  if (options.httpOnly) {
    parts.push("HttpOnly");
  }
  if (options.secure) {
    parts.push("Secure");
  }
  if (typeof options.sameSite === "string") {
    parts.push(`SameSite=${options.sameSite.charAt(0).toUpperCase()}${options.sameSite.slice(1).toLowerCase()}`);
  }

  return parts.join("; ");
}

function normalizeMethod(method: string | undefined) {
  return (method ?? "GET").toUpperCase() as keyof InternalRouteModule;
}

function buildHeaders(source: Record<string, string | string[] | undefined>) {
  const headers = new Headers();

  for (const [name, rawValue] of Object.entries(source)) {
    if (Array.isArray(rawValue)) {
      for (const value of rawValue) {
        headers.append(name, value);
      }
      continue;
    }

    if (typeof rawValue === "string") {
      headers.set(name, rawValue);
    }
  }

  return headers;
}

function isBodylessMethod(method: string) {
  return method === "GET" || method === "HEAD";
}

function findRoute(pathname: string) {
  for (const route of routes as InternalRouteEntry[]) {
    const match = route.pattern.exec(pathname);
    if (!match) {
      continue;
    }

    const params: Record<string, string> = {};
    for (const [index, name] of route.paramNames.entries()) {
      params[name] = decodeURIComponent(match[index + 1] ?? "");
    }

    return {
      route,
      params
    };
  }

  return null;
}

function resolveAppErrorStatus(code: string) {
  switch (code) {
    case "EMPTY_MESSAGE":
    case "INVALID_QUEUE_MODE":
      return 400;
    case "FORBIDDEN_ROLE":
      return 403;
    case "QUEUE_ITEM_NOT_FOUND":
    case "PENDING_REQUEST_NOT_FOUND":
    case "SESSION_NOT_FOUND":
      return 404;
    case "QUEUE_ALREADY_DISPATCHING":
    case "SESSION_ALREADY_ARCHIVED":
    case "SESSION_NOT_ARCHIVED":
      return 409;
    case "NO_ACTIVE_TURN":
      return 412;
    default:
      return 500;
  }
}

async function toNodeResponse(nodeResponse: import("node:http").ServerResponse, response: Response, suppressBody = false) {
  nodeResponse.statusCode = response.status;
  if (response.statusText) {
    nodeResponse.statusMessage = response.statusText;
  }

  const responseHeaders = response.headers as Headers & {
    getSetCookie?: () => string[];
  };
  const setCookies = typeof responseHeaders.getSetCookie === "function" ? responseHeaders.getSetCookie() : [];

  response.headers.forEach((value, key) => {
    if (key === "set-cookie") {
      return;
    }
    nodeResponse.setHeader(key, value);
  });
  if (setCookies.length > 0) {
    nodeResponse.setHeader("set-cookie", setCookies);
  }

  if (suppressBody || !response.body) {
    nodeResponse.end();
    return;
  }

  const bodyStream = Readable.fromWeb(response.body as any);
  bodyStream.on("error", () => {
    nodeResponse.destroy();
  });
  nodeResponse.on("close", () => {
    bodyStream.destroy();
  });
  bodyStream.pipe(nodeResponse);
}

async function start() {
  const internalProxyToken = process.env.CODEX_WEBUI_INTERNAL_PROXY_TOKEN?.trim();
  if (!internalProxyToken) {
    throw new Error("Missing CODEX_WEBUI_INTERNAL_PROXY_TOKEN.");
  }

  const host = process.env.HOST ?? "127.0.0.1";
  const port = Number(process.env.PORT ?? "0");

  const server = createServer(async (req, res) => {
    try {
      const internalHeaderValue = Array.isArray(req.headers[INTERNAL_PROXY_HEADER])
        ? req.headers[INTERNAL_PROXY_HEADER][0]
        : req.headers[INTERNAL_PROXY_HEADER];
      if (internalHeaderValue !== internalProxyToken) {
        res.statusCode = 401;
        res.end("Unauthorized.");
        return;
      }

      const url = new URL(req.url ?? "/", `http://${req.headers.host ?? `${host}:${port}`}`);
      if (url.pathname === "/" || url.pathname === "/health") {
        res.statusCode = 200;
        res.setHeader("content-type", "application/json");
        res.end(JSON.stringify({ ok: true }));
        return;
      }

      const matched = findRoute(url.pathname);
      if (!matched) {
        res.statusCode = 404;
        res.end("Not found.");
        return;
      }

      const method = normalizeMethod(req.method);
      const handler = matched.route.module[method] ?? (method === "HEAD" ? matched.route.module.GET : undefined);
      if (!handler) {
        res.statusCode = 405;
        res.end("Method not allowed.");
        return;
      }

      const profile = getRuntimeProfile(
        Array.isArray(req.headers[PROFILE_HEADER]) ? req.headers[PROFILE_HEADER][0] : req.headers[PROFILE_HEADER]
      );
      const headers = buildHeaders(req.headers);
      const cookies = new InternalCookies(headers.get("cookie"));
      const request = new Request(url, {
        method,
        headers,
        body: isBodylessMethod(method) ? undefined : (Readable.toWeb(req) as any),
        duplex: isBodylessMethod(method) ? undefined : "half"
      } as RequestInit & { duplex?: "half" });

      const response = await runWithProfile(profile.id, () =>
        Promise.resolve(
          handler({
            params: matched.params,
            request,
            url,
            locals: {
              authenticated: true,
              profileId: profile.id
            },
            cookies,
            getClientAddress: () => {
              const remoteAddress = req.socket.remoteAddress;
              return typeof remoteAddress === "string" && remoteAddress.length > 0 ? remoteAddress : "127.0.0.1";
            }
          })
        )
      );

      cookies.apply(response.headers);
      await toNodeResponse(res, response, method === "HEAD");
    } catch (error) {
      const parsedAppError = parseAppError(error);
      if (error instanceof HttpError) {
        res.statusCode = error.status;
        res.end(error.body);
        return;
      }
      if (parsedAppError) {
        res.statusCode = parsedAppError.status ?? resolveAppErrorStatus(parsedAppError.code);
        res.end(error instanceof Error ? error.message : JSON.stringify(parsedAppError));
        return;
      }

      void appendRuntimeErrorLog({
        source: "internal-api",
        message: "request handling failed",
        details: {
          method: req.method ?? "GET",
          url: req.url ?? "/",
          error: serializeErrorForLog(error)
        }
      }).catch(() => {});

      res.statusCode = 500;
      res.end(error instanceof Error ? error.message : "Internal server error.");
    }
  });

  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(port, host, () => resolve());
  });

  const runtimeConfig = getRuntimeConfig();
  console.error(
    `[internal-api] ready on http://${host}:${port} with ${runtimeConfig.profiles.length} profile(s)`
  );
}

void start().catch((error) => {
  console.error(`[internal-api] failed to start: ${error instanceof Error ? error.stack ?? error.message : String(error)}`);
  process.exitCode = 1;
});
