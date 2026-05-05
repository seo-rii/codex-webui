<script lang="ts">
  import { onMount } from "svelte";
  import { CornerDownLeft, Paperclip } from "lucide-svelte";

  import { api } from "$lib/api";
  import { localeSignal } from "$lib/i18n";
  import { m } from "$lib/paraglide/messages.js";
  import { getResolvedTheme, subscribeThemeChange } from "$lib/theme";
  import type { TerminalContextPayload, TerminalSummary } from "$lib/types";

  let {
    terminalId,
    selectedSessionId = null,
    readOnly = false,
    onAttachContext = null
  }: {
    terminalId: string;
    selectedSessionId?: string | null;
    readOnly?: boolean;
    onAttachContext?: ((payload: TerminalContextPayload) => void | Promise<void>) | null;
  } = $props();

  let container = $state<HTMLDivElement | null>(null);
  let errorText = $state("");
  let summary = $state<TerminalSummary | null>(null);
  let loading = $state(true);
  let attachingContext = $state(false);
  let ctrlModifierArmed = $state(false);
  let mobileInput = $state("");
  let mobileInputElement = $state<HTMLInputElement | null>(null);
  let terminalInputSender: ((data: string) => void) | null = null;
  let focusTerminalViewport: (() => void) | null = null;
  const mobileTerminalKeyRows = [
    [
      { label: "Esc", value: "\u001b", ariaLabel: "Escape" },
      { label: "Tab", value: "\t", ariaLabel: "Tab" },
      { label: "Ctrl", value: "__ctrl__", ariaLabel: "Control" },
      { label: "Bksp", value: "\u007f", ariaLabel: "Backspace" }
    ],
    [
      { label: "↑", value: "\u001b[A", ariaLabel: "Arrow up" },
      { label: "↓", value: "\u001b[B", ariaLabel: "Arrow down" },
      { label: "←", value: "\u001b[D", ariaLabel: "Arrow left" },
      { label: "→", value: "\u001b[C", ariaLabel: "Arrow right" },
      { label: "Enter", value: "\r", ariaLabel: "Enter", wide: true }
    ]
  ] as const;
  const ui = $derived.by(() => {
    const _locale = $localeSignal;

    return {
      terminal: m.terminal(),
      failedSendInput: m.failed_to_send_terminal_input(),
      failedInitialize: m.failed_to_initialize_terminal(),
      connecting: m.connecting_to_terminal(),
      loading: m.status_loading(),
      hostShellWarning: m.terminal_host_shell_warning(),
      attachContext: m.attach_terminal_context(),
      attachRequiresThread: m.terminal_context_requires_thread(),
      send: m.send()
    };
  });

  async function attachTerminalContext() {
    if (!selectedSessionId || readOnly || attachingContext) {
      return;
    }

    attachingContext = true;
    errorText = "";
    try {
      const payload = await api.attachTerminalContext(selectedSessionId, terminalId);
      await onAttachContext?.(payload);
    } catch (error) {
      errorText = error instanceof Error ? error.message : ui.failedInitialize;
    } finally {
      attachingContext = false;
    }
  }

  function applyCtrlModifier(value: string) {
    if (value.length !== 1) {
      return value;
    }

    const codePoint = value.charCodeAt(0);
    if ((codePoint >= 65 && codePoint <= 90) || (codePoint >= 97 && codePoint <= 122)) {
      return String.fromCharCode((codePoint & 31) || codePoint);
    }

    if (value === "@") {
      return "\u0000";
    }
    if (value === "[") {
      return "\u001b";
    }
    if (value === "\\") {
      return "\u001c";
    }
    if (value === "]") {
      return "\u001d";
    }
    if (value === "^") {
      return "\u001e";
    }
    if (value === "_") {
      return "\u001f";
    }
    if (value === "?") {
      return "\u007f";
    }

    return value;
  }

  function sendTerminalInput(value: string, useCtrlModifier = true) {
    const nextValue = useCtrlModifier && ctrlModifierArmed ? applyCtrlModifier(value) : value;
    ctrlModifierArmed = false;
    terminalInputSender?.(nextValue);
  }

  function removeLastTextCluster(value: string) {
    const clusters = Array.from(value);
    clusters.pop();
    return clusters.join("");
  }

  function flushMobileInputBuffer() {
    if (!mobileInput) {
      return false;
    }
    sendTerminalInput(mobileInput, false);
    mobileInput = "";
    return true;
  }

  function submitMobileInput() {
    flushMobileInputBuffer();
    sendTerminalInput("\r", false);
    mobileInputElement?.focus();
  }

  function focusBestTerminalInput() {
    if (mobileInputElement && window.matchMedia("(max-width: 720px)").matches) {
      mobileInputElement.focus();
      return;
    }
    focusTerminalViewport?.();
  }

  function handleMobileTerminalKey(value: string) {
    if (value === "__ctrl__") {
      ctrlModifierArmed = !ctrlModifierArmed;
      focusBestTerminalInput();
      return;
    }

    if (value === "\u007f" && mobileInput) {
      mobileInput = removeLastTextCluster(mobileInput);
      focusBestTerminalInput();
      return;
    }

    if (value === "\r") {
      submitMobileInput();
      return;
    }

    if (value === "\t" || value.startsWith("\u001b[")) {
      flushMobileInputBuffer();
    }

    sendTerminalInput(value, false);
    focusBestTerminalInput();
  }

  function handleMobileInputKeydown(event: KeyboardEvent) {
    if (event.key === "Enter") {
      event.preventDefault();
      submitMobileInput();
      return;
    }

    if (event.key === "Tab") {
      event.preventDefault();
      flushMobileInputBuffer();
      sendTerminalInput("\t", false);
      return;
    }

    if (ctrlModifierArmed && event.key.length === 1) {
      event.preventDefault();
      sendTerminalInput(event.key, true);
    }
  }

  function handleMobileInputBeforeInput(event: InputEvent) {
    if (event.inputType === "insertLineBreak") {
      event.preventDefault();
      submitMobileInput();
      return;
    }

    if (!ctrlModifierArmed || !event.data) {
      return;
    }

    event.preventDefault();
    sendTerminalInput(Array.from(event.data)[0] ?? event.data, true);
  }

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
    let inputObserver: MutationObserver | null = null;
    let pendingOutput = "";
    let flushFrame: number | null = null;
    let mobileInputModeQuery: MediaQueryList | null = null;
    let releaseMobileInputMode: (() => void) | null = null;
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
    const syncTerminalInputAttributes = () => {
      const helperTextarea = container?.querySelector(".xterm-helper-textarea");
      if (!(helperTextarea instanceof HTMLTextAreaElement)) {
        return;
      }

      helperTextarea.autocapitalize = "none";
      helperTextarea.autocomplete = "off";
      helperTextarea.spellcheck = false;
      helperTextarea.setAttribute("autocorrect", "off");
      helperTextarea.setAttribute("aria-autocomplete", "none");
      helperTextarea.setAttribute("data-gramm", "false");
      helperTextarea.setAttribute("data-gramm_editor", "false");
      helperTextarea.setAttribute("data-enable-grammarly", "false");
      helperTextarea.setAttribute("enterkeyhint", "enter");
    };

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
        mobileInputModeQuery = window.matchMedia("(max-width: 720px)");
        const mobileControlsPreferred = mobileInputModeQuery.matches;
        xterm = new Terminal({
          cursorBlink: true,
          disableStdin: mobileControlsPreferred,
          fontFamily: '"IBM Plex Mono", "SFMono-Regular", monospace',
          fontSize: 13,
          lineHeight: 1.35,
          theme: getTerminalTheme()
        });
        xterm.loadAddon(fitAddon);
        xterm.open(container);
        syncTerminalInputAttributes();
        fitAddon.fit();
        if (!mobileControlsPreferred) {
          xterm.focus();
        }
        focusTerminalViewport = () => {
          syncTerminalInputAttributes();
          if (mobileInputElement && window.matchMedia("(max-width: 720px)").matches) {
            mobileInputElement.focus();
            return;
          }
          xterm?.focus();
        };
        const syncMobileInputMode = () => {
          if (!xterm || !mobileInputModeQuery) {
            return;
          }
          xterm.options.disableStdin = mobileInputModeQuery.matches;
          if (mobileInputModeQuery.matches) {
            mobileInputElement?.focus();
          }
        };
        mobileInputModeQuery.addEventListener("change", syncMobileInputMode);
        releaseMobileInputMode = () => {
          mobileInputModeQuery?.removeEventListener("change", syncMobileInputMode);
        };
        terminalInputSender = (data: string) => {
          void api.sendTerminalInput(terminalId, data).catch((error) => {
            errorText = error instanceof Error ? error.message : ui.failedSendInput;
          });
        };

        const dataListener = xterm.onData((data) => {
          sendTerminalInput(data);
        });

        resizeObserver = new ResizeObserver(() => {
          fitAddon.fit();
        });
        resizeObserver.observe(container);
        inputObserver = new MutationObserver(() => {
          syncTerminalInputAttributes();
        });
        inputObserver.observe(container, { childList: true, subtree: true });

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
      inputObserver?.disconnect();
      releaseMobileInputMode?.();
      releaseTerminal?.();
      terminalInputSender = null;
      focusTerminalViewport = null;
      ctrlModifierArmed = false;
      mobileInput = "";
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
    <div class="terminal-shell__header-actions">
      <button
        class="terminal-shell__attach-button"
        disabled={readOnly || !selectedSessionId || loading || attachingContext}
        onclick={() => void attachTerminalContext()}
        title={!selectedSessionId ? ui.attachRequiresThread : ui.attachContext}
        type="button"
      >
        <Paperclip size={14} />
        <span>{attachingContext ? ui.loading : ui.attachContext}</span>
      </button>
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
  </div>

  {#if errorText}
    <div class="error-banner small">{errorText}</div>
  {/if}

  <div class="terminal-shell__safety-note" role="note">
    {ui.hostShellWarning}
  </div>

  <div class="terminal-shell__body">
    {#if loading}
      <div class="placeholder-card">{ui.connecting}</div>
    {/if}
    <div bind:this={container} class:hidden={loading} class="terminal-shell__viewport"></div>
  </div>

  {#if !loading}
    <div class="terminal-shell__mobile-input-bar">
      <input
        bind:this={mobileInputElement}
        bind:value={mobileInput}
        aria-label={ui.terminal}
        autocapitalize="none"
        autocomplete="off"
        autocorrect="off"
        class="terminal-shell__mobile-input"
        enterkeyhint="enter"
        inputmode="text"
        onbeforeinput={handleMobileInputBeforeInput}
        onkeydown={handleMobileInputKeydown}
        placeholder="$"
        spellcheck="false"
        type="text"
      />
      <button class="terminal-shell__mobile-submit" disabled={!mobileInput} onclick={submitMobileInput} type="button">
        <CornerDownLeft size={15} />
        <span>{ui.send}</span>
      </button>
    </div>
    <div class="terminal-shell__mobile-keys" aria-label="Terminal mobile shortcuts">
      {#each mobileTerminalKeyRows as row, rowIndex (`row-${rowIndex}`)}
        <div class="terminal-shell__mobile-key-row">
          {#each row as key (`${key.label}-${key.value}`)}
            <button
              aria-label={key.ariaLabel}
              class={`terminal-shell__mobile-key ${key.value === "__ctrl__" && ctrlModifierArmed ? "terminal-shell__mobile-key--active" : ""} ${"wide" in key && key.wide ? "terminal-shell__mobile-key--wide" : ""}`}
              onclick={() => handleMobileTerminalKey(key.value)}
              onpointerdown={(event) => event.preventDefault()}
              type="button"
            >
              {key.label}
            </button>
          {/each}
        </div>
      {/each}
    </div>
  {/if}
</section>

<style>
  .terminal-shell {
    display: grid;
    grid-template-rows: auto auto minmax(0, 1fr) auto;
    gap: 0.75rem;
    min-height: 0;
    overflow: hidden;
    padding: 1rem;
    background: var(--panel-strong);
  }

  .terminal-shell__header,
  .terminal-shell__header-actions,
  .terminal-shell__meta {
    display: flex;
    gap: 0.75rem;
    align-items: center;
    justify-content: space-between;
  }

  .terminal-shell__header {
    align-items: flex-start;
  }

  .terminal-shell__header-actions {
    flex-wrap: wrap;
    justify-content: flex-end;
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

  .terminal-shell__attach-button {
    display: inline-flex;
    align-items: center;
    gap: 0.45rem;
    border: 1px solid var(--line);
    border-radius: 999px;
    background: color-mix(in srgb, var(--panel-soft) 82%, transparent);
    color: var(--ink);
    padding: 0.55rem 0.95rem;
    font: 700 0.82rem/1 var(--font-ui);
    transition:
      background-color 140ms ease,
      border-color 140ms ease,
      color 140ms ease,
      transform 140ms ease,
      opacity 140ms ease;
  }

  .terminal-shell__attach-button:disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }

  .terminal-shell__attach-button:not(:disabled):hover {
    border-color: color-mix(in srgb, var(--accent, #d85e2a) 44%, var(--line));
    background: color-mix(in srgb, var(--accent, #d85e2a) 10%, var(--panel-strong));
    color: var(--ink-strong);
  }

  .terminal-shell__attach-button:not(:disabled):active {
    transform: scale(0.99);
  }

  .terminal-shell__safety-note {
    border: 1px solid color-mix(in srgb, var(--accent, #d85e2a) 24%, var(--line));
    border-radius: 0.9rem;
    background: color-mix(in srgb, var(--accent, #d85e2a) 7%, var(--panel-soft));
    color: color-mix(in srgb, var(--ink) 86%, var(--accent, #d85e2a));
    padding: 0.48rem 0.65rem;
    font: 650 0.73rem/1.35 var(--font-ui);
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

  .terminal-shell__mobile-input-bar,
  .terminal-shell__mobile-keys {
    display: none;
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

    .terminal-shell__header-actions,
    .terminal-shell__meta {
      justify-content: flex-start;
    }

    .terminal-shell__mobile-keys {
      display: grid;
      gap: 0.45rem;
      padding-bottom: max(0.2rem, env(safe-area-inset-bottom));
    }

    .terminal-shell__mobile-input-bar {
      display: grid;
      grid-template-columns: minmax(0, 1fr) auto;
      gap: 0.5rem;
      align-items: center;
    }

    .terminal-shell__mobile-input {
      min-width: 0;
      border: 1px solid var(--line);
      border-radius: 0.95rem;
      outline: none;
      background: color-mix(in srgb, var(--panel-soft) 90%, transparent);
      color: var(--ink-strong);
      padding: 0.7rem 0.85rem;
      font: 700 16px/1.15 var(--font-ui);
      transition:
        border-color 140ms ease,
        box-shadow 140ms ease,
        background-color 140ms ease;
    }

    .terminal-shell__mobile-input::placeholder {
      color: color-mix(in srgb, var(--muted) 62%, transparent);
    }

    .terminal-shell__mobile-input:focus {
      border-color: color-mix(in srgb, var(--accent, #d85e2a) 54%, var(--line));
      background: color-mix(in srgb, var(--panel-strong) 90%, transparent);
      box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent, #d85e2a) 14%, transparent);
    }

    .terminal-shell__mobile-submit {
      display: inline-flex;
      align-items: center;
      justify-content: center;
      gap: 0.35rem;
      border: 1px solid color-mix(in srgb, var(--accent, #d85e2a) 30%, var(--line));
      border-radius: 0.95rem;
      background: color-mix(in srgb, var(--accent, #d85e2a) 12%, var(--panel-soft));
      color: var(--ink-strong);
      padding: 0.72rem 0.85rem;
      font: 800 0.78rem/1 var(--font-ui);
      touch-action: manipulation;
      transition:
        background-color 140ms ease,
        border-color 140ms ease,
        opacity 140ms ease,
        transform 140ms ease;
    }

    .terminal-shell__mobile-submit:not(:disabled):active {
      transform: scale(0.98);
    }

    .terminal-shell__mobile-submit:disabled {
      opacity: 0.45;
      cursor: not-allowed;
    }

    .terminal-shell__mobile-key-row {
      display: grid;
      gap: 0.45rem;
      grid-template-columns: repeat(4, minmax(0, 1fr));
    }

    .terminal-shell__mobile-key {
      min-width: 0;
      border: 1px solid var(--line);
      border-radius: 0.9rem;
      background: color-mix(in srgb, var(--panel-soft) 86%, transparent);
      color: var(--ink);
      padding: 0.65rem 0.35rem;
      font: 700 0.82rem/1 var(--font-ui);
      touch-action: manipulation;
      transition:
        background-color 140ms ease,
        border-color 140ms ease,
        transform 140ms ease,
        color 140ms ease;
    }

    .terminal-shell__mobile-key:active {
      transform: scale(0.98);
    }

    .terminal-shell__mobile-key--active {
      border-color: color-mix(in srgb, var(--accent, #d85e2a) 48%, var(--line));
      background: color-mix(in srgb, var(--accent, #d85e2a) 18%, var(--panel-strong));
      color: var(--ink-strong);
    }

    .terminal-shell__mobile-key--wide {
      grid-column: span 2;
    }
  }
</style>
