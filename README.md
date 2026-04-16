# codex-webui

`codex-webui` is a reconnect-safe web workspace for Codex CLI.

It keeps Codex turns running on the server when the browser disconnects, exposes a Claude-like multi-panel UI, and aims to cover the day-to-day workflow people expect from the Codex app and the Codex VS Code extension without requiring VS Code itself.

## Why This Exists

Codex already has strong native surfaces:

- `codex` for local CLI usage
- `codex app` for the desktop app experience
- the Codex IDE extension for editor-integrated workflows

`codex-webui` focuses on a different deployment shape:

- a browser UI you can host on your own machine or server
- reconnect-safe long-running work
- explicit password-gated access
- Git, terminals, queueing, and file inspection in one place
- base-path and reverse-proxy friendly deployment

The goal is not to replace upstream surfaces. The goal is to make Codex usable from a browser while preserving the operational model of local Codex execution.

## Highlights

- Password-protected browser access with signed HTTP-only cookies
- Reconnect-safe WebSocket control plane for chat, sessions, Git, terminals, runtime actions, and account flows
- Session queue, explicit steer flow, persisted queued follow-ups, and resume prompts after restart
- Attachments, Monaco-backed diff/file editing, aggregated live diff, live plan, and subagent activity views
- Git repository discovery, status, diff, commit inspection, branch checkout, and worktree management
- Terminal tabs that survive page reloads as long as the server process stays up
- Runtime install/update checks, quota display, plugin/skill catalog visibility, and `config.toml` editing
- Global "shutdown after queue completes" control that is synchronized across clients and still executes with no browser attached
- Base-path deployment, configurable CORS, dark/light themes, and Paraglide-based i18n

## Current Status

The app is usable today and already supports real work, but the packaging and some internal APIs are still moving.

Stable enough to use:

- background server lifecycle
- browser login
- multi-client session sync
- progressive session loading
- queue persistence
- Git and terminal workflows

Still evolving:

- npm distribution polish
- documentation depth
- parity details with upstream Codex surfaces

## Feature Coverage

`codex-webui` intentionally tracks the workflows people expect from the Codex app and IDE surfaces, but it does not copy them one-to-one.

- For a high-level feature matrix, see [docs/feature-matrix.md](./docs/feature-matrix.md).
- For architecture details, see [docs/architecture.md](./docs/architecture.md).
- For packaging and `npx` distribution details, see [docs/distribution.md](./docs/distribution.md).

## Architecture

`codex-webui` has a narrow public edge and a Codex-focused private layer:

1. the browser loads a single workspace page
2. password login and attachment upload use credentialed HTTP requests
3. session activity, chat, Git, terminals, and runtime state use a reconnect-safe WebSocket RPC channel
4. a Rust gateway owns auth, cookies, WebSocket fan-out, terminal persistence, runtime install/update actions, and static asset serving
5. the Rust gateway starts an internal SvelteKit/Node service that talks to `codex app-server` and implements Codex-specific logic such as session hydration, queue persistence, Git operations, attachment storage, and `config.toml` synchronization

More detail is in [docs/architecture.md](./docs/architecture.md).

## Requirements

- Node.js with `pnpm`
- Rust toolchain
- a working `codex` installation on the machine that will host the server
- access to the Codex home directory you want to expose, usually `~/.codex`

## Quick Start From Source

```bash
pnpm install
pnpm build
pnpm gateway:build
node ./bin/codex-webui.mjs
```

On first launch the CLI opens an interactive setup flow and writes:

- config: `~/.codex/codex-webui.yml`
- runtime state: `~/.codex/codex-webui/`

After setup, running `codex-webui` again starts the background server and prints:

- launch URL
- PID
- config path
- log path

The printed URL may still end in `/login` for compatibility, but that route redirects to the main workspace and the login experience is handled inline by the workspace shell.

## Using The Published CLI

The intended distribution path is:

```bash
npx codex-webui
```

On first run the CLI:

1. asks for host, port, base path, Codex binary, `CODEX_HOME`, allowed roots, optional CORS origins, and password
2. hashes the password with scrypt
3. writes `~/.codex/codex-webui.yml`
4. starts the Rust gateway in the background

Once configured, the CLI supports:

```bash
codex-webui
codex-webui config
codex-webui restart
codex-webui stop
codex-webui tunnel
```

`tunnel` prefers `cloudflared` and falls back to `ngrok`.

More detail is in [docs/distribution.md](./docs/distribution.md).

## Configuration

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

Meaning of the main fields:

- `host` / `port`: public bind address for the Rust gateway
- `basePath`: deployment prefix, for example `/absproxy/4173`
- `codexBin`: path or command name for the Codex CLI binary
- `codexHome`: Codex runtime, config, and session directory
- `dataDir`: `codex-webui` runtime state, uploads, queue state, and editor metadata
- `allowedRoots`: filesystem roots the UI is allowed to browse
- `passwordHash`: hashed login password
- `sessionSecret`: cookie signing secret
- `corsAllowedOrigins`: trusted origins allowed to use browser credentials against the gateway
- `backendBinaryPath`: explicit Rust gateway path, mainly for packaged or custom deployments

## Runtime And Config Behavior

- Session defaults are sourced from `~/.codex/config.toml`.
- The Settings workspace can edit `config.toml` directly.
- Changing session or composer preferences syncs the relevant defaults back into `config.toml`.
- Existing sessions keep their own persisted preferences; changing defaults mainly affects new sessions and future default state.
- Queued follow-ups are stored server-side and can continue after the page closes as long as the server remains up.
- Terminals also stay alive while the Rust gateway remains up.
- "Shutdown after queue completes" is a server-global operational toggle, not a per-session preference.
- When that toggle is armed, the gateway waits until every session queue is empty and no live Codex turn is still running before scheduling shutdown.
- The scheduled shutdown timestamp is persisted in `codex-webui` state, synchronized to every connected client, and can still execute if no client is connected.

## Environment Overrides

The Rust gateway and the internal Node service honor a focused set of `CODEX_WEBUI_*` environment variables. The most important ones are:

- `CODEX_WEBUI_PASSWORD_HASH`
- `CODEX_WEBUI_PASSWORD`
- `CODEX_WEBUI_SESSION_SECRET`
- `CODEX_WEBUI_CORS_ALLOWED_ORIGINS`
- `CODEX_WEBUI_ALLOWED_ROOTS`
- `CODEX_WEBUI_BASE_PATH`
- `CODEX_WEBUI_DATA_DIR`
- `CODEX_WEBUI_CODEX_BIN`
- `CODEX_WEBUI_CODEX_HOME`
- `CODEX_WEBUI_MAX_UPLOAD_MB`
- `CODEX_WEBUI_DEFAULT_*` session defaults such as model, sandbox, approval, speed, effort, network, and steering resume mode
- `CODEX_WEBUI_GIT_DISCOVERY_DEPTH`
- `CODEX_WEBUI_ENABLE_SYSTEM_SHUTDOWN`
- `CODEX_WEBUI_SHUTDOWN_DELAY_SECONDS`
- `CODEX_WEBUI_SHUTDOWN_COMMAND`

See [.env.example](./.env.example) for a concise example set.

## Security Notes

- Prefer `CODEX_WEBUI_PASSWORD_HASH` over plaintext password variables.
- Keep `CODEX_WEBUI_SESSION_SECRET` unique per deployment.
- Restrict `CODEX_WEBUI_ALLOWED_ROOTS` to the smallest practical set.
- Leave cookies on `SameSite=Strict` unless you explicitly need cross-site browser sessions.
- Run behind HTTPS in production.
- Do not expose the internal SvelteKit service directly.
- Git actions are intentionally gated on explicit repository selection.
- System shutdown support is disabled by default and must be explicitly enabled.
- The shutdown control is global to the running server, so all connected clients see the same armed and scheduled state.

## Development

### Frontend dev server only

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

## Troubleshooting

### A session appears to still be running after Codex stopped

The detailed session view reconciles the persisted rollout with the live `thread/loaded/list` state from `codex app-server`. If a rollout still contains `running` or `inProgress` markers after an interrupted process, the UI marks that session as stopped without rewriting the session file.

### A session appears in search but not in the sidebar

The sidebar combines a local session index with live app-server data and loads progressively. A selected session is pinned back into view even if it was not part of the current list page yet.

### Shutdown after queue completion did not trigger

The shutdown timer arms only when both of these are true:

- every persisted session queue is empty
- no Codex thread is still live according to runtime state

If new work is queued or a turn becomes active again, the pending shutdown is cancelled and must be re-armed by those conditions becoming true again.

### Attachments do not upload

Attachment uploads use credentialed `multipart/form-data` requests. Check:

- `CODEX_WEBUI_MAX_UPLOAD_MB`
- allowed filesystem roots
- reverse-proxy body size limits

### `npx codex-webui` cannot start the gateway

Make sure one of these exists:

- `backendBinaryPath` in `~/.codex/codex-webui.yml`
- `CODEX_WEBUI_BACKEND_BIN`
- a matching prebuilt binary under `dist/backend/<target>/`
- a locally built binary under `backend/target/release/`

## Repository Docs

- [docs/architecture.md](./docs/architecture.md)
- [docs/distribution.md](./docs/distribution.md)
- [docs/feature-matrix.md](./docs/feature-matrix.md)

## Upstream References

- the upstream `codex` repository
- the Codex app and Codex IDE surfaces described there
