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

The project also optimizes for a case where native Codex surfaces can become expensive: many historical sessions, very long rollouts, and browser clients that reconnect frequently. Instead of asking Codex to materialize every session and every turn up front, the web gateway maintains its own lightweight session index, parses only the rollout metadata and recent turn window needed for the current view, and hydrates older turns or large tool details only when the user asks for them.

## Highlights

- Password-protected browser access with signed HTTP-only cookies
- Multi-account profile switching backed by separate `CODEX_HOME` directories and profile-scoped `codex app-server` instances
- Profile-aware backend routing, so each browser session is routed to the correct Codex account without rewriting a shared `auth.json`
- Lazy profile activation with a configurable cap on active Codex app-server processes, avoiding one app-server per configured account at startup
- Reconnect-safe WebSocket control plane for chat, sessions, Git, terminals, runtime actions, and account flows
- Request-ID dedupe at the public gateway so reconnect replays do not execute queue or other mutating RPC calls twice, with role/method/parameter matching and response-size budgets
- Custom local rollout parser and session index for large histories: sidebar pages, search summaries, recent turns, and per-item details are loaded progressively instead of pulling entire session transcripts into the browser
- Operational probes for `/healthz`, `/readyz`, and admin-only `/metrics`
- Dedicated runtime error logs under `<dataDir>/logs/` for Rust gateway failures and per-profile `codex app-server` stderr
- Optional owner role for host-level capabilities such as terminals, runtime install/update, forced worktree removal, and dangerous session approval/sandbox settings
- Session queue, explicit steer flow, persisted queued follow-ups, and resume prompts after restart
- Composer history recall with keyboard navigation, a quick "reuse last message" chip, and one-click resend/queue for the most recent prompt
- Session completion and input-required badges are persisted server-side, so they survive reconnects and show up consistently across multiple clients
- Attachments, Monaco-backed diff/file editing, aggregated live diff, live plan, and subagent activity views
- Git repository discovery, status, fetch/pull, staged/unstaged Source Control sections, dense tree-or-list change browsing, commit inspection, branch checkout, and worktree management in a VS Code-inspired Git workspace
- Terminal tabs that survive page reloads as long as the server process stays up
- Runtime install/update checks, quota display, plugin/skill/app catalog visibility, and `config.toml` editing
- Computer-use groundwork through Codex app-server plugin/app proxies, dynamic tool-call responses, image/text tool output rendering, remote-control status events, and a documented WebSocket snapshot-stream path before any WebRTC dependency
- Persistent notification center with unread sync, plus Slack and generic webhook delivery for completion, attention, queue-failure, and shutdown events
- Session pinning, per-session tags, and saved sidebar filters that persist on the server and stay synchronized across clients
- Prompt presets stored server-side, plus composer slash commands for presets, queueing, steering, model changes, and plan-mode toggles
- Optional admin/viewer role split with server-enforced read-only access and a persisted audit log for privileged actions
- Structured backend error codes mapped to localized UI messages for common queue, steer, and archive timing failures
- Cross-platform per-user automatic startup management from the Settings page, with Windows Startup, macOS LaunchAgent, and Linux systemd/XDG support
- Global "shutdown after queue completes" control that is synchronized across clients and still executes with no browser attached
- Base-path deployment, configurable CORS, dark/light themes, and Paraglide-based i18n

## Current Status

The app is usable today and already supports real work, but the packaging and some internal APIs are still moving.

Stable enough to use:

- background server lifecycle
- browser login
- multi-client session sync
- progressive session loading
- large-history session listing and recent-turn hydration
- queue persistence
- Git and terminal workflows

Recent UI work worth calling out:

- a denser Git Source Control workspace with separate staged and unstaged sections
- tree and flat-list change browsing for repository diffs
- faster one-row file actions for open, stage, and unstage flows
- mobile navigation that preserves the same Git workflow in a compressed layout

Still evolving:

- npm distribution polish
- documentation depth
- parity details with upstream Codex surfaces

## Feature Coverage

`codex-webui` intentionally tracks the workflows people expect from the Codex app and IDE surfaces, but it does not copy them one-to-one.

- For a high-level feature matrix, see [docs/feature-matrix.md](./docs/feature-matrix.md).
- For architecture details, see [docs/architecture.md](./docs/architecture.md).
- For packaging and `npx` distribution details, see [docs/distribution.md](./docs/distribution.md).
- For computer-use and realtime transport decisions, see [docs/computer-use.md](./docs/computer-use.md).

## Architecture

`codex-webui` has a narrow public edge and a Codex-focused private layer:

1. the browser loads a single workspace page
2. password login and attachment upload use credentialed HTTP requests
3. session activity, chat, Git, terminals, and runtime state use a reconnect-safe WebSocket RPC channel
4. a Rust gateway owns auth, cookies, WebSocket fan-out, terminal persistence, runtime install/update actions, session and Git APIs, and static asset serving
5. the Rust gateway serves the prebuilt static SPA from `build/static`, rewrites the compile-time base-path placeholder at response time, and talks directly to per-profile `codex app-server` processes for live Codex state
6. the Rust gateway also owns the browser-facing session index and rollout-window parser so long histories can be paged, searched, and expanded without loading every transcript item
7. SvelteKit server hooks and `/api` route handlers are removed from the frontend source tree; all shipped API behavior now lives in Rust

More detail is in [docs/architecture.md](./docs/architecture.md).

## Requirements

- Node.js with `pnpm`
- Rust toolchain
- a working `codex` installation on the machine that will host the server
- access to the Codex home directory or directories you want to expose, usually `~/.codex` for a single profile or separate paths such as `~/.codex-work` and `~/.codex-personal` for multiple profiles

## Quick Start From Source

```bash
pnpm install
pnpm build
node ./bin/codex-webui.mjs
```

On first launch the CLI opens an interactive setup flow and writes:

- config: `~/.codex/codex-webui.yml`
- runtime state: `~/.codex/codex-webui/`

After setup, running `codex-webui` again starts the background server and prints:

- launch URL
- PID
- config path
- CLI launcher log path
- runtime error log path

The Rust gateway writes its own non-blocking runtime log to
`<dataDir>/logs/codex-webui-gateway.log`. Slow WebSocket request warnings are
rate-limited there so a closed code-server terminal or an undrained stdout pipe
cannot stall request handling.

`pnpm build` produces the public SPA bundle under `build/static`, builds the Rust gateway in release mode, and copies the current-platform gateway binary to `dist/backend/<target>/` for the CLI.

For migration regression checks, run:

```bash
pnpm verify:static-build
pnpm verify:api-parity
pnpm verify:codex-protocol
pnpm verify:security-regressions
```

The CLI prints the workspace root URL and the login experience is handled inline by the workspace shell.

## Using The Published CLI

The intended distribution path is:

```bash
npx codex-webui
```

On first run the CLI:

1. asks for host, port, base path, Codex binary, the global data directory, one or more profile-specific `CODEX_HOME` paths, allowed roots, optional CORS origins, password, optional owner password, and optional hCaptcha keys
2. hashes the password with scrypt
3. writes `~/.codex/codex-webui.yml`
4. starts the Rust gateway in the background

Once configured, the CLI supports:

```bash
codex-webui
codex-webui config
codex-webui status
codex-webui restart
codex-webui stop
codex-webui tunnel start --yes
codex-webui tunnel status
codex-webui tunnel stop
codex-webui tunnel logs
codex-webui --hcaptcha-site-key <site-key> --hcaptcha-secret-key <secret>
```

`codex-webui restart` prepares a Codex app-server handoff before it stops the gateway. On Unix this handoff is enabled by default: active Codex work is kept behind a local control socket, then the newly started gateway attaches through a fresh proxy process. Normal `codex-webui stop` still tears down the managed Codex app-server. If `CODEX_WEBUI_APP_SERVER_HANDOFF=false` disables handoff while app-server clients are active, restart is refused instead of silently killing in-flight turns.

`tunnel` supports provider selection, background or foreground execution, status inspection, and log inspection. It prefers `cloudflared` when available and falls back to `ngrok`. Starting a public tunnel prints a safety checklist and requires explicit confirmation; use `--yes` only after reviewing the exposure.

You can also override login protection at launch time with `--hcaptcha-site-key`, `--hcaptcha-secret-key`, or `--disable-hcaptcha`.

More detail is in [docs/distribution.md](./docs/distribution.md).

## Configuration

The interactive CLI stores YAML at `~/.codex/codex-webui.yml`.

Example:

```yaml
host: 127.0.0.1
port: 4173
basePath: /absproxy/4173
codexBin: codex
dataDir: /home/user/.codex/codex-webui/data
defaultProfileId: work
profiles:
  - id: work
    label: Work
    codexHome: /home/user/.codex-work
    dataDir: /home/user/.codex/codex-webui/data/profiles/work
  - id: personal
    label: Personal
    codexHome: /home/user/.codex-personal
    dataDir: /home/user/.codex/codex-webui/data/profiles/personal
allowedRoots:
  - /home/user/work
passwordHash: scrypt$...
ownerPasswordHash: scrypt$...
sessionSecret: ...
corsAllowedOrigins: []
backendBinaryPath: ""
tunnel:
  provider: auto
  background: true
  hostname: ""
  name: ""
  overwriteDns: false
  logLevel: info
  extraArgs: []
```

Meaning of the main fields:

- `host` / `port`: public bind address for the Rust gateway
- `basePath`: deployment prefix, for example `/absproxy/4173`
- `codexBin`: path or command name for the Codex CLI binary
- `dataDir`: global `codex-webui` runtime state, uploads, queue state, notifications, and editor metadata
- `defaultProfileId`: the profile selected for new browser sessions unless a different profile cookie is already set
- `profiles`: named Codex runtimes, each with its own `CODEX_HOME` and profile-local data directory
- `allowedRoots`: filesystem roots the UI is allowed to browse
- `passwordHash`: hashed login password
- `ownerPasswordHash`: stronger owner login for terminal, runtime install/update, shutdown, and other host-level operations; required before starting a public tunnel
- `sessionSecret`: cookie signing secret
- `corsAllowedOrigins`: trusted origins allowed to use browser credentials against the gateway
- `backendBinaryPath`: explicit Rust gateway path, mainly for packaged or custom deployments
- `tunnel`: optional CLI defaults for tunnel provider selection, run mode, and provider-specific arguments

## Multi-Account Profiles

`codex-webui` supports multiple Codex accounts by treating each account as a profile with its own `CODEX_HOME`.

- A profile should point at a distinct directory such as `~/.codex-work` or `~/.codex-personal`.
- Codex stores `auth.json`, `config.toml`, sessions, plugins, and skills under `CODEX_HOME`, so separating profiles at that level avoids account collisions.
- The web UI keeps profile state lightweight at startup and starts `codex app-server` processes only when a profile receives an active Codex request.
- This means two browsers can stay connected to different accounts at the same time without swapping a shared `~/.codex/auth.json` file.
- Active Codex app-server processes are capped by `CODEX_WEBUI_MAX_APP_SERVERS` and default to `1`; raise it only when the host has enough memory for concurrent profiles.

If you only want one account at a time, you can still keep a single profile and swap `auth.json` manually before restart. For simultaneous multi-account use, separate `CODEX_HOME` directories are the intended model.

## Runtime And Config Behavior

- Session defaults are sourced from the active profile's `CODEX_HOME/config.toml`.
- With multiple profiles, each profile reads and writes its own `CODEX_HOME/config.toml` and `CODEX_HOME/auth.json`.
- The Settings workspace can edit `config.toml` directly.
- Changing session or composer preferences syncs the relevant defaults back into `config.toml`.
- Existing sessions keep their own persisted preferences; changing defaults mainly affects new sessions and future default state.
- If a saved draft exists while a session is still hydrating, local input typed into the composer wins; draft restore will not clobber text or attachments the user entered during loading.
- Queued follow-ups are stored server-side and can continue after the page closes as long as the server remains up.

Resource limits:

- `CODEX_WEBUI_MAX_APP_SERVERS`: maximum active Codex app-server processes across profiles. Default: `1`.
- `CODEX_WEBUI_SERVER_THREADS`: gateway Tokio worker threads. Default: up to `2` based on available parallelism.
- `CODEX_WEBUI_BLOCKING_THREADS`: gateway blocking pool threads. Default: `max(server_threads * 2, 4)`.
- `CODEX_WEBUI_SERVER_THREAD_STACK_BYTES`: gateway worker stack size. Default and minimum: `16777216`.
- `CODEX_WEBUI_CONTROLLER_THREADS`: Codex app-server controller worker threads. Default: up to `2`.
- For constrained code-server containers, start with `CODEX_WEBUI_SERVER_THREADS=1`,
  `CODEX_WEBUI_BLOCKING_THREADS=4`, and the default stack size. Lowering the
  stack can crash deep session/JSON processing with Rust stack overflow.
- Terminals also stay alive while the Rust gateway remains up.
- "Shutdown after queue completes" is a server-global operational toggle, not a per-session preference.
- When that toggle is armed, the gateway waits until every session queue is empty and no live Codex turn is still running before scheduling shutdown.
- The scheduled shutdown timestamp is persisted in `codex-webui` state, synchronized to every connected client, and can still execute if no client is connected.
- Notification center history and webhook settings are also persisted in `codex-webui` runtime state so multiple clients see the same unread counts and delivery configuration.
- Session organization metadata such as pins, tags, and saved sidebar filters also lives in `codex-webui` runtime state rather than ephemeral browser storage.
- Prompt presets also live in `codex-webui` runtime state so slash-command behavior stays consistent across browsers.
- Audit entries for privileged login and WebSocket actions are appended to `CODEX_WEBUI_DATA_DIR/audit-log.jsonl`.

## Environment Overrides

The Rust gateway honors a focused set of `CODEX_WEBUI_*` environment variables. The most important ones are:

- `CODEX_WEBUI_PASSWORD_HASH`
- `CODEX_WEBUI_PASSWORD`
- `CODEX_WEBUI_OWNER_PASSWORD_HASH`
- `CODEX_WEBUI_OWNER_PASSWORD`
- `CODEX_WEBUI_VIEWER_PASSWORD_HASH`
- `CODEX_WEBUI_VIEWER_PASSWORD`
- `CODEX_WEBUI_SESSION_SECRET`
- `CODEX_WEBUI_HCAPTCHA_SITE_KEY`
- `CODEX_WEBUI_HCAPTCHA_SECRET_KEY`
- `CODEX_WEBUI_CORS_ALLOWED_ORIGINS`
- `CODEX_WEBUI_REQUIRE_OWNER`
- `CODEX_WEBUI_REQUIRE_ORIGIN_HEADER`
- `CODEX_WEBUI_WEBHOOK_ALLOWED_HOSTS`
- `CODEX_WEBUI_TRUST_PROXY_HEADERS`
- `CODEX_WEBUI_INSTANCE_TOKEN` for CLI-owned health verification of background processes
- `CODEX_WEBUI_ALLOWED_ROOTS`
- `CODEX_WEBUI_BASE_PATH`
- `CODEX_WEBUI_DATA_DIR`
- `CODEX_WEBUI_CODEX_BIN`
- `CODEX_HOME`
- `CODEX_WEBUI_DEFAULT_PROFILE_ID`
- `CODEX_WEBUI_PROFILES_JSON`
- `CODEX_WEBUI_MAX_UPLOAD_MB`
- `CODEX_WEBUI_MAX_ATTACHMENT_STORAGE_MB`
- `CODEX_WEBUI_DEFAULT_*` session defaults such as model, sandbox, approval, speed, effort, network, and steering resume mode
- `CODEX_WEBUI_GIT_DISCOVERY_DEPTH`
- `CODEX_WEBUI_ENABLE_SYSTEM_SHUTDOWN`
- `CODEX_WEBUI_SHUTDOWN_DELAY_SECONDS`
- `CODEX_WEBUI_SHUTDOWN_COMMAND`

See [.env.example](./.env.example) for a concise example set.

## Security Notes

- Prefer `CODEX_WEBUI_PASSWORD_HASH`, `CODEX_WEBUI_OWNER_PASSWORD_HASH`, and `CODEX_WEBUI_VIEWER_PASSWORD_HASH` over plaintext password variables. Plaintext password variables are rejected when the gateway binds to a non-loopback `HOST`.
- Prefer config or environment variables for hCaptcha secrets; command-line flags can leak through shell history and process inspection.
- If you need read-only browser access, prefer `CODEX_WEBUI_VIEWER_PASSWORD_HASH` over the plaintext viewer password variable.
- Set `CODEX_WEBUI_SESSION_SECRET` to a unique random value of at least 32 bytes per deployment; the gateway will not fall back to a password-derived cookie signing key.
- Optional hCaptcha login protection is only enabled when both `CODEX_WEBUI_HCAPTCHA_SITE_KEY` and `CODEX_WEBUI_HCAPTCHA_SECRET_KEY` are set.
- Set `CODEX_WEBUI_ALLOWED_ROOTS` explicitly and restrict it to the smallest practical set; the gateway does not infer broad fallback roots.
- Leave cookies on `SameSite=Strict` unless you explicitly need cross-site browser sessions.
- Run behind HTTPS in production.
- Leave `CODEX_WEBUI_TRUST_PROXY_HEADERS` unset unless the gateway only receives traffic from a trusted reverse proxy that controls `X-Forwarded-*` headers. When enabling it behind a non-loopback proxy, set `CODEX_WEBUI_TRUSTED_PROXY_CIDRS`.
- Externally bound deployments reject unsafe HTTP mutations that omit `Origin`; keep API clients on loopback or send a trusted Origin.
- Set `CODEX_WEBUI_REQUIRE_ORIGIN_HEADER=true` to reject Origin-less HTTP mutations even on loopback deployments.
- WebSocket upgrades check `Origin` against same-origin or configured CORS origins; do not rely on HTTP CORS alone when exposing the gateway.
- Cookie-authenticated HTTP mutations use a double-submit CSRF token; non-browser automation should prefer WebSocket RPC or explicitly carry the issued CSRF cookie/header pair.
- Set `CODEX_WEBUI_WEBHOOK_ALLOWED_HOSTS` when webhook delivery is enabled in exposed deployments, so Slack/generic notification webhooks can only target known outbound hosts.
- Login and JSON mutation bodies are size-limited, attachment uploads are streamed with per-file/request/profile storage caps, and large file/diff previews are bounded.
- User-facing HTTP and WebSocket errors redact common token-shaped values and the host user's home directory; detailed diagnostics go to server logs instead.
- Use the viewer password for observation-only access instead of sharing the admin password when multiple humans need browser visibility.
- Git actions are intentionally gated on explicit repository selection.
- Pull, branch switch, and worktree removal are blocked while a live or pending Codex turn is using that repository.
- If `ownerPasswordHash` is configured or `CODEX_WEBUI_REQUIRE_OWNER=true`, host-level controls require owner login rather than ordinary admin login.
- Terminal tabs run shell processes with the host user privileges of the gateway process; allowed roots only constrain the initial working directory and UI file tools, not arbitrary shell commands.
- System shutdown support is disabled by default and must be explicitly enabled.
- The shutdown control is global to the running server, so all connected clients see the same armed and scheduled state.
- File editor access to `CODEX_HOME` is limited to `config.toml`; broader project files must be under explicit allowed roots.
- Notification webhook URLs are validated when settings are saved and DNS-checked again immediately before delivery, so stale or corrupted state cannot silently post to local/private targets.

## Development

### Frontend dev server only

```bash
pnpm dev
```

### Full application

```bash
pnpm build
node ./bin/codex-webui.mjs
```

### Verification

```bash
pnpm check
pnpm build
pnpm verify:static-build
pnpm verify:api-parity
pnpm verify:codex-protocol
pnpm verify:security-regressions
cargo check --manifest-path backend/Cargo.toml
```

## Troubleshooting

### A session appears to still be running after Codex stopped

The detailed session view reconciles the persisted rollout with the live `thread/loaded/list` state from `codex app-server`. If a rollout still contains `running` or `inProgress` markers after an interrupted process, the UI marks that session as stopped without rewriting the session file.

### A session appears in search but not in the sidebar

The sidebar combines a local session index with live app-server data and loads progressively. A selected session is pinned back into view even if it was not part of the current list page yet.

### A queued follow-up was sent twice after a reconnect

The public WebSocket gateway treats request IDs as reconnect-safe replay keys. If the browser reconnects and resends a request with the same ID before the first response arrives, the gateway now waits for the original result instead of executing the same mutating action twice.

### A session shows "Done" or "Needs input" on one device but not another

Those sidebar badges are backend-owned state, not browser-local UI markers. `codex-webui` persists them in its own runtime store and includes them in session summaries, so reconnecting browsers and newly opened clients see the same completion or attention state until the session is acknowledged.

### I typed into the composer while a session was loading

Composer input is treated as authoritative once you start typing. If an older saved draft for that session exists, `codex-webui` skips restoring it rather than overwriting the text or attachments you already entered locally.

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

### The server keeps logging `invalid_grant: Invalid refresh token`

`codex app-server` can surface that error when the stored ChatGPT refresh token is no longer valid. `codex-webui` now degrades account reads to `requiresOpenaiAuth` so the workspace still loads, but the affected profile must be re-authenticated before account-specific features recover.

### `npx codex-webui` cannot start the gateway

Make sure one of these exists:

- `backendBinaryPath` in `~/.codex/codex-webui.yml`
- `CODEX_WEBUI_BACKEND_BIN`
- a matching prebuilt binary under `dist/backend/<target>/`
- a locally built binary under `backend/target/release/`

When building from source, `pnpm build` creates both the static frontend and the current-platform Rust gateway binary.

## Repository Docs

- [docs/architecture.md](./docs/architecture.md)
- [docs/distribution.md](./docs/distribution.md)
- [docs/feature-matrix.md](./docs/feature-matrix.md)

## Upstream References

- the upstream `codex` repository
- the Codex app and Codex IDE surfaces described there
