import adapterStatic from "@sveltejs/adapter-static";
import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";
import { loadEnv } from "vite";
import { createBuildMetadata } from "./scripts/build-metadata.mjs";

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
const staticBasePlaceholder = "/__CODEX_WEBUI_BASE__";
const basePath = normalizeBasePath(process.env.CODEX_WEBUI_BUILD_BASE_PATH ?? staticBasePlaceholder);
const buildMetadata = createBuildMetadata(process.cwd(), { ...env, ...process.env });
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
    adapter: adapterStatic({
      pages: "build/static",
      assets: "build/static",
      fallback: "200.html",
      strict: false
    }),
    paths: {
      base: basePath,
      relative: false
    },
    version: {
      name: buildMetadata.version,
      pollInterval: 60_000
    },
    csrf: {
      trustedOrigins
    }
  }
};

export default config;
