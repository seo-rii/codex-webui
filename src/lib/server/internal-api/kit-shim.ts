type JsonInit = number | ResponseInit | undefined;

type CookieSameSite = "strict" | "lax" | "none" | boolean | undefined;

export type Cookies = {
  get(name: string): string | undefined;
  set(name: string, value: string, options?: {
    path?: string;
    httpOnly?: boolean;
    secure?: boolean;
    sameSite?: CookieSameSite;
    maxAge?: number;
    expires?: Date;
  }): void;
  delete(name: string, options?: {
    path?: string;
    httpOnly?: boolean;
    secure?: boolean;
    sameSite?: CookieSameSite;
    maxAge?: number;
    expires?: Date;
  }): void;
};

export class HttpError extends Error {
  status: number;
  body: string;

  constructor(status: number, body: string) {
    super(body);
    this.name = "HttpError";
    this.status = status;
    this.body = body;
  }
}

function normalizeJsonInit(init: JsonInit) {
  if (typeof init === "number") {
    return {
      status: init,
      headers: new Headers()
    };
  }

  return {
    status: init?.status ?? 200,
    headers: new Headers(init?.headers ?? {})
  };
}

function stringifyErrorBody(body: unknown) {
  if (typeof body === "string") {
    return body;
  }

  if (body && typeof body === "object" && "message" in body && typeof (body as { message?: unknown }).message === "string") {
    return String((body as { message: string }).message);
  }

  return JSON.stringify(body ?? "Unknown error");
}

export function json(data: unknown, init?: JsonInit) {
  const resolved = normalizeJsonInit(init);
  if (!resolved.headers.has("content-type")) {
    resolved.headers.set("content-type", "application/json");
  }

  return new Response(JSON.stringify(data), {
    status: resolved.status,
    headers: resolved.headers
  });
}

export function error(status: number, body: unknown = "Error"): never {
  throw new HttpError(status, stringifyErrorBody(body));
}
