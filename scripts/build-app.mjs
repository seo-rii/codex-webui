import { spawn } from "node:child_process";
import fs from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import esbuild from "esbuild";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const projectRoot = path.resolve(__dirname, "..");
const viteBin = path.join(projectRoot, "node_modules", "vite", "bin", "vite.js");
const buildDir = path.join(projectRoot, "build");
const internalOutfile = path.join(buildDir, "internal", "index.js");
const staticBasePlaceholder = "/__CODEX_WEBUI_BASE__";

function runStaticBuild() {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [viteBin, "build"], {
      cwd: projectRoot,
      stdio: "inherit",
      env: {
        ...process.env,
        CODEX_WEBUI_BUILD_BASE_PATH: process.env.CODEX_WEBUI_BUILD_BASE_PATH ?? staticBasePlaceholder
      }
    });

    child.once("exit", (code, signal) => {
      if (code === 0) {
        resolve();
        return;
      }

      reject(new Error(`static build failed with ${signal ? `signal ${signal}` : `exit code ${code}`}.`));
    });
    child.once("error", reject);
  });
}

function escapeRegexSegment(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&");
}

function routePathPartsFromFile(filePath) {
  const relative = path.relative(path.join(projectRoot, "src", "routes"), filePath).replaceAll(path.sep, "/");
  const withoutSuffix = relative.replace(/\/\+server\.ts$/u, "");
  return withoutSuffix === "" ? [] : withoutSuffix.split("/");
}

function buildRouteEntry(filePath, index) {
  const parts = routePathPartsFromFile(filePath);
  const paramNames = [];
  const pattern = parts
    .map((segment) => {
      const match = /^\[([A-Za-z0-9_]+)\]$/u.exec(segment);
      if (match) {
        paramNames.push(match[1]);
        return "([^/]+)";
      }
      return escapeRegexSegment(segment);
    })
    .join("/");
  const importPath = path.relative(path.join(projectRoot, "src", "lib", "server", "internal-api"), filePath).replaceAll(path.sep, "/");
  return {
    importName: `route${index}`,
    importPath: importPath.startsWith(".") ? importPath : `./${importPath}`,
    paramNames,
    pattern: `^/${pattern}$`,
    routePath: `/${parts.join("/")}`
  };
}

async function listApiRouteFiles(currentDir) {
  const entries = await fs.readdir(currentDir, { withFileTypes: true });
  const files = [];

  for (const entry of entries) {
    const fullPath = path.join(currentDir, entry.name);
    if (entry.isDirectory()) {
      files.push(...(await listApiRouteFiles(fullPath)));
      continue;
    }

    if (entry.isFile() && entry.name === "+server.ts") {
      files.push(fullPath);
    }
  }

  return files.sort();
}

async function buildRouteManifestSource() {
  const routeFiles = await listApiRouteFiles(path.join(projectRoot, "src", "routes", "api"));
  const entries = routeFiles.map((filePath, index) => buildRouteEntry(filePath, index));

  return `${entries
    .map((entry) => `import * as ${entry.importName} from ${JSON.stringify(entry.importPath)};`)
    .join("\n")}

export const routes = [
${entries
  .map(
    (entry) => `  {
    path: ${JSON.stringify(entry.routePath)},
    pattern: new RegExp(${JSON.stringify(entry.pattern)}),
    paramNames: ${JSON.stringify(entry.paramNames)},
    module: ${entry.importName}
  }`
  )
  .join(",\n")}
];
`;
}

function internalBuildPlugin() {
  return {
    name: "codex-webui-internal-api",
    setup(build) {
      build.onResolve({ filter: /^virtual:internal-api-routes$/ }, () => ({
        path: "virtual:internal-api-routes",
        namespace: "codex-webui-virtual"
      }));

      build.onLoad({ filter: /^virtual:internal-api-routes$/, namespace: "codex-webui-virtual" }, async () => ({
        contents: await buildRouteManifestSource(),
        loader: "ts",
        resolveDir: path.join(projectRoot, "src", "lib", "server", "internal-api")
      }));

      build.onResolve({ filter: /^@sveltejs\/kit$/ }, () => ({
        path: path.join(projectRoot, "src", "lib", "server", "internal-api", "kit-shim.ts")
      }));

      build.onResolve({ filter: /^\$lib(?:\/.*)?$/ }, async (args) => {
        const target = path.join(projectRoot, "src", "lib", args.path.slice("$lib".length));
        const resolved = await build.resolve(target, {
          kind: args.kind,
          importer: args.importer,
          namespace: args.namespace,
          resolveDir: args.resolveDir,
          pluginData: args.pluginData
        });

        if (resolved.errors.length > 0) {
          return { errors: resolved.errors };
        }

        return {
          path: resolved.path,
          external: resolved.external,
          sideEffects: resolved.sideEffects,
          namespace: resolved.namespace || "file"
        };
      });
    }
  };
}

async function buildInternalApi() {
  await esbuild.build({
    absWorkingDir: projectRoot,
    entryPoints: [path.join(projectRoot, "src", "lib", "server", "internal-api", "server.ts")],
    outfile: internalOutfile,
    bundle: true,
    format: "esm",
    platform: "node",
    target: "node20",
    sourcemap: false,
    logLevel: "info",
    plugins: [internalBuildPlugin()]
  });
}

await fs.rm(buildDir, { recursive: true, force: true });

await runStaticBuild();
await buildInternalApi();
