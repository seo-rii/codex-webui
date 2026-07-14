import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { createBuildMetadata } from "../scripts/build-metadata.mjs";

const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

test("repository builds ignore stale inherited commit metadata", () => {
  const head = execFileSync("git", ["rev-parse", "HEAD"], {
    cwd: projectRoot,
    encoding: "utf8"
  }).trim();
  const metadata = createBuildMetadata(projectRoot, {
    CODEX_WEBUI_BUILD_COMMIT: "0000000000000000000000000000000000000000",
    CODEX_WEBUI_BUILD_COMMIT_SHORT: "000000000000",
    CODEX_WEBUI_BUILD_DIRTY: "false",
    CODEX_WEBUI_BUILD_EPOCH_MS: "1",
    CODEX_WEBUI_BUILD_TIMESTAMP: "1",
    CODEX_WEBUI_BUILD_VERSION: "0.1.0-000000000000-1"
  });

  assert.equal(metadata.commit, head);
  assert.equal(metadata.commitShort, head.slice(0, 12));
  assert.notEqual(metadata.epochMs, "1");
  assert.match(metadata.version, new RegExp(head.slice(0, 12)));
});
