<script lang="ts">
  import { X } from "lucide-svelte";

  import { m } from "$lib/paraglide/messages.js";
  import LazyMonacoDiffEditor from "$lib/components/LazyMonacoDiffEditor.svelte";

  type FileChangeView = {
    path: string;
    kind: "add" | "delete" | "update";
    movePath: string | null;
    diff: string;
    original: string;
    modified: string;
    renderable: boolean;
  };

  let {
    title,
    views,
    onClose
  }: {
    title: string;
    views: FileChangeView[];
    onClose: () => void | Promise<void>;
  } = $props();
</script>

<div class="h-full overflow-y-auto bg-gray-50/30 p-8">
  <div class="mx-auto max-w-5xl space-y-8">
    <div class="flex items-end justify-between">
      <div>
        <h2 class="text-2xl font-bold text-gray-900">{title}</h2>
        <p class="mt-1 text-sm text-gray-500">{m.files_count({ count: String(views.length) })}</p>
      </div>
      <button class="rounded-xl p-2 text-gray-400 transition-all hover:text-red-600" onclick={() => void onClose()} type="button">
        <X size={20} />
      </button>
    </div>

    <div class="space-y-6">
      {#each views as change}
        <div class="overflow-hidden rounded-2xl border border-gray-200 bg-white shadow-sm">
          <div class="flex items-center justify-between border-b border-gray-200 bg-gray-50 px-5 py-3">
            <div class="flex items-center gap-3">
              <span class="text-sm font-bold text-gray-900">{change.path}</span>
              <span class="rounded bg-amber-100 px-2 py-0.5 text-[10px] font-bold uppercase tracking-widest text-amber-700">
                {change.kind}
              </span>
            </div>
          </div>

          <div class="p-0">
            {#if change.renderable}
              <LazyMonacoDiffEditor fallbackText={change.diff} height={400} modified={change.modified} original={change.original} path={change.path} />
            {:else}
              <pre class="overflow-x-auto bg-gray-50/50 p-6 text-xs font-mono text-gray-600">{change.diff}</pre>
            {/if}
          </div>
        </div>
      {/each}
    </div>
  </div>
</div>
