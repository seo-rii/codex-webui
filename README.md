# codex-webui

`codex-webui` is a web interface for Codex CLI.

It serves a SvelteKit frontend behind a Rust gateway, keeps Codex turns running on the server when the browser disconnects, and exposes most of the workflow people expect from the Codex VS Code extension:

- session and chat management
- model, reasoning, plan, speed, sandbox, approval, and network controls
- queue and steer flows
- file attachments
- Git status, diff, commits, worktrees, and file editing
- terminal tabs that survive page reloads while the server stays up
- runtime install/update checks and quota display
- plugin and skill catalog visibility

## Status

The project is usable today, but it is still moving quickly. Expect UI and API changes while the packaging and distribution story settles.

## Architecture

The public surface is intentionally narrow:

- the browser uses HTTP for authentication only
- everything else is driven through a reconnect-safe WebSocket RPC layer
- a Rust gateway owns auth, session cookies, WebSocket fan-out, terminal persistence, and runtime management
- the gateway starts an internal Node/SvelteKit server for the Codex-specific proxy logic
- the internal server talks to `codex app-server` and manages session history, queue state, attachments, Git tooling, and Codex preferences

More detail is in [docs/architecture.md](./docs/architecture.md).

## Features

- Password-protected web UI with signed HTTP-only cookies
- Optional cross-origin API support for trusted origins
- Case-insensitive session search
- Session queue with persisted follow-up messages
- Explicit `Steer now` flow for queued work
- Resume prompts for queued work after a server restart
- Browser notifications and sidebar highlighting for completed sessions and approval-required sessions
- Aggregated live diff and plan panels above the transcript
- Monaco diff views for inline change review and dedicated diff tabs
- File editor backed by Monaco
- Config editor for `~/.codex/config.toml`
- Git repository discovery under allowed roots
- Git worktree list, create, open, and remove
- Persistent terminal tabs
- Account login, device-code login, quota display, and runtime install/update actions
- Plugin and skill catalog view sourced from the local Codex installation

## Quick Start

### 1. Install Codex CLI

Make sure `codex` is installed and works on the machine where the server will run.

### 2. Run the web UI

For a local checkout:

```bash
pnpm install
pnpm build
cargo build --release --manifest-path backend/Cargo.toml
node ./bin/codex-webui.mjs
```

The first launch opens an interactive setup and writes:

- config: `~/.codex/codex-webui.yml`
- runtime state: `~/.codex/codex-webui/`

After that, running `codex-webui` again starts the background server and prints the URL, PID, config path, and log path.

## `npx codex-webui`

The package is designed to be published so users can run:

```bash
npx codex-webui
```

On first launch the CLI:

1. asks for host, port, base path, Codex binary, `CODEX_HOME`, allowed roots, and password
2. hashes the password
3. writes `~/.codex/codex-webui.yml`
4. starts the background server

Once configured, these commands are available:

```bash
codex-webui
codex-webui config
codex-webui restart
codex-webui stop
codex-webui tunnel
```

`tunnel` uses `cloudflared` when available and falls back to `ngrok`.

## CLI Config

The interactive CLI stores YAML at `~/.codex/codex-webui.yml`.

Example:

```yaml
host: 127.0.0.1
port: 4173
basePath: /absproxy/4173
codexBin: codex
codexHome: /home/user/.codex
dataDir: /home/user/.codex/codex-webui/data
allowedRoots:
  - /home/user/work
passwordHash: scrypt$...
sessionSecret: ...
corsAllowedOrigins: []
backendBinaryPath: ""
```

## Development

### Frontend only

```bash
pnpm dev
```

### Full application

```bash
pnpm build
cargo run --manifest-path backend/Cargo.toml
```

### Verification

```bash
pnpm check
pnpm build
cargo check --manifest-path backend/Cargo.toml
```

## Packaging

The npm package ships the CLI entrypoint and expects prebuilt Rust gateway binaries under `dist/backend/<target>/`.

Build common targets with:

```bash
pnpm build:cross
```

The helper script prefers `cargo-zigbuild`, falls back to `cross`, and then to plain `cargo build` if needed.

More detail is in [docs/distribution.md](./docs/distribution.md).

## Security

- Prefer `CODEX_WEBUI_PASSWORD_HASH` over plaintext password environment variables.
- Keep `CODEX_WEBUI_SESSION_SECRET` unique per deployment.
- Restrict `CODEX_WEBUI_ALLOWED_ROOTS` to the smallest practical set.
- Leave cookies on `SameSite=Strict` unless you explicitly need cross-site browser sessions.
- Run behind HTTPS in production.
- Do not expose the internal SvelteKit server directly.
- System shutdown support is disabled by default and must be explicitly enabled.

## Runtime and Config Notes

- Session defaults are sourced from `~/.codex/config.toml`.
- Changing composer preferences updates `config.toml`.
- The Settings workspace tab lets you edit `config.toml` directly and reload the defaults visible in the UI.
- Existing sessions keep their own persisted preferences; changing defaults mainly affects new sessions and future default state.

## Troubleshooting

### Sessions appear in search but not in the sidebar

The sidebar is built from a local session index plus live app-server data. If a session is selected directly, it is pinned back into the visible list even when it did not arrive in the current page yet.

### Attachments do not upload

Uploads are sent as `multipart/form-data` to the internal attachment endpoint. Check:

- `CODEX_WEBUI_MAX_UPLOAD_MB`
- allowed filesystem roots
- reverse proxy request size limits

### `npx codex-webui` cannot start the gateway

Make sure one of these exists:

- `backendBinaryPath` in `~/.codex/codex-webui.yml`
- `CODEX_WEBUI_BACKEND_BIN`
- a matching prebuilt binary under `dist/backend/<target>/`
- a locally built binary under `backend/target/release/`

## References

- [docs/architecture.md](./docs/architecture.md)
- [docs/distribution.md](./docs/distribution.md)
- the upstream `codex` repository
- the `cdx` project as a secondary reference for app-style usage patterns
