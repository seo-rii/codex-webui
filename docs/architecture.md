# Architecture

## Overview

`codex-webui` is intentionally split into a public edge and a Codex-focused private layer:

1. browser UI
2. Rust public gateway
3. internal SvelteKit/Node service
4. `codex app-server`

The browser never talks to Codex directly.

## Component Roles

### Browser

The browser renders a single workspace page and keeps very little authoritative state locally.

- credentialed HTTP is used for password login/logout and multipart attachment upload
- WebSocket RPC is used for session activity, chat, queue operations, Git, terminals, account flows, and runtime actions
- the UI is optimistic where it helps responsiveness, but server state remains authoritative
- saved-draft restore is intentionally subordinate to active local composer input, so hydration does not overwrite text the user typed while the session was still loading

### Rust gateway

The Rust process is the public entrypoint.

It is responsible for:

- password validation and signed cookie issuance
- admin/viewer role resolution for browser sessions
- static asset serving
- public WebSocket upgrade handling and fan-out
- long-lived terminal processes
- runtime install/update checks
- quota fetching
- spawning and supervising the internal Node service
- enforcing base path and CORS policy
- appending audit-log entries for privileged login and WebSocket actions

### Internal SvelteKit/Node service

The internal service contains most Codex-specific application logic:

- `codex app-server` client management
- session hydration and turn shaping
- queue persistence and dispatch
- notification center persistence and webhook delivery settings
- session organization persistence for pins, tags, and saved sidebar filters
- prompt preset persistence for composer shortcuts and slash-command expansion
- attachment storage
- Git repository discovery, fetch/pull flows, and file operations
- `config.toml` synchronization
- plugin and skill catalog reads
- local session indexing and search

This service is not meant to be exposed directly.

### Codex app-server

`codex app-server` remains the source of truth for live Codex thread execution, thread metadata, and stream notifications.

## Request And Event Flow

### 1. Authentication

- browser calls `POST /api/auth/login`
- Rust validates the password
- Rust sets the signed `codex_webui_auth` cookie
- subsequent HTTP and WebSocket requests reuse that cookie

### 2. Workspace control

- browser opens the public WebSocket exposed by Rust
- Rust authenticates the socket
- browser sends JSON-RPC-like requests
- Rust either handles the request directly or forwards it to the internal Node service

### 3. Codex execution

- Node talks to `codex app-server`
- live notifications from `codex app-server` are normalized into UI events
- Rust fans those events out to every subscribed browser client

### 4. Attachment uploads

Uploads are a deliberate exception to the WebSocket-first rule:

- browser sends `multipart/form-data`
- Rust proxies the upload path to the internal service
- Node stores the upload and associates it with the target session

## Why Rust + Node

The split is deliberate rather than transitional.

Rust is the public edge because it is a good fit for:

- cookie auth
- WebSocket lifecycle management
- terminal persistence
- durable fan-out to multiple clients
- runtime actions that should survive browser churn
- packaging as the publicly exposed backend binary

Node remains the Codex-facing layer because it already matches the `codex app-server` model well and contains the higher-level session logic that is easier to iterate on there.

## Persistence Model

Several kinds of state live on disk:

- Codex sessions under `~/.codex/sessions`
- user defaults under `~/.codex/config.toml`
- `codex-webui` runtime state under `CODEX_WEBUI_DATA_DIR`
- uploaded attachments under `CODEX_WEBUI_DATA_DIR/uploads`
- privileged-action audit history under `CODEX_WEBUI_DATA_DIR/audit-log.jsonl`
- CLI background server metadata under `~/.codex/codex-webui/`

This separation matters:

- Codex rollout files remain Codex-owned
- UI queue/draft/editor state remains `codex-webui`-owned
- global operational state, such as a scheduled shutdown-after-queue-completion timer, remains `codex-webui`-owned
- long-running work can survive browser disconnects because the server-side state is durable

## Session Listing And Search

The sidebar is built from two sources:

- live `thread/list` data from `codex app-server`
- a local JSONL-style session index built from `~/.codex/sessions`
- `codex-webui`-owned per-session sidebar metadata such as completion or attention highlights
- `codex-webui`-owned per-session organization metadata such as pins and tags
- `codex-webui`-owned prompt preset metadata used by the composer and settings workspace

The local index is used because large session histories make direct thread enumeration expensive. The index work runs in a worker so the main Node event loop does not block while the sidebar updates.

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
- because the schedule lives on disk and in the Rust/Node backend, it can still fire with zero connected browsers

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

The terminal lifecycle is intentionally separate from Codex thread lifecycle.

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
2. `~/.codex/config.toml`

The UI can edit `config.toml` directly. Session preference changes also write the relevant defaults back into that file so the web UI and Codex CLI do not silently drift apart.

## Security Model

The trust boundary is narrow:

- public browser traffic reaches only the Rust gateway
- the internal Node service is protected behind the gateway
- browser sessions can authenticate as either admin or viewer, and the Rust gateway enforces the write boundary before forwarding WebSocket methods
- filesystem browsing is limited to allowed roots plus Codex-owned config/runtime paths
- Git actions require explicit repository selection
- cookies are signed and HTTP-only
- cross-origin browser access must be explicitly allowed

The model is designed to reduce accidental exposure, not to make an untrusted multi-tenant Codex host safe by default.

## UI Error Contract

For expected user-facing failures, the backend avoids leaking raw timing-dependent strings as the primary UX contract.

Instead:

- route handlers and gateway logic emit stable application error codes
- the browser parses those codes
- Paraglide message catalogs provide localized copy for each known case

This keeps common race conditions, queue conflicts, archive state mismatches, and read-only role violations understandable across locales without forcing the frontend to pattern-match arbitrary exception text.
