import { error, json } from "@sveltejs/kit";

import {
  authenticatePassword,
  checkRateLimit,
  clearLoginFailures,
  getLoginHcaptchaConfig,
  issueAuthCookie,
  recordLoginFailure
} from "$lib/server/auth";

export async function POST(event) {
  const body = (await event.request.json().catch(() => ({}))) as { password?: string; hcaptchaToken?: string };
  const password = typeof body.password === "string" ? body.password : "";
  const hcaptchaToken = typeof body.hcaptchaToken === "string" ? body.hcaptchaToken.trim() : "";
  const identifier =
    event.request.headers.get("x-forwarded-for")?.split(",")[0]?.trim() ??
    event.getClientAddress?.() ??
    "unknown";

  if (!checkRateLimit(identifier)) {
    throw error(429, "Too many login attempts. Try again later.");
  }

  const hcaptcha = getLoginHcaptchaConfig();
  if (hcaptcha.enabled) {
    if (!hcaptchaToken) {
      throw error(400, "Complete the hCaptcha challenge before signing in.");
    }

    const verificationBody = new URLSearchParams({
      secret: hcaptcha.secretKey ?? "",
      response: hcaptchaToken
    });
    if (identifier !== "unknown") {
      verificationBody.set("remoteip", identifier);
    }

    const verificationResponse = await fetch("https://api.hcaptcha.com/siteverify", {
      method: "POST",
      headers: {
        "content-type": "application/x-www-form-urlencoded"
      },
      body: verificationBody
    }).catch(() => null);

    if (!verificationResponse?.ok) {
      throw error(502, "Failed to verify hCaptcha.");
    }

    const verificationPayload = (await verificationResponse.json().catch(() => ({}))) as { success?: boolean };
    if (!verificationPayload.success) {
      recordLoginFailure(identifier);
      throw error(401, "Complete the hCaptcha challenge before signing in.");
    }
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
