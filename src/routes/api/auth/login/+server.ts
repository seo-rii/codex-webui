import { error, json } from "@sveltejs/kit";

import {
  authenticatePassword,
  checkRateLimit,
  clearLoginFailures,
  issueAuthCookie,
  recordLoginFailure
} from "$lib/server/auth";

export async function POST(event) {
  const body = (await event.request.json().catch(() => ({}))) as { password?: string };
  const password = typeof body.password === "string" ? body.password : "";
  const identifier =
    event.request.headers.get("x-forwarded-for")?.split(",")[0]?.trim() ??
    event.getClientAddress?.() ??
    "unknown";

  if (!checkRateLimit(identifier)) {
    throw error(429, "Too many login attempts. Try again later.");
  }

  if (!authenticatePassword(password)) {
    recordLoginFailure(identifier);
    throw error(401, "Invalid password.");
  }

  clearLoginFailures(identifier);
  const forwardedProto = event.request.headers.get("x-forwarded-proto");
  const isLocalHost = new Set(["localhost", "127.0.0.1", "::1"]).has(event.url.hostname);
  const secure = forwardedProto ? forwardedProto === "https" : event.url.protocol === "https:" && !isLocalHost;
  try {
    issueAuthCookie(event.cookies, secure);
  } catch (issueError) {
    throw error(500, issueError instanceof Error ? issueError.message : "Failed to issue auth cookie.");
  }
  return json({ ok: true });
}
