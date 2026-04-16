<script lang="ts">
  import { onMount } from "svelte";

  import { loadMonaco } from "$lib/monaco";
  import { getResolvedTheme, subscribeThemeChange } from "$lib/theme";

  let {
    value = $bindable(""),
    path,
    height = 320,
    readonly = false
  }: {
    value: string;
    path: string;
    height?: number;
    readonly?: boolean;
  } = $props();

  let container = $state<HTMLDivElement | null>(null);

  let monacoRef: typeof import("monaco-editor") | null = null;
  let editor: import("monaco-editor").editor.IStandaloneCodeEditor | null = null;
  let model: import("monaco-editor").editor.ITextModel | null = null;
  let modelChangeSubscription: import("monaco-editor").IDisposable | null = null;
  let syncing = false;

  function getMonacoTheme() {
    return getResolvedTheme() === "dark" ? "vs-dark" : "vs";
  }

  function syncMonacoTheme() {
    if (!monacoRef) {
      return;
    }
    monacoRef.editor.setTheme(getMonacoTheme());
  }

  function attachModel() {
    if (!monacoRef || !editor) {
      return;
    }

    modelChangeSubscription?.dispose();
    model?.dispose();
    model = monacoRef.editor.createModel(value, undefined, monacoRef.Uri.file(path));
    editor.setModel(model);
    editor.updateOptions({ readOnly: readonly });

    modelChangeSubscription = editor.onDidChangeModelContent(() => {
      if (syncing || !model) {
        return;
      }
      value = model.getValue();
    });
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

      monacoRef = monaco;
      editor = monaco.editor.create(container, {
        automaticLayout: true,
        minimap: { enabled: false },
        theme: getMonacoTheme(),
        lineNumbers: "on",
        scrollBeyondLastLine: false,
        readOnly: readonly
      });
      attachModel();
    });

    return () => {
      disposed = true;
      modelChangeSubscription?.dispose();
      model?.dispose();
      editor?.dispose();
      releaseThemeChange();
    };
  });

  $effect(() => {
    if (!model || model.getValue() === value) {
      return;
    }
    syncing = true;
    model.setValue(value);
    syncing = false;
  });

  $effect(() => {
    path;
    readonly;
    attachModel();
  });
</script>

<div bind:this={container} class="monaco-surface" style={`height:${height}px`}></div>

<style>
  .monaco-surface {
    min-height: 0;
    overflow: hidden;
  }
</style>
