<script lang="ts">
  import {
    Activity,
    Archive,
    ArrowRightLeft,
    Brain,
    ChevronDown,
    GitBranch,
    History,
    Menu,
    MessageSquare,
    Monitor,
    Pencil,
    Pin,
    Plus,
    RotateCcw,
    Search,
    Settings,
    Terminal,
    User,
    UserCog
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
    moveSessionToAccount: string;
    newThread: string;
    tasks: string;
    gitWorkspace: string;
    settingsSkills: string;
    computer: string;
    diagnostics: string;
    memory: string;
    newTerminal: string;
  };

  let {
    isMobileLayout,
    titleDraft = $bindable(""),
    titleInputElement = $bindable(),
    running,
    selectedSessionSummary,
    profiles = [],
    readOnly,
    tokenCountLabel,
    contextUsage,
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
    onRequestMoveSessionProfile,
    onForkHandoff,
    onTogglePinned,
    onToggleArchive,
    onCreateSession,
    onOpenTasksTab,
    onOpenGitTab,
    onOpenComputerTab,
    onOpenDiagnosticsTab,
    onOpenMemoryTab,
    onOpenSettingsTab,
    onCreateTerminalTab
  }: {
    isMobileLayout: boolean;
    titleDraft: string;
    titleInputElement?: HTMLInputElement | undefined;
    running: boolean;
    selectedSessionSummary: SessionSummary | null;
    profiles?: Array<{
      id: string;
      label: string;
      codexHome: string;
      active: boolean;
    }>;
    readOnly: boolean;
    tokenCountLabel: string | null;
    contextUsage: {
      label: string;
      percent: number;
      tooltip: string;
    } | null;
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
    onRequestMoveSessionProfile: (session: SessionSummary) => void;
    onForkHandoff: () => void | Promise<void>;
    onTogglePinned: () => void | Promise<void>;
    onToggleArchive: () => void | Promise<void>;
    onCreateSession: () => void | Promise<void>;
    onOpenTasksTab: () => void;
    onOpenGitTab: () => void;
    onOpenComputerTab: () => void;
    onOpenDiagnosticsTab: () => void;
    onOpenMemoryTab: () => void;
    onOpenSettingsTab: () => void;
    onCreateTerminalTab: () => void | Promise<void>;
  } = $props();

  function selectedSessionAccountLabel() {
    const profileLabel = (selectedSessionSummary?.profileLabel ?? "").trim();
    if (profileLabel) {
      return profileLabel;
    }
    return (selectedSessionSummary?.accountEmail ?? "").trim();
  }

  function selectedSessionCanMoveProfile() {
    if (!selectedSessionSummary || readOnly || profiles.length <= 1) {
      return false;
    }
    const currentProfileId = selectedSessionSummary.profileId ?? profiles.find((profile) => profile.active)?.id ?? null;
    return profiles.some((profile) => profile.id !== currentProfileId);
  }
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
          {#if selectedSessionAccountLabel()}
            {#if selectedSessionCanMoveProfile()}
              <button
                class="session-title-account-badge ui-animated-button ui-animated-button--soft inline-flex max-w-[14rem] items-center gap-1 rounded-full border border-gray-200 bg-gray-50 px-2 py-0.5 text-[10px] font-semibold text-gray-500 transition-colors hover:border-amber-200 hover:bg-amber-50 hover:text-amber-700"
                title={`${ui.moveSessionToAccount}: ${selectedSessionSummary.profileCodexHome ?? selectedSessionAccountLabel()}`}
                type="button"
                onclick={() => {
                  if (selectedSessionSummary) {
                    onRequestMoveSessionProfile(selectedSessionSummary);
                  }
                }}
              >
                <UserCog size={10} class="shrink-0" />
                <span class="truncate">{selectedSessionAccountLabel()}</span>
              </button>
            {:else}
              <span
                class="session-title-account-badge inline-flex max-w-[14rem] items-center gap-1 rounded-full border border-gray-200 bg-gray-50 px-2 py-0.5 text-[10px] font-semibold text-gray-500"
                title={selectedSessionSummary.profileCodexHome ?? selectedSessionAccountLabel()}
              >
                <User size={10} class="shrink-0" />
                <span class="truncate">{selectedSessionAccountLabel()}</span>
              </span>
            {/if}
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
    {#if tokenCountLabel || contextUsage}
      <div class="mr-1.5 hidden items-center gap-1.5 lg:flex">
        {#if tokenCountLabel}
          <span class="rounded-md border border-gray-100 bg-gray-50 px-1.5 py-0.5 text-[9px] font-bold uppercase tracking-tight text-gray-500">
            {tokenCountLabel}
          </span>
        {/if}
        {#if contextUsage}
          <div class="group relative">
            <div
              aria-label={contextUsage.tooltip}
              class="inline-flex items-center gap-1.5 rounded-full border px-1.5 py-0.5 text-[9px] font-bold uppercase tracking-tight"
              style="border-color: var(--line); background: var(--panel-soft); color: var(--muted);"
              title={contextUsage.tooltip}
            >
              <span
                class="inline-flex h-4 w-4 items-center justify-center rounded-full p-[2px]"
                style={`background: conic-gradient(var(--accent) 0 ${contextUsage.percent}%, var(--line) ${contextUsage.percent}% 100%);`}
              >
                <span class="h-full w-full rounded-full" style="background: var(--panel-strong);"></span>
              </span>
              <span>{contextUsage.label}</span>
            </div>
            <div
              class="pointer-events-none absolute right-0 top-full z-[82] mt-2 hidden whitespace-nowrap rounded-xl border px-2.5 py-2 text-[11px] font-semibold shadow-2xl group-hover:block group-focus-within:block"
              style="border-color: var(--line); background: var(--panel-strong); color: var(--ink-strong);"
            >
              {contextUsage.tooltip}
            </div>
          </div>
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
        class="workspace-open-trigger surface-contrast-button ui-animated-button ui-animated-button--strong flex h-8 shrink-0 items-center gap-1 whitespace-nowrap rounded-lg px-2.5 text-[11px] font-bold shadow-sm transition-all active:scale-95 sm:h-9 sm:px-3 sm:text-xs"
        onclick={() => (workspaceMenuOpen = !workspaceMenuOpen)}
        title={ui.open}
        type="button"
      >
        <Plus size={14} />
        <span class="hidden xl:inline">{ui.open}</span>
        <ChevronDown size={12} class={workspaceMenuOpen ? "rotate-180" : ""} />
      </button>

      {#if workspaceMenuOpen}
        <div class="workspace-open-menu absolute right-0 top-10 z-[72] w-56 rounded-xl border p-1 shadow-2xl">
          {#if isMobileLayout}
            <button
              class="workspace-open-menu__item ui-animated-button ui-animated-button--soft group flex w-full items-center gap-3 rounded-lg px-3 py-2 text-sm transition-colors disabled:cursor-not-allowed disabled:opacity-50"
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
            <div class="workspace-open-menu__divider mx-2 my-1 h-px"></div>
          {/if}
          <button class="workspace-open-menu__item ui-animated-button ui-animated-button--soft group flex w-full items-center gap-3 rounded-lg px-3 py-2 text-sm transition-colors" onclick={() => { workspaceMenuOpen = false; onOpenTasksTab(); }} type="button">
            <History size={16} class="text-gray-400 group-hover:text-amber-600" />
            <span>{ui.tasks}</span>
          </button>
          <button class="workspace-open-menu__item ui-animated-button ui-animated-button--soft group flex w-full items-center gap-3 rounded-lg px-3 py-2 text-sm transition-colors" onclick={() => { workspaceMenuOpen = false; onOpenGitTab(); }} type="button">
            <GitBranch size={16} class="text-gray-400 group-hover:text-amber-600" />
            <span>{ui.gitWorkspace}</span>
          </button>
          <button class="workspace-open-menu__item ui-animated-button ui-animated-button--soft group flex w-full items-center gap-3 rounded-lg px-3 py-2 text-sm transition-colors" onclick={() => { workspaceMenuOpen = false; onOpenComputerTab(); }} type="button">
            <Monitor size={16} class="text-gray-400 group-hover:text-amber-600" />
            <span>{ui.computer}</span>
          </button>
          <button class="workspace-open-menu__item ui-animated-button ui-animated-button--soft group flex w-full items-center gap-3 rounded-lg px-3 py-2 text-sm transition-colors" onclick={() => { workspaceMenuOpen = false; onOpenDiagnosticsTab(); }} type="button">
            <Activity size={16} class="text-gray-400 group-hover:text-amber-600" />
            <span>{ui.diagnostics}</span>
          </button>
          <button class="workspace-open-menu__item ui-animated-button ui-animated-button--soft group flex w-full items-center gap-3 rounded-lg px-3 py-2 text-sm transition-colors" onclick={() => { workspaceMenuOpen = false; onOpenMemoryTab(); }} type="button">
            <Brain size={16} class="text-gray-400 group-hover:text-amber-600" />
            <span>{ui.memory}</span>
          </button>
          <button class="workspace-open-menu__item ui-animated-button ui-animated-button--soft group flex w-full items-center gap-3 rounded-lg px-3 py-2 text-sm transition-colors" onclick={() => { workspaceMenuOpen = false; onOpenSettingsTab(); }} type="button">
            <Settings size={16} class="text-gray-400 group-hover:text-amber-600" />
            <span>{ui.settingsSkills}</span>
          </button>
          <button
            class="workspace-open-menu__item ui-animated-button ui-animated-button--soft group flex w-full items-center gap-3 rounded-lg px-3 py-2 text-sm transition-colors disabled:cursor-not-allowed disabled:opacity-50"
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

<style>
  .workspace-open-trigger {
    border: 1px solid rgba(15, 23, 42, 0.08);
    background: linear-gradient(180deg, #111827, #1f2937);
    color: #fff;
    box-shadow: 0 16px 30px -22px rgba(15, 23, 42, 0.46);
  }

  .workspace-open-trigger:hover {
    background: linear-gradient(180deg, #1f2937, #334155);
  }

  .workspace-open-menu {
    border-color: var(--line);
    background: color-mix(in srgb, var(--panel-strong) 96%, transparent);
    color: var(--ink-strong);
    box-shadow: 0 24px 44px -28px rgba(15, 23, 42, 0.38);
  }

  .workspace-open-menu__item {
    color: var(--ink);
  }

  .workspace-open-menu__item:hover {
    background: var(--panel-soft);
    color: var(--ink-strong);
  }

  .workspace-open-menu__divider {
    background: var(--line);
  }

  :global(:root[data-theme="dark"]) .workspace-open-menu {
    background: color-mix(in srgb, var(--panel-strong) 96%, #020617 4%);
    box-shadow: 0 26px 48px -26px rgba(0, 0, 0, 0.62);
  }

  :global(:root[data-theme="dark"]) .workspace-open-trigger {
    border-color: rgba(148, 163, 184, 0.22);
    background: linear-gradient(180deg, rgba(51, 65, 85, 0.96), rgba(30, 41, 59, 0.98));
    color: #f8fafc;
    box-shadow: 0 18px 34px -26px rgba(2, 6, 23, 0.72);
  }

  :global(:root[data-theme="dark"]) .workspace-open-trigger:hover {
    background: linear-gradient(180deg, rgba(71, 85, 105, 0.98), rgba(51, 65, 85, 1));
  }

  :global(:root[data-theme="dark"]) .session-title-account-badge {
    border-color: rgba(71, 85, 105, 0.42);
    background: rgba(15, 23, 42, 0.74);
    color: #94a3b8;
  }

  :global(:root[data-theme="dark"]) button.session-title-account-badge:hover {
    border-color: rgba(245, 158, 11, 0.34);
    background: rgba(69, 39, 10, 0.36);
    color: #fbbf24;
  }
</style>
