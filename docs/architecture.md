# Architecture

## Overview

`codex-webui` is split into three layers:

1. SvelteKit frontend
2. Rust public gateway
3. Internal Node/SvelteKit Codex proxy

The browser never talks to Codex directly.

## Request Flow

### Auth

- Browser calls `POST /api/auth/login`
- Rust validates the password and sets the signed auth cookie
- Subsequent browser requests use that cookie

### Runtime RPC

- Browser opens `WebSocket /ws`
- Rust authenticates the socket
- Browser sends JSON-RPC-like requests
- Rust either handles them locally or proxies them to the internal Node server

### Codex Execution

- Internal Node server wraps `codex app-server`
- Session operations, turn streaming, queue state, attachments, and Git operations live there
- Rust relays events back to every subscribed browser

## Why split Rust and Node?

Rust owns the public edge because it is a better place for:

- cookie auth
- reconnect-safe WebSocket relays
- terminal persistence
- request caching
- runtime install/update checks
- tunnel-friendly single-binary serving

Node remains the Codex-facing layer because the existing proxy logic is already implemented there and closely matches the `codex app-server` model.

## Persistence

Several kinds of state are persisted server-side:

- Codex sessions under `~/.codex/sessions`
- queued follow-ups and UI state under `CODEX_WEBUI_DATA_DIR`
- uploaded attachments under `CODEX_WEBUI_DATA_DIR/uploads`
- CLI background-server metadata under `~/.codex/codex-webui/`

This lets a session continue while the browser disconnects.

## Session List Strategy

The sidebar combines:

- live `thread/list` data from Codex app-server
- a local JSONL session index built from `~/.codex/sessions`

The JSONL scan runs in a worker thread so long session histories do not block the main Node event loop.

## Terminals

The Rust gateway owns terminal processes.

- terminal output is buffered in memory
- terminal tabs survive page reloads while the gateway stays up
- frontend writes are subscribed incrementally over WebSocket

## Config Sources

Session defaults come from:

1. environment variables
2. `~/.codex/config.toml`

The UI can edit `config.toml`, and per-session preference changes also synchronize back into that file.

## Security Model

- public browser traffic only reaches the Rust gateway
- the internal Node server is protected by a random internal header token
- filesystem access is restricted to allowed roots plus Codex-owned configuration locations
- Git actions require explicit repository selection
- cookies are HTTP-only and signed
