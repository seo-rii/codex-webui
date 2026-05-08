import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const tempRoot = await fs.mkdtemp(path.join(os.tmpdir(), "codex-webui-tunnel-safety-"));

try {
  const fakeBin = path.join(tempRoot, "bin");
  const home = path.join(tempRoot, "home");
  const workspace = path.join(tempRoot, "workspace");
  const codexHome = path.join(tempRoot, "codex-home");
  const dataDir = path.join(home, ".codex", "codex-webui", "data");
  const configDir = path.join(home, ".codex");
  await fs.mkdir(fakeBin, { recursive: true });
  await fs.mkdir(workspace, { recursive: true });
  await fs.mkdir(codexHome, { recursive: true });
  await fs.mkdir(configDir, { recursive: true });

  const fakeCloudflared = path.join(fakeBin, process.platform === "win32" ? "cloudflared.cmd" : "cloudflared");
  const fakeCloudflaredBody = process.platform === "win32" ? "@echo off\r\nexit /b 0\r\n" : "#!/bin/sh\nexit 0\n";
  await fs.writeFile(fakeCloudflared, fakeCloudflaredBody, { mode: 0o755 });

  const config = {
    host: "127.0.0.1",
    port: 49173,
    basePath: "/absproxy/49173",
    codexBin: "codex",
    dataDir,
    defaultProfileId: "default",
    profiles: [
      {
        id: "default",
        label: "Default",
        codexHome,
        dataDir: path.join(dataDir, "profiles", "default")
      }
    ],
    allowedRoots: [workspace],
    passwordHash: "scrypt$placeholder$placeholder",
    ownerPasswordHash: "",
    hcaptchaSiteKey: "",
    hcaptchaSecretKey: "",
    sessionSecret: "0123456789abcdef0123456789abcdef",
    corsAllowedOrigins: [],
    backendBinaryPath: path.join(tempRoot, "missing-backend"),
    tunnel: {
      provider: "cloudflared",
      background: true,
      hostname: "",
      name: "",
      overwriteDns: false,
      logLevel: "info",
      extraArgs: []
    }
  };
  await fs.writeFile(path.join(configDir, "codex-webui.yml"), JSON.stringify(config, null, 2));

  const missingOwnerResult = spawnSync(
    process.execPath,
    [
      path.join(repoRoot, "bin", "codex-webui.mjs"),
      "tunnel",
      "start",
      "--provider",
      "cloudflared",
      "--json",
      "--yes"
    ],
    {
      cwd: repoRoot,
      encoding: "utf8",
      env: {
        ...process.env,
        HOME: home,
        USERPROFILE: home,
        PATH: `${fakeBin}${path.delimiter}${process.env.PATH ?? ""}`
      }
    }
  );

  assert.notEqual(missingOwnerResult.status, 0, "tunnel start must require an owner password hash");
  assert.match(
    missingOwnerResult.stderr,
    /Owner password hash is required/u,
    "failure should explain the owner credential requirement for public tunnels"
  );

  config.ownerPasswordHash = "scrypt$owner-placeholder$placeholder";
  await fs.writeFile(path.join(configDir, "codex-webui.yml"), JSON.stringify(config, null, 2));

  const result = spawnSync(
    process.execPath,
    [path.join(repoRoot, "bin", "codex-webui.mjs"), "tunnel", "start", "--provider", "cloudflared", "--json"],
    {
      cwd: repoRoot,
      encoding: "utf8",
      env: {
        ...process.env,
        HOME: home,
        USERPROFILE: home,
        PATH: `${fakeBin}${path.delimiter}${process.env.PATH ?? ""}`
      }
    }
  );

  assert.notEqual(result.status, 0, "tunnel start without explicit confirmation must fail in JSON/non-interactive mode");
  assert.match(
    result.stderr,
    /Refusing to start a public tunnel without explicit confirmation/u,
    "failure should explain the explicit public-exposure confirmation requirement"
  );

  await assert.rejects(
    fs.access(path.join(home, ".codex", "codex-webui", "server.pid")),
    /ENOENT/u,
    "tunnel preflight must not start the gateway before confirmation"
  );

  const readme = await fs.readFile(path.join(repoRoot, "README.md"), "utf8");
  const distribution = await fs.readFile(path.join(repoRoot, "docs", "distribution.md"), "utf8");
  const architecture = await fs.readFile(path.join(repoRoot, "docs", "architecture.md"), "utf8");
  assert.match(readme, /host user privileges/u, "README should document host-terminal privilege boundaries");
  assert.match(distribution, /Type "expose"/u, "distribution docs should document interactive tunnel confirmation");
  assert.match(architecture, /not a filesystem sandbox/u, "architecture docs should document terminal sandbox limits");
} finally {
  await fs.rm(tempRoot, { recursive: true, force: true });
}
