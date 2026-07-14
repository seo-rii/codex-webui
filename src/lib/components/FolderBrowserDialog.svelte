<script lang="ts">
  import { ChevronDown, ChevronUp, Layout, RefreshCw, X } from "lucide-svelte";

  import type { DirectoryPayload } from "$lib/types";

  let {
    busy,
    directoryPayload,
    title,
    subtitle,
    loadingLabel,
    closeLabel,
    confirmLabel,
    onBrowse,
    onClose
  }: {
    busy: boolean;
    directoryPayload: DirectoryPayload | null;
    title: string;
    subtitle: string;
    loadingLabel: string;
    closeLabel: string;
    confirmLabel: string;
    onBrowse: (path: string) => void | Promise<void>;
    onClose: () => void;
  } = $props();
</script>

<div aria-modal="true" class="fixed inset-0 flex items-center justify-center p-4 sm:p-6" role="dialog" style="z-index:var(--z-modal);">
  <button
    aria-label={closeLabel}
    class="ui-scrim ui-scrim--strong absolute inset-0"
    onclick={onClose}
    type="button"
  ></button>
  <div class="folder-dialog-card relative flex max-h-[80vh] w-full max-w-2xl flex-col overflow-hidden rounded-3xl bg-white shadow-2xl">
    <header class="folder-dialog-card__header flex items-center justify-between border-b border-gray-100 px-8 py-6">
      <div>
        <h2 class="text-xl font-bold text-gray-900">{title}</h2>
        <p class="mt-1 text-[10px] font-bold uppercase tracking-widest text-gray-400">{subtitle}</p>
      </div>
      <button class="rounded-xl p-2 text-gray-400 transition-all hover:bg-gray-100 hover:text-gray-900" onclick={onClose} type="button">
        <X size={20} />
      </button>
    </header>
    <div class="flex-1 overflow-y-auto p-6">
      {#if busy}
        <div class="flex flex-col items-center gap-4 py-24 text-gray-400">
          <RefreshCw size={32} class="animate-spin" />
          <p class="text-sm font-medium">{loadingLabel}</p>
        </div>
      {:else if directoryPayload}
        <div class="space-y-4">
          <div class="mb-6 flex items-center gap-2">
            <button
              class="rounded-lg bg-gray-100 p-2 text-gray-600 transition-all hover:bg-gray-200 disabled:opacity-30"
              disabled={!directoryPayload.parentPath}
              onclick={() => directoryPayload.parentPath && void onBrowse(directoryPayload.parentPath)}
              type="button"
            >
              <ChevronUp size={18} />
            </button>
            <div class="flex-1 truncate rounded-xl border border-gray-200 bg-gray-50 px-4 py-2 text-xs font-mono text-gray-600">
              {directoryPayload.currentPath}
            </div>
          </div>
          <div class="grid grid-cols-1 gap-1">
            {#each directoryPayload.entries as entry (entry.path)}
              <button class="group flex items-center justify-between rounded-xl p-3 text-left transition-all hover:bg-gray-50" onclick={() => void onBrowse(entry.path)} type="button">
                <div class="flex items-center gap-3">
                  <div class="rounded-lg bg-amber-50 p-2 text-amber-600 group-hover:bg-amber-100">
                    <Layout size={16} />
                  </div>
                  <div>
                    <span class="text-sm font-semibold text-gray-700">{entry.name}</span>
                    <p class="mt-0.5 text-[10px] font-mono text-gray-400">{entry.path}</p>
                  </div>
                </div>
                <ChevronDown size={14} class="-rotate-90 text-gray-300 group-hover:text-gray-500" />
              </button>
            {/each}
          </div>
        </div>
      {/if}
    </div>
    <div class="folder-dialog-card__footer flex items-center justify-end border-t border-gray-100 bg-gray-50 px-8 py-4">
      <button class="rounded-xl bg-amber-600 px-6 py-2 text-xs font-bold text-white shadow-md transition-all active:scale-95 hover:bg-amber-700" onclick={onClose} type="button">
        {confirmLabel}
      </button>
    </div>
  </div>
</div>
