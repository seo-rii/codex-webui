# Computer Use And Realtime Transport

`codex-webui` tracks the newer Codex app-server surfaces that power plugins,
apps, dynamic tool calls, and realtime interaction. The goal is to expose the
useful parts of computer-use in the browser without forcing the project into a
full WebRTC media stack before the UX needs it.

## Current Support

Implemented today:

- `plugin/list`, `plugin/read`, `plugin/install`, and `plugin/uninstall` are
  proxied through the Rust gateway.
- `app/list` is proxied and rendered in the Settings workspace.
- Codex plugin entries are merged into the web catalog, can be installed from
  the UI, and can be mentioned from the composer as `plugin://...` mentions.
- Dynamic tool-call requests can be answered from the browser with structured
  `contentItems`, including text and image content.
- Dynamic tool-call transcript items render returned text and image payloads
  instead of falling back to raw JSON.
- Computer-use image content from dynamic tool calls is detected by the gateway
  and forwarded as `codex-webui/computerFrame` session events. The browser shows
  the latest frame in the Computer workspace tab without reloading the full
  transcript.
- `remoteControl/status/changed` and `app/list/updated` app-server events are
  forwarded into the browser event model.
- `/apps`, `/plugins`, and `/realtime` slash commands are classified and wired
  to browser actions.
- `thread/realtime/listVoices`, `thread/realtime/start`,
  `thread/realtime/appendText`, and `thread/realtime/stop` are available over
  the reconnect-safe WebSocket RPC channel.

Not implemented yet:

- microphone/speaker capture
- WebRTC SDP negotiation
- continuous low-latency video transport
- direct browser control of a remote desktop surface

Those are intentionally separate from the app/plugin protocol bridge. The bridge
lets users install, inspect, and invoke computer-use-capable plugins first.

## Transport Direction

### Recommended MVP: WebSocket Snapshot Stream

For the first browser-visible computer-use surface, use the existing WebSocket
RPC connection or a dedicated authenticated WebSocket stream to send periodic
frames.

Suggested shape:

- app-server or gateway emits a best-effort screen snapshot event only while the
  computer-use panel or session subscription is active
- payload should prefer low-quality AVIF/WebP/JPEG snapshots when the source can
  encode them cheaply; existing data URLs are forwarded as-is for compatibility
- server throttles to a small rate such as 0.2-1 FPS by default
- every frame supersedes the previous frame; the client drops stale frames if it
  is still decoding or rendering
- frame size is capped, downscaled, and quality-limited before sending
- input events such as click, key, scroll, and text are separate reliable RPC
  messages with explicit target/session ids

This is less efficient than WebRTC for video, but it fits the current deployment
model:

- works through existing HTTPS reverse proxies and code-server base paths
- reuses the current cookie/auth/origin/role model
- avoids ICE/TURN/DTLS/SCTP setup
- keeps resource use bounded and auditable
- is good enough for "look at a few frames, click, wait for next frame" workflows

This should not be treated as real video. It is a remote-inspection channel with
occasional frames.

Using AV1 as a stateful video stream is deferred. At the low frame rates useful
for computer-use inspection, per-frame AVIF snapshots are simpler to reconnect,
cache, and audit. A true AV1 stream only becomes attractive once inter-frame
compression is needed and the project is ready to own encoder state, keyframe
recovery, and browser decoder compatibility.

### Optional Later Path: WebTransport Datagrams

WebTransport datagrams are a better semantic fit for best-effort screen frames:
delivery is unreliable, unordered, and low latency, so new frames can supersede
older frames without head-of-line blocking.

However, WebTransport is not a drop-in replacement for the current deployment:

- it requires HTTPS secure contexts
- it is tied to HTTP/3/QUIC infrastructure
- it may not pass through every reverse proxy or code-server base-path setup
- it needs feature detection and a WebSocket fallback
- it adds a second transport stack beside the existing WebSocket RPC channel

Use it later as an opt-in acceleration path:

1. keep WebSocket snapshots as the baseline
2. add `CODEX_WEBUI_COMPUTER_STREAM_TRANSPORT=auto|websocket|webtransport`
3. use WebTransport datagrams only when browser support and server/proxy support
   are both confirmed
4. keep control/input events on reliable RPC unless there is a reason to move
   them

Reference material:

- MDN documents WebTransport datagrams as a Baseline 2026 feature and describes
  their unreliable, unordered delivery model:
  <https://developer.mozilla.org/en-US/docs/Web/API/WebTransport/datagrams>
- Chrome's WebTransport guide notes that WebSockets remain the more robust
  out-of-the-box choice for common server setups, while datagrams are useful for
  low-latency best-effort data:
  <https://developer.chrome.com/docs/capabilities/web-apis/webtransport>

### Deferred Path: WebRTC

WebRTC remains the right tool for high-frame-rate audio/video and very low
latency interaction, but it is the wrong first dependency for `codex-webui`.

Reasons to defer:

- negotiation and failure modes are much broader than the rest of the app
- TURN/ICE/network behavior complicates self-hosted and tunneled deployments
- it requires a distinct media lifecycle UI
- it is harder to audit than a bounded snapshot stream

The WebRTC path should only be revisited if users need continuous interactive
video/audio rather than occasional screen frames.

## Safety Constraints

Computer-use UI should remain owner/admin gated unless a narrower permission
model is added.

Before sending frames:

- require an explicit open computer-use panel or selected session subscription
- redact or disable streaming for viewer role
- cap frame bytes, frame rate, and total per-client buffered frames
- stop streaming immediately on panel close, session close, or WebSocket close
- log privileged stream start/stop events

Before accepting input:

- require the session id to match the active computer-use context
- route every input command through the same role and audit model as other
  host-facing actions
- reject stale input sequence ids after a stream reset
