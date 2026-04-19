import adapterNode from "@sveltejs/adapter-node";
import adapterStatic from "@sveltejs/adapter-static";
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
const buildTarget = process.env.CODEX_WEBUI_BUILD_TARGET === "static" ? "static" : "node";
const staticBasePlaceholder = "/__CODEX_WEBUI_BASE__";
const basePath =
  buildTarget === "static"
    ? normalizeBasePath(process.env.CODEX_WEBUI_BUILD_BASE_PATH ?? staticBasePlaceholder)
    : "";
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
    adapter:
      buildTarget === "static"
        ? adapterStatic({
            pages: "build/static",
            assets: "build/static",
            fallback: "200.html",
            strict: false
          })
        : adapterNode({
            out: "build/node"
          }),
    paths: {
      base: basePath,
      relative: false
    },
    csrf: {
      trustedOrigins
    }
  }
};

export default config;
