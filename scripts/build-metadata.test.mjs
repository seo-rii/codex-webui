import assert from "node:assert/strict";
import test from "node:test";

import { buildMetadataEnv } from "./build-metadata.mjs";

test("release build metadata is explicitly pinned for the Rust build script", () => {
  const environment = buildMetadataEnv({
    packageVersion: "1.2.3",
    version: "1.2.3-deadbeef0000-1000",
    commit: "deadbeef00000000000000000000000000000000",
    commitShort: "deadbeef0000",
    dirty: false,
    epochMs: "1000",
    timestamp: "1970-01-01T00:00:01.000Z"
  });

  assert.equal(environment.CODEX_WEBUI_BUILD_METADATA_PINNED, "true");
  assert.equal(environment.CODEX_WEBUI_BUILD_COMMIT_SHORT, "deadbeef0000");
});
