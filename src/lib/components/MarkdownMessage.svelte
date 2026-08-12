<script lang="ts">
  import { createEventDispatcher } from "svelte";
  import { renderMarkdown } from "$lib/markdown-renderer";
  import { m } from "$lib/paraglide/messages.js";

  let {
    text = "",
    compact = false,
    maxInitialChars = null,
    expandLabel = "Show full message"
  }: {
    text?: string | null;
    compact?: boolean;
    maxInitialChars?: number | null;
    expandLabel?: string;
  } = $props();

  const dispatch = createEventDispatcher<{
    openLocalPath: {
      href: string;
    };
  }>();
  let rootElement = $state<HTMLDivElement | null>(null);
  let expanded = $state(false);

  const codeCopyResetTimers = new WeakMap<HTMLButtonElement, number>();
  const normalizedText = $derived(text ?? "");
  const shouldTruncate = $derived(
    typeof maxInitialChars === "number" &&
      maxInitialChars > 0 &&
      normalizedText.length > maxInitialChars &&
      !expanded
  );
  const visibleText = $derived(
    shouldTruncate ? `${normalizedText.slice(0, maxInitialChars ?? 0).trimEnd()}\n\n...` : normalizedText
  );

  const html = $derived.by(() => {
    return renderMarkdown(visibleText, m.copy_code());
  });

  $effect(() => {
    if (!rootElement) {
      return;
    }

    const element = rootElement;
    element.addEventListener("click", handleClick);
    return () => {
      element.removeEventListener("click", handleClick);
    };
  });

  async function handleClick(event: Event) {
    const copyButton = (event.target as HTMLElement | null)?.closest("button[data-copy-code]");
    if (copyButton instanceof HTMLButtonElement) {
      event.preventDefault();
      const code = copyButton.closest(".code-block")?.querySelector("pre code")?.textContent ?? "";
      if (!code) {
        return;
      }

      try {
        let copied = false;
        if (navigator.clipboard?.writeText) {
          try {
            await navigator.clipboard.writeText(code);
            copied = true;
          } catch {
            copied = false;
          }
        }

        if (!copied) {
          const textarea = document.createElement("textarea");
          textarea.value = code;
          textarea.setAttribute("readonly", "");
          textarea.style.position = "fixed";
          textarea.style.opacity = "0";
          textarea.style.pointerEvents = "none";
          document.body.appendChild(textarea);
          textarea.select();
          copied = document.execCommand("copy");
          textarea.remove();
        }

        if (!copied) {
          throw new Error("Clipboard copy failed");
        }

        const label = copyButton.querySelector<HTMLElement>("[data-copy-code-label]");
        const copyLabel = copyButton.dataset.copyLabel ?? m.copy_code();
        const copiedLabel = m.copied_to_clipboard();
        const previousTimer = codeCopyResetTimers.get(copyButton);
        if (previousTimer) {
          clearTimeout(previousTimer);
        }

        copyButton.dataset.copied = "true";
        copyButton.title = copiedLabel;
        copyButton.setAttribute("aria-label", copiedLabel);
        if (label) {
          label.textContent = copiedLabel;
        }

        const timer = window.setTimeout(() => {
          copyButton.dataset.copied = "false";
          copyButton.title = copyLabel;
          copyButton.setAttribute("aria-label", copyLabel);
          if (label) {
            label.textContent = copyLabel;
          }
          codeCopyResetTimers.delete(copyButton);
        }, 1400);
        codeCopyResetTimers.set(copyButton, timer);
      } catch {
        copyButton.dataset.copied = "false";
      }
      return;
    }

    const anchor = (event.target as HTMLElement | null)?.closest("a");
    if (!anchor) {
      return;
    }

    const href = anchor.getAttribute("href")?.trim() ?? "";
    if (!href.startsWith("/")) {
      return;
    }

    event.preventDefault();
    dispatch("openLocalPath", { href });
  }
</script>

<div bind:this={rootElement} class={`markdown-body ${compact ? "markdown-body--compact" : ""}`}>
  {@html html}
  {#if shouldTruncate}
    <button class="markdown-body__expand" onclick={() => { expanded = true; }} type="button">
      {expandLabel}
    </button>
  {/if}
</div>

<style>
  .markdown-body {
    color: var(--markdown-body-fg, rgb(31 41 55));
    font-size: 15px;
    line-height: 1.7;
  }

  .markdown-body :global(p) {
    margin: 0 0 1rem;
  }

  .markdown-body :global(p:last-child) {
    margin-bottom: 0;
  }

  .markdown-body__expand {
    margin-top: 0.75rem;
    display: inline-flex;
    align-items: center;
    border-radius: 999px;
    border: 1px solid rgba(245, 158, 11, 0.28);
    background: rgba(255, 251, 235, 0.84);
    padding: 0.35rem 0.7rem;
    color: rgb(180 83 9);
    font-size: 0.72rem;
    font-weight: 700;
    transition:
      background-color 160ms ease,
      border-color 160ms ease,
      color 160ms ease;
  }

  .markdown-body__expand:hover {
    border-color: rgba(245, 158, 11, 0.42);
    background: rgba(254, 243, 199, 0.96);
    color: rgb(146 64 14);
  }

  .markdown-body :global(h1),
  .markdown-body :global(h2),
  .markdown-body :global(h3),
  .markdown-body :global(h4) {
    margin: 1.5rem 0 0.75rem;
    color: var(--markdown-heading-fg, rgb(17 24 39));
    font-weight: 700;
  }

  .markdown-body :global(h1:first-child),
  .markdown-body :global(h2:first-child),
  .markdown-body :global(h3:first-child),
  .markdown-body :global(h4:first-child) {
    margin-top: 0;
  }

  .markdown-body :global(h1) {
    font-size: 1.5rem;
    line-height: 1.2;
  }

  .markdown-body :global(h2) {
    font-size: 1.25rem;
    line-height: 1.25;
  }

  .markdown-body :global(h3) {
    font-size: 1.125rem;
    line-height: 1.3;
  }

  .markdown-body :global(ul),
  .markdown-body :global(ol) {
    margin: 0 0 1rem;
    padding-left: 1.5rem;
  }

  .markdown-body :global(ul) {
    list-style: disc;
  }

  .markdown-body :global(ol) {
    list-style: decimal;
  }

  .markdown-body :global(li + li) {
    margin-top: 0.375rem;
  }

  .markdown-body :global(blockquote) {
    margin: 0 0 1rem;
    border-left: 4px solid var(--markdown-blockquote-border, rgb(253 230 138));
    padding-left: 1rem;
    color: var(--markdown-blockquote-fg, rgb(75 85 99));
    font-style: italic;
  }

  .markdown-body :global(code:not(.hljs)) {
    border: 1px solid var(--markdown-inline-code-border, rgb(229 231 235 / 0.5));
    border-radius: 0.375rem;
    background: var(--markdown-inline-code-bg, rgb(243 244 246));
    color: var(--markdown-inline-code-fg, rgb(17 24 39));
    padding: 0.125rem 0.375rem;
    font-family: "IBM Plex Mono", "SFMono-Regular", monospace;
    font-size: 0.9em;
  }

  .markdown-body :global(table) {
    width: 100%;
    margin: 0 0 1rem;
    border: 1px solid var(--markdown-table-border, rgb(229 231 235));
    border-collapse: collapse;
    overflow: hidden;
    border-radius: 0.75rem;
  }

  .markdown-body :global(th),
  .markdown-body :global(td) {
    border: 1px solid var(--markdown-table-border, rgb(229 231 235));
    padding: 0.75rem;
    text-align: left;
  }

  .markdown-body :global(th) {
    background: var(--markdown-table-head-bg, rgb(249 250 251));
    color: var(--markdown-table-head-fg, rgb(55 65 81));
    font-size: 0.75rem;
    font-weight: 700;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  .markdown-body :global(td) {
    color: var(--markdown-body-fg, rgb(31 41 55));
  }

  .markdown-body :global(.code-block) {
    border-color: var(--markdown-code-block-border, rgb(229 231 235)) !important;
    background: var(--markdown-code-block-bg, rgb(249 250 251 / 0.5)) !important;
    overflow: visible !important;
  }

  .markdown-body :global(.code-block > div:first-child) {
    position: sticky;
    top: var(--markdown-code-header-sticky-top, 0.45rem);
    z-index: 14;
    border-top-left-radius: inherit;
    border-top-right-radius: inherit;
    border-bottom-color: var(--markdown-code-block-border, rgb(229 231 235)) !important;
    background: var(--markdown-code-block-header-bg, rgb(243 244 246 / 0.6)) !important;
    backdrop-filter: blur(12px);
    color: var(--markdown-code-block-header-fg, rgb(107 114 128)) !important;
  }

  .markdown-body :global(.code-block pre) {
    border-bottom-left-radius: inherit;
    border-bottom-right-radius: inherit;
  }

  .markdown-body :global(.code-copy-button) {
    display: inline-flex;
    max-width: min(9rem, 44vw);
    align-items: center;
    gap: 0.35rem;
    border: 1px solid var(--markdown-code-copy-border, rgb(209 213 219 / 0.78));
    border-radius: 999px;
    background: var(--markdown-code-copy-bg, rgb(255 255 255 / 0.72));
    color: var(--markdown-code-copy-fg, rgb(75 85 99));
    padding: 0.22rem 0.5rem;
    font-size: 0.62rem;
    font-weight: 750;
    letter-spacing: 0.06em;
    line-height: 1;
    text-transform: uppercase;
    transition:
      background-color 150ms ease,
      border-color 150ms ease,
      color 150ms ease,
      transform 150ms ease;
  }

  .markdown-body :global(.code-copy-button:hover) {
    border-color: var(--markdown-code-copy-hover-border, rgb(245 158 11 / 0.45));
    background: var(--markdown-code-copy-hover-bg, rgb(255 251 235 / 0.95));
    color: var(--markdown-code-copy-hover-fg, rgb(180 83 9));
    transform: translateY(-1px);
  }

  .markdown-body :global(.code-copy-button[data-copied="true"]) {
    border-color: var(--markdown-code-copy-copied-border, rgb(16 185 129 / 0.38));
    background: var(--markdown-code-copy-copied-bg, rgb(236 253 245 / 0.94));
    color: var(--markdown-code-copy-copied-fg, rgb(4 120 87));
  }

  .markdown-body :global(.code-copy-button__icon) {
    height: 0.74rem;
    width: 0.74rem;
    flex-shrink: 0;
  }

  .markdown-body :global(.code-copy-button span) {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .markdown-body :global(.code-block pre) {
    color: var(--markdown-code-block-fg, rgb(31 41 55));
  }

  .markdown-body :global(.hljs-comment) {
    color: rgb(156 163 175);
    font-style: italic;
  }

  .markdown-body :global(.hljs-keyword),
  .markdown-body :global(.hljs-selector-tag) {
    color: rgb(217 119 6);
    font-weight: 500;
  }

  .markdown-body--compact {
    font-size: 0.9375rem;
    line-height: 1.55;
  }

  .markdown-body--compact :global(p) {
    margin: 0 0 0.5rem;
  }

  .markdown-body--compact :global(ul),
  .markdown-body--compact :global(ol),
  .markdown-body--compact :global(blockquote),
  .markdown-body--compact :global(table) {
    margin-bottom: 0.75rem;
  }

  .markdown-body--compact :global(h1),
  .markdown-body--compact :global(h2),
  .markdown-body--compact :global(h3),
  .markdown-body--compact :global(h4) {
    margin: 1rem 0 0.5rem;
  }

  .markdown-body :global(.hljs-string),
  .markdown-body :global(.hljs-title) {
    color: rgb(5 150 105);
  }

  .markdown-body :global(.hljs-number),
  .markdown-body :global(.hljs-attr) {
    color: rgb(249 115 22);
  }

  .markdown-body :global(.hljs-function) {
    color: rgb(37 99 235);
  }

  :global(:root[data-theme="dark"]) .markdown-body {
    --markdown-body-fg: rgb(226 232 240);
    --markdown-heading-fg: rgb(248 250 252);
    --markdown-blockquote-border: rgb(217 119 6 / 0.65);
    --markdown-blockquote-fg: rgb(148 163 184);
    --markdown-inline-code-border: rgb(71 85 105 / 0.65);
    --markdown-inline-code-bg: rgb(15 23 42 / 0.92);
    --markdown-inline-code-fg: rgb(248 250 252);
    --markdown-table-border: rgb(71 85 105 / 0.65);
    --markdown-table-head-bg: rgb(30 41 59 / 0.95);
    --markdown-table-head-fg: rgb(203 213 225);
    --markdown-code-block-border: rgb(71 85 105 / 0.65);
    --markdown-code-block-bg: rgb(15 23 42 / 0.78);
    --markdown-code-block-header-bg: rgb(30 41 59 / 0.92);
    --markdown-code-block-header-fg: rgb(148 163 184);
    --markdown-code-block-fg: rgb(226 232 240);
    --markdown-code-copy-border: rgb(71 85 105 / 0.7);
    --markdown-code-copy-bg: rgb(15 23 42 / 0.72);
    --markdown-code-copy-fg: rgb(203 213 225);
    --markdown-code-copy-hover-border: rgb(245 158 11 / 0.45);
    --markdown-code-copy-hover-bg: rgb(69 39 10 / 0.58);
    --markdown-code-copy-hover-fg: rgb(252 211 77);
    --markdown-code-copy-copied-border: rgb(52 211 153 / 0.38);
    --markdown-code-copy-copied-bg: rgb(6 78 59 / 0.46);
    --markdown-code-copy-copied-fg: rgb(167 243 208);
  }

  :global(:root[data-theme="dark"]) .markdown-body__expand {
    border-color: rgb(245 158 11 / 0.34);
    background: rgb(69 39 10 / 0.42);
    color: rgb(252 211 77);
  }

  :global(:root[data-theme="dark"]) .markdown-body__expand:hover {
    border-color: rgb(245 158 11 / 0.48);
    background: rgb(92 53 15 / 0.56);
    color: rgb(254 240 138);
  }

  :global(:root[data-theme="dark"]) .markdown-body :global(a) {
    color: rgb(251 191 36) !important;
  }

  :global(:root[data-theme="dark"]) .markdown-body :global(a:hover) {
    color: rgb(252 211 77) !important;
  }

  :global(:root[data-theme="dark"]) .markdown-body :global(.hljs-comment) {
    color: rgb(100 116 139);
  }

  :global(:root[data-theme="dark"]) .markdown-body :global(.hljs-keyword),
  :global(:root[data-theme="dark"]) .markdown-body :global(.hljs-selector-tag) {
    color: rgb(251 191 36);
  }

  :global(:root[data-theme="dark"]) .markdown-body :global(.hljs-string),
  :global(:root[data-theme="dark"]) .markdown-body :global(.hljs-title) {
    color: rgb(52 211 153);
  }

  :global(:root[data-theme="dark"]) .markdown-body :global(.hljs-number),
  :global(:root[data-theme="dark"]) .markdown-body :global(.hljs-attr) {
    color: rgb(251 146 60);
  }

  :global(:root[data-theme="dark"]) .markdown-body :global(.hljs-function) {
    color: rgb(96 165 250);
  }
</style>
