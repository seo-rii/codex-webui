import fs from "node:fs";
import path from "node:path";

import { expect, test } from "@playwright/test";

const DEV_BYPASS_COOKIE = {
  name: "dev_bypass_waf",
  value: "seorii_bypass_token_is_this"
};

test.beforeEach(async ({ baseURL, context }) => {
  if (!baseURL) {
    throw new Error("Expected Playwright baseURL to be configured.");
  }

  await context.addCookies([
    {
      ...DEV_BYPASS_COOKIE,
      url: baseURL
    }
  ]);
});

test("builds a static shell plus private internal API bundle", async () => {
  const projectRoot = process.cwd();
  expect(fs.existsSync(path.join(projectRoot, "build", "static", "index.html"))).toBeTruthy();
  expect(fs.existsSync(path.join(projectRoot, "build", "static", "200.html"))).toBeTruthy();
  expect(fs.existsSync(path.join(projectRoot, "build", "internal", "index.js"))).toBeTruthy();
  expect(fs.existsSync(path.join(projectRoot, "build", "node"))).toBeFalsy();
});

test("serves the static shell under a base path and keeps login state after reload", async ({ baseURL, page }) => {
  if (!baseURL) {
    throw new Error("Expected Playwright baseURL to be configured.");
  }

  const basePath = new URL(baseURL).pathname.replace(/\/$/u, "");
  const observedUrls = new Set<string>();
  page.on("request", (request) => {
    observedUrls.add(request.url());
  });

  await page.goto("/");
  await expect(page.getByTestId("login-form")).toBeVisible();

  await page.getByTestId("login-password").fill(process.env.CODEX_WEBUI_E2E_PASSWORD ?? "test");
  const loginRequestPromise = page.waitForRequest(
    (request) => request.method() === "POST" && request.url().endsWith(`${basePath}/api/auth/login`)
  );
  const loginResponsePromise = page.waitForResponse(
    (response) => response.request().method() === "POST" && response.url().endsWith(`${basePath}/api/auth/login`)
  );
  await page.getByTestId("login-submit").click();

  const [loginRequest, loginResponse] = await Promise.all([loginRequestPromise, loginResponsePromise]);
  expect(loginResponse.ok()).toBeTruthy();
  expect(new URL(loginRequest.url()).pathname).toBe(`${basePath}/api/auth/login`);

  await expect(page.getByTestId("workspace-shell")).toBeVisible();
  expect(new URL(page.url()).pathname).toBe(`${basePath}/`);
  expect([...observedUrls].some((url) => url.includes(`${basePath}/_app/immutable/`))).toBeTruthy();

  await page.reload();
  await expect(page.getByTestId("workspace-shell")).toBeVisible();
});
