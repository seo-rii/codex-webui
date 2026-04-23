import fs from "node:fs/promises";
import path from "node:path";
import { spawnSync } from "node:child_process";

import { buildMetadataEnv, createBuildMetadata } from "./build-metadata.mjs";

const projectRoot = path.resolve(new URL("..", import.meta.url).pathname);
const outputRoot = path.join(projectRoot, "dist", "backend");
const backendManifest = path.join(projectRoot, "backend", "Cargo.toml");
const buildMetadata = createBuildMetadata(projectRoot);
const buildEnv = buildMetadataEnv(buildMetadata);
const targets = [
  "x86_64-unknown-linux-gnu",
  "aarch64-unknown-linux-gnu",
  "x86_64-apple-darwin",
  "aarch64-apple-darwin",
  "x86_64-pc-windows-msvc"
];

function commandAvailable(command) {
  return spawnSync(process.platform === "win32" ? "where" : "which", [command], { stdio: "ignore" }).status === 0;
}

function run(command, args) {
  const result = spawnSync(command, args, {
    cwd: projectRoot,
    env: {
      ...process.env,
      ...buildEnv
    },
    stdio: "inherit"
  });
  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(" ")} failed`);
  }
}

function builtBinaryPath(target) {
  const fileName = target.includes("windows") ? "backend.exe" : "backend";
  return path.join(projectRoot, "backend", "target", target, "release", fileName);
}

const builder = commandAvailable("cargo-zigbuild")
  ? { command: "cargo", extraArgs: ["zigbuild"] }
  : commandAvailable("cross")
    ? { command: "cross", extraArgs: ["build"] }
    : { command: "cargo", extraArgs: ["build"] };

await fs.mkdir(outputRoot, { recursive: true });

for (const target of targets) {
  run(builder.command, [...builder.extraArgs, "--release", "--target", target, "--manifest-path", backendManifest]);
  const source = builtBinaryPath(target);
  const targetDir = path.join(outputRoot, target);
  await fs.mkdir(targetDir, { recursive: true });
  await fs.copyFile(source, path.join(targetDir, path.basename(source)));
}

console.log(`Copied backend binaries into ${outputRoot}`);
