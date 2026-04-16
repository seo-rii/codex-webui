# Distribution

## Goal

The intended user-facing entrypoint is:

```bash
npx codex-webui
```

That implies the published package must be able to:

- guide a first-time user through setup
- start the server in the background
- resolve a runnable Rust gateway binary for the current platform
- serve the already-built frontend and internal SvelteKit bundle

## What The Package Needs To Ship

The npm package should include:

- `bin/codex-webui.mjs`
- the built frontend/internal SvelteKit output under `build`
- docs and helper scripts that the CLI depends on
- Rust gateway binaries under `dist/backend/<target>/`

The package already exposes:

```json
{
  "bin": {
    "codex-webui": "./bin/codex-webui.mjs"
  }
}
```

## CLI Contract

`bin/codex-webui.mjs` is responsible for:

- first-run interactive setup
- reading and writing `~/.codex/codex-webui.yml`
- starting, restarting, and stopping the background server
- printing the launch URL, PID, config path, and log path
- exposing `config`, `restart`, `stop`, and `tunnel`

On first run, it prompts for:

- host
- port
- base path
- Codex binary path
- Codex home
- data directory
- allowed roots
- CORS origins
- optional explicit backend binary path
- password

The password is stored as a scrypt hash in the YAML config.

## Generated Local State

The CLI owns two user-local locations:

- config: `~/.codex/codex-webui.yml`
- background runtime state: `~/.codex/codex-webui/`

That runtime state currently includes:

- PID file
- server log

## Binary Resolution Order

At runtime the CLI searches for a Rust gateway binary in this order:

1. `backendBinaryPath` from `~/.codex/codex-webui.yml`
2. `CODEX_WEBUI_BACKEND_BIN`
3. `dist/backend/<current-target>/backend`
4. `backend/target/release/backend`
5. `backend/target/debug/backend`

The `dist/backend/<target>/` path is the intended npm-distribution location.

## Launch Model

The CLI starts the Rust gateway as a detached background process and injects the resolved configuration through environment variables such as:

- `CODEX_WEBUI_BASE_PATH`
- `CODEX_WEBUI_CODEX_BIN`
- `CODEX_WEBUI_CODEX_HOME`
- `CODEX_WEBUI_DATA_DIR`
- `CODEX_WEBUI_ALLOWED_ROOTS`
- `CODEX_WEBUI_PASSWORD_HASH`
- `CODEX_WEBUI_SESSION_SECRET`
- `CODEX_WEBUI_CORS_ALLOWED_ORIGINS`

The CLI currently prints a URL ending in `/login`; that route redirects to the workspace root, so either URL is acceptable for end users.

When system shutdown support is enabled, the actual armed and scheduled state is still runtime data rather than static CLI config:

- arming "shutdown after queue completes" happens through the running app
- the armed flag and any scheduled shutdown timestamp are persisted under `CODEX_WEBUI_DATA_DIR`
- the backend remains authoritative, so the shutdown can still execute without an attached browser session

## Tunnel Behavior

`codex-webui tunnel` ensures the server is running and then:

1. tries `cloudflared tunnel --url <base-url>`
2. falls back to `ngrok http <host>:<port>`

This command assumes the selected tunneling tool is already installed and authenticated where necessary.

## Cross Build

Use:

```bash
pnpm build:cross
```

The helper script targets common platforms:

- `x86_64-unknown-linux-gnu`
- `aarch64-unknown-linux-gnu`
- `x86_64-apple-darwin`
- `aarch64-apple-darwin`
- `x86_64-pc-windows-msvc`

Build preference order:

1. `cargo-zigbuild`
2. `cross`
3. plain `cargo`

Built binaries are copied to:

```text
dist/backend/<target>/
```

## Release Notes For Maintainers

Before publishing, check:

1. `pnpm install`
2. `pnpm build`
3. `pnpm build:cross`
4. `pnpm check`
5. `cargo check --manifest-path backend/Cargo.toml`
6. `package.json` includes `bin`, `build`, `dist`, and docs
7. `node ./bin/codex-webui.mjs` works from a clean checkout
8. `npx .` or a packed tarball works on a machine that does not rely on local build artifacts by accident

## Recommended Smoke Tests

Before publishing a package, verify at least:

- first-run interactive setup creates `~/.codex/codex-webui.yml`
- `codex-webui` starts the background server and prints a usable URL
- login works through the printed base path
- WebSocket connection succeeds after login
- `codex-webui restart` and `codex-webui stop` work
- `codex-webui tunnel` selects the expected tunneling tool
- the packaged binary resolution path works without a local Rust build tree
