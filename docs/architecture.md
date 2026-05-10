# Architecture

## Overview

`codex-webui` is intentionally split into a public edge and a Codex-focused private layer:

1. browser UI
2. Rust public gateway
3. `codex app-server`

The browser never talks to Codex directly.

## Component Roles

### Browser

The browser renders a single workspace page and keeps very little authoritative state locally.

- credentialed HTTP is used for password login/logout and multipart attachment upload
- WebSocket RPC is used for session activity, chat, queue operations, Git, terminals, account flows, and runtime actions
- the UI is optimistic where it helps responsiveness, but server state remains authoritative
- session screens initially receive summaries and the most recent turn window; older turns, large command output, MCP calls, and file diffs are fetched lazily
- saved-draft restore is intentionally subordinate to active local composer input, so hydration does not overwrite text the user typed while the session was still loading

### Rust gateway

The Rust process is the public entrypoint.

It is responsible for:

- password validation and signed cookie issuance
- admin/viewer role resolution for browser sessions
- static asset serving from the prebuilt SPA bundle
- runtime base-path placeholder replacement for HTML, JS, and CSS assets
- public WebSocket upgrade handling and fan-out
- request-ID response caching and in-flight dedupe for reconnect-safe RPC replay
- WebSocket Origin enforcement, request concurrency caps, message-size caps, and bounded response dedupe cache memory
- long-lived terminal processes
- runtime install/update checks
- quota fetching
- session, queue, attachment, Git, editor, notification, and runtime API handling
- local session rollout indexing, summary hydration, recent-turn parsing, older-turn paging, and lazy transcript item detail loading
- profile-aware routing to the correct Codex app-server, with lazy startup and a cap on concurrently active profile runtimes
- enforcing base path and CORS policy
- appending audit-log entries for privileged login and WebSocket actions
- exposing lightweight `/healthz`, `/readyz`, and admin-only `/metrics` diagnostics

### Codex app-server

`codex app-server` remains the source of truth for live Codex thread execution, thread metadata, and stream notifications.

For cold history reads, `codex-webui` may serve browser-optimized summary and recent-turn payloads from local rollout files first, then reconcile with app-server state when live status matters. This keeps the browser responsive even when the upstream app-server would have to enumerate or serialize very large histories.

## Request And Event Flow

### 0. Public page load

- browser requests the configured base path from Rust
- Rust serves files from `build/static`
- HTML, JS, and CSS assets contain a compile-time base-path placeholder
- Rust rewrites that placeholder to the configured runtime base path before sending text assets
- unknown non-asset paths fall back to the SPA entry document so the single-page shell can hydrate

### 1. Authentication

- browser calls `POST /api/auth/login`
- Rust validates the password
- Rust sets the signed `codex_webui_auth` cookie
- subsequent HTTP and WebSocket requests reuse that cookie

### 2. Workspace control

- browser opens the public WebSocket exposed by Rust
- Rust authenticates the socket
- browser sends JSON-RPC-like requests
- Rust handles the request directly
- mutating requests are keyed by profile, role, and client request ID
- replays only reuse an in-flight or cached response when the method and parameter hash match; conflicting reuse returns an error instead of executing a second action
- very large responses are not stored in the replay cache, preventing multi-megabyte session reads from accumulating in memory

### 3. Codex execution

- Rust talks to `codex app-server`
- live notifications from `codex app-server` are normalized into UI events
- Rust fans those events out to every subscribed browser client

### 4. Attachment uploads

Uploads are a deliberate exception to the WebSocket-first rule:

- browser sends `multipart/form-data`
- Rust streams the upload to disk, enforces per-file/request/profile storage limits, and associates it with the target session

## Why Rust + app-server

The current split is deliberate:

- Rust is the public edge and owns browser-facing state, auth, API handling, terminal persistence, and reconnect safety
- `codex app-server` stays responsible for live Codex execution and runtime thread notifications

## Performance Model For Large Histories

Long Codex sessions and large session directories are one of the main reasons the gateway does more than simple proxying.

The browser-facing session path is optimized around bounded payloads:

- session listing starts from filesystem candidates ordered by modification time rather than waiting for a full app-server history enumeration
- candidate metadata is hydrated from Codex state databases or rollout headers only for the visible page
- the local rollout parser reads bounded tail windows for session detail, so opening a long thread loads recent turns first
- older turns are requested with an explicit cursor and limit
- large transcript items, command output, MCP payloads, file changes, and Monaco diffs stay collapsed until expanded
- summary pages include per-session version hashes so clients can request diffs and avoid replacing the whole list when only a few sessions changed

This gives the web UI a different performance profile from a native local surface. A browser reconnect can restore the visible shell quickly, then progressively hydrate deeper history without blocking chat input, queue operations, or live WebSocket notifications.

The same principle applies to account routing:

- each configured account is represented by a profile with its own `CODEX_HOME`
- Rust chooses the profile from the signed browser cookie and routes app-server RPC to that profile
- app-server processes start only when a profile receives active Codex work or an account/runtime request
- `CODEX_WEBUI_MAX_APP_SERVERS` caps concurrent profile runtimes so a host with many configured accounts does not launch every backend at startup

The result is a multi-backend architecture without making the browser manage backend selection, auth files, or process lifetimes.

## Multi-Account Runtime Model

Multi-account support is implemented as runtime profiles rather than by mutating one shared `~/.codex/auth.json`.

- each profile has its own `CODEX_HOME`
- each profile therefore has its own `auth.json`, `config.toml`, session tree, skills, and plugins
- the browser chooses an active profile through a signed HTTP-only profile cookie
- Rust resolves a profile-local `codex app-server` client and profile-local UI state store for the current request context
- account bootstrap reads are profile-local and use a degraded `requiresOpenaiAuth` fallback if an upstream refresh token is no longer valid

This makes simultaneous use practical:

- browser A can stay connected to profile `work`
- browser B can stay connected to profile `personal`
- both can stream, queue, inspect quota, and browse sessions without clobbering each other's Codex state

## Persistence Model

Several kinds of state live on disk:

- per-profile Codex sessions under `<CODEX_HOME>/sessions`
- per-profile user defaults under `<CODEX_HOME>/config.toml`
- per-profile auth under `<CODEX_HOME>/auth.json`
- `codex-webui` runtime state under `CODEX_WEBUI_DATA_DIR`
- uploaded attachments under `CODEX_WEBUI_DATA_DIR/uploads`
- privileged-action audit history under `CODEX_WEBUI_DATA_DIR/audit-log.jsonl`
- CLI background server metadata under `~/.codex/codex-webui/`

This separation matters:

- Codex rollout files remain Codex-owned
- UI queue/draft/editor state remains `codex-webui`-owned
- global operational state, such as a scheduled shutdown-after-queue-completion timer, remains `codex-webui`-owned
- long-running work can survive browser disconnects because the server-side state is durable
- important `codex-webui`-owned JSON/TOML state is written via temp-file, fsync, rename, and best-effort parent-directory fsync
- the CLI writes config, PID, server metadata, tunnel metadata, and tunnel logs atomically

## Session Listing And Search

The sidebar is built from two sources:

- live `thread/list` data from `codex app-server`
- a local JSONL-style session index built from `~/.codex/sessions`
- `codex-webui`-owned per-session sidebar metadata such as completion or attention highlights
- `codex-webui`-owned per-session organization metadata such as pins and tags
- `codex-webui`-owned prompt preset metadata used by the composer and settings workspace

The local index is used because large session histories make direct thread enumeration expensive. It does not blindly parse every full transcript for every sidebar refresh. It first scans bounded rollout candidates, then hydrates the metadata needed for the current page, search result, or selected session. Expensive parsing work runs off the hot request path so session-list refreshes do not block the main server flow.

Session detail uses the same strategy. The first detail response contains only the recent turn window and hydration metadata. If the user scrolls upward, the browser asks for older turn pages. If the user expands a collapsed tool call or file change, the browser asks for that item's detail. This is why a very long session can remain usable in the web UI without sending a multi-megabyte transcript on every reconnect.

Completion and input-required badges are not treated as frontend-only affordances. They are persisted in `codex-webui` state, injected into session summaries, and cleared by backend acknowledgement flows when a user opens the relevant session or resolves the pending request state.

Pins, tags, and saved sidebar filters follow the same principle: they are stored in backend-owned UI state, merged into session summaries before they reach the browser, and broadcast back out through config/session-summary updates so multiple clients stay in sync without inventing local conflict resolution rules.

Prompt presets are treated similarly. They are saved in backend-owned UI state, exposed through the config payload, edited from the settings workspace, and consumed by composer-side slash commands without depending on local browser storage.

## Runtime Session Reconciliation

Persisted rollout data can lag behind real runtime state. For example, a rollout may still contain `running` or `inProgress` markers after a crash or abrupt shutdown.

To avoid mutating Codex-owned session files just to fix the UI:

- `codex-webui` reads the rollout
- it also queries `thread/loaded/list` from `codex app-server`
- when a session looks live on disk but is not loaded in memory, the UI response is normalized to a stopped state

This reconciliation is applied when loading:

- session detail
- older turn pages
- individual turn expansion
- queue dispatch decisions that depend on whether a turn is really still active

## Queue Model

Queued follow-ups are stored in `CODEX_WEBUI_DATA_DIR`, not in Codex rollout files.

Important properties:

- queue items survive page reloads and browser disconnects
- queue draining happens server-side
- restart flows can require user confirmation before resuming queued work
- auto-resume can be enabled per session

Queue state is also part of the global shutdown-after-completion story:

- the shutdown toggle is global to the running server, not scoped to one thread
- arming it writes the intent into persisted `codex-webui` state
- scheduling only happens once all queues are empty and no live turn remains active
- if new work appears, the pending shutdown is cancelled and the updated state is broadcast to all clients
- because the schedule lives on disk and in the Rust backend, it can still fire with zero connected browsers

Queue mutations also use structured application errors for expected conflicts such as:

- empty queued messages
- queue items that have already been removed or dispatched
- dispatch attempts while another queued item is already being sent

Those errors are emitted from the backend as stable codes and translated into localized browser copy at the UI layer.

## Terminal Model

The Rust gateway owns terminal processes.

- terminal output is buffered server-side
- terminal tabs survive browser reloads while the gateway stays up
- terminal input/output is streamed incrementally over WebSocket
- terminal creation is capped, idle/exited sessions are cleaned up, and Unix shutdown targets the process group with TERM followed by KILL if the group ignores TERM
- terminal sessions run as the same host user as the gateway; the terminal feature is not a filesystem sandbox and should be paired with external OS/container isolation for untrusted or public deployments

The terminal lifecycle is intentionally separate from Codex thread lifecycle.

## Operational Probes

The Rust gateway exposes:

- `/healthz` for process liveness and version/build metadata
- `/readyz` for readiness checks such as profile presence, allowed-root configuration, and `CODEX_WEBUI_DATA_DIR` writability
- `/metrics` for admin-authenticated Prometheus-style counters covering cache sizes, active turns, terminals, relays, and pending server requests

The CLI starts the gateway with a per-instance token and stores it in `~/.codex/codex-webui/server.json`. `codex-webui status`, `stop`, and `restart` verify that token against `/healthz` before managing the recorded PID, which avoids acting on a reused PID that belongs to another process.

For restart handoff, the gateway runs Codex app-server as a long-lived local control-socket process and talks to it through `codex app-server proxy`. Unix handoff is enabled by default. Before `codex-webui restart` sends SIGTERM, it calls an instance-token-protected handoff endpoint so the gateway closes only its proxy connections. The replacement gateway may be a different backend binary; it attaches to the same Codex app-server socket and resumes receiving session state. A normal `codex-webui stop` does not set that flag, so graceful shutdown closes the proxy and terminates the managed Codex app-server. If handoff is disabled while active app-server clients exist, restart preparation fails to avoid killing active turns.

## Global Operational State

Some UI-visible state is intentionally shared across every connected client rather than living inside one session.

Current examples include:

- queued-work resume prompts restored after restart
- globally armed or scheduled shutdown-after-queue-completion state
- persisted per-session completion and attention highlights used by the sidebar
- persisted per-session pins, tags, and saved sidebar filters
- persisted prompt presets used by the composer and settings workspace
- notification center history, unread state, and webhook settings
- persisted audit history for privileged login and RPC activity

This state is persisted in `CODEX_WEBUI_DATA_DIR`, exposed through config payloads and global WebSocket notifications, and treated as authoritative by the backend so reconnecting clients do not need to rebuild it from local browser memory.

## Config Sources

Session defaults are resolved from:

1. `CODEX_WEBUI_*` environment overrides
2. the active profile's `<CODEX_HOME>/config.toml`

The UI can edit `config.toml` directly. Session preference changes also write the relevant defaults back into that file so the web UI and Codex CLI do not silently drift apart.

## Security Model

The trust boundary is narrow:

- public browser traffic reaches only the Rust gateway
- browser sessions can authenticate as either admin or viewer, and the Rust gateway enforces the write boundary before handling WebSocket methods
- deployments can also configure an owner password; when present, host-level controls such as terminal access, runtime install/update, forced worktree removal, auto-approve session settings, no-prompt approval policies, and `danger-full-access` sandbox selection require owner role
- filesystem browsing is limited to allowed roots plus Codex-owned config/runtime paths
- Git actions require explicit repository selection
- destructive Git mutations such as pull, branch switch, and worktree removal refuse to run while a live or pending Codex turn is associated with the same repository
- cookies are signed and HTTP-only
- cross-origin browser access must be explicitly allowed
- cookie paths are scoped to the configured base path
- unsafe HTTP mutations reject cross-origin requests unless the origin is explicitly trusted
- CSP disallows inline scripts while retaining inline styles for Svelte-generated and existing component styles
- WebSocket upgrades validate Origin separately from HTTP CORS
- cookie-authenticated HTTP mutations require a matching CSRF cookie/header token, except for initial login and instance-token maintenance hooks
- `CODEX_WEBUI_REQUIRE_OWNER=true` forces owner-role checks for host-level actions even on loopback deployments, which is useful when a manual reverse proxy exposes the gateway
- `CODEX_WEBUI_REQUIRE_ORIGIN_HEADER=true` can make Origin headers mandatory for unsafe HTTP methods even on loopback deployments
- default viewer access is transcript-oriented; code, terminal, audit, config, and Git file reads remain admin-only
- file reads/writes deny common secret paths and bound preview sizes
- forwarded headers are ignored unless `CODEX_WEBUI_TRUST_PROXY_HEADERS=true` and the peer is loopback or matches `CODEX_WEBUI_TRUSTED_PROXY_CIDRS`
- file reads/writes inside `CODEX_HOME` are restricted to `config.toml`; other workspace file access must come from explicit allowed roots
- webhook URLs must use HTTPS and cannot target localhost, `.local`, or private/link-local IP literal targets; delivery also DNS-checks the persisted URL before sending and can be restricted with `CODEX_WEBUI_WEBHOOK_ALLOWED_HOSTS`

The model is designed to reduce accidental exposure, not to make an untrusted multi-tenant Codex host safe by default.

## UI Error Contract

For expected user-facing failures, the backend avoids leaking raw timing-dependent strings as the primary UX contract.

Instead:

- route handlers and gateway logic emit stable application error codes
- the browser parses those codes
- Paraglide message catalogs provide localized copy for each known case

This keeps common race conditions, queue conflicts, archive state mismatches, and read-only role violations understandable across locales without forcing the frontend to pattern-match arbitrary exception text.
