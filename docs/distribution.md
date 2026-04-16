# Distribution

## Goal

The intended distribution path is:

```bash
npx codex-webui
```

That means the published package must contain:

- the CLI entrypoint
- the built frontend bundle
- a runnable Rust gateway binary for the user platform

## CLI Contract

`bin/codex-webui.mjs` is responsible for:

- first-run interactive setup
- reading and writing `~/.codex/codex-webui.yml`
- starting and stopping the background server
- printing the launch URL, PID, config path, and log path
- exposing the `config`, `restart`, `stop`, and `tunnel` commands

## Binary Resolution Order

At runtime the CLI looks for a backend binary in this order:

1. `backendBinaryPath` from `~/.codex/codex-webui.yml`
2. `CODEX_WEBUI_BACKEND_BIN`
3. `dist/backend/<current-target>/backend`
4. `backend/target/release/backend`
5. `backend/target/debug/backend`

The `dist/backend/<target>/` path is the one intended for npm distribution.

## Cross Build

Use:

```bash
pnpm build:cross
```

The script builds these targets:

- `x86_64-unknown-linux-gnu`
- `aarch64-unknown-linux-gnu`
- `x86_64-apple-darwin`
- `aarch64-apple-darwin`
- `x86_64-pc-windows-msvc`

It prefers:

1. `cargo-zigbuild`
2. `cross`
3. plain `cargo`

Built binaries are copied to:

```text
dist/backend/<target>/
```

## Publish Checklist

1. Run `pnpm install`
2. Run `pnpm build`
3. Run `pnpm build:cross`
4. Run `pnpm check`
5. Run `cargo check --manifest-path backend/Cargo.toml`
6. Confirm `package.json` `files` includes `bin`, `build`, and `dist`
7. Test `node ./bin/codex-webui.mjs`
8. Test `npx .` or a packed tarball locally before publishing
