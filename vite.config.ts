import { paraglideVitePlugin } from "@inlang/paraglide-js";
import { sveltekit } from "@sveltejs/kit/vite";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [
    tailwindcss(),
    paraglideVitePlugin({
      project: "./project.inlang",
      outdir: "./src/lib/paraglide",
      strategy: ["cookie", "globalVariable", "preferredLanguage", "baseLocale"],
      isServer: "import.meta.env.SSR",
      emitTsDeclarations: true
    }),
    sveltekit()
  ],
  server: {
    allowedHosts: true
  }
});
