<script lang="ts">
  import {
    Archive,
    ArrowRightLeft,
    ChevronDown,
    GitBranch,
    History,
    Menu,
    MessageSquare,
    Pencil,
    Pin,
    Plus,
    RotateCcw,
    Search,
    Settings,
    Terminal
  } from "lucide-svelte";

  import { m } from "$lib/paraglide/messages.js";
  import type { SessionSummary } from "$lib/types";

  type UiCopy = {
    threadTitle: string;
    generatingResponse: string;
    handoffToNewThread: string;
    restoreThread: string;
    archiveThread: string;
    open: string;
    newThread: string;
    tasks: string;
    gitWorkspace: string;
    settingsSkills: string;
    newTerminal: string;
  };

  let {
    isMobileLayout,
    titleDraft = $bindable(""),
    titleInputElement = $bindable(),
    running,
    selectedSessionSummary,
    readOnly,
    tokenCountLabel,
    contextUsageLabel,
    selectedSessionId,
    activeWorkspaceTabId,
    searchTriggerElement = $bindable(),
    sessionSearchOpen,
    showArchivedSessions,
    workspaceMenuOpen = $bindable(false),
    ui,
    searchOpenLabel,
    onOpenMobileSidebar,
    onSaveTitle,
    onEditTags,
    onToggleSearch,
    onForkHandoff,
    onTogglePinned,
    onToggleArchive,
    onCreateSession,
    onOpenTasksTab,
    onOpenGitTab,
    onOpenSettingsTab,
    onCreateTerminalTab
  }: {
    isMobileLayout: boolean;
    titleDraft: string;
    titleInputElement?: HTMLInputElement | undefined;
    running: boolean;
    selectedSessionSummary: SessionSummary | null;
    readOnly: boolean;
    tokenCountLabel: string | null;
    contextUsageLabel: string | null;
    selectedSessionId: string | null;
    activeWorkspaceTabId: string;
    searchTriggerElement?: HTMLButtonElement | undefined;
    sessionSearchOpen: boolean;
    showArchivedSessions: boolean;
    workspaceMenuOpen: boolean;
    ui: UiCopy;
    searchOpenLabel: string;
    onOpenMobileSidebar: () => void;
    onSaveTitle: () => void | Promise<void>;
    onEditTags: () => void | Promise<void>;
    onToggleSearch: () => void;
    onForkHandoff: () => void | Promise<void>;
    onTogglePinned: () => void | Promise<void>;
    onToggleArchive: () => void | Promise<void>;
    onCreateSession: () => void | Promise<void>;
    onOpenTasksTab: () => void;
    onOpenGitTab: () => void;
    onOpenSettingsTab: () => void;
    onCreateTerminalTab: () => void | Promise<void>;
  } = $props();
</script>

<header class="sticky top-0 z-40 flex items-center justify-between border-b border-gray-100 bg-white/80 px-6 py-3 backdrop-blur-md">
  <div class="flex min-w-0 items-center gap-4">
    {#if isMobileLayout}
      <button class="-ml-2 rounded-lg p-2 text-gray-500 transition-colors hover:bg-gray-100 hover:text-gray-900" onclick={onOpenMobileSidebar} type="button">
        <Menu size={20} />
      </button>
    {/if}

    <div class="flex min-w-0 flex-col">
      <div class="flex min-w-0 items-center gap-2">
        <input
          bind:this={titleInputElement}
          bind:value={titleDraft}
          class="w-full max-w-md truncate border-none bg-transparent p-0 text-sm font-semibold placeholder-gray-400 focus:ring-0"
          onblur={() => void onSaveTitle()}
          onkeydown={(event) => {
            if (event.key !== "Enter") {
              return;
            }
            event.preventDefault();
            void onSaveTitle();
          }}
          placeholder={ui.threadTitle}
          readonly={readOnly}
        />
        {#if running}
          <span
            aria-label={ui.generatingResponse}
            class="inline-flex h-2.5 w-2.5 shrink-0 animate-pulse rounded-full bg-amber-500 shadow-[0_0_8px_rgba(245,158,11,0.45)]"
            title={ui.generatingResponse}
          ></span>
        {/if}
      </div>
      {#if selectedSessionSummary}
        <div class="mt-1.5 flex flex-wrap items-center gap-1.5">
          {#if selectedSessionSummary.pinned}
            <span class="inline-flex items-center gap-1 rounded-full border border-amber-200 bg-amber-50 px-2 py-0.5 text-[10px] font-bold text-amber-700">
              <Pin size={10} />
              {m.pinned_only()}
            </span>
          {/if}
          {#each selectedSessionSummary.tags as tag (tag)}
            <span class="inline-flex items-center rounded-full border border-gray-200 bg-gray-50 px-2 py-0.5 text-[10px] font-semibold text-gray-500">
              {tag}
            </span>
          {/each}
          <button
            class="ui-animated-button ui-animated-button--soft inline-flex items-center gap-1 rounded-full border border-gray-200 bg-white px-2 py-0.5 text-[10px] font-semibold text-gray-500 hover:text-gray-800 disabled:cursor-not-allowed disabled:opacity-50"
            disabled={readOnly}
            onclick={() => void onEditTags()}
            type="button"
          >
            <Pencil size={10} />
            <span>{m.edit_tags()}</span>
          </button>
        </div>
      {/if}
    </div>
  </div>

  <div class="app-header-actions flex min-w-0 shrink-0 items-center gap-1 sm:gap-1.5">
    {#if tokenCountLabel}
      <div class="mr-1.5 hidden items-center gap-1.5 xl:flex">
        <span class="rounded-md border border-gray-100 bg-gray-50 px-1.5 py-0.5 text-[9px] font-bold uppercase tracking-tight text-gray-500">
          {tokenCountLabel}
        </span>
        {#if contextUsageLabel}
          <span class="rounded-md border border-amber-100 bg-amber-50 px-1.5 py-0.5 text-[9px] font-bold uppercase tracking-tight text-amber-700">
            {contextUsageLabel}
          </span>
        {/if}
      </div>
    {/if}

    {#if selectedSessionId && activeWorkspaceTabId === "chat"}
      <button
        bind:this={searchTriggerElement}
        class={`ui-animated-button ui-animated-button--icon inline-flex h-8 w-8 items-center justify-center rounded-lg p-0 transition-all sm:h-9 sm:w-9 ${
          sessionSearchOpen ? "bg-amber-50 text-amber-600 hover:bg-amber-100" : "text-gray-400 hover:bg-amber-50 hover:text-amber-600"
        }`}
        onclick={onToggleSearch}
        title={searchOpenLabel}
        type="button"
      >
        <Search size={18} />
      </button>
      <button
        class="ui-animated-button ui-animated-button--icon inline-flex h-8 w-8 items-center justify-center rounded-lg p-0 text-gray-400 transition-all hover:bg-amber-50 hover:text-amber-600 disabled:cursor-not-allowed disabled:opacity-50 sm:h-9 sm:w-9"
        disabled={readOnly}
        onclick={() => void onForkHandoff()}
        title={ui.handoffToNewThread}
        type="button"
      >
        <ArrowRightLeft size={18} />
      </button>
      <button
        class={`ui-animated-button ui-animated-button--icon inline-flex h-8 w-8 items-center justify-center rounded-lg p-0 transition-all sm:h-9 sm:w-9 ${
          selectedSessionSummary?.pinned ? "bg-amber-50 text-amber-600 hover:bg-amber-100" : "text-gray-400 hover:bg-amber-50 hover:text-amber-600"
        }`}
        disabled={readOnly}
        onclick={() => void onTogglePinned()}
        title={selectedSessionSummary?.pinned ? m.unpin_thread() : m.pin_thread()}
        type="button"
      >
        <Pin size={18} />
      </button>
      <button
        class="ui-animated-button ui-animated-button--icon inline-flex h-8 w-8 items-center justify-center rounded-lg p-0 text-gray-400 transition-all hover:bg-amber-50 hover:text-amber-600 disabled:cursor-not-allowed disabled:opacity-50 sm:h-9 sm:w-9"
        disabled={readOnly}
        onclick={() => void onToggleArchive()}
        title={showArchivedSessions ? ui.restoreThread : ui.archiveThread}
        type="button"
      >
        {#if showArchivedSessions}
          <RotateCcw size={18} />
        {:else}
          <Archive size={18} />
        {/if}
      </button>
    {/if}

    <div class="mx-0.5 h-4 w-px bg-gray-200"></div>

    <div class="relative">
      <button
        class="surface-contrast-button ui-animated-button ui-animated-button--strong flex h-8 shrink-0 items-center gap-1 whitespace-nowrap rounded-lg bg-gray-900 px-2.5 text-[11px] font-bold text-white shadow-sm transition-all hover:bg-gray-800 active:scale-95 sm:h-9 sm:px-3 sm:text-xs"
        onclick={() => (workspaceMenuOpen = !workspaceMenuOpen)}
        title={ui.open}
        type="button"
      >
        <Plus size={14} />
        <span class="hidden xl:inline">{ui.open}</span>
        <ChevronDown size={12} class={workspaceMenuOpen ? "rotate-180" : ""} />
      </button>

      {#if workspaceMenuOpen}
        <div class="absolute right-0 top-10 z-[72] w-56 rounded-xl border border-gray-200 bg-white p-1 shadow-2xl">
          {#if isMobileLayout}
            <button
              class="ui-animated-button ui-animated-button--soft group flex w-full items-center gap-3 rounded-lg px-3 py-2 text-sm text-gray-700 transition-colors hover:bg-gray-50 disabled:cursor-not-allowed disabled:opacity-50"
              disabled={readOnly}
              onclick={() => {
                workspaceMenuOpen = false;
                void onCreateSession();
              }}
              type="button"
            >
              <MessageSquare size={16} class="text-gray-400 group-hover:text-amber-600" />
              <span>{ui.newThread}</span>
            </button>
            <div class="mx-2 my-1 h-px bg-gray-100"></div>
          {/if}
          <button class="ui-animated-button ui-animated-button--soft group flex w-full items-center gap-3 rounded-lg px-3 py-2 text-sm text-gray-700 transition-colors hover:bg-gray-50" onclick={() => { workspaceMenuOpen = false; onOpenTasksTab(); }} type="button">
            <History size={16} class="text-gray-400 group-hover:text-amber-600" />
            <span>{ui.tasks}</span>
          </button>
          <button class="ui-animated-button ui-animated-button--soft group flex w-full items-center gap-3 rounded-lg px-3 py-2 text-sm text-gray-700 transition-colors hover:bg-gray-50" onclick={() => { workspaceMenuOpen = false; onOpenGitTab(); }} type="button">
            <GitBranch size={16} class="text-gray-400 group-hover:text-amber-600" />
            <span>{ui.gitWorkspace}</span>
          </button>
          <button class="ui-animated-button ui-animated-button--soft group flex w-full items-center gap-3 rounded-lg px-3 py-2 text-sm text-gray-700 transition-colors hover:bg-gray-50" onclick={() => { workspaceMenuOpen = false; onOpenSettingsTab(); }} type="button">
            <Settings size={16} class="text-gray-400 group-hover:text-amber-600" />
            <span>{ui.settingsSkills}</span>
          </button>
          <button
            class="ui-animated-button ui-animated-button--soft group flex w-full items-center gap-3 rounded-lg px-3 py-2 text-sm text-gray-700 transition-colors hover:bg-gray-50 disabled:cursor-not-allowed disabled:opacity-50"
            disabled={readOnly}
            onclick={() => {
              workspaceMenuOpen = false;
              void onCreateTerminalTab();
            }}
            type="button"
          >
            <Terminal size={16} class="text-gray-400 group-hover:text-amber-600" />
            <span>{ui.newTerminal}</span>
          </button>
        </div>
      {/if}
    </div>
  </div>
</header>
