<script lang="ts">
  import { onMount } from "svelte";

  type MonacoDiffEditorComponent = typeof import("$lib/components/MonacoDiffEditor.svelte").default;

  let {
    original,
    modified,
    path,
    height = 320,
    fallbackText = ""
  }: {
    original: string;
    modified: string;
    path: string;
    height?: number;
    fallbackText?: string;
  } = $props();

  let EditorComponent = $state<MonacoDiffEditorComponent | null>(null);
  let loadError = $state("");

  onMount(() => {
    let cancelled = false;

    void import("$lib/components/MonacoDiffEditor.svelte")
      .then((module) => {
        if (!cancelled) {
          EditorComponent = module.default;
        }
      })
      .catch((error) => {
        if (!cancelled) {
          loadError = error instanceof Error ? error.message : "Failed to load diff editor.";
        }
      });

    return () => {
      cancelled = true;
    };
  });
</script>

{#if EditorComponent}
  <EditorComponent {fallbackText} {height} {modified} {original} {path} />
{:else if loadError}
  <pre class="lazy-monaco-fallback">{fallbackText || modified || original}</pre>
{:else}
  <div aria-busy="true" class="lazy-monaco-placeholder" style={`height:${height}px`}>
    <div class="lazy-monaco-placeholder__bar"></div>
  </div>
{/if}

<style>
  .lazy-monaco-placeholder {
    display: flex;
    min-height: 0;
    align-items: center;
    justify-content: center;
    overflow: hidden;
    border-radius: 1rem;
    background:
      linear-gradient(90deg, color-mix(in srgb, var(--panel-soft) 82%, transparent) 0%, color-mix(in srgb, var(--panel) 88%, transparent) 50%, color-mix(in srgb, var(--panel-soft) 82%, transparent) 100%);
    background-size: 220% 100%;
    animation: lazy-monaco-shimmer 1.2s linear infinite;
  }

  .lazy-monaco-placeholder__bar {
    width: min(16rem, 56%);
    height: 0.4rem;
    border-radius: 999px;
    background: color-mix(in srgb, var(--brand) 18%, var(--ink) 8%);
  }

  .lazy-monaco-fallback {
    overflow: auto;
    border-radius: 1rem;
    background: var(--panel-soft);
    color: var(--ink);
    padding: 0.95rem 1rem;
    white-space: pre-wrap;
    word-break: break-word;
  }

  @keyframes lazy-monaco-shimmer {
    0% {
      background-position: 200% 0;
    }

    100% {
      background-position: -20% 0;
    }
  }
</style>
