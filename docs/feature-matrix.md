# Feature Matrix

This document describes how `codex-webui` maps onto the upstream Codex surfaces it references:

- `codex app` for the desktop-app workflow
- the Codex IDE extension for editor-integrated workflows

This is not an official upstream product matrix. It is a project-maintainer view of current `codex-webui` coverage and where the UX intentionally differs.

In the comparison columns:

- `Comparable` means `codex-webui` covers the same broad workflow, even if the UI shape differs
- `Partial` means the analogous upstream workflow is deeper or more polished than the current web implementation
- `Different shape` means the capability exists, but `codex-webui` exposes it in a self-hosted browser-specific way
- `Not targeted` means direct parity is not the point of that row

## Status Labels

- `Implemented`: available in `codex-webui` today
- `Partial`: available, but not yet as polished or as deep as the target workflow
- `Different shape`: supported, but with a browser-specific UX rather than a direct clone
- `Not targeted`: not a current parity goal

## Upstream Compatibility Tracking

`codex-webui` tracks upstream Codex app-server and slash-command drift with:

```bash
pnpm verify:codex-protocol
```

The check reads a sibling Codex checkout at `../codex` by default, or the path in
`CODEX_REPO_PATH`. It verifies that every upstream slash command is classified in
`src/lib/codex-commands.ts` and that each app-server request/notification is
explicitly marked as supported, planned, blocked, or relayed. If the Codex
checkout is missing, the check skips so packaged builds do not depend on a local
source tree.

Compatibility decisions use these buckets:

| Upstream surface | codex-webui policy |
| --- | --- |
| Core thread/session/model/account requests | Proxy through the Rust gateway to the managed Codex app-server when the browser needs the native behavior. |
| Browser-native workflows such as queue, Git workspace, attachments, and tabs | Implement directly in `codex-webui` because they are specific to reconnect-safe web operation. |
| Host-level process, filesystem, plugin install, and MCP approval calls | Keep blocked unless they are wrapped in `codex-webui`'s explicit allowed-root, owner-role, audit, and UI safety model. |
| Emerging upstream features | Classify immediately, then implement as native UI, app-server proxy, planned work, or intentionally blocked behavior before exposing them in the composer. |

## High-Level Comparison

| Capability | codex-webui | Codex app comparison | Codex IDE extension comparison | Notes |
| --- | --- | --- | --- | --- |
| Session list and chat history | Implemented | Comparable | Comparable | Progressive loading, search, archive, reconnect-safe refresh, backend-persisted sidebar badges, and a local rollout metadata index are browser-first additions. |
| Large-history performance | Implemented | Different shape | Different shape | Uses Rust-side rollout candidate scanning, visible-page metadata hydration, recent-turn windows, older-turn paging, and lazy item detail loading so many or very long sessions do not require full transcript serialization up front. |
| Multi-turn streaming chat | Implemented | Comparable | Comparable | Streaming survives tab reloads because execution remains server-side. |
| Model selection | Implemented | Comparable | Comparable | Exposed from the chat composer shell and persisted in session preferences. |
| Multi-account switching | Implemented | Different shape | Different shape | Uses profile-scoped `CODEX_HOME` directories, independent browser profile cookies, profile-aware backend routing, lazy app-server startup, and a cap on active app-server processes rather than a single shared desktop login state. |
| Reasoning effort selection | Implemented | Comparable | Comparable | Matches the app-server preference model rather than copying a specific upstream layout. |
| Plan mode selection | Implemented | Comparable | Comparable | Uses the same session preference model that Codex consumes. |
| Speed mode selection | Implemented | Comparable | Comparable | Supports `auto`, `fast`, and `flex` where available from model metadata. |
| Working directory selection | Implemented | Comparable | Comparable | Chosen per session and constrained to allowed roots. |
| File attachments | Implemented | Comparable | Comparable | Files and images are persisted server-side and attached to turns or queued work. |
| Prompt presets and slash commands | Implemented | Different shape | Different shape | Browser-first composer workflow with server-persisted presets and slash commands for presets, queueing, steering, and quick session preference changes. |
| Admin/viewer browser roles and audit log | Implemented | Different shape | Different shape | `codex-webui` adds self-hosted browser access control with a read-only viewer role and a persisted audit log for privileged actions. |
| Composer history recall and last-message resend | Implemented | Different shape | Different shape | Browser-first affordance for quickly reusing or re-sending the most recent prompt without losing current draft text. |
| Steer during an active turn | Implemented | Comparable | Comparable | Supports explicit steer while work is active. |
| Queued follow-up messages | Implemented | Different shape | Comparable | Queue is a first-class server-side feature in `codex-webui`; it survives browser churn. |
| Resume queued work after restart | Implemented | Different shape | Different shape | Browser-specific persistence flow with optional auto-resume. |
| Multi-client state sync | Implemented | Different shape | Different shape | Designed for several browser clients watching the same session, including shared completion and attention badges in the sidebar. |
| Reconnect-safe execution after disconnect | Implemented | Different shape | Partial | One of the main reasons this project exists. |
| Session search | Implemented | Comparable | Comparable | Supports summary search and optional deeper search scope. |
| Session pinning, tags, and saved filters | Implemented | Different shape | Different shape | Browser-first session organization layer persisted by `codex-webui`, including pinned ordering, reusable filter presets, and server-synced tag metadata. |
| Session title inference | Implemented | Comparable | Comparable | Titles are inferred from Codex turn flow and session summary updates. |
| Monaco inline diff for file changes | Implemented | Different shape | Comparable | Browser-native replacement for editor diff affordances. |
| Aggregated diff view | Implemented | Different shape | Comparable | Exposes grouped file changes per turn and in dedicated tabs. |
| Inline file editing | Implemented | Partial | Comparable | Focused on quick inspection and edits rather than full IDE parity. |
| Git repository discovery | Implemented | Different shape | Comparable | Discovery is depth-limited and gated on explicit repository selection. |
| Git status, fetch/pull, diff, commit inspection | Implemented | Partial | Comparable | Designed as a browser-side Git workspace, not a full desktop VCS client, and now includes VS Code-inspired staged/unstaged Source Control sections, dense one-row actions, tree-or-list change browsing, fetch/pull controls, active-turn safety checks, and a mobile-friendly navigation mode. |
| Git worktree management | Implemented | Partial | Comparable | Explicitly exposed in the web UI because multiple browser workspaces benefit from it. |
| Terminal tabs | Implemented | Different shape | Comparable | Terminals live in the Rust gateway and survive page reloads while the server stays up. |
| Subagent activity view | Implemented | Comparable | Comparable | Subagent activity is rendered inline and can open related threads in tabs. |
| Plugin, app, and skill visibility | Implemented | Different shape | Partial | Lists local `CODEX_HOME` skills/plugins and app-server plugin/app catalog data, including installable marketplace plugins. |
| Computer-use plugin bridge | Partial | Partial | Partial | Proxies Codex plugin/app/realtime protocol surfaces, renders dynamic tool-call text/image output, and documents a WebSocket snapshot-stream transport before taking on WebRTC. |
| `config.toml` editor | Implemented | Different shape | Different shape | Browser-native settings page that syncs session defaults back to Codex config. |
| Runtime install and update checks | Implemented | Different shape | Not targeted | Added for browser-hosted deployments where the runtime might not be present yet. |
| Quota display | Implemented | Comparable | Comparable | Exposed in the account surface with cached refresh support. |
| Notification center and webhook delivery | Implemented | Different shape | Different shape | Browser-native notification inbox with persisted unread state and Slack/generic webhook delivery hooks. |
| Password-protected remote access | Implemented | Different shape | Not targeted | Core web deployment feature rather than upstream app or extension parity. |
| Base path and reverse-proxy support | Implemented | Different shape | Not targeted | Important for self-hosted setups. |
| Configurable CORS | Implemented | Different shape | Not targeted | Needed for controlled remote browser access. |
| Localized actionable error messages | Implemented | Different shape | Different shape | Common queue, steer, and archive conflicts return stable error codes that the browser maps into locale-aware copy. |
| System shutdown after queued work completes | Implemented | Different shape | Not targeted | Global server-side control that waits for all queues and active turns to settle, then persists and syncs the scheduled shutdown state across clients. |

## Areas Where codex-webui Intentionally Differs

### Browser-first transport

Upstream Codex surfaces are not primarily optimized for a self-hosted browser deployment. `codex-webui` is. That is why it uses:

- a Rust public gateway
- signed cookies
- WebSocket fan-out
- a local rollout parser and session index optimized for visible summaries and recent turns
- profile-aware app-server routing instead of swapping one global account file
- long-lived server-side queue and terminal state

### Remote-operations focus

`codex-webui` includes features that matter more in a browser-hosted environment than in a local desktop window:

- base-path support
- CORS controls
- reconnect-safe background execution
- optional remote tunnel flow
- password-gated access to a locally running Codex runtime
- global operational controls, such as synchronized shutdown-after-queue-completion scheduling

### Git and workspace affordances

The project leans into browser-native Git inspection and workspace management:

- repository discovery under configured roots
- staged and unstaged Source Control sections similar to VS Code
- dense tree or flat-list file browsing for change sets
- grouped file-change views
- Monaco diff panes
- worktree operations
- safety checks that block destructive repo mutations while a Codex turn is active in that repo

These features are meant to make remote Codex usage practical, not to replace a full desktop Git client.

## Gaps To Expect

Even where the matrix says `Implemented`, some areas are still rougher than the upstream native surfaces:

- interaction polish
- performance on very large histories
- packaging maturity
- exact control placement and naming parity

The goal is workflow parity first, pixel parity second.
