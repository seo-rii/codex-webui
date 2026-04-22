<script lang="ts">
  import { ChevronDown, RefreshCw, Search, X } from "lucide-svelte";

  import { m } from "$lib/paraglide/messages.js";
  import type { SessionTurnSearchMatch } from "$lib/types";

  type SearchCopy = {
    placeholder: string;
    results: string;
    hint: string;
    noResults: string;
    turn: string;
    loadMore: string;
  };

  let {
    popoverElement = $bindable(),
    inputElement = $bindable(),
    style = "",
    query = $bindable(""),
    results,
    busy,
    totalMatches,
    error,
    jumpingTurnId,
    cursor,
    loadingMore,
    searchCopy,
    planModeLabel,
    contextCompressionLabel,
    closeLabel,
    onInput,
    onReset,
    onJump,
    onLoadMore
  }: {
    popoverElement?: HTMLDivElement | undefined;
    inputElement?: HTMLInputElement | undefined;
    style?: string;
    query: string;
    results: SessionTurnSearchMatch[];
    busy: boolean;
    totalMatches: number;
    error: string;
    jumpingTurnId: string | null;
    cursor: string | null;
    loadingMore: boolean;
    searchCopy: SearchCopy;
    planModeLabel: string;
    contextCompressionLabel: string;
    closeLabel: string;
    onInput: () => void;
    onReset: () => void;
    onJump: (match: SessionTurnSearchMatch) => void | Promise<void>;
    onLoadMore: () => void | Promise<void>;
  } = $props();
</script>

<div
  bind:this={popoverElement}
  class="composer-popover search-popover fixed z-[72] overflow-hidden rounded-2xl border border-gray-200 bg-white shadow-2xl"
  style={style || "opacity:0;pointer-events:none;"}
>
  <div class="search-popover__header flex items-center gap-2 border-b border-gray-100 px-3 py-2.5">
    <Search size={14} class="shrink-0 text-gray-400" />
    <input
      bind:this={inputElement}
      bind:value={query}
      class="search-popover__input w-full border-none bg-transparent p-0 text-sm text-gray-800 placeholder-gray-400 focus:outline-none focus:ring-0"
      oninput={onInput}
      onkeydown={(event) => {
        if (event.key === "Escape") {
          event.preventDefault();
          onReset();
        }
        if (event.key === "Enter" && results.length > 0) {
          event.preventDefault();
          void onJump(results[0]);
        }
      }}
      placeholder={searchCopy.placeholder}
      type="search"
    />
    {#if busy}
      <RefreshCw size={14} class="shrink-0 animate-spin text-gray-400" />
    {:else if query.trim()}
      <span class="search-popover__meta shrink-0 text-[10px] font-bold uppercase tracking-widest text-gray-400">
        {totalMatches} {searchCopy.results}
      </span>
    {/if}
    <button
      class="search-popover__close ui-animated-button ui-animated-button--icon rounded-lg p-1 text-gray-400 transition-colors hover:bg-gray-100 hover:text-gray-700"
      onclick={onReset}
      title={closeLabel}
      type="button"
    >
      <X size={14} />
    </button>
  </div>
  <div class="max-h-72 overflow-y-auto overscroll-contain">
    {#if error}
      <div class="search-popover__empty px-3 py-3 text-xs text-red-600">{error}</div>
    {:else if !query.trim()}
      <div class="search-popover__empty px-3 py-3 text-xs leading-relaxed text-gray-500">{searchCopy.hint}</div>
    {:else if !busy && results.length === 0}
      <div class="search-popover__empty px-3 py-3 text-xs text-gray-500">{searchCopy.noResults}</div>
    {:else}
      <div class="search-popover__list divide-y divide-gray-100">
        {#each results as result (`${result.turnId}:${result.itemId ?? "turn"}:${result.preview}`)}
          <button
            class="search-popover__item ui-animated-button ui-animated-button--soft flex w-full items-start justify-between gap-3 px-3 py-3 text-left transition-colors hover:bg-amber-50/60"
            disabled={jumpingTurnId === result.turnId}
            onclick={() => void onJump(result)}
            type="button"
          >
            <div class="min-w-0 flex-1">
              <div class="flex flex-wrap items-center gap-1.5">
                <span class="search-popover__badge rounded-full bg-gray-100 px-2 py-0.5 text-[10px] font-bold uppercase tracking-widest text-gray-500">
                  {searchCopy.turn} {result.turnIndex + 1}
                </span>
                <span class="rounded-full bg-amber-50 px-2 py-0.5 text-[10px] font-bold uppercase tracking-widest text-amber-700">
                  {result.itemType === "userMessage"
                    ? "User"
                    : result.itemType === "agentMessage"
                      ? "Assistant"
                      : result.itemType === "reasoning"
                        ? m.reasoning()
                        : result.itemType === "plan"
                          ? planModeLabel
                          : result.itemType === "commandExecution"
                            ? m.run_command()
                            : result.itemType === "fileChange"
                              ? m.files_changed_fallback()
                              : result.itemType === "webSearch"
                                ? m.web_search()
                                : result.itemType === "mcpToolCall"
                                  ? m.mcp_call()
                                  : result.itemType === "dynamicToolCall"
                                    ? m.tool_call()
                                    : result.itemType === "contextCompaction"
                                      ? contextCompressionLabel
                                      : result.itemType ?? "Item"}
                </span>
              </div>
              <p class="mt-2 line-clamp-2 text-sm leading-5 text-gray-700">{result.preview}</p>
            </div>
            {#if jumpingTurnId === result.turnId}
              <RefreshCw size={14} class="mt-1 shrink-0 animate-spin text-gray-400" />
            {:else}
              <ChevronDown size={14} class="-rotate-90 shrink-0 text-gray-300" />
            {/if}
          </button>
        {/each}
      </div>
      {#if cursor}
        <div class="border-t border-gray-100 px-3 py-2.5">
          <button
            class="ui-animated-button ui-animated-button--soft flex w-full items-center justify-center gap-2 rounded-xl border border-gray-200 bg-white px-3 py-2 text-xs font-bold text-gray-700 transition-colors hover:bg-gray-50 disabled:cursor-not-allowed disabled:opacity-60"
            disabled={loadingMore}
            onclick={() => void onLoadMore()}
            type="button"
          >
            {#if loadingMore}
              <RefreshCw size={13} class="animate-spin" />
            {/if}
            <span>{searchCopy.loadMore}</span>
          </button>
        </div>
      {/if}
    {/if}
  </div>
</div>
