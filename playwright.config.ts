import { defineConfig } from "@playwright/test";

const port = Number(process.env.CODEX_WEBUI_E2E_PORT ?? 44173);
const basePath = process.env.CODEX_WEBUI_E2E_BASE_PATH ?? "/e2e/base";
const password = process.env.CODEX_WEBUI_E2E_PASSWORD ?? "test";
const baseURL = `http://127.0.0.1:${port}${basePath}`;

export default defineConfig({
  testDir: "e2e",
  fullyParallel: false,
  retries: 0,
  timeout: 90_000,
  expect: {
    timeout: 15_000
  },
  use: {
    baseURL,
    browserName: "chromium",
    trace: "retain-on-failure"
  },
  webServer: {
    command: "cargo run --manifest-path backend/Cargo.toml --bin backend",
    cwd: ".",
    timeout: 180_000,
    reuseExistingServer: false,
    url: `${baseURL}/`,
    env: {
      ...process.env,
      HOST: "127.0.0.1",
      PORT: String(port),
      CODEX_WEBUI_BASE_PATH: basePath,
      CODEX_WEBUI_PASSWORD: password,
      RUST_LOG: process.env.RUST_LOG ?? "warn"
    }
  }
});
