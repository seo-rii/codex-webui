import { createHmac, randomBytes, scryptSync, timingSafeEqual } from "node:crypto";

import type { Cookies } from "@sveltejs/kit";

import { getRuntimeConfig } from "./env";

const AUTH_COOKIE = "codex_webui_auth";
const LOGIN_WINDOW_MS = 10 * 60 * 1000;
const LOGIN_MAX_ATTEMPTS = 8;

const attempts = new Map<string, number[]>();

function getPasswordMaterial() {
  const config = getRuntimeConfig();
  const password = config.password;
  const passwordHash = config.passwordHash;

  if (!password && !passwordHash) {
    throw new Error("Set CODEX_WEBUI_PASSWORD_HASH (recommended) or CODEX_WEBUI_PASSWORD before using codex-webui.");
  }

  return {
    password,
    passwordHash,
    secret: config.sessionSecret ?? passwordHash ?? password ?? ""
  };
}

function hashPassword(password: string, salt: Buffer) {
  return scryptSync(password, salt, 64);
}

function verifyPassword(input: string) {
  const { password, passwordHash } = getPasswordMaterial();
  if (password) {
    const left = Buffer.from(input);
    const right = Buffer.from(password);
    return left.length === right.length && timingSafeEqual(left, right);
  }
  if (!passwordHash) {
    return false;
  }
  const [, savedSalt, savedKey] = passwordHash.split("$");
  if (!savedSalt || !savedKey) {
    throw new Error("Invalid CODEX_WEBUI_PASSWORD_HASH format.");
  }
  const derived = hashPassword(input, Buffer.from(savedSalt, "base64url"));
  return timingSafeEqual(derived, Buffer.from(savedKey, "base64url"));
}

function sign(value: string) {
  const { secret } = getPasswordMaterial();
  return createHmac("sha256", secret).update(value).digest("base64url");
}

function makeToken() {
  const now = Date.now();
  const expires = now + 7 * 24 * 60 * 60 * 1000;
  const nonce = randomBytes(18).toString("base64url");
  const payload = `${now}.${expires}.${nonce}`;
  return `${payload}.${sign(payload)}`;
}

export function checkRateLimit(identifier: string) {
  const now = Date.now();
  const history = (attempts.get(identifier) ?? []).filter((timestamp) => now - timestamp < LOGIN_WINDOW_MS);
  attempts.set(identifier, history);
  return history.length < LOGIN_MAX_ATTEMPTS;
}

export function recordLoginFailure(identifier: string) {
  const now = Date.now();
  const history = (attempts.get(identifier) ?? []).filter((timestamp) => now - timestamp < LOGIN_WINDOW_MS);
  history.push(now);
  attempts.set(identifier, history);
}

export function clearLoginFailures(identifier: string) {
  attempts.delete(identifier);
}

function resolveCookieSecurity(isSecureRequest: boolean) {
  const { cookieSameSite, cookieSecureMode } = getRuntimeConfig();

  if (cookieSameSite === "none" && cookieSecureMode === "never") {
    throw new Error("CODEX_WEBUI_COOKIE_SAMESITE=none cannot be combined with CODEX_WEBUI_COOKIE_SECURE=never.");
  }

  if (cookieSecureMode === "always") {
    return true;
  }

  if (cookieSecureMode === "never") {
    return false;
  }

  if (cookieSameSite === "none" && !isSecureRequest) {
    throw new Error("CODEX_WEBUI_COOKIE_SAMESITE=none requires HTTPS or CODEX_WEBUI_COOKIE_SECURE=always.");
  }

  return isSecureRequest;
}

export function issueAuthCookie(cookies: Cookies, isSecureRequest: boolean) {
  const { cookieSameSite } = getRuntimeConfig();
  const secure = resolveCookieSecurity(isSecureRequest);
  cookies.set(AUTH_COOKIE, makeToken(), {
    path: "/",
    httpOnly: true,
    sameSite: cookieSameSite,
    secure,
    maxAge: 7 * 24 * 60 * 60
  });
}

export function clearAuthCookie(cookies: Cookies) {
  cookies.delete(AUTH_COOKIE, { path: "/" });
}

export function isAuthenticated(cookies: Cookies) {
  const token = cookies.get(AUTH_COOKIE);
  if (!token) {
    return false;
  }
  const parts = token.split(".");
  if (parts.length !== 4) {
    return false;
  }
  const payload = parts.slice(0, 3).join(".");
  const signature = parts[3];
  const expected = sign(payload);
  const actual = Buffer.from(signature);
  const signed = Buffer.from(expected);
  if (actual.length !== signed.length || !timingSafeEqual(actual, signed)) {
    return false;
  }
  const [, expires] = parts;
  return Date.now() < Number(expires);
}

export function authenticatePassword(input: string) {
  try {
    return verifyPassword(input);
  } catch {
    return false;
  }
}
