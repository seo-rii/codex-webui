# Distribution

## Goal

The intended user-facing entrypoint is:

```bash
npx codex-webui
```

That implies the published package must be able to:

- guide a first-time user through setup
- start the server in the background
- resolve a runnable Rust gateway binary for the current platform
- serve the already-built static frontend bundle

## What The Package Needs To Ship

The npm package should include:

- `bin/codex-webui.mjs`
- the built static frontend under `build/static`
- docs and helper scripts that the CLI depends on
- Rust gateway binaries under `dist/backend/<target>/`

The package already exposes:

```json
{
  "bin": {
    "codex-webui": "./bin/codex-webui.mjs"
  }
}
```

## CLI Contract

`bin/codex-webui.mjs` is responsible for:

- first-run interactive setup
- reading and writing `~/.codex/codex-webui.yml`
- starting, restarting, and stopping the background server
- verifying the recorded background PID with a per-instance `/healthz` token before status, stop, or restart actions
- printing the launch URL, PID, config path, and log path
- exposing `config`, `status`, `restart`, `stop`, and richer `tunnel` management commands

On first run, it prompts for:

- host
- port
- base path
- Codex binary path
- data directory
- profile count
- per-profile id, label, Codex home, and optional profile-local data dir
- default profile id
- allowed roots
- CORS origins
- optional explicit backend binary path
- password
- optional hCaptcha site key and secret key

The password is stored as a scrypt hash in the YAML config.

## Generated Local State

The CLI owns two user-local locations:

- config: `~/.codex/codex-webui.yml`
- background runtime state: `~/.codex/codex-webui/`

That runtime state currently includes:

- PID file
- server metadata JSON with a per-instance verification token
- server log
- tunnel PID file
- tunnel log
- tunnel metadata JSON

## Binary Resolution Order

At runtime the CLI searches for a Rust gateway binary in this order:

1. `backendBinaryPath` from `~/.codex/codex-webui.yml`
2. `CODEX_WEBUI_BACKEND_BIN`
3. `dist/backend/<current-target>/backend`
4. `backend/target/release/backend`
5. `backend/target/debug/backend`

The `dist/backend/<target>/` path is the intended npm-distribution location.

## Launch Model

The CLI starts the Rust gateway as a detached background process and injects the resolved configuration through environment variables such as:

- `CODEX_WEBUI_BASE_PATH`
- `CODEX_WEBUI_CODEX_BIN`
- `CODEX_HOME`
- `CODEX_WEBUI_DATA_DIR`
- `CODEX_WEBUI_DEFAULT_PROFILE_ID`
- `CODEX_WEBUI_PROFILES_JSON`
- `CODEX_WEBUI_ALLOWED_ROOTS`
- `CODEX_WEBUI_PASSWORD_HASH`
- `CODEX_WEBUI_HCAPTCHA_SITE_KEY`
- `CODEX_WEBUI_HCAPTCHA_SECRET_KEY`
- `CODEX_WEBUI_SESSION_SECRET`
- `CODEX_WEBUI_INSTANCE_TOKEN`
- `CODEX_WEBUI_CORS_ALLOWED_ORIGINS`

At runtime the public base path is owned by Rust, not baked permanently into the shipped SPA:

- the static frontend is built with a placeholder base path
- Rust serves `build/static`
- Rust rewrites the placeholder in HTML, JS, and CSS responses to the configured `CODEX_WEBUI_BASE_PATH`
- session, Git, and runtime APIs are served directly by the Rust gateway

`CODEX_HOME` provides the default-profile home path at launch time, while `CODEX_WEBUI_PROFILES_JSON` remains the authoritative multi-profile runtime description.

The CLI also accepts transient launch overrides:

- `--hcaptcha-site-key <site-key>`
- `--hcaptcha-secret-key <secret>`
- `--disable-hcaptcha`

Prefer the YAML config or environment variables for the secret key in long-lived deployments, because shell history and process inspection can expose command-line secrets.

The CLI prints the workspace root URL, and the login experience is handled inline by the workspace shell.

The CLI writes config, PID, server metadata, tunnel metadata, and tunnel log state through temp files followed by rename. On stop or restart, it refuses to signal the recorded PID unless the running gateway confirms the stored instance token through `/healthz`; stale files are removed only when the PID is no longer alive.

When system shutdown support is enabled, the actual armed and scheduled state is still runtime data rather than static CLI config:

- arming "shutdown after queue completes" happens through the running app
- the armed flag and any scheduled shutdown timestamp are persisted under `CODEX_WEBUI_DATA_DIR`
- the backend remains authoritative, so the shutdown can still execute without an attached browser session

The Settings page also exposes a per-user automatic startup toggle:

- Windows uses the current user's Startup folder
- macOS uses `~/Library/LaunchAgents/`
- Linux prefers `systemd --user` and falls back to XDG autostart desktop entries when user-systemd is unavailable
- the generated startup entry launches the packaged `codex-webui` CLI, so normal config resolution and background PID handling stay unchanged

## Tunnel Behavior

The tunnel command family is:

```bash
codex-webui tunnel start [--provider auto|cloudflared|ngrok] [--foreground] [--hostname host] [--name tunnel] [--overwrite-dns] [--log-level level] [--arg value]
codex-webui tunnel status [--json]
codex-webui tunnel stop
codex-webui tunnel logs [--lines 80] [--json]
```

Behavior:

1. `start` ensures the server is running first
2. the provider defaults to the configured tunnel provider, or `auto`
3. `auto` prefers `cloudflared` and falls back to `ngrok`
4. background launches persist tunnel PID and metadata under `~/.codex/codex-webui/`
5. `status` reports the provider, PID, origin URL, public URL when discovered, and log path
6. `logs` prints the most recent tunnel log lines without requiring the user to hunt down the log file manually

Provider notes:

- `cloudflared` supports `--hostname`, `--name`, and `--overwrite-dns`
- `ngrok` currently uses the generic `http <origin>` launch path plus any extra args supplied through config or repeated `--arg` flags
- both providers assume the user has already installed and authenticated the relevant CLI where necessary

## Cross Build

Use:

```bash
pnpm build:cross
```

The helper script targets common platforms:

- `x86_64-unknown-linux-gnu`
- `aarch64-unknown-linux-gnu`
- `x86_64-apple-darwin`
- `aarch64-apple-darwin`
- `x86_64-pc-windows-msvc`

Build preference order:

1. `cargo-zigbuild`
2. `cross`
3. plain `cargo`

Built binaries are copied to:

```text
dist/backend/<target>/
```

## Release Notes For Maintainers

Before publishing, check:

1. `pnpm install`
2. `pnpm build`
3. `pnpm verify:static-build`
4. `pnpm verify:api-parity`
5. `pnpm build:cross`
6. `pnpm check`
7. `cargo check --manifest-path backend/Cargo.toml`
8. `pnpm exec playwright test e2e/base-path.spec.ts`
9. `package.json` includes `bin`, `build`, `dist`, and docs
10. `node ./bin/codex-webui.mjs` works from a clean checkout
11. `npx .` or a packed tarball works on a machine that does not rely on local build artifacts by accident

## Recommended Smoke Tests

Before publishing a package, verify at least:

- first-run interactive setup creates `~/.codex/codex-webui.yml`
- `codex-webui` starts the background server and prints a usable URL
- `codex-webui status` verifies the current instance rather than trusting a raw PID file
- login works through the printed base path
- WebSocket connection succeeds after login
- `codex-webui restart` and `codex-webui stop` work
- `codex-webui tunnel` selects the expected tunneling tool
- the packaged binary resolution path works without a local Rust build tree
