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
    Ticket,
    History,
    RefreshCw,
    AlertCircle,
    CheckCircle2,
    Bell,
	    ExternalLink,
	    Folder,
	    FolderOpen,
	    Monitor,
	    Pin,
    Moon,
    Sun,
    Power,
    Download,
    UserCog,
    Pencil,
    Trash2
  } from "lucide-svelte";

  import { describeUiError } from "$lib/ui-errors";
  import { portal } from "$lib/actions/portal";
  import { activeLocale, localeOptions, localeSignal, updateLocale } from "$lib/i18n";
  import { m } from "$lib/paraglide/messages.js";
  import { getLocale } from "$lib/paraglide/runtime.js";
  import { anchoredPopoverStyle } from "$lib/popover-position";
  import type { ResolvedTheme, ThemeMode } from "$lib/theme";
  import type {
    AppNotification,
    CodexAccountLoginFlow,
    CodexProfileAccountSummary,
    CodexQuotaLimit,
    CodexQuotaStatus,
    CodexQuotaWindow,
    CodexResetTicket,
    CodexResetTicketsPayload,
    CodexRuntimeStatus,
	    SavedSessionFilter,
	    SessionFolder,
	    SessionSearchScope,
    UserRole,
    SessionSummary,
    SessionSummaryFilter
  } from "$lib/types";

  let {
    sessions,
    notifications,
    notificationsBusy,
    notificationsUnreadCount,
    sessionHighlights,
    selectedId,
    sessionsBusy,
    sessionsHasMore,
    sessionsLoadingMore,
    sessionsLoadPercent,
    searchQuery,
    searchScope,
    sessionFilter,
    savedSessionFilters,
    knownSessionTags,
    sessionFolders,
    activeSessionFolder,
    activeSavedSessionFilterId,
    showArchived,
    onSelect,
    onCreate,
    onLoadMore,
    onSearchQueryChange,
    onSearchScopeChange,
    onSessionFilterChange,
    onSelectSessionFolder,
    onSelectUnfiledSessions,
    onCreateSessionFolder,
    onToggleSessionFolderPin,
    onAddSelectedSessionToFolder,
    onRemoveSelectedSessionFromFolder,
    onApplySavedFilter,
    onSaveCurrentFilter,
    onDeleteSavedFilter,
    onArchivedChange,
    onTogglePin,
    onToggleArchive,
    onRequestMoveSessionProfile,
    profiles = [],
    profileAccounts = [],
    profileAccountsBusy = false,
    account,
    webRole = "admin",
    readOnly = false,
    quota,
    quotaBusy,
    resetTickets = null,
    resetTicketsBusy = false,
    resetTicketUseBusyId = null,
    runtime,
    runtimeBusyAction,
    gatewayRestartAvailable = true,
    gatewayRestartBusy = false,
    showPwaInstall = false,
    pwaInstalled = false,
    pwaInstallBusy = false,
    defaultLanguageBridgeEnabled = false,
    defaultLanguageBridgeBusy = false,
    systemShutdownArmed,
    systemShutdownAvailable,
    systemShutdownDelaySeconds,
    themeMode,
    resolvedTheme,
    accountLoginFlow,
    onRefreshQuota,
    onRefreshProfileAccounts,
    onRefreshResetTickets,
    onUseResetTicket,
    onRefreshNotifications,
    onMarkNotificationsRead,
    onClearNotifications,
    onRefreshRuntime,
    onInstallApp,
    onDefaultLanguageBridgeChange,
    onSystemShutdownArmedChange,
    onInstallRuntime,
    onUpdateRuntime,
    onRestartGateway,
    onThemeModeChange,
    onStartAccountLogin,
    onImportAccountCredentials,
    onSelectProfile,
    onRenameProfile,
    onDeleteProfile,
    onCancelAccountLogin,
    onLogoutAccount,
    onLogoutWeb,
    showCloseButton = false,
    onClose = () => {}
  }: {
    sessions: SessionSummary[];
    notifications: AppNotification[];
    notificationsBusy: boolean;
    notificationsUnreadCount: number;
    sessionHighlights: Record<string, { kind: "completed" | "attention"; at: number; reason?: string }>;
    selectedId: string | null;
    sessionsBusy: boolean;
    sessionsHasMore: boolean;
    sessionsLoadingMore: boolean;
    sessionsLoadPercent: number;
    searchQuery: string;
    searchScope: SessionSearchScope;
    sessionFilter: SessionSummaryFilter;
    savedSessionFilters: SavedSessionFilter[];
    knownSessionTags: string[];
    sessionFolders: SessionFolder[];
    activeSessionFolder: string | null;
    activeSavedSessionFilterId: string | null;
    showArchived: boolean;
    onSelect: (sessionId: string, profileId?: string | null) => void;
    onCreate: () => void;
    onLoadMore: () => void;
    onSearchQueryChange: (query: string) => void;
    onSearchScopeChange: (scope: SessionSearchScope) => void;
    onSessionFilterChange: (patch: Partial<SessionSummaryFilter>) => void;
    onSelectSessionFolder: (folderName: string | null) => void;
    onSelectUnfiledSessions: () => void;
    onCreateSessionFolder: () => void;
    onToggleSessionFolderPin: (folder: SessionFolder) => void;
    onAddSelectedSessionToFolder: (folderName: string) => void;
    onRemoveSelectedSessionFromFolder: (folderName: string) => void;
    onApplySavedFilter: (filter: SavedSessionFilter | null) => void;
    onSaveCurrentFilter: () => void;
    onDeleteSavedFilter: (filterId: string) => void;
    onArchivedChange: (nextValue: boolean) => void;
    onTogglePin: (session: SessionSummary) => void;
    onToggleArchive: (session: SessionSummary) => void;
    onRequestMoveSessionProfile: (session: SessionSummary) => void;
    profiles?: Array<{
      id: string;
      label: string;
      codexHome: string;
      active: boolean;
    }>;
    profileAccounts?: CodexProfileAccountSummary[];
    profileAccountsBusy?: boolean;
    account:
      | {
          type: "apiKey" | "chatgpt" | null;
          email: string | null;
          planType: string | null;
          requiresOpenaiAuth: boolean;
        }
      | null;
    webRole?: UserRole | null;
    readOnly?: boolean;
    quota: CodexQuotaStatus | null;
    quotaBusy: boolean;
    resetTickets?: CodexResetTicketsPayload | null;
    resetTicketsBusy?: boolean;
    resetTicketUseBusyId?: string | null;
    runtime: CodexRuntimeStatus | null;
    runtimeBusyAction: "install" | "update" | "check" | "status" | null;
    gatewayRestartAvailable?: boolean;
    gatewayRestartBusy?: boolean;
    showPwaInstall?: boolean;
    pwaInstalled?: boolean;
    pwaInstallBusy?: boolean;
    defaultLanguageBridgeEnabled?: boolean;
    defaultLanguageBridgeBusy?: boolean;
    systemShutdownArmed: boolean;
    systemShutdownAvailable: boolean;
    systemShutdownDelaySeconds: number;
    themeMode: ThemeMode;
    resolvedTheme: ResolvedTheme;
    accountLoginFlow: CodexAccountLoginFlow | null;
    onRefreshQuota: () => void;
    onRefreshProfileAccounts: (force?: boolean) => void;
    onRefreshResetTickets: () => void;
    onUseResetTicket: (ticket: CodexResetTicket) => void;
    onRefreshNotifications: () => void;
    onMarkNotificationsRead: (ids: string[] | null) => void;
    onClearNotifications: () => void;
    onRefreshRuntime: () => void;
    onInstallApp: () => void;
    onDefaultLanguageBridgeChange: (enabled: boolean) => void;
    onSystemShutdownArmedChange: (armed: boolean) => void;
    onInstallRuntime: () => void;
    onUpdateRuntime: () => void;
    onRestartGateway: () => void;
    onThemeModeChange: (mode: ThemeMode) => void;
    onStartAccountLogin: (type: "chatgpt" | "chatgptDeviceCode") => void;
    onImportAccountCredentials: (
      path: string,
      options?: { createProfile?: boolean; profileLabel?: string | null; profileId?: string | null }
    ) => void | Promise<void>;
    onSelectProfile: (profileId: string) => void;
    onRenameProfile: (profileId: string, label: string) => void;
    onDeleteProfile: (profileId: string, label: string) => void;
    onCancelAccountLogin: (loginId: string) => void;
    onLogoutAccount: () => void;
    onLogoutWeb: () => void;
    showCloseButton?: boolean;
    onClose?: () => void;
  } = $props();

  let accountMenuOpen = $state(false);
  let notificationsOpen = $state(false);
  let searchPanelOpen = $state(false);
  let listElement = $state<HTMLDivElement | undefined>(undefined);
  let searchTriggerElement = $state<HTMLButtonElement | undefined>(undefined);
  let searchPanelElement = $state<HTMLDivElement | undefined>(undefined);
  let searchInputElement = $state<HTMLInputElement | undefined>(undefined);
  let notificationButtonElement = $state<HTMLButtonElement | undefined>(undefined);
  let notificationPanelElement = $state<HTMLDivElement | undefined>(undefined);
  let notificationPanelStyle = $state("");
  let accountButtonElement = $state<HTMLButtonElement | undefined>(undefined);
  let accountPopoverElement = $state<HTMLDivElement | undefined>(undefined);
  let accountPopoverStyle = $state("");
  let sessionFoldersCollapsed = $state(true);
  let boundedAutoloadPasses = $state(0);
  let loadMoreOrigin = $state<"manual" | "auto" | null>(null);
  let lastSessionListSignature = "";
  let pendingSessionListScrollAnchor: { sessionId: string; offset: number } | null = null;
  let serverAuthJsonPath = $state("");
  let serverAuthJsonCreateProfile = $state(false);
  let serverAuthJsonProfileLabel = $state("");
  let serverAuthJsonImportBusy = $state(false);

  const ui = $derived.by(() => {
    const _locale = $localeSignal;
    const isKorean = getLocale().startsWith("ko");

    return {
      appShortName: m.app_short_name(),
      newThread: m.new_thread(),
      active: m.active(),
      archived: m.archived(),
      searchThreads: m.search_threads(),
      searchArchived: m.search_archived(),
      searchScopeSummary: m.search_scope_summary(),
      searchScopeFull: m.search_scope_full(),
      pinnedOnly: m.pinned_only(),
      runningOnly: m.running_only(),
      queuedOnly: m.queued_only(),
      allActivity: m.all_activity(),
      savedFilters: m.saved_filters(),
      saveCurrentFilter: m.save_current_filter(),
      filterTags: m.filter_tags(),
      sessionFolders: m.session_folders(),
      allFolders: m.all_folders(),
      unfiledSessions: m.unfiled_sessions(),
      newFolder: m.new_folder(),
      createInFolder: m.create_in_folder(),
      pinFolder: m.pin_folder(),
      unpinFolder: m.unpin_folder(),
      addSessionToFolder: m.add_session_to_folder(),
      removeSessionFromFolder: m.remove_session_from_folder(),
      noSavedFilters: m.no_saved_filters(),
      noArchivedSessions: m.no_archived_sessions(),
      archiveThread: m.archive_thread(),
      restoreThread: m.restore_thread(),
      pinThread: m.pin_thread(),
      unpinThread: m.unpin_thread(),
      noSessions: m.no_sessions(),
      noThreadsMatchingSearch: m.no_threads_matching_search(),
      createNewThreadPrompt: m.create_new_thread_prompt(),
      needsInput: m.needs_input(),
      sessionFailed: m.session_failed(),
      sessionStopped: m.session_stopped(),
      done: m.done(),
      newCodexSession: m.new_codex_session(),
      loadingMoreThreads: m.loading_more_threads(),
      autoloadingMoreThreads: m.autoloading_more_threads(),
      loadMoreThreads: m.load_more_threads(),
      loadingNextSessionList: m.loading_next_session_list(),
      runningSessionsCount: (count: number) => m.running_sessions_count({ count: String(count) }),
      notifications: m.notifications(),
      notificationCenter: m.notification_center(),
      noNotifications: m.no_notifications(),
      markAllRead: m.mark_all_read(),
      clearAll: m.clear_all(),
      notificationSessionCompleted: m.notification_session_completed(),
      notificationInputRequired: m.notification_input_required(),
      notificationQueueFailed: m.notification_queue_failed(),
      notificationShutdownScheduled: m.notification_shutdown_scheduled(),
      account: m.account(),
      appearance: m.appearance(),
      language: m.language(),
      quotaUsage: m.quota_usage(),
      quota5h: m.quota_5h(),
      quotaWeekly: m.quota_weekly(),
      resetTickets: isKorean ? "리셋 티켓" : "Reset tickets",
      refreshResetTickets: isKorean ? "리셋 티켓 새로고침" : "Refresh reset tickets",
      useResetTicket: isKorean ? "사용" : "Use",
      resetTicketsAvailable: (count: number) =>
        isKorean ? `${count}개 사용 가능` : `${count} available`,
      resetTicketUnsupported: isKorean
        ? "현재 Codex가 리셋 티켓 정보를 노출하지 않습니다."
        : "This Codex build does not expose reset-ticket records.",
      resetTicketEmpty: isKorean ? "사용 가능한 리셋 티켓이 없습니다." : "No reset tickets available.",
      resetTicketUnavailable: isKorean ? "사용 불가" : "Unavailable",
      runtime: m.runtime(),
      loading: m.loading(),
      version: m.version(),
      binary: m.binary(),
      shutdownAfterQueueCompletes: m.shutdown_after_queue_completes(),
      shutdownWaitDescription: (seconds: number) => m.shutdown_wait_description({ seconds: String(seconds) }),
      missing: m.missing(),
      check: m.check(),
      update: m.update(),
      install: m.install(),
      restartWebui: m.restart_webui(),
      restartingWebui: m.restarting_webui(),
      restartWebuiDescription: m.restart_webui_description(),
      installApp: m.install_app(),
      installingApp: m.installing_app(),
      appInstalled: m.app_installed(),
      close: m.close(),
      refreshQuota: m.refresh_quota(),
      switchAccount: m.switch_account(),
      renameAccount: isKorean ? "계정 라벨 변경" : "Rename account",
      deleteAccount: isKorean ? "계정 삭제" : "Delete account",
      signInAction: m.sign_in_action(),
      signOut: m.sign_out(),
      connected: m.connected(),
      signInRequired: m.sign_in_required(),
      serverCredentialsJson: isKorean ? "서버 credentials JSON" : "Server credentials JSON",
      serverCredentialsJsonPlaceholder: isKorean ? "~/.codex/auth.json 또는 /path/to/auth.json" : "~/.codex/auth.json or /path/to/auth.json",
      serverCredentialsJsonDescription: isKorean
        ? "브라우저 파일이 아니라 서버 파일시스템의 Codex auth.json을 현재 프로필로 가져옵니다."
        : "Imports a Codex auth.json from the server filesystem into the active profile.",
      createProfileFromCredentials: isKorean ? "새 프로필로 가져오기" : "Import as new profile",
      createProfileFromCredentialsDescription: isKorean
        ? "설정 파일에 프로필을 추가합니다. 새 프로필은 WebUI 재시작 후 선택할 수 있습니다."
        : "Adds a profile to the config file. The new profile is selectable after restarting WebUI.",
      newProfileLabelPlaceholder: isKorean ? "프로필 이름(선택)" : "Profile label (optional)",
      importCredentials: isKorean ? "가져오기" : "Import",
      moveSessionToAccount: isKorean ? "세션 계정 이동" : "Move session to account",
      localRuntime: m.local_runtime(),
      currentDarkMode: m.current_dark_mode(),
      currentLightMode: m.current_light_mode(),
      sessionDefaults: m.session_defaults(),
      defaultLanguageBridge: m.default_language_bridge(),
      defaultLanguageBridgeDescription: m.default_language_bridge_description(),
      readOnlyMode: m.read_only_mode(),
      roleAdmin: m.role_admin(),
      roleViewer: m.role_viewer()
    };
  });

  const selectedSession = $derived(sessions.find((session) => session.id === selectedId) ?? null);
  const runningSessionCount = $derived(sessions.filter((session) => isSessionRunning(session)).length);
  const displayedQuotaLimits = $derived(quotaLimitsForDisplay(quota));

  function getDateLocale() {
    return getLocale();
  }

  function formatUpdated(value: number) {
    const normalizedValue = value >= 1_000_000_000_000 ? value : value * 1000;
    return new Intl.DateTimeFormat(getDateLocale(), {
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit"
    }).format(new Date(normalizedValue));
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

  function sessionAccountBadgeLabel(session: SessionSummary) {
    const profileLabel = (session.profileLabel ?? "").trim();
    if (profileLabel) {
      return profileLabel;
    }
    const email = (session.accountEmail ?? "").trim();
    if (email) {
      return email;
    }
    if (profiles.length <= 1) {
      return "";
    }
    return profiles.find((profile) => profile.id === session.profileId)?.label ?? "";
  }

  function shouldShowSessionAccountBadge(session: SessionSummary) {
    return profiles.length > 1 || Boolean(session.accountEmail || session.profileLabel);
  }

  function moveSessionProfileOptions(session: SessionSummary) {
    const currentProfileId = session.profileId ?? profiles.find((profile) => profile.active)?.id ?? null;
    return profiles.filter((profile) => profile.id !== currentProfileId);
  }

  function requestMoveSessionProfile(session: SessionSummary) {
    if (readOnly) {
      return;
    }
    const options = moveSessionProfileOptions(session);
    if (options.length === 0) {
      return;
    }
    onRequestMoveSessionProfile(session);
  }

  async function importServerAuthJson() {
    if (serverAuthJsonImportBusy || readOnly) {
      return;
    }
    const trimmedPath = serverAuthJsonPath.trim();
    if (!trimmedPath) {
      return;
    }
    serverAuthJsonImportBusy = true;
    try {
      await onImportAccountCredentials(trimmedPath, {
        createProfile: serverAuthJsonCreateProfile,
        profileLabel: serverAuthJsonProfileLabel.trim() || null
      });
      serverAuthJsonPath = "";
      serverAuthJsonProfileLabel = "";
    } finally {
      serverAuthJsonImportBusy = false;
    }
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

  function quotaLimitsForDisplay(status: CodexQuotaStatus | null | undefined): CodexQuotaLimit[] {
    const normalizedLimits = (status?.limits ?? []).filter((limit) => Array.isArray(limit.windows) && limit.windows.length > 0);
    if (normalizedLimits.length > 0) {
      return normalizedLimits;
    }

    const normalizedWindows = (status?.windows ?? []).filter(Boolean);
    if (normalizedWindows.length > 0) {
      return [{ id: "codex", name: "Codex", windows: normalizedWindows }];
    }

    const legacyWindows: CodexQuotaWindow[] = [];
    if (status?.fiveHour) {
      legacyWindows.push({
        ...status.fiveHour,
        id: status.fiveHour.id ?? "codex:legacy-5h",
        kind: status.fiveHour.kind ?? "primary",
        label: status.fiveHour.label ?? ui.quota5h,
        windowDurationMinutes: status.fiveHour.windowDurationMinutes ?? 5 * 60
      });
    }
    if (status?.weekly) {
      legacyWindows.push({
        ...status.weekly,
        id: status.weekly.id ?? "codex:legacy-weekly",
        kind: status.weekly.kind ?? "secondary",
        label: status.weekly.label ?? ui.quotaWeekly,
        windowDurationMinutes: status.weekly.windowDurationMinutes ?? 7 * 24 * 60
      });
    }
    return legacyWindows.length > 0 ? [{ id: "codex", name: "Codex", windows: legacyWindows }] : [];
  }

  function quotaWindowsForDisplay(status: CodexQuotaStatus | null | undefined) {
    return quotaLimitsForDisplay(status).flatMap((limit) => limit.windows);
  }

  function quotaWindowLabel(window: CodexQuotaWindow, index: number) {
    const duration = window.windowDurationMinutes;
    if (typeof duration === "number" && Number.isFinite(duration)) {
      if (Math.abs(duration - 5 * 60) <= 15) {
        return ui.quota5h;
      }
      if (Math.abs(duration - 7 * 24 * 60) <= 7 * 24 * 60 * 0.05) {
        return ui.quotaWeekly;
      }
    }
    const label = window.label?.trim();
    if (label) {
      return label;
    }
    return index === 0 ? ui.quotaUsage : `${ui.quotaUsage} ${index + 1}`;
  }

  function formatResetTicketDate(value: string | number | null | undefined) {
    if (value == null || value === "") {
      return null;
    }
    const numericValue = typeof value === "number" ? value : Number(value);
    const date = Number.isFinite(numericValue)
      ? new Date(numericValue >= 1_000_000_000_000 ? numericValue : numericValue * 1000)
      : new Date(value);
    if (!Number.isFinite(date.getTime())) {
      return String(value);
    }
    return new Intl.DateTimeFormat(getDateLocale(), {
      year: "numeric",
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
      timeZoneName: "short"
    }).format(date);
  }

  function resetTicketExpirationLabel(ticket: CodexResetTicket) {
    const expiresAt = formatResetTicketDate(ticket.expiresAt);
    if (expiresAt) {
      return m.reset_ticket_expires_at({ value: expiresAt });
    }
    if (ticket.expirationStatus === "never") {
      return m.reset_ticket_never_expires();
    }
    return m.reset_ticket_expiration_unknown();
  }

  function compactQuotaLabel() {
    const candidates = quotaWindowsForDisplay(quota)
      .map((window) => window.remainingPercent)
      .filter((value): value is number => typeof value === "number" && Number.isFinite(value));
    if (candidates.length === 0) {
      return null;
    }
    return m.quota_left_compact({ percent: String(Math.round(Math.min(...candidates))) });
  }

  function profileAccount(profileId: string) {
    return profileAccounts.find((profile) => profile.profileId === profileId) ?? null;
  }

  function shouldShowResetTickets() {
    return (
      Boolean(resetTicketsBusy && !resetTickets) ||
      Boolean(resetTickets?.supported) ||
      Boolean(resetTickets?.tickets?.length) ||
      Boolean(resetTickets?.error)
    );
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
    const activeProfile = profiles.find((profile) => profile.active) ?? null;
    if (account?.planType) {
      return activeProfile ? `${account.planType} · ${activeProfile.label}` : account.planType;
    }
    if (account?.requiresOpenaiAuth) {
      const base = account?.email || account?.type ? ui.connected : ui.signInRequired;
      return activeProfile ? `${base} · ${activeProfile.label}` : base;
    }
    return activeProfile ? `${ui.localRuntime} · ${activeProfile.label}` : ui.localRuntime;
  }

  function toggleNotifications() {
    notificationsOpen = !notificationsOpen;
    if (notificationsOpen) {
      onRefreshNotifications();
    }
  }

  function closeNotifications() {
    notificationsOpen = false;
    notificationPanelStyle = "";
  }

  function notificationTitle(notification: AppNotification) {
    if (notification.type === "sessionCompleted") {
      return ui.notificationSessionCompleted;
    }
    if (notification.type === "sessionAttention") {
      return attentionReasonLabel(notification.payload.reason);
    }
    if (notification.type === "queueDispatchFailed") {
      return ui.notificationQueueFailed;
    }
    return ui.notificationShutdownScheduled;
  }

  function notificationBody(notification: AppNotification) {
    const sessionLabel = notification.sessionName?.trim();
    if (notification.type === "sessionCompleted") {
      return sessionLabel ? `${sessionLabel} · ${ui.done}` : ui.done;
    }
    if (notification.type === "sessionAttention") {
      const label = attentionReasonLabel(notification.payload.reason);
      return sessionLabel ? `${sessionLabel} · ${label}` : label;
    }
    if (notification.type === "queueDispatchFailed") {
      const message = describeUiError(notification.payload).trim();
      return sessionLabel && message ? `${sessionLabel} · ${message}` : sessionLabel || message || ui.notificationQueueFailed;
    }
    const delaySeconds = Number(notification.payload.delaySeconds ?? 0);
    return delaySeconds > 0 ? `${delaySeconds}s` : ui.notificationShutdownScheduled;
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

  function attentionReasonLabel(reason: unknown) {
    if (reason === "failed") {
      return ui.sessionFailed;
    }
    if (reason === "stopped") {
      return ui.sessionStopped;
    }
    return ui.needsInput;
  }

  function isFailedAttention(highlight: { reason?: string } | undefined) {
    return highlight?.reason === "failed";
  }

  function isStoppedAttention(highlight: { reason?: string } | undefined) {
    return highlight?.reason === "stopped";
  }

  function attentionLabel(highlight: { reason?: string } | undefined) {
    return attentionReasonLabel(highlight?.reason);
  }

  function attentionPillClass(highlight: { reason?: string } | undefined) {
    if (isFailedAttention(highlight)) {
      return "session-attention-pill--failed bg-red-100 text-red-700";
    }
    return isStoppedAttention(highlight)
      ? "session-attention-pill--stopped bg-slate-200 text-slate-700"
      : "session-attention-pill--needs-input bg-amber-100 text-amber-700";
  }

  function sessionCardHighlightClass(sessionId: string) {
    const highlight = sessionHighlights[sessionId];
    if (highlight?.kind === "attention") {
      return isFailedAttention(highlight)
        ? "session-card--attention-failed bg-red-50 border border-red-200/80 ring-1 ring-red-200/60"
        : isStoppedAttention(highlight)
          ? "session-card--attention-stopped bg-slate-50 border border-slate-200/80 ring-1 ring-slate-200/60"
          : "session-card--attention-needs-input bg-amber-50 border border-amber-200/80 ring-1 ring-amber-200/60";
    }
    if (highlight?.kind === "completed") {
      return "bg-emerald-50 border border-emerald-200/80 ring-1 ring-emerald-200/60";
    }
    return "hover:bg-gray-200/50 border border-transparent";
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

  function sessionListSignature() {
    return sessions.map((session) => session.id).join("\u001f");
  }

  function captureSessionListScrollAnchor() {
    if (!listElement || listElement.scrollTop <= 0) {
      return null;
    }
    const containerTop = listElement.getBoundingClientRect().top;
    const sessionRows = Array.from(listElement.querySelectorAll<HTMLElement>("[data-session-id]"));
    for (const row of sessionRows) {
      const sessionId = row.dataset.sessionId;
      if (!sessionId) {
        continue;
      }
      const offset = row.getBoundingClientRect().top - containerTop;
      if (offset >= -4) {
        return { sessionId, offset };
      }
    }
    return null;
  }

  function restoreSessionListScrollAnchor(anchor: { sessionId: string; offset: number }) {
    if (!listElement) {
      return;
    }
    const containerTop = listElement.getBoundingClientRect().top;
    const target = Array.from(listElement.querySelectorAll<HTMLElement>("[data-session-id]")).find(
      (row) => row.dataset.sessionId === anchor.sessionId
    );
    if (!target) {
      return;
    }
    const nextOffset = target.getBoundingClientRect().top - containerTop;
    listElement.scrollTop += nextOffset - anchor.offset;
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
    const hasFilters =
      sessionFilter.pinnedOnly ||
      sessionFilter.runningOnly ||
      sessionFilter.queuedOnly ||
      sessionFilter.untaggedOnly ||
      sessionFilter.highlight !== "all" ||
      sessionFilter.tags.length > 0;
    if (searchScope === "full") {
      return ui.searchScopeFull;
    }
    if (searchQuery.trim()) {
      return ui.searchScopeSummary;
    }
    if (hasFilters) {
      return ui.savedFilters;
    }
    return null;
  }

  function toggleFilterTag(tag: string) {
    const nextTags = sessionFilter.tags.includes(tag)
      ? sessionFilter.tags.filter((entry) => entry !== tag)
      : [...sessionFilter.tags, tag];
    onSessionFilterChange({ tags: nextTags });
  }

  $effect(() => {
    searchQuery;
    searchScope;
    showArchived;
    boundedAutoloadPasses = 0;
    loadMoreOrigin = null;
  });

  $effect.pre(() => {
    const signature = sessionListSignature();
    if (signature === lastSessionListSignature) {
      return;
    }
    pendingSessionListScrollAnchor = captureSessionListScrollAnchor();
  });

  $effect(() => {
    const signature = sessionListSignature();
    if (signature === lastSessionListSignature) {
      return;
    }
    lastSessionListSignature = signature;
    const anchor = pendingSessionListScrollAnchor;
    pendingSessionListScrollAnchor = null;
    if (!anchor) {
      return;
    }
    void tick().then(() => restoreSessionListScrollAnchor(anchor));
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
    accountPopoverStyle = anchoredPopoverStyle(accountButtonElement, accountPopoverElement, {
      align: "start",
      maxWidth: 352,
      minHeight: 96,
      minWidth: 288,
      placement: "above",
      preferredWidth: 320
    });
  }

  async function updateNotificationPanelPosition() {
    if (!notificationsOpen || !notificationButtonElement || !notificationPanelElement || typeof window === "undefined") {
      return;
    }

    await tick();
    notificationPanelStyle = anchoredPopoverStyle(notificationButtonElement, notificationPanelElement, {
      align: "end",
      maxWidth: 352,
      minHeight: 96,
      minWidth: 272,
      placement: "below",
      preferredWidth: 352
    });
  }

  $effect(() => {
    if (!accountMenuOpen) {
      accountPopoverStyle = "";
      return;
    }
    void updateAccountPopoverPosition();
  });

  $effect(() => {
    if (!notificationsOpen) {
      notificationPanelStyle = "";
      return;
    }
    void updateNotificationPanelPosition();
  });

  onMount(() => {
    const update = () => {
      if (notificationsOpen) {
        void updateNotificationPanelPosition();
      }
      if (accountMenuOpen) {
        void updateAccountPopoverPosition();
      }
    };

    const handlePointerDown = (event: MouseEvent) => {
      const target = event.target as Node | null;
      if (
        notificationsOpen &&
        target &&
        !notificationPanelElement?.contains(target) &&
        !notificationButtonElement?.contains(target)
      ) {
        closeNotifications();
      }
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
        if (notificationsOpen) {
          closeNotifications();
        }
        if (accountMenuOpen) {
          accountMenuOpen = false;
        }
      }
    };

    window.addEventListener("resize", update);
    window.addEventListener("scroll", update, true);
    window.visualViewport?.addEventListener("resize", update);
    window.visualViewport?.addEventListener("scroll", update);
    window.addEventListener("mousedown", handlePointerDown);
    window.addEventListener("keydown", handleKeyDown);

    return () => {
      window.removeEventListener("resize", update);
      window.removeEventListener("scroll", update, true);
      window.visualViewport?.removeEventListener("resize", update);
      window.visualViewport?.removeEventListener("scroll", update);
      window.removeEventListener("mousedown", handlePointerDown);
      window.removeEventListener("keydown", handleKeyDown);
    };
  });
</script>

<aside class="sidebar flex h-full w-full min-w-0 flex-col border-r border-gray-200/50 bg-gray-50/80 transition-all">
  <div class="p-4 flex flex-col gap-4">
    <div class="relative flex items-center justify-between">
      <div class="flex items-center gap-2 px-1">
        <div class="w-8 h-8 bg-amber-600 rounded-lg flex items-center justify-center text-white font-bold">C</div>
        <h1 class="text-lg font-semibold tracking-tight text-gray-900">{ui.appShortName}</h1>
      </div>
      <div class="flex items-center gap-1.5">
        <button
          bind:this={notificationButtonElement}
          aria-expanded={notificationsOpen}
          class="relative rounded-lg p-2 text-gray-400 transition-colors hover:bg-gray-100 hover:text-gray-600"
          onclick={toggleNotifications}
          type="button"
        >
          <Bell size={18} />
          {#if notificationsUnreadCount > 0}
            <span class="absolute -right-0.5 -top-0.5 min-w-[1rem] rounded-full bg-amber-500 px-1 py-0.5 text-[9px] font-bold leading-none text-white shadow-sm">
              {Math.min(notificationsUnreadCount, 99)}
            </span>
          {/if}
        </button>
        {#if showCloseButton}
          <button class="p-2 text-gray-400 hover:text-gray-600 hover:bg-gray-100 rounded-lg transition-colors" onclick={onClose}>
            <X size={20} />
          </button>
        {/if}
      </div>

      {#if notificationsOpen}
        <div
          bind:this={notificationPanelElement}
          use:portal
          class="floating-popover sidebar-flyout sidebar-notification-panel grid grid-rows-[auto_auto_minmax(0,1fr)] gap-3 rounded-2xl border p-3 opacity-0 pointer-events-none"
          data-positioned={notificationPanelStyle.includes("top:")}
          style={notificationPanelStyle}
        >
          <div class="flex items-center justify-between gap-3">
            <div>
              <p class="text-[10px] font-bold uppercase tracking-widest text-gray-400">{ui.notifications}</p>
              <p class="text-sm font-semibold text-gray-900">{ui.notificationCenter}</p>
            </div>
            <button class="rounded-lg p-1.5 text-gray-400 transition-colors hover:bg-gray-100 hover:text-gray-600" onclick={closeNotifications} type="button">
              <X size={14} />
            </button>
          </div>

          <div class="flex flex-wrap items-center gap-2">
            <button class="rounded-lg border border-gray-200 bg-gray-50 px-2.5 py-1.5 text-[11px] font-semibold text-gray-600 transition-colors hover:bg-white hover:text-gray-800" onclick={() => onMarkNotificationsRead(null)} type="button">
              {ui.markAllRead}
            </button>
            <button class="rounded-lg border border-gray-200 bg-gray-50 px-2.5 py-1.5 text-[11px] font-semibold text-gray-600 transition-colors hover:bg-white hover:text-gray-800" onclick={onClearNotifications} type="button">
              {ui.clearAll}
            </button>
          </div>

          <div class="min-h-0 overflow-y-auto pr-1 scrollbar-thin">
            {#if notificationsBusy}
              <div class="sidebar-flyout-surface rounded-xl border border-gray-200 bg-gray-50 px-3 py-3 text-xs text-gray-500">{ui.loadingMoreThreads}</div>
            {:else if notifications.length === 0}
              <div class="sidebar-flyout-surface rounded-xl border border-dashed border-gray-200 bg-gray-50/80 px-3 py-4 text-center text-xs text-gray-500">{ui.noNotifications}</div>
            {:else}
              <div class="grid gap-2">
                {#each notifications as notification (notification.id)}
                  <button
                    class={`sidebar-notification-item rounded-xl border px-3 py-2.5 text-left transition-colors ${
                      notification.readAt === null ? "border-amber-200 bg-amber-50/70" : "border-gray-200 bg-gray-50/70 hover:bg-white"
                    }`}
                    onclick={() => {
                      onMarkNotificationsRead([notification.id]);
                      if (notification.sessionId) {
                        onSelect(notification.sessionId);
                        closeNotifications();
                      }
                    }}
                    type="button"
                  >
                    <div class="flex items-start justify-between gap-3">
                      <div class="min-w-0">
                        <p class="truncate text-xs font-semibold text-gray-900">{notificationTitle(notification)}</p>
                        <p class="mt-1 line-clamp-2 text-[11px] leading-5 text-gray-500">{notificationBody(notification)}</p>
                      </div>
                      <div class="flex shrink-0 flex-col items-end gap-1">
                        {#if notification.readAt === null}
                          <span class="h-2 w-2 rounded-full bg-amber-500"></span>
                        {/if}
                        <span class="text-[10px] font-medium text-gray-400">{formatUpdated(Math.floor(notification.createdAt / 1000))}</span>
                      </div>
                    </div>
                  </button>
                {/each}
              </div>
            {/if}
          </div>
        </div>
      {/if}
    </div>

      <button 
      class={`flex w-full items-center gap-2 rounded-xl border px-4 py-3 shadow-sm transition-all group ${
        readOnly
          ? "cursor-not-allowed border-gray-200 bg-gray-100/90 text-gray-400 opacity-70"
          : "bg-white border-gray-200 hover:border-amber-500/50 hover:shadow-md"
      }`}
      disabled={readOnly}
      onclick={onCreate}
      type="button"
    >
      <div class="w-6 h-6 rounded-md bg-amber-50 text-amber-600 flex items-center justify-center group-hover:bg-amber-100 transition-colors">
        <Plus size={18} />
      </div>
      <span class="font-medium text-gray-700">{ui.newThread}</span>
    </button>
  </div>

  <div class="px-4 pb-2 space-y-4">
	    <div class="sidebar-mode-toggle flex p-1 bg-gray-200/50 rounded-lg text-sm">
	      <button
	        class="sidebar-mode-toggle__button flex-1 py-1.5 rounded-md transition-all { !showArchived ? 'bg-white shadow-sm text-gray-900 font-medium' : 'text-gray-500 hover:text-gray-700' }"
	        onclick={() => onArchivedChange(false)}
      >
        {ui.active}
      </button>
      <button
        class="sidebar-mode-toggle__button flex-1 py-1.5 rounded-md transition-all { showArchived ? 'bg-white shadow-sm text-gray-900 font-medium' : 'text-gray-500 hover:text-gray-700' }"
        onclick={() => onArchivedChange(true)}
      >
	        {ui.archived}
	      </button>
	    </div>

	    <div class="sidebar-folders rounded-2xl border border-gray-200 bg-white/70 p-2 shadow-sm">
	      <div class="flex items-center justify-between gap-2">
	        <button
	          aria-expanded={!sessionFoldersCollapsed}
	          class="sidebar-folders-toggle flex min-w-0 flex-1 items-center gap-2 rounded-xl px-1.5 py-1 text-left transition-colors hover:bg-gray-100"
	          onclick={() => {
	            sessionFoldersCollapsed = !sessionFoldersCollapsed;
	          }}
	          type="button"
	        >
	          <ChevronDown
	            size={14}
	            class={`shrink-0 text-gray-400 transition-transform ${sessionFoldersCollapsed ? "-rotate-90" : ""}`}
	          />
	          <Folder size={14} class="shrink-0 text-gray-400" />
	          <span class="min-w-0 flex-1 truncate text-[10px] font-bold uppercase tracking-widest text-gray-400">
	            {ui.sessionFolders}
	          </span>
	          <span class="sidebar-folder-count shrink-0 rounded-full bg-gray-100 px-1.5 py-0.5 text-[10px] font-semibold tabular-nums text-gray-500">
	            {sessionFolders.length}
	          </span>
	        </button>
	        <button
	          class="sidebar-folder-action rounded-lg p-1 text-gray-400 transition-colors hover:bg-amber-50 hover:text-amber-700 disabled:cursor-not-allowed disabled:opacity-45"
	          disabled={readOnly}
	          onclick={onCreateSessionFolder}
	          title={ui.newFolder}
	          type="button"
	        >
	          <Plus size={13} />
	        </button>
	      </div>
              {#if !sessionFoldersCollapsed}
                <div class="mt-1 grid gap-1">
                  <button
                    class={`sidebar-folder-item flex min-w-0 items-center gap-2 rounded-xl px-2 py-1.5 text-left text-xs transition-colors ${
                      activeSessionFolder === null && !sessionFilter.untaggedOnly
                        ? "sidebar-folder-item--active bg-gray-900 text-white shadow-sm"
                        : "text-gray-600 hover:bg-gray-100"
                    }`}
                    onclick={() => onSelectSessionFolder(null)}
                    type="button"
	          >
                    <FolderOpen size={14} class="shrink-0" />
                    <span class="min-w-0 flex-1 truncate">{ui.allFolders}</span>
                  </button>
                  <button
                    class={`sidebar-folder-item flex min-w-0 items-center gap-2 rounded-xl px-2 py-1.5 text-left text-xs transition-colors ${
                      sessionFilter.untaggedOnly ? "sidebar-folder-item--active bg-amber-50 text-amber-800" : "text-gray-600 hover:bg-gray-100"
                    }`}
                    onclick={onSelectUnfiledSessions}
                    type="button"
                  >
                    <Folder size={14} class={`shrink-0 ${sessionFilter.untaggedOnly ? "text-amber-600" : "text-gray-400"}`} />
                    <span class="min-w-0 flex-1 truncate">{ui.unfiledSessions}</span>
                  </button>
                  {#each sessionFolders as folder (folder.name)}
	            <div class={`sidebar-folder-item group/folder flex min-w-0 items-center gap-1 rounded-xl px-1 py-1 transition-colors ${
	              activeSessionFolder === folder.name ? "sidebar-folder-item--active bg-amber-50 text-amber-800" : "text-gray-600 hover:bg-gray-100"
	            }`}>
	              <button
	                class="flex min-w-0 flex-1 items-center gap-2 rounded-lg px-1.5 py-1 text-left text-xs"
	                onclick={() => onSelectSessionFolder(folder.name)}
	                type="button"
	              >
	                {#if activeSessionFolder === folder.name}
	                  <FolderOpen size={14} class="shrink-0 text-amber-600" />
	                {:else}
	                  <Folder size={14} class="shrink-0 text-gray-400" />
	                {/if}
	                <span class="min-w-0 flex-1 truncate font-medium">{folder.name}</span>
	                <span class="sidebar-folder-count shrink-0 rounded-full bg-gray-100 px-1.5 py-0.5 text-[10px] font-semibold tabular-nums text-gray-500">{folder.sessionCount}</span>
	              </button>
	              {#if activeSessionFolder === folder.name}
	                <button
	                  class="sidebar-folder-action rounded-lg p-1 text-amber-600 transition-colors hover:bg-amber-100 disabled:cursor-not-allowed disabled:opacity-45"
	                  disabled={readOnly}
	                  onclick={() => onCreate()}
	                  title={ui.createInFolder}
	                  type="button"
	                >
	                  <Plus size={12} />
	                </button>
	              {/if}
	              {#if selectedSession}
	                <button
	                  class="sidebar-folder-action rounded-lg p-1 text-gray-400 opacity-0 transition-all hover:bg-white hover:text-amber-700 group-hover/folder:opacity-100 group-focus-within/folder:opacity-100 disabled:cursor-not-allowed disabled:opacity-45"
	                  disabled={readOnly}
	                  onclick={() => {
	                    if (selectedSession.tags.includes(folder.name)) {
	                      onRemoveSelectedSessionFromFolder(folder.name);
	                    } else {
	                      onAddSelectedSessionToFolder(folder.name);
	                    }
	                  }}
	                  title={selectedSession.tags.includes(folder.name) ? ui.removeSessionFromFolder : ui.addSessionToFolder}
	                  type="button"
	                >
	                  {#if selectedSession.tags.includes(folder.name)}
	                    <X size={12} />
	                  {:else}
	                    <Plus size={12} />
	                  {/if}
	                </button>
	              {/if}
	              <button
	                class={`sidebar-folder-action rounded-lg p-1 transition-colors disabled:cursor-not-allowed disabled:opacity-45 ${
	                  folder.pinned ? "text-amber-600 hover:bg-amber-100" : "text-gray-400 hover:bg-white hover:text-amber-700"
	                }`}
	                disabled={readOnly}
	                onclick={() => onToggleSessionFolderPin(folder)}
	                title={folder.pinned ? ui.unpinFolder : ui.pinFolder}
	                type="button"
	              >
	                <Pin size={12} />
	              </button>
	            </div>
	          {/each}
	        </div>
	      {:else if activeSessionFolder}
	        <button
	          class="sidebar-folder-item sidebar-folder-item--active mt-1 flex min-w-0 items-center gap-2 rounded-xl bg-amber-50 px-2 py-1.5 text-left text-xs text-amber-800 transition-colors"
	          onclick={() => {
	            sessionFoldersCollapsed = false;
	          }}
	          type="button"
	        >
	          <FolderOpen size={14} class="shrink-0 text-amber-600" />
	          <span class="min-w-0 flex-1 truncate font-medium">{activeSessionFolder}</span>
	        </button>
	      {:else if sessionFilter.untaggedOnly}
	        <button
	          class="sidebar-folder-item sidebar-folder-item--active mt-1 flex min-w-0 items-center gap-2 rounded-xl bg-amber-50 px-2 py-1.5 text-left text-xs text-amber-800 transition-colors"
	          onclick={() => {
	            sessionFoldersCollapsed = false;
	          }}
	          type="button"
	        >
	          <Folder size={14} class="shrink-0 text-amber-600" />
	          <span class="min-w-0 flex-1 truncate font-medium">{ui.unfiledSessions}</span>
	        </button>
	      {/if}
	    </div>

	    <div class="relative">
	      <button
        bind:this={searchTriggerElement}
        aria-expanded={searchPanelOpen}
        class={`sidebar-search-trigger flex w-full items-center gap-3 rounded-xl border px-3 py-2.5 text-left transition-all ${
          searchPanelOpen
            ? "border-amber-300 bg-white shadow-lg shadow-amber-100/40"
            : searchQuery.trim() ||
                searchScope === "full" ||
                sessionFilter.pinnedOnly ||
                sessionFilter.runningOnly ||
                sessionFilter.queuedOnly ||
                sessionFilter.highlight !== "all" ||
                sessionFilter.tags.length > 0
              ? "border-amber-200/80 bg-amber-50/70 shadow-sm"
              : "border-gray-200 bg-white/70 hover:border-gray-300 hover:bg-white hover:shadow-sm"
        }`}
        onclick={toggleSearchPanel}
        type="button"
      >
        <div class={`sidebar-search-trigger__icon flex h-8 w-8 shrink-0 items-center justify-center rounded-lg border ${
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
          class="sidebar-search-panel absolute left-0 right-0 top-[calc(100%+0.55rem)] z-20 overflow-hidden rounded-2xl border border-amber-200/80 bg-white shadow-[0_20px_48px_-28px_rgba(217,119,6,0.45)]"
        >
          <div class="space-y-3 p-3.5">
            <div class="relative group">
              <div class="absolute inset-y-0 left-3 flex items-center pointer-events-none text-gray-400 transition-colors group-focus-within:text-amber-600">
                <Search size={16} />
              </div>
              <input
                bind:this={searchInputElement}
                class="sidebar-search-input w-full rounded-xl border border-gray-200 bg-gray-50/80 py-2.5 pl-10 pr-10 text-sm text-gray-700 placeholder-gray-400 transition-all focus:border-amber-400 focus:bg-white focus:outline-none focus:ring-2 focus:ring-amber-500/10"
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
                  class="sidebar-search-clear absolute inset-y-0 right-2 my-auto flex h-7 w-7 items-center justify-center rounded-lg text-gray-400 transition-colors hover:bg-gray-100 hover:text-gray-600"
                  onclick={clearSearchQuery}
                  title={ui.close}
                  type="button"
                >
                  <X size={14} />
                </button>
              {/if}
            </div>

            <div class="sidebar-search-scope flex gap-1 rounded-xl bg-gray-100 p-1">
              <button
                class={`sidebar-search-scope__button flex-1 rounded-lg px-2 py-1.5 text-[11px] font-semibold transition-all ${
                  searchScope === "summary" ? "bg-white text-gray-900 shadow-sm" : "text-gray-500 hover:text-gray-700"
                }`}
                onclick={() => onSearchScopeChange("summary")}
                type="button"
              >
                {ui.searchScopeSummary}
              </button>
              <button
                class={`sidebar-search-scope__button flex-1 rounded-lg px-2 py-1.5 text-[11px] font-semibold transition-all ${
                  searchScope === "full" ? "bg-white text-gray-900 shadow-sm" : "text-gray-500 hover:text-gray-700"
                }`}
                onclick={() => onSearchScopeChange("full")}
                type="button"
              >
                {ui.searchScopeFull}
              </button>
            </div>

            <div class="grid grid-cols-3 gap-2">
              <button
                class={`sidebar-search-filter rounded-lg border px-2 py-1.5 text-[11px] font-semibold transition-all ${
                  sessionFilter.pinnedOnly
                    ? "border-amber-200 bg-amber-50 text-amber-700"
                    : "border-gray-200 bg-gray-50 text-gray-500 hover:bg-white hover:text-gray-700"
                }`}
                onclick={() => onSessionFilterChange({ pinnedOnly: !sessionFilter.pinnedOnly })}
                type="button"
              >
                {ui.pinnedOnly}
              </button>
              <button
                class={`sidebar-search-filter rounded-lg border px-2 py-1.5 text-[11px] font-semibold transition-all ${
                  sessionFilter.runningOnly
                    ? "border-amber-200 bg-amber-50 text-amber-700"
                    : "border-gray-200 bg-gray-50 text-gray-500 hover:bg-white hover:text-gray-700"
                }`}
                onclick={() => onSessionFilterChange({ runningOnly: !sessionFilter.runningOnly })}
                type="button"
              >
                {ui.runningOnly}
              </button>
              <button
                class={`sidebar-search-filter rounded-lg border px-2 py-1.5 text-[11px] font-semibold transition-all ${
                  sessionFilter.queuedOnly
                    ? "border-amber-200 bg-amber-50 text-amber-700"
                    : "border-gray-200 bg-gray-50 text-gray-500 hover:bg-white hover:text-gray-700"
                }`}
                onclick={() => onSessionFilterChange({ queuedOnly: !sessionFilter.queuedOnly })}
                type="button"
              >
                {ui.queuedOnly}
              </button>
            </div>

            <div class="flex gap-1 rounded-xl bg-gray-100 p-1">
              <button
                class={`flex-1 rounded-lg px-2 py-1.5 text-[11px] font-semibold transition-all ${
                  sessionFilter.highlight === "all" ? "bg-white text-gray-900 shadow-sm" : "text-gray-500 hover:text-gray-700"
                }`}
                onclick={() => onSessionFilterChange({ highlight: "all" })}
                type="button"
              >
                {ui.allActivity}
              </button>
              <button
                class={`flex-1 rounded-lg px-2 py-1.5 text-[11px] font-semibold transition-all ${
                  sessionFilter.highlight === "attention" ? "bg-white text-amber-700 shadow-sm" : "text-gray-500 hover:text-gray-700"
                }`}
                onclick={() => onSessionFilterChange({ highlight: "attention" })}
                type="button"
              >
                {ui.needsInput}
              </button>
              <button
                class={`flex-1 rounded-lg px-2 py-1.5 text-[11px] font-semibold transition-all ${
                  sessionFilter.highlight === "completed" ? "bg-white text-emerald-700 shadow-sm" : "text-gray-500 hover:text-gray-700"
                }`}
                onclick={() => onSessionFilterChange({ highlight: "completed" })}
                type="button"
              >
                {ui.done}
              </button>
            </div>

            {#if knownSessionTags.length > 0}
              <div class="space-y-2">
                <p class="text-[10px] font-bold uppercase tracking-widest text-gray-400">{ui.filterTags}</p>
                <div class="flex flex-wrap gap-1.5">
                  {#each knownSessionTags as tag (tag)}
                    <button
                      class={`rounded-full border px-2 py-1 text-[10px] font-semibold transition-all ${
                        sessionFilter.tags.includes(tag)
                          ? "border-amber-200 bg-amber-50 text-amber-700"
                          : "border-gray-200 bg-gray-50 text-gray-500 hover:bg-white hover:text-gray-700"
                      }`}
                      onclick={() => toggleFilterTag(tag)}
                      type="button"
                    >
                      {tag}
                    </button>
                  {/each}
                </div>
              </div>
            {/if}

            <div class="space-y-2">
              <div class="flex items-center justify-between gap-2">
                <p class="text-[10px] font-bold uppercase tracking-widest text-gray-400">{ui.savedFilters}</p>
                <button
                  class="rounded-lg border border-gray-200 bg-gray-50 px-2 py-1 text-[10px] font-bold text-gray-600 transition-colors hover:bg-white hover:text-gray-800 disabled:cursor-not-allowed disabled:opacity-50"
                  disabled={readOnly}
                  onclick={onSaveCurrentFilter}
                  type="button"
                >
                  {ui.saveCurrentFilter}
                </button>
              </div>

              <div class="flex flex-wrap gap-1.5">
                <button
                  class={`rounded-full border px-2 py-1 text-[10px] font-semibold transition-all ${
                    activeSavedSessionFilterId === null && !sessionFilter.pinnedOnly && !sessionFilter.runningOnly && !sessionFilter.queuedOnly && !sessionFilter.untaggedOnly && sessionFilter.highlight === "all" && sessionFilter.tags.length === 0
                      ? "border-gray-900 bg-gray-900 text-white"
                      : "border-gray-200 bg-gray-50 text-gray-500 hover:bg-white hover:text-gray-700"
                  }`}
                  onclick={() => onApplySavedFilter(null)}
                  type="button"
                >
                  {ui.allActivity}
                </button>
                {#each savedSessionFilters as filter (filter.id)}
                  <div class="inline-flex items-center gap-1 rounded-full border border-gray-200 bg-gray-50 pr-1">
                    <button
                      class={`rounded-full px-2 py-1 text-[10px] font-semibold transition-all ${
                        activeSavedSessionFilterId === filter.id
                          ? "bg-gray-900 text-white"
                          : "text-gray-600 hover:bg-white hover:text-gray-800"
                      }`}
                      onclick={() => onApplySavedFilter(filter)}
                      type="button"
                    >
                      {filter.name}
                    </button>
                    <button
                      class="rounded-full p-1 text-gray-400 transition-colors hover:bg-white hover:text-gray-700 disabled:cursor-not-allowed disabled:opacity-50"
                      disabled={readOnly}
                      onclick={() => onDeleteSavedFilter(filter.id)}
                      title={ui.close}
                      type="button"
                    >
                      <X size={10} />
                    </button>
                  </div>
                {/each}
              </div>
              {#if savedSessionFilters.length === 0}
                <p class="text-[11px] text-gray-400">{ui.noSavedFilters}</p>
              {/if}
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

    <div class="px-4 pb-1 pt-2">
      <div class={`flex h-8 items-center justify-between rounded-xl border px-2.5 text-xs font-semibold ${
        runningSessionCount > 0
          ? "border-amber-200 bg-amber-50 text-amber-800"
          : "border-gray-200 bg-gray-50 text-gray-500"
      }`}>
        <span class="flex min-w-0 items-center gap-2">
          <span class={`h-2 w-2 shrink-0 rounded-full ${runningSessionCount > 0 ? "animate-pulse bg-amber-500" : "bg-gray-300"}`}></span>
          <span class="truncate">{ui.runningSessionsCount(runningSessionCount)}</span>
        </span>
      </div>
    </div>

    <div
      bind:this={listElement}
      class="flex-1 overflow-y-auto px-4 pb-2 pt-1 space-y-1 scrollbar-thin"
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

      {#each sessions as session (`${session.profileId ?? ""}:${session.id}`)}
        <div class="group relative" data-session-id={session.id}>
          <button
            class={`w-full rounded-xl p-3 text-left transition-all relative ${
              session.id === selectedId ? "bg-white shadow-sm border border-gray-200 ring-1 ring-gray-200/50" : sessionCardHighlightClass(session.id)
            }`}
            onclick={() => onSelect(session.id, session.profileId ?? null)}
            type="button"
          >
            <div class="flex flex-col gap-1.5">
              <div class="flex min-w-0 items-center gap-1.5">
                {#if session.pinned}
                  <span class="flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-amber-50 text-amber-600">
                    <Pin size={10} />
                  </span>
                {/if}
                {#if isSessionRunning(session)}
                  <span class="h-2 w-2 shrink-0 rounded-full bg-amber-500 shadow-[0_0_8px_rgba(245,158,11,0.5)] animate-pulse"></span>
                {/if}
                <span class="min-w-0 flex-1 truncate text-sm font-medium text-gray-900 transition-colors group-hover:text-amber-700">
                  {displaySessionTitle(session)}
                </span>
              </div>
              
              <p class="text-xs text-gray-500 line-clamp-1 break-all">
                {session.preview || ui.newCodexSession}
              </p>

              {#if session.queueCount > 0 || sessionHighlights[session.id]}
                <div class="flex min-w-0 flex-wrap items-center gap-1.5">
                  {#if session.queueCount > 0}
                    <span class="inline-flex items-center gap-1 rounded-full bg-slate-100 px-1.5 py-0.5 text-[10px] font-semibold tabular-nums text-slate-600">
                      <History size={10} />
                      <span>{session.queueCount}</span>
                    </span>
                  {/if}
                  {#if sessionHighlights[session.id]?.kind === "attention"}
                    <span class="text-[9px] px-1.5 py-0.5 rounded-full {attentionPillClass(sessionHighlights[session.id])} font-bold uppercase tracking-widest">{attentionLabel(sessionHighlights[session.id])}</span>
                  {:else if sessionHighlights[session.id]?.kind === "completed"}
                    <span class="text-[9px] px-1.5 py-0.5 rounded-full bg-emerald-100 text-emerald-700 font-bold uppercase tracking-widest">{ui.done}</span>
                  {/if}
                </div>
              {/if}

              {#if session.tags.length > 0}
                <div class="flex flex-wrap gap-1">
                  {#each session.tags.slice(0, 3) as tag (tag)}
                    <span class="rounded-full border border-gray-200 bg-white px-1.5 py-0.5 text-[9px] font-semibold text-gray-500">
                      {tag}
                    </span>
                  {/each}
                  {#if session.tags.length > 3}
                    <span class="rounded-full border border-gray-200 bg-white px-1.5 py-0.5 text-[9px] font-semibold text-gray-400">
                      +{session.tags.length - 3}
                    </span>
                  {/if}
                </div>
              {/if}
              
              <div class="flex items-center justify-between gap-2 mt-1">
                <div class="flex min-w-0 items-center gap-1.5">
                  <span class="shrink-0 text-[10px] font-medium tracking-tight text-gray-400">
                    {formatUpdated(session.updatedAt)}
                  </span>
                  {#if shouldShowSessionAccountBadge(session) && sessionAccountBadgeLabel(session)}
                    {#if moveSessionProfileOptions(session).length > 0 && !readOnly}
                      <span
                        aria-label={ui.moveSessionToAccount}
                        class="session-account-badge session-account-badge--button inline-flex min-w-0 max-w-[8.5rem] items-center gap-1 rounded-full border border-gray-200 bg-white px-1.5 py-0.5 text-[9px] font-semibold text-gray-500"
                        role="button"
                        tabindex="0"
                        title={`${ui.moveSessionToAccount}: ${session.profileCodexHome ?? sessionAccountBadgeLabel(session)}`}
                        onclick={(event) => {
                          event.stopPropagation();
                          requestMoveSessionProfile(session);
                        }}
                        onkeydown={(event) => {
                          if (event.key !== "Enter" && event.key !== " ") {
                            return;
                          }
                          event.preventDefault();
                          event.stopPropagation();
                          requestMoveSessionProfile(session);
                        }}
                      >
                        <User size={9} class="shrink-0" />
                        <span class="truncate">{sessionAccountBadgeLabel(session)}</span>
                      </span>
                    {:else}
                      <span
                        class="session-account-badge inline-flex min-w-0 max-w-[8.5rem] items-center gap-1 rounded-full border border-gray-200 bg-white px-1.5 py-0.5 text-[9px] font-semibold text-gray-500"
                        title={session.profileCodexHome ?? sessionAccountBadgeLabel(session)}
                      >
                        <User size={9} class="shrink-0" />
                        <span class="truncate">{sessionAccountBadgeLabel(session)}</span>
                      </span>
                    {/if}
                  {/if}
                </div>
                {#if session.agentNickname}
                  <span class="text-[10px] px-1.5 py-0.5 bg-gray-100 text-gray-500 rounded font-semibold uppercase tracking-wider">
                    {session.agentNickname}
                  </span>
                {/if}
              </div>
            </div>
          </button>
          <button
            aria-label={session.pinned ? ui.unpinThread : ui.pinThread}
            class={`absolute right-9 top-2 z-10 rounded-lg border border-gray-200 bg-white/95 p-1.5 text-gray-400 shadow-sm transition-all ${
              readOnly
                ? "cursor-not-allowed opacity-45"
                : "opacity-0 pointer-events-none group-hover:pointer-events-auto group-hover:opacity-100 group-focus-within:pointer-events-auto group-focus-within:opacity-100 hover:border-amber-200 hover:bg-amber-50 hover:text-amber-700"
            }`}
            disabled={readOnly}
            onclick={(event) => {
              event.stopPropagation();
              onTogglePin(session);
            }}
            title={session.pinned ? ui.unpinThread : ui.pinThread}
            type="button"
          >
            <Pin size={14} />
          </button>
          {#if moveSessionProfileOptions(session).length > 0}
            <button
              aria-label={ui.moveSessionToAccount}
              class={`absolute right-16 top-2 z-10 rounded-lg border border-gray-200 bg-white/95 p-1.5 text-gray-400 shadow-sm transition-all ${
                readOnly
                  ? "cursor-not-allowed opacity-45"
                  : "opacity-0 pointer-events-none group-hover:pointer-events-auto group-hover:opacity-100 group-focus-within:pointer-events-auto group-focus-within:opacity-100 hover:border-amber-200 hover:bg-amber-50 hover:text-amber-700"
              }`}
              disabled={readOnly}
              onclick={(event) => {
                event.stopPropagation();
                requestMoveSessionProfile(session);
              }}
              title={ui.moveSessionToAccount}
              type="button"
            >
              <UserCog size={14} />
            </button>
          {/if}
          <button
            aria-label={showArchived ? ui.restoreThread : ui.archiveThread}
            class={`absolute right-2 top-2 z-10 rounded-lg border border-gray-200 bg-white/95 p-1.5 text-gray-400 shadow-sm transition-all ${
              readOnly
                ? "cursor-not-allowed opacity-45"
                : "opacity-0 pointer-events-none group-hover:pointer-events-auto group-hover:opacity-100 group-focus-within:pointer-events-auto group-focus-within:opacity-100 hover:border-amber-200 hover:bg-amber-50 hover:text-amber-700"
            }`}
            disabled={readOnly}
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
      onclick={() => {
        const opening = !accountMenuOpen;
        accountMenuOpen = opening;
        if (opening && !readOnly) {
          onRefreshProfileAccounts(false);
        }
      }}
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
        use:portal
        class="floating-popover sidebar-flyout sidebar-account-popover grid min-h-0 grid-rows-[auto_minmax(0,1fr)_auto] border rounded-2xl overflow-hidden opacity-0 pointer-events-none p-2 w-80 max-w-[calc(100vw-1rem)]"
        data-positioned={accountPopoverStyle.includes("top:")}
        style={accountPopoverStyle}
      >
        <div class="sidebar-flyout-header p-4 border-b border-gray-100 flex items-center justify-between">
          <div>
            <h3 class="text-xs font-bold text-gray-400 uppercase tracking-widest mb-1">{ui.account}</h3>
            <p class="text-sm font-semibold text-gray-900">{accountLabel()}</p>
            <div class="mt-2 flex flex-wrap items-center gap-1.5">
              <span class="sidebar-flyout-badge rounded-full border border-gray-200 bg-gray-50 px-2 py-0.5 text-[10px] font-bold uppercase tracking-[0.18em] text-gray-500">
                {webRole === "viewer" ? ui.roleViewer : ui.roleAdmin}
              </span>
              {#if readOnly}
                <span class="sidebar-flyout-badge sidebar-flyout-badge--warning rounded-full border border-amber-200 bg-amber-50 px-2 py-0.5 text-[10px] font-bold uppercase tracking-[0.18em] text-amber-700">
                  {ui.readOnlyMode}
                </span>
              {/if}
            </div>
          </div>
          <button class="p-1.5 text-gray-400 hover:text-gray-600 hover:bg-gray-100 rounded-lg transition-colors" onclick={() => (accountMenuOpen = false)}>
            <X size={16} />
          </button>
        </div>

        <div class="min-h-0 overflow-y-auto px-4 py-4 space-y-6 pr-3 scrollbar-thin">
          {#if profiles.length > 0}
            <div class="space-y-3">
              <div class="flex items-center justify-between gap-2">
                <h4 class="text-[10px] font-bold text-gray-400 uppercase tracking-widest flex items-center gap-1.5">
                  <User size={10} /> {ui.switchAccount}
                </h4>
                <button
                  aria-label={ui.refreshQuota}
                  class="rounded-md p-1 text-gray-400 transition-colors hover:bg-gray-100 hover:text-gray-700 disabled:cursor-not-allowed disabled:opacity-50"
                  disabled={profileAccountsBusy || readOnly}
                  onclick={() => onRefreshProfileAccounts(true)}
                  title={ui.refreshQuota}
                  type="button"
                >
                  <RefreshCw size={12} class={profileAccountsBusy ? "animate-spin" : ""} />
                </button>
              </div>

              <div class="space-y-2">
                {#each profiles as profile (profile.id)}
                  {@const profileAccountState = profileAccount(profile.id)}
                  <div
                    class={`sidebar-account-switcher flex w-full items-start justify-between gap-2 rounded-xl border px-3 py-2 text-left transition-all ${
                      profile.active
                        ? "border-amber-300 bg-amber-50 text-amber-800 shadow-sm"
                        : "border-gray-200 bg-gray-50 text-gray-600 hover:border-gray-300 hover:bg-white hover:text-gray-800"
                    }`}
                  >
                    <button class="min-w-0 flex-1 text-left" onclick={() => onSelectProfile(profile.id)} type="button">
                      <div class="flex min-w-0 items-center gap-2">
                        <p class="truncate text-sm font-semibold">{profile.label}</p>
                        {#if profileAccountState?.account.planType}
                          <span class="sidebar-flyout-badge shrink-0 rounded-full border border-gray-200 bg-white/80 px-1.5 py-0.5 text-[9px] font-bold uppercase tracking-wide text-gray-500">
                            {profileAccountState.account.planType}
                          </span>
                        {/if}
                      </div>
                      <p class="mt-0.5 truncate text-[10px] text-gray-400">
                        {profileAccountState?.account.email || profile.codexHome}
                      </p>
                      {#if profileAccountState}
                        <div class="mt-1.5 flex flex-wrap items-center gap-x-2 gap-y-1 text-[9px] font-semibold text-gray-500">
                          {#each quotaWindowsForDisplay(profileAccountState.quota).slice(0, 2) as window, windowIndex (window.id ?? `${profile.id}:${windowIndex}`)}
                            <span>{quotaWindowLabel(window, windowIndex)} {formatQuotaPercent(window.remainingPercent)}</span>
                          {/each}
                        </div>
                      {/if}
                    </button>
                    <div class="flex shrink-0 items-center gap-1">
                      {#if profile.active}
                        <span class="sidebar-flyout-badge rounded-full bg-white/80 px-2 py-0.5 text-[10px] font-bold uppercase tracking-[0.18em] text-amber-700">
                          {ui.connected}
                        </span>
                      {/if}
                      <button
                        aria-label={ui.renameAccount}
                        class="rounded-md p-1 text-gray-400 transition-colors hover:bg-white hover:text-gray-700 disabled:cursor-not-allowed disabled:opacity-40"
                        disabled={readOnly}
                        onclick={() => onRenameProfile(profile.id, profile.label)}
                        title={ui.renameAccount}
                        type="button"
                      >
                        <Pencil size={12} />
                      </button>
                      <button
                        aria-label={ui.deleteAccount}
                        class="rounded-md p-1 text-gray-400 transition-colors hover:bg-red-50 hover:text-red-600 disabled:cursor-not-allowed disabled:opacity-40"
                        disabled={readOnly || profile.active || profiles.length <= 1}
                        onclick={() => onDeleteProfile(profile.id, profile.label)}
                        title={ui.deleteAccount}
                        type="button"
                      >
                        <Trash2 size={12} />
                      </button>
                    </div>
                  </div>
                {/each}
              </div>
            </div>
          {/if}

          <div class="space-y-3">
            <h4 class="flex items-center gap-1.5 text-[10px] font-bold uppercase tracking-widest text-gray-400">
              <Download size={10} /> {ui.serverCredentialsJson}
            </h4>
            <div class="sidebar-flyout-surface rounded-xl border border-gray-100 bg-gray-50 p-3">
              <p class="mb-2 text-[11px] leading-snug text-gray-500">{ui.serverCredentialsJsonDescription}</p>
              <label
                class:checkbox-card--disabled={readOnly || serverAuthJsonImportBusy}
                class="checkbox-card checkbox-card--compact mb-2 w-full max-w-full"
                for="server-auth-json-create-profile"
              >
                <input
                  bind:checked={serverAuthJsonCreateProfile}
                  class="checkbox-input"
                  disabled={readOnly || serverAuthJsonImportBusy}
                  id="server-auth-json-create-profile"
                  type="checkbox"
                />
                <span aria-hidden="true" class="checkbox-control"></span>
                <span class="checkbox-copy min-w-0">
                  <span class="checkbox-title">{ui.createProfileFromCredentials}</span>
                  <span class="checkbox-description">{ui.createProfileFromCredentialsDescription}</span>
                </span>
              </label>
              {#if serverAuthJsonCreateProfile}
                <input
                  bind:value={serverAuthJsonProfileLabel}
                  class="mb-2 w-full rounded-lg border border-gray-200 bg-white px-2.5 py-2 text-xs font-medium text-gray-700 outline-none transition focus:border-amber-400 focus:ring-4 focus:ring-amber-100 disabled:cursor-not-allowed disabled:opacity-60"
                  disabled={readOnly || serverAuthJsonImportBusy}
                  placeholder={ui.newProfileLabelPlaceholder}
                  type="text"
                />
              {/if}
              <div class="flex gap-2">
                <input
                  bind:value={serverAuthJsonPath}
                  class="min-w-0 flex-1 rounded-lg border border-gray-200 bg-white px-2.5 py-2 text-xs font-medium text-gray-700 outline-none transition focus:border-amber-400 focus:ring-4 focus:ring-amber-100 disabled:cursor-not-allowed disabled:opacity-60"
                  disabled={readOnly || serverAuthJsonImportBusy}
                  onkeydown={(event) => {
                    if (event.key === "Enter") {
                      event.preventDefault();
                      void importServerAuthJson();
                    }
                  }}
                  placeholder={ui.serverCredentialsJsonPlaceholder}
                  type="text"
                />
                <button
                  class="shrink-0 rounded-lg bg-gray-900 px-3 py-2 text-[11px] font-bold text-white shadow-sm transition-all hover:bg-gray-800 disabled:cursor-not-allowed disabled:opacity-50"
                  disabled={readOnly || serverAuthJsonImportBusy || !serverAuthJsonPath.trim()}
                  onclick={() => {
                    void importServerAuthJson();
                  }}
                  type="button"
                >
                  {#if serverAuthJsonImportBusy}
                    <RefreshCw size={12} class="animate-spin" />
                  {:else}
                    {ui.importCredentials}
                  {/if}
                </button>
              </div>
            </div>
          </div>

          <div class="space-y-4">
            <h4 class="text-[10px] font-bold text-gray-400 uppercase tracking-widest flex items-center gap-1.5">
              <Monitor size={10} /> {ui.appearance}
            </h4>

            <div class="grid grid-cols-3 gap-2">
              {#each themeOptions as option (option.mode)}
                <button
                  class={`sidebar-theme-option flex flex-col items-center gap-1 rounded-xl border px-2.5 py-2 text-[10px] font-bold transition-all ${
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
                class="sidebar-flyout-select w-full appearance-none rounded-xl border border-gray-200 bg-gray-50 px-3 py-2.5 pr-9 text-sm font-semibold text-gray-700 shadow-sm outline-none transition focus:border-amber-400 focus:bg-white focus:ring-4 focus:ring-amber-100"
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
              <Settings size={10} /> {ui.sessionDefaults}
            </h4>

            <label
              class:checkbox-card--disabled={readOnly || defaultLanguageBridgeBusy}
              class="checkbox-card checkbox-card--compact w-full max-w-full"
              for="default-language-bridge"
            >
              <input
                checked={defaultLanguageBridgeEnabled}
                class="checkbox-input"
                disabled={readOnly || defaultLanguageBridgeBusy}
                id="default-language-bridge"
                onchange={(event) => onDefaultLanguageBridgeChange((event.currentTarget as HTMLInputElement).checked)}
                type="checkbox"
              />
              <span aria-hidden="true" class="checkbox-control"></span>
              <span class="checkbox-copy min-w-0">
                <span class="checkbox-title">{ui.defaultLanguageBridge}</span>
                <span class="checkbox-description">{ui.defaultLanguageBridgeDescription}</span>
              </span>
            </label>
          </div>

          <div class="space-y-4">
            <h4 class="text-[10px] font-bold text-gray-400 uppercase tracking-widest flex items-center gap-1.5">
              <Zap size={10} /> {ui.quotaUsage}
            </h4>

            {#if displayedQuotaLimits.length > 0}
              <div class="space-y-4">
                {#each displayedQuotaLimits as limit (limit.id)}
                  <div class="space-y-3">
                    {#if displayedQuotaLimits.length > 1}
                      <p class="truncate text-[10px] font-bold uppercase tracking-wide text-gray-500">{limit.name}</p>
                    {/if}
                    {#each limit.windows as window, windowIndex (window.id ?? `${limit.id}:${windowIndex}`)}
                      <div class="space-y-1.5">
                        <div class="flex justify-between gap-3 text-xs">
                          <span class="min-w-0 truncate font-medium text-gray-600">{quotaWindowLabel(window, windowIndex)}</span>
                          <span class="shrink-0 font-bold text-gray-900">{formatQuotaPercent(window.remainingPercent)}</span>
                        </div>
                        <div class="sidebar-flyout-meter h-1.5 w-full overflow-hidden rounded-full bg-gray-100">
                          <div
                            class="h-full rounded-full bg-amber-500"
                            style={`width: ${Math.max(0, Math.min(100, window.remainingPercent ?? 0))}%`}
                          ></div>
                        </div>
                        <p class="text-[10px] italic text-gray-400">{formatQuotaReset(window.resetAt)}</p>
                      </div>
                    {/each}
                  </div>
                {/each}
              </div>
            {:else if quota?.error}
              <p class="text-[11px] leading-snug text-red-500">{quota.error}</p>
            {:else}
              <p class="text-[11px] leading-snug text-gray-500">{formatQuotaPercent(null)}</p>
            {/if}
          </div>

          {#if shouldShowResetTickets()}
            <div class="space-y-3">
              <div class="flex items-center justify-between gap-2">
                <div class="flex min-w-0 items-center gap-2">
                  <h4 class="flex items-center gap-1.5 text-[10px] font-bold uppercase tracking-widest text-gray-400">
                    <Ticket size={10} /> {ui.resetTickets}
                  </h4>
                  {#if resetTickets?.supported}
                    <span class="rounded-full bg-amber-100 px-2 py-0.5 text-[10px] font-bold text-amber-700">
                      {ui.resetTicketsAvailable(resetTickets.availableCount ?? resetTickets.tickets.length)}
                    </span>
                  {/if}
                </div>
                <button
                  aria-label={ui.refreshResetTickets}
                  class="rounded-md p-1 text-gray-400 transition-colors hover:bg-gray-100 hover:text-gray-700 disabled:cursor-not-allowed disabled:opacity-50"
                  disabled={resetTicketsBusy}
                  onclick={onRefreshResetTickets}
                  title={ui.refreshResetTickets}
                  type="button"
                >
                  <RefreshCw size={12} class={resetTicketsBusy ? "animate-spin" : ""} />
                </button>
              </div>

              <div class="sidebar-flyout-surface rounded-xl border border-gray-100 bg-gray-50 p-2 space-y-2">
                {#if resetTicketsBusy && !resetTickets}
                  <div class="flex items-center gap-2 text-[11px] text-gray-500">
                    <RefreshCw size={12} class="animate-spin" />
                    {ui.loading}
                  </div>
                {:else if resetTickets?.error}
                  <div class="flex items-start gap-2 text-[11px] text-red-500">
                    <AlertCircle size={12} class="mt-0.5 shrink-0" />
                    <span>{resetTickets.error}</span>
                  </div>
                {:else if resetTickets && !resetTickets.supported}
                  <p class="text-[11px] leading-snug text-gray-500">{resetTickets.message ?? ui.resetTicketUnsupported}</p>
                {:else if resetTickets?.tickets?.length}
                  <div class="space-y-1.5">
                    {#each resetTickets.tickets as ticket (ticket.id)}
                      <div class="rounded-lg border border-gray-200 bg-white p-2">
                        <div class="flex items-start justify-between gap-2">
                          <div class="min-w-0">
                            <div class="truncate text-[11px] font-bold text-gray-800">
                              {ticket.label ?? ticket.limitName ?? ticket.limitId ?? ticket.id}
                            </div>
                            <div class="truncate text-[10px] text-gray-400" title={resetTicketExpirationLabel(ticket)}>
                              {resetTicketExpirationLabel(ticket)}
                            </div>
                          </div>
                          <button
                            class="shrink-0 rounded-md border border-amber-200 bg-amber-50 px-2 py-1 text-[10px] font-bold text-amber-700 transition-colors hover:bg-amber-100 disabled:cursor-not-allowed disabled:opacity-50"
                            disabled={readOnly || !ticket.available || resetTicketUseBusyId === ticket.id}
                            onclick={() => onUseResetTicket(ticket)}
                            type="button"
                          >
                            {#if resetTicketUseBusyId === ticket.id}
                              <RefreshCw size={10} class="animate-spin" />
                            {:else if ticket.available}
                              {ui.useResetTicket}
                            {:else}
                              {ui.resetTicketUnavailable}
                            {/if}
                          </button>
                        </div>
                      </div>
                    {/each}
                  </div>
                {:else}
                  <p class="text-[11px] leading-snug text-gray-500">{ui.resetTicketEmpty}</p>
                {/if}
              </div>
            </div>
          {/if}

          <div class="space-y-4">
            <h4 class="text-[10px] font-bold text-gray-400 uppercase tracking-widest flex items-center gap-1.5">
              <Cpu size={10} /> {ui.runtime}
            </h4>
            <div class="sidebar-flyout-surface rounded-xl border border-gray-100 bg-gray-50 p-3 space-y-3">
              <div class="flex justify-between text-xs">
                <span class="text-gray-500">{ui.version}</span>
                <span class="font-semibold text-gray-700">
                  {#if runtimeBusyAction === "status" && !runtime}
                    <span class="inline-flex items-center gap-1.5 text-gray-400">
                      <RefreshCw size={10} class="animate-spin" />
                      {ui.loading}
                    </span>
                  {:else}
                    {runtime?.version ?? ui.missing}
                  {/if}
                </span>
              </div>
              <div class="flex justify-between text-xs">
                <span class="text-gray-500">{ui.binary}</span>
                <code class="sidebar-flyout-code rounded border border-gray-200 bg-white px-1.5 py-0.5 text-[10px]">{runtime?.configuredBin ?? "codex"}</code>
              </div>
              <div class="flex gap-2 pt-1">
                {#if runtimeBusyAction === "status" && !runtime}
                  <button
                    class="flex-1 px-3 py-1.5 bg-white border border-gray-200 rounded-lg text-[10px] font-bold text-gray-500 transition-all flex items-center justify-center gap-1.5"
                    disabled
                  >
                    <RefreshCw size={10} class="animate-spin" />
                    {ui.loading}
                  </button>
                {:else if runtime === null}
                  <button
                    class="flex-1 px-3 py-1.5 bg-white border border-gray-200 rounded-lg text-[10px] font-bold text-gray-600 hover:bg-gray-50 hover:border-gray-300 transition-all flex items-center justify-center gap-1.5"
                    disabled={runtimeBusyAction === "check" || runtimeBusyAction === "status"}
                    onclick={onRefreshRuntime}
                  >
                    <RefreshCw size={10} class={runtimeBusyAction === "check" ? 'animate-spin' : ''} />
                    {ui.check}
                  </button>
                {:else if !runtime.installed}
                  <button
                    class="flex-1 px-3 py-1.5 bg-amber-600 rounded-lg text-[10px] font-bold text-white hover:bg-amber-700 transition-all flex items-center justify-center gap-1.5 shadow-sm"
                    disabled={readOnly || !runtime?.npmAvailable || runtimeBusyAction === "install"}
                    onclick={onInstallRuntime}
                  >
                    <Plus size={10} />
                    {ui.install}
                  </button>
                {:else if runtime?.updateAvailable === true}
                  <button 
                    class="flex-1 px-3 py-1.5 bg-amber-600 rounded-lg text-[10px] font-bold text-white hover:bg-amber-700 transition-all flex items-center justify-center gap-1.5 shadow-sm"
                    disabled={readOnly || !runtime?.npmAvailable || runtimeBusyAction === "update"}
                    onclick={onUpdateRuntime}
                  >
                    <Zap size={10} />
                    {ui.update}
                  </button>
                {:else}
                  <button 
                    class="flex-1 px-3 py-1.5 bg-white border border-gray-200 rounded-lg text-[10px] font-bold text-gray-600 hover:bg-gray-50 hover:border-gray-300 transition-all flex items-center justify-center gap-1.5"
                    disabled={runtimeBusyAction === "check" || runtimeBusyAction === "status"}
                    onclick={onRefreshRuntime}
                  >
                    <RefreshCw size={10} class={runtimeBusyAction === "check" ? 'animate-spin' : ''} />
                    {ui.check}
                  </button>
                {/if}
              </div>
              <div class="border-t border-gray-200/70 pt-2">
                <button
                  class="w-full px-3 py-1.5 bg-white border border-gray-200 rounded-lg text-[10px] font-bold text-gray-600 hover:bg-gray-50 hover:border-gray-300 transition-all flex items-center justify-center gap-1.5 disabled:cursor-not-allowed disabled:opacity-50"
                  disabled={readOnly || gatewayRestartBusy || !gatewayRestartAvailable}
                  onclick={onRestartGateway}
                  title={ui.restartWebuiDescription}
                  type="button"
                >
                  <RefreshCw size={10} class={gatewayRestartBusy ? 'animate-spin' : ''} />
                  {gatewayRestartBusy ? ui.restartingWebui : ui.restartWebui}
                </button>
              </div>
            </div>
          </div>

          {#if systemShutdownAvailable}
            <div class="space-y-4">
              <div class="sidebar-flyout-surface rounded-2xl border border-gray-200 bg-gray-50/80 p-3 shadow-sm">
                <div class="flex items-start gap-2 sm:gap-3">
                  <div class="sidebar-flyout-icon mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-xl border border-gray-200 bg-white text-gray-500">
                    <Power size={14} />
                  </div>
                  <div class="min-w-0 flex-1">
                    <label class:checkbox-card--disabled={readOnly} class="checkbox-card checkbox-card--compact w-full max-w-full" for="global-shutdown-after-queue">
                      <input
                        class="checkbox-input"
                        checked={systemShutdownArmed}
                        disabled={readOnly}
                        id="global-shutdown-after-queue"
                        onchange={(event) => onSystemShutdownArmedChange((event.currentTarget as HTMLInputElement).checked)}
                        type="checkbox"
                      />
                      <span aria-hidden="true" class="checkbox-control"></span>
                      <span class="checkbox-copy min-w-0">
                        <span class="checkbox-title">{ui.shutdownAfterQueueCompletes}</span>
                        <span class="checkbox-description">{ui.shutdownWaitDescription(systemShutdownDelaySeconds)}</span>
                      </span>
                    </label>
                  </div>
                </div>
              </div>
            </div>
          {/if}
        </div>

        <div class="sidebar-flyout-footer p-2 border-t border-gray-100 bg-gray-50/50 space-y-1">
          <button 
            class="w-full flex items-center gap-2 px-3 py-2 text-xs font-medium text-gray-600 hover:bg-white hover:text-amber-600 rounded-lg transition-all"
            disabled={quotaBusy}
            onclick={onRefreshQuota}
          >
            <RefreshCw size={14} class={quotaBusy ? 'animate-spin' : ''} />
            {ui.refreshQuota}
          </button>
          {#if showPwaInstall}
            <button
              class={`w-full flex items-center gap-2 px-3 py-2 text-xs font-medium rounded-lg transition-all ${
                pwaInstalled
                  ? "text-emerald-600 hover:bg-emerald-50"
                  : "text-gray-600 hover:bg-white hover:text-amber-600"
              }`}
              disabled={pwaInstallBusy || pwaInstalled}
              onclick={onInstallApp}
              type="button"
            >
              {#if pwaInstalled}
                <CheckCircle2 size={14} />
                {ui.appInstalled}
              {:else}
                <Download size={14} />
                {pwaInstallBusy ? ui.installingApp : ui.installApp}
              {/if}
            </button>
          {/if}
          <button 
            class="w-full flex items-center gap-2 px-3 py-2 text-xs font-medium text-gray-600 hover:bg-white hover:text-amber-600 rounded-lg transition-all disabled:cursor-not-allowed disabled:opacity-50"
            disabled={readOnly}
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

  .sidebar-flyout {
    z-index: 150;
  }

  :global(:root[data-theme="dark"]) .sidebar-mode-toggle {
    background: rgba(30, 41, 59, 0.5);
    box-shadow: inset 0 0 0 1px rgba(71, 85, 105, 0.26);
  }

  :global(:root[data-theme="dark"]) .sidebar-mode-toggle__button {
    color: #94a3b8;
  }

	  :global(:root[data-theme="dark"]) .sidebar-mode-toggle__button.bg-white {
	    background: linear-gradient(180deg, rgba(30, 41, 59, 0.96), rgba(15, 23, 42, 0.98)) !important;
	    color: #f8fafc !important;
	    box-shadow:
	      0 14px 28px -24px rgba(2, 6, 23, 0.92),
	      inset 0 0 0 1px rgba(148, 163, 184, 0.14);
	  }

	  :global(:root[data-theme="dark"]) .sidebar-folders {
	    border-color: rgba(71, 85, 105, 0.36) !important;
	    background: linear-gradient(180deg, rgba(17, 24, 39, 0.82), rgba(15, 23, 42, 0.92)) !important;
	    box-shadow:
	      0 18px 38px -32px rgba(2, 6, 23, 0.92),
	      inset 0 0 0 1px rgba(148, 163, 184, 0.08);
	  }

	  :global(:root[data-theme="dark"]) .sidebar-folders-toggle:hover {
	    background: rgba(30, 41, 59, 0.82) !important;
	  }

	  :global(:root[data-theme="dark"]) .sidebar-folder-item {
	    color: #cbd5e1 !important;
	  }

	  :global(:root[data-theme="dark"]) .sidebar-folder-item:hover {
	    background: rgba(30, 41, 59, 0.82) !important;
	    color: #f8fafc !important;
	  }

	  :global(:root[data-theme="dark"]) .sidebar-folder-item--active {
	    background: linear-gradient(180deg, rgba(69, 39, 10, 0.42), rgba(15, 23, 42, 0.88)) !important;
	    color: #fde68a !important;
	    box-shadow: inset 0 0 0 1px rgba(245, 158, 11, 0.24);
	  }

	  :global(:root[data-theme="dark"]) .sidebar-folder-count {
	    background: rgba(30, 41, 59, 0.9) !important;
	    color: #94a3b8 !important;
	  }

	  :global(:root[data-theme="dark"]) .sidebar-folder-item--active .sidebar-folder-count {
	    background: rgba(245, 158, 11, 0.16) !important;
	    color: #fde68a !important;
	  }

	  :global(:root[data-theme="dark"]) .sidebar-folder-action {
	    color: #94a3b8 !important;
	  }

	  :global(:root[data-theme="dark"]) .sidebar-folder-action:hover {
	    background: rgba(51, 65, 85, 0.82) !important;
	    color: #fde68a !important;
	  }

	  :global(:root[data-theme="dark"]) .session-card--attention-stopped {
	    border-color: rgba(148, 163, 184, 0.24) !important;
	    background: linear-gradient(180deg, rgba(30, 41, 59, 0.72), rgba(15, 23, 42, 0.9)) !important;
	    box-shadow:
	      0 18px 34px -32px rgba(2, 6, 23, 0.88),
	      inset 0 0 0 1px rgba(148, 163, 184, 0.08) !important;
	  }

	  :global(:root[data-theme="dark"]) .session-attention-pill--stopped {
	    background: rgba(51, 65, 85, 0.92) !important;
	    color: #cbd5e1 !important;
	    box-shadow: inset 0 0 0 1px rgba(148, 163, 184, 0.18);
	  }

	  :global(:root[data-theme="dark"]) .sidebar-search-trigger {
	    border-color: rgba(71, 85, 105, 0.48) !important;
    background: linear-gradient(180deg, rgba(17, 24, 39, 0.9), rgba(15, 23, 42, 0.96)) !important;
    box-shadow: 0 20px 40px -34px rgba(2, 6, 23, 0.85) !important;
  }

  :global(:root[data-theme="dark"]) .sidebar-search-trigger:hover {
    border-color: rgba(148, 163, 184, 0.32) !important;
  }

  :global(:root[data-theme="dark"]) .sidebar-search-trigger__icon {
    border-color: rgba(71, 85, 105, 0.45) !important;
    background: rgba(15, 23, 42, 0.78) !important;
    color: #94a3b8 !important;
  }

  :global(:root[data-theme="dark"]) .sidebar-search-panel {
    border-color: rgba(245, 158, 11, 0.28) !important;
    background: linear-gradient(180deg, rgba(17, 24, 39, 0.98), rgba(11, 18, 32, 1)) !important;
    box-shadow: 0 28px 64px -40px rgba(2, 6, 23, 0.94) !important;
  }

  :global(:root[data-theme="dark"]) .sidebar-search-input {
    border-color: rgba(71, 85, 105, 0.42) !important;
    background: rgba(15, 23, 42, 0.82) !important;
    color: #e2e8f0 !important;
  }

  :global(:root[data-theme="dark"]) .sidebar-search-input::placeholder {
    color: #64748b !important;
  }

  :global(:root[data-theme="dark"]) .sidebar-search-input:focus {
    background: rgba(15, 23, 42, 0.94) !important;
  }

  :global(:root[data-theme="dark"]) .sidebar-search-clear:hover {
    background: rgba(51, 65, 85, 0.82) !important;
    color: #e2e8f0 !important;
  }

  :global(:root[data-theme="dark"]) .sidebar-search-scope {
    background: rgba(15, 23, 42, 0.82) !important;
    box-shadow: inset 0 0 0 1px rgba(71, 85, 105, 0.22);
  }

  :global(:root[data-theme="dark"]) .sidebar-search-scope__button {
    color: #94a3b8 !important;
  }

  :global(:root[data-theme="dark"]) .sidebar-search-scope__button.bg-white {
    background: rgba(30, 41, 59, 0.96) !important;
    color: #f8fafc !important;
    box-shadow: inset 0 0 0 1px rgba(148, 163, 184, 0.12);
  }

  :global(:root[data-theme="dark"]) .sidebar-search-filter {
    border-color: rgba(71, 85, 105, 0.38) !important;
    background: rgba(15, 23, 42, 0.76) !important;
    color: #cbd5e1 !important;
  }

  :global(:root[data-theme="dark"]) .sidebar-search-filter:hover {
    background: rgba(30, 41, 59, 0.92) !important;
    color: #f8fafc !important;
  }

  :global(:root[data-theme="dark"]) .sidebar-flyout {
    border-color: rgba(71, 85, 105, 0.44) !important;
    background: linear-gradient(180deg, rgba(17, 24, 39, 0.97), rgba(11, 18, 32, 0.995)) !important;
    box-shadow: 0 30px 68px -38px rgba(2, 6, 23, 0.96) !important;
  }

  :global(:root[data-theme="dark"]) .sidebar-flyout-header,
  :global(:root[data-theme="dark"]) .sidebar-flyout-footer {
    border-color: rgba(71, 85, 105, 0.34) !important;
    background: rgba(15, 23, 42, 0.78) !important;
  }

  :global(:root[data-theme="dark"]) .sidebar-flyout .text-gray-900,
  :global(:root[data-theme="dark"]) .sidebar-flyout .text-gray-700 {
    color: #f8fafc !important;
  }

  :global(:root[data-theme="dark"]) .sidebar-flyout .text-gray-600,
  :global(:root[data-theme="dark"]) .sidebar-flyout .text-gray-500,
  :global(:root[data-theme="dark"]) .sidebar-flyout .text-gray-400 {
    color: #94a3b8 !important;
  }

  :global(:root[data-theme="dark"]) .sidebar-flyout-surface {
    border-color: rgba(71, 85, 105, 0.32) !important;
    background: rgba(15, 23, 42, 0.78) !important;
  }

  :global(:root[data-theme="dark"]) .session-account-badge {
    border-color: rgba(71, 85, 105, 0.34) !important;
    background: rgba(15, 23, 42, 0.8) !important;
    color: #94a3b8 !important;
  }

  :global(:root[data-theme="dark"]) .sidebar-flyout input {
    border-color: rgba(71, 85, 105, 0.42) !important;
    background: rgba(15, 23, 42, 0.86) !important;
    color: #f8fafc !important;
  }

  :global(:root[data-theme="dark"]) .sidebar-flyout input::placeholder {
    color: #64748b !important;
  }

  :global(:root[data-theme="dark"]) .sidebar-notification-item {
    color: #e2e8f0 !important;
  }

  :global(:root[data-theme="dark"]) .sidebar-notification-item.border-gray-200 {
    border-color: rgba(71, 85, 105, 0.32) !important;
    background: rgba(15, 23, 42, 0.72) !important;
  }

  :global(:root[data-theme="dark"]) .sidebar-notification-item.border-gray-200:hover {
    background: rgba(30, 41, 59, 0.94) !important;
  }

  :global(:root[data-theme="dark"]) .sidebar-notification-item.border-amber-200 {
    border-color: rgba(245, 158, 11, 0.3) !important;
    background: linear-gradient(180deg, rgba(69, 39, 10, 0.28), rgba(15, 23, 42, 0.9)) !important;
  }

  :global(:root[data-theme="dark"]) .sidebar-account-switcher.border-gray-200,
  :global(:root[data-theme="dark"]) .sidebar-theme-option.border-gray-200 {
    border-color: rgba(71, 85, 105, 0.34) !important;
    background: rgba(15, 23, 42, 0.74) !important;
    color: #cbd5e1 !important;
  }

  :global(:root[data-theme="dark"]) .sidebar-account-switcher.border-gray-200:hover,
  :global(:root[data-theme="dark"]) .sidebar-theme-option.border-gray-200:hover {
    border-color: rgba(148, 163, 184, 0.3) !important;
    background: rgba(30, 41, 59, 0.94) !important;
    color: #f8fafc !important;
  }

  :global(:root[data-theme="dark"]) .sidebar-account-switcher.border-amber-300,
  :global(:root[data-theme="dark"]) .sidebar-theme-option.border-amber-300 {
    border-color: rgba(245, 158, 11, 0.38) !important;
    background: linear-gradient(180deg, rgba(69, 39, 10, 0.32), rgba(15, 23, 42, 0.9)) !important;
    color: #fde68a !important;
  }

  :global(:root[data-theme="dark"]) .sidebar-flyout-badge {
    border-color: rgba(71, 85, 105, 0.3) !important;
    background: rgba(30, 41, 59, 0.92) !important;
    color: #cbd5e1 !important;
  }

  :global(:root[data-theme="dark"]) .sidebar-flyout-badge--warning {
    border-color: rgba(245, 158, 11, 0.34) !important;
    background: rgba(69, 39, 10, 0.34) !important;
    color: #fde68a !important;
  }

  :global(:root[data-theme="dark"]) .sidebar-flyout-select,
  :global(:root[data-theme="dark"]) .sidebar-flyout-code {
    border-color: rgba(71, 85, 105, 0.42) !important;
    background: rgba(15, 23, 42, 0.86) !important;
    color: #f8fafc !important;
  }

  :global(:root[data-theme="dark"]) .sidebar-flyout-meter {
    background: rgba(51, 65, 85, 0.78) !important;
  }

  :global(:root[data-theme="dark"]) .sidebar-flyout-icon {
    border-color: rgba(71, 85, 105, 0.36) !important;
    background: rgba(15, 23, 42, 0.88) !important;
    color: #cbd5e1 !important;
  }
</style>
