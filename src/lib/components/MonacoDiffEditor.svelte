<script module lang="ts">
  let monacoEditorInstance = 0;
</script>

<script lang="ts">
  import { onMount } from "svelte";

  import { loadMonaco } from "$lib/monaco";
  import { m } from "$lib/paraglide/messages.js";
  import { getResolvedTheme, subscribeThemeChange } from "$lib/theme";

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

  let container = $state<HTMLDivElement | null>(null);
  let loadError = $state("");

  let monacoRef: typeof import("monaco-editor") | null = null;
  let diffEditor: import("monaco-editor").editor.IStandaloneDiffEditor | null = null;
  let originalModel: import("monaco-editor").editor.ITextModel | null = null;
  let modifiedModel: import("monaco-editor").editor.ITextModel | null = null;
  const editorInstanceId = `diff-${++monacoEditorInstance}`;

  function getMonacoTheme() {
    return getResolvedTheme() === "dark" ? "vs-dark" : "vs";
  }

  function syncMonacoTheme() {
    if (!monacoRef) {
      return;
    }
    monacoRef.editor.setTheme(getMonacoTheme());
  }

  function updateModels() {
    if (!monacoRef || !diffEditor) {
      return;
    }

    try {
      loadError = "";
      diffEditor.setModel(null);
      originalModel?.dispose();
      modifiedModel?.dispose();

      const safePath = path.trim() || "file";
      const originalUri = monacoRef.Uri.parse(`codex-webui-diff://${editorInstanceId}/original/${encodeURIComponent(safePath)}`);
      const modifiedUri = monacoRef.Uri.parse(`codex-webui-diff://${editorInstanceId}/modified/${encodeURIComponent(safePath)}`);
      monacoRef.editor.getModel(originalUri)?.dispose();
      monacoRef.editor.getModel(modifiedUri)?.dispose();

      originalModel = monacoRef.editor.createModel(String(original ?? ""), undefined, originalUri);
      modifiedModel = monacoRef.editor.createModel(String(modified ?? ""), undefined, modifiedUri);
      diffEditor.setModel({
        original: originalModel,
        modified: modifiedModel
      });
    } catch (error) {
      loadError = error instanceof Error ? error.message : m.render_diff_failed();
    }
  }

  onMount(() => {
    let disposed = false;
    const releaseThemeChange = subscribeThemeChange(() => {
      syncMonacoTheme();
    });

    void loadMonaco().then((monaco) => {
      if (disposed || !container) {
        return;
      }

      try {
        monacoRef = monaco;
        diffEditor = monaco.editor.createDiffEditor(container, {
          automaticLayout: true,
          readOnly: true,
          minimap: { enabled: false },
          renderSideBySide: !window.matchMedia("(max-width: 860px)").matches,
          theme: getMonacoTheme(),
          lineNumbers: "on"
        });
        updateModels();
      } catch (error) {
        loadError = error instanceof Error ? error.message : m.init_diff_editor_failed();
      }
    });

    return () => {
      disposed = true;
      diffEditor?.setModel(null);
      originalModel?.dispose();
      modifiedModel?.dispose();
      diffEditor?.dispose();
      releaseThemeChange();
    };
  });

  $effect(() => {
    original;
    modified;
    path;
    updateModels();
  });
</script>

<div bind:this={container} class:hidden={Boolean(loadError)} class="monaco-surface" style={`height:${height}px`}></div>
{#if loadError}
  <pre class="monaco-fallback">{fallbackText || modified || original}</pre>
{/if}

<style>
  .monaco-surface {
    min-height: 0;
    overflow: hidden;
  }

  .hidden {
    display: none;
  }

  .monaco-fallback {
    overflow: auto;
    border-radius: 1rem;
    background: var(--panel-soft);
    color: var(--ink);
    padding: 0.95rem 1rem;
    white-space: pre-wrap;
    word-break: break-word;
  }
</style>
