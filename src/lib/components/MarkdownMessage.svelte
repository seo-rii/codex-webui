<script lang="ts">
  import { createEventDispatcher } from "svelte";
  import { Marked, Renderer } from "marked";
  import { createLowlight, common } from "lowlight";
  import { toHtml } from "hast-util-to-html";

  let { text = "" }: { text?: string | null } = $props();

  const dispatch = createEventDispatcher<{
    openLocalPath: {
      href: string;
    };
  }>();
  let rootElement = $state<HTMLDivElement | null>(null);

  const lowlight = createLowlight(common);

  function escapeHtml(value: string) {
    return value
      .replaceAll("&", "&amp;")
      .replaceAll("<", "&lt;")
      .replaceAll(">", "&gt;")
      .replaceAll('"', "&quot;")
      .replaceAll("'", "&#39;");
  }

  function escapeAttribute(value: string) {
    return escapeHtml(value);
  }

  function safeHref(value: string) {
    const trimmed = value.trim();
    if (!trimmed) {
      return null;
    }
    if (
      trimmed.startsWith("/") ||
      trimmed.startsWith("./") ||
      trimmed.startsWith("../") ||
      trimmed.startsWith("#")
    ) {
      return trimmed;
    }
    try {
      const parsed = new URL(trimmed, "http://localhost");
      if (["http:", "https:", "mailto:"].includes(parsed.protocol)) {
        return trimmed;
      }
    } catch {
      return null;
    }
    return null;
  }

  const renderer = new Renderer();

  renderer.code = function ({ text: code, lang }) {
    const normalizedLanguage = lang?.trim().toLowerCase() ?? "";
    const tree = normalizedLanguage && lowlight.registered(normalizedLanguage)
      ? lowlight.highlight(normalizedLanguage, code)
      : lowlight.highlightAuto(code);
    const highlighted = toHtml(tree);
    const languageClass = normalizedLanguage ? ` language-${escapeAttribute(normalizedLanguage)}` : "";
    const languageLabel = normalizedLanguage || (tree as any).data?.language || "text";
    
    return `<div class="code-block group relative my-4 rounded-xl overflow-hidden border border-gray-200 bg-gray-50/50">
      <div class="flex items-center justify-between px-4 py-1.5 bg-gray-100/50 border-b border-gray-200 text-[10px] font-bold text-gray-500 uppercase tracking-widest">
        <span>${escapeHtml(languageLabel)}</span>
      </div>
      <pre class="p-4 overflow-x-auto text-sm leading-relaxed"><code class="hljs${languageClass}">${highlighted}</code></pre>
    </div>`;
  };

  renderer.html = function ({ text: html }) {
    return `<span>${escapeHtml(html)}</span>`;
  };

  renderer.link = function ({ href, title, tokens }) {
    const safe = safeHref(href);
    const body = this.parser.parseInline(tokens);
    if (!safe) {
      return `<span>${body}</span>`;
    }
    const titleAttr = title ? ` title="${escapeAttribute(title)}"` : "";
    const isLocal = safe.startsWith("/");
    const classes = isLocal 
      ? "text-amber-600 font-medium hover:underline decoration-amber-500/30 underline-offset-4" 
      : "text-amber-600 font-medium hover:underline decoration-amber-500/30 underline-offset-4 inline-flex items-center gap-0.5";
    
    return `<a href="${escapeAttribute(safe)}" class="${classes}" rel="noreferrer" target="_blank"${titleAttr}>${body}${isLocal ? '' : '<svg class="w-3 h-3 opacity-60" fill="none" stroke="currentColor" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg"><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6M15 3h6v6M10 14L21 3" stroke-linecap="round" stroke-linejoin="round" stroke-width="2"></path></svg>'}</a>`;
  };

  renderer.image = function ({ href, title, text: altText }) {
    const safe = safeHref(href);
    if (!safe) {
      return "";
    }
    const titleAttr = title ? ` title="${escapeAttribute(title)}"` : "";
    return `<div class="my-4 overflow-hidden rounded-xl border border-gray-200 shadow-sm"><img alt="${escapeAttribute(altText || "")}" class="w-full h-auto block" loading="lazy" src="${escapeAttribute(safe)}"${titleAttr} /></div>`;
  };

  const marked = new Marked({
    breaks: true,
    gfm: true,
    renderer
  });

  const html = $derived.by(() => {
    if (!text?.trim()) {
      return "";
    }
    return marked.parse(text) as string;
  });

  $effect(() => {
    if (!rootElement) {
      return;
    }

    const element = rootElement;
    element.addEventListener("click", handleClick as EventListener);
    return () => {
      element.removeEventListener("click", handleClick as EventListener);
    };
  });

  function handleClick(event: MouseEvent) {
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

<div bind:this={rootElement} class="markdown-body">
  {@html html}
</div>

<style>
  .markdown-body {
    color: rgb(31 41 55);
    font-size: 15px;
    line-height: 1.7;
  }

  .markdown-body :global(p) {
    margin: 0 0 1rem;
  }

  .markdown-body :global(h1),
  .markdown-body :global(h2),
  .markdown-body :global(h3),
  .markdown-body :global(h4) {
    margin: 1.5rem 0 0.75rem;
    color: rgb(17 24 39);
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
    border-left: 4px solid rgb(253 230 138);
    padding-left: 1rem;
    color: rgb(75 85 99);
    font-style: italic;
  }

  .markdown-body :global(code:not(.hljs)) {
    border: 1px solid rgb(229 231 235 / 0.5);
    border-radius: 0.375rem;
    background: rgb(243 244 246);
    color: rgb(17 24 39);
    padding: 0.125rem 0.375rem;
    font-family: "IBM Plex Mono", "SFMono-Regular", monospace;
    font-size: 0.9em;
  }

  .markdown-body :global(table) {
    width: 100%;
    margin: 0 0 1rem;
    border: 1px solid rgb(229 231 235);
    border-collapse: collapse;
    overflow: hidden;
    border-radius: 0.75rem;
  }

  .markdown-body :global(th),
  .markdown-body :global(td) {
    border: 1px solid rgb(229 231 235);
    padding: 0.75rem;
    text-align: left;
  }

  .markdown-body :global(th) {
    background: rgb(249 250 251);
    color: rgb(55 65 81);
    font-size: 0.75rem;
    font-weight: 700;
    letter-spacing: 0.08em;
    text-transform: uppercase;
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
</style>
