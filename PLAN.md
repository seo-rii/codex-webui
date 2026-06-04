# PLAN

This document tracks the next implementation direction for `codex-webui`.

The current priority is upstream Codex feature coverage without regressing the
large-history performance model that makes this project usable in a hosted
browser environment.

## Guiding Decisions

### Keep The Custom Rollout Parser

`codex-webui` must keep its Rust-side rollout parser and session index as the
primary source for sidebar lists, search, recent-turn windows, and lazy item
detail loading.

Native Codex thread APIs are still valuable, but they are not the right primary
path for large session histories. The current expectation is that native thread
enumeration materializes substantially more session state than the web sidebar
needs. On machines with many sessions or very long rollouts, that can make a
simple session list take minutes, and in observed usage it can exceed ten
minutes before the UI becomes useful.

The parser should therefore be treated as a performance boundary:

- use native Codex APIs as a correctness reference, reconciliation source, and
  live-thread authority
- keep sidebar list/search/detail bootstrapping on bounded local parsing
- compare sampled parser output against native thread output to find parser
  bugs
- fix parser gaps directly instead of replacing the parser with native listing
- never require loading full transcripts before the user can send, queue, steer,
  or switch sessions

### Prefer Native Protocols For Mutating Codex State

When upstream Codex exposes an API that represents Codex-owned state directly,
`codex-webui` should prefer the native API unless it conflicts with browser
reconnect safety or large-history performance.

This applies to:

- archive and unarchive
- memory controls
- review and rollback actions
- plugin, marketplace, skill, and MCP management
- image generation, standalone web search, and other typed transcript items

`thread/settings/update` is explicitly deferred for now. Existing
`codex-webui` session preference synchronization stays in place until the
native settings API is reviewed more deeply.

## Implementation Order

### 1. Parser Parity And Stability

Objective: keep the custom parser, but make it easier to prove where it differs
from native Codex output.

Planned work:

- add a sampled parser-vs-native comparison command or diagnostics panel
- compare title, created/updated timestamps, archived/subagent flags, latest
  turn status, goal state, and visible recent items
- record mismatches with stable categories so they can be fixed one by one
- add regression fixtures for known hard cases:
  - subagent sessions appearing in the active list
  - crashed context-compression turns looking active
  - completed goal sessions retaining stale running state
  - latest assistant response missing until a full refresh
  - corrupted or non-UTF-8 rollout tails

Acceptance criteria:

- sidebar loading still uses bounded parser/index reads
- native APIs are not on the hot path for every session-list refresh
- parser mismatch output is actionable enough to debug individual sessions

### 2. `clientUserMessageId` Everywhere

Objective: give every user-authored message a stable client id across send,
steer, and queued dispatch paths.

Status: implemented for current send, steer, queue, queued dispatch, and
optimistic reconciliation paths. The browser generates stable client ids for
composer submissions, the backend passes them to native `turn/start` and
`turn/steer`, queue items persist them, queued dispatch reuses them, and tests
cover native app-server propagation. Future work should focus on keeping this
model intact as new message entry points are added.

Planned work:

- generate or preserve a client-side id for every composer submission
- pass `clientUserMessageId` to native `turn/start`
- pass `clientUserMessageId` to native `turn/steer`
- persist the same id on queue items before dispatch
- pass the stored id when queued items are dispatched
- reconcile optimistic user messages against Codex `userMessage.clientId`
  instead of matching only by text or temporary local ids
- include the id in retry/dedupe logic so reconnect replay does not duplicate
  visible messages

Acceptance criteria:

- send, steer, queue, retry, and reconnect all use the same id model
- optimistic user messages do not disappear and reappear as duplicates
- queued items can be edited/reordered without losing their eventual
  `clientUserMessageId`

### 3. Diagnostics Workspace Tab

Objective: expose runtime and parser diagnostics from the `Open` menu as a
dedicated tab.

Status: implemented for the current diagnostics surface. The `Open` menu has a
Diagnostics tab with runtime status, WebSocket status, managed Codex processes,
active/queued/attention session counts, recent runtime notifications,
host-memory/OOM indicators, and explicit parser-vs-native comparison for a
selected session. Expensive native comparison remains user-triggered so sidebar
loading stays on the bounded parser/index path.

Planned work:

- add `Diagnostics` to the `Open` menu
- show gateway health, app-server processes, active sessions, queue drain state,
  WebSocket status, cache sizes, parser/index status, and recent runtime errors
- include parser-vs-native comparison tools from stage 1
- make expensive checks explicit actions, not automatic page-load work

Acceptance criteria:

- opening diagnostics does not start every Codex app-server
- owner/admin-only data remains gated
- large outputs are paged or lazy-loaded

### 4. Native Archive And Unarchive

Objective: move archive/restore actions onto upstream Codex APIs where possible.

Status: implemented. Archive and restore actions call native `thread/archive`
and `thread/unarchive`, runtime archive notifications reconcile sidebar state,
and tests verify native thread state and sidebar visibility stay aligned.

Planned work:

- call native `thread/archive` and `thread/unarchive` for Codex-owned archive
  state
- keep `codex-webui` local metadata only for browser-specific filters and
  transition compatibility
- update sidebar parser/index reconciliation to respect native archive state
- keep existing snackbar/error-code behavior

Acceptance criteria:

- archive state matches Codex app/extension expectations
- archived sessions do not reappear because of local index cache staleness

### 5. Memory Workspace Tab

Objective: add a first-class memory UI available from the `Open` menu.

Status: implemented for the currently exposed upstream memory surface. The
workspace inspects profile memory settings/storage without starting every
app-server, calls native `memory/reset`, and exposes native
`thread/memoryMode/set` for the selected session. Current Codex `thread/read`
does not directly return memory mode, so the UI reports that limitation instead
of inventing local state.

Planned work:

- add `Memory` to the `Open` menu
- display current memory mode/state where upstream exposes it
- support native memory reset where allowed
- clearly separate global/profile memory from per-session preferences
- include permission gating and audit logging for destructive memory actions

Acceptance criteria:

- memory state can be inspected without opening a chat session
- destructive actions require the same owner/admin policy as other host-impacting
  runtime operations

### 6. Plugin, Marketplace, Skill, And MCP Management

Objective: turn catalog visibility into actionable management surfaces.

Status: implemented for the currently exposed upstream catalog and MCP
surfaces. Settings now supports marketplace add/remove/upgrade, plugin
detail/install/uninstall, native skill and hook list inspection, MCP server
status/tool/resource inspection, MCP reload, and OAuth login launch. Extra skill
root editing remains limited by upstream surface availability and can still be
done through `config.toml`.

Planned work:

- manage native plugins:
  - list installed plugins
  - inspect details
  - install/uninstall where supported
  - refresh plugin auth/cache state
- manage marketplace plugins:
  - browse available plugins
  - add/remove/upgrade
  - surface install errors with structured error codes
- manage skills:
  - list active skill roots
  - configure extra skill roots where supported
  - show unavailable or invalid skill metadata clearly
- improve MCP management:
  - list server status with native details
  - show tools/resources exposed by each server
  - expose restart/refresh actions only when safe

Acceptance criteria:

- catalog changes invalidate cached composer/search data
- viewer role receives redacted catalog data only
- install/update operations are owner-gated and audited

### 7. Rich Transcript Item Support

Objective: render newer Codex transcript item types as first-class cards.

Status: partially implemented. The local rollout parser now mirrors upstream
thread history for standalone web search begin/end events, image generation
begin/end events, local image-view tool calls, review-mode entry/exit markers,
and thread rollback markers. The chat UI renders generated images with
open/download actions, local image-view references, structured web-search
details, and review findings when upstream provides them. Native `review/start`
is available through `/review`, and loaded user-message turns can execute native
`thread/rollback` after an explicit confirmation. File-level rollback preview
and rollback targets outside the loaded turn window are available through a
lazy-loaded target picker.

Planned work:

- improve image generation item support:
  - show generated images in chat [done]
  - lazy-load large image payloads
  - include download/open actions [done]
  - avoid embedding large base64 images in default session payloads
- add standalone web search cards:
  - render query and status [done]
  - render citations and result summary [done for rollout payloads that include
    `summary`, `results`, `sources`, or `citations`; upstream currently exposes
    `web_search_call.action` as the stable typed field]
  - keep raw result details lazy-loaded [done]
- add review mode UI:
  - start review sessions [done via `/review`, with `--detached` support]
  - display review-mode markers [done]
  - show findings with file references and severity [done when present]
  - link findings to file/diff tabs [done]
- add rollback UI:
  - list rollback targets [done with lazy-loaded explicit target picker]
  - preview affected files before execution [done for file-change items in the
    loaded rollback range]
  - require confirmation for destructive rollback actions [done for direct thread rollback]

Acceptance criteria:

- new item types do not break older sessions
- large generated media and search payloads stay collapsed/lazy by default
- Codex history rollback clearly states that it does not revert file changes

### 8. Deferred Native Thread Settings Sync

Status: deferred.

Native `thread/settings/update` should not be implemented in this batch.

Before adopting it, verify:

- how it interacts with existing `config.toml` sync
- whether it replaces or only complements current session preference writes
- how model, reasoning, plan mode, speed mode, sandbox, approval, language
  bridge, and 100M-context settings are represented
- how errors and unsupported fields are returned by upstream Codex

## Test Strategy

Each implementation stage should include targeted tests instead of relying on a
full manual UI pass.

Required coverage:

- parser fixtures for large-history and stale-status edge cases
- client id propagation tests for send, steer, and queue dispatch
- WebSocket reconnect tests that replay client ids without duplicating messages
- role-policy tests for diagnostics, memory, plugin, marketplace, skill, MCP,
  review, rollback, and media payloads
- lazy-loading tests for image generation and web search item details
- archive/unarchive reconciliation tests against native Codex responses

Manual checks should focus on:

- very large session directories
- mobile reconnect
- browser refresh during active turn
- multiple clients watching one session
- code-server reverse-proxy/base-path deployment

## Non-Goals For This Batch

- Replacing the custom parser with native `thread/list`
- Implementing native `thread/settings/update`
- Starting every app-server at page load
- Adding WebRTC as a required computer-use dependency
- Making the browser the source of truth for Codex-owned session state
