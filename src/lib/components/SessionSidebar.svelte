<script lang="ts">
  import { onMount, tick } from "svelte";
  import { 
    Plus, 
    MessageSquare, 
    Search, 
    Archive, 
    RotateCcw,
    User, 
    Settings, 
    LogOut, 
    ChevronDown, 
    ChevronUp, 
    X,
    Cpu,
    Zap,
    History,
    RefreshCw,
    AlertCircle,
    CheckCircle2,
    ExternalLink,
    Monitor,
    Moon,
    Sun
  } from "lucide-svelte";

  import { activeLocale, localeOptions, localeSignal, updateLocale } from "$lib/i18n";
  import { m } from "$lib/paraglide/messages.js";
  import { getLocale } from "$lib/paraglide/runtime.js";
  import type { ResolvedTheme, ThemeMode } from "$lib/theme";
  import type { CodexAccountLoginFlow, CodexQuotaStatus, CodexRuntimeStatus, SessionSearchScope, SessionSummary } from "$lib/types";

  let {
    sessions,
    sessionHighlights,
    selectedId,
    sessionsBusy,
    sessionsHasMore,
    sessionsLoadingMore,
    sessionsLoadPercent,
    searchQuery,
    searchScope,
    showArchived,
    onSelect,
    onCreate,
    onLoadMore,
    onSearchQueryChange,
    onSearchScopeChange,
    onArchivedChange,
    onToggleArchive,
    account,
    quota,
    quotaBusy,
    runtime,
    runtimeBusyAction,
    systemShutdownArmed,
    systemShutdownAvailable,
    systemShutdownDelaySeconds,
    themeMode,
    resolvedTheme,
    accountLoginFlow,
    onRefreshQuota,
    onRefreshRuntime,
    onSystemShutdownArmedChange,
    onInstallRuntime,
    onUpdateRuntime,
    onThemeModeChange,
    onStartAccountLogin,
    onCancelAccountLogin,
    onLogoutAccount,
    onLogoutWeb,
    showCloseButton = false,
    onClose = () => {}
  }: {
    sessions: SessionSummary[];
    sessionHighlights: Record<string, { kind: "completed" | "attention"; at: number }>;
    selectedId: string | null;
    sessionsBusy: boolean;
    sessionsHasMore: boolean;
    sessionsLoadingMore: boolean;
    sessionsLoadPercent: number;
    searchQuery: string;
    searchScope: SessionSearchScope;
    showArchived: boolean;
    onSelect: (sessionId: string) => void;
    onCreate: () => void;
    onLoadMore: () => void;
    onSearchQueryChange: (query: string) => void;
    onSearchScopeChange: (scope: SessionSearchScope) => void;
    onArchivedChange: (nextValue: boolean) => void;
    onToggleArchive: (session: SessionSummary) => void;
    account:
      | {
          type: "apiKey" | "chatgpt" | null;
          email: string | null;
          planType: string | null;
          requiresOpenaiAuth: boolean;
        }
      | null;
    quota: CodexQuotaStatus | null;
    quotaBusy: boolean;
    runtime: CodexRuntimeStatus | null;
    runtimeBusyAction: "install" | "update" | "check" | null;
    systemShutdownArmed: boolean;
    systemShutdownAvailable: boolean;
    systemShutdownDelaySeconds: number;
    themeMode: ThemeMode;
    resolvedTheme: ResolvedTheme;
    accountLoginFlow: CodexAccountLoginFlow | null;
    onRefreshQuota: () => void;
    onRefreshRuntime: () => void;
    onSystemShutdownArmedChange: (armed: boolean) => void;
    onInstallRuntime: () => void;
    onUpdateRuntime: () => void;
    onThemeModeChange: (mode: ThemeMode) => void;
    onStartAccountLogin: (type: "chatgpt" | "chatgptDeviceCode") => void;
    onCancelAccountLogin: (loginId: string) => void;
    onLogoutAccount: () => void;
    onLogoutWeb: () => void;
    showCloseButton?: boolean;
    onClose?: () => void;
  } = $props();

  let accountMenuOpen = $state(false);
  let searchPanelOpen = $state(false);
  let listElement = $state<HTMLDivElement | undefined>(undefined);
  let searchTriggerElement = $state<HTMLButtonElement | undefined>(undefined);
  let searchPanelElement = $state<HTMLDivElement | undefined>(undefined);
  let searchInputElement = $state<HTMLInputElement | undefined>(undefined);
  let accountButtonElement = $state<HTMLButtonElement | undefined>(undefined);
  let accountPopoverElement = $state<HTMLDivElement | undefined>(undefined);
  let accountPopoverStyle = $state("");
  let boundedAutoloadPasses = $state(0);
  let loadMoreOrigin = $state<"manual" | "auto" | null>(null);

  const ui = $derived.by(() => {
    const _locale = $localeSignal;

    return {
      appShortName: m.app_short_name(),
      newThread: m.new_thread(),
      active: m.active(),
      archived: m.archived(),
      searchThreads: m.search_threads(),
      searchArchived: m.search_archived(),
      searchScopeSummary: m.search_scope_summary(),
      searchScopeFull: m.search_scope_full(),
      noArchivedSessions: m.no_archived_sessions(),
      archiveThread: m.archive_thread(),
      restoreThread: m.restore_thread(),
      noSessions: m.no_sessions(),
      noThreadsMatchingSearch: m.no_threads_matching_search(),
      createNewThreadPrompt: m.create_new_thread_prompt(),
      needsInput: m.needs_input(),
      done: m.done(),
      newCodexSession: m.new_codex_session(),
      loadingMoreThreads: m.loading_more_threads(),
      autoloadingMoreThreads: m.autoloading_more_threads(),
      loadMoreThreads: m.load_more_threads(),
      loadingNextSessionList: m.loading_next_session_list(),
      account: m.account(),
      appearance: m.appearance(),
      language: m.language(),
      quotaUsage: m.quota_usage(),
      quota5h: m.quota_5h(),
      quotaWeekly: m.quota_weekly(),
      runtime: m.runtime(),
      version: m.version(),
      binary: m.binary(),
      shutdownAfterQueueCompletes: m.shutdown_after_queue_completes(),
      shutdownWaitDescription: (seconds: number) => m.shutdown_wait_description({ seconds: String(seconds) }),
      missing: m.missing(),
      check: m.check(),
      update: m.update(),
      install: m.install(),
      close: m.close(),
      refreshQuota: m.refresh_quota(),
      switchAccount: m.switch_account(),
      signInAction: m.sign_in_action(),
      signOut: m.sign_out(),
      connected: m.connected(),
      signInRequired: m.sign_in_required(),
      localRuntime: m.local_runtime(),
      currentDarkMode: m.current_dark_mode(),
      currentLightMode: m.current_light_mode()
    };
  });

  function getDateLocale() {
    return getLocale() === "ko" ? "ko-KR" : "en-US";
  }

  function formatUpdated(value: number) {
    return new Intl.DateTimeFormat(getDateLocale(), {
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit"
    }).format(new Date(value * 1000));
  }

  function isSessionRunning(session: SessionSummary) {
    return session.status === "running" || session.status === "active";
  }

  function displaySessionTitle(session: SessionSummary) {
    const normalizedName = (session.name ?? "").trim();
    if (normalizedName && normalizedName !== "New thread" && normalizedName !== m.new_thread()) {
      return normalizedName;
    }

    const preview = session.preview.replace(/\s+/g, " ").trim();
    if (!preview) {
      return m.new_thread();
    }

    let candidate =
      preview.split(/\r?\n/u, 1)[0]?.split(/(?<=[.?!])\s+/u, 1)[0]?.split(/\s[-:|]\s/u, 1)[0]?.trim() ?? preview;

    candidate = candidate
      .replace(/^[#>*`\-.\d()\[\]\s]+/u, "")
      .replace(/\s+/g, " ")
      .replace(
        /(해줘|해주세요|해 줘|고쳐줘|고쳐 줘|수정해줘|수정해 줘|추가해줘|추가해 줘|구현해줘|구현해 줘|만들어줘|만들어 줘|계속 작업해|계속 진행해|계속해|부탁해|please|can you|could you|help me)\s*$/iu,
        ""
      )
      .replace(/[.?!…。]+$/u, "")
      .trim();

    if (!candidate) {
      return preview.length > 60 ? `${preview.slice(0, 60).trimEnd()}...` : preview;
    }

    return candidate.length > 60 ? `${candidate.slice(0, 60).trimEnd()}...` : candidate;
  }

  function formatQuotaPercent(value: number | null | undefined) {
    if (typeof value !== "number" || !Number.isFinite(value)) {
      return "—";
    }
    return `${Math.max(0, Math.min(100, Math.round(value)))}%`;
  }

  function formatQuotaReset(value: number | null | undefined) {
    if (typeof value !== "number" || !Number.isFinite(value) || value <= 0) {
      return m.reset_unknown();
    }

    const formatted = new Intl.DateTimeFormat(getDateLocale(), {
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit"
    }).format(new Date(value));

    return m.reset_at({ value: formatted });
  }

  function compactQuotaLabel() {
    const candidates = [quota?.fiveHour?.remainingPercent, quota?.weekly?.remainingPercent].filter(
      (value): value is number => typeof value === "number" && Number.isFinite(value)
    );
    if (candidates.length === 0) {
      return null;
    }
    return m.quota_left_compact({ percent: String(Math.round(Math.min(...candidates))) });
  }

  function accountLabel() {
    if (account?.email) {
      return account.email;
    }
    if (account?.type === "apiKey") {
      return m.api_key();
    }
    return m.codex_account();
  }

  function accountSubLabel() {
    if (account?.planType) {
      return account.planType;
    }
    if (account?.requiresOpenaiAuth) {
      return account?.email || account?.type ? ui.connected : ui.signInRequired;
    }
    return ui.localRuntime;
  }

  const themeOptions: Array<{ mode: ThemeMode; key: "system" | "light" | "dark"; icon: typeof Monitor }> = [
    { mode: "system", key: "system", icon: Monitor },
    { mode: "light", key: "light", icon: Sun },
    { mode: "dark", key: "dark", icon: Moon }
  ];

  function getThemeOptionLabel(mode: ThemeMode) {
    if (mode === "light") {
      return m.light();
    }
    if (mode === "dark") {
      return m.dark();
    }
    return m.system();
  }

  function getResolvedThemeLabel() {
    return resolvedTheme === "dark" ? ui.currentDarkMode : ui.currentLightMode;
  }

  function handleListScroll() {
    if (!listElement || !sessionsHasMore || sessionsLoadingMore) {
      return;
    }

    const remaining = listElement.scrollHeight - listElement.scrollTop - listElement.clientHeight;
    if (remaining <= 160) {
      loadMoreOrigin = "auto";
      onLoadMore();
    }
  }

  function loadMoreByButton() {
    if (!sessionsHasMore || sessionsLoadingMore) {
      return;
    }
    loadMoreOrigin = "manual";
    onLoadMore();
  }

  function toggleSearchPanel() {
    searchPanelOpen = !searchPanelOpen;
  }

  function closeSearchPanel() {
    searchPanelOpen = false;
  }

  function clearSearchQuery() {
    onSearchQueryChange("");
  }

  function searchTriggerLabel() {
    const query = searchQuery.trim();
    if (query) {
      return query;
    }
    return showArchived ? ui.searchArchived : ui.searchThreads;
  }

  function searchTriggerSubLabel() {
    if (searchScope === "full") {
      return ui.searchScopeFull;
    }
    if (searchQuery.trim()) {
      return ui.searchScopeSummary;
    }
    return null;
  }

  $effect(() => {
    searchQuery;
    searchScope;
    showArchived;
    boundedAutoloadPasses = 0;
    loadMoreOrigin = null;
  });

  $effect(() => {
    if (!searchPanelOpen) {
      return;
    }
    void tick().then(() => {
      searchInputElement?.focus();
      searchInputElement?.select();
    });
  });

  $effect(() => {
    if (!listElement || !sessionsHasMore || sessionsLoadingMore) {
      return;
    }
    if (boundedAutoloadPasses >= 2) {
      return;
    }
    if (sessions.length >= 12) {
      return;
    }
    if (listElement.scrollHeight > listElement.clientHeight + 24) {
      return;
    }

    boundedAutoloadPasses += 1;
    loadMoreOrigin = "auto";
    onLoadMore();
  });

  $effect(() => {
    if (!sessionsLoadingMore) {
      loadMoreOrigin = null;
    }
  });

  async function updateAccountPopoverPosition() {
    if (!accountMenuOpen || !accountButtonElement || !accountPopoverElement || typeof window === "undefined") {
      return;
    }

    await tick();
    const margin = 12;
    const triggerRect = accountButtonElement.getBoundingClientRect();
    const popoverRect = accountPopoverElement.getBoundingClientRect();
    const width = Math.min(Math.max(popoverRect.width || 320, 320), window.innerWidth - margin * 2);
    let left = triggerRect.left;
    if (left + width > window.innerWidth - margin) {
      left = window.innerWidth - width - margin;
    }
    if (left < margin) {
      left = margin;
    }

    let top = triggerRect.top - popoverRect.height - 12;
    if (top < margin) {
      top = triggerRect.bottom + 12;
    }
    if (top + popoverRect.height > window.innerHeight - margin) {
      top = Math.max(margin, window.innerHeight - popoverRect.height - margin);
    }

    accountPopoverStyle = `top:${Math.round(top)}px;left:${Math.round(left)}px;width:${Math.round(width)}px;max-height:${Math.max(240, window.innerHeight - margin * 2)}px;opacity:1;pointer-events:auto;`;
  }

  $effect(() => {
    if (!accountMenuOpen) {
      accountPopoverStyle = "";
      return;
    }
    void updateAccountPopoverPosition();
  });

  onMount(() => {
    const update = () => {
      if (accountMenuOpen) {
        void updateAccountPopoverPosition();
      }
    };

    const handlePointerDown = (event: MouseEvent) => {
      const target = event.target as Node | null;
      if (
        searchPanelOpen &&
        target &&
        !searchPanelElement?.contains(target) &&
        !searchTriggerElement?.contains(target)
      ) {
        closeSearchPanel();
      }
      if (
        accountMenuOpen &&
        target &&
        !accountPopoverElement?.contains(target) &&
        !accountButtonElement?.contains(target)
      ) {
        accountMenuOpen = false;
      }
    };

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        if (searchPanelOpen) {
          closeSearchPanel();
        }
        if (accountMenuOpen) {
          accountMenuOpen = false;
        }
      }
    };

    window.addEventListener("resize", update);
    window.addEventListener("scroll", update, true);
    window.addEventListener("mousedown", handlePointerDown);
    window.addEventListener("keydown", handleKeyDown);

    return () => {
      window.removeEventListener("resize", update);
      window.removeEventListener("scroll", update, true);
      window.removeEventListener("mousedown", handlePointerDown);
      window.removeEventListener("keydown", handleKeyDown);
    };
  });
</script>

<aside class="sidebar flex flex-col h-full bg-gray-50/80 border-r border-gray-200/50 w-72 min-w-[18rem] transition-all">
  <div class="p-4 flex flex-col gap-4">
    <div class="flex items-center justify-between">
      <div class="flex items-center gap-2 px-1">
        <div class="w-8 h-8 bg-amber-600 rounded-lg flex items-center justify-center text-white font-bold">C</div>
        <h1 class="text-lg font-semibold tracking-tight text-gray-900">{ui.appShortName}</h1>
      </div>
      {#if showCloseButton}
        <button class="p-2 text-gray-400 hover:text-gray-600 hover:bg-gray-100 rounded-lg transition-colors" onclick={onClose}>
          <X size={20} />
        </button>
      {/if}
    </div>

    <button 
      class="flex items-center gap-2 w-full px-4 py-3 bg-white border border-gray-200 rounded-xl shadow-sm hover:border-amber-500/50 hover:shadow-md transition-all group" 
      onclick={onCreate}
    >
      <div class="w-6 h-6 rounded-md bg-amber-50 text-amber-600 flex items-center justify-center group-hover:bg-amber-100 transition-colors">
        <Plus size={18} />
      </div>
      <span class="font-medium text-gray-700">{ui.newThread}</span>
    </button>
  </div>

  <div class="px-4 pb-2 space-y-4">
    <div class="flex p-1 bg-gray-200/50 rounded-lg text-sm">
      <button
        class="flex-1 py-1.5 rounded-md transition-all { !showArchived ? 'bg-white shadow-sm text-gray-900 font-medium' : 'text-gray-500 hover:text-gray-700' }"
        onclick={() => onArchivedChange(false)}
      >
        {ui.active}
      </button>
      <button
        class="flex-1 py-1.5 rounded-md transition-all { showArchived ? 'bg-white shadow-sm text-gray-900 font-medium' : 'text-gray-500 hover:text-gray-700' }"
        onclick={() => onArchivedChange(true)}
      >
        {ui.archived}
      </button>
    </div>

    <div class="relative">
      <button
        bind:this={searchTriggerElement}
        aria-expanded={searchPanelOpen}
        class={`flex w-full items-center gap-3 rounded-xl border px-3 py-2.5 text-left transition-all ${
          searchPanelOpen
            ? "border-amber-300 bg-white shadow-lg shadow-amber-100/40"
            : searchQuery.trim() || searchScope === "full"
              ? "border-amber-200/80 bg-amber-50/70 shadow-sm"
              : "border-gray-200 bg-white/70 hover:border-gray-300 hover:bg-white hover:shadow-sm"
        }`}
        onclick={toggleSearchPanel}
        type="button"
      >
        <div class={`flex h-8 w-8 shrink-0 items-center justify-center rounded-lg border ${
          searchPanelOpen || searchQuery.trim() || searchScope === "full"
            ? "border-amber-200 bg-amber-50 text-amber-700"
            : "border-gray-200 bg-gray-50 text-gray-400"
        }`}>
          <Search size={15} />
        </div>
        <div class="min-w-0 flex-1">
          <p class={`truncate text-sm font-medium ${searchQuery.trim() ? "text-gray-900" : "text-gray-500"}`}>
            {searchTriggerLabel()}
          </p>
          {#if searchTriggerSubLabel()}
            <p class="mt-0.5 truncate text-[11px] font-medium text-gray-400">{searchTriggerSubLabel()}</p>
          {/if}
        </div>
        <ChevronDown size={15} class={`shrink-0 text-gray-400 transition-transform ${searchPanelOpen ? "rotate-180" : ""}`} />
      </button>

      {#if searchPanelOpen}
        <div
          bind:this={searchPanelElement}
          class="absolute left-0 right-0 top-[calc(100%+0.55rem)] z-20 overflow-hidden rounded-2xl border border-amber-200/80 bg-white shadow-[0_20px_48px_-28px_rgba(217,119,6,0.45)]"
        >
          <div class="space-y-3 p-3.5">
            <div class="relative group">
              <div class="absolute inset-y-0 left-3 flex items-center pointer-events-none text-gray-400 transition-colors group-focus-within:text-amber-600">
                <Search size={16} />
              </div>
              <input
                bind:this={searchInputElement}
                class="w-full rounded-xl border border-gray-200 bg-gray-50/80 py-2.5 pl-10 pr-10 text-sm text-gray-700 placeholder-gray-400 transition-all focus:border-amber-400 focus:bg-white focus:outline-none focus:ring-2 focus:ring-amber-500/10"
                oninput={(event) => onSearchQueryChange((event.currentTarget as HTMLInputElement).value)}
                onkeydown={(event) => {
                  if (event.key === "Escape") {
                    event.preventDefault();
                    closeSearchPanel();
                  }
                }}
                placeholder={showArchived ? ui.searchArchived : ui.searchThreads}
                type="search"
                value={searchQuery}
              />
              {#if searchQuery.trim()}
                <button
                  class="absolute inset-y-0 right-2 my-auto flex h-7 w-7 items-center justify-center rounded-lg text-gray-400 transition-colors hover:bg-gray-100 hover:text-gray-600"
                  onclick={clearSearchQuery}
                  title={ui.close}
                  type="button"
                >
                  <X size={14} />
                </button>
              {/if}
            </div>

            <div class="flex gap-1 rounded-xl bg-gray-100 p-1">
              <button
                class={`flex-1 rounded-lg px-2 py-1.5 text-[11px] font-semibold transition-all ${
                  searchScope === "summary" ? "bg-white text-gray-900 shadow-sm" : "text-gray-500 hover:text-gray-700"
                }`}
                onclick={() => onSearchScopeChange("summary")}
                type="button"
              >
                {ui.searchScopeSummary}
              </button>
              <button
                class={`flex-1 rounded-lg px-2 py-1.5 text-[11px] font-semibold transition-all ${
                  searchScope === "full" ? "bg-white text-gray-900 shadow-sm" : "text-gray-500 hover:text-gray-700"
                }`}
                onclick={() => onSearchScopeChange("full")}
                type="button"
              >
                {ui.searchScopeFull}
              </button>
            </div>
          </div>
        </div>
      {/if}
    </div>
  </div>

  <div class="flex-1 overflow-hidden relative flex flex-col">
    {#if sessionsBusy || sessionsLoadingMore}
      <div class="absolute top-0 left-0 right-0 z-10">
        <div class="h-0.5 w-full bg-gray-100 overflow-hidden">
          <div 
            class="h-full bg-amber-500 transition-all duration-300 ease-out" 
            style={`width:${Math.max(8, Math.min(100, sessionsLoadPercent))}%`}
          ></div>
        </div>
      </div>
    {/if}

    <div 
      bind:this={listElement} 
      class="flex-1 overflow-y-auto px-4 py-2 space-y-1 scrollbar-thin"
      onscroll={handleListScroll}
    >
      {#if sessions.length === 0}
        <div class="py-12 px-4 text-center">
          <div class="w-12 h-12 bg-gray-100 rounded-full flex items-center justify-center mx-auto mb-3 text-gray-400">
            {#if showArchived}
              <Archive size={24} />
            {:else}
              <MessageSquare size={24} />
            {/if}
          </div>
          <p class="text-sm font-medium text-gray-600">
            {showArchived ? ui.noArchivedSessions : ui.noSessions}
          </p>
          <p class="text-xs text-gray-400 mt-1">
            {searchQuery ? ui.noThreadsMatchingSearch : ui.createNewThreadPrompt}
          </p>
        </div>
      {/if}

      {#each sessions as session (session.id)}
        <div class="group relative">
          <button
            class="w-full text-left p-3 pr-11 rounded-xl transition-all relative { session.id === selectedId ? 'bg-white shadow-sm border border-gray-200 ring-1 ring-gray-200/50' : sessionHighlights[session.id]?.kind === 'attention' ? 'bg-amber-50 border border-amber-200/80 ring-1 ring-amber-200/60' : sessionHighlights[session.id]?.kind === 'completed' ? 'bg-emerald-50 border border-emerald-200/80 ring-1 ring-emerald-200/60' : 'hover:bg-gray-200/50 border border-transparent' }"
            onclick={() => onSelect(session.id)}
            type="button"
          >
            <div class="flex flex-col gap-1.5">
              <div class="flex items-start justify-between gap-2">
                <span class="min-w-0 flex-1 truncate text-sm font-medium text-gray-900 transition-colors group-hover:text-amber-700">
                  {displaySessionTitle(session)}
                </span>
                <div class="flex shrink-0 flex-nowrap items-center gap-1.5 whitespace-nowrap">
                  {#if session.queueCount > 0}
                    <span class="rounded-full bg-slate-100 px-1.5 py-0.5 text-[10px] font-semibold tabular-nums text-slate-600">
                      Q {session.queueCount}
                    </span>
                  {/if}
                  {#if sessionHighlights[session.id]?.kind === "attention"}
                    <span class="text-[9px] px-1.5 py-0.5 rounded-full bg-amber-100 text-amber-700 font-bold uppercase tracking-widest">{ui.needsInput}</span>
                  {:else if sessionHighlights[session.id]?.kind === "completed"}
                    <span class="text-[9px] px-1.5 py-0.5 rounded-full bg-emerald-100 text-emerald-700 font-bold uppercase tracking-widest">{ui.done}</span>
                  {/if}
                  {#if isSessionRunning(session)}
                    <span class="flex-shrink-0 w-2 h-2 rounded-full bg-amber-500 shadow-[0_0_8px_rgba(245,158,11,0.5)] animate-pulse mt-1.5"></span>
                  {/if}
                </div>
              </div>
              
              <p class="text-xs text-gray-500 line-clamp-1 break-all">
                {session.preview || ui.newCodexSession}
              </p>
              
              <div class="flex items-center justify-between gap-2 mt-1">
                <span class="text-[10px] text-gray-400 font-medium tracking-tight">
                  {formatUpdated(session.updatedAt)}
                </span>
                {#if session.agentNickname}
                  <span class="text-[10px] px-1.5 py-0.5 bg-gray-100 text-gray-500 rounded font-semibold uppercase tracking-wider">
                    {session.agentNickname}
                  </span>
                {/if}
              </div>
            </div>
          </button>
          <button
            aria-label={showArchived ? ui.restoreThread : ui.archiveThread}
            class="absolute right-2 top-2 z-10 rounded-lg border border-gray-200 bg-white/95 p-1.5 text-gray-400 opacity-0 shadow-sm transition-all pointer-events-none group-hover:pointer-events-auto group-hover:opacity-100 group-focus-within:pointer-events-auto group-focus-within:opacity-100 hover:border-amber-200 hover:bg-amber-50 hover:text-amber-700"
            onclick={(event) => {
              event.stopPropagation();
              onToggleArchive(session);
            }}
            title={showArchived ? ui.restoreThread : ui.archiveThread}
            type="button"
          >
            {#if showArchived}
              <RotateCcw size={14} />
            {:else}
              <Archive size={14} />
            {/if}
          </button>
        </div>
      {/each}

      {#if sessionsLoadingMore && loadMoreOrigin === "auto"}
        <div class="px-1 py-3">
          <div class="rounded-xl border border-amber-200/70 bg-amber-50/70 px-3 py-2.5 shadow-sm">
            <div class="flex items-center gap-2 text-xs font-semibold text-amber-800">
              <RefreshCw size={12} class="animate-spin" />
              <span>{ui.loadingMoreThreads}</span>
            </div>
            <p class="mt-1 text-[11px] text-amber-700/80">{ui.autoloadingMoreThreads}</p>
          </div>
        </div>
      {/if}

      {#if sessionsHasMore || (sessionsLoadingMore && loadMoreOrigin === "manual")}
        <div class="py-4">
          <button
            class="flex w-full items-center justify-center gap-2 rounded-xl border border-gray-200 bg-white px-3 py-2 text-xs font-semibold text-gray-500 shadow-sm transition-all hover:border-gray-300 hover:text-gray-700 disabled:cursor-wait disabled:border-amber-200 disabled:bg-amber-50/70 disabled:text-amber-700"
            disabled={sessionsLoadingMore}
            onclick={loadMoreByButton}
            type="button"
          >
            {#if sessionsLoadingMore && loadMoreOrigin === "manual"}
              <RefreshCw size={13} class="animate-spin" />
              <span>{ui.loadingMoreThreads}</span>
            {:else}
              <ChevronDown size={13} />
              <span>{ui.loadMoreThreads}</span>
            {/if}
          </button>
          {#if sessionsLoadingMore && loadMoreOrigin === "manual"}
            <p class="mt-2 text-center text-[11px] text-amber-700/80">{ui.loadingNextSessionList}</p>
          {/if}
        </div>
      {/if}
    </div>
  </div>

  <div class="p-4 border-t border-gray-200/50 bg-gray-50/50">
    <button
      aria-expanded={accountMenuOpen}
      bind:this={accountButtonElement}
      class="flex items-center gap-3 w-full p-2 rounded-xl hover:bg-gray-200/50 transition-all text-left group"
      onclick={() => (accountMenuOpen = !accountMenuOpen)}
    >
      <div class="w-10 h-10 rounded-full bg-white border border-gray-200 flex items-center justify-center text-gray-500 group-hover:border-amber-500/30 group-hover:text-amber-600 transition-all shadow-sm">
        <User size={20} />
      </div>
      <div class="flex-1 min-w-0">
        <p class="text-sm font-semibold text-gray-900 truncate leading-none mb-1">{accountLabel()}</p>
        <div class="flex items-center gap-1.5">
          <span class="text-[10px] font-medium text-gray-500 uppercase tracking-wider">{accountSubLabel()}</span>
          {#if compactQuotaLabel()}
            <span class="w-1 h-1 rounded-full bg-gray-300"></span>
            <span class="text-[10px] font-bold text-amber-600">{compactQuotaLabel()}</span>
          {/if}
        </div>
      </div>
      <div class="text-gray-400 group-hover:text-gray-600 transition-colors">
        {#if accountMenuOpen}
          <ChevronDown size={16} />
        {:else}
          <ChevronUp size={16} />
        {/if}
      </div>
    </button>

    {#if accountMenuOpen}
      <div 
        bind:this={accountPopoverElement} 
        class="fixed z-50 bg-white border border-gray-200 rounded-2xl shadow-2xl overflow-hidden opacity-0 pointer-events-none p-2 w-80" 
        style={accountPopoverStyle}
      >
        <div class="p-4 border-b border-gray-100 flex items-center justify-between">
          <div>
            <h3 class="text-xs font-bold text-gray-400 uppercase tracking-widest mb-1">{ui.account}</h3>
            <p class="text-sm font-semibold text-gray-900">{accountLabel()}</p>
          </div>
          <button class="p-1.5 text-gray-400 hover:text-gray-600 hover:bg-gray-100 rounded-lg transition-colors" onclick={() => (accountMenuOpen = false)}>
            <X size={16} />
          </button>
        </div>

        <div class="p-4 space-y-6">
          <div class="space-y-4">
            <h4 class="text-[10px] font-bold text-gray-400 uppercase tracking-widest flex items-center gap-1.5">
              <Monitor size={10} /> {ui.appearance}
            </h4>

            <div class="grid grid-cols-3 gap-2">
              {#each themeOptions as option (option.mode)}
                <button
                  class={`flex flex-col items-center gap-1 rounded-xl border px-2.5 py-2 text-[10px] font-bold transition-all ${
                    themeMode === option.mode
                      ? "border-amber-300 bg-amber-50 text-amber-700 shadow-sm"
                      : "border-gray-200 bg-gray-50 text-gray-500 hover:border-gray-300 hover:bg-white hover:text-gray-700"
                  }`}
                  onclick={() => onThemeModeChange(option.mode)}
                  type="button"
                >
                  <option.icon size={14} />
                  <span>{getThemeOptionLabel(option.mode)}</span>
                </button>
              {/each}
            </div>

            <p class="text-[10px] text-gray-400 italic">{getResolvedThemeLabel()}</p>
          </div>

          <div class="space-y-4">
            <h4 class="text-[10px] font-bold text-gray-400 uppercase tracking-widest flex items-center gap-1.5">
              <Settings size={10} /> {ui.language}
            </h4>

            <div class="relative">
              <select
                aria-label={ui.language}
                class="w-full appearance-none rounded-xl border border-gray-200 bg-gray-50 px-3 py-2.5 pr-9 text-sm font-semibold text-gray-700 shadow-sm outline-none transition focus:border-amber-400 focus:bg-white focus:ring-4 focus:ring-amber-100"
                onchange={(event) =>
                  updateLocale((event.currentTarget as HTMLSelectElement).value as (typeof localeOptions)[number]["value"])}
                value={$activeLocale}
              >
                {#each localeOptions as option (option.value)}
                  <option value={option.value}>{option.label}</option>
                {/each}
              </select>
              <div class="pointer-events-none absolute inset-y-0 right-3 flex items-center text-gray-400">
                <ChevronDown size={16} />
              </div>
            </div>
          </div>

          <div class="space-y-4">
            <h4 class="text-[10px] font-bold text-gray-400 uppercase tracking-widest flex items-center gap-1.5">
              <Zap size={10} /> {ui.quotaUsage}
            </h4>
            
            <div class="space-y-4">
              <div class="space-y-1.5">
                <div class="flex justify-between text-xs">
                  <span class="font-medium text-gray-600">{ui.quota5h}</span>
                  <span class="font-bold text-gray-900">{formatQuotaPercent(quota?.fiveHour?.remainingPercent)}</span>
                </div>
                <div class="h-1.5 w-full bg-gray-100 rounded-full overflow-hidden">
                  <div class="h-full bg-amber-500 rounded-full" style={`width: ${Math.max(0, Math.min(100, quota?.fiveHour?.remainingPercent ?? 0))}%`}></div>
                </div>
                <p class="text-[10px] text-gray-400 italic">Reset {formatQuotaReset(quota?.fiveHour?.resetAt)}</p>
              </div>

              <div class="space-y-1.5">
                <div class="flex justify-between text-xs">
                  <span class="font-medium text-gray-600">{ui.quotaWeekly}</span>
                  <span class="font-bold text-gray-900">{formatQuotaPercent(quota?.weekly?.remainingPercent)}</span>
                </div>
                <div class="h-1.5 w-full bg-gray-100 rounded-full overflow-hidden">
                  <div class="h-full bg-amber-500 rounded-full" style={`width: ${Math.max(0, Math.min(100, quota?.weekly?.remainingPercent ?? 0))}%`}></div>
                </div>
                <p class="text-[10px] text-gray-400 italic">Reset {formatQuotaReset(quota?.weekly?.resetAt)}</p>
              </div>
            </div>
          </div>

          <div class="space-y-4">
            <h4 class="text-[10px] font-bold text-gray-400 uppercase tracking-widest flex items-center gap-1.5">
              <Cpu size={10} /> {ui.runtime}
            </h4>
            <div class="bg-gray-50 rounded-xl p-3 border border-gray-100 space-y-3">
              <div class="flex justify-between text-xs">
                <span class="text-gray-500">{ui.version}</span>
                <span class="font-semibold text-gray-700">{runtime?.version ?? ui.missing}</span>
              </div>
              <div class="flex justify-between text-xs">
                <span class="text-gray-500">{ui.binary}</span>
                <code class="px-1.5 py-0.5 bg-white border border-gray-200 rounded text-[10px]">{runtime?.configuredBin ?? "codex"}</code>
              </div>
              <label class:checkbox-card--disabled={!systemShutdownAvailable} class="checkbox-card checkbox-card--compact" for="global-shutdown-after-queue">
                <input
                  class="checkbox-input"
                  checked={systemShutdownArmed}
                  disabled={!systemShutdownAvailable}
                  id="global-shutdown-after-queue"
                  onchange={(event) => onSystemShutdownArmedChange((event.currentTarget as HTMLInputElement).checked)}
                  type="checkbox"
                />
                <span aria-hidden="true" class="checkbox-control"></span>
                <span class="checkbox-copy">
                  <span class="checkbox-title">{ui.shutdownAfterQueueCompletes}</span>
                  <span class="checkbox-description">{ui.shutdownWaitDescription(systemShutdownDelaySeconds)}</span>
                </span>
              </label>

              <div class="flex gap-2 pt-1">
                {#if !runtime?.installed}
                  <button
                    class="flex-1 px-3 py-1.5 bg-amber-600 rounded-lg text-[10px] font-bold text-white hover:bg-amber-700 transition-all flex items-center justify-center gap-1.5 shadow-sm"
                    disabled={!runtime?.npmAvailable || runtimeBusyAction === "install"}
                    onclick={onInstallRuntime}
                  >
                    <Plus size={10} />
                    {ui.install}
                  </button>
                {:else if runtime?.updateAvailable === true}
                  <button 
                    class="flex-1 px-3 py-1.5 bg-amber-600 rounded-lg text-[10px] font-bold text-white hover:bg-amber-700 transition-all flex items-center justify-center gap-1.5 shadow-sm"
                    disabled={!runtime?.npmAvailable || runtimeBusyAction === "update"}
                    onclick={onUpdateRuntime}
                  >
                    <Zap size={10} />
                    {ui.update}
                  </button>
                {:else}
                  <button 
                    class="flex-1 px-3 py-1.5 bg-white border border-gray-200 rounded-lg text-[10px] font-bold text-gray-600 hover:bg-gray-50 hover:border-gray-300 transition-all flex items-center justify-center gap-1.5"
                    disabled={runtimeBusyAction === "check"}
                    onclick={onRefreshRuntime}
                  >
                    <RefreshCw size={10} class={runtimeBusyAction === "check" ? 'animate-spin' : ''} />
                    {ui.check}
                  </button>
                {/if}
              </div>
            </div>
          </div>
        </div>

        <div class="p-2 border-t border-gray-100 bg-gray-50/50 space-y-1">
          <button 
            class="w-full flex items-center gap-2 px-3 py-2 text-xs font-medium text-gray-600 hover:bg-white hover:text-amber-600 rounded-lg transition-all"
            disabled={quotaBusy}
            onclick={onRefreshQuota}
          >
            <RefreshCw size={14} class={quotaBusy ? 'animate-spin' : ''} />
            {ui.refreshQuota}
          </button>
          <button 
            class="w-full flex items-center gap-2 px-3 py-2 text-xs font-medium text-gray-600 hover:bg-white hover:text-amber-600 rounded-lg transition-all"
            onclick={() => onStartAccountLogin("chatgpt")}
          >
            <ExternalLink size={14} />
            {account?.email || account?.type ? ui.switchAccount : ui.signInAction}
          </button>
          <div class="h-px bg-gray-200/50 my-1 mx-2"></div>
          <button 
            class="w-full flex items-center gap-2 px-3 py-2 text-xs font-medium text-red-600 hover:bg-red-50 rounded-lg transition-all"
            onclick={onLogoutWeb}
          >
            <LogOut size={14} />
            {ui.signOut}
          </button>
        </div>
      </div>
    {/if}
  </div>
</aside>

<style>
  :global(.scrollbar-thin::-webkit-scrollbar) {
    width: 4px;
  }
  :global(.scrollbar-thin::-webkit-scrollbar-track) {
    background: transparent;
  }
  :global(.scrollbar-thin::-webkit-scrollbar-thumb) {
    background: var(--scrollbar-thumb);
    border-radius: 20px;
  }
  :global(.scrollbar-thin:hover::-webkit-scrollbar-thumb) {
    background: var(--scrollbar-thumb-hover);
  }
</style>
