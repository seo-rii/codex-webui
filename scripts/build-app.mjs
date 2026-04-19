import { spawn } from "node:child_process";
import fs from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const projectRoot = path.resolve(__dirname, "..");
const viteBin = path.join(projectRoot, "node_modules", "vite", "bin", "vite.js");
const buildDir = path.join(projectRoot, "build");

function runBuild(label, extraEnv) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [viteBin, "build"], {
      cwd: projectRoot,
      stdio: "inherit",
      env: {
        ...process.env,
        ...extraEnv
      }
    });

    child.once("exit", (code, signal) => {
      if (code === 0) {
        resolve();
        return;
      }

      reject(new Error(`${label} build failed with ${signal ? `signal ${signal}` : `exit code ${code}`}.`));
    });
    child.once("error", reject);
  });
}

await fs.rm(buildDir, { recursive: true, force: true });

await runBuild("static", {
  CODEX_WEBUI_BUILD_TARGET: "static",
  CODEX_WEBUI_BUILD_BASE_PATH: "/__CODEX_WEBUI_BASE__"
});

await runBuild("internal-node", {
  CODEX_WEBUI_BUILD_TARGET: "node",
  CODEX_WEBUI_BUILD_BASE_PATH: ""
});
