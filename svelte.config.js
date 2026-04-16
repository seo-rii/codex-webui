import adapter from "@sveltejs/adapter-node";
import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";
import { loadEnv } from "vite";

function normalizeBasePath(value) {
  if (!value) {
    return "";
  }

  const trimmed = value.trim();
  if (!trimmed || trimmed === "/") {
    return "";
  }

  const withoutTrailingSlash = trimmed.replace(/\/+$/u, "");
  return withoutTrailingSlash.startsWith("/") ? withoutTrailingSlash : `/${withoutTrailingSlash}`;
}

const env = loadEnv(process.env.NODE_ENV ?? "development", process.cwd(), "");
const basePath = normalizeBasePath(
  process.env.CODEX_WEBUI_BASE_PATH ?? env.CODEX_WEBUI_BASE_PATH ?? process.env.VITE_BASE_PATH ?? env.VITE_BASE_PATH
);
const trustedOrigins = [
  ...new Set(
    String(process.env.CODEX_WEBUI_CORS_ALLOWED_ORIGINS ?? env.CODEX_WEBUI_CORS_ALLOWED_ORIGINS ?? "")
      .split(/[,\n]/u)
      .map((value) => value.trim())
      .filter(Boolean)
  )
];

/** @type {import('@sveltejs/kit').Config} */
const config = {
  preprocess: vitePreprocess(),
  kit: {
    adapter: adapter(),
    paths: {
      base: basePath
    },
    csrf: {
      trustedOrigins
    }
  }
};

export default config;
