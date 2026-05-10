import { expect, test } from "@playwright/test";

const DEV_BYPASS_COOKIE = {
  name: "dev_bypass_waf",
  value: "seorii_bypass_token_is_this"
};

type WsResponse = {
  kind: "response";
  id: string;
  ok: boolean;
  result?: {
    ok?: boolean;
    routed?: string;
  };
  error?: string;
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

async function login(page: import("@playwright/test").Page) {
  await page.goto("/");
  await expect(page.getByTestId("login-form")).toBeVisible();
  await page.getByTestId("login-password").fill(process.env.CODEX_WEBUI_E2E_PASSWORD ?? "test");
  await page.getByTestId("login-submit").click();
  await expect(page.getByTestId("workspace-shell")).toBeVisible();
}

async function wsRequest(
  page: import("@playwright/test").Page,
  method: string,
  params: Record<string, unknown>
): Promise<WsResponse> {
  return page.evaluate(
    ({ method, params }) =>
      new Promise<WsResponse>((resolve, reject) => {
        const url = new URL(window.location.href);
        url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
        url.pathname = `${url.pathname.replace(/\/$/u, "")}/ws`;
        url.search = "";
        url.hash = "";

        const id = `e2e-${Date.now()}`;
        const socket = new WebSocket(url.toString());
        const timeout = window.setTimeout(() => {
          socket.close();
          reject(new Error(`Timed out waiting for ${method}`));
        }, 15_000);

        socket.addEventListener("open", () => {
          socket.send(
            JSON.stringify({
              kind: "request",
              id,
              method,
              params
            })
          );
        });

        socket.addEventListener("message", (event) => {
          if (typeof event.data !== "string") {
            return;
          }
          const payload = JSON.parse(event.data) as WsResponse;
          if (payload.kind !== "response" || payload.id !== id) {
            return;
          }
          window.clearTimeout(timeout);
          socket.close();
          resolve(payload);
        });

        socket.addEventListener("error", () => {
          window.clearTimeout(timeout);
          reject(new Error(`WebSocket ${method} request failed.`));
        });
      }),
    { method, params }
  );
}

test("delivers computer input through the WebSocket fallback path", async ({ page }) => {
  await login(page);

  const response = await wsRequest(page, "computer/input", {
    sessionId: "thread-e2e-computer",
    input: {
      type: "click",
      x: 0.25,
      y: 0.75,
      button: "left",
      coordinateSpace: "normalized"
    }
  });

  expect(response.ok, response.error).toBeTruthy();
  expect(response.result?.ok).toBeTruthy();
  expect(response.result?.routed).toBe("mcpServerTool");
});
