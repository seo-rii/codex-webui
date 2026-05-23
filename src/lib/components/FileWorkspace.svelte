<script lang="ts">
  import { Edit3, FileDiff, FileText, GitBranch, RefreshCw, Save, X } from "lucide-svelte";

  import { api } from "$lib/api";
  import MarkdownMessage from "$lib/components/MarkdownMessage.svelte";
  import MonacoTextEditor from "$lib/components/MonacoTextEditor.svelte";
  import { m } from "$lib/paraglide/messages.js";
  import type { EditableFilePayload } from "$lib/types";

  type ViewMode = "preview" | "source";

  let {
    filePath,
    readOnly = false,
    onClose,
    onOpenDiff,
    onOpenGit,
    onOpenLocalPath
  }: {
    filePath: string;
    readOnly?: boolean;
    onClose: () => void | Promise<void>;
    onOpenDiff?: (filePath: string) => void | Promise<void>;
    onOpenGit?: (filePath: string) => void | Promise<void>;
    onOpenLocalPath?: (href: string) => void | Promise<void>;
  } = $props();

  let payload = $state<EditableFilePayload | null>(null);
  let editorValue = $state("");
  let loading = $state(false);
  let saving = $state(false);
  let errorText = $state("");
  let noticeText = $state("");
  let editMode = $state(false);
  let viewMode = $state<ViewMode>("source");
  let loadedPath = "";
  let loadVersion = 0;

  const displayName = $derived(payload?.displayName || baseName(filePath) || filePath);
  const activePath = $derived(payload?.path || filePath);
  const markdown = $derived(isMarkdownPath(activePath));
  const dirty = $derived(Boolean(payload && editorValue !== payload.content));
  const canEdit = $derived(Boolean(payload?.writable && !readOnly));

  function baseName(path: string) {
    return path.split(/[\\/]/u).filter(Boolean).at(-1) ?? path;
  }

  function isMarkdownPath(path: string) {
    return /\.(md|mdown|markdown|mdx)$/iu.test(path);
  }

  function describeError(error: unknown) {
    return error instanceof Error ? error.message : String(error);
  }

  async function loadFile(path: string) {
    const version = ++loadVersion;
    loading = true;
    errorText = "";
    noticeText = "";
    try {
      const nextPayload = await api.getEditableFile(path);
      if (version !== loadVersion) {
        return;
      }
      payload = nextPayload;
      editorValue = nextPayload.content;
      editMode = false;
      viewMode = "source";
    } catch (error) {
      if (version !== loadVersion) {
        return;
      }
      payload = null;
      editorValue = "";
      errorText = describeError(error);
    } finally {
      if (version === loadVersion) {
        loading = false;
      }
    }
  }

  async function saveFile() {
    if (!payload || !canEdit || !dirty) {
      return;
    }
    saving = true;
    errorText = "";
    noticeText = "";
    try {
      const nextPayload = await api.saveEditableFile(payload.path, editorValue);
      payload = nextPayload;
      editorValue = nextPayload.content;
      editMode = false;
      noticeText = "";
    } catch (error) {
      errorText = describeError(error);
    } finally {
      saving = false;
    }
  }

  $effect(() => {
    const nextPath = filePath.trim();
    if (!nextPath || nextPath === loadedPath) {
      return;
    }
    loadedPath = nextPath;
    void loadFile(nextPath);
  });
</script>

<div class="file-workspace">
  <div class="file-workspace__inner">
    <header class="file-workspace__header">
      <div class="min-w-0">
        <div class="file-workspace__title-row">
          <FileText size={18} />
          <h2>{displayName}</h2>
        </div>
        <p title={activePath}>{activePath}</p>
      </div>
      <div class="file-workspace__actions">
        {#if onOpenDiff}
          <button class="file-workspace__button" onclick={() => void onOpenDiff(activePath)} type="button">
            <FileDiff size={14} />
            <span>{m.diff()}</span>
          </button>
        {/if}
        {#if onOpenGit}
          <button class="file-workspace__button" onclick={() => void onOpenGit(activePath)} type="button">
            <GitBranch size={14} />
            <span>{m.file_viewer_open_git()}</span>
          </button>
        {/if}
        {#if markdown}
          <button class="file-workspace__button" data-active={viewMode === "preview" && !editMode} onclick={() => { editMode = false; viewMode = "preview"; }} type="button">
            {m.file_viewer_preview()}
          </button>
        {/if}
        <button class="file-workspace__button" data-active={viewMode === "source" || editMode} onclick={() => { editMode = false; viewMode = "source"; }} type="button">
          <FileText size={14} />
          <span>{m.file_viewer_source()}</span>
        </button>
        <button class="file-workspace__button" disabled={!canEdit} data-active={editMode} onclick={() => { editMode = true; viewMode = "source"; }} type="button">
          <Edit3 size={14} />
          <span>{m.edit()}</span>
        </button>
        <button class="file-workspace__button file-workspace__button--strong" disabled={!dirty || !canEdit || saving} onclick={() => void saveFile()} type="button">
          {#if saving}
            <span class="file-workspace__spin"><RefreshCw size={14} /></span>
            <span>{m.saving()}</span>
          {:else}
            <Save size={14} />
            <span>{m.save()}</span>
          {/if}
        </button>
        <button class="file-workspace__icon-button" onclick={() => void onClose()} type="button" title={m.close()}>
          <X size={18} />
        </button>
      </div>
    </header>

    {#if loading}
      <div class="file-workspace__loading">
        <div class="file-workspace__progress"></div>
      </div>
    {/if}

    {#if errorText}
      <div class="file-workspace__message file-workspace__message--error">{errorText}</div>
    {:else if noticeText}
      <div class="file-workspace__message">{noticeText}</div>
    {/if}

    <section class="file-workspace__panel">
      {#if loading && !payload}
        <div class="file-workspace__empty">
          <span class="file-workspace__spin"><RefreshCw size={18} /></span>
          <span>{m.loading_file()}</span>
        </div>
      {:else if payload && markdown && viewMode === "preview" && !editMode}
        <div class="file-workspace__markdown">
          <MarkdownMessage
            text={editorValue}
            on:openLocalPath={(event: CustomEvent<{ href: string }>) => {
              if (onOpenLocalPath) {
                void onOpenLocalPath(event.detail.href);
              }
            }}
          />
        </div>
      {:else if payload}
        <MonacoTextEditor bind:value={editorValue} height={640} path={activePath} readonly={!editMode || !canEdit} />
      {:else}
        <div class="file-workspace__empty">{m.loading_file()}</div>
      {/if}
    </section>
  </div>
</div>

<style>
  .file-workspace {
    height: 100%;
    overflow-y: auto;
    background: color-mix(in srgb, var(--panel-soft) 72%, transparent);
    padding: 1.25rem;
  }

  .file-workspace__inner {
    margin: 0 auto;
    max-width: 82rem;
    min-height: 100%;
  }

  .file-workspace__header {
    position: sticky;
    top: 0;
    z-index: 18;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    border: 1px solid var(--line);
    border-radius: 1rem;
    background: color-mix(in srgb, var(--panel-strong) 94%, transparent);
    padding: 0.85rem;
    box-shadow: 0 18px 40px -34px rgba(15, 23, 42, 0.45);
    backdrop-filter: blur(14px);
  }

  .file-workspace__title-row {
    display: flex;
    min-width: 0;
    align-items: center;
    gap: 0.5rem;
    color: var(--ink-strong);
  }

  .file-workspace__title-row h2 {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 1rem;
    font-weight: 800;
  }

  .file-workspace__header p {
    margin-top: 0.25rem;
    max-width: min(48rem, 72vw);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--muted);
    font-size: 0.75rem;
    font-weight: 600;
  }

  .file-workspace__actions {
    display: flex;
    flex-shrink: 0;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 0.4rem;
  }

  .file-workspace__button,
  .file-workspace__icon-button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 0.35rem;
    border: 1px solid var(--line);
    border-radius: 0.75rem;
    background: var(--panel-strong);
    color: var(--ink);
    min-height: 2rem;
    padding: 0 0.7rem;
    font-size: 0.75rem;
    font-weight: 800;
    transition:
      transform 140ms ease,
      background 140ms ease,
      color 140ms ease,
      border-color 140ms ease;
  }

  .file-workspace__icon-button {
    width: 2rem;
    padding: 0;
  }

  .file-workspace__button:hover:not(:disabled),
  .file-workspace__icon-button:hover {
    transform: translateY(-1px);
    border-color: color-mix(in srgb, var(--accent) 34%, var(--line));
    background: var(--accent-soft);
    color: var(--accent);
  }

  .file-workspace__button[data-active="true"] {
    border-color: color-mix(in srgb, var(--accent) 38%, var(--line));
    background: var(--accent-soft);
    color: var(--accent);
  }

  .file-workspace__button:disabled {
    cursor: not-allowed;
    opacity: 0.48;
  }

  .file-workspace__button--strong {
    background: var(--ink-strong);
    color: var(--panel-strong);
  }

  .file-workspace__loading {
    position: sticky;
    top: 4.65rem;
    z-index: 17;
    overflow: hidden;
    border-radius: 999px;
    height: 0.2rem;
    margin: 0.6rem 0 -0.8rem;
    background: color-mix(in srgb, var(--line) 70%, transparent);
  }

  .file-workspace__progress {
    height: 100%;
    width: 45%;
    border-radius: inherit;
    background: linear-gradient(90deg, transparent, var(--accent), transparent);
    animation: file-progress 1.15s ease-in-out infinite;
  }

  .file-workspace__message {
    margin-top: 0.8rem;
    border: 1px solid color-mix(in srgb, var(--accent) 28%, var(--line));
    border-radius: 0.9rem;
    background: var(--accent-soft);
    color: var(--accent);
    padding: 0.65rem 0.85rem;
    font-size: 0.78rem;
    font-weight: 700;
  }

  .file-workspace__message--error {
    border-color: rgba(248, 113, 113, 0.35);
    background: rgba(248, 113, 113, 0.12);
    color: #dc2626;
  }

  .file-workspace__panel {
    margin-top: 1rem;
    overflow: hidden;
    border: 1px solid var(--line);
    border-radius: 1.15rem;
    background: var(--panel-strong);
    box-shadow: 0 24px 60px -44px rgba(15, 23, 42, 0.42);
  }

  .file-workspace__markdown {
    padding: clamp(1rem, 3vw, 2rem);
  }

  .file-workspace__empty {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.5rem;
    min-height: 18rem;
    color: var(--muted);
    font-size: 0.85rem;
    font-weight: 700;
  }

  .file-workspace__spin {
    display: inline-flex;
    animation: file-spin 1s linear infinite;
  }

  @keyframes file-progress {
    0% {
      transform: translateX(-100%);
    }
    100% {
      transform: translateX(230%);
    }
  }

  @keyframes file-spin {
    to {
      transform: rotate(360deg);
    }
  }

  @media (max-width: 760px) {
    .file-workspace {
      padding: 0.75rem;
    }

    .file-workspace__header {
      align-items: flex-start;
      flex-direction: column;
    }

    .file-workspace__actions {
      width: 100%;
      justify-content: flex-start;
    }

    .file-workspace__button {
      flex: 1 1 auto;
    }
  }
</style>
