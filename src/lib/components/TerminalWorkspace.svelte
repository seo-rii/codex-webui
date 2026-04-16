<script lang="ts">
  import { onMount } from "svelte";

  import { api } from "$lib/api";
  import { localeSignal } from "$lib/i18n";
  import { m } from "$lib/paraglide/messages.js";
  import { getResolvedTheme, subscribeThemeChange } from "$lib/theme";
  import type { TerminalSummary } from "$lib/types";

  let { terminalId }: { terminalId: string } = $props();

  let container = $state<HTMLDivElement | null>(null);
  let errorText = $state("");
  let summary = $state<TerminalSummary | null>(null);
  let loading = $state(true);
  const ui = $derived.by(() => {
    const _locale = $localeSignal;

    return {
      terminal: m.terminal(),
      failedSendInput: m.failed_to_send_terminal_input(),
      failedInitialize: m.failed_to_initialize_terminal(),
      connecting: m.connecting_to_terminal(),
      loading: m.status_loading()
    };
  });

  function getTerminalTheme() {
    if (getResolvedTheme() === "dark") {
      return {
        background: "#0f172a",
        foreground: "#e2e8f0",
        cursor: "#f59e0b",
        selectionBackground: "rgba(245, 158, 11, 0.24)"
      };
    }

    return {
      background: "#f6f1eb",
      foreground: "#2d2016",
      cursor: "#d85e2a",
      selectionBackground: "rgba(216, 94, 42, 0.18)"
    };
  }

  onMount(() => {
    let disposed = false;
    let releaseTerminal: (() => void) | null = null;
    let resizeObserver: ResizeObserver | null = null;
    let pendingOutput = "";
    let flushFrame: number | null = null;
    let xterm:
      | (import("@xterm/xterm").Terminal & {
          dispose: () => void;
          write: (data: string) => void;
          clear: () => void;
          focus: () => void;
          loadAddon: (addon: import("@xterm/addon-fit").FitAddon) => void;
          onData: (listener: (data: string) => void) => { dispose: () => void };
        })
      | null = null;
    const releaseThemeChange = subscribeThemeChange(() => {
      if (!xterm) {
        return;
      }
      xterm.options.theme = getTerminalTheme();
    });

    void (async () => {
      if (!container) {
        return;
      }

      try {
        const [{ Terminal }, { FitAddon }] = await Promise.all([
          import("@xterm/xterm"),
          import("@xterm/addon-fit"),
          import("@xterm/xterm/css/xterm.css")
        ]);
        if (disposed || !container) {
          return;
        }

        const fitAddon = new FitAddon();
        xterm = new Terminal({
          cursorBlink: true,
          fontFamily: '"IBM Plex Mono", "SFMono-Regular", monospace',
          fontSize: 13,
          lineHeight: 1.35,
          theme: getTerminalTheme()
        });
        xterm.loadAddon(fitAddon);
        xterm.open(container);
        fitAddon.fit();
        xterm.focus();

        const dataListener = xterm.onData((data) => {
          void api.sendTerminalInput(terminalId, data).catch((error) => {
            errorText = error instanceof Error ? error.message : ui.failedSendInput;
          });
        });

        resizeObserver = new ResizeObserver(() => {
          fitAddon.fit();
        });
        resizeObserver.observe(container);

        const snapshot = await api.readTerminal(terminalId);
        if (disposed || !xterm) {
          dataListener.dispose();
          return;
        }

        summary = snapshot.terminal;
        xterm.clear();
        if (snapshot.snapshot) {
          xterm.write(snapshot.snapshot);
        }
        loading = false;

        releaseTerminal = api.subscribeTerminal(terminalId, (event) => {
          if (!xterm) {
            return;
          }

          if (event.method === "terminal/output") {
            pendingOutput += String(event.params.text ?? "");
            if (flushFrame === null) {
              flushFrame = requestAnimationFrame(() => {
                flushFrame = null;
                if (!xterm || !pendingOutput) {
                  pendingOutput = "";
                  return;
                }
                const nextChunk = pendingOutput;
                pendingOutput = "";
                xterm.write(nextChunk);
              });
            }
            return;
          }

          if (event.method === "terminal/exit") {
            summary = summary
              ? {
                  ...summary,
                  status: "exited",
                  exitCode: typeof event.params.exitCode === "number" ? event.params.exitCode : null
                }
              : summary;
          }
        });

        return () => {
          dataListener.dispose();
        };
      } catch (error) {
        loading = false;
        errorText = error instanceof Error ? error.message : ui.failedInitialize;
      }
    })();

    return () => {
      disposed = true;
      if (flushFrame !== null) {
        cancelAnimationFrame(flushFrame);
      }
      resizeObserver?.disconnect();
      releaseTerminal?.();
      xterm?.dispose();
      releaseThemeChange();
    };
  });
</script>

<section class="terminal-shell surface">
  <div class="terminal-shell__header">
    <div>
      <p class="eyebrow">{ui.terminal}</p>
      <h2>{summary?.title ?? ui.terminal}</h2>
    </div>
    <div class="terminal-shell__meta">
      <span class="meta-pill">{summary?.status ?? ui.loading}</span>
      {#if summary?.exitCode !== null && summary?.exitCode !== undefined}
        <span class="meta-pill subtle">{m.exit_code({ code: String(summary.exitCode) })}</span>
      {/if}
      {#if summary?.cwd}
        <span class="meta-pill subtle">{summary.cwd}</span>
      {/if}
    </div>
  </div>

  {#if errorText}
    <div class="error-banner small">{errorText}</div>
  {/if}

  <div class="terminal-shell__body">
    {#if loading}
      <div class="placeholder-card">{ui.connecting}</div>
    {/if}
    <div bind:this={container} class:hidden={loading} class="terminal-shell__viewport"></div>
  </div>
</section>

<style>
  .terminal-shell {
    display: grid;
    gap: 1rem;
    min-height: 0;
    overflow: hidden;
    padding: 1rem;
    background: var(--panel-strong);
  }

  .terminal-shell__header,
  .terminal-shell__meta {
    display: flex;
    gap: 0.75rem;
    align-items: center;
    justify-content: space-between;
  }

  .terminal-shell__header {
    align-items: flex-start;
  }

  .terminal-shell__header h2 {
    margin: 0.15rem 0 0;
    color: var(--ink-strong);
    font: 600 1.2rem/1.1 var(--font-display);
  }

  .terminal-shell__meta {
    flex-wrap: wrap;
    justify-content: flex-end;
  }

  .terminal-shell__body {
    min-height: 0;
    overflow: hidden;
    border: 1px solid var(--line);
    border-radius: 1.1rem;
    background: var(--panel-soft);
    padding: 0.2rem;
  }

  .terminal-shell__viewport {
    height: 100%;
    min-height: 0;
  }

  .terminal-shell__viewport :global(.xterm),
  .terminal-shell__viewport :global(.xterm-viewport),
  .terminal-shell__viewport :global(.xterm-screen) {
    height: 100%;
  }

  .hidden {
    display: none;
  }

  .meta-pill {
    border-radius: 999px;
    background: color-mix(in srgb, var(--panel-strong) 88%, transparent);
    color: var(--ink);
    padding: 0.45rem 0.8rem;
    font-size: 0.8rem;
  }

  .meta-pill.subtle {
    color: var(--muted);
  }

  @media (max-width: 720px) {
    .terminal-shell__header {
      flex-direction: column;
    }

    .terminal-shell__meta {
      justify-content: flex-start;
    }
  }
</style>
