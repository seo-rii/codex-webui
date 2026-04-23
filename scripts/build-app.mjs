import { spawn } from "node:child_process";
import fs from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const projectRoot = path.resolve(__dirname, "..");
const viteBin = path.join(projectRoot, "node_modules", "vite", "bin", "vite.js");
const buildDir = path.join(projectRoot, "build");
const backendManifest = path.join(projectRoot, "backend", "Cargo.toml");
const backendReleaseDir = path.join(projectRoot, "backend", "target", "release");
const distBackendDir = path.join(projectRoot, "dist", "backend");
const staticBasePlaceholder = "/__CODEX_WEBUI_BASE__";

function currentRustTarget() {
  if (process.platform === "linux" && process.arch === "x64") {
    return "x86_64-unknown-linux-gnu";
  }
  if (process.platform === "linux" && process.arch === "arm64") {
    return "aarch64-unknown-linux-gnu";
  }
  if (process.platform === "darwin" && process.arch === "x64") {
    return "x86_64-apple-darwin";
  }
  if (process.platform === "darwin" && process.arch === "arm64") {
    return "aarch64-apple-darwin";
  }
  if (process.platform === "win32" && process.arch === "x64") {
    return "x86_64-pc-windows-msvc";
  }
  if (process.platform === "win32" && process.arch === "arm64") {
    return "aarch64-pc-windows-msvc";
  }
  return `${process.platform}-${process.arch}`;
}

function binaryName() {
  return process.platform === "win32" ? "backend.exe" : "backend";
}

function runCargoBuild() {
  return new Promise((resolve, reject) => {
    const child = spawn("cargo", ["build", "--release", "--manifest-path", backendManifest], {
      cwd: projectRoot,
      stdio: "inherit"
    });

    child.once("exit", (code, signal) => {
      if (code === 0) {
        resolve();
        return;
      }

      reject(new Error(`backend build failed with ${signal ? `signal ${signal}` : `exit code ${code}`}.`));
    });
    child.once("error", reject);
  });
}

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

await fs.rm(buildDir, { recursive: true, force: true });
await runStaticBuild();
await runCargoBuild();

const targetDir = path.join(distBackendDir, currentRustTarget());
await fs.rm(targetDir, { recursive: true, force: true });
await fs.mkdir(targetDir, { recursive: true });
await fs.copyFile(path.join(backendReleaseDir, binaryName()), path.join(targetDir, binaryName()));
