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

## High-Level Comparison

| Capability | codex-webui | Codex app comparison | Codex IDE extension comparison | Notes |
| --- | --- | --- | --- | --- |
| Session list and chat history | Implemented | Comparable | Comparable | Progressive loading, search, archive, and reconnect-safe refresh are browser-first additions. |
| Multi-turn streaming chat | Implemented | Comparable | Comparable | Streaming survives tab reloads because execution remains server-side. |
| Model selection | Implemented | Comparable | Comparable | Exposed from the chat composer shell and persisted in session preferences. |
| Reasoning effort selection | Implemented | Comparable | Comparable | Matches the app-server preference model rather than copying a specific upstream layout. |
| Plan mode selection | Implemented | Comparable | Comparable | Uses the same session preference model that Codex consumes. |
| Speed mode selection | Implemented | Comparable | Comparable | Supports `auto`, `fast`, and `flex` where available from model metadata. |
| Working directory selection | Implemented | Comparable | Comparable | Chosen per session and constrained to allowed roots. |
| File attachments | Implemented | Comparable | Comparable | Files and images are persisted server-side and attached to turns or queued work. |
| Steer during an active turn | Implemented | Comparable | Comparable | Supports explicit steer while work is active. |
| Queued follow-up messages | Implemented | Different shape | Comparable | Queue is a first-class server-side feature in `codex-webui`; it survives browser churn. |
| Resume queued work after restart | Implemented | Different shape | Different shape | Browser-specific persistence flow with optional auto-resume. |
| Multi-client state sync | Implemented | Different shape | Different shape | Designed for several browser clients watching the same session. |
| Reconnect-safe execution after disconnect | Implemented | Different shape | Partial | One of the main reasons this project exists. |
| Session search | Implemented | Comparable | Comparable | Supports summary search and optional deeper search scope. |
| Session title inference | Implemented | Comparable | Comparable | Titles are inferred from Codex turn flow and session summary updates. |
| Monaco inline diff for file changes | Implemented | Different shape | Comparable | Browser-native replacement for editor diff affordances. |
| Aggregated diff view | Implemented | Different shape | Comparable | Exposes grouped file changes per turn and in dedicated tabs. |
| Inline file editing | Implemented | Partial | Comparable | Focused on quick inspection and edits rather than full IDE parity. |
| Git repository discovery | Implemented | Different shape | Comparable | Discovery is depth-limited and gated on explicit repository selection. |
| Git status, diff, commit inspection | Implemented | Partial | Comparable | Designed as a browser-side Git workspace, not a full desktop VCS client. |
| Git worktree management | Implemented | Partial | Comparable | Explicitly exposed in the web UI because multiple browser workspaces benefit from it. |
| Terminal tabs | Implemented | Different shape | Comparable | Terminals live in the Rust gateway and survive page reloads while the server stays up. |
| Subagent activity view | Implemented | Comparable | Comparable | Subagent activity is rendered inline and can open related threads in tabs. |
| Plugin and skill visibility | Implemented | Different shape | Partial | Lists locally installed plugins and skills from `CODEX_HOME`. |
| `config.toml` editor | Implemented | Different shape | Different shape | Browser-native settings page that syncs session defaults back to Codex config. |
| Runtime install and update checks | Implemented | Different shape | Not targeted | Added for browser-hosted deployments where the runtime might not be present yet. |
| Quota display | Implemented | Comparable | Comparable | Exposed in the account surface with cached refresh support. |
| Password-protected remote access | Implemented | Different shape | Not targeted | Core web deployment feature rather than upstream app or extension parity. |
| Base path and reverse-proxy support | Implemented | Different shape | Not targeted | Important for self-hosted setups. |
| Configurable CORS | Implemented | Different shape | Not targeted | Needed for controlled remote browser access. |
| System shutdown after queued work completes | Implemented | Different shape | Not targeted | Optional operational feature for self-hosted runs. |

## Areas Where codex-webui Intentionally Differs

### Browser-first transport

Upstream Codex surfaces are not primarily optimized for a self-hosted browser deployment. `codex-webui` is. That is why it uses:

- a Rust public gateway
- signed cookies
- WebSocket fan-out
- long-lived server-side queue and terminal state

### Remote-operations focus

`codex-webui` includes features that matter more in a browser-hosted environment than in a local desktop window:

- base-path support
- CORS controls
- reconnect-safe background execution
- optional remote tunnel flow
- password-gated access to a locally running Codex runtime

### Git and workspace affordances

The project leans into browser-native Git inspection and workspace management:

- repository discovery under configured roots
- grouped file-change views
- Monaco diff panes
- worktree operations

These features are meant to make remote Codex usage practical, not to replace a full desktop Git client.

## Gaps To Expect

Even where the matrix says `Implemented`, some areas are still rougher than the upstream native surfaces:

- interaction polish
- performance on very large histories
- packaging maturity
- exact control placement and naming parity

The goal is workflow parity first, pixel parity second.
