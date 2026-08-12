<script lang="ts">
  import {
    Menu,
    Plus,
    X,
    Archive,
    History,
    CheckCircle2,
    Clock,
    User,
    Bot,
    Send,
    Paperclip,
    Settings,
    MoreHorizontal,
    ChevronDown,
    ChevronUp,
    Terminal,
    GitBranch,
    FileDiff,
    Layout,
    Maximize2,
    Minimize2,
    Trash2,
    RotateCcw,
    Zap,
    Cpu,
    ExternalLink,
    AlertCircle,
    FileText,
    ListTodo,
    MessageSquare,
    Pin,
    RefreshCw,
    Search,
    Pencil,
    Copy,
    ArrowRightLeft,
    Shield,
    GripVertical,
    Keyboard,
    Monitor,
    Download,
    UserCog
  } from "lucide-svelte";
  import { onMount, tick } from "svelte";
  import { fly, slide } from "svelte/transition";
  import { portal } from "$lib/actions/portal";
  import { extractAttachmentPaths, stripAttachmentPreamble } from "$lib/attachments";
  import { api } from "$lib/api";
  import {
    applyStreamEvent,
    createConversationState,
    mergeConversationState,
    mergeConversationTurnState,
    normalizeSessionStateProfileId,
    sessionStateKey,
    type ConversationState
  } from "$lib/chat-state";
  import { CODEX_SLASH_COMMANDS, findCodexSlashCommand, type CodexSlashCommandEntry } from "$lib/codex-commands";
  import AuthLoginOverlay from "$lib/components/AuthLoginOverlay.svelte";
  import FolderBrowserDialog from "$lib/components/FolderBrowserDialog.svelte";
  import LazyMonacoDiffEditor from "$lib/components/LazyMonacoDiffEditor.svelte";
  import MarkdownMessage from "$lib/components/MarkdownMessage.svelte";
  import {
    clearSessionBrowserCache,
    readSessionDetailCache,
    readSessionListCache,
    writeSessionDetailCache,
    writeSessionListCache
  } from "$lib/session-browser-cache";
  import {
    sessionCacheModeForStreamEvent,
    sessionCachePersistDelay,
    type SessionCachePersistMode
  } from "$lib/session-cache-policy";
  import {
    observeSessionStreamEvent,
    reconcileSessionStreamBoundary,
    type SessionStreamCursor,
    type SessionStreamCursorResult
  } from "$lib/session-stream-cursor";
  import {
    buildTranscriptLayout,
    computeTranscriptWindowFromLayout,
    EMPTY_TRANSCRIPT_LAYOUT,
    EMPTY_TRANSCRIPT_WINDOW,
    type TranscriptLayout,
    type TranscriptWindowAlignment
  } from "$lib/transcript-window";
  import SessionRecoveryModal from "$lib/components/SessionRecoveryModal.svelte";
  import SessionSidebar from "$lib/components/SessionSidebar.svelte";
  import SessionTurnSearchPopover from "$lib/components/SessionTurnSearchPopover.svelte";
  import StartupAlertModal from "$lib/components/StartupAlertModal.svelte";
  import WorkspaceHeader from "$lib/components/WorkspaceHeader.svelte";
  import WorkspaceTabStrip from "$lib/components/WorkspaceTabStrip.svelte";
  import { isContextWindowExceededPayload, isUsageLimitErrorPayload, parseAppError } from "$lib/errors";
  import { describeUiError } from "$lib/ui-errors";
  import { activeLocale, localeOptions, localeSignal, updateLocale } from "$lib/i18n";
  import { m } from "$lib/paraglide/messages.js";
  import { getLocale } from "$lib/paraglide/runtime.js";
  import { anchoredPopoverStyle } from "$lib/popover-position";
  import {
    applyThemeSettings,
    applyThemeMode,
    getResolvedTheme,
    readThemeMode,
    subscribeThemeChange,
    type ResolvedTheme,
    type ThemeMode
  } from "$lib/theme";
  import type { ThemeSettings } from "$lib/theme-customization";
  import type {
    AutomationDefinition,
    AppNotification,
    AppConfigPayload,
    CatalogPayload,
    AttachmentRecord,
    CodexAccountLoginFlow,
    CodexItem,
    ComputerFramePayload,
    ComputerInputEvent,
    CodexProfileAccountSummary,
    CodexQuotaStatus,
    CodexResetTicket,
    CodexResetTicketsPayload,
    CodexRuntimeStatus,
    CodexTurn,
    DirectoryPayload,
    FileMentionSearchEntry,
    GitCommit,
    GitOpenRequest,
    GlobalStreamEvent,
    LoginHcaptchaConfig,
    NotificationSettings,
    PendingServerRequest,
    PromptPreset,
    SavedSessionFilter,
    SelectedSkill,
    SessionListPayload,
    SessionListPatchPayload,
    SessionListResponse,
    SessionDetailPatchPayload,
    SessionDetailPayload,
    SessionDetailResponse,
    SessionLatestCompletedTurnPayload,
    SessionPreferences,
    SessionQueueItem,
    SessionQueuePayload,
    SessionReviewTarget,
    SessionRollbackTarget,
    SessionRollbackTargetsPayload,
    SessionRolloutRecoveryPayload,
    SessionSearchScope,
    SessionFolder,
    SessionSummaryFilter,
    SessionSummary,
    SessionTurnSearchMatch,
    SessionTurnsPagePayload,
    StreamEvent,
    TerminalContextPayload,
    TerminalSummary,
    SkillCatalogEntry,
    UserRole,
    WsConnectionState
  } from "$lib/types";

  const SESSION_LIST_BROWSER_CACHE_SCHEMA_VERSION = 2;
  const SESSION_DETAIL_BROWSER_CACHE_SCHEMA_VERSION = 2;
  const SESSION_LIST_VERSION_HINT_LIMIT = 240;
  const SESSION_DETAIL_VERSION_HINT_LIMIT = 5_000;
  const transcriptTurnEstimatedHeight = 420;
  const transcriptTurnGap = 48;
  const transcriptTurnOverscan = 1_200;
  const transcriptTurnMountLimit = 48;

  type WorkspaceTabId =
    | "chat"
    | "tasks"
    | "git"
    | "settings"
    | "computer"
    | "diagnostics"
    | "memory"
    | `git-diff:${string}`
    | `code-diff:${string}`
    | `file:${string}`
    | `terminal:${string}`;
  type ComposerSettingsTabId = "session" | "security" | "skills";
  type TranscriptScrollAnchor = {
    turnId: string;
    viewportOffset: number;
  };
  type GitDiffTab = {
    id: `git-diff:${string}`;
    repoPath: string;
    filePath: string | null;
    filePaths: string[] | null;
    label: string;
    request: GitOpenRequest;
  };
  type CodeDiffTab = {
    id: `code-diff:${string}`;
    label: string;
    title: string;
    views: FileChangeView[];
  };
  type FileTab = {
    id: `file:${string}`;
    path: string;
    label: string;
  };
  type SubagentTaskEntry = {
    key: string;
    turnId: string;
    itemId: string;
    tool: string;
    status: string;
    prompt: string;
    model: string;
    reasoningEffort: string;
    primaryThreadId: string | null;
    states: Array<[string, { status?: string; message?: string | null }]>;
  };
  type OptimisticMessageState = {
    sessionId: string;
    profileId: string | null;
    clientUserMessageId: string;
    prompt: string;
    skills: SelectedSkill[];
    attachmentNames: string[];
    createdAt: number;
    baselineTurnId: string | null;
    baselineTurnCount: number;
  };
  type PendingSteerResumeState = {
    sessionId: string;
    profileId: string | null;
    draft: string;
    updatedAt: number | null;
  };
  type PendingEnqueueState = {
    sessionId: string;
    profileId: string | null;
    optimisticQueueId: string;
    item: SessionQueueItem;
    deleted: boolean;
    edited: boolean;
  };
  type SessionRecoveryPromptState = {
    sessionId: string;
    message: string;
    issue: string | null;
    totalLines: number | null;
    recoverableLines: number | null;
    skippedLines: number | null;
    busy: boolean;
  };
  type ManualCompactPromptState = {
    sessionId: string;
    message: string;
    busy: boolean;
  };
  type ArenaWorkspaceComponent = typeof import("$lib/components/ArenaWorkspace.svelte").default;
  type CodeDiffWorkspaceComponent = typeof import("$lib/components/CodeDiffWorkspace.svelte").default;
  type DiagnosticsWorkspaceComponent = typeof import("$lib/components/DiagnosticsWorkspace.svelte").default;
  type FileWorkspaceComponent = typeof import("$lib/components/FileWorkspace.svelte").default;
  type GitWorkspaceComponent = typeof import("$lib/components/GitWorkspace.svelte").default;
  type MemoryWorkspaceComponent = typeof import("$lib/components/MemoryWorkspace.svelte").default;
  type SettingsWorkspaceComponent = typeof import("$lib/components/SettingsWorkspace.svelte").default;
  type TerminalWorkspaceComponent = typeof import("$lib/components/TerminalWorkspace.svelte").default;
  type LazyWorkspaceKind = "arena" | "codeDiff" | "diagnostics" | "file" | "git" | "memory" | "settings" | "terminal";
  type SlashSuggestion = {
    key: string;
    command: string;
    title: string;
    description: string;
    value: string;
    support?: CodexSlashCommandEntry["support"];
  };
  type FileMentionTrigger = {
    start: number;
    end: number;
    query: string;
  };
  type ThemedConfigPayload = AppConfigPayload & { theme?: ThemeSettings };
  type BeforeInstallPromptEvent = Event & {
    prompt: () => Promise<void>;
    userChoice: Promise<{
      outcome: "accepted" | "dismissed";
      platform: string;
    }>;
  };
  const HUNDRED_M_CONTEXT_WINDOW = 100_000_000;
  const LOCAL_QUEUE_MODE_GRACE_MS = 120_000;
  const sessionPageSize = 20;
  const SESSION_DETAIL_CACHE_INLINE_IMAGE_RESULT_MAX_CHARS = 256 * 1024;

  let config = $state<AppConfigPayload | null>(null);
  let catalog = $state<CatalogPayload | null>(null);
  let quota = $state<CodexQuotaStatus | null>(null);
  let profileAccounts = $state<CodexProfileAccountSummary[]>([]);
  let profileAccountsBusy = $state(false);
  let resetTickets = $state<CodexResetTicketsPayload | null>(null);
  let runtime = $state<CodexRuntimeStatus | null>(null);
  let remoteControlStatus = $state<{ status: string; environmentId: string | null; updatedAt: number } | null>(null);
  let computerFramesBySessionId = $state<Record<string, ComputerFramePayload>>({});
  let computerInputText = $state("");
  let computerInputBusy = $state(false);
  let defaultLanguageBridgeBusy = $state(false);
  let computerInputStatus = $state<string | null>(null);
  let dismissedRemoteControlErrorAt = $state(0);
  let sessions = $state<SessionSummary[]>([]);
  let notifications = $state<AppNotification[]>([]);
  let sessionsCursor = $state<string | null>(null);
  let sessionsHasMore = $state(false);
  let sessionsLoadingMore = $state(false);
  let conversation = $state<ConversationState | null>(null);
  let selectedSessionId = $state<string | null>(null);
  let selectedSessionProfileId = $state<string | null>(null);
  let sessionProfileIdsBySessionId = $state<Record<string, string>>({});
  let profileMoveDialogSession = $state<SessionSummary | null>(null);
  let profileMoveDialogTargetId = $state("");
  let activeProfileId = $state<string | null>(null);
  let authenticated = $state<boolean | null>(null);
  let webRole = $state<UserRole | null>(null);
  let loading = $state(true);
  let loadingDetail = $state(false);
  let manualSessionResyncSessionId = $state<string | null>(null);
  let sessionsBusy = $state(false);
  let sending = $state(false);
  let startingMessage = $state(false);
  let submitComposerBusy = $state(false);
  let uploading = $state(false);
  let errorText = $state("");
  let noticeText = $state("");
  let loginPassword = $state("");
  let loginBusy = $state(false);
  let loginMessage = $state("");
  let loginHcaptcha = $state<LoginHcaptchaConfig>({ enabled: false, siteKey: null });
  let loginHcaptchaToken = $state("");
  let loginHcaptchaWidgetId = $state<string | number | null>(null);
  let loginHcaptchaContainer = $state<HTMLDivElement | null>(null);
  let draft = $state("");
  let draftAttachments = $state<AttachmentRecord[]>([]);
  let fileMentionTrigger = $state<FileMentionTrigger | null>(null);
  let fileMentionResults = $state<FileMentionSearchEntry[]>([]);
  let fileMentionBusy = $state(false);
  let fileMentionActiveIndex = $state(0);
  let fileMentionRequestVersion = 0;
  let fileMentionSearchTimer: ReturnType<typeof setTimeout> | null = null;
  let titleDraft = $state("");
  let browserOpen = $state(false);
  let browserBusy = $state(false);
  let runtimeBusyAction = $state<"install" | "update" | "check" | "status" | null>(null);
  let gatewayRestartBusy = $state(false);
  let quotaBusy = $state(false);
  let quotaRefreshPromise: Promise<void> | null = null;
  let quotaForceRefreshQueued = false;
  let profileAccountsRefreshPromise: Promise<void> | null = null;
  let profileAccountsForceRefreshQueued = false;
  let resetTicketsBusy = $state(false);
  let resetTicketsRefreshPromise: Promise<void> | null = null;
  let resetTicketsForceRefreshQueued = false;
  let resetTicketUseBusyId = $state<string | null>(null);
  let notificationsBusy = $state(false);
  let directoryPayload = $state<DirectoryPayload | null>(null);
  let requestAnswers = $state<Record<string, Record<string, string>>>({});
  let rawRequestResponses = $state<Record<string, string>>({});
  let pendingSessionEvents = $state<Record<string, StreamEvent[]>>({});
  let expandedItems = $state<Record<string, boolean>>({});
  let expandedFileChangeEntries = $state<Record<string, boolean>>({});
  let loadingItemDetails = $state<Record<string, boolean>>({});
  let itemDetailErrors = $state<Record<string, string>>({});
  let expandedTurnLogs = $state<Record<string, boolean>>({});
  let turnEntryRenderLimits = $state<Record<string, number>>({});
  let expandedLargeOutputs = $state<Record<string, boolean>>({});
  let loadingTurns = $state<Record<string, boolean>>({});
  let turnLoadErrors = $state<Record<string, string>>({});
  let sessionSearchQuery = $state("");
  let sessionSearchScope = $state<SessionSearchScope>("summary");
  let sessionFilter = $state<SessionSummaryFilter>({
    pinnedOnly: false,
    runningOnly: false,
    queuedOnly: false,
    untaggedOnly: false,
    highlight: "all",
    tags: []
  });
  let activeSavedSessionFilterId = $state<string | null>(null);
  let activeSessionFolder = $state<string | null>(null);
  let showArchivedSessions = $state(false);
  let accountLoginFlow = $state<CodexAccountLoginFlow | null>(null);
  let composerSettingsOpen = $state(false);
  let composerSettingsTab = $state<ComposerSettingsTabId>("session");
  let composerSettingsAnchor = $state<ComposerSettingsTabId>("session");
  let catalogLoading = $state(false);
  let composerSkillQuery = $state("");
  let draftSelectedSkills = $state<SelectedSkill[]>([]);
  let connectionState = $state<WsConnectionState>("idle");
  let themeMode = $state<ThemeMode>("system");
  let resolvedTheme = $state<ResolvedTheme>("light");
  let loadingOlderTurns = $state(false);
  let olderTurnsAutoLoadEnabled = $state(true);
  let rollbackTargetsOpen = $state(false);
  let rollbackTargetsLoading = $state(false);
  let rollbackTargetsPayload = $state<SessionRollbackTargetsPayload | null>(null);
  let rollbackTargetsSessionId = $state<string | null>(null);
  let rollbackTargetsError = $state("");
  let rollbackTargetsResetSessionId = $state<string | null>(null);
  let olderTurnsAutoLoadPaused = $state(false);
  let olderTurnsAutoTriggerTimestamps = $state<number[]>([]);
  let terminals = $state<TerminalSummary[]>([]);
  let activeWorkspaceTabId = $state<WorkspaceTabId>("chat");
  let ArenaWorkspaceView = $state<ArenaWorkspaceComponent | null>(null);
  let CodeDiffWorkspaceView = $state<CodeDiffWorkspaceComponent | null>(null);
  let DiagnosticsWorkspaceView = $state<DiagnosticsWorkspaceComponent | null>(null);
  let FileWorkspaceView = $state<FileWorkspaceComponent | null>(null);
  let GitWorkspaceView = $state<GitWorkspaceComponent | null>(null);
  let MemoryWorkspaceView = $state<MemoryWorkspaceComponent | null>(null);
  let SettingsWorkspaceView = $state<SettingsWorkspaceComponent | null>(null);
  let TerminalWorkspaceView = $state<TerminalWorkspaceComponent | null>(null);
  let lazyWorkspaceLoadErrors = $state<Partial<Record<LazyWorkspaceKind, string>>>({});
  let workspaceMenuOpen = $state(false);
  let tasksTabOpen = $state(false);
  let gitTabOpen = $state(false);
  let gitOpenRequest = $state<GitOpenRequest | null>(null);
  let settingsTabOpen = $state(false);
  let computerTabOpen = $state(false);
  let diagnosticsTabOpen = $state(false);
  let memoryTabOpen = $state(false);
  let settingsInitialTab = $state<"config" | "defaults" | "startup" | "audit" | "theme" | "notifications" | "presets" | "automations" | "apps" | "plugins" | "skills" | "mcp" | null>(null);
  let gitDiffTabs = $state<GitDiffTab[]>([]);
  let codeDiffTabs = $state<CodeDiffTab[]>([]);
  let fileTabs = $state<FileTab[]>([]);
  let viewerGitRepoPath = $state<string | null>(null);
  let pendingSteerResume = $state<PendingSteerResumeState | null>(null);
  let dismissedQueueResumeBySessionId = $state<Record<string, boolean>>({});
  let draftSaveTimer: ReturnType<typeof setTimeout> | null = null;
  let draftPersistencePaused = $state(false);
  let mobileSidebarOpen = $state(false);
  let isMobileLayout = $state(false);
  let optimisticMessage = $state<OptimisticMessageState | null>(null);
  let optimisticQueuedItemsBySessionId = $state<Record<string, SessionQueueItem[]>>({});
  let sessionQueueSnapshotsBySessionId = $state<Record<string, SessionQueuePayload>>({});
  let queuedMessageRequestCountsBySessionId = $state<Record<string, number>>({});
  let pendingQueueModeSessionKey = $state<string | null>(null);
  let pendingQueueModeActivatedAt = $state(0);
  let liveTurnCardExpanded = $state(false);
  let sendIntent = $state<"message" | "steer" | "queue" | null>(null);
  let editingQueueId = $state<string | null>(null);
  let editingQueuePrompt = $state("");
  let queuedFollowupsExpanded = $state(true);
  let queueReorderBusy = $state(false);
  let queueDragState = $state<{
    pointerId: number;
    queueId: string;
    targetQueueId: string | null;
    targetPosition: "before" | "after" | null;
  } | null>(null);
  let sessionTurnSearchOpen = $state(false);
  let sessionTurnSearchQuery = $state("");
  let sessionTurnSearchResults = $state<SessionTurnSearchMatch[]>([]);
  let sessionTurnSearchCursor = $state<string | null>(null);
  let sessionTurnSearchTotalMatches = $state(0);
  let sessionTurnSearchBusy = $state(false);
  let sessionTurnSearchLoadingMore = $state(false);
  let sessionTurnSearchError = $state("");
  let sessionTurnSearchFocusedTurnId = $state<string | null>(null);
  let sessionTurnSearchJumpingTurnId = $state<string | null>(null);
  let staleSessionCatchup = $state<{
    sessionId: string;
    hiddenDurationMs: number;
    eventCount: number;
    refreshing: boolean;
    refreshRetries: number;
  } | null>(null);
  let startupAlertModalOpen = $state(false);
  let startupAlertDismissed = $state(false);
  let startupAlertInitialConfigHandled = $state(false);
  let sessionRecoveryPrompt = $state<SessionRecoveryPromptState | null>(null);
  let dismissedSessionRecoveryPromptForSessionId = $state<string | null>(null);
  let manualCompactPrompt = $state<ManualCompactPromptState | null>(null);
  let dismissedManualCompactPromptForSessionId = $state<string | null>(null);
  let startupAlertNow = $state(Date.now());
  let deferredInstallPrompt = $state<BeforeInstallPromptEvent | null>(null);
  let pwaInstalled = $state(false);
  let pwaInstallBusy = $state(false);
  let pwaManualInstallOnly = $state(false);
  let sessionListCacheKey = $state<string | null>(null);
  let sessionListCacheVersion = $state<string | null>(null);
  let sessionListStateHash = $state<string | null>(null);
  let sessionListWindowKey = $state<string | null>(null);
  let sessionListRequestedLimit = $state(sessionPageSize);
  let sessionSummaryVersionsById = $state<Record<string, string>>({});
  let sessionDetailCacheVersion = $state<string | null>(null);
  let sessionDetailStateHash = $state<string | null>(null);
  let sessionDetailMetadataVersion = $state<string | null>(null);
  let sessionTurnVersionsById = $state<Record<string, string>>({});
  const readOnlyRole = $derived(webRole === "viewer");

  $effect(() => {
    selectedSessionId;
    viewerGitRepoPath = null;
  });

  $effect(() => {
    if (!staleSessionCatchup) {
      return;
    }
    if (selectedSessionId && staleSessionCatchup.sessionId === selectedSessionId) {
      return;
    }
    clearStaleSessionCatchup();
  });

  type FileChangeView = {
    path: string;
    kind: "add" | "delete" | "update";
    movePath: string | null;
    diff: string;
    original: string;
    modified: string;
    renderable: boolean;
  };
  type FileChangeSummaryEntry = {
    path: string;
    kind: "add" | "delete" | "update";
    movePath: string | null;
  };
  type RenderableTurnEntry =
    | {
        kind: "item";
        key: string;
        item: CodexItem;
      }
    | {
        kind: "fileChangeGroup";
        key: string;
        items: CodexItem[];
      }
    | {
        kind: "readGroup";
        key: string;
        items: CodexItem[];
      };
  type TurnRenderModel = {
    userItems: CodexItem[];
    finalAgentItem: CodexItem | null;
    collapsedItems: CodexItem[];
    collapsedEntries: RenderableTurnEntry[];
    visibleSummaryEntries: RenderableTurnEntry[];
    fullEntries: RenderableTurnEntry[];
  };

  const rawRequestPlaceholder = '{"action":"decline","content":null}';
  const readOnlyParsedCommandTypes = new Set(["read", "list_files", "search"]);
  const scrollBottomThreshold = 48;
  const transcriptTopLoadThreshold = 96;
  const olderTurnPageSize = 20;
  const olderTurnAutoLoadWindowMs = 1500;
  const olderTurnAutoLoadBurstLimit = 3;
  const staleSessionCatchupHiddenThresholdMs = 45_000;
  const staleSessionCatchupEventThreshold = 40;
  const staleSessionCatchupWindowMs = 7_500;
  const activeSessionStatusPollMs = 15_000;
  const initialTurnEntryRenderLimit = 80;
  const turnEntryRenderIncrement = 80;
  const largeMarkdownInitialChars = 18_000;
  const compactMarkdownInitialChars = 8_000;
  const toolOutputInitialChars = 24_000;
  const composerTextareaMinHeight = 52;
  const sessionQueryParamKey = "session";
  const sessionNewParamKey = "sessionNew";
  const sessionSearchQueryParamKey = "sessionSearch";
  const sessionSearchScopeParamKey = "sessionSearchScope";
  const sessionArchivedParamKey = "sessionArchived";
  const sessionFilterPinnedParamKey = "sessionPinned";
  const sessionFilterRunningParamKey = "sessionRunning";
  const sessionFilterQueuedParamKey = "sessionQueued";
  const sessionFilterUntaggedParamKey = "sessionUntagged";
  const sessionFilterHighlightParamKey = "sessionHighlight";
  const sessionFilterTagParamKey = "sessionTag";
  const sessionFolderParamKey = "sessionFolder";
  const sessionSavedFilterParamKey = "sessionSavedFilter";
  const notificationPromptStorageKey = "codex-webui.notifications.permission-prompted";
  const sendOnEnterPreferenceStorageKey = "codex-webui.composer.send-on-enter";
  let loginHcaptchaScriptPromise: Promise<void> | null = null;

  const ui = $derived.by(() => {
    const _locale = $localeSignal;
    const locale = $activeLocale;

    return {
      appTitle: m.app_title(),
      loginTitle: m.login_page_title(),
      privateGateway: m.private_gateway(),
      loginLede: m.login_lede(),
      password: m.password(),
      signIn: m.sign_in(),
      signingIn: m.signing_in(),
      enterPassword: m.enter_password(),
      loginFailed: m.login_failed(),
      completeHcaptcha: m.complete_hcaptcha(),
      hcaptchaLoadFailed: m.hcaptcha_load_failed(),
      installApp: m.install_app(),
      installingApp: m.installing_app(),
      appInstalled: m.app_installed(),
      appAlreadyInstalled: m.app_already_installed(),
      appInstallUnavailable: m.app_install_unavailable(),
      appInstallIosHint: m.app_install_ios_hint(),
      appInstalledNotice: m.app_installed_notice(),
      appInstallPromptDismissed: m.app_install_prompt_dismissed(),
      restartingWebui: m.restarting_webui(),
      restartWebuiNotice: m.restart_webui_notice(),
      language: m.language(),
      close: m.close(),
      newThread: m.new_thread(),
      loadingOlderTurns: m.loading_previous_conversation_history(),
      loadingSessions: m.loading_sessions(),
      refreshingSessions: m.refreshing_sessions(),
      loadingMoreSessions: m.loading_more_sessions(),
      loadingSessionBasics: m.loading_session_basics(),
      loadingConversationProgressive: m.loading_conversation_progressive(),
      reconnecting: m.reconnecting(),
      connectingRealtime: m.connecting_realtime(),
      realtimeDisconnected: m.realtime_disconnected(),
      queueingFollowUp: m.queueing_follow_up(),
      steeringCurrentTurn: m.steering_current_turn(),
      generatingResponse: m.generating_response(),
      chat: m.chat(),
      tasks: m.tasks(),
      gitWorkspace: m.git_workspace(),
      settings: m.settings(),
      settingsSkills: m.settings_skills(),
      installedSkills: m.installed_skills(),
      noSkills: m.no_local_skills(),
      newTerminal: m.new_terminal(),
      diagnostics:
        locale === "ko"
          ? "진단"
          : locale === "ja"
            ? "Diagnostics"
            : locale === "zh-Hans"
              ? "Diagnostics"
              : locale === "zh-Hant"
              ? "Diagnostics"
              : "Diagnostics",
      memory: locale === "ko" ? "메모리" : "Memory",
      computer: m.computer(),
      computerSnapshotStream: m.computer_snapshot_stream(),
      computerNoFrames: m.computer_no_frames(),
      computerFrameUpdated: m.computer_frame_updated(),
      computerInputHint: m.computer_input_hint(),
      computerClickHint: m.computer_click_hint(),
      computerInputDelivered: m.computer_input_delivered(),
      computerInputFailed: m.computer_input_failed(),
      sendComputerInput: m.send_computer_input(),
      scrollUp: m.scroll_up(),
      scrollDown: m.scroll_down(),
      threadTitle: m.thread_title(),
      restoreThread: m.restore_thread(),
      archiveThread: m.archive_thread(),
      open: m.open(),
      moveSessionToAccount: locale === "ko" ? "세션 계정 이동" : "Move session to account",
      moveSessionToAccountDescription:
        locale === "ko"
          ? "이 세션을 처리할 Codex 계정을 선택하세요. 세션 파일은 대상 프로필로 이동됩니다."
          : "Choose the Codex account that should own this session. The session files will be moved to the selected profile.",
      currentAccount: locale === "ko" ? "현재 계정" : "Current account",
      targetAccount: locale === "ko" ? "대상 계정" : "Target account",
      moveSession: locale === "ko" ? "이동" : "Move",
      refresh: locale === "ko" ? "새로고침" : "Refresh",
      loadingHistory: m.loading_history(),
      historyAvailable: m.history_available(),
      autoLoadPaused: m.auto_load_paused(),
      resumeAutoLoad: m.resume_auto_load(),
      loadOlderTurns: m.load_older_turns(),
      copyMessage: m.copy_message(),
      editInComposer: m.edit_in_composer(),
      branchIntoNewThread: m.branch_into_new_thread(),
      handoffToNewThread: m.handoff_to_new_thread(),
      rollbackToThisTurn: m.rollback_to_this_turn(),
      rollbackConfirm: m.rollback_confirm(),
      rollbackTargets:
        locale === "ko"
          ? "롤백 대상"
          : locale === "ja"
            ? "Rollback targets"
            : locale === "zh-Hans"
              ? "Rollback targets"
              : locale === "zh-Hant"
                ? "Rollback targets"
                : "Rollback targets",
      rollbackTargetsHint:
        locale === "ko"
          ? "되돌릴 기준 메시지를 선택합니다. 파일 변경 자체는 되돌리지 않습니다."
          : "Choose the message to roll back to. File changes are not reverted.",
      rollbackTargetsLoading:
        locale === "ko"
          ? "롤백 대상을 불러오는 중"
          : "Loading rollback targets",
      rollbackTargetsEmpty:
        locale === "ko"
          ? "되돌릴 수 있는 이전 메시지가 없습니다."
          : "No earlier messages can be rolled back to.",
      rollbackPreviewIncomplete:
        locale === "ko"
          ? "일부 이전 턴은 아직 로드되지 않아 파일 변경 미리보기가 불완전할 수 있습니다."
          : "Some earlier turns are not loaded, so the file-change preview may be incomplete.",
      rollbackTurnsCount:
        locale === "ko"
          ? "제거할 턴"
          : "Turns to remove",
      userMessageTimestamp:
        locale === "ko"
          ? "보낸 시각"
          : locale === "ja"
            ? "送信時刻"
            : locale === "zh-Hans"
              ? "发送时间"
              : locale === "zh-Hant"
                ? "傳送時間"
                : locale === "fr"
                  ? "Envoyé"
                  : locale === "es"
                    ? "Enviado"
                    : locale === "de"
                      ? "Gesendet"
                      : locale === "it"
                        ? "Inviato"
                        : locale === "pt-BR"
                          ? "Enviado"
                          : locale === "ru"
                            ? "Отправлено"
                            : "Sent",
      agentReplyTimestamp:
        locale === "ko"
          ? "응답 시각"
          : locale === "ja"
            ? "返信時刻"
            : locale === "zh-Hans"
              ? "回复时间"
              : locale === "zh-Hant"
                ? "回覆時間"
                : locale === "fr"
                  ? "Réponse"
                  : locale === "es"
                    ? "Respuesta"
                    : locale === "de"
                      ? "Antwort"
                      : locale === "it"
                        ? "Risposta"
                        : locale === "pt-BR"
                          ? "Resposta"
                          : locale === "ru"
                            ? "Ответ"
                            : "Reply",
      livePlan: m.live_plan(),
      aggregatedDiff: m.aggregated_diff(),
      openTab: m.open_tab(),
      openInNewTab: m.open_in_new_tab(),
      approvalRequired: m.approval_required(),
      inputRequired: m.input_required(),
      typeYourResponse: m.type_your_response(),
      submitResponse: m.submit_response(),
      savedDraftFound: m.saved_draft_found(),
      resumeSavedSteeringPrompt: m.resume_saved_steering_prompt(),
      resume: m.resume(),
      keepDraft: m.keep_draft(),
      queuedWorkPaused: m.queued_work_paused(),
      startupAlertTitle: m.startup_alert_title(),
      startupAlertDescription: m.startup_alert_description(),
      startupAlertPausedQueues: m.startup_alert_paused_queues(),
      startupAlertPausedQueuesDescription: m.startup_alert_paused_queues_description(),
      startupAlertScheduledShutdown: m.startup_alert_scheduled_shutdown(),
      startupAlertOpenThread: m.startup_alert_open_thread(),
      startupAlertContinue: m.startup_alert_continue(),
      startupAlertPendingTasks: (count: number) => m.startup_alert_pending_tasks({ count: String(count) }),
      startupAlertShutdownCountdown: (seconds: number) =>
        m.startup_alert_shutdown_countdown({ seconds: String(seconds) }),
      startupAlertShutdownThread: (name: string) => m.startup_alert_shutdown_thread({ name }),
      resumeQueue: m.resume_queue(),
      ignore: m.ignore(),
      queuedFollowups: m.queued_followups(),
      paused: m.paused(),
      reorderQueue: locale === "ko" ? "대기 메시지 순서 변경" : "Reorder queued message",
      queueSave: m.save(),
      edit: m.edit(),
      cancel: m.close(),
      steerNow: m.steer_now(),
      sendNow: m.send_now(),
      liveTurn: m.live_turn(),
      composerSettings: m.composer_settings(),
      sendShortcut:
        locale === "ko"
          ? "전송 키"
          : locale === "ja"
            ? "送信キー"
            : locale === "zh-Hans"
              ? "发送按键"
              : locale === "zh-Hant"
                ? "傳送按鍵"
                : locale === "fr"
                  ? "Touche d'envoi"
                  : locale === "es"
                    ? "Tecla de envío"
                    : locale === "de"
                      ? "Senden-Taste"
                      : locale === "it"
                        ? "Tasto di invio"
                        : locale === "pt-BR"
                          ? "Tecla de envio"
                          : locale === "ru"
                            ? "Клавиша отправки"
                            : "Send key",
      sendShortcutDescription:
        locale === "ko"
          ? "Shift+Enter는 줄바꿈으로 유지됩니다."
          : locale === "ja"
            ? "Shift+Enter で改行します。"
            : locale === "zh-Hans"
              ? "Shift+Enter 仍然插入换行。"
              : locale === "zh-Hant"
                ? "Shift+Enter 仍會插入換行。"
                : locale === "fr"
                  ? "Shift+Enter insère toujours un retour à la ligne."
                  : locale === "es"
                    ? "Shift+Enter sigue insertando un salto de línea."
                    : locale === "de"
                      ? "Shift+Enter fügt weiterhin einen Zeilenumbruch ein."
                      : locale === "it"
                        ? "Shift+Invio continua a inserire una nuova riga."
                        : locale === "pt-BR"
                          ? "Shift+Enter continua inserindo uma nova linha."
                          : locale === "ru"
                            ? "Shift+Enter по-прежнему вставляет новую строку."
                            : "Shift+Enter still inserts a new line.",
      sendShortcutCtrlEnter:
        locale === "ko"
          ? "Ctrl+Enter"
          : locale === "ja"
            ? "Ctrl+Enter"
            : locale === "zh-Hans"
              ? "Ctrl+Enter"
              : locale === "zh-Hant"
                ? "Ctrl+Enter"
                : locale === "fr"
                  ? "Ctrl+Enter"
                  : locale === "es"
                    ? "Ctrl+Enter"
                    : locale === "de"
                      ? "Strg+Enter"
                      : locale === "it"
                        ? "Ctrl+Invio"
                        : locale === "pt-BR"
                          ? "Ctrl+Enter"
                          : locale === "ru"
                            ? "Ctrl+Enter"
                            : "Ctrl+Enter",
      sendShortcutEnter:
        locale === "ko"
          ? "Enter"
          : locale === "ja"
            ? "Enter"
            : locale === "zh-Hans"
              ? "Enter"
              : locale === "zh-Hant"
                ? "Enter"
                : locale === "fr"
                  ? "Entrée"
                  : locale === "es"
                    ? "Enter"
                    : locale === "de"
                      ? "Enter"
                      : locale === "it"
                        ? "Invio"
                        : locale === "pt-BR"
                          ? "Enter"
                          : locale === "ru"
                            ? "Enter"
                            : "Enter",
      sessionResyncing:
        locale === "ko"
          ? "세션 상태를 다시 동기화하는 중입니다."
          : locale === "ja"
            ? "セッション状態を再同期しています。"
            : locale === "zh-Hans"
              ? "正在重新同步会话状态。"
              : locale === "zh-Hant"
                ? "正在重新同步對話狀態。"
                : locale === "fr"
                  ? "Resynchronisation de l'état de la session."
                  : locale === "es"
                    ? "Resincronizando el estado de la sesión."
                    : locale === "de"
                      ? "Sitzungsstatus wird erneut synchronisiert."
                      : locale === "it"
                        ? "Sincronizzazione dello stato della sessione in corso."
                        : locale === "pt-BR"
                          ? "Ressincronizando o estado da sessão."
                          : locale === "ru"
                            ? "Повторная синхронизация состояния сессии."
                            : "Resynchronizing session state.",
      securitySession: m.security_session(),
      addAttachments: m.add_attachments(),
      selectFolder: m.select_folder(),
      stop: m.stop(),
      steer: m.steer(),
      queue: m.queue(),
      send: m.send(),
      askCodex: m.ask_codex(),
      queueFollowUpPlaceholder: m.queue_follow_up_placeholder(),
      model: m.model(),
      personality: m.personality(),
      personalityFriendly: m.personality_friendly(),
      personalityPragmatic: m.personality_pragmatic(),
      personalityNone: m.personality_none(),
      speed: m.speed(),
      planMode: m.plan_mode(),
      autoDefault: m.auto_default(),
      speedAuto: m.speed_auto(),
      speedFast: m.speed_fast(),
      speedFlex: m.speed_flex(),
      languageBridge:
        locale === "ko"
          ? "언어 브리지"
          : locale === "ja"
            ? "Language bridge"
            : locale === "zh-Hans"
              ? "Language bridge"
              : locale === "zh-Hant"
                ? "Language bridge"
                : "Language bridge",
      languageBridgeDescription:
        locale === "ko"
          ? "비영어 입력은 영어로 내부 처리하고 최종 답변만 지정 언어로 맞춥니다."
          : "Handle non-English prompts in English internally and keep the final answer in the selected language.",
      languageBridgeOutput:
        locale === "ko" ? "최종 답변 언어" : "Final answer language",
      languageBridgeAuto:
        locale === "ko" ? "자동: 마지막 사용자 메시지 언어" : "Auto: latest user message language",
      slashPersonalityDescription: m.slash_personality_description(),
      slashPersonalityUpdated: (personality: string) => m.slash_personality_updated({ personality }),
      slashPersonalityInvalid: m.slash_personality_invalid(),
      approvalMode: m.approval_mode(),
      manual: m.manual(),
      autoOnce: m.auto_once(),
      autoSession: m.auto_session(),
      allowNetworkAccess: m.allow_network_access(),
      shutdownAfterQueueCompletes: m.shutdown_after_queue_completes(),
      taskCenter: m.task_center(),
      subagentActivities: m.subagent_activities(),
      noActiveTasks: m.no_active_tasks(),
      openThread: m.open_thread(),
      closeThreadList: m.close_thread_list(),
      shutdownScheduledNotice: (seconds: number) => m.shutdown_scheduled_notice({ seconds: String(seconds) }),
      invalidJsonResponse: (message: string) => m.invalid_json_response({ message }),
      active: m.active(),
      done: m.done(),
      copyReply: m.copy_reply(),
      plannedStrategy: m.planned_strategy(),
      executing: m.executing(),
      fetching: m.fetching(),
      noAdditionalOutput: m.no_additional_output(),
      subagentInvocation: m.subagent_invocation(),
      viewThread: m.view_thread(),
      instructions: m.instructions(),
      opsCount: (count: number) => m.ops_count({ count: String(count) }),
      readingFileData: m.reading_file_data(),
      computingDiffs: m.computing_diffs(),
      contextCompression: m.context_compression(),
      contextCompressionInProgress: m.context_compression_in_progress(),
      contextCompressionCompleted: m.context_compression_completed(),
      manualCompactTitle:
        locale === "ko"
          ? "컨텍스트가 가득 찼습니다"
          : locale === "ja"
            ? "Context window is full"
            : locale === "zh-Hans"
              ? "上下文窗口已满"
              : locale === "zh-Hant"
                ? "上下文視窗已滿"
                : locale === "fr"
                  ? "Fenêtre de contexte pleine"
                  : locale === "es"
                    ? "La ventana de contexto está llena"
                    : "Context window is full",
      manualCompactDescription:
        locale === "ko"
          ? "자동 압축이 멈춘 경우 수동으로 컨텍스트 압축을 다시 시작할 수 있습니다."
          : "If automatic compaction stopped, start context compression manually.",
      manualCompactAction:
        locale === "ko"
          ? "수동 압축 시작"
          : locale === "ja"
            ? "Start manual compact"
            : locale === "zh-Hans"
              ? "Start manual compact"
              : locale === "zh-Hant"
                ? "Start manual compact"
                : "Start manual compact",
      manualCompactStarting:
        locale === "ko" ? "압축 시작 중" : "Starting compact",
      manualCompactStarted:
        locale === "ko" ? "컨텍스트 압축을 시작했습니다." : "Context compression started.",
      showOlderWorkItems: (count: number) =>
        locale === "ko" ? `이전 작업 과정 ${count}개 더 보기` : `Show ${count} older work items`,
      showFullMessage: locale === "ko" ? "전체 메시지 보기" : "Show full message",
      showFullOutput: locale === "ko" ? "전체 출력 보기" : "Show full output",
      outputTruncatedPrefix: (count: number) =>
        locale === "ko"
          ? `... 이전 출력 ${count.toLocaleString()}자 생략 ...`
          : `... ${count.toLocaleString()} earlier characters hidden ...`,
      stopped: m.stopped()
    };
  });

  const sessionSearchCopy = $derived.by(() => {
    const locale = $activeLocale;
    if (locale === "ko") {
      return {
        placeholder: "이 대화에서 검색",
        hint: "메시지, 추론, 명령, 도구 호출까지 검색합니다.",
        noResults: "일치하는 내용이 없습니다.",
        loadMore: "결과 더 보기",
        turn: "턴",
        results: "결과",
        openSearch: "세션 검색"
      };
    }

    return {
      placeholder: "Search this thread",
      hint: "Search messages, reasoning, commands, and tool calls.",
      noResults: "No matches found.",
      loadMore: "Load more results",
      turn: "Turn",
      results: "results",
      openSearch: "Search thread"
    };
  });

  function getDefaultThreadTitle() {
    return m.new_thread();
  }

  function isPlaceholderThreadTitle(value: string | null | undefined) {
    const normalized = formatValue(value).trim();
    return !normalized || normalized === "New thread" || normalized === getDefaultThreadTitle();
  }

  function inferDisplayThreadTitle(text: string) {
    const normalized = stripAttachmentPreamble(text).replace(/\s+/g, " ").trim();
    if (!normalized) {
      return null;
    }

    let titleSource = normalized;
    while (true) {
      const nextWhitespace = titleSource.search(/\s/u);
      if (nextWhitespace <= 0) {
        break;
      }
      const token = titleSource.slice(0, nextWhitespace).trim();
      const commandLike = token.match(/^[$/]([A-Za-z0-9_-]+)$/u);
      if (!commandLike) {
        break;
      }
      titleSource = titleSource.slice(nextWhitespace).trimStart();
    }

    let candidate =
      titleSource.split(/\r?\n/u, 1)[0]?.split(/(?<=[.?!])\s+/u, 1)[0]?.split(/\s[-:|]\s/u, 1)[0]?.trim() ?? titleSource;

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
      candidate = titleSource;
    }

    return candidate.length > 60 ? `${candidate.slice(0, 60).trimEnd()}...` : candidate;
  }

  function getDisplayThreadTitle(name: string | null | undefined, preview: string | null | undefined) {
    if (!isPlaceholderThreadTitle(name)) {
      return formatValue(name);
    }
    return inferDisplayThreadTitle(formatValue(preview));
  }

  function normalizeSessionSummaryPreviewText(value: unknown) {
    return stripAttachmentPreamble(formatValue(value)).replace(/\s+/g, " ").trim();
  }

  function deriveConversationSummaryPreview(
    preview: string | null | undefined,
    turns: ConversationState["thread"]["turns"] | null | undefined
  ) {
    const normalizedPreview = normalizeSessionSummaryPreviewText(preview);
    let previewMatchesCommentary = false;

    if (normalizedPreview && turns) {
      outer: for (let turnIndex = turns.length - 1; turnIndex >= 0; turnIndex -= 1) {
        const turn = turns[turnIndex];
        for (let itemIndex = turn.items.length - 1; itemIndex >= 0; itemIndex -= 1) {
          const item = turn.items[itemIndex];
          if (item.type !== "agentMessage" || String(item.phase ?? "") !== "commentary") {
            continue;
          }

          const commentaryText = normalizeSessionSummaryPreviewText(getUserText(item as Record<string, unknown>));
          if (
            commentaryText &&
            (commentaryText === normalizedPreview ||
              commentaryText.startsWith(normalizedPreview) ||
              normalizedPreview.startsWith(commentaryText))
          ) {
            previewMatchesCommentary = true;
            break outer;
          }
        }
      }
    }

    if (normalizedPreview && !previewMatchesCommentary) {
      return normalizedPreview;
    }

    if (turns) {
      for (let turnIndex = turns.length - 1; turnIndex >= 0; turnIndex -= 1) {
        const turn = turns[turnIndex];
        for (let itemIndex = turn.items.length - 1; itemIndex >= 0; itemIndex -= 1) {
          const item = turn.items[itemIndex];
          if (item.type !== "agentMessage" || String(item.phase ?? "") === "commentary") {
            continue;
          }

          const text = normalizeSessionSummaryPreviewText(getUserText(item as Record<string, unknown>));
          if (text) {
            return text;
          }
        }
      }

      for (let turnIndex = turns.length - 1; turnIndex >= 0; turnIndex -= 1) {
        const turn = turns[turnIndex];
        for (let itemIndex = turn.items.length - 1; itemIndex >= 0; itemIndex -= 1) {
          const item = turn.items[itemIndex];
          if (item.type !== "userMessage") {
            continue;
          }

          const text = normalizeSessionSummaryPreviewText(getUserText(item as Record<string, unknown>));
          if (text) {
            return text;
          }
        }
      }
    }

    return normalizedPreview;
  }

  function getConversationDisplayTitle(state: ConversationState | null | undefined) {
    if (!state) {
      return null;
    }
    return getDisplayThreadTitle(state.thread.name, deriveConversationSummaryPreview(state.thread.preview, state.thread.turns));
  }

  function readLocalSendOnEnterPreference() {
    if (typeof window === "undefined") {
      return null;
    }
    let stored: string | null = null;
    try {
      stored = window.localStorage.getItem(sendOnEnterPreferenceStorageKey);
    } catch {
      return null;
    }
    if (stored === "true") {
      return true;
    }
    if (stored === "false") {
      return false;
    }
    return null;
  }

  function persistLocalSendOnEnterPreference(value: boolean) {
    if (typeof window === "undefined") {
      return;
    }
    try {
      window.localStorage.setItem(sendOnEnterPreferenceStorageKey, value ? "true" : "false");
    } catch {
      return;
    }
  }

  function applyLocalSendOnEnterPreference(preferences: SessionPreferences): SessionPreferences {
    const localValue = readLocalSendOnEnterPreference();
    if (localValue === null || preferences.sendOnEnter === localValue) {
      return preferences;
    }
    return {
      ...preferences,
      sendOnEnter: localValue
    };
  }

  function applyLocalComposerPreferencesToConfig(nextConfig: AppConfigPayload): AppConfigPayload {
    const nextDefaults = applyLocalSendOnEnterPreference(nextConfig.defaults);
    if (nextDefaults === nextConfig.defaults) {
      return nextConfig;
    }
    return {
      ...nextConfig,
      defaults: nextDefaults
    };
  }

  function applyLocalComposerPreferencesToConversation(state: ConversationState): ConversationState {
    const nextPreferences = applyLocalSendOnEnterPreference(state.preferences);
    if (nextPreferences === state.preferences) {
      return state;
    }
    return {
      ...state,
      preferences: nextPreferences
    };
  }

  function createDraftConversation(preferences: SessionPreferences, title: string | null = null): ConversationState {
    const now = Date.now();
    const nextPreferences = applyLocalSendOnEnterPreference(preferences);
    return {
      thread: {
        id: "",
        preview: "",
        name: title,
        cwd: nextPreferences.cwd,
        status: "idle",
        createdAt: now,
        updatedAt: now,
        isSubagent: false,
        agentNickname: null,
        agentRole: null,
        turns: []
      },
      preferences: {
        ...nextPreferences
      },
      selectedSkills: [],
      goal: null,
      attachments: [],
      queue: {
        sessionId: "",
        items: [],
        resumeRequired: false,
        updatedAt: null
      },
      pendingRequests: [],
      activeTurnId: null,
      tokenUsage: null,
      hydration: {
        state: "complete",
        loadedTurns: 0,
        totalTurns: 0,
        remainingTurns: 0,
        message: null,
        recovery: {
          available: false,
          issue: null,
          totalLines: null,
          recoverableLines: null,
          skippedLines: null
        }
      },
      cacheVersion: "",
      notModified: false,
      livePlans: {},
      liveDiffs: {}
    };
  }

  function activateDraftSession(
    preferences: SessionPreferences,
    options: {
      draftText?: string;
      draftAttachments?: AttachmentRecord[];
      title?: string | null;
    } = {}
  ) {
    sessionSelectionVersion += 1;
    clearHydrationRefresh();
    disconnectStream();
    dismissLastComposerPromptChip();
    resetSessionTurnSearch();
    selectedSessionId = null;
    selectedSessionProfileId = null;
    conversation = createDraftConversation(preferences, options.title ?? null);
    draftPersistencePaused = false;
    pendingSessionEvents = {};
    expandedItems = {};
    expandedFileChangeEntries = {};
    loadingItemDetails = {};
    itemDetailErrors = {};
    expandedTurnLogs = {};
    turnEntryRenderLimits = {};
    expandedLargeOutputs = {};
    loadingTurns = {};
    turnLoadErrors = {};
    pendingInitialTranscriptScrollSessionId = null;
    pendingTranscriptBottomScroll = false;
    draft = options.draftText ?? "";
    draftAttachments = [...(options.draftAttachments ?? [])];
    titleDraft = options.title ?? "";
    loadingDetail = false;
    pendingSteerResume = null;
    optimisticMessage = null;
    sendIntent = null;
    olderTurnsAutoLoadEnabled = true;
    olderTurnsAutoLoadPaused = false;
    olderTurnsAutoTriggerTimestamps = [];
    loadingOlderTurns = false;
    mobileSidebarOpen = false;
    composerSettingsOpen = false;
    activeWorkspaceTabId = "chat";
    syncSelectedSessionInUrl(null, { draft: true });
    queueMicrotask(() => {
      scheduleComposerTextareaResize();
    });
  }

  function resetSessionTurnSearch(close = true) {
    if (sessionTurnSearchTimer) {
      clearTimeout(sessionTurnSearchTimer);
      sessionTurnSearchTimer = null;
    }
    if (sessionTurnSearchHighlightTimer) {
      clearTimeout(sessionTurnSearchHighlightTimer);
      sessionTurnSearchHighlightTimer = null;
    }

    sessionTurnSearchRequestVersion += 1;
    sessionTurnSearchQuery = "";
    sessionTurnSearchResults = [];
    sessionTurnSearchCursor = null;
    sessionTurnSearchTotalMatches = 0;
    sessionTurnSearchBusy = false;
    sessionTurnSearchLoadingMore = false;
    sessionTurnSearchError = "";
    sessionTurnSearchFocusedTurnId = null;
    sessionTurnSearchJumpingTurnId = null;
    if (close) {
      sessionTurnSearchOpen = false;
    }
  }

  function clearStaleSessionCatchup() {
    if (staleSessionCatchupTimer) {
      clearTimeout(staleSessionCatchupTimer);
      staleSessionCatchupTimer = null;
    }
    staleSessionCatchup = null;
  }

  function beginStaleSessionCatchup(sessionId: string, hiddenDurationMs: number) {
    clearStaleSessionCatchup();
    staleSessionCatchup = {
      sessionId,
      hiddenDurationMs,
      eventCount: 0,
      refreshing: false,
      refreshRetries: 0
    };
    staleSessionCatchupTimer = setTimeout(() => {
      staleSessionCatchupTimer = null;
      staleSessionCatchup = null;
    }, staleSessionCatchupWindowMs);
  }

  function queuePendingSessionEvent(sessionId: string, profileId: string | null, payload: StreamEvent) {
    const scopeKey = sessionStateKey(sessionId, profileId);
    pendingSessionEvents = {
      ...pendingSessionEvents,
      [scopeKey]: [...(pendingSessionEvents[scopeKey] ?? []), payload]
    };
  }

  function toggleSessionTurnSearch() {
    if (sessionTurnSearchOpen) {
      resetSessionTurnSearch(true);
      return;
    }

    sessionTurnSearchOpen = true;
    queueMicrotask(() => {
      sessionTurnSearchInputElement?.focus();
      sessionTurnSearchInputElement?.select();
    });
  }

  function scheduleSessionTurnSearch(reset = true) {
    if (sessionTurnSearchTimer) {
      clearTimeout(sessionTurnSearchTimer);
      sessionTurnSearchTimer = null;
    }

    sessionTurnSearchError = "";

    if (!selectedSessionId || !sessionTurnSearchOpen) {
      return;
    }

    if (!sessionTurnSearchQuery.trim()) {
      sessionTurnSearchResults = [];
      sessionTurnSearchCursor = null;
      sessionTurnSearchTotalMatches = 0;
      sessionTurnSearchBusy = false;
      sessionTurnSearchLoadingMore = false;
      return;
    }

    sessionTurnSearchTimer = setTimeout(() => {
      sessionTurnSearchTimer = null;
      void runSessionTurnSearch(reset);
    }, 180);
  }

  async function runSessionTurnSearch(reset = true) {
    const sessionId = selectedSessionId;
    const query = sessionTurnSearchQuery.trim();
    if (!sessionId || !sessionTurnSearchOpen || !query) {
      return;
    }

    const requestVersion = ++sessionTurnSearchRequestVersion;
    if (reset) {
      sessionTurnSearchBusy = true;
      sessionTurnSearchCursor = null;
    } else {
      sessionTurnSearchLoadingMore = true;
    }
    sessionTurnSearchError = "";

    try {
      const response = await api.searchSessionTurns(sessionId, query, reset ? null : sessionTurnSearchCursor, 20, profileIdForSession(sessionId));
      if (
        requestVersion !== sessionTurnSearchRequestVersion ||
        selectedSessionId !== sessionId ||
        sessionTurnSearchQuery.trim() !== query
      ) {
        return;
      }

      sessionTurnSearchResults = reset
        ? response.matches
        : [...sessionTurnSearchResults, ...response.matches].filter(
            (match, index, collection) =>
              collection.findIndex(
                (candidate) =>
                  candidate.turnId === match.turnId && candidate.itemId === match.itemId && candidate.preview === match.preview
              ) === index
          );
      sessionTurnSearchCursor = response.nextCursor;
      sessionTurnSearchTotalMatches = response.totalMatches;
    } catch (error) {
      if (requestVersion !== sessionTurnSearchRequestVersion) {
        return;
      }
      sessionTurnSearchError = describeError(error);
    } finally {
      if (requestVersion === sessionTurnSearchRequestVersion) {
        sessionTurnSearchBusy = false;
        sessionTurnSearchLoadingMore = false;
      }
    }
  }

  async function loadMoreSessionTurnSearchResults() {
    if (!sessionTurnSearchCursor || sessionTurnSearchBusy || sessionTurnSearchLoadingMore) {
      return;
    }
    await runSessionTurnSearch(false);
  }

  function getSelectedSessionBinding() {
    const sessionId = selectedSessionId;
    const state = conversation;
    if (!sessionId || !state || state.thread.id !== sessionId) {
      return null;
    }

    return {
      sessionId,
      state
    };
  }

  function requestSelectedSessionResync(showMessage = true) {
    const sessionId = selectedSessionId;
    if (!sessionId) {
      return;
    }

    const requestVersion = showMessage ? ++manualSessionResyncRequestVersion : 0;
    if (showMessage) {
      manualSessionResyncSessionId = sessionId;
      errorText = "";
      noticeText = "";
    }

    void refreshSelectedSessionState(sessionId, Math.max(conversation?.thread.turns.length ?? 0, olderTurnPageSize), true)
      .catch((error) => {
        if (selectedSessionId === sessionId) {
          errorText = describeError(error);
        }
      })
      .finally(() => {
        if (showMessage && requestVersion === manualSessionResyncRequestVersion) {
          manualSessionResyncSessionId = null;
        }
      });
  }

  function ensureSelectedSessionBinding(showMessage = true) {
    const binding = getSelectedSessionBinding();
    if (binding) {
      return binding;
    }

    requestSelectedSessionResync(showMessage);
    return null;
  }

  async function ensureSessionForComposer() {
    const selectedBinding = getSelectedSessionBinding();
    if (selectedBinding) {
      return selectedBinding;
    }

    if (selectedSessionId) {
      requestSelectedSessionResync();
      return null;
    }

    if (!runtime?.installed) {
      errorText = m.codex_cli_required();
      return null;
    }
    if (!config || !conversation) {
      return null;
    }

    const draftState = conversation;
    const draftTextSnapshot = draft;
    const draftAttachmentSnapshot = [...draftAttachments];
    const draftTitleSnapshot = titleDraft.trim();
    const draftSelectedSkillsSnapshot = [...draftSelectedSkills];
    const nextTitle = draftTitleSnapshot && !isPlaceholderThreadTitle(draftTitleSnapshot) ? draftTitleSnapshot : null;

    const created = await api.createSession(draftState.preferences, nextTitle, draftSelectedSkillsSnapshot);
    upsertSessionSummary(created);
    if (activeSessionFolder) {
      try {
        const response = await api.updateSessionOrganization(
          created.id,
          {
            tags: [activeSessionFolder]
          },
          profileIdForSession(created.id)
        );
        upsertSessionSummary({
          ...created,
          pinned: response.meta.pinned,
          tags: response.meta.tags
        });
        updateConfigSessionOrganization({
          knownTags: response.knownTags,
          sessionFolders: response.sessionFolders
        });
      } catch (error) {
        errorText = describeError(error);
      }
    }
    const now = Date.now();
    const createdProfileId = created.profileId ?? profileIdForSession(created.id);
    const createdSelectionVersion = sessionSelectionVersion + 1;
    sessionSelectionVersion = createdSelectionVersion;
    selectedSessionId = created.id;
    selectedSessionProfileId = createdProfileId;
    syncSelectedSessionInUrl(created.id);
    connectStream(created.id, createdProfileId, createdSelectionVersion);
    pendingInitialTranscriptScrollSessionId = created.id;
    requestTranscriptBottomScroll(true);
    conversation = createConversationState({
      profileId: createdProfileId,
      profileLabel: created.profileLabel ?? null,
      profileCodexHome: created.profileCodexHome ?? null,
      accountEmail: created.accountEmail ?? null,
      accountType: created.accountType ?? null,
      thread: {
        id: created.id,
        preview: created.preview ?? "",
        name: created.name ?? nextTitle,
        cwd: created.cwd ?? draftState.preferences.cwd,
        status: created.status ?? "idle",
        createdAt: normalizeSessionTimestamp(created.createdAt) || now,
        updatedAt: normalizeSessionTimestamp(created.updatedAt) || now,
        isSubagent: created.isSubagent ?? false,
        agentNickname: created.agentNickname ?? null,
        agentRole: created.agentRole ?? null,
        turns: []
      },
      preferences: created.preferences ?? draftState.preferences,
      selectedSkills: draftSelectedSkillsSnapshot,
      goal: null,
      attachments: [],
      queue: {
        sessionId: created.id,
        items: [],
        resumeRequired: false,
        updatedAt: null
      },
      pendingRequests: [],
      activeTurnId: null,
      tokenUsage: null,
      hydration: {
        state: "complete",
        loadedTurns: 0,
        totalTurns: 0,
        remainingTurns: 0,
        message: null,
        recovery: {
          available: false,
          issue: null,
          totalLines: null,
          recoverableLines: null,
          skippedLines: null
        }
      },
      cacheVersion: "",
      notModified: false
    });
    markConversationCacheDirty();

    draft = draftTextSnapshot;
    draftAttachments = draftAttachmentSnapshot;
    draftSelectedSkills = [];
    scheduleComposerTextareaResize();

    return {
      sessionId: created.id,
      state: conversation
    };
  }

  function hasStartupAlerts(nextConfig: AppConfigPayload | null) {
    if (!nextConfig) {
      return false;
    }

    return (
      nextConfig.startup.pausedQueues.length > 0 ||
      Boolean(nextConfig.startup.scheduledShutdown && nextConfig.startup.scheduledShutdown.scheduledFor > Date.now())
    );
  }

  function syncStartupAlertModal(nextConfig: AppConfigPayload | null, forceOpen = false) {
    const allowInitialOpen = !startupAlertInitialConfigHandled;
    startupAlertInitialConfigHandled = true;
    if (!hasStartupAlerts(nextConfig)) {
      startupAlertModalOpen = false;
      startupAlertDismissed = false;
      return;
    }

    if (forceOpen || (allowInitialOpen && !startupAlertDismissed)) {
      startupAlertModalOpen = true;
    }
  }

  function dismissStartupAlertModal() {
    startupAlertDismissed = true;
    startupAlertModalOpen = false;
  }

  async function openStartupAlertSession(sessionId: string) {
    dismissStartupAlertModal();
    await selectSession(sessionId);
  }

  let releaseSessionStream: (() => void) | null = null;
  let releaseGlobalStream: (() => void) | null = null;
  let releaseReconnectListener: (() => void) | null = null;
  let releaseResyncRequiredListener: (() => void) | null = null;
  let releaseConnectionStateListener: (() => void) | null = null;
  let releaseThemeListener: (() => void) | null = null;
  let saveTimer: ReturnType<typeof setTimeout> | null = null;
  let preferenceSaveVersion = 0;
  const pendingPreferencePatchesBySessionId = new Map<string, Partial<SessionPreferences>>();
  let hydrationRefreshTimer: ReturnType<typeof setTimeout> | null = null;
  let sessionRefreshTimer: ReturnType<typeof setTimeout> | null = null;
  let websocketResyncTimer: ReturnType<typeof setTimeout> | null = null;
  let websocketResyncInFlight = false;
  let websocketResyncQueued = false;
  let selectedSessionDetailRefreshTimer: ReturnType<typeof setTimeout> | null = null;
  let selectedSessionCompletionRefreshJobs = new Map<
    string,
    {
      timers: Set<ReturnType<typeof setTimeout>>;
    }
  >();
  let sessionListCachePersistTimer: ReturnType<typeof setTimeout> | null = null;
  let sessionDetailCachePersistTimer: ReturnType<typeof setTimeout> | null = null;
  let sessionDetailCachePersistMode: SessionCachePersistMode | null = null;
  let sessionDetailCachePersistInFlight = false;
  let queuedSessionDetailCachePersist: {
    sessionId: string;
    cacheKey: string;
    version: string | null;
  } | null = null;
  let sessionListRequestVersion = 0;
  let sessionSelectionVersion = 0;
  let accountProfileSwitchGeneration = 0;
  let accountProfileSwitchQueue: Promise<void> = Promise.resolve();
  const itemDetailRefreshTimers = new Map<string, ReturnType<typeof setTimeout>>();
  const lazyWorkspaceLoads = new Map<LazyWorkspaceKind, Promise<void>>();
  let transcriptElement = $state<HTMLDivElement | undefined>(undefined);
  let transcriptContentElement = $state<HTMLDivElement | undefined>(undefined);
  let transcriptTurnsElement = $state<HTMLDivElement | undefined>(undefined);
  let transcriptDockElement = $state<HTMLDivElement | undefined>(undefined);
  let transcriptTurnWindow = $state(EMPTY_TRANSCRIPT_WINDOW);
  let transcriptTurnWindowSessionId: string | null = null;
  let transcriptTurnLayout: TranscriptLayout = EMPTY_TRANSCRIPT_LAYOUT;
  const transcriptTurnHeights = new Map<string, number>();
  const pendingTranscriptTurnHeights = new Map<string, number>();
  let transcriptMeasurementFrame: number | null = null;
  let transcriptMeasuredWidth = 0;
  let transcriptPinnedTurnId: string | null = null;
  let transcriptPinnedTurnAlignment: TranscriptWindowAlignment = "center";
  let transcriptPinReleaseTimer: ReturnType<typeof setTimeout> | null = null;
  let transcriptPinScrollEndCleanup: (() => void) | null = null;
  let stickTranscriptToBottom = $state(true);
  let forceTranscriptScroll = $state(false);
  let pendingTranscriptBottomScroll = $state(false);
  let transcriptAutoScrollSuspendedByUser = $state(false);
  let composerSettingsTriggerElement = $state<HTMLButtonElement | undefined>(undefined);
  let composerSettingsPopoverElement = $state<HTMLDivElement | undefined>(undefined);
  let composerSettingsPopoverStyle = $state("");
  let composerSecurityTriggerElement = $state<HTMLButtonElement | undefined>(undefined);
  let sessionTurnSearchTriggerElement = $state<HTMLButtonElement | undefined>(undefined);
  let sessionTurnSearchPopoverElement = $state<HTMLDivElement | undefined>(undefined);
  let sessionTurnSearchInputElement = $state<HTMLInputElement | undefined>(undefined);
  let sessionTurnSearchTimer: ReturnType<typeof setTimeout> | null = null;
  let sessionTurnSearchHighlightTimer: ReturnType<typeof setTimeout> | null = null;
  let staleSessionCatchupTimer: ReturnType<typeof setTimeout> | null = null;
  let sessionTurnSearchRequestVersion = 0;
  let manualSessionResyncRequestVersion = 0;
  let sessionTurnSearchPopoverStyle = $state("");
  let titleInputElement = $state<HTMLInputElement | undefined>(undefined);
  let composerTextareaElement = $state<HTMLTextAreaElement | undefined>(undefined);
  let composerPanelElement = $state<HTMLFormElement | undefined>(undefined);
  let composerToolbarElement = $state<HTMLDivElement | undefined>(undefined);
  let filePickerElement = $state<HTMLInputElement | undefined>(undefined);
  let fakeTopLoadPercent = $state(0);
  let transcriptScrollFrame: number | null = null;
  let composerTextareaResizeFrame: number | null = null;
  let transcriptResizeObserver: ResizeObserver | null = null;
  let transcriptDockResizeObserver: ResizeObserver | null = null;
  let composerToolbarResizeObserver: ResizeObserver | null = null;
  let composerToolbarCompact = $state(true);
  let transcriptDockReservePx = $state(196);
  let transcriptScrollGeneration = 0;
  let transcriptUserScrollIntentUntil = 0;
  let transcriptProgrammaticScrollUntil = 0;
  let pendingInitialTranscriptScrollSessionId = $state<string | null>(null);
  let composerHistory = $state<string[]>([]);
  let composerHistoryIndex = $state(-1);
  let composerHistoryDraft = $state("");
  let lastComposerPromptChip = $state<{ sessionId: string; prompt: string } | null>(null);
  let notificationPermissionRequested = false;
  const recentAttentionNotificationKeys: Record<string, number> = {};
  const recentLiveSessionEvidenceTtlMs = 8_000;
  let recentLiveSessionEvidenceAtBySessionId: Record<string, number> = {};
  const recentLiveSessionEvidenceTimers = new Map<string, ReturnType<typeof setTimeout>>();
  let lastLiveSessionEvidenceAt = 0;
  const handledResumeDraftKeys = new Set<string>();
  const pendingComposerMutationSignatures = new Set<string>();
  const pendingEnqueuesByOptimisticId = new Map<string, PendingEnqueueState>();
  const sessionEventRevisions = new Map<string, number>();
  const sessionStreamCursors = new Map<string, SessionStreamCursor>();
  const queueStateRevisions = new Map<string, number>();
  let pendingComposerMutationRevision = $state(0);
  let lastLoadedConversationId = $state<string | null>(null);
  let lastActiveLiveTurnId = $state<string | null>(null);
  const visibleTranscriptTurns = $derived.by(() => {
    if (!conversation) {
      return [];
    }
    return conversation.thread.turns.slice(transcriptTurnWindow.start, transcriptTurnWindow.end);
  });

  function selectedSessionStateKey() {
    return selectedSessionId ? sessionStateKey(selectedSessionId, selectedSessionProfileId) : null;
  }

  function selectedSessionBindingMatches(sessionId: string, profileId: string | null | undefined) {
    return selectedSessionStateKey() === sessionStateKey(sessionId, profileId);
  }

  function applySessionStreamCursorResult(
    scopeKey: string,
    sessionId: string,
    profileId: string | null,
    result: SessionStreamCursorResult
  ) {
    if (result.cursor) {
      sessionStreamCursors.set(scopeKey, result.cursor);
    }
    if (!result.gap || !selectedSessionBindingMatches(sessionId, profileId)) {
      return;
    }

    sessionDetailCacheVersion = null;
    sessionDetailStateHash = null;
    sessionDetailMetadataVersion = null;
    sessionTurnVersionsById = {};
    scheduleSelectedSessionStateRefresh(sessionId, 0, true);
  }

  function observeSelectedSessionStreamEvent(
    scopeKey: string,
    sessionId: string,
    profileId: string | null,
    event: StreamEvent
  ) {
    applySessionStreamCursorResult(
      scopeKey,
      sessionId,
      profileId,
      observeSessionStreamEvent(sessionStreamCursors.get(scopeKey) ?? null, event)
    );
  }

  function reconcileSelectedSessionStreamBoundary(
    scopeKey: string,
    sessionId: string,
    profileId: string | null,
    requestCursor: SessionStreamCursor | null,
    detail: SessionDetailResponse
  ) {
    applySessionStreamCursorResult(
      scopeKey,
      sessionId,
      profileId,
      reconcileSessionStreamBoundary(sessionStreamCursors.get(scopeKey) ?? null, requestCursor, detail)
    );
  }

  function isLiveConversationStatus(status: string | null | undefined) {
    return status === "running" || status === "active";
  }

  function noteRecentLiveSessionEvidence(sessionId: string) {
    if (!sessionId) {
      return;
    }
    const seenAt = Date.now();
    lastLiveSessionEvidenceAt = seenAt;
    const previousSeenAt = recentLiveSessionEvidenceAtBySessionId[sessionId] ?? 0;
    if (seenAt - previousSeenAt < 1_000) {
      return;
    }
    recentLiveSessionEvidenceAtBySessionId = {
      ...recentLiveSessionEvidenceAtBySessionId,
      [sessionId]: seenAt
    };
    const previousTimer = recentLiveSessionEvidenceTimers.get(sessionId);
    if (previousTimer) {
      clearTimeout(previousTimer);
    }
    const timer = setTimeout(() => {
      recentLiveSessionEvidenceTimers.delete(sessionId);
      if (recentLiveSessionEvidenceAtBySessionId[sessionId] === seenAt) {
        clearRecentLiveSessionEvidence(sessionId);
      }
    }, recentLiveSessionEvidenceTtlMs + 100);
    recentLiveSessionEvidenceTimers.set(sessionId, timer);
  }

  function clearRecentLiveSessionEvidence(sessionId: string) {
    const timer = recentLiveSessionEvidenceTimers.get(sessionId);
    if (timer) {
      clearTimeout(timer);
      recentLiveSessionEvidenceTimers.delete(sessionId);
    }
    if (!sessionId || recentLiveSessionEvidenceAtBySessionId[sessionId] === undefined) {
      return;
    }
    const nextEvidence = { ...recentLiveSessionEvidenceAtBySessionId };
    delete nextEvidence[sessionId];
    recentLiveSessionEvidenceAtBySessionId = nextEvidence;
  }

  function hasRecentLiveSessionEvidence(sessionId: string | null | undefined) {
    if (!sessionId) {
      return false;
    }
    const seenAt = recentLiveSessionEvidenceAtBySessionId[sessionId] ?? 0;
    return seenAt > 0 && Date.now() - seenAt < recentLiveSessionEvidenceTtlMs;
  }

  function shouldDeferTerminalSessionStatus(sessionId: string | null | undefined, status: string | null | undefined) {
    return Boolean(sessionId && status && !isLiveConversationStatus(status) && hasRecentLiveSessionEvidence(sessionId));
  }

  function shouldSuppressAttentionReason(sessionId: string, reason: string, profileId: string | null = null) {
    if (
      selectedSessionId === sessionId &&
      profileId &&
      selectedSessionProfileId &&
      selectedSessionProfileId !== profileId
    ) {
      return false;
    }
    if (!hasRecentLiveSessionEvidence(sessionId)) {
      return false;
    }
    if (reason === "failed" || reason === "stopped") {
      return true;
    }
    return reason === "needsInput" && conversation?.thread.id === sessionId && conversation.pendingRequests.length === 0;
  }

  function getConversationLiveTurn(currentConversation: ConversationState | null = conversation) {
    if (!currentConversation) {
      return null;
    }

    if (currentConversation.activeTurnId) {
      const trackedTurn = currentConversation.thread.turns.find((turn) => turn.id === currentConversation.activeTurnId) ?? null;
      if (trackedTurn && String(trackedTurn.status ?? "") === "inProgress") {
        return trackedTurn;
      }
    }

    for (let index = currentConversation.thread.turns.length - 1; index >= 0; index -= 1) {
      const candidate = currentConversation.thread.turns[index];
      if (String(candidate.status ?? "") === "inProgress") {
        return candidate;
      }
    }

    return null;
  }

  function hasConversationLiveTurn(currentConversation: ConversationState | null = conversation) {
    return Boolean(
      getConversationLiveTurn(currentConversation) ||
        (currentConversation?.activeTurnId && isLiveConversationStatus(currentConversation.thread.status))
    );
  }

  function normalizeConversationExecutionState(currentConversation: ConversationState) {
    const liveTurn = getConversationLiveTurn(currentConversation);
    if (liveTurn) {
      if (currentConversation.activeTurnId === liveTurn.id && isLiveConversationStatus(currentConversation.thread.status)) {
        return currentConversation;
      }

      return {
        ...currentConversation,
        activeTurnId: liveTurn.id,
        thread: isLiveConversationStatus(currentConversation.thread.status)
          ? currentConversation.thread
          : {
              ...currentConversation.thread,
              status: "running"
            }
      };
    }

    const shouldKeepLiveShell =
      isLiveConversationStatus(currentConversation.thread.status) &&
      (currentConversation.thread.turns.length === 0 || Boolean(currentConversation.activeTurnId));
    const nextStatus = !shouldKeepLiveShell && isLiveConversationStatus(currentConversation.thread.status) ? "completed" : currentConversation.thread.status;
    if (currentConversation.activeTurnId === null && nextStatus === currentConversation.thread.status) {
      return currentConversation;
    }

    return {
      ...currentConversation,
      activeTurnId: null,
      thread:
        nextStatus === currentConversation.thread.status
          ? currentConversation.thread
          : {
              ...currentConversation.thread,
              status: nextStatus
            }
    };
  }

  function hasQueueableConversationActivity(currentConversation: ConversationState | null = conversation) {
    const sessionId = selectedSessionId;
    if (!sessionId) {
      return false;
    }

    const matchingConversation = currentConversation?.thread.id === sessionId ? currentConversation : null;
    const scopeKey = sessionStateKey(sessionId, selectedSessionProfileId);
    const cachedQueue = sessionQueueSnapshotsBySessionId[scopeKey] ?? null;
    const selectedSummaryQueueCount =
      sessions.find(
        (session) =>
          session.id === sessionId &&
          (!selectedSessionProfileId || !session.profileId || session.profileId === selectedSessionProfileId)
      )?.queueCount ?? 0;
    const hasCachedQueuedWork =
      (matchingConversation?.queue.items.length ?? 0) > 0 ||
      (cachedQueue?.items.length ?? 0) > 0 ||
      (optimisticQueuedItemsBySessionId[scopeKey]?.length ?? 0) > 0 ||
      (queuedMessageRequestCountsBySessionId[scopeKey] ?? 0) > 0 ||
      selectedSummaryQueueCount > 0;

    if (hasCachedQueuedWork) {
      return true;
    }

    if (pendingQueueModeWithinGrace(sessionId)) {
      return true;
    }

    if (!matchingConversation) {
      return false;
    }

    return hasConversationLiveTurn(matchingConversation) || isLiveConversationStatus(matchingConversation.thread.status);
  }

  function canQueueComposerMessage(currentConversation: ConversationState | null = conversation) {
    return hasQueueableConversationActivity(currentConversation);
  }

  function activatePendingQueueMode(sessionId: string, profileId: string | null = profileIdForSession(sessionId)) {
    pendingQueueModeSessionKey = sessionStateKey(sessionId, profileId);
    pendingQueueModeActivatedAt = Date.now();
  }

  function clearPendingQueueMode(
    sessionId: string | null = null,
    profileId: string | null = sessionId ? profileIdForSession(sessionId) : null
  ) {
    if (sessionId && pendingQueueModeSessionKey !== sessionStateKey(sessionId, profileId)) {
      return;
    }

    pendingQueueModeSessionKey = null;
    pendingQueueModeActivatedAt = 0;
  }

  function pendingQueueModeWithinGrace(sessionId: string, profileId: string | null = profileIdForSession(sessionId)) {
    return (
      pendingQueueModeSessionKey === sessionStateKey(sessionId, profileId) &&
      pendingQueueModeActivatedAt > 0 &&
      Date.now() - pendingQueueModeActivatedAt < LOCAL_QUEUE_MODE_GRACE_MS
    );
  }

  function canQueueDuringLocalSubmission(sessionId: string | null | undefined) {
    if (!sessionId || selectedSessionId !== sessionId) {
      return false;
    }

    return Boolean(
      pendingQueueModeWithinGrace(sessionId) ||
        (optimisticMessage?.sessionId === sessionId &&
          sessionStateKey(optimisticMessage.sessionId, optimisticMessage.profileId) === selectedSessionStateKey()) ||
        (startingMessage && conversation?.thread.id === sessionId) ||
        (sending && conversation?.thread.id === sessionId)
    );
  }

  function mergeQueueSnapshot(existingQueue: SessionQueuePayload | null | undefined, incomingQueue: SessionQueuePayload) {
    if (!existingQueue) {
      return incomingQueue;
    }

    const existingItemCount = Array.isArray(existingQueue.items) ? existingQueue.items.length : 0;
    const incomingItemCount = Array.isArray(incomingQueue.items) ? incomingQueue.items.length : 0;
    if (incomingItemCount === 0 && existingItemCount > 0 && !incomingQueue.resumeRequired) {
      return incomingQueue;
    }

    const existingUpdatedAt = Number(existingQueue.updatedAt ?? 0);
    const incomingUpdatedAt = Number(incomingQueue.updatedAt ?? 0);
    return existingUpdatedAt > incomingUpdatedAt ? existingQueue : incomingQueue;
  }

  function queuePayloadFromEvent(event: StreamEvent) {
    if (!isQueueUpdatedEvent(event)) {
      return null;
    }

    const queue = event.params.queue as SessionQueuePayload | undefined;
    return queue && Array.isArray(queue.items) ? queue : null;
  }

  function mergeOptimisticQueueItems(sessionId: string, profileId: string | null, queueItems: SessionQueueItem[]) {
    const scopeKey = sessionStateKey(sessionId, profileId);
    const deletedOptimisticIds = new Set(
      [...pendingEnqueuesByOptimisticId.values()]
        .filter(
          (pending) =>
            pending.deleted && sessionStateKey(pending.sessionId, pending.profileId) === scopeKey
        )
        .flatMap((pending) =>
          [
            pending.optimisticQueueId,
            pending.item.clientRequestId,
            pending.item.clientUserMessageId
          ].filter((value): value is string => Boolean(value))
        )
    );
    const visibleQueueItems = queueItems.filter(
      (item) =>
        ![item.id, item.clientRequestId, item.clientUserMessageId].some(
          (value) => value && deletedOptimisticIds.has(value)
        )
    );
    const optimisticItems = optimisticQueuedItemsBySessionId[scopeKey] ?? [];
    if (optimisticItems.length === 0) {
      return visibleQueueItems;
    }

    const realIds = new Set<string>();
    const realCounts = new Map<string, number>();
    for (const item of visibleQueueItems) {
      for (const value of [item.id, item.clientRequestId, item.clientUserMessageId]) {
        if (value) {
          realIds.add(value);
        }
      }
      const signature = buildQueueItemSignature(item.prompt, item.skills, item.attachmentIds);
      realCounts.set(signature, (realCounts.get(signature) ?? 0) + 1);
    }

    const visibleOptimisticItems = optimisticItems.filter((item) => {
      const pendingEnqueue = pendingEnqueuesByOptimisticId.get(item.id);
      const matchedRealItem = visibleQueueItems.find((candidate) =>
        [candidate.id, candidate.clientRequestId, candidate.clientUserMessageId].some(
          (value) => value && [item.id, item.clientRequestId, item.clientUserMessageId].includes(value)
        )
      );
      if (
        pendingEnqueue &&
        matchedRealItem &&
        buildQueueItemSignature(item.prompt, item.skills, item.attachmentIds) !==
          buildQueueItemSignature(matchedRealItem.prompt, matchedRealItem.skills, matchedRealItem.attachmentIds)
      ) {
        return true;
      }
      if ([item.id, item.clientRequestId, item.clientUserMessageId].some((value) => value && realIds.has(value))) {
        return false;
      }
      const signature = buildQueueItemSignature(item.prompt, item.skills, item.attachmentIds);
      const remaining = realCounts.get(signature) ?? 0;
      if (remaining <= 0) {
        return true;
      }

      realCounts.set(signature, remaining - 1);
      return false;
    });

    const optimisticIdentityValues = new Set(
      visibleOptimisticItems.flatMap((item) =>
        [item.id, item.clientRequestId, item.clientUserMessageId].filter((value): value is string => Boolean(value))
      )
    );
    return [
      ...visibleQueueItems.filter(
        (item) =>
          ![item.id, item.clientRequestId, item.clientUserMessageId].some(
            (value) => value && optimisticIdentityValues.has(value)
          )
      ),
      ...visibleOptimisticItems
    ];
  }

  const running = $derived.by(() => {
    const currentConversation = conversation;
    const sessionId = currentConversation?.thread.id ?? null;
    return Boolean(
      hasConversationLiveTurn(currentConversation) ||
        isLiveConversationStatus(currentConversation?.thread.status) ||
        hasRecentLiveSessionEvidence(sessionId)
    );
  });
  const sessionListNeedsActiveStatusPolling = $derived.by(() => {
    if (authenticated !== true) {
      return false;
    }

    if (
      conversation &&
      (hasConversationLiveTurn(conversation) ||
        isLiveConversationStatus(conversation.thread.status) ||
        conversation.queue.items.length > 0)
    ) {
      return true;
    }

    return sessions.some((session) => isLiveConversationStatus(session.status) || session.queueCount > 0);
  });
  const queueModeActive = $derived.by(() => canQueueComposerMessage());
  const composerQueueModeActive = $derived.by(() => {
    const selectedBinding = getSelectedSessionBinding();
    return canQueueComposerMessage(selectedBinding?.state ?? conversation) || canQueueDuringLocalSubmission(selectedBinding?.sessionId);
  });
  const lastComposerHistoryPrompt = $derived.by(() => lastComposerPromptChip?.prompt ?? "");
  const composerHasContent = $derived.by(() => draft.trim().length > 0 || draftAttachments.length > 0);
  const selectedSessionQueuedRequestCount = $derived.by(() => {
    const sessionId = selectedSessionId ?? conversation?.thread.id ?? null;
    if (!sessionId) {
      return 0;
    }

    return queuedMessageRequestCountsBySessionId[sessionStateKey(sessionId, selectedSessionProfileId)] ?? 0;
  });
  const composerQueueActionDisabled = $derived.by(() => {
    if (readOnlyRole || uploading) {
      return true;
    }

    const selectedBinding = getSelectedSessionBinding();
    if (!selectedBinding) {
      return true;
    }

    return (
      composerCurrentDraftHasPendingMutation(selectedBinding.sessionId, selectedBinding.state) ||
      (!canQueueComposerMessage(selectedBinding.state) && !canQueueDuringLocalSubmission(selectedBinding.sessionId))
    );
  });
  const composerPrimaryActionDisabled = $derived.by(() => {
    if (!composerHasContent) {
      return true;
    }

    if (composerQueueModeActive) {
      return composerQueueActionDisabled;
    }

    return readOnlyRole || sending || startingMessage || submitComposerBusy || uploading;
  });
  const recentComposerActionDisabled = $derived.by(() => {
    if (!lastComposerHistoryPrompt) {
      return true;
    }

    if (composerQueueModeActive) {
      return composerQueueActionDisabled;
    }

    return readOnlyRole || sending || startingMessage || submitComposerBusy || uploading;
  });
  const selectedSessionSummary = $derived(
    sessions.find(
      (session) =>
        session.id === selectedSessionId &&
        (!selectedSessionProfileId || !session.profileId || session.profileId === selectedSessionProfileId)
    ) ??
      sessions.find((session) => session.id === selectedSessionId) ??
      null
  );
  const sessionHighlights = $derived.by(() =>
    Object.fromEntries(
      sessions.flatMap((session) =>
        session.highlight && !shouldSuppressAttentionReason(session.id, String(session.highlight.reason ?? ""))
          ? ([[session.id, session.highlight]] as const)
          : []
      )
    )
  );
  const selectedSessionQueue = $derived.by(() => {
    const sessionId = selectedSessionId ?? conversation?.thread.id ?? null;
    if (!sessionId) {
      return conversation?.queue ?? null;
    }

    return sessionQueueSnapshotsBySessionId[sessionStateKey(sessionId, selectedSessionProfileId)] ?? conversation?.queue ?? null;
  });
  const queuedMessages = $derived.by(() => {
    const sessionId = selectedSessionId ?? conversation?.thread.id ?? null;
    const queueItems = selectedSessionQueue?.items ?? ([] as SessionQueueItem[]);
    return sessionId ? mergeOptimisticQueueItems(sessionId, selectedSessionProfileId, queueItems) : queueItems;
  });
  const serverQueuedMessages = $derived(selectedSessionQueue?.items ?? ([] as SessionQueueItem[]));
  const hasPendingQueueRequests = $derived.by(() => selectedSessionQueuedRequestCount > 0);
  const activeTurn = $derived.by(() => getConversationLiveTurn());
  const activeLiveTurnSubagents = $derived.by(() => {
    if (!activeTurn) {
      return [] as SubagentTaskEntry[];
    }

    const tasks: SubagentTaskEntry[] = [];
    for (const item of activeTurn.items) {
      if (item.type !== "collabAgentToolCall") {
        continue;
      }

      tasks.push({
        key: `${activeTurn.id}:${item.id}`,
        turnId: activeTurn.id,
        itemId: item.id,
        tool: String(item.tool ?? "spawn_agent"),
        status: String(item.status ?? "unknown"),
        prompt: String(item.prompt ?? "").trim(),
        model: String(item.model ?? "default"),
        reasoningEffort: String(item.reasoningEffort ?? "default"),
        primaryThreadId: getPrimarySubagentThreadId(item),
        states: getSubagentStates(item)
      });
    }

    return tasks;
  });
  const activeLiveTurnId = $derived.by(() => {
    if (!activeTurn || !conversation) {
      return null;
    }
    if (
      conversation.livePlans[activeTurn.id] ||
      conversation.liveDiffs[activeTurn.id] ||
      activeLiveTurnSubagents.length > 0
    ) {
      return activeTurn.id;
    }
    return null;
  });
  const activeLiveTurnPlan = $derived.by(() => (activeLiveTurnId ? conversation?.livePlans[activeLiveTurnId] ?? null : null));
  const activeLiveTurnDiff = $derived.by(() => (activeLiveTurnId ? conversation?.liveDiffs[activeLiveTurnId] ?? null : null));
  const activeLiveTurnDiffViews = $derived.by(() => (activeLiveTurnDiff ? parseAggregatedDiffViews(activeLiveTurnDiff) : []));
  const visibleOptimisticMessage = $derived.by(() => {
    if (
      !optimisticMessage ||
      optimisticMessage.sessionId !== selectedSessionId ||
      sessionStateKey(optimisticMessage.sessionId, optimisticMessage.profileId) !== selectedSessionStateKey()
    ) {
      return null;
    }
    if (!conversation || conversation.thread.id !== optimisticMessage.sessionId) {
      return optimisticMessage;
    }
    return hasConversationEchoedOptimisticMessage(conversation, optimisticMessage) ? null : optimisticMessage;
  });
  const optimisticAnchorTurnId = $derived.by(() => {
    if (!visibleOptimisticMessage || !conversation || conversation.thread.id !== visibleOptimisticMessage.sessionId) {
      return null;
    }

    for (let index = conversation.thread.turns.length - 1; index >= visibleOptimisticMessage.baselineTurnCount; index -= 1) {
      const turn = conversation.thread.turns[index];
      const hasUserMessage = turn.items.some((item) => item.type === "userMessage");
      if (hasUserMessage) {
        continue;
      }

      const hasAgentEntries = turn.items.some((item) => item.type !== "userMessage");
      const isLiveTurn = turn.id === conversation.activeTurnId || String(turn.status ?? "") === "inProgress";
      if (hasAgentEntries || isLiveTurn) {
        return turn.id;
      }
    }

    return null;
  });
  const standaloneOptimisticMessage = $derived.by(() =>
    optimisticAnchorTurnId ? null : visibleOptimisticMessage
  );
  const showQueueResumeBanner = $derived.by(
    () =>
      Boolean(
        selectedSessionId &&
          selectedSessionQueue?.resumeRequired &&
          !dismissedQueueResumeBySessionId[sessionStateKey(selectedSessionId, selectedSessionProfileId)]
      )
  );
  const selectedModel = $derived.by(() => {
    if (!config) {
      return null;
    }
    return config.models.find((model) => model.id === conversation?.preferences.model) ?? config.models.find((model) => model.isDefault) ?? null;
  });
  const reasoningOptions = $derived.by(() => {
    if (!selectedModel) {
      return ["medium"];
    }
    return selectedModel.supportedReasoningEfforts.length > 0
      ? selectedModel.supportedReasoningEfforts
      : [selectedModel.defaultReasoningEffort];
  });
  const speedOptions = $derived.by(() => {
    const available = ["auto", ...(selectedModel?.additionalSpeedTiers ?? [])];
    return [...new Set(available.filter((value) => value === "auto" || value === "fast" || value === "flex"))];
  });
  const composerSelectedSkills = $derived.by(() => conversation?.selectedSkills ?? draftSelectedSkills);
  const personalityOptions = $derived.by(
    () => ["pragmatic", "friendly", "none"] as Array<SessionPreferences["personality"]>
  );
  const filteredComposerSkills = $derived.by(() => {
    const pluginMentions = (catalog?.plugins ?? [])
      .filter((plugin) => plugin.mentionPath || plugin.path.startsWith("plugin://"))
      .map((plugin) => {
        const mentionPath = plugin.mentionPath ?? plugin.path;
        return {
          id: mentionPath,
          name: plugin.displayName || plugin.name,
          description: [
            plugin.description,
            plugin.capabilities?.length ? plugin.capabilities.join(", ") : ""
          ]
            .filter(Boolean)
            .join(" · "),
          path: mentionPath,
          source: "codex-plugin" as const,
          pluginName: plugin.marketplaceName ?? plugin.name
        };
      });
    const skills = [...(catalog?.skills ?? []), ...pluginMentions];
    const needle = composerSkillQuery.trim().toLowerCase();
    const selectedPaths = new Set(composerSelectedSkills.map((skill) => skill.path));
    const filtered = needle
      ? skills.filter((skill) => {
          const haystack = `${skill.name}\n${skill.description}\n${skill.pluginName ?? ""}\n${skill.source}`.toLowerCase();
          return haystack.includes(needle);
        })
      : skills;
    return [...filtered].sort((left, right) => {
      const leftSelected = selectedPaths.has(left.path) ? 1 : 0;
      const rightSelected = selectedPaths.has(right.path) ? 1 : 0;
      if (leftSelected !== rightSelected) {
        return rightSelected - leftSelected;
      }
      return left.name.localeCompare(right.name);
    });
  });
  function slashCommandDescription(entry: CodexSlashCommandEntry) {
    switch (entry.command) {
      case "queue":
        return m.slash_queue_description();
      case "steer":
        return m.slash_steer_description();
      case "preset":
        return m.slash_preset_description();
      case "model":
        return m.slash_model_description();
      case "personality":
        return ui.slashPersonalityDescription;
      case "plan":
        return m.slash_plan_description();
      case "goal":
        return m.slash_goal_description();
      case "compact":
        return ui.manualCompactDescription;
      case "fast":
        return m.slash_fast_description();
      default:
        return entry.description;
    }
  }

  const slashSuggestions = $derived.by(() => {
    const value = draft.trimStart();
    if (!value.startsWith("/")) {
      return [] as SlashSuggestion[];
    }

    const body = value.slice(1);
    const lower = body.toLowerCase();
    if (lower.startsWith("personality ")) {
      const personalityNeedle = body.slice("personality ".length).trim().toLowerCase();
      return personalityOptions
        .filter((personality) => !personalityNeedle || personality.includes(personalityNeedle))
        .slice(0, 6)
        .map((personality) => ({
          key: `personality:${personality}`,
          command: "personality",
          title: `/personality ${personality}`,
          description: getPersonalityOptionLabel(personality),
          value: `/personality ${personality}`
        }));
    }

    if (lower.startsWith("preset ")) {
      const presetNeedle = body.slice("preset ".length).trim().toLowerCase();
      return (config?.promptPresets ?? [])
        .filter((preset) => !presetNeedle || preset.name.toLowerCase().includes(presetNeedle))
        .slice(0, 6)
        .map((preset) => ({
          key: `preset:${preset.id}`,
          command: "preset",
          title: `/preset ${preset.name}`,
          description: preset.prompt.split(/\r?\n/u, 1)[0]?.trim() || preset.prompt.trim(),
          value: `/preset ${preset.name}`
        }));
    }

    const builtinSuggestions: SlashSuggestion[] = CODEX_SLASH_COMMANDS.filter((entry) => entry.visibleInComposer).map((entry) => ({
      key: entry.command,
      command: entry.command,
      title: `/${entry.command}`,
      description: slashCommandDescription(entry),
      value: `/${entry.command}${entry.inlineArgs ? " " : ""}`,
      support: entry.support
    }));

    return builtinSuggestions.filter((entry) => !lower || entry.command.includes(lower) || entry.title.includes(lower)).slice(0, 6);
  });
  const sessionHydrationRemainingTurns = $derived(conversation?.hydration.remainingTurns ?? 0);
  const sessionHydrationPercent = $derived.by(() => {
    if (!conversation?.hydration || !conversation.hydration.totalTurns || conversation.hydration.totalTurns <= 0) {
      return null;
    }
    return Math.max(6, Math.min(100, Math.round((conversation.hydration.loadedTurns / conversation.hydration.totalTurns) * 100)));
  });
  const topLoadKind = $derived.by(() => {
    if (loadingOlderTurns) {
      return "olderTurns";
    }
    if (sessionsBusy && sessions.length === 0) {
      return "sessionsInitial";
    }
    if (sessionsBusy) {
      return "sessionsRefresh";
    }
    if (sessionsLoadingMore) {
      return "sessionsMore";
    }
    if (loadingDetail && conversation) {
      return "sessionRefresh";
    }
    if (loadingDetail && !conversation) {
      return "sessionDetail";
    }
    if (conversation && sessionHydrationRemainingTurns > 0) {
      return "sessionHydration";
    }
    return "idle";
  });
  const topLoadLabel = $derived.by(() => {
    const _locale = $localeSignal;
    if (loadingOlderTurns) {
      return ui.loadingOlderTurns;
    }
    if (sessionsBusy && sessions.length === 0) {
      return ui.loadingSessions;
    }
    if (sessionsBusy) {
      return ui.refreshingSessions;
    }
    if (sessionsLoadingMore) {
      return ui.loadingMoreSessions;
    }
    if (loadingDetail && conversation) {
      return ui.sessionResyncing;
    }
    if (loadingDetail && !conversation) {
      return ui.loadingSessionBasics;
    }
    if (conversation && sessionHydrationRemainingTurns > 0) {
      return ui.sessionResyncing;
    }
    return "";
  });
  const sessionSyncLabel = $derived.by(() => {
    if (manualSessionResyncSessionId === selectedSessionId || staleSessionCatchup?.refreshing) {
      return ui.sessionResyncing;
    }
    if (topLoadKind === "sessionRefresh" || topLoadKind === "sessionDetail" || topLoadKind === "sessionHydration") {
      return topLoadLabel;
    }
    return "";
  });
  const showTopLoadBar = $derived(Boolean(topLoadLabel || sessionSyncLabel));
  const showTopLoadPill = $derived(
    Boolean(topLoadLabel) &&
      topLoadKind !== "sessionRefresh" &&
      topLoadKind !== "sessionDetail" &&
      topLoadKind !== "sessionHydration"
  );
  const showComposerSyncPill = $derived(
    activeWorkspaceTabId === "chat" &&
      Boolean(selectedSessionId) &&
      Boolean(sessionSyncLabel)
  );
  const topLoadPercent = $derived.by(() => {
    if (topLoadKind === "sessionHydration" && sessionHydrationPercent !== null) {
      return sessionHydrationPercent;
    }
    if (topLoadKind !== "idle") {
      return Math.max(8, Math.min(94, Math.round(fakeTopLoadPercent || 12)));
    }
    return 0;
  });
  const connectionBannerText = $derived.by(() => {
    const _locale = $localeSignal;
    if (connectionState === "reconnecting") {
      return ui.reconnecting;
    }
    if (connectionState === "connecting") {
      return ui.connectingRealtime;
    }
    if (connectionState === "disconnected") {
      return ui.realtimeDisconnected;
    }
    return "";
  });
  const showConnectionSnackbar = $derived(Boolean(connectionBannerText) && authenticated === true);
  const connectionSnackbarTone = $derived.by(() => {
    if (connectionState === "disconnected") {
      return "error";
    }
    if (connectionState === "reconnecting") {
      return "warning";
    }
    return "info";
  });
  const feedbackSnackbar = $derived.by(() => {
    if (authenticated !== true) {
      return null;
    }
    if (errorText) {
      return {
        tone: "error" as const,
        text: errorText,
        dismissible: true
      };
    }
    if (noticeText) {
      return {
        tone: "success" as const,
        text: noticeText,
        dismissible: true
      };
    }
    if (remoteControlStatus?.status === "errored" && remoteControlStatus.updatedAt > dismissedRemoteControlErrorAt) {
      return {
        tone: "warning" as const,
        text: m.computer_use_connection_failed(),
        dismissible: true
      };
    }
    return null;
  });
  const showPwaInstallAction = $derived.by(
    () => authenticated === true && (pwaInstalled || deferredInstallPrompt !== null || pwaManualInstallOnly)
  );
  const sessionLoadPercent = $derived.by(() => {
    if (sessionsLoadingMore) {
      return Math.max(42, Math.min(96, Math.round(fakeTopLoadPercent || 42)));
    }
    if (sessionsBusy) {
      return Math.max(16, Math.min(88, Math.round(fakeTopLoadPercent || 18)));
    }
    return 0;
  });
  const inlineGenerationState = $derived.by(() => {
    const _locale = $localeSignal;
    if (uploading) {
      return null;
    }
    if (sending && sendIntent === "queue") {
      return { icon: "schedule_send", label: ui.queueingFollowUp };
    }
    if (sending && sendIntent === "steer") {
      return { icon: "tune", label: ui.steeringCurrentTurn };
    }
    if (visibleOptimisticMessage) {
      return { icon: "autorenew", label: ui.generatingResponse };
    }
    if (running) {
      return { icon: "autorenew", label: ui.generatingResponse };
    }
    if (hasPendingQueueRequests) {
      return { icon: "schedule_send", label: ui.queueingFollowUp };
    }
    return null;
  });
  const subagentTasks = $derived.by(() => {
    if (!conversation) {
      return [] as SubagentTaskEntry[];
    }

    const tasks: SubagentTaskEntry[] = [];
    for (const turn of [...conversation.thread.turns].reverse()) {
      for (const item of turn.items) {
        if (item.type !== "collabAgentToolCall") {
          continue;
        }

        tasks.push({
          key: `${turn.id}:${item.id}`,
          turnId: turn.id,
          itemId: item.id,
          tool: String(item.tool ?? "spawn_agent"),
          status: String(item.status ?? "unknown"),
          prompt: String(item.prompt ?? "").trim(),
          model: String(item.model ?? "default"),
          reasoningEffort: String(item.reasoningEffort ?? "default"),
          primaryThreadId: getPrimarySubagentThreadId(item),
          states: getSubagentStates(item)
        });
      }
    }

    return tasks;
  });
  const selectedComputerFrame = $derived.by(() => {
    if (!selectedSessionId) {
      return null;
    }
    const streamedFrame = computerFramesBySessionId[selectedSessionId];
    if (streamedFrame) {
      return streamedFrame;
    }
    if (!conversation || conversation.thread.id !== selectedSessionId) {
      return null;
    }
    for (let turnIndex = conversation.thread.turns.length - 1; turnIndex >= 0; turnIndex -= 1) {
      const turn = conversation.thread.turns[turnIndex];
      for (let itemIndex = turn.items.length - 1; itemIndex >= 0; itemIndex -= 1) {
        const item = turn.items[itemIndex];
        if (item.type !== "dynamicToolCall" && item.type !== "mcpToolCall") {
          continue;
        }
        const toolText = [
          item.namespace,
          item.serverName,
          item.tool,
          item.toolName,
          item.name,
          item.title,
          item.detailPreview
        ]
          .filter((value) => typeof value === "string")
          .join(" ")
          .toLowerCase();
        if (!["computer", "screenshot", "browser", "desktop", "remote"].some((needle) => toolText.includes(needle))) {
          continue;
        }
        const imageUrl = getDynamicToolImageUrls(item).at(-1);
        if (!imageUrl) {
          continue;
        }
        return {
          threadId: selectedSessionId,
          turnId: turn.id,
          itemId: item.id,
          imageUrl,
          mimeType: imageUrl.startsWith("data:image/") ? (imageUrl.slice(5).split(";")[0] ?? null) : null,
          tool: String(item.tool ?? item.toolName ?? item.title ?? ui.computerFrameUpdated),
          transport: "websocket",
          frameMode: "snapshot",
          fpsHint: 1,
          updatedAt: turn.completedAt ?? conversation.thread.updatedAt ?? Date.now()
        } satisfies ComputerFramePayload;
      }
    }
    return null;
  });
  const workspaceTabs = $derived.by(() => {
    const _locale = $localeSignal;
    const tabs: Array<{ id: WorkspaceTabId; label: string; kind: "chat" | "tasks" | "git" | "settings" | "computer" | "diagnostics" | "memory" | "git-diff" | "code-diff" | "file" | "terminal" }> = [
      { id: "chat", label: ui.chat, kind: "chat" }
    ];
    if (tasksTabOpen) {
      tabs.push({
        id: "tasks",
        label: subagentTasks.length > 0 ? `${ui.tasks} ${subagentTasks.length}` : ui.tasks,
        kind: "tasks"
      });
    }
    if (gitTabOpen) {
      tabs.push({
        id: "git",
        label: ui.gitWorkspace,
        kind: "git"
      });
    }
    if (settingsTabOpen) {
      tabs.push({
        id: "settings",
        label: ui.settings,
        kind: "settings"
      });
    }
    if (computerTabOpen) {
      tabs.push({
        id: "computer",
        label: ui.computer,
        kind: "computer"
      });
    }
    if (diagnosticsTabOpen) {
      tabs.push({
        id: "diagnostics",
        label: ui.diagnostics,
        kind: "diagnostics"
      });
    }
    if (memoryTabOpen) {
      tabs.push({
        id: "memory",
        label: ui.memory,
        kind: "memory"
      });
    }
    for (const tab of gitDiffTabs) {
      tabs.push({
        id: tab.id,
        label: tab.label,
        kind: "git-diff"
      });
    }
    for (const tab of codeDiffTabs) {
      tabs.push({
        id: tab.id,
        label: tab.label,
        kind: "code-diff"
      });
    }
    for (const tab of fileTabs) {
      tabs.push({
        id: tab.id,
        label: tab.label,
        kind: "file"
      });
    }
    for (const terminal of terminals) {
      tabs.push({
        id: `terminal:${terminal.id}`,
        label: terminal.title,
        kind: "terminal"
      });
    }
    return tabs;
  });
  const activeGitDiffTab = $derived.by(() => gitDiffTabs.find((tab) => tab.id === activeWorkspaceTabId) ?? null);
  const activeCodeDiffTab = $derived.by(() => codeDiffTabs.find((tab) => tab.id === activeWorkspaceTabId) ?? null);
  const activeFileTab = $derived.by(() => fileTabs.find((tab) => tab.id === activeWorkspaceTabId) ?? null);
  const startupPausedQueues = $derived(config?.startup.pausedQueues ?? []);
  const startupScheduledShutdown = $derived.by(() => {
    const shutdown = config?.startup.scheduledShutdown ?? null;
    if (!shutdown || shutdown.scheduledFor <= startupAlertNow) {
      return null;
    }
    return shutdown;
  });
  const startupShutdownRemainingSeconds = $derived.by(() =>
    startupScheduledShutdown ? Math.max(0, Math.ceil((startupScheduledShutdown.scheduledFor - startupAlertNow) / 1000)) : null
  );
  const startupScheduledShutdownThreadLabel = $derived.by(() => {
    if (!startupScheduledShutdown?.sessionId) {
      return null;
    }

    const sessionName =
      sessions.find((session) => session.id === startupScheduledShutdown.sessionId)?.name ||
      (conversation?.thread.id === startupScheduledShutdown.sessionId
        ? getConversationDisplayTitle(conversation) || getDefaultThreadTitle()
        : startupScheduledShutdown.sessionId);
    return ui.startupAlertShutdownThread(sessionName);
  });

  async function ensureLazyWorkspaceLoaded(kind: LazyWorkspaceKind) {
    if (kind === "arena" && ArenaWorkspaceView) {
      return;
    }
    if (kind === "codeDiff" && CodeDiffWorkspaceView) {
      return;
    }
    if (kind === "diagnostics" && DiagnosticsWorkspaceView) {
      return;
    }
    if (kind === "file" && FileWorkspaceView) {
      return;
    }
    if (kind === "git" && GitWorkspaceView) {
      return;
    }
    if (kind === "memory" && MemoryWorkspaceView) {
      return;
    }
    if (kind === "settings" && SettingsWorkspaceView) {
      return;
    }
    if (kind === "terminal" && TerminalWorkspaceView) {
      return;
    }

    const existing = lazyWorkspaceLoads.get(kind);
    if (existing) {
      await existing;
      return;
    }
    if (lazyWorkspaceLoadErrors[kind]) {
      lazyWorkspaceLoadErrors = {
        ...lazyWorkspaceLoadErrors,
        [kind]: undefined
      };
    }

    const pending = (async () => {
      if (kind === "arena") {
        const module = await import("$lib/components/ArenaWorkspace.svelte");
        ArenaWorkspaceView = module.default;
        return;
      }
      if (kind === "codeDiff") {
        const module = await import("$lib/components/CodeDiffWorkspace.svelte");
        CodeDiffWorkspaceView = module.default;
        return;
      }
      if (kind === "diagnostics") {
        const module = await import("$lib/components/DiagnosticsWorkspace.svelte");
        DiagnosticsWorkspaceView = module.default;
        return;
      }
      if (kind === "file") {
        const module = await import("$lib/components/FileWorkspace.svelte");
        FileWorkspaceView = module.default;
        return;
      }
      if (kind === "git") {
        const module = await import("$lib/components/GitWorkspace.svelte");
        GitWorkspaceView = module.default;
        return;
      }
      if (kind === "memory") {
        const module = await import("$lib/components/MemoryWorkspace.svelte");
        MemoryWorkspaceView = module.default;
        return;
      }
      if (kind === "settings") {
        let timeout: ReturnType<typeof setTimeout> | null = null;
        const module = await Promise.race([
          import("$lib/components/SettingsWorkspace.svelte"),
          new Promise<never>((_, reject) => {
            timeout = setTimeout(
              () => reject(new Error(getLocale().startsWith("ko") ? "설정 워크스페이스 로딩 시간이 초과됐습니다." : "Timed out loading the settings workspace.")),
              15_000
            );
          })
        ]).finally(() => {
          if (timeout) {
            clearTimeout(timeout);
          }
        });
        SettingsWorkspaceView = module.default;
        return;
      }

      const module = await import("$lib/components/TerminalWorkspace.svelte");
      TerminalWorkspaceView = module.default;
    })()
      .catch((error) => {
        const message = describeError(error);
        lazyWorkspaceLoadErrors = {
          ...lazyWorkspaceLoadErrors,
          [kind]: message
        };
        errorText = message;
      })
      .finally(() => {
        lazyWorkspaceLoads.delete(kind);
      });

    lazyWorkspaceLoads.set(kind, pending);
    await pending;
  }

  function getWorkspaceLoadingLabel() {
    if (activeWorkspaceTabId === "tasks") {
      return ui.taskCenter;
    }
    if (activeWorkspaceTabId === "settings") {
      return ui.settings;
    }
    if (activeWorkspaceTabId === "diagnostics") {
      return ui.diagnostics;
    }
    if (activeWorkspaceTabId === "memory") {
      return ui.memory;
    }
    if (activeWorkspaceTabId === "git" || activeGitDiffTab) {
      return ui.gitWorkspace;
    }
    if (activeCodeDiffTab) {
      return ui.aggregatedDiff;
    }
    if (activeFileTab) {
      return activeFileTab.label;
    }
    if (activeWorkspaceTabId.startsWith("terminal:")) {
      return ui.newTerminal;
    }
    return m.loading();
  }

  $effect(() => {
    if (activeWorkspaceTabId === "tasks") {
      void ensureLazyWorkspaceLoaded("arena");
      return;
    }
    if (activeWorkspaceTabId === "settings") {
      void ensureLazyWorkspaceLoaded("settings");
      return;
    }
    if (activeWorkspaceTabId === "diagnostics") {
      void ensureLazyWorkspaceLoaded("diagnostics");
      return;
    }
    if (activeWorkspaceTabId === "memory") {
      void ensureLazyWorkspaceLoaded("memory");
      return;
    }
    if (activeWorkspaceTabId === "git" || Boolean(activeGitDiffTab)) {
      void ensureLazyWorkspaceLoaded("git");
      return;
    }
    if (activeCodeDiffTab) {
      void ensureLazyWorkspaceLoaded("codeDiff");
      return;
    }
    if (activeFileTab) {
      void ensureLazyWorkspaceLoaded("file");
      return;
    }
    if (activeWorkspaceTabId.startsWith("terminal:")) {
      void ensureLazyWorkspaceLoaded("terminal");
    }
  });

  function dismissFeedbackSnackbar() {
    errorText = "";
    noticeText = "";
    if (remoteControlStatus?.status === "errored") {
      dismissedRemoteControlErrorAt = remoteControlStatus.updatedAt;
    }
  }

  $effect(() => {
    if (authenticated !== true || typeof window === "undefined") {
      return;
    }

    const snackbar = feedbackSnackbar;
    if (!snackbar) {
      return;
    }

    const timeoutMs = snackbar.tone === "error" ? 6400 : 3600;
    const capturedError = errorText;
    const capturedNotice = noticeText;
    const timer = window.setTimeout(() => {
      if (capturedError && errorText === capturedError) {
        errorText = "";
      }
      if (!capturedError && capturedNotice && noticeText === capturedNotice) {
        noticeText = "";
      }
    }, timeoutMs);

    return () => {
      window.clearTimeout(timer);
    };
  });

  onMount(() => {
    themeMode = readThemeMode();
    resolvedTheme = getResolvedTheme();
    releaseThemeListener = subscribeThemeChange((detail) => {
      themeMode = detail.mode;
      resolvedTheme = detail.resolved;
    });

    const mobileQuery = window.matchMedia("(max-width: 900px)");
    const syncMobileLayout = () => {
      isMobileLayout = mobileQuery.matches;
      if (!mobileQuery.matches) {
        mobileSidebarOpen = false;
      }
    };
    releaseReconnectListener = api.onReconnect(() => {
      if (authenticated === true) {
        void recoverFromReconnect();
      }
    });
    releaseResyncRequiredListener = api.onResyncRequired(() => {
      recoverFromWebSocketResync();
    });
    releaseConnectionStateListener = api.onConnectionState((state) => {
      connectionState = state;
    });
    const handleViewportChange = () => {
      if (composerSettingsOpen) {
        void updateComposerSettingsPopoverPosition();
      }
      if (sessionTurnSearchOpen) {
        void updateSessionTurnSearchPopoverPosition();
      }
    };
    const handleGlobalKeydown = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "f" && selectedSessionId && activeWorkspaceTabId === "chat") {
        event.preventDefault();
        sessionTurnSearchOpen = true;
        queueMicrotask(() => {
          sessionTurnSearchInputElement?.focus();
          sessionTurnSearchInputElement?.select();
        });
      }
    };
    const syncPwaInstallState = () => {
      const installed =
        window.matchMedia("(display-mode: standalone)").matches ||
        (window.navigator as Navigator & { standalone?: boolean }).standalone === true;
      pwaInstalled = installed;
      if (installed) {
        deferredInstallPrompt = null;
        pwaManualInstallOnly = false;
        return;
      }

      const touchMac = window.navigator.platform === "MacIntel" && window.navigator.maxTouchPoints > 1;
      const appleMobile = /iphone|ipad|ipod/iu.test(window.navigator.userAgent) || touchMac;
      pwaManualInstallOnly = appleMobile && deferredInstallPrompt === null;
    };
    const handleBeforeInstallPrompt = (event: Event) => {
      event.preventDefault();
      deferredInstallPrompt = event as BeforeInstallPromptEvent;
      pwaManualInstallOnly = false;
      syncPwaInstallState();
    };
    const handleAppInstalled = () => {
      deferredInstallPrompt = null;
      pwaInstallBusy = false;
      syncPwaInstallState();
      noticeText = ui.appInstalledNotice;
    };
    const displayModeQuery = window.matchMedia("(display-mode: standalone)");
    const handleDisplayModeChange = () => {
      syncPwaInstallState();
    };
    let lastForegroundReconnectAt = 0;
    const requestForegroundReconnect = () => {
      if (authenticated !== true) {
        return;
      }

      const now = Date.now();
      if (now - lastForegroundReconnectAt < 750) {
        return;
      }

      lastForegroundReconnectAt = now;
      api.reconnectNow();
    };
    let hiddenStartedAt: number | null = document.hidden ? Date.now() : null;
    const handleVisibilityChange = () => {
      if (document.hidden) {
        hiddenStartedAt = Date.now();
        return;
      }

      requestForegroundReconnect();
      const lastHiddenAt = hiddenStartedAt;
      hiddenStartedAt = null;
      if (!selectedSessionId || connectionState !== "connected" || lastHiddenAt === null) {
        return;
      }

      const hiddenDurationMs = Date.now() - lastHiddenAt;
      if (hiddenDurationMs < staleSessionCatchupHiddenThresholdMs) {
        return;
      }

      beginStaleSessionCatchup(selectedSessionId, hiddenDurationMs);
    };
    let notificationPromptRecorded = false;
    try {
      notificationPromptRecorded = window.localStorage.getItem(notificationPromptStorageKey) !== null;
      if (typeof Notification !== "undefined" && Notification.permission !== "default") {
        window.localStorage.setItem(notificationPromptStorageKey, Notification.permission);
        notificationPromptRecorded = true;
      }
    } catch {
      notificationPromptRecorded = false;
    }
    const requestNotificationPermissionFromGesture = () => {
      if (typeof Notification === "undefined" || Notification.permission !== "default") {
        return;
      }
      if (notificationPromptRecorded) {
        return;
      }
      if (notificationPermissionRequested) {
        return;
      }
      notificationPermissionRequested = true;
      void Notification.requestPermission()
        .then((permission) => {
          try {
            window.localStorage.setItem(notificationPromptStorageKey, permission);
            notificationPromptRecorded = true;
          } catch {
            notificationPromptRecorded = true;
          }
        })
        .catch(() => {})
        .finally(() => {
          notificationPermissionRequested = false;
        });
    };
    syncMobileLayout();
    syncPwaInstallState();
    mobileQuery.addEventListener("change", syncMobileLayout);
    if (typeof displayModeQuery.addEventListener === "function") {
      displayModeQuery.addEventListener("change", handleDisplayModeChange);
    } else {
      displayModeQuery.addListener(handleDisplayModeChange);
    }
    window.addEventListener("resize", handleViewportChange);
    window.addEventListener("scroll", handleViewportChange, true);
    window.visualViewport?.addEventListener("resize", handleViewportChange);
    window.visualViewport?.addEventListener("scroll", handleViewportChange);
    window.addEventListener("keydown", handleGlobalKeydown, true);
    window.addEventListener("pointerdown", requestNotificationPermissionFromGesture, true);
    window.addEventListener("keydown", requestNotificationPermissionFromGesture, true);
    window.addEventListener("beforeinstallprompt", handleBeforeInstallPrompt);
    window.addEventListener("appinstalled", handleAppInstalled);
    window.addEventListener("focus", requestForegroundReconnect);
    window.addEventListener("online", requestForegroundReconnect);
    window.addEventListener("pageshow", requestForegroundReconnect);
    document.addEventListener("visibilitychange", handleVisibilityChange, true);
    void bootstrap();

    return () => {
      disconnectStream();
      clearHydrationRefresh();
      if (sessionRefreshTimer) {
        clearTimeout(sessionRefreshTimer);
      }
      if (selectedSessionDetailRefreshTimer) {
        clearTimeout(selectedSessionDetailRefreshTimer);
      }
      clearSelectedSessionCompletionRefreshes();
      if (saveTimer) {
        clearTimeout(saveTimer);
      }
      if (draftSaveTimer) {
        clearTimeout(draftSaveTimer);
      }
      if (sessionTurnSearchTimer) {
        clearTimeout(sessionTurnSearchTimer);
      }
      if (sessionTurnSearchHighlightTimer) {
        clearTimeout(sessionTurnSearchHighlightTimer);
      }
      if (staleSessionCatchupTimer) {
        clearTimeout(staleSessionCatchupTimer);
      }
      if (sessionListCachePersistTimer) {
        clearTimeout(sessionListCachePersistTimer);
      }
      if (sessionDetailCachePersistTimer) {
        clearTimeout(sessionDetailCachePersistTimer);
      }
      sessionDetailCachePersistMode = null;
      queuedSessionDetailCachePersist = null;
      if (transcriptScrollFrame !== null) {
        cancelAnimationFrame(transcriptScrollFrame);
      }
      if (transcriptMeasurementFrame !== null) {
        cancelAnimationFrame(transcriptMeasurementFrame);
      }
      clearTranscriptPinReleaseWait();
      if (composerTextareaResizeFrame !== null) {
        cancelAnimationFrame(composerTextareaResizeFrame);
      }
      transcriptResizeObserver?.disconnect();
      transcriptResizeObserver = null;
      transcriptDockResizeObserver?.disconnect();
      transcriptDockResizeObserver = null;
      for (const timer of itemDetailRefreshTimers.values()) {
        clearTimeout(timer);
      }
      itemDetailRefreshTimers.clear();
      releaseGlobalStream?.();
      releaseGlobalStream = null;
      releaseReconnectListener?.();
      releaseReconnectListener = null;
      releaseResyncRequiredListener?.();
      releaseResyncRequiredListener = null;
      releaseConnectionStateListener?.();
      releaseConnectionStateListener = null;
      releaseThemeListener?.();
      releaseThemeListener = null;
      mobileQuery.removeEventListener("change", syncMobileLayout);
      if (typeof displayModeQuery.removeEventListener === "function") {
        displayModeQuery.removeEventListener("change", handleDisplayModeChange);
      } else {
        displayModeQuery.removeListener(handleDisplayModeChange);
      }
      window.removeEventListener("resize", handleViewportChange);
      window.removeEventListener("scroll", handleViewportChange, true);
      window.visualViewport?.removeEventListener("resize", handleViewportChange);
      window.visualViewport?.removeEventListener("scroll", handleViewportChange);
      window.removeEventListener("keydown", handleGlobalKeydown, true);
      window.removeEventListener("pointerdown", requestNotificationPermissionFromGesture, true);
      window.removeEventListener("keydown", requestNotificationPermissionFromGesture, true);
      window.removeEventListener("beforeinstallprompt", handleBeforeInstallPrompt);
      window.removeEventListener("appinstalled", handleAppInstalled);
      window.removeEventListener("focus", requestForegroundReconnect);
      window.removeEventListener("online", requestForegroundReconnect);
      window.removeEventListener("pageshow", requestForegroundReconnect);
      document.removeEventListener("visibilitychange", handleVisibilityChange, true);
      api.disconnect();
    };
  });

  $effect(() => {
    if (typeof window === "undefined") {
      return;
    }
    if (authenticated !== false || !loginHcaptcha.enabled || !loginHcaptcha.siteKey || !loginHcaptchaContainer) {
      return;
    }
    if (loginHcaptchaWidgetId !== null) {
      return;
    }

    let cancelled = false;
    const renderWidget = async () => {
      try {
        if (!window.hcaptcha) {
          if (!loginHcaptchaScriptPromise) {
            loginHcaptchaScriptPromise = new Promise<void>((resolve, reject) => {
              const existingScript = document.querySelector<HTMLScriptElement>('script[data-codex-webui-hcaptcha="true"]');
              if (existingScript) {
                if (existingScript.dataset.loaded === "true") {
                  resolve();
                  return;
                }
                existingScript.addEventListener("load", () => resolve(), { once: true });
                existingScript.addEventListener("error", () => reject(new Error("Failed to load hCaptcha.")), { once: true });
                return;
              }

              const script = document.createElement("script");
              script.src = "https://js.hcaptcha.com/1/api.js?render=explicit";
              script.async = true;
              script.defer = true;
              script.dataset.codexWebuiHcaptcha = "true";
              script.addEventListener("load", () => {
                script.dataset.loaded = "true";
                resolve();
              }, { once: true });
              script.addEventListener("error", () => reject(new Error("Failed to load hCaptcha.")), { once: true });
              document.head.appendChild(script);
            });
          }
          await loginHcaptchaScriptPromise;
        }

        const siteKey = loginHcaptcha.siteKey;
        if (cancelled || !window.hcaptcha || !loginHcaptchaContainer || !siteKey) {
          return;
        }

        loginHcaptchaWidgetId = window.hcaptcha.render(loginHcaptchaContainer, {
          sitekey: siteKey,
          theme: resolvedTheme === "dark" ? "dark" : "light",
          callback: (token) => {
            loginHcaptchaToken = token;
            loginMessage = "";
          },
          "expired-callback": () => {
            loginHcaptchaToken = "";
          },
          "error-callback": () => {
            loginHcaptchaToken = "";
          }
        });
      } catch {
        if (!cancelled) {
          loginMessage = ui.hcaptchaLoadFailed;
        }
      }
    };

    void renderWidget();

    return () => {
      cancelled = true;
    };
  });

  $effect(() => {
    const scheduledFor = config?.startup.scheduledShutdown?.scheduledFor ?? null;
    if (!scheduledFor) {
      return;
    }

    startupAlertNow = Date.now();
    const timer = setInterval(() => {
      startupAlertNow = Date.now();
    }, 1000);

    return () => {
      clearInterval(timer);
    };
  });

  $effect(() => {
    const nextDisplayTitle = getConversationDisplayTitle(conversation) ?? "";
    const activeTitleInput = titleInputElement;
    if (
      activeTitleInput &&
      typeof document !== "undefined" &&
      document.activeElement === activeTitleInput
    ) {
      return;
    }
    if (titleDraft !== nextDisplayTitle) {
      titleDraft = nextDisplayTitle;
    }
  });

  $effect(() => {
    if (typeof window === "undefined" || !sessionListNeedsActiveStatusPolling) {
      return;
    }

    const timer = window.setInterval(() => {
      if (
        authenticated !== true ||
        sessionsBusy ||
        connectionState !== "connected" ||
        (typeof document !== "undefined" && document.hidden)
      ) {
        return;
      }

      if (Date.now() - lastLiveSessionEvidenceAt < activeSessionStatusPollMs) {
        return;
      }

      scheduleSessionRefresh(0);
    }, activeSessionStatusPollMs);

    return () => {
      window.clearInterval(timer);
    };
  });

  $effect(() => {
    if (!sessionTurnSearchOpen || !sessionTurnSearchInputElement) {
      return;
    }

    queueMicrotask(() => {
      sessionTurnSearchInputElement?.focus();
      sessionTurnSearchInputElement?.select();
    });
  });

  $effect(() => {
    if (
      !conversation ||
      !transcriptElement ||
      (!stickTranscriptToBottom && !forceTranscriptScroll && !pendingTranscriptBottomScroll)
    ) {
      return;
    }
    scheduleTranscriptScrollToBottom();
  });

  $effect(() => {
    if (
      !transcriptElement ||
      !loadingDetail ||
      conversation ||
      !isInitialTranscriptScrollPending()
    ) {
      return;
    }
    requestTranscriptBottomScroll(true);
    scheduleTranscriptScrollToBottom();
  });

  $effect(() => {
    if (!transcriptElement || !conversation) {
      return;
    }
    if (!inlineGenerationState && !loadingDetail && !sending && !pendingTranscriptBottomScroll) {
      return;
    }
    if (!stickTranscriptToBottom && !forceTranscriptScroll && !pendingTranscriptBottomScroll) {
      return;
    }
    scheduleTranscriptScrollToBottom();
  });

  $effect(() => {
    if (
      !transcriptElement ||
      !conversation ||
      loadingDetail ||
      loadingOlderTurns ||
      (!pendingTranscriptBottomScroll && !forceTranscriptScroll)
    ) {
      return;
    }
    scheduleTranscriptScrollToBottom();
  });

  $effect(() => {
    const sessionId = conversation?.thread.id ?? null;
    const turnCount = conversation?.thread.turns.length ?? 0;
    const firstTurnId = conversation?.thread.turns[0]?.id ?? null;
    const lastTurnId = conversation?.thread.turns.at(-1)?.id ?? null;
    const sessionChanged = transcriptTurnWindowSessionId !== sessionId;
    turnCount;
    firstTurnId;

    if (sessionChanged) {
      transcriptTurnWindowSessionId = sessionId;
      if (transcriptMeasurementFrame !== null && typeof window !== "undefined") {
        cancelAnimationFrame(transcriptMeasurementFrame);
        transcriptMeasurementFrame = null;
      }
      pendingTranscriptTurnHeights.clear();
      transcriptTurnHeights.clear();
      transcriptTurnLayout = EMPTY_TRANSCRIPT_LAYOUT;
      transcriptTurnWindow = EMPTY_TRANSCRIPT_WINDOW;
      transcriptPinnedTurnId = null;
      transcriptMeasuredWidth = transcriptElement?.clientWidth ?? 0;
      clearTranscriptPinReleaseWait();
    }

    void tick().then(() => {
      if ((conversation?.thread.id ?? null) !== sessionId) {
        return;
      }
      const anchorNewestTurn = sessionChanged || stickTranscriptToBottom || isInitialTranscriptScrollPending(sessionId);
      refreshTranscriptTurnWindow(anchorNewestTurn ? lastTurnId : null, anchorNewestTurn ? "end" : "center");
      if (anchorNewestTurn) {
        scheduleTranscriptScrollToBottom();
      }
    });
  });

  $effect(() => {
    if (typeof window === "undefined" || !transcriptContentElement) {
      transcriptResizeObserver?.disconnect();
      transcriptResizeObserver = null;
      return;
    }

    transcriptResizeObserver?.disconnect();
    transcriptResizeObserver = new ResizeObserver(() => {
      const nextWidth = transcriptElement?.clientWidth ?? 0;
      const widthChanged = transcriptMeasuredWidth > 0 && nextWidth > 0 && Math.abs(nextWidth - transcriptMeasuredWidth) >= 1;
      transcriptMeasuredWidth = nextWidth;
      if (widthChanged) {
        const anchor = stickTranscriptToBottom ? null : captureTranscriptScrollAnchor();
        pendingTranscriptTurnHeights.clear();
        transcriptTurnHeights.clear();
        rebuildTranscriptTurnLayout();
        if (anchor) {
          void restoreTranscriptScrollAnchor(anchor);
        } else if (stickTranscriptToBottom) {
          const newestTurnId = conversation?.thread.turns.at(-1)?.id ?? null;
          refreshTranscriptTurnWindow(newestTurnId, "end");
          scheduleTranscriptScrollToBottom();
        } else {
          refreshTranscriptTurnWindow();
        }
        return;
      }
      refreshTranscriptTurnWindow();
      if (!transcriptElement || loadingOlderTurns || (!stickTranscriptToBottom && !forceTranscriptScroll)) {
        return;
      }
      scheduleTranscriptScrollToBottom();
    });
    transcriptResizeObserver.observe(transcriptContentElement);
    if (transcriptElement) {
      transcriptResizeObserver.observe(transcriptElement);
    }

    return () => {
      transcriptResizeObserver?.disconnect();
      transcriptResizeObserver = null;
    };
  });

  $effect(() => {
    const transcript = transcriptElement;
    if (typeof window === "undefined" || !transcript) {
      return;
    }

    const handleWheel = () => {
      handleTranscriptWheel();
    };
    const handleTouchMove = () => {
      handleTranscriptTouchMove();
    };
    const handlePointerMove = (event: Event) => {
      handleTranscriptPointerMove(event as PointerEvent);
    };

    transcript.addEventListener("wheel", handleWheel, { passive: true });
    transcript.addEventListener("touchmove", handleTouchMove, { passive: true });
    transcript.addEventListener("pointermove", handlePointerMove, { passive: true });

    return () => {
      transcript.removeEventListener("wheel", handleWheel);
      transcript.removeEventListener("touchmove", handleTouchMove);
      transcript.removeEventListener("pointermove", handlePointerMove);
    };
  });

  $effect(() => {
    if (typeof window === "undefined" || activeWorkspaceTabId !== "chat") {
      transcriptDockResizeObserver?.disconnect();
      transcriptDockResizeObserver = null;
      return;
    }

    const dock = transcriptDockElement;
    if (!dock) {
      return;
    }

    syncTranscriptDockReserve();
    transcriptDockResizeObserver?.disconnect();
    transcriptDockResizeObserver = new ResizeObserver(() => {
      syncTranscriptDockReserve();
    });
    transcriptDockResizeObserver.observe(dock);

    return () => {
      transcriptDockResizeObserver?.disconnect();
      transcriptDockResizeObserver = null;
    };
  });

  $effect(() => {
    if (typeof window === "undefined" || activeWorkspaceTabId !== "chat") {
      composerToolbarResizeObserver?.disconnect();
      composerToolbarResizeObserver = null;
      return;
    }

    const toolbarSignature = [
      composerSettingsSummary.model,
      composerSettingsSummary.speed,
      running ? "running" : "idle",
      composerQueueModeActive ? "queue" : "send",
      draftAttachments.length
    ].join(":");
    toolbarSignature;

    const syncComposerToolbarCompact = () => {
      const toolbar = composerToolbarElement;
      if (!toolbar) {
        return;
      }

      const modelWidthBias = Math.max(
        0,
        Math.min(isMobileLayout ? 56 : 84, (composerSettingsSummary.model.length - 9) * (isMobileLayout ? 3 : 4))
      );
      const threshold =
        (conversation
          ? running
            ? isMobileLayout
              ? 430
              : 560
            : isMobileLayout
              ? 338
              : 470
          : isMobileLayout
            ? 220
            : 340) +
        modelWidthBias +
        (composerSettingsSummary.speed === "flex" ? (isMobileLayout ? 10 : 18) : 0) +
        (draftAttachments.length > 0 ? (isMobileLayout ? 8 : 12) : 0);

      composerToolbarCompact = toolbar.clientWidth < threshold;
    };

    const toolbar = composerToolbarElement;
    if (!toolbar) {
      return;
    }

    syncComposerToolbarCompact();
    composerToolbarResizeObserver?.disconnect();
    composerToolbarResizeObserver = new ResizeObserver(() => {
      syncComposerToolbarCompact();
    });
    composerToolbarResizeObserver.observe(toolbar);

    return () => {
      composerToolbarResizeObserver?.disconnect();
      composerToolbarResizeObserver = null;
    };
  });

  $effect(() => {
    if (activeWorkspaceTabId !== "chat" || !composerTextareaElement) {
      return;
    }

    queueMicrotask(() => {
      scheduleComposerTextareaResize();
    });
  });

  $effect(() => {
    if (!composerSettingsOpen) {
      composerSettingsPopoverStyle = "";
      return;
    }
    const _composerSettingsTab = composerSettingsTab;
    const _composerSettingsAnchor = composerSettingsAnchor;
    void updateComposerSettingsPopoverPosition();
  });

  $effect(() => {
    if (!sessionTurnSearchOpen) {
      sessionTurnSearchPopoverStyle = "";
      return;
    }
    void updateSessionTurnSearchPopoverPosition();
  });

  $effect(() => {
    if (!optimisticMessage) {
      return;
    }
    if (!selectedSessionBindingMatches(optimisticMessage.sessionId, optimisticMessage.profileId)) {
      optimisticMessage = null;
      return;
    }
    if (!conversation || conversation.thread.id !== optimisticMessage.sessionId) {
      return;
    }

    const newestTurn = conversation.thread.turns.at(-1);
    const hasEcho = hasConversationEchoedOptimisticMessage(conversation, optimisticMessage);
    const hasCompletedReplacementTurn =
      Boolean(newestTurn) &&
      conversation.thread.turns.length > optimisticMessage.baselineTurnCount &&
      newestTurn?.id !== optimisticMessage.baselineTurnId &&
      String(newestTurn?.status ?? "") !== "inProgress";

    if (hasEcho || hasCompletedReplacementTurn) {
      optimisticMessage = null;
    }
  });

  $effect(() => {
    if (!pendingQueueModeSessionKey) {
      return;
    }
    if (!conversation || selectedSessionStateKey() !== pendingQueueModeSessionKey) {
      return;
    }
    if (hasQueueableConversationActivity(conversation)) {
      return;
    }
    clearPendingQueueMode();
  });

  $effect(() => {
    if (activeLiveTurnId !== lastActiveLiveTurnId) {
      lastActiveLiveTurnId = activeLiveTurnId;
      liveTurnCardExpanded = false;
    }
  });

  $effect(() => {
    if (!editingQueueId) {
      return;
    }
    const queuedItem = queuedMessages.find((item) => item.id === editingQueueId);
    if (!queuedItem) {
      editingQueueId = null;
      editingQueuePrompt = "";
    }
  });

  $effect(() => {
    if (!editingQueueId || !selectedSessionId) {
      return;
    }
    if (conversation?.thread.id !== selectedSessionId) {
      editingQueueId = null;
      editingQueuePrompt = "";
    }
  });

  $effect(() => {
    const dragState = queueDragState;
    if (!dragState) {
      return;
    }
    if (!queuedMessages.some((item) => item.id === dragState.queueId)) {
      queueDragState = null;
    }
  });

  $effect(() => {
    const currentConversationId = conversation?.thread.id ?? null;
    if (!currentConversationId) {
      lastLoadedConversationId = null;
      editingQueueId = null;
      editingQueuePrompt = "";
      return;
    }
    if (currentConversationId !== lastLoadedConversationId) {
      lastLoadedConversationId = currentConversationId;
      mobileSidebarOpen = false;
      editingQueueId = null;
      editingQueuePrompt = "";
    }
  });

  $effect(() => {
    const kind = topLoadKind;
    const hydrationPercent = sessionHydrationPercent;

    if (kind === "idle") {
      fakeTopLoadPercent = 0;
      return;
    }

    if (kind === "sessionHydration" && hydrationPercent !== null) {
      fakeTopLoadPercent = hydrationPercent;
      return;
    }

    const caps: Record<string, number> = {
      olderTurns: 86,
      sessionsInitial: 78,
      sessionsRefresh: 84,
      sessionsMore: 90,
      sessionRefresh: 76,
      sessionDetail: 72,
      sessionHydration: 88
    };
    const starts: Record<string, number> = {
      olderTurns: 18,
      sessionsInitial: 14,
      sessionsRefresh: 28,
      sessionsMore: 42,
      sessionRefresh: 22,
      sessionDetail: 20,
      sessionHydration: 24
    };
    const cap = caps[kind] ?? 80;

    if (fakeTopLoadPercent <= 0 || fakeTopLoadPercent > cap) {
      fakeTopLoadPercent = starts[kind] ?? 16;
    }

    const timer = setInterval(() => {
      fakeTopLoadPercent = Math.min(cap, fakeTopLoadPercent + Math.max(1, Math.ceil((cap - fakeTopLoadPercent) / 7)));
    }, 160);

    return () => {
      clearInterval(timer);
    };
  });

  function getSessionSortPriority(status: string | null | undefined) {
    return status === "running" || status === "active" ? 1 : 0;
  }

  function normalizeSessionFilterState(filter: Partial<SessionSummaryFilter> | null | undefined): SessionSummaryFilter {
    return {
      pinnedOnly: Boolean(filter?.pinnedOnly),
      runningOnly: Boolean(filter?.runningOnly),
      queuedOnly: Boolean(filter?.queuedOnly),
      untaggedOnly: Boolean(filter?.untaggedOnly),
      highlight: filter?.highlight === "attention" || filter?.highlight === "completed" ? filter.highlight : "all",
      tags: Array.isArray(filter?.tags)
        ? [...new Set(filter.tags.map((entry) => entry.trim()).filter((entry) => entry.length > 0))]
        : []
    };
  }

  function isDefaultSessionFilter(filter: SessionSummaryFilter) {
    return (
      !filter.pinnedOnly &&
      !filter.runningOnly &&
      !filter.queuedOnly &&
      !filter.untaggedOnly &&
      filter.highlight === "all" &&
      filter.tags.length === 0
    );
  }

  function isEnabledQueryParam(value: string | null) {
    return value === "1" || value === "true";
  }

  function readSessionListStateFromUrl() {
    if (typeof window === "undefined") {
      return;
    }

    const params = new URL(window.location.href).searchParams;
    const query = params.get(sessionSearchQueryParamKey)?.trim() ?? "";
    const scope = params.get(sessionSearchScopeParamKey);
    const highlight = params.get(sessionFilterHighlightParamKey);
    const folder = params.get(sessionFolderParamKey)?.trim() ?? "";
    const untaggedOnly = !folder && isEnabledQueryParam(params.get(sessionFilterUntaggedParamKey));
    const tags = params
      .getAll(sessionFilterTagParamKey)
      .map((entry) => entry.trim())
      .filter((entry) => entry.length > 0);

    sessionSearchQuery = query;
    sessionSearchScope = scope === "full" ? "full" : "summary";
    showArchivedSessions = isEnabledQueryParam(params.get(sessionArchivedParamKey));
    activeSavedSessionFilterId = params.get(sessionSavedFilterParamKey)?.trim() || null;
    activeSessionFolder = folder || null;
    sessionFilter = normalizeSessionFilterState({
      pinnedOnly: isEnabledQueryParam(params.get(sessionFilterPinnedParamKey)),
      runningOnly: isEnabledQueryParam(params.get(sessionFilterRunningParamKey)),
      queuedOnly: isEnabledQueryParam(params.get(sessionFilterQueuedParamKey)),
      untaggedOnly,
      highlight: highlight === "attention" || highlight === "completed" ? highlight : "all",
      tags: untaggedOnly ? [] : folder ? [...new Set([folder, ...tags])] : tags
    });
  }

  function syncSessionListStateInUrl() {
    if (typeof window === "undefined") {
      return;
    }

    const url = new URL(window.location.href);
    const query = sessionSearchQuery.trim();
    if (query) {
      url.searchParams.set(sessionSearchQueryParamKey, query);
    } else {
      url.searchParams.delete(sessionSearchQueryParamKey);
    }

    if (sessionSearchScope === "full") {
      url.searchParams.set(sessionSearchScopeParamKey, sessionSearchScope);
    } else {
      url.searchParams.delete(sessionSearchScopeParamKey);
    }

    if (showArchivedSessions) {
      url.searchParams.set(sessionArchivedParamKey, "1");
    } else {
      url.searchParams.delete(sessionArchivedParamKey);
    }

    const normalizedFilter = normalizeSessionFilterState(sessionFilter);
    const filterParams: Array<[string, boolean]> = [
      [sessionFilterPinnedParamKey, normalizedFilter.pinnedOnly],
      [sessionFilterRunningParamKey, normalizedFilter.runningOnly],
      [sessionFilterQueuedParamKey, normalizedFilter.queuedOnly],
      [sessionFilterUntaggedParamKey, normalizedFilter.untaggedOnly && !activeSessionFolder]
    ];
    for (const [key, enabled] of filterParams) {
      if (enabled) {
        url.searchParams.set(key, "1");
      } else {
        url.searchParams.delete(key);
      }
    }

    if (normalizedFilter.highlight !== "all") {
      url.searchParams.set(sessionFilterHighlightParamKey, normalizedFilter.highlight);
    } else {
      url.searchParams.delete(sessionFilterHighlightParamKey);
    }

    url.searchParams.delete(sessionFilterTagParamKey);
    for (const tag of normalizedFilter.tags) {
      if (tag !== activeSessionFolder) {
        url.searchParams.append(sessionFilterTagParamKey, tag);
      }
    }

    if (activeSessionFolder) {
      url.searchParams.set(sessionFolderParamKey, activeSessionFolder);
    } else {
      url.searchParams.delete(sessionFolderParamKey);
    }

    if (activeSavedSessionFilterId) {
      url.searchParams.set(sessionSavedFilterParamKey, activeSavedSessionFilterId);
    } else {
      url.searchParams.delete(sessionSavedFilterParamKey);
    }

    const nextUrl = `${url.pathname}${url.search}${url.hash}`;
    const currentUrl = `${window.location.pathname}${window.location.search}${window.location.hash}`;
    if (nextUrl !== currentUrl) {
      window.history.replaceState(window.history.state, "", nextUrl);
    }
  }

  function updateConfigSessionOrganization(patch: Partial<AppConfigPayload["sessionOrganization"]>) {
    if (!config) {
      return;
    }
    config = {
      ...config,
      sessionOrganization: {
        ...config.sessionOrganization,
        ...patch
      }
    };
  }

  function matchesSessionSummaryFilter(session: SessionSummary, filter: SessionSummaryFilter) {
    if (filter.pinnedOnly && !session.pinned) {
      return false;
    }
    if (filter.runningOnly && getSessionSortPriority(session.status) === 0) {
      return false;
    }
    if (filter.queuedOnly && session.queueCount <= 0) {
      return false;
    }
    if (filter.untaggedOnly && session.tags.some((tag) => tag.trim().length > 0)) {
      return false;
    }
    if (filter.highlight !== "all" && session.highlight?.kind !== filter.highlight) {
      return false;
    }
    if (filter.tags.length > 0) {
      const sessionTags = new Set(session.tags.map((entry) => entry.trim()));
      if (!filter.tags.every((tag) => sessionTags.has(tag))) {
        return false;
      }
    }
    return true;
  }

  function compareSessions(left: SessionSummary, right: SessionSummary) {
    const pinnedDifference = Number(Boolean(right.pinned)) - Number(Boolean(left.pinned));
    if (pinnedDifference !== 0) {
      return pinnedDifference;
    }

    const priorityDifference = getSessionSortPriority(right.status) - getSessionSortPriority(left.status);
    if (priorityDifference !== 0) {
      return priorityDifference;
    }

    const updatedDifference =
      normalizeSessionTimestamp(right.updatedAt || 0) - normalizeSessionTimestamp(left.updatedAt || 0);
    if (updatedDifference !== 0) {
      return updatedDifference;
    }

    const createdDifference =
      normalizeSessionTimestamp(right.createdAt || 0) - normalizeSessionTimestamp(left.createdAt || 0);
    if (createdDifference !== 0) {
      return createdDifference;
    }

    return 0;
  }

  function sortSessions(items: SessionSummary[]) {
    return [...items].sort(compareSessions);
  }

  function notifyBrowser(title: string, body: string) {
    if (typeof window === "undefined" || typeof Notification === "undefined") {
      return;
    }
    if (Notification.permission !== "granted") {
      return;
    }
    if (!document.hidden) {
      return;
    }
    new Notification(title, { body });
  }

  function notifyAttentionEvent(
    sessionId: string,
    reason: string,
    requestId: string | null,
    profileId: string | null = null
  ) {
    if (shouldSuppressAttentionReason(sessionId, reason, profileId)) {
      return;
    }
    const now = Date.now();
    for (const [key, notifiedAt] of Object.entries(recentAttentionNotificationKeys)) {
      if (now - notifiedAt > 60_000) {
        delete recentAttentionNotificationKeys[key];
      }
    }
    const notificationKey = `${profileId ?? ""}:${sessionId}:${reason}:${requestId ?? ""}`;
    if (now - (recentAttentionNotificationKeys[notificationKey] ?? 0) < 15_000) {
      return;
    }
    recentAttentionNotificationKeys[notificationKey] = now;

    if (reason === "completed") {
      void notifyBrowser(m.task_completed_notification_title(), m.task_completed_notification_body());
      return;
    }
    if (reason === "approval" || reason === "needsInput") {
      void notifyBrowser(m.input_required_notification_title(), m.input_required_notification_body());
      return;
    }
    if (reason === "stopped") {
      void notifyBrowser(m.session_stopped(), "");
      return;
    }
    if (reason === "failed") {
      void notifyBrowser(m.session_failed(), "");
    }
  }

  function normalizeDiffPath(rawPath: string | null) {
    if (!rawPath) {
      return null;
    }
    const trimmed = rawPath.trim();
    if (!trimmed || trimmed === "/dev/null") {
      return trimmed || null;
    }
    const withoutPrefix = trimmed.replace(/^([ab])\//u, "");
    return withoutPrefix.replace(/^"+|"+$/gu, "");
  }

  function buildDiffView(path: string, kind: "add" | "delete" | "update", movePath: string | null, diff: string): FileChangeView {
    const normalizedDiff = diff.replace(/\r\n/g, "\n");
    let original = "";
    let modified = "";
    let renderable = false;

    if (kind === "add") {
      const addedLines: string[] = [];
      let sawHunk = false;
      for (const line of normalizedDiff.split("\n")) {
        if (line.startsWith("@@")) {
          sawHunk = true;
          continue;
        }
        if (!sawHunk || line === "\\ No newline at end of file") {
          continue;
        }
        if (line.startsWith("+")) {
          addedLines.push(line.slice(1));
        } else if (line.startsWith(" ")) {
          addedLines.push(line.slice(1));
        }
      }
      modified = sawHunk ? addedLines.join("\n") : normalizedDiff;
      renderable = modified.length > 0;
    } else if (kind === "delete") {
      const removedLines: string[] = [];
      let sawHunk = false;
      for (const line of normalizedDiff.split("\n")) {
        if (line.startsWith("@@")) {
          sawHunk = true;
          continue;
        }
        if (!sawHunk || line === "\\ No newline at end of file") {
          continue;
        }
        if (line.startsWith("-")) {
          removedLines.push(line.slice(1));
        } else if (line.startsWith(" ")) {
          removedLines.push(line.slice(1));
        }
      }
      original = sawHunk ? removedLines.join("\n") : normalizedDiff;
      renderable = original.length > 0;
    } else {
      const originalLines: string[] = [];
      const modifiedLines: string[] = [];
      let sawHunk = false;

      for (const line of normalizedDiff.split("\n")) {
        if (line.startsWith("@@")) {
          sawHunk = true;
          continue;
        }
        if (!sawHunk || line === "\\ No newline at end of file") {
          continue;
        }

        const prefix = line[0] ?? "";
        const content = prefix === "+" || prefix === "-" || prefix === " " ? line.slice(1) : line;
        if (prefix === "-") {
          originalLines.push(content);
        } else if (prefix === "+") {
          modifiedLines.push(content);
        } else if (prefix === " ") {
          originalLines.push(content);
          modifiedLines.push(content);
        }
      }

      if (sawHunk) {
        original = originalLines.join("\n");
        modified = modifiedLines.join("\n");
        renderable = true;
      }
    }

    return {
      path,
      kind,
      movePath,
      diff: normalizedDiff,
      original,
      modified,
      renderable
    };
  }

  function parseAggregatedDiffViews(diff: string) {
    const normalizedDiff = diff.replace(/\r\n/g, "\n");
    const lines = normalizedDiff.split("\n");
    const views: FileChangeView[] = [];
    let currentLines: string[] = [];
    let currentOldPath: string | null = null;
    let currentNewPath: string | null = null;

    const flushCurrent = () => {
      if (currentLines.length === 0) {
        return;
      }
      const nextDiff = currentLines.join("\n").trimEnd();
      const normalizedOldPath = normalizeDiffPath(currentOldPath);
      const normalizedNewPath = normalizeDiffPath(currentNewPath);
      const kind: "add" | "delete" | "update" =
        normalizedOldPath === "/dev/null" ? "add" : normalizedNewPath === "/dev/null" ? "delete" : "update";
      const path =
        kind === "delete"
          ? normalizedOldPath && normalizedOldPath !== "/dev/null"
            ? normalizedOldPath
            : normalizedNewPath && normalizedNewPath !== "/dev/null"
              ? normalizedNewPath
              : "aggregated.diff"
          : normalizedNewPath && normalizedNewPath !== "/dev/null"
            ? normalizedNewPath
            : normalizedOldPath && normalizedOldPath !== "/dev/null"
              ? normalizedOldPath
              : "aggregated.diff";
      const movePath =
        kind === "update" &&
        normalizedOldPath &&
        normalizedNewPath &&
        normalizedOldPath !== normalizedNewPath &&
        normalizedOldPath !== "/dev/null" &&
        normalizedNewPath !== "/dev/null"
          ? normalizedNewPath
          : null;
      views.push(buildDiffView(path, kind, movePath, nextDiff));
      currentLines = [];
      currentOldPath = null;
      currentNewPath = null;
    };

    for (const line of lines) {
      const diffGitMatch = line.match(/^diff --git a\/(.+?) b\/(.+)$/u);
      if (diffGitMatch) {
        flushCurrent();
        currentOldPath = diffGitMatch[1];
        currentNewPath = diffGitMatch[2];
        currentLines = [line];
        continue;
      }

      if (line.startsWith("--- ")) {
        if (currentLines.length === 0) {
          currentLines = [];
        }
        currentOldPath = line.slice(4).trim();
        currentLines.push(line);
        continue;
      }

      if (line.startsWith("+++ ")) {
        if (currentLines.length === 0) {
          currentLines = [];
        }
        currentNewPath = line.slice(4).trim();
        currentLines.push(line);
        continue;
      }

      if (currentLines.length > 0) {
        currentLines.push(line);
      }
    }

    flushCurrent();
    return views;
  }

  function diffLineStats(diff: string) {
    let added = 0;
    let removed = 0;
    for (const line of diff.split("\n")) {
      if (line.startsWith("+++ ") || line.startsWith("--- ")) {
        continue;
      }
      if (line.startsWith("+")) {
        added += 1;
        continue;
      }
      if (line.startsWith("-")) {
        removed += 1;
      }
    }
    return { added, removed };
  }

  function getTranscriptTurnsOffsetTop() {
    if (!transcriptElement || !transcriptTurnsElement) {
      return 0;
    }
    const transcriptRect = transcriptElement.getBoundingClientRect();
    const turnsRect = transcriptTurnsElement.getBoundingClientRect();
    return Math.max(0, turnsRect.top - transcriptRect.top + transcriptElement.scrollTop);
  }

  function transcriptWindowsMatch(left: typeof transcriptTurnWindow, right: typeof transcriptTurnWindow) {
    return (
      left.start === right.start &&
      left.end === right.end &&
      Math.abs(left.topSpacer - right.topSpacer) < 1 &&
      Math.abs(left.bottomSpacer - right.bottomSpacer) < 1 &&
      Math.abs(left.totalHeight - right.totalHeight) < 1
    );
  }

  function rebuildTranscriptTurnLayout() {
    const turnIds = conversation?.thread.turns.map((turn) => turn.id) ?? [];
    transcriptTurnLayout = buildTranscriptLayout({
      turnIds,
      measuredHeights: transcriptTurnHeights,
      estimatedHeight: transcriptTurnEstimatedHeight,
      gap: transcriptTurnGap
    });
  }

  function ensureTranscriptTurnLayout() {
    const turns = conversation?.thread.turns ?? [];
    if (
      transcriptTurnLayout.turnIds.length !== turns.length ||
      transcriptTurnLayout.turnIds[0] !== turns[0]?.id ||
      transcriptTurnLayout.turnIds.at(-1) !== turns.at(-1)?.id
    ) {
      rebuildTranscriptTurnLayout();
    }
  }

  function clearTranscriptPinReleaseWait() {
    if (transcriptPinReleaseTimer) {
      clearTimeout(transcriptPinReleaseTimer);
      transcriptPinReleaseTimer = null;
    }
    transcriptPinScrollEndCleanup?.();
    transcriptPinScrollEndCleanup = null;
  }

  function pinTranscriptTurn(turnId: string, alignment: TranscriptWindowAlignment = "center") {
    clearTranscriptPinReleaseWait();
    transcriptPinnedTurnId = turnId;
    transcriptPinnedTurnAlignment = alignment;
  }

  function releaseTranscriptTurnPin(turnId: string | null = transcriptPinnedTurnId, delay = 0) {
    if (!turnId || transcriptPinnedTurnId !== turnId) {
      return;
    }
    clearTranscriptPinReleaseWait();
    transcriptPinReleaseTimer = setTimeout(() => {
      transcriptPinReleaseTimer = null;
      if (transcriptPinnedTurnId !== turnId) {
        return;
      }
      transcriptPinnedTurnId = null;
      refreshTranscriptTurnWindow();
    }, delay);
  }

  function releaseTranscriptTurnPinWhenScrollEnds(turnId: string) {
    if (!transcriptElement || transcriptPinnedTurnId !== turnId) {
      releaseTranscriptTurnPin(turnId);
      return;
    }
    clearTranscriptPinReleaseWait();
    const transcript = transcriptElement;
    const finish = () => {
      if (transcriptPinnedTurnId !== turnId) {
        clearTranscriptPinReleaseWait();
        return;
      }
      transcriptPinnedTurnId = null;
      clearTranscriptPinReleaseWait();
      refreshTranscriptTurnWindow();
    };
    transcript.addEventListener("scrollend", finish, { once: true });
    transcriptPinScrollEndCleanup = () => {
      transcript.removeEventListener("scrollend", finish);
    };
    transcriptPinReleaseTimer = setTimeout(finish, 4_000);
  }

  function captureTranscriptScrollAnchor(): TranscriptScrollAnchor | null {
    if (!transcriptElement || !transcriptTurnsElement) {
      return null;
    }
    const transcriptTop = transcriptElement.getBoundingClientRect().top;
    const turnElements = transcriptTurnsElement.querySelectorAll<HTMLElement>("[data-turn-id]");
    for (const element of turnElements) {
      const rect = element.getBoundingClientRect();
      if (rect.bottom > transcriptTop + 1) {
        const turnId = element.dataset.turnId;
        return turnId ? { turnId, viewportOffset: rect.top - transcriptTop } : null;
      }
    }
    return null;
  }

  async function restoreTranscriptScrollAnchor(anchor: TranscriptScrollAnchor | null) {
    if (!anchor || !transcriptElement || !conversation?.thread.turns.some((turn) => turn.id === anchor.turnId)) {
      return false;
    }
    pinTranscriptTurn(anchor.turnId, "start");
    refreshTranscriptTurnWindow(anchor.turnId, "start");
    await tick();
    if (!transcriptElement) {
      releaseTranscriptTurnPin(anchor.turnId);
      return false;
    }
    const escapedTurnId =
      typeof CSS !== "undefined" && typeof CSS.escape === "function"
        ? CSS.escape(anchor.turnId)
        : anchor.turnId.replace(/"/g, '\\"');
    const target = transcriptTurnsElement?.querySelector<HTMLElement>(`[data-turn-id="${escapedTurnId}"]`);
    if (!target) {
      releaseTranscriptTurnPin(anchor.turnId);
      return false;
    }
    const transcriptTop = transcriptElement.getBoundingClientRect().top;
    const nextOffset = target.getBoundingClientRect().top - transcriptTop;
    noteTranscriptProgrammaticScroll();
    transcriptElement.scrollTo({
      top: transcriptElement.scrollTop + nextOffset - anchor.viewportOffset,
      behavior: "auto"
    });
    releaseTranscriptTurnPin(anchor.turnId);
    return true;
  }

  function refreshTranscriptTurnWindow(
    anchorTurnId: string | null = null,
    anchorAlignment: TranscriptWindowAlignment = "center"
  ) {
    if (!conversation) {
      if (transcriptTurnWindow.end !== 0) {
        transcriptTurnWindow = EMPTY_TRANSCRIPT_WINDOW;
      }
      return;
    }

    ensureTranscriptTurnLayout();
    const effectiveAnchorTurnId = anchorTurnId ?? transcriptPinnedTurnId;
    const effectiveAnchorAlignment = anchorTurnId ? anchorAlignment : transcriptPinnedTurnAlignment;
    const anchorIndex = effectiveAnchorTurnId
      ? transcriptTurnLayout.turnIds.indexOf(effectiveAnchorTurnId)
      : undefined;
    const nextWindow = computeTranscriptWindowFromLayout({
      layout: transcriptTurnLayout,
      scrollOffset: Math.max(0, (transcriptElement?.scrollTop ?? 0) - getTranscriptTurnsOffsetTop()),
      viewportHeight: transcriptElement?.clientHeight ?? 800,
      overscan: transcriptTurnOverscan,
      maxItems: transcriptTurnMountLimit,
      anchorIndex: anchorIndex !== undefined && anchorIndex >= 0 ? anchorIndex : undefined,
      anchorAlignment: effectiveAnchorAlignment
    });

    if (!transcriptWindowsMatch(transcriptTurnWindow, nextWindow)) {
      transcriptTurnWindow = nextWindow;
    }
  }

  function flushTranscriptTurnMeasurements() {
    transcriptMeasurementFrame = null;
    let changed = false;
    for (const [turnId, nextHeight] of pendingTranscriptTurnHeights) {
      const previousHeight = transcriptTurnHeights.get(turnId) ?? 0;
      if (Math.abs(nextHeight - previousHeight) < 1) {
        continue;
      }
      transcriptTurnHeights.set(turnId, nextHeight);
      changed = true;
    }
    pendingTranscriptTurnHeights.clear();
    if (!changed) {
      return;
    }
    rebuildTranscriptTurnLayout();
    refreshTranscriptTurnWindow();
    if (stickTranscriptToBottom || forceTranscriptScroll || pendingTranscriptBottomScroll) {
      scheduleTranscriptScrollToBottom();
    }
  }

  function queueTranscriptTurnMeasurement(turnId: string, nextHeight: number) {
    if (!Number.isFinite(nextHeight) || nextHeight <= 0) {
      return;
    }
    pendingTranscriptTurnHeights.set(turnId, nextHeight);
    if (transcriptMeasurementFrame !== null) {
      return;
    }
    if (typeof window === "undefined") {
      flushTranscriptTurnMeasurements();
      return;
    }
    transcriptMeasurementFrame = window.requestAnimationFrame(flushTranscriptTurnMeasurements);
  }

  function measureTranscriptTurn(node: HTMLElement, initialTurnId: string) {
    let turnId = initialTurnId;
    const recordHeight = (entry?: ResizeObserverEntry) => {
      const borderBoxSize = entry?.borderBoxSize;
      const nextHeight = Array.isArray(borderBoxSize)
        ? borderBoxSize[0]?.blockSize
        : borderBoxSize?.blockSize;
      queueTranscriptTurnMeasurement(turnId, nextHeight ?? node.getBoundingClientRect().height);
    };
    const observer =
      typeof ResizeObserver === "undefined"
        ? null
        : new ResizeObserver((entries) => recordHeight(entries[0]));
    observer?.observe(node);
    recordHeight();

    return {
      update(nextTurnId: string) {
        if (nextTurnId === turnId) {
          return;
        }
        turnId = nextTurnId;
        recordHeight();
      },
      destroy() {
        observer?.disconnect();
      }
    };
  }

  function isTranscriptAtBottom(threshold = scrollBottomThreshold) {
    if (!transcriptElement) {
      return true;
    }
    return transcriptElement.scrollHeight - transcriptElement.scrollTop - transcriptElement.clientHeight <= threshold;
  }

  function isInitialTranscriptScrollPending(sessionId: string | null = selectedSessionId) {
    return Boolean(sessionId && pendingInitialTranscriptScrollSessionId === sessionId);
  }

  function clearInitialTranscriptScrollPending(sessionId: string | null = selectedSessionId) {
    if (!sessionId || pendingInitialTranscriptScrollSessionId !== sessionId) {
      return;
    }
    pendingInitialTranscriptScrollSessionId = null;
    if (!olderTurnsAutoLoadPaused) {
      olderTurnsAutoLoadEnabled = true;
    }
  }

  function suspendTranscriptAutoScrollForUser(preservePinnedTurn = false) {
    if (!preservePinnedTurn) {
      transcriptPinnedTurnId = null;
      clearTranscriptPinReleaseWait();
    }
    transcriptAutoScrollSuspendedByUser = true;
    stickTranscriptToBottom = false;
    forceTranscriptScroll = false;
    pendingTranscriptBottomScroll = false;
    clearInitialTranscriptScrollPending();
    if (transcriptScrollFrame !== null && typeof window !== "undefined") {
      cancelAnimationFrame(transcriptScrollFrame);
    }
    transcriptScrollFrame = null;
    transcriptScrollGeneration += 1;
  }

  function requestTranscriptBottomScroll(force = false) {
    if (force) {
      transcriptAutoScrollSuspendedByUser = false;
      pendingTranscriptBottomScroll = true;
      noteTranscriptProgrammaticScroll(700);
    } else if (transcriptAutoScrollSuspendedByUser && !isTranscriptAtBottom()) {
      stickTranscriptToBottom = false;
      forceTranscriptScroll = false;
      pendingTranscriptBottomScroll = false;
      return;
    }

    stickTranscriptToBottom = true;
    forceTranscriptScroll = true;
  }

  function preserveTranscriptScrollAfterDataUpdate(force = false) {
    if (force || isTranscriptAtBottom()) {
      requestTranscriptBottomScroll(true);
      return;
    }

    if (transcriptAutoScrollSuspendedByUser) {
      stickTranscriptToBottom = false;
      forceTranscriptScroll = false;
      return;
    }

    if (stickTranscriptToBottom || forceTranscriptScroll) {
      requestTranscriptBottomScroll();
    }
  }

  function scheduleTranscriptScrollToBottom() {
    if (!transcriptElement) {
      return;
    }
    if (
      transcriptAutoScrollSuspendedByUser &&
      !forceTranscriptScroll &&
      !pendingTranscriptBottomScroll &&
      !isInitialTranscriptScrollPending()
    ) {
      return;
    }

    const scrollTranscript = (top: number) => {
      if (!transcriptElement) {
        return;
      }

      const previousBehavior = transcriptElement.style.scrollBehavior;
      transcriptElement.style.scrollBehavior = "auto";
      noteTranscriptProgrammaticScroll();
      transcriptElement.scrollTo({ top, behavior: "auto" });

      if (previousBehavior) {
        transcriptElement.style.scrollBehavior = previousBehavior;
      } else {
        transcriptElement.style.removeProperty("scroll-behavior");
      }
    };

    if (typeof window === "undefined") {
      scrollTranscript(transcriptElement.scrollHeight);
      forceTranscriptScroll = false;
      return;
    }

    if (transcriptScrollFrame !== null) {
      cancelAnimationFrame(transcriptScrollFrame);
    }

    const generation = ++transcriptScrollGeneration;

    void tick().then(() => {
      let lastHeight = -1;
      let stableFrames = 0;
      const startedAt = window.performance.now();

      const step = () => {
        if (generation !== transcriptScrollGeneration) {
          return;
        }
        if (!transcriptElement) {
          transcriptScrollFrame = null;
          return;
        }
        const initialScrollPending = isInitialTranscriptScrollPending();
        if (loadingOlderTurns || (loadingDetail && !initialScrollPending)) {
          transcriptScrollFrame = null;
          return;
        }
        if (
          (transcriptAutoScrollSuspendedByUser && !forceTranscriptScroll && !pendingTranscriptBottomScroll && !initialScrollPending) ||
          (!stickTranscriptToBottom && !forceTranscriptScroll && !pendingTranscriptBottomScroll && !initialScrollPending)
        ) {
          transcriptScrollFrame = null;
          forceTranscriptScroll = false;
          pendingTranscriptBottomScroll = false;
          return;
        }

        const nextHeight = transcriptElement.scrollHeight;
        scrollTranscript(nextHeight);

        if (nextHeight === lastHeight) {
          stableFrames += 1;
        } else {
          lastHeight = nextHeight;
          stableFrames = 0;
        }

        if (stableFrames >= 2 || window.performance.now() - startedAt >= 900) {
          const keepInitialScrollPendingForDetailLoad = loadingDetail && initialScrollPending;
          transcriptScrollFrame = null;
          if (!keepInitialScrollPendingForDetailLoad) {
            pendingTranscriptBottomScroll = false;
            forceTranscriptScroll = false;
            if (initialScrollPending) {
              clearInitialTranscriptScrollPending();
            }
          }
          return;
        }

        transcriptScrollFrame = window.requestAnimationFrame(step);
      };

      transcriptScrollFrame = window.requestAnimationFrame(step);
    });
  }

  function getTranscriptNow() {
    if (typeof window !== "undefined" && typeof window.performance !== "undefined") {
      return window.performance.now();
    }
    return Date.now();
  }

  function normalizeSessionTimestamp(value: number | null | undefined) {
    if (typeof value !== "number" || !Number.isFinite(value) || value <= 0) {
      return 0;
    }
    return value >= 1_000_000_000_000 ? value : value * 1000;
  }

  function getDateTimeLocale() {
    const locale = $activeLocale;
    if (locale === "ko") {
      return "ko-KR";
    }
    if (locale === "zh-Hans") {
      return "zh-CN";
    }
    if (locale === "zh-Hant") {
      return "zh-TW";
    }
    return locale || "en-US";
  }

  function isSameLocalDate(left: Date, right: Date) {
    return (
      left.getFullYear() === right.getFullYear() &&
      left.getMonth() === right.getMonth() &&
      left.getDate() === right.getDate()
    );
  }

  function formatTurnTimestamp(value: number | null | undefined, long = false) {
    const timestamp = normalizeSessionTimestamp(value);
    if (!timestamp) {
      return "";
    }

    const locale = getDateTimeLocale();
    const date = new Date(timestamp);
    if (long) {
      return new Intl.DateTimeFormat(locale, {
        dateStyle: "medium",
        timeStyle: "medium"
      }).format(date);
    }

    const now = new Date();
    return new Intl.DateTimeFormat(locale, {
      month: isSameLocalDate(date, now) ? undefined : "short",
      day: isSameLocalDate(date, now) ? undefined : "numeric",
      hour: "2-digit",
      minute: "2-digit"
    }).format(date);
  }

  function turnTimestampIso(value: number | null | undefined) {
    const timestamp = normalizeSessionTimestamp(value);
    return timestamp ? new Date(timestamp).toISOString() : undefined;
  }

  function noteTranscriptUserScrollIntent(durationMs = 800) {
    transcriptUserScrollIntentUntil = getTranscriptNow() + durationMs;
  }

  function hasTranscriptUserScrollIntent() {
    return getTranscriptNow() <= transcriptUserScrollIntentUntil;
  }

  function noteTranscriptProgrammaticScroll(durationMs = 180) {
    transcriptProgrammaticScrollUntil = getTranscriptNow() + durationMs;
  }

  function isCacheValidationResponse(
    payload: SessionListResponse | SessionDetailResponse
  ): payload is { cacheVersion: string; notModified: true } {
    return payload.notModified === true;
  }

  function isSessionListPatchResponse(payload: SessionListResponse): payload is SessionListPatchPayload {
    return "patch" in payload;
  }

  function isSessionDetailPatchResponse(payload: SessionDetailResponse): payload is SessionDetailPatchPayload {
    return "patch" in payload;
  }

  function fnv1a32Hex(source: string) {
    let hash = 0x811c9dc5;
    for (let index = 0; index < source.length; index += 1) {
      hash ^= source.charCodeAt(index);
      hash = Math.imul(hash, 0x01000193) >>> 0;
    }
    return hash.toString(16).padStart(8, "0");
  }

  function buildSessionListStateHash(
    sessionIds: string[],
    nextCursor: string | null,
    summaryVersions: Record<string, string>
  ) {
    let source = `cursor=${nextCursor ?? ""}\n`;
    for (const sessionId of sessionIds) {
      source += `${sessionId}\t${summaryVersions[sessionId] ?? ""}\n`;
    }
    return fnv1a32Hex(source);
  }

  function buildSessionDetailStateHash(
    metadataVersion: string,
    turnIds: string[],
    turnVersions: Record<string, string>
  ) {
    let source = `metadata=${metadataVersion}\n`;
    for (const turnId of turnIds) {
      source += `${turnId}\t${turnVersions[turnId] ?? ""}\n`;
    }
    return fnv1a32Hex(source);
  }

  function boundedVersionHints(source: Record<string, string>, limit: number) {
    const entries = Object.entries(source);
    if (entries.length === 0 || entries.length > limit) {
      return null;
    }
    return Object.fromEntries(entries);
  }

  function clonePayloadForCache<T>(payload: T): T {
    return JSON.parse(JSON.stringify(payload)) as T;
  }

  function sanitizeSessionDetailItemForBrowserCache(item: CodexItem) {
    const largeImageResult =
      item.type === "imageGeneration" &&
      typeof item.result === "string" &&
      item.result.length > SESSION_DETAIL_CACHE_INLINE_IMAGE_RESULT_MAX_CHARS;
    if (!largeImageResult && !["commandExecution", "fileChange", "mcpToolCall", "dynamicToolCall", "webSearch"].includes(item.type)) {
      return clonePayloadForCache(item);
    }

    const sanitized = clonePayloadForCache(item) as CodexItem & Record<string, unknown>;
    delete sanitized.aggregatedOutput;
    delete sanitized.output;
    delete sanitized.stdout;
    delete sanitized.stderr;
    delete sanitized.logs;
    delete sanitized.result;
    delete sanitized.response;
    delete sanitized.raw;
    delete sanitized.diff;
    delete sanitized.original;
    delete sanitized.modified;
    delete sanitized.content;
    delete sanitized.patch;

    if (sanitized.type === "fileChange" && Array.isArray(sanitized.changes)) {
      sanitized.changes = sanitized.changes.map((change) => {
        if (!change || typeof change !== "object") {
          return change;
        }
        const record = change as Record<string, unknown>;
        return {
          path: typeof record.path === "string" ? record.path : "",
          kind: record.kind ?? "update"
        };
      });
    }

    if (sanitized.type === "webSearch") {
      delete sanitized.action;
      delete sanitized.results;
      delete sanitized.sources;
      delete sanitized.citations;
      delete sanitized.searchResults;
      delete sanitized.sourceResults;
      delete sanitized.citationResults;
    }

    if (sanitized.type === "mcpToolCall" || sanitized.type === "dynamicToolCall") {
      delete sanitized.action;
      delete sanitized.invocation;
    }

    if (sanitized.type === "imageGeneration") {
      sanitized.result = null;
      sanitized.resultOmitted = true;
    }

    sanitized.detailState = "deferred";
    return sanitized;
  }

  function sanitizeSessionDetailForBrowserCache(payload: SessionDetailPayload): SessionDetailPayload {
    const cloned = clonePayloadForCache(payload);
    return {
      ...cloned,
      thread: {
        ...cloned.thread,
        turns: cloned.thread.turns.map((turn) => ({
          ...turn,
          items: turn.items.map((item) => sanitizeSessionDetailItemForBrowserCache(item))
        }))
      }
    };
  }

  function buildSessionListBrowserCacheKey(cursor: string | null = null, limit = sessionPageSize) {
    if (!activeProfileId) {
      return null;
    }

    return JSON.stringify({
      schema: SESSION_LIST_BROWSER_CACHE_SCHEMA_VERSION,
      profileId: activeProfileId,
      archived: showArchivedSessions,
      cursor,
      limit,
      query: sessionSearchQuery.trim(),
      scope: sessionSearchScope,
      filter: {
        pinnedOnly: sessionFilter.pinnedOnly,
        runningOnly: sessionFilter.runningOnly,
        queuedOnly: sessionFilter.queuedOnly,
        untaggedOnly: sessionFilter.untaggedOnly,
        highlight: sessionFilter.highlight,
        tags: [...sessionFilter.tags].sort()
      }
    });
  }

  function buildSessionListWindowKey() {
    return buildSessionListBrowserCacheKey(null, 0);
  }

  function ensureSessionListWindowKey(windowKey: string | null) {
    if (sessionListWindowKey === windowKey) {
      return;
    }
    sessionListWindowKey = windowKey;
    sessionListRequestedLimit = sessionPageSize;
  }

  function buildSessionDetailBrowserCacheKey(sessionId: string, profileId = profileIdForSession(sessionId)) {
    if (!profileId) {
      return null;
    }

    return JSON.stringify({
      schema: SESSION_DETAIL_BROWSER_CACHE_SCHEMA_VERSION,
      profileId,
      sessionId
    });
  }

  function conversationToSessionDetailPayload(state: ConversationState): SessionDetailPayload {
    const { livePlans, liveDiffs, ...detail } = state;
    const payload: SessionDetailPayload = {
      ...clonePayloadForCache(detail),
      cacheVersion: sessionDetailCacheVersion ?? detail.cacheVersion ?? "",
      notModified: false
    };
    if (sessionDetailStateHash) {
      payload.stateHash = sessionDetailStateHash;
    }
    if (sessionDetailMetadataVersion) {
      payload.metadataVersion = sessionDetailMetadataVersion;
    }
    if (Object.keys(sessionTurnVersionsById).length > 0) {
      payload.turnVersions = { ...sessionTurnVersionsById };
      payload.turnIds = state.thread.turns.map((turn) => turn.id);
    }
    return sanitizeSessionDetailForBrowserCache(payload);
  }

  function currentSessionListPayload(): SessionListPayload {
    const payload: SessionListPayload = {
      sessions: clonePayloadForCache(sessions),
      nextCursor: sessionsCursor,
      sessionIds: sessions.map((session) => session.id),
      cacheVersion: sessionListCacheVersion ?? "",
      notModified: false
    };
    if (sessionListStateHash) {
      payload.stateHash = sessionListStateHash;
    }
    if (Object.keys(sessionSummaryVersionsById).length > 0) {
      payload.summaryVersions = { ...sessionSummaryVersionsById };
    }
    return payload;
  }

  function scheduleSessionListCachePersist(version: string | null = sessionListCacheVersion) {
    const cacheKey = sessionListCacheKey;
    if (!cacheKey || typeof window === "undefined") {
      return;
    }

    if (sessionListCachePersistTimer) {
      clearTimeout(sessionListCachePersistTimer);
    }

    sessionListCachePersistTimer = setTimeout(() => {
      sessionListCachePersistTimer = null;
      void writeSessionListCache(cacheKey, currentSessionListPayload(), version);
    }, 120);
  }

  async function persistSessionDetailCacheSnapshot(
    sessionId: string,
    cacheKey: string,
    version: string | null
  ) {
    if (sessionDetailCachePersistInFlight) {
      queuedSessionDetailCachePersist = { sessionId, cacheKey, version };
      return;
    }
    if (!conversation || conversation.thread.id !== sessionId) {
      return;
    }

    sessionDetailCachePersistInFlight = true;
    const payload = conversationToSessionDetailPayload(conversation);
    try {
      await writeSessionDetailCache(cacheKey, payload, version);
    } finally {
      sessionDetailCachePersistInFlight = false;
      const queuedPersist = queuedSessionDetailCachePersist;
      queuedSessionDetailCachePersist = null;
      if (queuedPersist) {
        void persistSessionDetailCacheSnapshot(
          queuedPersist.sessionId,
          queuedPersist.cacheKey,
          queuedPersist.version
        );
      }
    }
  }

  function scheduleSessionDetailCachePersist(
    version: string | null = sessionDetailCacheVersion,
    mode: SessionCachePersistMode = "interactive"
  ) {
    const sessionId = selectedSessionId;
    if (!sessionId || !conversation || conversation.thread.id !== sessionId || typeof window === "undefined") {
      return;
    }

    const cacheKey = buildSessionDetailBrowserCacheKey(sessionId);
    if (!cacheKey) {
      return;
    }

    if (sessionDetailCachePersistTimer) {
      if (mode === "stream" && sessionDetailCachePersistMode !== "stream") {
        return;
      }
      clearTimeout(sessionDetailCachePersistTimer);
    }

    sessionDetailCachePersistMode = mode;
    sessionDetailCachePersistTimer = setTimeout(() => {
      sessionDetailCachePersistTimer = null;
      sessionDetailCachePersistMode = null;
      if (!conversation || conversation.thread.id !== sessionId) {
        return;
      }
      void persistSessionDetailCacheSnapshot(sessionId, cacheKey, version);
    }, sessionCachePersistDelay(mode));
  }

  function markConversationCacheDirty(mode: SessionCachePersistMode = "interactive") {
    // Streamed state is newer than the browser cache, but the last server snapshot
    // remains the correct base for conditional detail reads and patches.
    scheduleSessionDetailCachePersist(null, mode);
  }

  function setSessionsStable(nextSessions: SessionSummary[]) {
    const sorted = sortSessions(nextSessions);
    if (sorted.length === sessions.length && sorted.every((session, index) => session === sessions[index])) {
      return;
    }
    sessions = sorted;
  }

  function reuseUnchangedSessionSummaries(nextSessions: SessionSummary[], nextSummaryVersions: Record<string, string> | null | undefined) {
    const enrichedNextSessions = nextSessions.map(enrichSessionSummaryContext);
    if (!nextSummaryVersions || Object.keys(sessionSummaryVersionsById).length === 0) {
      return enrichedNextSessions;
    }
    const currentById = new Map(sessions.map((session) => [session.id, session]));
    return enrichedNextSessions.map((session) => {
      const existing = currentById.get(session.id);
      const nextVersion = nextSummaryVersions[session.id];
      if (existing && nextVersion && sessionSummaryVersionsById[session.id] === nextVersion) {
        return enrichSessionSummaryContext(existing);
      }
      return session;
    });
  }

  function activeProfileConfig() {
    return config?.profiles.find((profile) => profile.active) ?? config?.profiles.find((profile) => profile.id === activeProfileId) ?? null;
  }

  function rememberSessionProfile(summary: Pick<SessionSummary, "id" | "profileId"> | null | undefined) {
    if (!summary?.id || !summary.profileId) {
      return;
    }
    if (sessionProfileIdsBySessionId[summary.id] !== summary.profileId) {
      sessionProfileIdsBySessionId = {
        ...sessionProfileIdsBySessionId,
        [summary.id]: summary.profileId
      };
    }
  }

  function sessionSummaryKey(summary: Pick<SessionSummary, "id" | "profileId">) {
    return `${summary.profileId ?? sessionProfileIdsBySessionId[summary.id] ?? ""}:${summary.id}`;
  }

  function hasDuplicateSessionIds(summaries: Pick<SessionSummary, "id">[]) {
    return summaries.some(
      (summary, index, collection) => collection.findIndex((candidate) => candidate.id === summary.id) !== index
    );
  }

  function profileIdForSession(sessionId: string | null | undefined) {
    if (!sessionId) {
      return activeProfileId;
    }
    if (selectedSessionId === sessionId && selectedSessionProfileId) {
      return selectedSessionProfileId;
    }
    const summary =
      sessions.find(
        (session) =>
          session.id === sessionId &&
          (!selectedSessionProfileId || !session.profileId || session.profileId === selectedSessionProfileId)
      ) ??
      sessions.find((session) => session.id === sessionId) ??
      (selectedSessionSummary?.id === sessionId ? selectedSessionSummary : null);
    return summary?.profileId ?? sessionProfileIdsBySessionId[sessionId] ?? activeProfileId;
  }

  function matchesSessionSelection(sessionId: string, profileId: string | null, selectionVersion: number) {
    return (
      selectedSessionId === sessionId &&
      selectedSessionProfileId === profileId &&
      sessionSelectionVersion === selectionVersion
    );
  }

  function enrichSessionSummaryContext(summary: SessionSummary): SessionSummary {
    const cachedProfileId = summary.id ? (sessionProfileIdsBySessionId[summary.id] ?? null) : null;
    const summaryProfileId = summary.profileId ?? cachedProfileId ?? null;
    const profile = summaryProfileId ? config?.profiles.find((entry) => entry.id === summaryProfileId) : null;
    const isActiveProfile = Boolean(summaryProfileId && summaryProfileId === activeProfileId);
    return {
      ...summary,
      profileId: summaryProfileId,
      profileLabel: summary.profileLabel ?? profile?.label ?? null,
      profileCodexHome: summary.profileCodexHome ?? profile?.codexHome ?? null,
      accountEmail: summary.accountEmail ?? (isActiveProfile ? (config?.account.email ?? null) : null),
      accountType: summary.accountType ?? (isActiveProfile ? (config?.account.type ?? null) : null)
    };
  }

  function mergeSessionPage(payload: SessionListPayload, pinnedSession: SessionSummary | null, append = false) {
    const visiblePayloadSessions = reuseUnchangedSessionSummaries(
      payload.sessions.filter((session) => !isSubagentSessionSummary(session)),
      payload.summaryVersions
    );
    for (const session of visiblePayloadSessions) {
      rememberSessionProfile(session);
    }
    const baseSessions = append ? sessions : [];
    const deduped = [...baseSessions, ...visiblePayloadSessions].filter(
      (session, index, collection) =>
        collection.findIndex((candidate) => sessionSummaryKey(candidate) === sessionSummaryKey(session)) === index
    );

    if (
      shouldPinSession(pinnedSession) &&
      pinnedSession &&
      !deduped.some((session) => sessionSummaryKey(session) === sessionSummaryKey(pinnedSession))
    ) {
      deduped.unshift(pinnedSession);
    }

    setSessionsStable(deduped);
    sessionListRequestedLimit = payload.nextCursor
      ? Math.max(sessionPageSize, sessionListRequestedLimit, sessions.length)
      : Math.max(sessionPageSize, sessions.length);
    sessionsCursor = payload.nextCursor;
    sessionsHasMore = Boolean(payload.nextCursor);
    if (!append) {
      sessionListCacheVersion = payload.cacheVersion ?? null;
      sessionListStateHash = payload.stateHash ?? null;
      sessionSummaryVersionsById = payload.summaryVersions ? { ...payload.summaryVersions } : {};
      sessionListCacheKey = buildSessionListBrowserCacheKey(null, sessionListRequestedLimit);
    } else {
      sessionListCacheVersion = null;
      sessionListStateHash = null;
      sessionSummaryVersionsById = {};
      sessionListCacheKey = buildSessionListBrowserCacheKey(null, sessionListRequestedLimit);
    }
  }

  function applySessionListPatch(payload: SessionListPatchPayload, pinnedSession: SessionSummary | null) {
    const patch = payload.patch;
    if (hasDuplicateSessionIds(sessions) || hasDuplicateSessionIds(patch.upserts) || hasDuplicateSessionIds(patch.sessionIds.map((id) => ({ id })))) {
      return false;
    }
    const currentById = new Map(sessions.map((session) => [session.id, session]));
    const upsertsById = new Map(
      patch.upserts.filter((session) => !isSubagentSessionSummary(session)).map((session) => [session.id, session])
    );
    const pageSessions: SessionSummary[] = [];

    for (const sessionId of patch.sessionIds) {
      const summary = upsertsById.get(sessionId) ?? currentById.get(sessionId);
      if (!summary || isSubagentSessionSummary(summary)) {
        return false;
      }
      pageSessions.push(summary);
    }

    const computedHash = buildSessionListStateHash(patch.sessionIds, patch.nextCursor, patch.summaryVersions);
    if (computedHash !== patch.finalStateHash) {
      return false;
    }

    const deduped = reuseUnchangedSessionSummaries(
      pageSessions.filter(
        (session, index, collection) =>
          collection.findIndex((candidate) => sessionSummaryKey(candidate) === sessionSummaryKey(session)) === index
      ),
      patch.summaryVersions
    );
    for (const session of deduped) {
      rememberSessionProfile(session);
    }
    if (
      shouldPinSession(pinnedSession) &&
      pinnedSession &&
      !deduped.some((session) => sessionSummaryKey(session) === sessionSummaryKey(pinnedSession))
    ) {
      deduped.unshift(pinnedSession);
    }

    setSessionsStable(deduped);
    sessionListRequestedLimit = patch.nextCursor
      ? Math.max(sessionPageSize, sessionListRequestedLimit, sessions.length)
      : Math.max(sessionPageSize, sessions.length);
    sessionsCursor = patch.nextCursor;
    sessionsHasMore = Boolean(patch.nextCursor);
    sessionListCacheVersion = payload.cacheVersion;
    sessionListStateHash = patch.finalStateHash;
    sessionSummaryVersionsById = { ...patch.summaryVersions };
    sessionListCacheKey = buildSessionListBrowserCacheKey(null, sessionListRequestedLimit);
    return true;
  }

  function shouldPinSession(session: SessionSummary | null) {
    return Boolean(
      session &&
        !isSubagentSessionSummary(session) &&
        !sessionSearchQuery.trim() &&
        session.archived === showArchivedSessions &&
        matchesSessionSummaryFilter(session, sessionFilter)
    );
  }

  function upsertSessionSummary(summary: SessionSummary, cacheDirty = true) {
    const enrichedSummary = enrichSessionSummaryContext(summary);
    rememberSessionProfile(enrichedSummary);
    if (isSubagentSessionSummary(enrichedSummary)) {
      setSessionsStable(sessions.filter((session) => sessionSummaryKey(session) !== sessionSummaryKey(enrichedSummary)));
      if (cacheDirty) {
        sessionListCacheVersion = null;
        sessionListStateHash = null;
        sessionSummaryVersionsById = {};
        scheduleSessionListCachePersist(null);
      }
      return;
    }
    setSessionsStable([
      enrichedSummary,
      ...sessions.filter((session) => sessionSummaryKey(session) !== sessionSummaryKey(enrichedSummary))
    ]);
    if (cacheDirty) {
      sessionListCacheVersion = null;
      sessionListStateHash = null;
      sessionSummaryVersionsById = {};
      scheduleSessionListCachePersist(null);
    }
  }

  function applySessionSummaryUpdate(summary: SessionSummary) {
    const enrichedSummary = enrichSessionSummaryContext(summary);
    rememberSessionProfile(enrichedSummary);
    if (isSubagentSessionSummary(enrichedSummary)) {
      const nextSessions = sessions.filter((session) => sessionSummaryKey(session) !== sessionSummaryKey(enrichedSummary));
      if (nextSessions.length !== sessions.length) {
        setSessionsStable(nextSessions);
        sessionListCacheVersion = null;
        sessionListStateHash = null;
        sessionSummaryVersionsById = {};
        scheduleSessionListCachePersist(null);
      }
      return;
    }

    if (sessionSearchQuery.trim()) {
      scheduleSessionRefresh(60);
      return;
    }

    if (enrichedSummary.archived !== showArchivedSessions) {
      const nextSessions = sessions.filter((session) => sessionSummaryKey(session) !== sessionSummaryKey(enrichedSummary));
      if (nextSessions.length !== sessions.length) {
        setSessionsStable(nextSessions);
        sessionListCacheVersion = null;
        sessionListStateHash = null;
        sessionSummaryVersionsById = {};
        scheduleSessionListCachePersist(null);
      }
      return;
    }

    if (!matchesSessionSummaryFilter(enrichedSummary, sessionFilter)) {
      const nextSessions = sessions.filter((session) => sessionSummaryKey(session) !== sessionSummaryKey(enrichedSummary));
      if (nextSessions.length !== sessions.length) {
        setSessionsStable(nextSessions);
        sessionListCacheVersion = null;
        sessionListStateHash = null;
        sessionSummaryVersionsById = {};
        scheduleSessionListCachePersist(null);
      }
      return;
    }

    upsertSessionSummary(enrichedSummary);
  }

  function buildSessionSummaryFromConversation(state: ConversationState): SessionSummary {
    const hasLiveTurn = hasConversationLiveTurn(state);
    const hasRecentLiveEvidence = hasRecentLiveSessionEvidence(state.thread.id);
    const preview = deriveConversationSummaryPreview(state.thread.preview, state.thread.turns);
    const existingSummary =
      sessions.find(
        (session) =>
          session.id === state.thread.id &&
          (!selectedSessionProfileId || !session.profileId || session.profileId === selectedSessionProfileId)
      ) ?? null;
    const status =
      hasLiveTurn || hasRecentLiveEvidence
        ? "running"
        : isLiveConversationStatus(state.thread.status) && state.thread.turns.length > 0
          ? "completed"
          : state.thread.status;
    const updatedAt = Math.max(
      normalizeSessionTimestamp(state.thread.updatedAt),
      normalizeSessionTimestamp(existingSummary?.updatedAt ?? 0),
      normalizeSessionTimestamp(state.thread.createdAt),
      hasLiveTurn || hasRecentLiveEvidence ? Date.now() : 0
    );

    const activeProfile = activeProfileConfig();
    const summaryProfileId =
      selectedSessionSummary?.profileId ??
      existingSummary?.profileId ??
      sessionProfileIdsBySessionId[state.thread.id] ??
      (selectedSessionId === state.thread.id ? selectedSessionProfileId : null) ??
      activeProfile?.id ??
      activeProfileId ??
      null;
    const summaryProfile =
      (summaryProfileId ? config?.profiles.find((entry) => entry.id === summaryProfileId) : null) ?? activeProfile;
    return {
      id: state.thread.id,
      profileId: summaryProfileId,
      profileLabel: selectedSessionSummary?.profileLabel ?? existingSummary?.profileLabel ?? summaryProfile?.label ?? null,
      profileCodexHome: selectedSessionSummary?.profileCodexHome ?? existingSummary?.profileCodexHome ?? summaryProfile?.codexHome ?? null,
      accountEmail: selectedSessionSummary?.accountEmail ?? existingSummary?.accountEmail ?? null,
      accountType: selectedSessionSummary?.accountType ?? existingSummary?.accountType ?? null,
      name: getDisplayThreadTitle(state.thread.name, preview),
      preview,
      queueCount: state.queue.items.length,
      highlight: selectedSessionSummary?.highlight ?? null,
      pinned: selectedSessionSummary?.pinned ?? false,
      tags: [...(selectedSessionSummary?.tags ?? [])],
      cwd: state.thread.cwd,
      archived: selectedSessionSummary?.archived ?? showArchivedSessions,
      createdAt: Math.max(
        normalizeSessionTimestamp(state.thread.createdAt),
        normalizeSessionTimestamp(existingSummary?.createdAt ?? 0)
      ),
      updatedAt,
      status,
      isSubagent: state.thread.isSubagent,
      agentNickname: state.thread.agentNickname,
      agentRole: state.thread.agentRole,
      preferences: state.preferences
    };
  }

  function isSubagentSessionSummary(
    session: Pick<SessionSummary, "isSubagent" | "agentNickname" | "agentRole"> | null | undefined
  ) {
    return Boolean(session?.isSubagent || (session?.agentNickname ?? "").trim() || (session?.agentRole ?? "").trim());
  }

  function applyAccountState(payload: { account: Record<string, unknown>; requiresOpenaiAuth: boolean }) {
    if (!config) {
      return;
    }

    const nextType = payload.account.type;
    config = {
      ...config,
      account: {
        type: nextType === "apiKey" || nextType === "chatgpt" ? nextType : null,
        email: formatValue(payload.account.email) || null,
        planType: formatValue(payload.account.planType) || null,
        requiresOpenaiAuth: payload.requiresOpenaiAuth
      }
    };
  }

  function scheduleSessionRefresh(delay = 100) {
    if (sessionRefreshTimer) {
      clearTimeout(sessionRefreshTimer);
    }
    const nextDelay = delay === 0 ? 0 : Math.max(delay, 180);
    sessionRefreshTimer = setTimeout(() => {
      sessionRefreshTimer = null;
      void refreshSessions(shouldPinSession(selectedSessionSummary) ? selectedSessionSummary : null);
    }, nextDelay);
  }

  function scheduleSelectedSessionStateRefresh(sessionId: string, delay = 160, replaceWithRecentWindow = false) {
    if (selectedSessionDetailRefreshTimer) {
      clearTimeout(selectedSessionDetailRefreshTimer);
    }

    const scheduledSelectionVersion = sessionSelectionVersion;
    const scheduledProfileId = profileIdForSession(sessionId);
    const nextDelay = delay === 0 ? 0 : Math.max(delay, 180);
    selectedSessionDetailRefreshTimer = setTimeout(() => {
      selectedSessionDetailRefreshTimer = null;
      if (
        selectedSessionId !== sessionId ||
        sessionSelectionVersion !== scheduledSelectionVersion ||
        profileIdForSession(sessionId) !== scheduledProfileId
      ) {
        return;
      }

      void refreshSelectedSessionState(
        sessionId,
        replaceWithRecentWindow ? olderTurnPageSize : Math.max(conversation?.thread.turns.length ?? 0, olderTurnPageSize),
        false,
        sessionDetailCacheVersion,
        replaceWithRecentWindow,
        scheduledSelectionVersion,
        scheduledProfileId
      ).catch(() => {});
    }, nextDelay);
  }

  function clearSelectedSessionCompletionRefreshes() {
    for (const job of selectedSessionCompletionRefreshJobs.values()) {
      for (const timer of job.timers) {
        clearTimeout(timer);
      }
    }
    selectedSessionCompletionRefreshJobs.clear();
  }

  function applyLatestCompletedTurnPayload(
    sessionId: string,
    profileId: string | null,
    payload: SessionLatestCompletedTurnPayload,
    expectedTurnId: string | null,
    requestActiveTurnId: string | null
  ) {
    if (
      !conversation ||
      conversation.thread.id !== sessionId ||
      !selectedSessionBindingMatches(sessionId, profileId)
    ) {
      return false;
    }

    const requestedTurnId = String(expectedTurnId ?? payload.targetTurnId ?? "").trim() || null;
    const payloadTurnId = String(payload.turnId ?? "").trim() || null;
    if (
      !payload.settled ||
      !payload.sourceStable ||
      !payloadTurnId ||
      (requestedTurnId && requestedTurnId !== payloadTurnId) ||
      (payload.turn && payload.turn.id !== payloadTurnId)
    ) {
      return false;
    }

    let nextConversation = conversation;
    if (payload.turn) {
      const existingTurnIndex = nextConversation.thread.turns.findIndex((turn) => turn.id === payloadTurnId);
      const activeTurnIndex = nextConversation.activeTurnId
        ? nextConversation.thread.turns.findIndex((turn) => turn.id === nextConversation.activeTurnId)
        : -1;
      const loadedStart =
        nextConversation.hydration.totalTurns === null
          ? null
          : Math.max(0, nextConversation.hydration.totalTurns - nextConversation.thread.turns.length);
      const positionedInsertionIndex =
        payload.turnPosition === null || loadedStart === null
          ? null
          : Math.max(0, Math.min(nextConversation.thread.turns.length, payload.turnPosition - loadedStart));
      const activeTurn = activeTurnIndex >= 0 ? nextConversation.thread.turns[activeTurnIndex] : null;
      const activeTurnIsRealtime = String(activeTurn?.id ?? "").startsWith("realtime:");
      const payloadCompletedAt = normalizeSessionTimestamp(payload.turn.completedAt);
      const activeStartedAt = normalizeSessionTimestamp(activeTurn?.startedAt);
      const payloadPredatesActiveTurn =
        payloadCompletedAt > 0 && activeStartedAt > 0 && payloadCompletedAt < activeStartedAt;
      const replaceRealtimeTurn =
        existingTurnIndex === -1 &&
        activeTurnIndex >= 0 &&
        activeTurnIsRealtime &&
        !payloadPredatesActiveTurn &&
        (positionedInsertionIndex === null || positionedInsertionIndex >= activeTurnIndex);
      const mergeTargetIndex = existingTurnIndex !== -1 ? existingTurnIndex : replaceRealtimeTurn ? activeTurnIndex : -1;
      let incomingTurn = payload.turn;
      if (mergeTargetIndex !== -1) {
        const existingTurn = nextConversation.thread.turns[mergeTargetIndex];
        incomingTurn = {
          ...incomingTurn,
          items: incomingTurn.items.map((incomingItem) => {
            if (String(incomingItem.type ?? "") !== "agentMessage") {
              return incomingItem;
            }
            const incomingLineage = String(incomingItem.completionLineage ?? "").trim();
            const incomingText = String(incomingItem.text ?? "").replace(/\s+/gu, " ").trim();
            const matchingExistingItem = existingTurn.items.find((existingItem) => {
              if (String(existingItem.type ?? "") !== "agentMessage") {
                return false;
              }
              const existingLineage = String(existingItem.completionLineage ?? "").trim();
              if (incomingLineage && existingLineage === incomingLineage) {
                return true;
              }
              return (
                Boolean(incomingText) &&
                String(existingItem.text ?? "").replace(/\s+/gu, " ").trim() === incomingText
              );
            }) ??
              (String(incomingItem.phase ?? "") !== "commentary"
                ? [...existingTurn.items]
                    .reverse()
                    .find(
                      (existingItem) =>
                        String(existingItem.type ?? "") === "agentMessage" &&
                        String(existingItem.phase ?? "") !== "commentary"
                    )
                : undefined);
            return matchingExistingItem && matchingExistingItem.id !== incomingItem.id
              ? { ...incomingItem, id: matchingExistingItem.id }
              : incomingItem;
          })
        };
      }
      let turns: CodexTurn[];
      if (existingTurnIndex !== -1) {
        turns = nextConversation.thread.turns.map((turn, index) =>
          index === existingTurnIndex ? mergeConversationTurnState(turn, incomingTurn) : turn
        );
      } else if (replaceRealtimeTurn) {
        turns = nextConversation.thread.turns.map((turn, index) =>
          index === activeTurnIndex ? mergeConversationTurnState(turn, incomingTurn) : turn
        );
      } else {
        const insertionIndex =
          positionedInsertionIndex ??
          (activeTurnIndex >= 0 ? activeTurnIndex : nextConversation.thread.turns.length);
        turns = [
          ...nextConversation.thread.turns.slice(0, insertionIndex),
          incomingTurn,
          ...nextConversation.thread.turns.slice(insertionIndex)
        ];
      }
      const currentActiveTurnId = nextConversation.activeTurnId;
      const currentActiveTurnRepresentsPayload =
        currentActiveTurnId === payloadTurnId || (replaceRealtimeTurn && currentActiveTurnId === activeTurn?.id);
      const requestActiveTurnRepresentsPayload =
        requestActiveTurnId === payloadTurnId || (replaceRealtimeTurn && requestActiveTurnId === activeTurn?.id);
      const newerTurnIsActive = Boolean(currentActiveTurnId && !currentActiveTurnRepresentsPayload);
      const requestObservedNewerTurn = Boolean(requestActiveTurnId && !requestActiveTurnRepresentsPayload);
      const canSettleThread = !newerTurnIsActive && !requestObservedNewerTurn;
      nextConversation = {
        ...nextConversation,
        activeTurnId:
          canSettleThread && currentActiveTurnRepresentsPayload
            ? null
            : currentActiveTurnId,
        thread: {
          ...nextConversation.thread,
          turns,
          status:
            canSettleThread
              ? isLiveConversationStatus(payload.threadStatus)
                ? "completed"
                : payload.threadStatus
              : nextConversation.thread.status,
          updatedAt: Math.max(
            normalizeSessionTimestamp(nextConversation.thread.updatedAt),
            normalizeSessionTimestamp(payload.threadUpdatedAt)
          )
        }
      };
    } else {
      const currentActiveTurnId = nextConversation.activeTurnId;
      const canSettleThread =
        (!currentActiveTurnId || currentActiveTurnId === payloadTurnId) &&
        (!requestActiveTurnId || requestActiveTurnId === payloadTurnId);
      nextConversation = {
        ...nextConversation,
        activeTurnId: canSettleThread && currentActiveTurnId === payloadTurnId ? null : currentActiveTurnId,
        thread: {
          ...nextConversation.thread,
          status: canSettleThread
            ? isLiveConversationStatus(payload.threadStatus)
              ? "completed"
              : payload.threadStatus
            : nextConversation.thread.status,
          updatedAt: Math.max(
            normalizeSessionTimestamp(nextConversation.thread.updatedAt),
            normalizeSessionTimestamp(payload.threadUpdatedAt)
          )
        }
      };
    }

    conversation = applyLocalComposerPreferencesToConversation(
      normalizeConversationExecutionState(nextConversation)
    );
    if (payload.turnId && payload.completionVersion) {
      sessionTurnVersionsById = {
        ...sessionTurnVersionsById,
        [payload.turnId]: payload.completionVersion
      };
    }
    if (payload.turn || payload.notModified) {
      sessionDetailCacheVersion = null;
      sessionDetailStateHash = null;
      sessionDetailMetadataVersion = null;
      markConversationCacheDirty();
      applySessionSummaryUpdate(buildSessionSummaryFromConversation(conversation));
      preserveTranscriptScrollAfterDataUpdate();
    }
    return true;
  }

  async function refreshSelectedSessionCompletionTail(
    sessionId: string,
    profileId: string | null,
    selectionVersion: number,
    expectedTurnId: string | null,
    baselineCompletionVersion: string | null,
    requireCompletionVersionChange: boolean
  ) {
    const latestTurn = conversation?.thread.id === sessionId ? conversation.thread.turns.at(-1) : null;
    const expectedTurnIsLoaded = Boolean(
      expectedTurnId &&
      conversation?.thread.id === sessionId &&
      conversation.thread.turns.some((turn) => turn.id === expectedTurnId)
    );
    const knownCompletionVersion =
      expectedTurnId && expectedTurnIsLoaded
        ? (sessionTurnVersionsById[expectedTurnId] ?? null)
        : expectedTurnId
          ? null
          : (latestTurn ? (sessionTurnVersionsById[latestTurn.id] ?? null) : null);
    const requestActiveTurnId = conversation?.thread.id === sessionId ? conversation.activeTurnId : null;
    const scopeKey = sessionStateKey(sessionId, profileId);
    const requestStreamCursor = sessionStreamCursors.get(scopeKey) ?? null;
    const payload = await api.getSessionLatestCompletedTurn(
      sessionId,
      expectedTurnId,
      knownCompletionVersion,
      profileId
    );
    if (
      selectedSessionId !== sessionId ||
      selectedSessionProfileId !== profileId ||
      sessionSelectionVersion !== selectionVersion
    ) {
      return {
        settled: true,
        retryAfterMs: 0
      };
    }
    reconcileSelectedSessionStreamBoundary(
      scopeKey,
      sessionId,
      profileId,
      requestStreamCursor,
      payload
    );

    const applied = applyLatestCompletedTurnPayload(
      sessionId,
      profileId,
      payload,
      expectedTurnId,
      requestActiveTurnId
    );
    const completionChanged =
      !requireCompletionVersionChange ||
      !baselineCompletionVersion ||
      (payload.completionVersion !== null && payload.completionVersion !== baselineCompletionVersion);
    return {
      settled: payload.expectedTurnReady && payload.sourceStable && completionChanged && applied,
      retryAfterMs: Math.max(200, Math.min(Number(payload.retryAfterMs) || 750, 2_000))
    };
  }

  function scheduleSelectedSessionCompletionRefresh(
    sessionId: string,
    expectedTurnId: string | null = null,
    baselineCompletionVersion: string | null = null,
    requireCompletionVersionChange = false,
    probeDespiteLiveDetail = false
  ) {
    const scheduledSelectionVersion = sessionSelectionVersion;
    const scheduledProfileId = profileIdForSession(sessionId);
    const normalizedExpectedTurnId = String(expectedTurnId ?? "").trim() || null;
    const jobKey = `${sessionId}\u0000${normalizedExpectedTurnId ?? "latest"}`;
    const existingJob = selectedSessionCompletionRefreshJobs.get(jobKey);
    if (existingJob) {
      for (const timer of existingJob.timers) {
        clearTimeout(timer);
      }
    }
    const refreshJob = {
      timers: new Set<ReturnType<typeof setTimeout>>()
    };
    selectedSessionCompletionRefreshJobs.set(jobKey, refreshJob);
    const retryDelays = [200, 500, 1_000, 2_000, 4_000, 8_000, 16_000];

    const scheduleAttempt = (attempt: number, delay: number) => {
      const timer = setTimeout(() => {
        refreshJob.timers.delete(timer);
        if (
          selectedSessionCompletionRefreshJobs.get(jobKey) !== refreshJob ||
          selectedSessionId !== sessionId ||
          sessionSelectionVersion !== scheduledSelectionVersion ||
          profileIdForSession(sessionId) !== scheduledProfileId ||
          !conversation ||
          conversation.thread.id !== sessionId ||
          (!normalizedExpectedTurnId &&
            !probeDespiteLiveDetail &&
            isLiveConversationStatus(conversation.thread.status))
        ) {
          if (selectedSessionCompletionRefreshJobs.get(jobKey) === refreshJob) {
            selectedSessionCompletionRefreshJobs.delete(jobKey);
          }
          return;
        }

        if (loadingDetail) {
          scheduleAttempt(attempt, 250);
          return;
        }

        void refreshSelectedSessionCompletionTail(
          sessionId,
          scheduledProfileId,
          scheduledSelectionVersion,
          normalizedExpectedTurnId,
          baselineCompletionVersion,
          requireCompletionVersionChange
        ).then((result) => {
          if (selectedSessionCompletionRefreshJobs.get(jobKey) !== refreshJob) {
            return;
          }
          if (result.settled) {
            for (const pendingTimer of refreshJob.timers) {
              clearTimeout(pendingTimer);
            }
            selectedSessionCompletionRefreshJobs.delete(jobKey);
            return;
          }
          const nextAttempt = attempt + 1;
          if (nextAttempt < retryDelays.length) {
            scheduleAttempt(nextAttempt, Math.max(retryDelays[nextAttempt], result.retryAfterMs));
          } else {
            selectedSessionCompletionRefreshJobs.delete(jobKey);
          }
        }).catch(() => {
          if (selectedSessionCompletionRefreshJobs.get(jobKey) !== refreshJob) {
            return;
          }
          const nextAttempt = attempt + 1;
          if (nextAttempt < retryDelays.length) {
            scheduleAttempt(nextAttempt, retryDelays[nextAttempt]);
          } else {
            selectedSessionCompletionRefreshJobs.delete(jobKey);
          }
        });
      }, delay);
      refreshJob.timers.add(timer);
    };

    scheduleAttempt(0, retryDelays[0]);
  }

  function getRequestedSessionIdFromUrl() {
    if (typeof window === "undefined") {
      return null;
    }
    const value = new URL(window.location.href).searchParams.get(sessionQueryParamKey)?.trim() ?? "";
    return value || null;
  }

  function getDraftSessionRequestedFromUrl() {
    if (typeof window === "undefined") {
      return false;
    }
    const params = new URL(window.location.href).searchParams;
    return !params.get(sessionQueryParamKey)?.trim() && isEnabledQueryParam(params.get(sessionNewParamKey));
  }

  function syncSelectedSessionInUrl(sessionId: string | null, options: { draft?: boolean } = {}) {
    if (typeof window === "undefined") {
      return;
    }

    const url = new URL(window.location.href);
    if (sessionId) {
      url.searchParams.set(sessionQueryParamKey, sessionId);
      url.searchParams.delete(sessionNewParamKey);
    } else if (options.draft) {
      url.searchParams.delete(sessionQueryParamKey);
      url.searchParams.set(sessionNewParamKey, "1");
    } else {
      url.searchParams.delete(sessionQueryParamKey);
      url.searchParams.delete(sessionNewParamKey);
    }

    const nextUrl = `${url.pathname}${url.search}${url.hash}`;
    const currentUrl = `${window.location.pathname}${window.location.search}${window.location.hash}`;
    if (nextUrl !== currentUrl) {
      window.history.replaceState(window.history.state, "", nextUrl);
    }
  }

  function resetWorkspaceState() {
    sessionSelectionVersion += 1;
    disconnectStream();
    clearHydrationRefresh();

    if (sessionRefreshTimer) {
      clearTimeout(sessionRefreshTimer);
      sessionRefreshTimer = null;
    }
    if (selectedSessionDetailRefreshTimer) {
      clearTimeout(selectedSessionDetailRefreshTimer);
      selectedSessionDetailRefreshTimer = null;
    }
    clearSelectedSessionCompletionRefreshes();
    if (sessionListCachePersistTimer) {
      clearTimeout(sessionListCachePersistTimer);
      sessionListCachePersistTimer = null;
    }
    if (sessionDetailCachePersistTimer) {
      clearTimeout(sessionDetailCachePersistTimer);
      sessionDetailCachePersistTimer = null;
    }
    sessionDetailCachePersistMode = null;
    queuedSessionDetailCachePersist = null;
    if (saveTimer) {
      clearTimeout(saveTimer);
      saveTimer = null;
    }

    for (const timer of itemDetailRefreshTimers.values()) {
      clearTimeout(timer);
    }
    itemDetailRefreshTimers.clear();
    if (websocketResyncTimer) {
      clearTimeout(websocketResyncTimer);
      websocketResyncTimer = null;
    }

    config = null;
    quota = null;
    profileAccounts = [];
    profileAccountsBusy = false;
    profileAccountsRefreshPromise = null;
    profileAccountsForceRefreshQueued = false;
    resetTickets = null;
    sessions = [];
    sessionsCursor = null;
    sessionsHasMore = false;
    sessionsLoadingMore = false;
    sessionListCacheKey = null;
    sessionListCacheVersion = null;
    sessionListStateHash = null;
    sessionListWindowKey = null;
    sessionListRequestedLimit = sessionPageSize;
    sessionSummaryVersionsById = {};
    conversation = null;
    selectedSessionId = null;
    selectedSessionProfileId = null;
    sessionProfileIdsBySessionId = {};
    sessionDetailCacheVersion = null;
    sessionDetailStateHash = null;
    sessionDetailMetadataVersion = null;
    sessionTurnVersionsById = {};
    loadingDetail = false;
    sessionsBusy = false;
    sending = false;
    startingMessage = false;
    uploading = false;
    errorText = "";
    noticeText = "";
    draft = "";
    draftAttachments = [];
    titleDraft = "";
    browserOpen = false;
    browserBusy = false;
    directoryPayload = null;
    requestAnswers = {};
    rawRequestResponses = {};
    pendingSessionEvents = {};
    expandedItems = {};
    expandedFileChangeEntries = {};
    loadingItemDetails = {};
    itemDetailErrors = {};
    expandedTurnLogs = {};
    turnEntryRenderLimits = {};
    expandedLargeOutputs = {};
    loadingTurns = {};
    turnLoadErrors = {};
    resetSessionTurnSearch();
    sessionSearchQuery = "";
    sessionSearchScope = "summary";
    sessionFilter = normalizeSessionFilterState(null);
    activeSavedSessionFilterId = null;
    activeSessionFolder = null;
    showArchivedSessions = false;
    accountLoginFlow = null;
    composerSettingsOpen = false;
    composerSettingsPopoverStyle = "";
    loadingOlderTurns = false;
    olderTurnsAutoLoadEnabled = true;
    olderTurnsAutoLoadPaused = false;
    olderTurnsAutoTriggerTimestamps = [];
    terminals = [];
    activeWorkspaceTabId = "chat";
    workspaceMenuOpen = false;
    tasksTabOpen = false;
    gitTabOpen = false;
    settingsTabOpen = false;
    gitDiffTabs = [];
    codeDiffTabs = [];
    fileTabs = [];
    pendingSteerResume = null;
    dismissedQueueResumeBySessionId = {};
    draftPersistencePaused = false;
    mobileSidebarOpen = false;
    optimisticMessage = null;
    optimisticQueuedItemsBySessionId = {};
    sessionQueueSnapshotsBySessionId = {};
    queuedMessageRequestCountsBySessionId = {};
    pendingEnqueuesByOptimisticId.clear();
    sessionEventRevisions.clear();
    sessionStreamCursors.clear();
    queueStateRevisions.clear();
    clearPendingQueueMode();
    liveTurnCardExpanded = false;
    sendIntent = null;
    dismissLastComposerPromptChip();
    editingQueueId = null;
    editingQueuePrompt = "";
    startupAlertModalOpen = false;
    startupAlertDismissed = false;
    startupAlertInitialConfigHandled = false;
    sessionRecoveryPrompt = null;
    dismissedSessionRecoveryPromptForSessionId = null;
    syncSelectedSessionInUrl(null);
  }

  function clearWorkspaceForLoggedOut() {
    resetWorkspaceState();
    authenticated = false;
    activeProfileId = null;
    api.setDefaultProfileId(null);
    webRole = null;
    viewerGitRepoPath = null;
    runtime = null;
    notifications = [];
    loginBusy = false;
    loginHcaptchaToken = "";
    loginHcaptchaWidgetId = null;
    notificationsBusy = false;
    resetRealtimeConnectionForAuthChange();
  }

  function ensureGlobalStreamSubscription() {
    if (releaseGlobalStream) {
      return;
    }

    releaseGlobalStream = api.subscribeGlobal((event) => {
      handleGlobalEvent(event);
    });
  }

  function syncConfiguredTheme(nextConfig: AppConfigPayload | null | undefined) {
    const themeSettings = (nextConfig as ThemedConfigPayload | null | undefined)?.theme;
    if (!themeSettings) {
      return;
    }
    const detail = applyThemeSettings(themeSettings, themeMode);
    themeMode = detail.mode;
    resolvedTheme = detail.resolved;
  }

  async function bootstrap() {
    loading = true;
    errorText = "";
    noticeText = "";

    try {
      const authSession = await api.getAuthSession();
      activeProfileId = authSession.activeProfileId ?? "default";
      api.setDefaultProfileId(activeProfileId);
      loginHcaptcha = authSession.hcaptcha ?? { enabled: false, siteKey: null };
      if (!authSession.authenticated) {
        clearWorkspaceForLoggedOut();
        loading = false;
        return;
      }

      authenticated = true;
      webRole = authSession.role ?? "admin";
      loginMessage = "";
      ensureGlobalStreamSubscription();
      void refreshRuntimeStatus(false, { silent: true });

      readSessionListStateFromUrl();
      const requestedSessionId = getRequestedSessionIdFromUrl();
      const draftSessionRequested = getDraftSessionRequestedFromUrl();
      const [nextConfig] = await Promise.all([api.getConfig(), refreshSessions()]);
      config = applyLocalComposerPreferencesToConfig(nextConfig);
      syncConfiguredTheme(config);
      syncStartupAlertModal(config);
      void refreshQuota(false);
      void refreshResetTickets(false);
      void refreshAccountState(false);
      void refreshNotifications();
      void refreshTerminals();
      loading = false;

      if (requestedSessionId) {
        const restored = await selectSession(requestedSessionId);
        if (restored) {
          return;
        }
        syncSelectedSessionInUrl(null);
      }

      if (draftSessionRequested) {
        activateDraftSession(config.defaults);
        return;
      }

      const firstSession = sessions[0] ?? null;
      if (firstSession) {
        await selectSession(firstSession.id);
        return;
      }

      activateDraftSession(config.defaults);
    } catch (error) {
      errorText = describeError(error);
    }

    loading = false;
  }

  async function refreshSessions(pinnedSession: SessionSummary | null = null) {
    const requestVersion = ++sessionListRequestVersion;
    sessionsBusy = true;
    sessionsLoadingMore = false;
    const query = sessionSearchQuery.trim();
    ensureSessionListWindowKey(buildSessionListWindowKey());
    const refreshLimit = Math.max(sessionPageSize, sessions.length, sessionListRequestedLimit);
    const listCacheKey = buildSessionListBrowserCacheKey(null, refreshLimit);
    let knownVersion: string | null = sessionListCacheKey === listCacheKey ? sessionListCacheVersion : null;
    let knownSummaryVersions: Record<string, string> | null =
      knownVersion && sessionListCacheKey === listCacheKey && sessionListStateHash
        ? boundedVersionHints(sessionSummaryVersionsById, SESSION_LIST_VERSION_HINT_LIMIT)
        : null;
    let knownStateHash: string | null = knownVersion && sessionListCacheKey === listCacheKey ? sessionListStateHash : null;
    const shouldHydrateListFromBrowserCache =
      Boolean(listCacheKey) && (sessionListCacheKey !== listCacheKey || sessions.length === 0);

    try {
      if (!knownVersion && listCacheKey && shouldHydrateListFromBrowserCache) {
        const cachedEntry = await readSessionListCache(listCacheKey);
        if (requestVersion !== sessionListRequestVersion) {
          return;
        }
        if (cachedEntry) {
          mergeSessionPage(cachedEntry.payload, pinnedSession, false);
          sessionListCacheKey = listCacheKey;
          sessionListCacheVersion = cachedEntry.version;
          knownVersion = cachedEntry.version;
          knownSummaryVersions = sessionListStateHash
            ? boundedVersionHints(sessionSummaryVersionsById, SESSION_LIST_VERSION_HINT_LIMIT)
            : null;
          knownStateHash = sessionListStateHash;
        }
      }

      const response = query
        ? await api.searchSessions(
            query,
            sessionSearchScope,
            showArchivedSessions,
            null,
            refreshLimit,
            sessionFilter,
            knownVersion,
            knownSummaryVersions,
            knownStateHash
          )
        : await api.getSessions(
            showArchivedSessions,
            null,
            refreshLimit,
            sessionFilter,
            knownVersion,
            knownSummaryVersions,
            knownStateHash
          );

      if (requestVersion !== sessionListRequestVersion) {
        return;
      }

      if (isCacheValidationResponse(response)) {
        sessionListCacheKey = listCacheKey;
        sessionListCacheVersion = response.cacheVersion;
        return;
      }

      if (isSessionListPatchResponse(response)) {
        if (applySessionListPatch(response, pinnedSession)) {
          if (listCacheKey) {
            void writeSessionListCache(listCacheKey, currentSessionListPayload(), response.cacheVersion);
          }
          return;
        }

        const fallback = query
          ? await api.searchSessions(query, sessionSearchScope, showArchivedSessions, null, refreshLimit, sessionFilter)
          : await api.getSessions(showArchivedSessions, null, refreshLimit, sessionFilter);
        if (requestVersion !== sessionListRequestVersion || isCacheValidationResponse(fallback) || isSessionListPatchResponse(fallback)) {
          return;
        }
        mergeSessionPage(fallback, pinnedSession, false);
        if (listCacheKey) {
          sessionListCacheKey = listCacheKey;
          sessionListCacheVersion = fallback.cacheVersion;
          void writeSessionListCache(listCacheKey, fallback, fallback.cacheVersion);
        }
        return;
      }

      mergeSessionPage(response, pinnedSession, false);
      if (listCacheKey) {
        sessionListCacheKey = listCacheKey;
        sessionListCacheVersion = response.cacheVersion;
        void writeSessionListCache(listCacheKey, response, response.cacheVersion);
      }
    } catch (error) {
      if (requestVersion === sessionListRequestVersion) {
        errorText = describeError(error);
      }
    } finally {
      if (requestVersion === sessionListRequestVersion) {
        sessionsBusy = false;
      }
    }
  }

  async function refreshNotifications() {
    notificationsBusy = true;
    try {
      const payload = await api.getNotifications();
      notifications = payload.notifications;
      if (config) {
        config = {
          ...config,
          notifications: {
            ...config.notifications,
            unreadCount: payload.unreadCount
          }
        };
      }
    } catch (error) {
      errorText = describeError(error);
    } finally {
      notificationsBusy = false;
    }
  }

  async function markNotificationsRead(ids: string[] | null = null) {
    const payload = await api.markNotificationsRead(ids);
    notifications = payload.notifications;
    if (config) {
      config = {
        ...config,
        notifications: {
          ...config.notifications,
          unreadCount: payload.unreadCount
        }
      };
    }
  }

  async function clearNotifications() {
    const payload = await api.clearNotifications();
    notifications = payload.notifications;
    if (config) {
      config = {
        ...config,
        notifications: {
          ...config.notifications,
          unreadCount: payload.unreadCount
        }
      };
    }
  }

  async function saveNotificationSettings(settings: Partial<NotificationSettings>) {
    const payload = await api.updateNotificationSettings(settings);
    if (config) {
      config = {
        ...config,
        notifications: {
          unreadCount: payload.unreadCount,
          settings: payload.settings
        }
      };
    }
  }

  async function loadMoreSessions() {
    if (!sessionsHasMore || sessionsLoadingMore) {
      return;
    }

    const requestVersion = sessionListRequestVersion;
    const cursor = sessionsCursor;
    if (!cursor) {
      return;
    }
    ensureSessionListWindowKey(buildSessionListWindowKey());
    sessionListRequestedLimit = Math.max(sessionListRequestedLimit, sessions.length + sessionPageSize);

    sessionsLoadingMore = true;
    try {
      const response = sessionSearchQuery.trim()
        ? await api.searchSessions(sessionSearchQuery.trim(), sessionSearchScope, showArchivedSessions, cursor, sessionPageSize, sessionFilter)
        : await api.getSessions(showArchivedSessions, cursor, sessionPageSize, sessionFilter);

      if (requestVersion !== sessionListRequestVersion) {
        return;
      }

      if (isCacheValidationResponse(response)) {
        return;
      }
      if (isSessionListPatchResponse(response)) {
        return;
      }

      mergeSessionPage(response, shouldPinSession(selectedSessionSummary) ? selectedSessionSummary : null, true);
      scheduleSessionListCachePersist(null);
    } catch (error) {
      if (requestVersion === sessionListRequestVersion) {
        errorText = describeError(error);
      }
    } finally {
      if (requestVersion === sessionListRequestVersion) {
        sessionsLoadingMore = false;
      }
    }
  }

  async function refreshAccountState(showError: boolean) {
    if (!runtime?.installed) {
      return;
    }

    try {
      applyAccountState(await api.getAccount());
    } catch (error) {
      if (showError) {
        errorText = describeError(error);
      }
    }
  }

  async function refreshTerminals() {
    try {
      terminals = (await api.listTerminals()).terminals;
      if (activeWorkspaceTabId.startsWith("terminal:")) {
        const terminalId = activeWorkspaceTabId.replace(/^terminal:/u, "");
        if (!terminals.some((terminal) => terminal.id === terminalId)) {
          activeWorkspaceTabId = "chat";
        }
      }
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function selectSession(sessionId: string, profileId: string | null = null) {
    const currentSelectionProfileId =
      selectedSessionId === sessionId
        ? (selectedSessionProfileId ?? conversation?.profileId ?? sessionProfileIdsBySessionId[sessionId] ?? activeProfileId)
        : null;
    if (selectedSessionId === sessionId && conversation && (!profileId || currentSelectionProfileId === profileId)) {
      syncSelectedSessionInUrl(sessionId);
      if (!isLiveConversationStatus(conversation.thread.status)) {
        const latestTurn = conversation.thread.turns.at(-1);
        scheduleSelectedSessionCompletionRefresh(
          sessionId,
          null,
          latestTurn ? (sessionTurnVersionsById[latestTurn.id] ?? null) : null
        );
      }
      return true;
    }

    const matchingSummaries = sessions.filter((session) => session.id === sessionId);
    const summaryForSelection =
      (profileId
        ? matchingSummaries.find((session) => session.profileId === profileId) ??
          (matchingSummaries.length === 1 && !matchingSummaries[0].profileId ? matchingSummaries[0] : null)
        : null) ??
      matchingSummaries[0] ??
      null;
    const resolvedProfileId = profileId ?? summaryForSelection?.profileId ?? sessionProfileIdsBySessionId[sessionId] ?? activeProfileId;
    const selectionScopeKey = sessionStateKey(sessionId, resolvedProfileId);
    const selectionVersion = sessionSelectionVersion + 1;
    sessionSelectionVersion = selectionVersion;
    clearHydrationRefresh();
    dismissLastComposerPromptChip();
    resetSessionTurnSearch();
    loadingDetail = true;
    selectedSessionId = sessionId;
    selectedSessionProfileId = resolvedProfileId;
    if (resolvedProfileId) {
      rememberSessionProfile({ id: sessionId, profileId: resolvedProfileId });
    }
    syncSelectedSessionInUrl(sessionId);
    conversation =
      summaryForSelection && config
        ? createConversationState({
            profileId: resolvedProfileId,
            profileLabel: summaryForSelection.profileLabel ?? null,
            profileCodexHome: summaryForSelection.profileCodexHome ?? null,
            accountEmail: summaryForSelection.accountEmail ?? null,
            accountType: summaryForSelection.accountType ?? null,
            thread: {
              id: summaryForSelection.id,
              preview: summaryForSelection.preview,
              name: summaryForSelection.name,
              cwd: summaryForSelection.cwd,
              status: summaryForSelection.status,
              createdAt: normalizeSessionTimestamp(summaryForSelection.createdAt),
              updatedAt: normalizeSessionTimestamp(summaryForSelection.updatedAt),
              isSubagent: summaryForSelection.isSubagent,
              agentNickname: summaryForSelection.agentNickname,
              agentRole: summaryForSelection.agentRole,
              turns: []
            },
            preferences: summaryForSelection.preferences ?? config.defaults,
            selectedSkills: [],
            goal: null,
            attachments: [],
            queue: sessionQueueSnapshotsBySessionId[selectionScopeKey] ?? {
              sessionId,
              items: [],
              resumeRequired: false,
              updatedAt: null
            },
            pendingRequests: [],
            activeTurnId: null,
            tokenUsage: null,
            hydration: {
              state: "loading",
              loadedTurns: 0,
              totalTurns: null,
              remainingTurns: 0,
              message: null,
              recovery: {
                available: false,
                issue: null,
                totalLines: null,
                recoverableLines: null,
                skippedLines: null
              }
            },
            cacheVersion: "",
            notModified: false
          })
        : null;
    pendingSessionEvents = {
      [selectionScopeKey]: []
    };
    expandedItems = {};
    expandedFileChangeEntries = {};
    loadingItemDetails = {};
    itemDetailErrors = {};
    expandedTurnLogs = {};
    turnEntryRenderLimits = {};
    expandedLargeOutputs = {};
    loadingTurns = {};
    turnLoadErrors = {};
    for (const timer of itemDetailRefreshTimers.values()) {
      clearTimeout(timer);
    }
    itemDetailRefreshTimers.clear();
    draft = "";
    draftAttachments = [];
    titleDraft = "";
    pendingInitialTranscriptScrollSessionId = sessionId;
    requestTranscriptBottomScroll(true);
    olderTurnsAutoLoadEnabled = false;
    olderTurnsAutoLoadPaused = false;
    olderTurnsAutoTriggerTimestamps = [];
    loadingOlderTurns = false;
    sessionDetailCacheVersion = null;
    sessionDetailStateHash = null;
    sessionDetailMetadataVersion = null;
    sessionTurnVersionsById = {};
    pendingSteerResume = null;
    optimisticMessage = null;
    sendIntent = null;
    sessionRecoveryPrompt = null;
    dismissedSessionRecoveryPromptForSessionId = null;
    manualCompactPrompt = null;
    dismissedManualCompactPromptForSessionId = null;
    clearStaleSessionCatchup();
    mobileSidebarOpen = false;
    composerSettingsOpen = false;
    activeWorkspaceTabId = "chat";
    disconnectStream();
    connectStream(sessionId, resolvedProfileId, selectionVersion);

    try {
      const detailCacheKey = buildSessionDetailBrowserCacheKey(sessionId, resolvedProfileId);
      let knownVersion: string | null = null;
      let turnLimit = olderTurnPageSize;
      let cachedCompletionVersion: string | null = null;
      let cachedDetailNeedsCompletionUpdate = false;
      const terminalSelection = Boolean(
        summaryForSelection && !isLiveConversationStatus(summaryForSelection.status)
      );

      if (detailCacheKey) {
        const cacheReadEventRevision = sessionEventRevisions.get(selectionScopeKey) ?? 0;
        const cachedEntry = await readSessionDetailCache(detailCacheKey);
        if (selectedSessionId !== sessionId || sessionSelectionVersion !== selectionVersion) {
          return false;
        }
        if (cachedEntry) {
          sessionDetailCacheVersion = cachedEntry.version;
          applyLoadedSessionDetail(
            sessionId,
            resolvedProfileId,
            cachedEntry.payload,
            true,
            (sessionEventRevisions.get(selectionScopeKey) ?? 0) > cacheReadEventRevision
          );
          knownVersion =
            cachedEntry.payload.stateHash && cachedEntry.payload.turnVersions
              ? cachedEntry.version
              : null;
          const cachedLatestTurn = cachedEntry.payload.thread.turns.at(-1);
          cachedCompletionVersion = cachedLatestTurn
            ? (cachedEntry.payload.turnVersions?.[cachedLatestTurn.id] ?? null)
            : null;
          cachedDetailNeedsCompletionUpdate =
            terminalSelection &&
            normalizeSessionTimestamp(summaryForSelection?.updatedAt) >
              normalizeSessionTimestamp(cachedEntry.payload.thread.updatedAt);
          turnLimit = Math.max(olderTurnPageSize, cachedEntry.payload.thread.turns.length);
          if (terminalSelection) {
            knownVersion = null;
          }
          requestTranscriptBottomScroll(true);
        }
      }
      if (terminalSelection) {
        knownVersion = null;
      }

      const nextConversation = await refreshSelectedSessionState(
        sessionId,
        turnLimit,
        true,
        knownVersion,
        false,
        selectionVersion,
        resolvedProfileId
      );
      if (selectedSessionId !== sessionId || sessionSelectionVersion !== selectionVersion || !nextConversation) {
        return false;
      }
      const hasOnlyLiveTurnShell =
        nextConversation.thread.turns.length <= 1 &&
        nextConversation.thread.turns.every((turn) => String(turn.status ?? "") === "inProgress");
      if (
        (nextConversation.thread.turns.length === 0 || hasOnlyLiveTurnShell) &&
        (nextConversation.activeTurnId || nextConversation.thread.status === "running" || nextConversation.thread.status === "active")
      ) {
        clearHydrationRefresh();
        hydrationRefreshTimer = setTimeout(() => {
          hydrationRefreshTimer = null;
          if (selectedSessionId !== sessionId || sessionSelectionVersion !== selectionVersion) {
            return;
          }
          void refreshSelectedSessionState(
            sessionId,
            olderTurnPageSize,
            false,
            sessionDetailCacheVersion,
            false,
            selectionVersion,
            resolvedProfileId
          ).catch(() => {});
        }, 250);
      } else {
        clearHydrationRefresh();
      }
      if (terminalSelection || !isLiveConversationStatus(nextConversation.thread.status)) {
        scheduleSelectedSessionCompletionRefresh(
          sessionId,
          null,
          cachedCompletionVersion,
          cachedDetailNeedsCompletionUpdate,
          true
        );
      }
      syncSelectedSessionInUrl(sessionId);
      return true;
    } catch (error) {
      if (selectedSessionId === sessionId && sessionSelectionVersion === selectionVersion) {
        clearHydrationRefresh();
        errorText = describeError(error);
        if (conversation?.thread.id === sessionId) {
          scheduleSelectedSessionStateRefresh(sessionId, 1_500);
        } else {
          disconnectStream();
          syncSelectedSessionInUrl(null);
        }
      }
      return false;
    } finally {
      if (selectedSessionId === sessionId && sessionSelectionVersion === selectionVersion) {
        loadingDetail = false;
        const currentConversation = conversation as unknown as ConversationState | null;
        if (currentConversation?.thread.id === sessionId && isInitialTranscriptScrollPending(sessionId)) {
          requestTranscriptBottomScroll(true);
          scheduleTranscriptScrollToBottom();
        }
      }
    }
  }

  function rebindSelectedSessionProfile(
    sessionId: string,
    sourceProfileId: string | null,
    targetProfileId: string,
    movedSummary: SessionSummary | null = null
  ) {
    const normalizedTargetProfileId = normalizeSessionStateProfileId(targetProfileId);
    const normalizedSourceProfileId = normalizeSessionStateProfileId(
      sourceProfileId ?? selectedSessionProfileId
    );
    if (
      selectedSessionId !== sessionId ||
      normalizeSessionStateProfileId(selectedSessionProfileId) !== normalizedSourceProfileId ||
      normalizedSourceProfileId === normalizedTargetProfileId
    ) {
      return false;
    }

    const sourceScopeKey = sessionStateKey(sessionId, normalizedSourceProfileId);
    const targetScopeKey = sessionStateKey(sessionId, normalizedTargetProfileId);
    const targetProfile = config?.profiles.find((profile) => profile.id === normalizedTargetProfileId) ?? null;
    const selectionVersion = sessionSelectionVersion + 1;
    const previousDraftPersistencePaused = draftPersistencePaused;
    sessionSelectionVersion = selectionVersion;
    draftPersistencePaused = true;
    clearHydrationRefresh();
    clearStaleSessionCatchup();
    if (draftSaveTimer) {
      clearTimeout(draftSaveTimer);
      draftSaveTimer = null;
    }

    const nextPendingSessionEvents = { ...pendingSessionEvents };
    const sourcePendingSessionEvents = nextPendingSessionEvents[sourceScopeKey] ?? [];
    delete nextPendingSessionEvents[sourceScopeKey];
    nextPendingSessionEvents[targetScopeKey] = [
      ...(nextPendingSessionEvents[targetScopeKey] ?? []),
      ...sourcePendingSessionEvents
    ];
    pendingSessionEvents = nextPendingSessionEvents;

    const nextQueueSnapshots = { ...sessionQueueSnapshotsBySessionId };
    if (nextQueueSnapshots[sourceScopeKey]) {
      nextQueueSnapshots[targetScopeKey] = nextQueueSnapshots[sourceScopeKey];
    }
    delete nextQueueSnapshots[sourceScopeKey];
    sessionQueueSnapshotsBySessionId = nextQueueSnapshots;

    const nextOptimisticQueueItems = { ...optimisticQueuedItemsBySessionId };
    if (nextOptimisticQueueItems[sourceScopeKey]) {
      nextOptimisticQueueItems[targetScopeKey] = nextOptimisticQueueItems[sourceScopeKey];
    }
    delete nextOptimisticQueueItems[sourceScopeKey];
    optimisticQueuedItemsBySessionId = nextOptimisticQueueItems;

    const nextQueuedRequestCounts = { ...queuedMessageRequestCountsBySessionId };
    if (nextQueuedRequestCounts[sourceScopeKey] !== undefined) {
      nextQueuedRequestCounts[targetScopeKey] = nextQueuedRequestCounts[sourceScopeKey];
    }
    delete nextQueuedRequestCounts[sourceScopeKey];
    queuedMessageRequestCountsBySessionId = nextQueuedRequestCounts;

    const nextDismissedQueueResume = { ...dismissedQueueResumeBySessionId };
    if (nextDismissedQueueResume[sourceScopeKey] !== undefined) {
      nextDismissedQueueResume[targetScopeKey] = nextDismissedQueueResume[sourceScopeKey];
    }
    delete nextDismissedQueueResume[sourceScopeKey];
    dismissedQueueResumeBySessionId = nextDismissedQueueResume;

    const sourceEventRevision = sessionEventRevisions.get(sourceScopeKey) ?? 0;
    sessionEventRevisions.delete(sourceScopeKey);
    sessionEventRevisions.set(
      targetScopeKey,
      Math.max(sourceEventRevision, sessionEventRevisions.get(targetScopeKey) ?? 0)
    );
    sessionStreamCursors.delete(sourceScopeKey);
    sessionStreamCursors.delete(targetScopeKey);
    const sourceQueueRevision = queueStateRevisions.get(sourceScopeKey) ?? 0;
    queueStateRevisions.delete(sourceScopeKey);
    queueStateRevisions.set(
      targetScopeKey,
      Math.max(sourceQueueRevision, queueStateRevisions.get(targetScopeKey) ?? 0)
    );

    for (const pendingEnqueue of pendingEnqueuesByOptimisticId.values()) {
      if (sessionStateKey(pendingEnqueue.sessionId, pendingEnqueue.profileId) === sourceScopeKey) {
        pendingEnqueue.profileId = normalizedTargetProfileId;
      }
    }
    if (
      optimisticMessage &&
      sessionStateKey(optimisticMessage.sessionId, optimisticMessage.profileId) === sourceScopeKey
    ) {
      optimisticMessage = {
        ...optimisticMessage,
        profileId: normalizedTargetProfileId
      };
    }
    if (
      pendingSteerResume &&
      sessionStateKey(pendingSteerResume.sessionId, pendingSteerResume.profileId) === sourceScopeKey
    ) {
      pendingSteerResume = {
        ...pendingSteerResume,
        profileId: normalizedTargetProfileId
      };
    }
    if (pendingQueueModeSessionKey === sourceScopeKey) {
      pendingQueueModeSessionKey = targetScopeKey;
    }

    selectedSessionProfileId = normalizedTargetProfileId;
    rememberSessionProfile({ id: sessionId, profileId: normalizedTargetProfileId });
    if (conversation?.thread.id === sessionId) {
      conversation = {
        ...conversation,
        profileId: normalizedTargetProfileId,
        profileLabel: movedSummary?.profileLabel ?? targetProfile?.label ?? null,
        profileCodexHome: movedSummary?.profileCodexHome ?? targetProfile?.codexHome ?? null,
        accountEmail: movedSummary?.accountEmail ?? null,
        accountType: movedSummary?.accountType ?? null
      };
    }
    sessionDetailCacheVersion = null;
    sessionDetailStateHash = null;
    sessionDetailMetadataVersion = null;
    sessionTurnVersionsById = {};
    syncSelectedSessionInUrl(sessionId);
    draftPersistencePaused = previousDraftPersistencePaused;

    if (!conversation || conversation.thread.id !== sessionId) {
      void selectSession(sessionId, normalizedTargetProfileId);
      return true;
    }

    disconnectStream();
    connectStream(sessionId, normalizedTargetProfileId, selectionVersion);
    markConversationCacheDirty();
    void refreshSelectedSessionState(
      sessionId,
      Math.max(conversation.thread.turns.length, olderTurnPageSize),
      false,
      null,
      false,
      selectionVersion,
      normalizedTargetProfileId
    ).catch((error) => {
      if (matchesSessionSelection(sessionId, normalizedTargetProfileId, selectionVersion)) {
        errorText = describeError(error);
        scheduleSelectedSessionStateRefresh(sessionId, 1_500);
      }
    });
    return true;
  }

  function connectStream(sessionId: string, profileId: string | null, selectionVersion: number) {
    disconnectStream();
    releaseSessionStream = api.subscribeSession(sessionId, (payload: StreamEvent) => {
      if (
        selectedSessionId !== sessionId ||
        selectedSessionProfileId !== profileId ||
        sessionSelectionVersion !== selectionVersion
      ) {
        return;
      }
      const scopeKey = sessionStateKey(sessionId, profileId);
      observeSelectedSessionStreamEvent(scopeKey, sessionId, profileId, payload);
      sessionEventRevisions.set(scopeKey, (sessionEventRevisions.get(scopeKey) ?? 0) + 1);
      const queuePayload = queuePayloadFromEvent(payload);
      if (queuePayload) {
        applyQueuePayloadToSession(sessionId, profileId, queuePayload);
      }

      if (payload.kind === "notification" && payload.method === "codex-webui/queueDispatchFailed") {
        if (showSessionRecoveryPromptFromError(payload.params, sessionId)) {
          return;
        }
        errorText = m.queue_dispatch_failed({ message: describeUiError(payload.params) });
      }

      if (payload.kind === "notification" && payload.method === "codex-webui/sessionHydrationFailed") {
        clearHydrationRefresh();
        errorText = m.session_history_failed({ message: describeUiError(payload.params.message ?? payload.params) });
      }

      if (payload.kind === "notification" && (payload.method === "error" || payload.method === "turn/completed")) {
        const params = payload.params as Record<string, unknown>;
        const turn = params.turn && typeof params.turn === "object" ? (params.turn as Record<string, unknown>) : null;
        const appServerError = payload.method === "error" ? (params.error ?? params) : (turn?.error ?? params.error);
        if (appServerError && showManualCompactPromptFromError(appServerError, sessionId)) {
          errorText = describeUiError(appServerError);
        }
        if (appServerError && isUsageLimitErrorPayload(appServerError)) {
          errorText = describeUiError(appServerError);
        }
      }

      if (payload.kind === "serverRequest") {
        notifyAttentionEvent(sessionId, "approval", payload.id, profileId);
      }

      if (conversation?.thread.id === sessionId) {
        const catchup = staleSessionCatchup;
        if (catchup?.sessionId === sessionId) {
          if (catchup.refreshing) {
            staleSessionCatchup = {
              ...catchup,
              eventCount: catchup.eventCount + 1
            };
          } else {
            const nextEventCount = catchup.eventCount + 1;
            if (nextEventCount >= staleSessionCatchupEventThreshold) {
              if (staleSessionCatchupTimer) {
                clearTimeout(staleSessionCatchupTimer);
                staleSessionCatchupTimer = null;
              }
              staleSessionCatchup = {
                ...catchup,
                eventCount: 1,
                refreshing: true
              };
              void refreshSelectedSessionState(
                sessionId,
                olderTurnPageSize,
                false,
                sessionDetailCacheVersion,
                true
              ).catch((error) => {
                clearStaleSessionCatchup();
                errorText = describeError(error);
              });
            } else {
              staleSessionCatchup = {
                ...catchup,
                eventCount: nextEventCount
              };
            }
          }
        }

        if (payload.kind === "notification" && payload.method === "codex-webui/preferencesUpdated") {
          const pendingPatch = pendingPreferencePatchesBySessionId.get(sessionId);
          const incomingPreferences = payload.params.preferences as Partial<SessionPreferences> | undefined;
          if (pendingPatch && (!incomingPreferences || !pendingPreferencePatchMatches(incomingPreferences, pendingPatch))) {
            return;
          }
          pendingPreferencePatchesBySessionId.delete(sessionId);
        }

        if (applyComputerFrameEvent(payload, sessionId)) {
          return;
        }

        if (payload.kind === "notification") {
          if (
            payload.method === "turn/started" ||
            payload.method === "turn/plan/updated" ||
            payload.method === "turn/diff/updated" ||
            payload.method === "thread/tokenUsage/updated" ||
            payload.method === "thread/goal/updated" ||
            (payload.method === "thread/status/changed" && isLiveConversationStatus(String(payload.params.status ?? ""))) ||
            payload.method === "thread/realtime/started" ||
            payload.method === "thread/realtime/transcript/delta" ||
            payload.method === "thread/realtime/transcript/done" ||
            [
              "item/started",
              "item/completed",
              "item/agentMessage/delta",
              "item/plan/delta",
              "item/reasoning/textDelta",
              "item/reasoning/summaryTextDelta",
              "item/reasoning/summaryPartAdded",
              "item/commandExecution/outputDelta",
              "item/commandExecution/requestApproval",
              "item/fileChange/requestApproval",
              "item/permissions/requestApproval",
              "item/tool/call"
            ].includes(payload.method)
          ) {
            noteRecentLiveSessionEvidence(sessionId);
          }
          if (payload.method === "turn/completed" || payload.method === "thread/realtime/closed") {
            clearRecentLiveSessionEvidence(sessionId);
          }
          if (
            payload.method === "thread/status/changed" &&
            shouldDeferTerminalSessionStatus(sessionId, String(payload.params.status ?? ""))
          ) {
            scheduleSelectedSessionStateRefresh(sessionId, recentLiveSessionEvidenceTtlMs + 250);
            return;
          }
        }

        conversation = applyLocalComposerPreferencesToConversation(
          normalizeConversationExecutionState(applyStreamEvent(conversation, payload))
        );
        markConversationCacheDirty(sessionCacheModeForStreamEvent(payload));
        if (
          pendingQueueModeSessionKey === scopeKey &&
          payload.kind === "notification" &&
          (payload.method === "turn/started" ||
            payload.method === "turn/completed" ||
            ((payload.method === "thread/status/changed") &&
              (String(payload.params.status ?? "") === "running" ||
                String(payload.params.status ?? "") === "active" ||
                !isLiveConversationStatus(String(payload.params.status ?? "")))))
        ) {
          clearPendingQueueMode(sessionId, profileId);
        }

        if (
          payload.kind === "notification" &&
          (["turn/started", "turn/completed", "thread/name/updated", "thread/status/changed"].includes(payload.method) ||
            ([
              "item/started",
              "item/agentMessage/delta",
              "item/plan/delta",
              "item/reasoning/textDelta",
              "item/reasoning/summaryTextDelta",
              "item/reasoning/summaryPartAdded",
              "item/commandExecution/outputDelta",
              "item/commandExecution/requestApproval",
              "item/fileChange/requestApproval",
              "item/permissions/requestApproval",
              "item/tool/call"
            ].includes(payload.method) &&
              !isLiveConversationStatus(selectedSessionSummary?.status)))
        ) {
          applySessionSummaryUpdate(buildSessionSummaryFromConversation(conversation));
        }

        if (payload.kind === "notification" && payload.method === "turn/completed") {
          updateManualCompactPrompt(sessionId, conversation);
          const completedTurn = payload.params.turn as { id?: unknown } | undefined;
          const completedTurnId = String(completedTurn?.id ?? payload.params.turnId ?? "").trim() || null;
          scheduleSelectedSessionCompletionRefresh(sessionId, completedTurnId);
        }

        if (isQueueUpdatedEvent(payload)) {
          applyQueueUpdatedSideEffects(sessionId, profileId, conversation);
          applySessionSummaryUpdate(buildSessionSummaryFromConversation(conversation));
        }

        if (payload.kind === "notification" && (payload.method === "item/started" || payload.method === "item/completed")) {
          const turnId = String(payload.params.turnId ?? "");
          const itemRecord = payload.params.item as Record<string, unknown> | undefined;
          const itemId = String(itemRecord?.id ?? payload.params.itemId ?? "");
          const itemType = String(itemRecord?.type ?? "");
          if (turnId && itemId) {
            if (payload.method === "item/started" && itemType === "commandExecution") {
              expandedItems = {
                ...expandedItems,
                [getItemKey(turnId, itemId)]: true
              };
              void loadItemDetail(turnId, itemId, true);
            }
            if (payload.method === "item/completed" && itemType === "commandExecution") {
              expandedItems = {
                ...expandedItems,
                [getItemKey(turnId, itemId)]: false
              };
            }
            scheduleExpandedItemRefresh(turnId, itemId);
          }
        }

        clearHydrationRefresh();
      } else if (selectedSessionId === sessionId) {
        queuePendingSessionEvent(sessionId, profileId, payload);
      }
    }, { includeInitialQueue: false, profileId });
  }

  function disconnectStream() {
    releaseSessionStream?.();
    releaseSessionStream = null;
  }

  function resetRealtimeConnectionForAuthChange() {
    disconnectStream();
    releaseGlobalStream?.();
    releaseGlobalStream = null;
    api.disconnect();
    connectionState = "idle";
  }

  function clearHydrationRefresh() {
    if (!hydrationRefreshTimer) {
      return;
    }
    clearTimeout(hydrationRefreshTimer);
    hydrationRefreshTimer = null;
  }

  function applyComputerFrameEvent(event: StreamEvent, fallbackSessionId: string) {
    if (event.kind !== "notification" || event.method !== "codex-webui/computerFrame") {
      return false;
    }
    const imageUrl = typeof event.params.imageUrl === "string" ? event.params.imageUrl : "";
    const threadId = String(event.params.threadId ?? fallbackSessionId);
    if (!imageUrl || !threadId) {
      return true;
    }
    computerFramesBySessionId = {
      ...computerFramesBySessionId,
      [threadId]: {
        threadId,
        turnId: typeof event.params.turnId === "string" ? event.params.turnId : null,
        itemId: typeof event.params.itemId === "string" ? event.params.itemId : null,
        imageUrl,
        mimeType: typeof event.params.mimeType === "string" ? event.params.mimeType : null,
        tool: typeof event.params.tool === "string" ? event.params.tool : null,
        transport: typeof event.params.transport === "string" ? event.params.transport : "websocket",
        frameMode: typeof event.params.frameMode === "string" ? event.params.frameMode : "snapshot",
        fpsHint: typeof event.params.fpsHint === "number" ? event.params.fpsHint : null,
        updatedAt: typeof event.params.updatedAt === "number" ? event.params.updatedAt : Date.now()
      }
    };
    return true;
  }

  function flushPendingSessionEvents(sessionId: string, profileId: string | null, nextConversation: ConversationState) {
    const scopeKey = sessionStateKey(sessionId, profileId);
    const queued = pendingSessionEvents[scopeKey] ?? [];
    if (queued.length === 0) {
      return nextConversation;
    }

    const remaining = { ...pendingSessionEvents };
    delete remaining[scopeKey];
    pendingSessionEvents = remaining;

    return queued.reduce(
      (current, event) => (applyComputerFrameEvent(event, sessionId) ? current : applyStreamEvent(current, event)),
      nextConversation
    );
  }

  function isQueueUpdatedEvent(event: StreamEvent) {
    return event.kind === "notification" && event.method === "codex-webui/queueUpdated";
  }

  function applyQueueUpdatedSideEffects(sessionId: string, profileId: string | null, currentConversation: ConversationState) {
    const scopeKey = sessionStateKey(sessionId, profileId);
    dismissedQueueResumeBySessionId = {
      ...dismissedQueueResumeBySessionId,
      [scopeKey]: false
    };

    if (!config) {
      return;
    }

    const nextPausedQueues = config.startup.pausedQueues.filter((entry) => entry.sessionId !== sessionId);
    if (currentConversation.queue.resumeRequired && currentConversation.queue.items.length > 0) {
      nextPausedQueues.unshift({
        sessionId,
        name: getConversationDisplayTitle(currentConversation),
        cwd: currentConversation.thread.cwd,
        pendingCount: currentConversation.queue.items.length,
        updatedAt: currentConversation.queue.updatedAt
      });
    }
    config = {
      ...config,
      startup: {
        ...config.startup,
        pausedQueues: nextPausedQueues
      }
    };
  }

  function applyQueuePayloadToSession(sessionId: string, profileId: string | null, queue: SessionQueuePayload) {
    const scopeKey = sessionStateKey(sessionId, profileId);
    const existingQueue =
      sessionQueueSnapshotsBySessionId[scopeKey] ??
      (conversation?.thread.id === sessionId && selectedSessionBindingMatches(sessionId, profileId) ? conversation.queue : null);
    const nextQueue = mergeQueueSnapshot(existingQueue, queue);

    sessionQueueSnapshotsBySessionId = {
      ...sessionQueueSnapshotsBySessionId,
      [scopeKey]: nextQueue
    };
    queueStateRevisions.set(scopeKey, (queueStateRevisions.get(scopeKey) ?? 0) + 1);
    reconcileOptimisticQueuedItems(sessionId, profileId, nextQueue.items);

    if (conversation?.thread.id === sessionId && selectedSessionBindingMatches(sessionId, profileId)) {
      conversation = {
        ...conversation,
        queue: nextQueue
      };
      markConversationCacheDirty();
      applySessionSummaryUpdate(buildSessionSummaryFromConversation(conversation));
    }
  }

  function updateSessionDetailSyncState(detail: SessionDetailPayload) {
    sessionDetailCacheVersion = detail.cacheVersion ?? null;
    sessionDetailStateHash = detail.stateHash ?? null;
    sessionDetailMetadataVersion = detail.metadataVersion ?? null;
    sessionTurnVersionsById = detail.turnVersions ? { ...detail.turnVersions } : {};
  }

  function applySessionDetailPatch(payload: SessionDetailPatchPayload): SessionDetailPayload | null {
    if (!conversation) {
      return null;
    }

    const patch = payload.patch;
    if (
      !sessionDetailStateHash ||
      patch.baseStateHash !== sessionDetailStateHash ||
      !sessionDetailCacheVersion ||
      patch.baseCacheVersion !== sessionDetailCacheVersion
    ) {
      return null;
    }
    const currentTurnsById = new Map(conversation.thread.turns.map((turn) => [turn.id, turn]));
    const upsertsById = new Map(patch.turnUpserts.map((turn) => [turn.id, turn]));
    const turns: SessionDetailPayload["thread"]["turns"] = [];

    for (const turnId of patch.turnIds) {
      const turn = upsertsById.get(turnId) ?? currentTurnsById.get(turnId);
      if (!turn) {
        return null;
      }
      turns.push(turn);
    }

    const computedHash = buildSessionDetailStateHash(patch.metadataVersion, patch.turnIds, patch.turnVersions);
    if (computedHash !== patch.finalStateHash) {
      return null;
    }

    return {
      thread: {
        ...patch.thread,
        turns
      },
      preferences: patch.preferences,
      selectedSkills: patch.selectedSkills,
      goal: patch.goal,
      attachments: patch.attachments,
      queue: patch.queue,
      pendingRequests: patch.pendingRequests,
      activeTurnId: patch.activeTurnId,
      tokenUsage: patch.tokenUsage,
      hydration: patch.hydration,
      turnIds: patch.turnIds,
      turnVersions: patch.turnVersions,
      metadataVersion: patch.metadataVersion,
      stateHash: patch.finalStateHash,
      cacheVersion: payload.cacheVersion,
      streamEpoch: payload.streamEpoch,
      streamSequence: payload.streamSequence,
      notModified: false
    };
  }

  function applyLoadedSessionDetail(
    sessionId: string,
    profileId: string | null,
    detail: SessionDetailPayload,
    flushPendingEvents = true,
    preserveStreamedState = false,
    replaceTranscriptWindow = false
  ) {
    const scopeKey = sessionStateKey(sessionId, profileId);
    const pendingEventsBeforeFlush = pendingSessionEvents[scopeKey] ?? [];
    const hadPendingQueueUpdate = flushPendingEvents && pendingEventsBeforeFlush.some(isQueueUpdatedEvent);
    const currentConversation = conversation;
    const existingConversationMatches =
      currentConversation?.thread.id === detail.thread.id &&
      sessionStateKey(detail.thread.id, currentConversation.profileId ?? selectedSessionProfileId) === scopeKey;
    let nextConversation =
      currentConversation && existingConversationMatches && (!replaceTranscriptWindow || preserveStreamedState)
        ? mergeConversationState(currentConversation, detail, preserveStreamedState)
        : createConversationState(detail);
    if (flushPendingEvents) {
      nextConversation = flushPendingSessionEvents(sessionId, profileId, nextConversation);
    }
    nextConversation = applyLocalComposerPreferencesToConversation(normalizeConversationExecutionState(nextConversation));
    const mergedQueue = mergeQueueSnapshot(sessionQueueSnapshotsBySessionId[scopeKey], nextConversation.queue);
    if (mergedQueue !== nextConversation.queue) {
      nextConversation = {
        ...nextConversation,
        queue: mergedQueue
      };
    }
    conversation = nextConversation;
    if (isInitialTranscriptScrollPending(sessionId)) {
      requestTranscriptBottomScroll(true);
    }
    updateSessionDetailSyncState(detail);
    sessionQueueSnapshotsBySessionId = {
      ...sessionQueueSnapshotsBySessionId,
      [scopeKey]: nextConversation.queue
    };
    reconcileOptimisticQueuedItems(sessionId, profileId, nextConversation.queue.items);
    if (hadPendingQueueUpdate) {
      applyQueueUpdatedSideEffects(sessionId, profileId, nextConversation);
    }
    if (
      pendingQueueModeSessionKey === scopeKey &&
      (hasConversationLiveTurn(nextConversation) || !isLiveConversationStatus(nextConversation.thread.status))
    ) {
      clearPendingQueueMode(sessionId, profileId);
    }
    titleDraft = getConversationDisplayTitle(nextConversation) ?? "";
    upsertSessionSummary(buildSessionSummaryFromConversation(nextConversation), false);
    clearHydrationRefresh();
    if (!replaceTranscriptWindow) {
      clearStaleSessionCatchup();
    }
    updateSessionRecoveryPrompt(sessionId, nextConversation);
    updateManualCompactPrompt(sessionId, nextConversation);
    return nextConversation;
  }

  function updateSessionRecoveryPrompt(sessionId: string, nextConversation: ConversationState) {
    const recovery = nextConversation.hydration.recovery;
    if (
      nextConversation.hydration.state !== "error" ||
      !recovery.available ||
      dismissedSessionRecoveryPromptForSessionId === sessionId
    ) {
      if (sessionRecoveryPrompt?.sessionId === sessionId && !sessionRecoveryPrompt.busy) {
        sessionRecoveryPrompt = null;
      }
      return;
    }

    sessionRecoveryPrompt = {
      sessionId,
      message: nextConversation.hydration.message ?? "",
      issue: recovery.issue,
      totalLines: recovery.totalLines,
      recoverableLines: recovery.recoverableLines,
      skippedLines: recovery.skippedLines,
      busy: sessionRecoveryPrompt?.sessionId === sessionId ? sessionRecoveryPrompt.busy : false
    };
  }

  function showSessionRecoveryPromptFromError(error: unknown, fallbackSessionId: string | null = null) {
    const parsed = parseAppError(error);
    if (parsed?.code !== "SESSION_ROLLOUT_RECOVERY_REQUIRED") {
      return false;
    }

    const sessionId = parsed.sessionId ?? fallbackSessionId;
    const recovery = parsed.recovery;
    if (!sessionId || !recovery?.available) {
      return false;
    }

    dismissedSessionRecoveryPromptForSessionId = null;
    sessionRecoveryPrompt = {
      sessionId,
      message: parsed.message ?? m.session_history_recovery_generic_message(),
      issue: typeof recovery.issue === "string" ? recovery.issue : null,
      totalLines: typeof recovery.totalLines === "number" ? recovery.totalLines : null,
      recoverableLines: typeof recovery.recoverableLines === "number" ? recovery.recoverableLines : null,
      skippedLines: typeof recovery.skippedLines === "number" ? recovery.skippedLines : null,
      busy: false
    };
    return true;
  }

  function getConversationContextWindowMessage(nextConversation: ConversationState) {
    for (const turn of [...nextConversation.thread.turns].reverse()) {
      if (!turn.error) {
        continue;
      }
      if (isContextWindowExceededPayload(turn.error)) {
        return describeUiError(turn.error);
      }
      if (isContextWindowExceededPayload(turn.error.message)) {
        return turn.error.message ?? ui.manualCompactDescription;
      }
    }
    return null;
  }

  function updateManualCompactPrompt(sessionId: string, nextConversation: ConversationState) {
    const message = getConversationContextWindowMessage(nextConversation);
    if (!message || dismissedManualCompactPromptForSessionId === sessionId) {
      if (manualCompactPrompt?.sessionId === sessionId && !manualCompactPrompt.busy) {
        manualCompactPrompt = null;
      }
      return;
    }

    manualCompactPrompt = {
      sessionId,
      message,
      busy: manualCompactPrompt?.sessionId === sessionId ? manualCompactPrompt.busy : false
    };
  }

  function showManualCompactPromptFromError(error: unknown, fallbackSessionId: string | null = null) {
    if (!isContextWindowExceededPayload(error)) {
      return false;
    }
    const parsed = parseAppError(error);
    const sessionId = parsed?.sessionId ?? fallbackSessionId;
    if (!sessionId || dismissedManualCompactPromptForSessionId === sessionId) {
      return false;
    }
    manualCompactPrompt = {
      sessionId,
      message: parsed?.message ?? describeUiError(error) ?? ui.manualCompactDescription,
      busy: manualCompactPrompt?.sessionId === sessionId ? manualCompactPrompt.busy : false
    };
    return true;
  }

  function dismissManualCompactPrompt() {
    if (manualCompactPrompt?.sessionId) {
      dismissedManualCompactPromptForSessionId = manualCompactPrompt.sessionId;
    }
    manualCompactPrompt = null;
  }

  async function startManualCompactFromPrompt() {
    const prompt = manualCompactPrompt;
    if (!prompt || prompt.busy || readOnlyRole) {
      return;
    }

    manualCompactPrompt = {
      ...prompt,
      busy: true
    };

    try {
      await api.startSessionCompact(prompt.sessionId, profileIdForSession(prompt.sessionId));
      noticeText = ui.manualCompactStarted;
      dismissedManualCompactPromptForSessionId = null;
      manualCompactPrompt = null;
      if (selectedSessionId === prompt.sessionId) {
        scheduleSelectedSessionStateRefresh(prompt.sessionId, 450);
      }
    } catch (error) {
      errorText = describeError(error);
      if (manualCompactPrompt?.sessionId === prompt.sessionId) {
        manualCompactPrompt = {
          ...manualCompactPrompt,
          busy: false
        };
      }
    }
  }

  function dismissSessionRecoveryPrompt() {
    if (sessionRecoveryPrompt?.sessionId) {
      dismissedSessionRecoveryPromptForSessionId = sessionRecoveryPrompt.sessionId;
    }
    sessionRecoveryPrompt = null;
  }

  function getSessionRecoveryIssueLabel(issue: string | null) {
    if (issue === "invalidUtf8") {
      return m.session_history_recovery_issue_invalid_utf8();
    }
    if (issue === "invalidJson") {
      return m.session_history_recovery_issue_invalid_json();
    }
    return m.session_history_recovery_issue_generic();
  }

  async function recoverSessionHistoryPrompt() {
    const prompt = sessionRecoveryPrompt;
    if (!prompt || prompt.busy) {
      return;
    }

    sessionRecoveryPrompt = {
      ...prompt,
      busy: true
    };

    try {
      const payload: SessionRolloutRecoveryPayload = await api.recoverSessionRollout(
        prompt.sessionId,
        profileIdForSession(prompt.sessionId)
      );
      noticeText = m.session_history_recovery_success({
        recovered: String(payload.recoveredLines),
        skipped: String(payload.skippedLines)
      });
      dismissedSessionRecoveryPromptForSessionId = null;
      sessionRecoveryPrompt = null;

      if (selectedSessionId === prompt.sessionId) {
        await refreshSelectedSessionState(
          prompt.sessionId,
          Math.max(conversation?.thread.turns.length ?? 0, olderTurnPageSize),
          true
        );
      }
    } catch (error) {
      errorText = describeError(error);
      if (sessionRecoveryPrompt?.sessionId === prompt.sessionId) {
        sessionRecoveryPrompt = {
          ...sessionRecoveryPrompt,
          busy: false
        };
      }
    }
  }

  async function refreshSelectedSessionState(
    sessionId: string,
    turnLimit: number,
    loadDraft = false,
    knownVersion: string | null = sessionDetailCacheVersion,
    replaceWithRecentWindow = false,
    selectionVersion: number | null = null,
    expectedProfileId: string | null = null
  ) {
    const requestSelectionVersion = selectionVersion ?? sessionSelectionVersion;
    const sessionProfileId = expectedProfileId ?? profileIdForSession(sessionId);
    const scopeKey = sessionStateKey(sessionId, sessionProfileId);
    const requestEventRevision = sessionEventRevisions.get(scopeKey) ?? 0;
    const requestStreamCursor = sessionStreamCursors.get(scopeKey) ?? null;
    const knownTurnVersions =
      knownVersion && sessionDetailStateHash && conversation?.thread.id === sessionId
        ? boundedVersionHints(sessionTurnVersionsById, SESSION_DETAIL_VERSION_HINT_LIMIT)
        : null;
    const knownStateHash =
      knownVersion && conversation?.thread.id === sessionId ? sessionDetailStateHash : null;
    const detail = await api.getSession(
      sessionId,
      turnLimit,
      knownVersion,
      knownTurnVersions,
      knownStateHash,
      sessionProfileId
    );
    if (
      selectedSessionId !== sessionId ||
      selectedSessionProfileId !== sessionProfileId ||
      sessionSelectionVersion !== requestSelectionVersion
    ) {
      return null;
    }
    reconcileSelectedSessionStreamBoundary(
      scopeKey,
      sessionId,
      sessionProfileId,
      requestStreamCursor,
      detail
    );
    if (isCacheValidationResponse(detail)) {
      sessionDetailCacheVersion = detail.cacheVersion;
      if (selectedSessionId !== sessionId || !conversation || conversation.thread.id !== sessionId) {
        return null;
      }
      const pendingEventsBeforeValidation = pendingSessionEvents[scopeKey] ?? [];
      const hadPendingQueueUpdate = pendingEventsBeforeValidation.some(isQueueUpdatedEvent);
      let nextConversation = conversation;
      if (pendingEventsBeforeValidation.length > 0) {
        nextConversation = normalizeConversationExecutionState(
          flushPendingSessionEvents(sessionId, sessionProfileId, nextConversation)
        );
        conversation = nextConversation;
        const cachePersistMode = pendingEventsBeforeValidation.some(
          (event) => sessionCacheModeForStreamEvent(event) === "terminal"
        )
          ? "terminal"
          : "stream";
        markConversationCacheDirty(cachePersistMode);
        if (hadPendingQueueUpdate) {
          applyQueueUpdatedSideEffects(sessionId, sessionProfileId, nextConversation);
          applySessionSummaryUpdate(buildSessionSummaryFromConversation(nextConversation));
        }
      }
      if (loadDraft) {
        await loadSavedDraft(
          sessionId,
          nextConversation.activeTurnId,
          nextConversation.preferences.steeringResumeMode,
          sessionProfileId,
          requestSelectionVersion
        );
      }
      clearHydrationRefresh();
      clearStaleSessionCatchup();
      updateSessionRecoveryPrompt(sessionId, nextConversation);
      preserveTranscriptScrollAfterDataUpdate(isInitialTranscriptScrollPending(sessionId) || replaceWithRecentWindow);
      return nextConversation;
    }

    if (isSessionDetailPatchResponse(detail)) {
      const patchedDetail = applySessionDetailPatch(detail);
      const fallbackRequestStreamCursor = sessionStreamCursors.get(scopeKey) ?? null;
      const fallbackDetail = patchedDetail
        ? null
        : await api.getSession(sessionId, turnLimit, null, null, null, sessionProfileId);
      if (
        selectedSessionId !== sessionId ||
        selectedSessionProfileId !== sessionProfileId ||
        sessionSelectionVersion !== requestSelectionVersion
      ) {
        return null;
      }
      if (fallbackDetail) {
        reconcileSelectedSessionStreamBoundary(
          scopeKey,
          sessionId,
          sessionProfileId,
          fallbackRequestStreamCursor,
          fallbackDetail
        );
      }
      const nextDetail = patchedDetail ?? fallbackDetail;
      if (!nextDetail || isCacheValidationResponse(nextDetail) || isSessionDetailPatchResponse(nextDetail)) {
        return null;
      }
      if (
        selectedSessionId !== nextDetail.thread.id ||
        (nextDetail.profileId && nextDetail.profileId !== sessionProfileId)
      ) {
        return null;
      }

      const skippedEventCount =
        replaceWithRecentWindow && staleSessionCatchup?.sessionId === nextDetail.thread.id
          ? staleSessionCatchup.eventCount
          : 0;
      if (nextDetail.profileId) {
        selectedSessionProfileId = nextDetail.profileId;
        rememberSessionProfile({ id: nextDetail.thread.id, profileId: nextDetail.profileId });
      }
      const streamedStateAdvanced = (sessionEventRevisions.get(scopeKey) ?? 0) > requestEventRevision;
      const preserveStreamedState =
        streamedStateAdvanced || (replaceWithRecentWindow && skippedEventCount > 0);
      const nextConversation = applyLoadedSessionDetail(
        nextDetail.thread.id,
        sessionProfileId,
        nextDetail,
        !replaceWithRecentWindow,
        preserveStreamedState,
        replaceWithRecentWindow
      );
      const detailCacheKey = buildSessionDetailBrowserCacheKey(nextDetail.thread.id, sessionProfileId);
      if (detailCacheKey) {
        void writeSessionDetailCache(
          detailCacheKey,
          conversationToSessionDetailPayload(nextConversation),
          preserveStreamedState ? null : nextDetail.cacheVersion
        );
      }
      if (
        replaceWithRecentWindow &&
        staleSessionCatchup?.sessionId === nextDetail.thread.id
      ) {
        if (skippedEventCount > 0 && staleSessionCatchup.refreshRetries < 1) {
          staleSessionCatchup = {
            ...staleSessionCatchup,
            eventCount: 0,
            refreshRetries: staleSessionCatchup.refreshRetries + 1
          };
          scheduleSelectedSessionStateRefresh(nextDetail.thread.id, 2_000, true);
        } else {
          clearStaleSessionCatchup();
        }
      }
      if (loadDraft) {
        await loadSavedDraft(
          nextDetail.thread.id,
          nextConversation.activeTurnId,
          nextConversation.preferences.steeringResumeMode,
          sessionProfileId,
          requestSelectionVersion
        );
      }
      preserveTranscriptScrollAfterDataUpdate(isInitialTranscriptScrollPending(nextDetail.thread.id) || replaceWithRecentWindow);
      return nextConversation;
    }

    if (
      selectedSessionId !== detail.thread.id ||
      (detail.profileId && detail.profileId !== sessionProfileId)
    ) {
      return null;
    }

    const skippedEventCount =
      replaceWithRecentWindow && staleSessionCatchup?.sessionId === detail.thread.id
        ? staleSessionCatchup.eventCount
        : 0;
    if (detail.profileId) {
      selectedSessionProfileId = detail.profileId;
      rememberSessionProfile({ id: detail.thread.id, profileId: detail.profileId });
    }
    const streamedStateAdvanced = (sessionEventRevisions.get(scopeKey) ?? 0) > requestEventRevision;
    const preserveStreamedState =
      streamedStateAdvanced || (replaceWithRecentWindow && skippedEventCount > 0);
    const nextConversation = applyLoadedSessionDetail(
      detail.thread.id,
      sessionProfileId,
      detail,
      !replaceWithRecentWindow,
      preserveStreamedState,
      replaceWithRecentWindow
    );
    const detailCacheKey = buildSessionDetailBrowserCacheKey(detail.thread.id, sessionProfileId);
    if (detailCacheKey) {
      void writeSessionDetailCache(
        detailCacheKey,
        conversationToSessionDetailPayload(nextConversation),
        preserveStreamedState ? null : detail.cacheVersion
      );
    }
    if (loadDraft) {
      await loadSavedDraft(
        detail.thread.id,
        nextConversation.activeTurnId,
        nextConversation.preferences.steeringResumeMode,
        sessionProfileId,
        requestSelectionVersion
      );
    }
    if (
      replaceWithRecentWindow &&
      staleSessionCatchup?.sessionId === detail.thread.id
    ) {
      if (skippedEventCount > 0 && staleSessionCatchup.refreshRetries < 1) {
        staleSessionCatchup = {
          ...staleSessionCatchup,
          eventCount: 0,
          refreshRetries: staleSessionCatchup.refreshRetries + 1
        };
        scheduleSelectedSessionStateRefresh(detail.thread.id, 2_000, true);
      } else {
        clearStaleSessionCatchup();
      }
    }
    preserveTranscriptScrollAfterDataUpdate(isInitialTranscriptScrollPending(detail.thread.id) || replaceWithRecentWindow);
    return nextConversation;
  }

  async function recoverFromReconnect() {
    if (authenticated !== true) {
      return;
    }

    ensureGlobalStreamSubscription();
    void refreshRuntimeStatus(false, { silent: true });
    if (activeWorkspaceTabId.startsWith("terminal:")) {
      void refreshTerminals();
    }
    recoverFromWebSocketResync();
  }

  function recoverFromWebSocketResync() {
    if (authenticated !== true) {
      return;
    }

    if (websocketResyncTimer) {
      clearTimeout(websocketResyncTimer);
    }
    websocketResyncTimer = setTimeout(() => {
      websocketResyncTimer = null;
      if (authenticated !== true) {
        return;
      }

      if (websocketResyncInFlight) {
        websocketResyncQueued = true;
        return;
      }

      websocketResyncInFlight = true;
      void (async () => {
        try {
          await refreshSessions(shouldPinSession(selectedSessionSummary) ? selectedSessionSummary : null);
          if (selectedSessionId) {
            await refreshSelectedSessionState(
              selectedSessionId,
              Math.max(conversation?.thread.turns.length ?? 0, olderTurnPageSize),
              true,
              sessionDetailCacheVersion,
              false
            );
          }
        } catch (error) {
          errorText = describeError(error);
        } finally {
          websocketResyncInFlight = false;
          if (websocketResyncQueued) {
            websocketResyncQueued = false;
            recoverFromWebSocketResync();
          }
        }
      })();
    }, 320);
  }

  function handleGlobalEvent(event: GlobalStreamEvent) {
    if (event.kind !== "notification") {
      return;
    }
    const eventProfileId =
      typeof event.params.profileId === "string" && event.params.profileId.trim()
        ? event.params.profileId.trim()
        : null;
    const eventMatchesSelectedProfile =
      !eventProfileId || !selectedSessionProfileId || eventProfileId === selectedSessionProfileId;

    if (event.method === "thread/goal/updated" || event.method === "thread/goal/cleared") {
      const goal = (event.params.goal ?? null) as SessionDetailPayload["goal"];
      const goalThreadId = goal?.threadId ?? "";
      const sessionId = String(event.params.threadId ?? event.params.thread_id ?? goalThreadId ?? "");
      if (sessionId && selectedSessionId === sessionId && eventMatchesSelectedProfile) {
        applyGoalPayloadToConversation(sessionId, event.method === "thread/goal/cleared" ? null : goal);
      }
      return;
    }

    if (event.method === "codex-webui/sessionAttention") {
      const sessionId = String(event.params.sessionId ?? "");
      const reason = String(event.params.reason ?? "");
      const requestId = typeof event.params.requestId === "string" ? event.params.requestId : null;
      if (!sessionId) {
        return;
      }
      notifyAttentionEvent(sessionId, reason, requestId, eventProfileId);
      return;
    }

    if (event.method === "codex-webui/sessionSummaryUpdated") {
      const incomingSummary = event.params.session as SessionSummary | undefined;
      const summary = incomingSummary
        ? {
            ...incomingSummary,
            profileId: incomingSummary.profileId ?? eventProfileId
          }
        : undefined;
      if (summary?.id) {
        let summaryForList = summary;
        const summaryMatchesSelectedProfile =
          !summary.profileId || !selectedSessionProfileId || summary.profileId === selectedSessionProfileId;
        if (summaryMatchesSelectedProfile && summary.status !== undefined && summary.status !== null) {
          if (isLiveConversationStatus(summary.status)) {
            noteRecentLiveSessionEvidence(summary.id);
          } else if (!shouldDeferTerminalSessionStatus(summary.id, summary.status)) {
            clearRecentLiveSessionEvidence(summary.id);
          }
        }
        if (
          summary.id === selectedSessionId &&
          summaryMatchesSelectedProfile &&
          conversation &&
          conversation.thread.id === summary.id
        ) {
          const conversationHasLiveTurn = hasConversationLiveTurn(conversation);
          const effectiveSummaryStatus = summary.status ?? conversation.thread.status;
          const shouldPreserveLiveStatus =
            (conversationHasLiveTurn || hasRecentLiveSessionEvidence(summary.id)) &&
            !isLiveConversationStatus(effectiveSummaryStatus);
          const nextStatus = shouldPreserveLiveStatus
            ? isLiveConversationStatus(conversation.thread.status)
              ? conversation.thread.status
              : "running"
            : effectiveSummaryStatus;
          const summaryUpdatedAt = normalizeSessionTimestamp(summary.updatedAt);
          const conversationUpdatedAt = normalizeSessionTimestamp(conversation.thread.updatedAt);
          const summaryIsNewer = summaryUpdatedAt > conversationUpdatedAt;
          if (shouldPreserveLiveStatus) {
            summaryForList = {
              ...summary,
              status: nextStatus,
              highlight:
                summary.highlight?.kind === "attention" &&
                shouldSuppressAttentionReason(
                  summary.id,
                  String(summary.highlight.reason ?? ""),
                  summary.profileId ?? null
                )
                  ? null
                  : summary.highlight
            };
            scheduleSelectedSessionStateRefresh(summary.id, recentLiveSessionEvidenceTtlMs + 250);
          }
          conversation = {
            ...conversation,
            activeTurnId: isLiveConversationStatus(nextStatus) ? conversation.activeTurnId : null,
            thread: {
              ...conversation.thread,
              name: summary.name ?? conversation.thread.name,
              preview: summary.preview ?? conversation.thread.preview,
              updatedAt: summaryIsNewer ? summary.updatedAt : conversation.thread.updatedAt,
              status: nextStatus
            }
          };
          markConversationCacheDirty();
          if (summaryIsNewer || !isLiveConversationStatus(nextStatus)) {
            const latestTurn = conversation.thread.turns.at(-1);
            const summaryClaimsTerminal =
              summary.status !== undefined &&
              summary.status !== null &&
              !isLiveConversationStatus(summary.status);
            scheduleSelectedSessionCompletionRefresh(
              summary.id,
              summaryIsNewer ? null : (latestTurn?.id ?? null),
              latestTurn ? (sessionTurnVersionsById[latestTurn.id] ?? null) : null,
              summaryIsNewer,
              summaryClaimsTerminal
            );
          }
        }
        applySessionSummaryUpdate(summaryForList);
      } else {
        scheduleSessionRefresh(60);
      }
      return;
    }

    if (event.method === "codex-webui/sessionListsInvalidated") {
      if (event.params.reason === "sessionProfileMoved") {
        const movedSessionId = String(event.params.sessionId ?? "");
        const sourceProfileId = String(event.params.sourceProfileId ?? "").trim();
        const targetProfileId = String(event.params.targetProfileId ?? "").trim();
        if (movedSessionId && sourceProfileId && targetProfileId) {
          rebindSelectedSessionProfile(
            movedSessionId,
            sourceProfileId,
            targetProfileId
          );
        }
      }
      scheduleSessionRefresh(60);
      return;
    }

    if (eventProfileId && activeProfileId && eventProfileId !== activeProfileId) {
      return;
    }

    if (event.method === "codex-webui/configUpdated") {
      if (config) {
        config = applyLocalComposerPreferencesToConfig({
          ...config,
          defaults: (event.params.defaults as SessionPreferences | undefined) ?? config.defaults,
          autostart:
            (event.params.autostart as AppConfigPayload["autostart"] | undefined) ?? config.autostart,
          systemShutdown:
            (event.params.systemShutdown as AppConfigPayload["systemShutdown"] | undefined) ?? config.systemShutdown,
          notifications: event.params.notifications
            ? {
                ...config.notifications,
                ...(event.params.notifications as Partial<AppConfigPayload["notifications"]>)
              }
            : config.notifications,
          sessionOrganization: event.params.sessionOrganization
            ? {
                ...config.sessionOrganization,
                ...(event.params.sessionOrganization as Partial<AppConfigPayload["sessionOrganization"]>)
              }
            : config.sessionOrganization,
          promptPresets: Array.isArray(event.params.promptPresets)
            ? (event.params.promptPresets as PromptPreset[])
            : config.promptPresets,
          automations: event.params.automations
            ? {
                ...config.automations,
                ...(event.params.automations as Partial<AppConfigPayload["automations"]>)
              }
            : config.automations,
          theme: event.params.theme ? (event.params.theme as ThemeSettings) : (config as ThemedConfigPayload).theme,
          startup: event.params.startup
            ? {
                ...config.startup,
                ...(event.params.startup as Partial<AppConfigPayload["startup"]>)
              }
            : config.startup
        } as ThemedConfigPayload);
        syncConfiguredTheme(config);
        syncStartupAlertModal(config);
      }
      return;
    }

    if (event.method === "codex-webui/notificationAdded") {
      const incoming = event.params.notification as AppNotification | undefined;
      if (incoming) {
        notifications = [incoming, ...notifications.filter((entry) => entry.id !== incoming.id)].slice(0, 80);
      }
      if (config) {
        config = {
          ...config,
          notifications: {
            ...config.notifications,
            unreadCount: Number(event.params.unreadCount ?? config.notifications.unreadCount)
          }
        };
      }
      return;
    }

    if (event.method === "codex-webui/notificationStateUpdated") {
      if (config) {
        config = {
          ...config,
          notifications: {
            ...config.notifications,
            unreadCount: Number(event.params.unreadCount ?? 0)
          }
        };
      }
      return;
    }

    if (event.method === "codex-webui/notificationSettingsUpdated") {
      if (config) {
        config = {
          ...config,
          notifications: {
            unreadCount: Number(event.params.unreadCount ?? config.notifications.unreadCount),
            settings: (event.params.settings as NotificationSettings | undefined) ?? config.notifications.settings
          }
        };
      }
      return;
    }

    if (event.method === "codex-webui/shutdownScheduled") {
      noticeText = ui.shutdownScheduledNotice(Number(event.params.delaySeconds ?? config?.systemShutdown.delaySeconds ?? 0));
      if (config) {
        config = {
          ...config,
          startup: {
            ...config.startup,
            scheduledShutdown: {
              sessionId: typeof event.params.sessionId === "string" ? String(event.params.sessionId) : null,
              scheduledFor: Number(event.params.scheduledFor ?? Date.now()),
              delaySeconds: Number(event.params.delaySeconds ?? config.systemShutdown.delaySeconds)
            }
          }
        };
        syncStartupAlertModal(config, true);
      }
      return;
    }

    if (event.method === "codex-webui/shutdownFailed") {
      errorText = m.shutdown_failed({ message: String(event.params.message ?? m.unknown_error()) });
      if (config) {
        config = {
          ...config,
          startup: {
            ...config.startup,
            scheduledShutdown: null
          }
        };
        syncStartupAlertModal(config);
      }
      return;
    }

    if (event.method === "codex-webui/accountUpdated") {
      accountLoginFlow = null;
      void refreshAccountState(false);
      void refreshAccountQuotaSurfaces();
      return;
    }

    if (event.method === "codex-webui/accountLoginCompleted") {
      const loginId = String(event.params.loginId ?? "");
      const success = Boolean(event.params.success);
      const error = typeof event.params.error === "string" ? event.params.error : m.account_login_failed();

      if (accountLoginFlow?.loginId === loginId) {
        if (success) {
          accountLoginFlow = null;
          noticeText = m.account_updated_notice();
          void refreshAccountState(true);
          void refreshAccountQuotaSurfaces();
        } else {
          accountLoginFlow = {
            ...accountLoginFlow,
            busy: false,
            error
          };
        }
      }
      return;
    }

    if (event.method === "codex-webui/accountRateLimitsUpdated") {
      void refreshAccountQuotaSurfaces();
      return;
    }

    if (event.method === "codex-webui/remoteControlStatusChanged") {
      remoteControlStatus = {
        status: String(event.params.status ?? "disabled"),
        environmentId: typeof event.params.environmentId === "string" ? String(event.params.environmentId) : null,
        updatedAt: Date.now()
      };
      return;
    }

    if (event.method === "codex-webui/appListUpdated") {
      catalog = null;
      if (composerSettingsOpen || activeWorkspaceTabId === "settings") {
        void ensureCatalogLoaded();
      }
      return;
    }

    if (event.method === "codex-webui/terminalsUpdated") {
      terminals = Array.isArray(event.params.terminals) ? (event.params.terminals as TerminalSummary[]) : [];
      if (activeWorkspaceTabId.startsWith("terminal:")) {
        const terminalId = activeWorkspaceTabId.replace(/^terminal:/u, "");
        if (!terminals.some((terminal) => terminal.id === terminalId)) {
          activeWorkspaceTabId = "chat";
        }
      }
    }
  }

  $effect(() => {
    const selectedBinding = getSelectedSessionBinding();
    const sessionId = selectedBinding?.sessionId ?? null;
    const profileId = sessionId ? profileIdForSession(sessionId) : null;
    const scopeKey = sessionId ? sessionStateKey(sessionId, profileId) : null;
    const currentDraft = draft;
    const intent: "message" | "queue" = composerQueueModeActive ? "queue" : "message";
    const hasPendingSteerResume =
      pendingSteerResume &&
      scopeKey === sessionStateKey(pendingSteerResume.sessionId, pendingSteerResume.profileId) &&
      !currentDraft.trim();

    if (!sessionId || draftPersistencePaused || hasPendingSteerResume) {
      return;
    }

    if (draftSaveTimer) {
      clearTimeout(draftSaveTimer);
    }

    draftSaveTimer = setTimeout(async () => {
      draftSaveTimer = null;
      try {
        if (!scopeKey || selectedSessionStateKey() !== scopeKey) {
          return;
        }
        if (currentDraft.trim()) {
          await api.saveSessionDraft(sessionId, currentDraft, intent, profileId);
        } else {
          await api.clearSessionDraft(sessionId, profileId);
        }
      } catch (error) {
        if (scopeKey && selectedSessionStateKey() === scopeKey) {
          errorText = describeError(error);
        }
      }
    }, 260);

    return () => {
      if (draftSaveTimer) {
        clearTimeout(draftSaveTimer);
        draftSaveTimer = null;
      }
    };
  });

  function getResumeDraftKey(sessionId: string, profileId: string | null, updatedAt: number | null) {
    return `${sessionStateKey(sessionId, profileId)}:${updatedAt ?? 0}`;
  }

  async function loadSavedDraft(
    sessionId: string,
    activeTurnId: string | null,
    resumeMode: SessionPreferences["steeringResumeMode"],
    profileId: string | null,
    selectionVersion: number
  ) {
    draftPersistencePaused = true;

    try {
      const saved = await api.getSessionDraft(sessionId, profileId);
      if (
        selectedSessionId !== sessionId ||
        selectedSessionProfileId !== profileId ||
        sessionSelectionVersion !== selectionVersion
      ) {
        return;
      }

      const hasLocalComposerState = draft.length > 0 || draftAttachments.length > 0;
      if (hasLocalComposerState) {
        pendingSteerResume = null;
        return;
      }

      const savedDraft = saved.draft.trim();
      if (!savedDraft) {
        pendingSteerResume = null;
        draft = "";
        return;
      }

      const resumeKey = getResumeDraftKey(sessionId, profileId, saved.updatedAt);
      if (saved.intent === "steer" && activeTurnId) {
        if (resumeMode === "auto" && !handledResumeDraftKeys.has(resumeKey)) {
          handledResumeDraftKeys.add(resumeKey);
          draft = saved.draft;
          pendingSteerResume = null;
          await sendSteerPrompt(saved.draft, true);
          return;
        }

        if (!handledResumeDraftKeys.has(resumeKey)) {
          pendingSteerResume = {
            sessionId,
            profileId,
            draft: saved.draft,
            updatedAt: saved.updatedAt
          };
          draft = "";
          return;
        }
      }

      pendingSteerResume = null;
      draft = saved.draft;
    } catch (error) {
      if (
        selectedSessionId === sessionId &&
        selectedSessionProfileId === profileId &&
        sessionSelectionVersion === selectionVersion
      ) {
        errorText = describeError(error);
      }
    } finally {
      queueMicrotask(() => {
        if (
          selectedSessionId === sessionId &&
          selectedSessionProfileId === profileId &&
          sessionSelectionVersion === selectionVersion
        ) {
          draftPersistencePaused = false;
        }
      });
    }
  }

  function keepSavedDraftInComposer() {
    if (
      !pendingSteerResume ||
      !selectedSessionBindingMatches(pendingSteerResume.sessionId, pendingSteerResume.profileId)
    ) {
      return;
    }

    handledResumeDraftKeys.add(
      getResumeDraftKey(pendingSteerResume.sessionId, pendingSteerResume.profileId, pendingSteerResume.updatedAt)
    );
    draftPersistencePaused = true;
    draft = pendingSteerResume.draft;
    pendingSteerResume = null;
    queueMicrotask(() => {
      draftPersistencePaused = false;
    });
  }

  async function discardSavedDraft() {
    if (!selectedSessionId) {
      return;
    }

    const sessionId = selectedSessionId;
    const profileId = profileIdForSession(sessionId);
    const scopeKey = sessionStateKey(sessionId, profileId);
    try {
      await api.clearSessionDraft(sessionId, profileId);
      if (selectedSessionStateKey() !== scopeKey) {
        return;
      }
      pendingSteerResume = null;
      if (!draft.trim()) {
        draft = "";
      }
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function resumeSavedSteer() {
    if (
      !pendingSteerResume ||
      !selectedSessionBindingMatches(pendingSteerResume.sessionId, pendingSteerResume.profileId)
    ) {
      return;
    }

    handledResumeDraftKeys.add(
      getResumeDraftKey(pendingSteerResume.sessionId, pendingSteerResume.profileId, pendingSteerResume.updatedAt)
    );
    draft = pendingSteerResume.draft;
    const steerDraft = pendingSteerResume.draft;
    pendingSteerResume = null;
    await sendSteerPrompt(steerDraft, true);
  }

  function openMobileSidebar() {
    workspaceMenuOpen = false;
    composerSettingsOpen = false;
    resetSessionTurnSearch();
    mobileSidebarOpen = true;
  }

  function closeMobileSidebar() {
    mobileSidebarOpen = false;
  }

  async function createSession() {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    if (!runtime?.installed) {
      errorText = m.codex_cli_required();
      return;
    }
    if (!config) {
      return;
    }

    showArchivedSessions = false;
    sessionSearchQuery = "";
    sessionSearchScope = "summary";
    syncSessionListStateInUrl();
    mobileSidebarOpen = false;
    draftSelectedSkills = [];
    composerSkillQuery = "";
    activateDraftSession(config.defaults);
  }

  function isHundredMContextEnabled(preferences: SessionPreferences | null | undefined) {
    return preferences?.modelContextWindow === HUNDRED_M_CONTEXT_WINDOW;
  }

  function isPlanModeEnabled(preferences: SessionPreferences | null | undefined) {
    return (preferences?.mode ?? "default") === "plan";
  }

  function pendingPreferencePatchMatches(preferences: Partial<SessionPreferences>, patch: Partial<SessionPreferences>) {
    for (const [key, value] of Object.entries(patch) as Array<[keyof SessionPreferences, SessionPreferences[keyof SessionPreferences]]>) {
      if (preferences[key] !== value) {
        return false;
      }
    }
    return true;
  }

  function setPlanModePreference(enabled: boolean, showNotice = true) {
    setPreference("mode", enabled ? "plan" : "default");
    if (showNotice) {
      noticeText = enabled ? m.slash_plan_enabled() : m.slash_plan_disabled();
    }
  }

  function setPreferencesPatch(patch: Partial<SessionPreferences>) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    if (!conversation) {
      return;
    }

    if (typeof patch.sendOnEnter === "boolean") {
      persistLocalSendOnEnterPreference(patch.sendOnEnter);
      if (config) {
        config = {
          ...config,
          defaults: {
            ...config.defaults,
            sendOnEnter: patch.sendOnEnter
          }
        };
      }
    }

    const nextPreferences = {
      ...conversation.preferences,
      ...patch
    };
    conversation = {
      ...conversation,
      preferences: nextPreferences
    };
    const selectedBinding = getSelectedSessionBinding();
    if (selectedBinding) {
      pendingPreferencePatchesBySessionId.set(selectedBinding.sessionId, patch);
    }
    preferenceSaveVersion += 1;
    markConversationCacheDirty();
    if (selectedBinding || typeof patch.sendOnEnter !== "boolean" || Object.keys(patch).length > 1) {
      schedulePreferenceSave();
    }
  }

  function setPreference<Key extends keyof SessionPreferences>(key: Key, value: SessionPreferences[Key]) {
    setPreferencesPatch({
      [key]: value
    } as Partial<SessionPreferences>);
  }

  function setSpeedPreference(speed: SessionPreferences["speed"]) {
    setPreferencesPatch({
      speed,
      modelContextWindow:
        speed === "fast" && isHundredMContextEnabled(conversation?.preferences)
          ? null
          : (conversation?.preferences.modelContextWindow ?? null)
    });
  }

  function setHundredMContextEnabled(enabled: boolean) {
    setPreferencesPatch({
      modelContextWindow: enabled ? HUNDRED_M_CONTEXT_WINDOW : null,
      speed:
        enabled && (conversation?.preferences.speed ?? "auto") === "fast"
          ? "auto"
          : (conversation?.preferences.speed ?? "auto")
    });
  }

  function normalizeSelectedSkills(skills: Array<SelectedSkill | SkillCatalogEntry>) {
    const seen = new Set<string>();
    const normalized: SelectedSkill[] = [];
    for (const skill of skills) {
      const name = String(skill.name ?? "").trim();
      const path = String(skill.path ?? "").trim();
      if (!name || !path) {
        continue;
      }
      const key = `${name}\u0000${path}`;
      if (seen.has(key)) {
        continue;
      }
      seen.add(key);
      normalized.push({
        id: String(skill.id ?? path),
        name,
        path
      });
    }
    return normalized;
  }

  async function ensureCatalogLoaded() {
    if (catalog || catalogLoading) {
      return;
    }
    catalogLoading = true;
    try {
      catalog = await api.getCatalog();
    } catch (error) {
      errorText = describeError(error);
    } finally {
      catalogLoading = false;
    }
  }

  function setSelectedSkills(skills: Array<SelectedSkill | SkillCatalogEntry>) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }

    const nextSkills = normalizeSelectedSkills(skills);
    if (conversation && selectedSessionId) {
      conversation = {
        ...conversation,
        selectedSkills: nextSkills
      };
      markConversationCacheDirty();
      void api.saveSessionSkills(selectedSessionId, nextSkills, profileIdForSession(selectedSessionId)).catch((error) => {
        errorText = describeError(error);
      });
      return;
    }

    draftSelectedSkills = nextSkills;
  }

  function toggleComposerSkill(skill: SkillCatalogEntry) {
    const exists = composerSelectedSkills.some((entry) => entry.path === skill.path);
    if (exists) {
      setSelectedSkills(composerSelectedSkills.filter((entry) => entry.path !== skill.path));
      return;
    }
    setSelectedSkills([...composerSelectedSkills, skill]);
  }

  function schedulePreferenceSave() {
    const selectedBinding = getSelectedSessionBinding();
    if (!selectedBinding) {
      requestSelectedSessionResync(false);
      return;
    }
    const sessionId = selectedBinding.sessionId;
    const saveVersion = preferenceSaveVersion;
    if (saveTimer) {
      clearTimeout(saveTimer);
    }
    saveTimer = setTimeout(async () => {
      try {
        const currentBinding = getSelectedSessionBinding();
        if (!currentBinding || currentBinding.sessionId !== sessionId) {
          requestSelectedSessionResync(false);
          return;
        }
        const saved = await api.savePreferences(
          sessionId,
          currentBinding.state.preferences,
          profileIdForSession(sessionId)
        );
        if (saveVersion !== preferenceSaveVersion) {
          return;
        }
        const nextPreferences = applyLocalSendOnEnterPreference(saved);
        if (conversation?.thread.id === sessionId) {
          conversation = {
            ...conversation,
            preferences: nextPreferences
          };
          markConversationCacheDirty();
          applySessionSummaryUpdate(buildSessionSummaryFromConversation(conversation));
        }
        pendingPreferencePatchesBySessionId.delete(sessionId);
        if (config) {
          config = applyLocalComposerPreferencesToConfig({
            ...config,
            defaults: nextPreferences
          });
          syncConfiguredTheme(config);
        }
      } catch (error) {
        errorText = describeError(error);
      }
    }, 350);
  }

  async function saveSystemShutdownAfterQueueCompletes(armed: boolean) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    try {
      const nextConfig = applyLocalComposerPreferencesToConfig(await api.saveSystemShutdownAfterQueueCompletes(armed));
      config = nextConfig;
      syncConfiguredTheme(config);
      syncStartupAlertModal(config, Boolean(config.startup.scheduledShutdown));
    } catch (error) {
      errorText = describeError(error);
    }
  }

  function inferImageGenerationMimeType(item: CodexItem) {
    const savedPath =
      (typeof item.savedPath === "string" && item.savedPath.trim()) ||
      (typeof item.saved_path === "string" && item.saved_path.trim()) ||
      "";
    const loweredPath = savedPath.toLowerCase();
    if (loweredPath.endsWith(".jpg") || loweredPath.endsWith(".jpeg")) {
      return "image/jpeg";
    }
    if (loweredPath.endsWith(".webp")) {
      return "image/webp";
    }
    if (loweredPath.endsWith(".gif")) {
      return "image/gif";
    }
    return "image/png";
  }

  function getImageGenerationSource(item: CodexItem) {
    const result = typeof item.result === "string" ? item.result.trim() : "";
    if (!result) {
      return null;
    }
    if (result.startsWith("data:image/")) {
      return result;
    }
    return `data:${inferImageGenerationMimeType(item)};base64,${result.replace(/\s+/gu, "")}`;
  }

  function getImageGenerationPrompt(item: CodexItem) {
    return (
      (typeof item.revisedPrompt === "string" && item.revisedPrompt.trim()) ||
      (typeof item.revised_prompt === "string" && item.revised_prompt.trim()) ||
      null
    );
  }

  function getImageGenerationSavedPath(item: CodexItem) {
    return (
      (typeof item.savedPath === "string" && item.savedPath.trim()) ||
      (typeof item.saved_path === "string" && item.saved_path.trim()) ||
      null
    );
  }

  function getImageGenerationDownloadName(item: CodexItem) {
    const savedPath = getImageGenerationSavedPath(item);
    const fromPath = savedPath ? baseName(savedPath) : "";
    return fromPath || `${String(item.id || "generated-image")}.png`;
  }

  function getImageViewPath(item: CodexItem) {
    return (typeof item.path === "string" && item.path.trim()) || null;
  }

  function getRecordValue(value: unknown): Record<string, unknown> | null {
    return value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : null;
  }

  function getWebSearchAction(item: CodexItem) {
    return getRecordValue(item.action);
  }

  function getWebSearchActionType(item: CodexItem) {
    const action = getWebSearchAction(item);
    return typeof action?.type === "string" ? action.type : null;
  }

  function getWebSearchQueries(item: CodexItem) {
    const action = getWebSearchAction(item);
    const queries = Array.isArray(action?.queries) ? action.queries.map((query) => formatValue(query)).filter(Boolean) : [];
    const query = formatValue(action?.query) || formatValue(item.query);
    return query ? [query, ...queries.filter((candidate) => candidate !== query)] : queries;
  }

  function getWebSearchUrl(item: CodexItem) {
    const action = getWebSearchAction(item);
    return formatValue(action?.url);
  }

  function getWebSearchPattern(item: CodexItem) {
    const action = getWebSearchAction(item);
    return formatValue(action?.pattern);
  }

  function getWebSearchStatus(item: CodexItem) {
    return formatValue(item.status) || "completed";
  }

  type WebSearchReference = {
    title: string;
    url: string;
    snippet: string;
  };

  function normalizeWebSearchReferences(value: unknown): WebSearchReference[] {
    const entries = Array.isArray(value) ? value : value ? [value] : [];
    return entries
      .map((entry) => {
        const record = getRecordValue(entry);
        if (!record) {
          const text = formatValue(entry);
          return text ? { title: text, url: "", snippet: "" } : null;
        }
        const title =
          formatValue(record.title) ||
          formatValue(record.name) ||
          formatValue(record.source) ||
          formatValue(record.url) ||
          formatValue(record.link);
        const url = formatValue(record.url) || formatValue(record.link) || formatValue(record.href);
        const snippet =
          formatValue(record.snippet) ||
          formatValue(record.summary) ||
          formatValue(record.text) ||
          formatValue(record.content) ||
          formatValue(record.description);
        return title || url || snippet ? { title, url, snippet } : null;
      })
      .filter((entry): entry is WebSearchReference => Boolean(entry));
  }

  function getWebSearchSummary(item: CodexItem) {
    return formatValue(item.summary) || formatValue(item.resultSummary) || formatValue(item.result_summary);
  }

  function getWebSearchResults(item: CodexItem) {
    return normalizeWebSearchReferences(item.results ?? item.searchResults ?? item.search_results);
  }

  function getWebSearchSources(item: CodexItem) {
    const references = [
      ...normalizeWebSearchReferences(item.sources ?? item.sourceResults ?? item.source_results),
      ...normalizeWebSearchReferences(item.citations ?? item.citationResults ?? item.citation_results)
    ];
    return references.filter((entry, index) => {
      const key = `${entry.url}\n${entry.title}\n${entry.snippet}`;
      return references.findIndex((candidate) => `${candidate.url}\n${candidate.title}\n${candidate.snippet}` === key) === index;
    });
  }

  function getReviewText(item: CodexItem) {
    return formatValue(item.review) || (item.type === "enteredReviewMode" ? "Review requested." : "Review mode completed.");
  }

  function getReviewOutput(item: CodexItem) {
    return getRecordValue(item.reviewOutput ?? item.review_output);
  }

  function getReviewFindings(item: CodexItem) {
    const output = getReviewOutput(item);
    return Array.isArray(output?.findings)
      ? output.findings.filter((finding): finding is Record<string, unknown> => Boolean(getRecordValue(finding)))
      : [];
  }

  function getReviewFindingLocation(finding: Record<string, unknown>) {
    const location = getRecordValue(finding.code_location ?? finding.codeLocation);
    const lineRange = getRecordValue(location?.line_range ?? location?.lineRange);
    const path = getReviewFindingPath(finding);
    const start = formatValue(lineRange?.start);
    const end = formatValue(lineRange?.end);
    if (!path) {
      return "";
    }
    return start && end ? `${path}:${start}-${end}` : path;
  }

  function getReviewFindingPath(finding: Record<string, unknown>) {
    const location = getRecordValue(finding.code_location ?? finding.codeLocation);
    return formatValue(
      location?.absolute_file_path ??
        location?.absoluteFilePath ??
        location?.file_path ??
        location?.filePath ??
        location?.path
    );
  }

  async function saveAutostartEnabled(enabled: boolean) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    try {
      const nextConfig = applyLocalComposerPreferencesToConfig(await api.saveAutostartEnabled(enabled));
      config = nextConfig;
      syncConfiguredTheme(config);
      syncStartupAlertModal(config);
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function saveThemeSettings(theme: ThemeSettings) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    try {
      const nextConfig = applyLocalComposerPreferencesToConfig(await api.saveThemeSettings(theme));
      config = nextConfig;
      syncConfiguredTheme(config);
      noticeText = m.theme_saved();
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function saveDefaultLanguageBridgeDefaults(enabled: boolean, outputLanguage: string | null = null) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    if (!config || defaultLanguageBridgeBusy) {
      return;
    }

    defaultLanguageBridgeBusy = true;
    try {
      const nextConfig = applyLocalComposerPreferencesToConfig(
        await api.saveDefaultSessionPreferences({
          languageBridgeEnabled: enabled,
          languageBridgeOutputLanguage: outputLanguage?.trim() || config.defaults.languageBridgeOutputLanguage || "auto"
        })
      );
      config = nextConfig;
      syncConfiguredTheme(config);
      if (!selectedSessionId && conversation) {
        conversation = {
          ...conversation,
          preferences: {
            ...conversation.preferences,
            languageBridgeEnabled: nextConfig.defaults.languageBridgeEnabled,
            languageBridgeOutputLanguage: nextConfig.defaults.languageBridgeOutputLanguage
          }
        };
        markConversationCacheDirty();
      }
      noticeText = enabled ? m.default_language_bridge_enabled_notice() : m.default_language_bridge_disabled_notice();
    } catch (error) {
      errorText = describeError(error);
    } finally {
      defaultLanguageBridgeBusy = false;
    }
  }

  async function saveTitle() {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    if (!titleDraft.trim()) {
      return;
    }

    const nextTitle = titleDraft.trim();
    const currentDisplayTitle = getConversationDisplayTitle(conversation) ?? "";
    if (nextTitle === currentDisplayTitle) {
      return;
    }

    if (!selectedSessionId) {
      if (conversation) {
        conversation = {
          ...conversation,
          thread: {
            ...conversation.thread,
            name: nextTitle
          }
        };
        markConversationCacheDirty();
      }
      titleDraft = nextTitle;
      return;
    }

    try {
      await api.renameSession(selectedSessionId, nextTitle, profileIdForSession(selectedSessionId));
      const archived = selectedSessionSummary?.archived ?? showArchivedSessions;
      upsertSessionSummary({
        id: selectedSessionId,
        name: nextTitle,
        preview: conversation?.thread.preview ?? "",
        queueCount: conversation?.queue.items.length ?? selectedSessionSummary?.queueCount ?? 0,
        highlight: selectedSessionSummary?.highlight ?? null,
        pinned: selectedSessionSummary?.pinned ?? false,
        tags: [...(selectedSessionSummary?.tags ?? [])],
        cwd: conversation?.thread.cwd ?? config?.defaults.cwd ?? "",
        archived,
        createdAt: normalizeSessionTimestamp(conversation?.thread.createdAt ?? Date.now()),
        updatedAt: Date.now(),
        status: conversation?.thread.status ?? "unknown",
        isSubagent: conversation?.thread.isSubagent ?? false,
        agentNickname: conversation?.thread.agentNickname ?? null,
        agentRole: conversation?.thread.agentRole ?? null,
        preferences: conversation?.preferences ?? null
      });

      if (conversation) {
        conversation = {
          ...conversation,
          thread: {
            ...conversation.thread,
            name: nextTitle
          }
        };
        markConversationCacheDirty();
      }
      scheduleSessionRefresh(80);
    } catch (error) {
      errorText = describeError(error);
    }
  }

  function findPromptPresetByName(value: string) {
    const normalized = value.trim().toLowerCase();
    if (!normalized) {
      return null;
    }

    return (
      [...(config?.promptPresets ?? [])]
        .sort((left, right) => right.name.length - left.name.length)
        .find((preset) => preset.name.trim().toLowerCase() === normalized) ?? null
    );
  }

  function applyGoalPayloadToConversation(sessionId: string, goal: SessionDetailPayload["goal"]) {
    if (!conversation || conversation.thread.id !== sessionId) {
      return;
    }
    conversation = {
      ...conversation,
      goal
    };
    markConversationCacheDirty();
  }

  function formatGoalSummary(goal: SessionDetailPayload["goal"]) {
    if (!goal) {
      return m.slash_goal_none();
    }
    return m.slash_goal_summary({
      objective: goal.objective,
      status: goal.status,
      tokensUsed: String(goal.tokensUsed ?? 0),
      tokenBudget: goal.tokenBudget === null ? "∞" : String(goal.tokenBudget)
    });
  }

  function goalPrimaryAction(status: string | null | undefined): "pause" | "resume" | null {
    if (status === "active") {
      return "pause";
    }
    if (status === "paused" || status === "blocked" || status === "usageLimited") {
      return "resume";
    }
    return null;
  }

  function goalPrimaryActionLabel(status: string | null | undefined) {
    return goalPrimaryAction(status) === "resume" ? ui.resume : "Pause";
  }

  async function handleGoalPrimaryAction(status: string | null | undefined) {
    const action = goalPrimaryAction(status);
    if (!action) {
      return;
    }
    await handleGoalSlashCommand(action);
  }

  async function handleGoalSlashCommand(args: string) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }

    const normalized = args.trim().toLowerCase();
    if (!args.trim()) {
      const selectedBinding = getSelectedSessionBinding();
      if (!selectedBinding) {
        noticeText = m.slash_goal_none();
        draft = "";
        scheduleComposerTextareaResize();
        return;
      }
      const response = await api.getSessionGoal(selectedBinding.sessionId, profileIdForSession(selectedBinding.sessionId));
      applyGoalPayloadToConversation(selectedBinding.sessionId, response.goal);
      noticeText = formatGoalSummary(response.goal);
      draft = "";
      scheduleComposerTextareaResize();
      return;
    }

    if (normalized === "clear" || normalized === "delete" || normalized === "remove") {
      const selectedBinding = ensureSelectedSessionBinding();
      if (!selectedBinding) {
        return;
      }
      const response = await api.clearSessionGoal(selectedBinding.sessionId, profileIdForSession(selectedBinding.sessionId));
      applyGoalPayloadToConversation(selectedBinding.sessionId, response.goal);
      noticeText = m.slash_goal_cleared();
      draft = "";
      scheduleComposerTextareaResize();
      return;
    }

    const materialized = await ensureSessionForComposer();
    if (!materialized) {
      return;
    }

    const materializedProfileId = profileIdForSession(materialized.sessionId);
    const response =
      normalized === "pause" || normalized === "paused"
        ? await api.setSessionGoal(materialized.sessionId, { status: "paused" }, materializedProfileId)
        : normalized === "resume" || normalized === "active"
          ? await api.setSessionGoal(materialized.sessionId, { status: "active" }, materializedProfileId)
          : normalized === "block" || normalized === "blocked"
            ? await api.setSessionGoal(materialized.sessionId, { status: "blocked" }, materializedProfileId)
            : normalized === "usage-limited" || normalized === "usage_limited" || normalized === "usagelimited"
              ? await api.setSessionGoal(materialized.sessionId, { status: "usageLimited" }, materializedProfileId)
              : normalized === "budget-limited" || normalized === "budget_limited" || normalized === "budgetlimited"
                ? await api.setSessionGoal(materialized.sessionId, { status: "budgetLimited" }, materializedProfileId)
                : normalized === "complete" || normalized === "completed"
                  ? await api.setSessionGoal(materialized.sessionId, { status: "complete" }, materializedProfileId)
                  : await api.setSessionGoal(materialized.sessionId, { objective: args.trim(), status: "active" }, materializedProfileId);
    applyGoalPayloadToConversation(materialized.sessionId, response.goal);
    if (normalized === "pause" || normalized === "paused") {
      noticeText = m.slash_goal_paused();
    } else if (normalized === "resume" || normalized === "active") {
      noticeText = m.slash_goal_resumed();
    } else {
      noticeText = response.goal ? formatGoalSummary(response.goal) : m.slash_goal_updated();
    }
    draft = "";
    scheduleComposerTextareaResize();
  }

  function parseReviewSlashTarget(args: string): { target: SessionReviewTarget; delivery: "inline" | "detached" } {
    const detachedPattern = /(?:^|\s)--detached(?:\s|$)/u;
    const delivery = detachedPattern.test(args) ? "detached" : "inline";
    const cleaned = args.replace(detachedPattern, " ").trim();
    const baseMatch = cleaned.match(/^(?:base|base-branch|branch)\s+(.+)$/iu);
    if (baseMatch?.[1]?.trim()) {
      return {
        delivery,
        target: {
          type: "baseBranch",
          branch: baseMatch[1].trim()
        }
      };
    }
    const commitMatch = cleaned.match(/^commit\s+([^\s]+)(?:\s+(.+))?$/iu);
    if (commitMatch?.[1]?.trim()) {
      return {
        delivery,
        target: {
          type: "commit",
          sha: commitMatch[1].trim(),
          title: commitMatch[2]?.trim() || null
        }
      };
    }
    if (cleaned) {
      return {
        delivery,
        target: {
          type: "custom",
          instructions: cleaned
        }
      };
    }
    return {
      delivery,
      target: {
        type: "uncommittedChanges"
      }
    };
  }

  async function startReviewFromSlash(args: string) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    const materialized = await ensureSessionForComposer();
    if (!materialized) {
      return;
    }
    const { target, delivery } = parseReviewSlashTarget(args);
    const response = await api.startReview(
      materialized.sessionId,
      { target, delivery },
      profileIdForSession(materialized.sessionId)
    );
    draft = "";
    scheduleComposerTextareaResize();
    scheduleSessionRefresh(80);
    scheduleSelectedSessionStateRefresh(response.reviewThreadId, 80);
    if (response.reviewThreadId !== materialized.sessionId) {
      await selectSession(response.reviewThreadId);
    }
    noticeText = m.review_started();
  }

  async function handleSlashCommand(rawValue: string) {
    const trimmed = rawValue.trim();
    if (!trimmed.startsWith("/")) {
      return false;
    }

    const match = trimmed.match(/^\/([^\s]+)\s*(.*)$/u);
    if (!match) {
      return false;
    }

    const command = match[1]?.toLowerCase() ?? "";
    const args = match[2]?.trim() ?? "";

    if (command === "goal") {
      try {
        await handleGoalSlashCommand(args);
      } catch (error) {
        errorText = describeError(error);
      }
      return true;
    }

    if (command === "review") {
      try {
        await startReviewFromSlash(args);
      } catch (error) {
        errorText = describeError(error);
      }
      return true;
    }

    if (command === "compact") {
      if (readOnlyRole) {
        errorText = m.error_forbidden_role();
        return true;
      }
      const selectedBinding = ensureSelectedSessionBinding();
      if (!selectedBinding) {
        return true;
      }
      try {
        await api.startSessionCompact(selectedBinding.sessionId, profileIdForSession(selectedBinding.sessionId));
        noticeText = ui.manualCompactStarted;
        dismissedManualCompactPromptForSessionId = null;
        if (manualCompactPrompt?.sessionId === selectedBinding.sessionId) {
          manualCompactPrompt = null;
        }
        scheduleSelectedSessionStateRefresh(selectedBinding.sessionId, 450);
      } catch (error) {
        errorText = describeError(error);
      } finally {
        draft = "";
        scheduleComposerTextareaResize();
      }
      return true;
    }

    if (command === "fast") {
      if (readOnlyRole) {
        errorText = m.error_forbidden_role();
        return true;
      }
      const normalized = args.toLowerCase();
      if (!normalized) {
        const nextSpeed = conversation?.preferences.speed === "fast" ? "auto" : "fast";
        setSpeedPreference(nextSpeed);
        draft = "";
        scheduleComposerTextareaResize();
        noticeText = nextSpeed === "fast" ? m.slash_fast_enabled() : m.slash_fast_disabled();
        return true;
      }
      if (["on", "fast", "true", "1"].includes(normalized)) {
        setSpeedPreference("fast");
        draft = "";
        scheduleComposerTextareaResize();
        noticeText = m.slash_fast_enabled();
        return true;
      }
      if (["off", "auto", "false", "0"].includes(normalized)) {
        setSpeedPreference("auto");
        draft = "";
        scheduleComposerTextareaResize();
        noticeText = m.slash_fast_disabled();
        return true;
      }
      if (normalized === "flex") {
        setSpeedPreference("flex");
        draft = "";
        scheduleComposerTextareaResize();
        noticeText = m.speed_flex();
        return true;
      }
      errorText = m.slash_fast_invalid();
      return true;
    }

    if (command === "rename" || command === "title") {
      if (readOnlyRole) {
        errorText = m.error_forbidden_role();
        return true;
      }
      if (!args) {
        errorText = m.slash_argument_required({ command: `/${command}` });
        return true;
      }
      titleDraft = args;
      await saveTitle();
      draft = "";
      scheduleComposerTextareaResize();
      return true;
    }

    if (command === "new" || command === "clear") {
      await createSession();
      draft = "";
      scheduleComposerTextareaResize();
      return true;
    }

    if (command === "queue") {
      if (!args) {
        errorText = m.slash_argument_required({ command: "/queue" });
        return true;
      }
      await queueMessage({ promptText: args, attachmentSnapshot: [], preserveComposer: false });
      return true;
    }

    if (command === "steer") {
      if (!args) {
        errorText = m.slash_argument_required({ command: "/steer" });
        return true;
      }
      await sendSteerPrompt(args, false);
      return true;
    }

    if (command === "preset") {
      if (!args) {
        errorText = m.slash_argument_required({ command: "/preset" });
        return true;
      }
      const preset = findPromptPresetByName(args);
      if (!preset) {
        errorText = m.slash_preset_not_found();
        return true;
      }
      draft = preset.prompt;
      draftAttachments = [];
      scheduleComposerTextareaResize();
      composerTextareaElement?.focus();
      noticeText = m.prompt_preset_applied({ name: preset.name });
      return true;
    }

    if (command === "model") {
      if (readOnlyRole) {
        errorText = m.error_forbidden_role();
        return true;
      }
      if (!args) {
        errorText = m.slash_argument_required({ command: "/model" });
        return true;
      }
      if (!config?.models.some((model) => model.id === args)) {
        errorText = m.slash_model_not_found();
        return true;
      }
      setPreference("model", args);
      draft = "";
      scheduleComposerTextareaResize();
      noticeText = m.slash_model_updated({ model: args });
      return true;
    }

    if (command === "personality") {
      if (readOnlyRole) {
        errorText = m.error_forbidden_role();
        return true;
      }
      if (!args) {
        errorText = m.slash_argument_required({ command: "/personality" });
        return true;
      }
      const normalized = args.toLowerCase();
      if (normalized !== "friendly" && normalized !== "pragmatic" && normalized !== "none") {
        errorText = ui.slashPersonalityInvalid;
        return true;
      }
      setPreference("personality", normalized as SessionPreferences["personality"]);
      draft = "";
      scheduleComposerTextareaResize();
      noticeText = ui.slashPersonalityUpdated(getPersonalityOptionLabel(normalized as SessionPreferences["personality"]));
      return true;
    }

    if (command === "plan") {
      if (readOnlyRole) {
        errorText = m.error_forbidden_role();
        return true;
      }
      if (!args) {
        errorText = m.slash_argument_required({ command: "/plan" });
        return true;
      }
      const normalized = args.toLowerCase();
      if (normalized === "plan" || normalized === "on") {
        setPlanModePreference(true, false);
        draft = "";
        scheduleComposerTextareaResize();
        noticeText = m.slash_plan_enabled();
        return true;
      }
      if (normalized === "default" || normalized === "off") {
        setPlanModePreference(false, false);
        draft = "";
        scheduleComposerTextareaResize();
        noticeText = m.slash_plan_disabled();
        return true;
      }
      errorText = m.slash_plan_invalid();
      return true;
    }

    if (command === "realtime") {
      if (readOnlyRole) {
        errorText = m.error_forbidden_role();
        return true;
      }
      const selectedBinding = ensureSelectedSessionBinding();
      if (!selectedBinding) {
        return true;
      }
      const sessionProfileId = profileIdForSession(selectedBinding.sessionId);
      const normalized = args.toLowerCase();
      try {
        if (["stop", "off", "end"].includes(normalized)) {
          await api.stopRealtimeSession(selectedBinding.sessionId, sessionProfileId);
          noticeText = m.realtime_stopped();
        } else if (normalized === "voices") {
          const response = await api.listRealtimeVoices();
          const voices = Array.isArray(response.voices)
            ? response.voices.map((voice) => String(voice)).join(", ")
            : JSON.stringify(response.voices);
          noticeText = voices ? m.realtime_voices({ voices }) : m.realtime_no_voices();
        } else {
          await api.startRealtimeSession(selectedBinding.sessionId, {
            outputModality: "text",
            prompt: args ? args : null,
            profileId: sessionProfileId
          });
          noticeText = m.realtime_started();
        }
        draft = "";
        scheduleComposerTextareaResize();
      } catch (error) {
        errorText = describeError(error);
      }
      return true;
    }

    if (command === "apps") {
      try {
        const response = await api.listCodexApps({
          limit: 20,
          threadId: selectedSessionId ?? null,
          profileId: selectedSessionId ? profileIdForSession(selectedSessionId) : null
        });
        const names = response.data.map((app) => app.name).filter(Boolean);
        noticeText = names.length > 0 ? `${m.apps()}: ${names.slice(0, 8).join(", ")}` : m.no_apps();
        openSettingsTab("apps");
        draft = "";
        scheduleComposerTextareaResize();
      } catch (error) {
        errorText = describeError(error);
      }
      return true;
    }

    if (command === "plugins") {
      await ensureCatalogLoaded();
      const names = (catalog?.plugins ?? []).map((plugin) => plugin.displayName || plugin.name).filter(Boolean);
      noticeText = names.length > 0 ? `Plugins: ${names.slice(0, 8).join(", ")}` : "No plugins returned.";
      openSettingsTab("plugins");
      draft = "";
      scheduleComposerTextareaResize();
      return true;
    }

    if (command === "mcp") {
      try {
        const response = await api.listMcpServers({ detail: "toolsAndAuthOnly", limit: 20 });
        const names = response.data.map((server) => server.name).filter(Boolean);
        noticeText = names.length > 0 ? `MCP: ${names.slice(0, 8).join(", ")}` : "No MCP servers returned.";
        openSettingsTab("mcp");
        draft = "";
        scheduleComposerTextareaResize();
      } catch (error) {
        errorText = describeError(error);
      }
      return true;
    }

    const knownCommand = findCodexSlashCommand(command);
    if (knownCommand) {
      errorText = m.slash_command_not_supported({
        command: `/${knownCommand.command}`,
        support: knownCommand.support
      });
      return true;
    }

    errorText = m.slash_unknown_command();
    return true;
  }

  async function sendMessage(options?: {
    promptText?: string;
    attachmentSnapshot?: AttachmentRecord[];
    preserveComposer?: boolean;
  }) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    if (!conversation || sending || startingMessage || uploading || queueModeActive) {
      return;
    }

    const draftText = options?.promptText ?? draft;
    const attachmentSnapshot = options?.attachmentSnapshot ?? [...draftAttachments];
    const prompt = draftText.trim();
    const attachmentNames = attachmentSnapshot.map((attachment) => attachment.originalName);
    const preserveComposer = options?.preserveComposer ?? false;
    startingMessage = true;
    errorText = "";
    noticeText = "";
    let mutationSignature: string | null = null;

    try {
      const materialized = await ensureSessionForComposer();
      if (!materialized) {
        startingMessage = false;
        return;
      }

      const sessionId = materialized.sessionId;
      const sessionProfileId = profileIdForSession(sessionId);
      const activeConversation = materialized.state;
      const attachmentIds = attachmentSnapshot.map((attachment) => attachment.id);
      const selectedSkillsSnapshot = [...(activeConversation.selectedSkills ?? [])];
      const preferences = activeConversation.preferences;
      const clientUserMessageId = createClientUserMessageId();
      mutationSignature = buildComposerMutationSignature("message", sessionId, prompt, selectedSkillsSnapshot, attachmentIds);
      if (!beginComposerMutation(mutationSignature)) {
        startingMessage = false;
        return;
      }
      setOptimisticMessageState(
        sessionId,
        sessionProfileId,
        clientUserMessageId,
        prompt,
        selectedSkillsSnapshot,
        attachmentNames,
        activeConversation
      );
      activatePendingQueueMode(sessionId);
      recordComposerHistory(prompt);
      rememberLastComposerPromptChip(sessionId, prompt);
      if (!preserveComposer) {
        draft = "";
        draftAttachments = [];
        closeFileMentionSearch();
        scheduleComposerTextareaResize();
        composerSettingsOpen = false;
      }

      void api
        .sendMessage(sessionId, {
          prompt: draftText,
          skills: selectedSkillsSnapshot,
          attachmentIds,
          preferences,
          clientUserMessageId,
          profileId: sessionProfileId
        })
      .then(() => {
        scheduleSessionRefresh(80);
        scheduleSelectedSessionStateRefresh(sessionId, 80);
      })
        .catch((error) => {
          if (pendingQueueModeSessionKey === sessionStateKey(sessionId, sessionProfileId)) {
            clearPendingQueueMode(sessionId, sessionProfileId);
          }
          clearOptimisticMessageState(sessionId, sessionProfileId, prompt);
          if (!preserveComposer && selectedSessionId === sessionId && !draft.trim() && draftAttachments.length === 0) {
            draft = draftText;
            draftAttachments = attachmentSnapshot;
            scheduleComposerTextareaResize();
          }
          if (showSessionRecoveryPromptFromError(error, sessionId)) {
            errorText = "";
            return;
          }
          errorText = describeError(error);
        })
        .finally(() => {
          finishComposerMutation(mutationSignature);
          startingMessage = false;
        });
    } catch (error) {
      finishComposerMutation(mutationSignature);
      if (showSessionRecoveryPromptFromError(error, selectedSessionId)) {
        errorText = "";
        startingMessage = false;
        return;
      }
      errorText = describeError(error);
      startingMessage = false;
    }
  }

  async function queueMessage(options?: {
    promptText?: string;
    attachmentSnapshot?: AttachmentRecord[];
    preserveComposer?: boolean;
  }) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    const selectedBinding = ensureSelectedSessionBinding();
    if (!selectedBinding) {
      return;
    }
    if (uploading || (!canQueueComposerMessage(selectedBinding.state) && !canQueueDuringLocalSubmission(selectedBinding.sessionId))) {
      return;
    }
    if (!options?.promptText?.trim() && !draft.trim() && draftAttachments.length === 0) {
      return;
    }

    const sessionId = selectedBinding.sessionId;
    const sessionProfileId = profileIdForSession(sessionId);
    const draftText = options?.promptText ?? draft;
    const attachmentSnapshot = options?.attachmentSnapshot ?? [...draftAttachments];
    const prompt = draftText.trim();
    const preserveComposer = options?.preserveComposer ?? false;
    if (!prompt && attachmentSnapshot.length === 0) {
      return;
    }
    errorText = "";
    noticeText = "";
    const selectedSkillsSnapshot = [...selectedBinding.state.selectedSkills];
    const attachmentIds = attachmentSnapshot.map((attachment) => attachment.id);
    const clientUserMessageId = createClientUserMessageId();
    const mutationSignature = buildComposerMutationSignature("queue", sessionId, prompt, selectedSkillsSnapshot, attachmentIds);
    if (!beginComposerMutation(mutationSignature)) {
      return;
    }
    const optimisticQueueId = addOptimisticQueuedItem(
      sessionId,
      sessionProfileId,
      clientUserMessageId,
      prompt,
      selectedSkillsSnapshot,
      attachmentSnapshot
    );

    recordComposerHistory(prompt);
    rememberLastComposerPromptChip(sessionId, prompt);
    if (!preserveComposer) {
      draft = "";
      draftAttachments = [];
      closeFileMentionSearch();
      scheduleComposerTextareaResize();
      composerSettingsOpen = false;
      void api.clearSessionDraft(sessionId, sessionProfileId).catch(() => {});
    }

    const scopeKey = sessionStateKey(sessionId, sessionProfileId);
    const enqueueQueueRevision = queueStateRevisions.get(scopeKey) ?? 0;
    queuedMessageRequestCountsBySessionId = {
      ...queuedMessageRequestCountsBySessionId,
      [scopeKey]: (queuedMessageRequestCountsBySessionId[scopeKey] ?? 0) + 1
    };

    void api
      .enqueueSessionMessage(sessionId, {
        prompt: draftText,
        clientRequestId: optimisticQueueId,
        clientUserMessageId,
        skills: selectedSkillsSnapshot,
        attachmentIds,
        profileId: sessionProfileId
      })
      .then(async (queue) => {
        const enqueueConfirmed =
          queue.enqueueAccepted === true &&
          typeof queue.enqueueItemId === "string" &&
          queue.enqueueItemId.trim().length > 0;

        if (!enqueueConfirmed) {
          pendingEnqueuesByOptimisticId.delete(optimisticQueueId);
          removeOptimisticQueuedItem(sessionId, sessionProfileId, optimisticQueueId);
          if (
            !preserveComposer &&
            selectedSessionBindingMatches(sessionId, sessionProfileId) &&
            !draft.trim() &&
            draftAttachments.length === 0
          ) {
            draft = draftText;
            draftAttachments = attachmentSnapshot;
            scheduleComposerTextareaResize();
          }
          if (selectedSessionBindingMatches(sessionId, sessionProfileId)) {
            noticeText = "";
            errorText = m.queue_enqueue_failed();
          }
          return;
        }

        const pendingEnqueue = pendingEnqueuesByOptimisticId.get(optimisticQueueId);
        const enqueueItemId = queue.enqueueItemId!.trim();
        const acknowledgedItem =
          queue.items.find(
            (item) =>
              item.id === enqueueItemId ||
              item.clientRequestId === optimisticQueueId ||
              item.clientUserMessageId === clientUserMessageId
          ) ?? null;

        if (pendingEnqueue?.deleted) {
          const updatedQueue = await api.removeQueuedMessage(sessionId, enqueueItemId, sessionProfileId);
          pendingEnqueuesByOptimisticId.delete(optimisticQueueId);
          applyQueuePayloadToSession(sessionId, sessionProfileId, updatedQueue);
        } else if (pendingEnqueue?.edited) {
          const updatedQueue = await api.updateQueuedMessage(sessionId, enqueueItemId, {
            prompt: pendingEnqueue.item.prompt,
            skills: pendingEnqueue.item.skills,
            attachmentIds: pendingEnqueue.item.attachmentIds,
            profileId: sessionProfileId
          });
          pendingEnqueuesByOptimisticId.delete(optimisticQueueId);
          applyQueuePayloadToSession(sessionId, sessionProfileId, updatedQueue);
        } else {
          pendingEnqueuesByOptimisticId.delete(optimisticQueueId);
          if ((queueStateRevisions.get(scopeKey) ?? 0) > enqueueQueueRevision && acknowledgedItem) {
            const currentQueue = sessionQueueSnapshotsBySessionId[scopeKey];
            if (currentQueue) {
              const acknowledgedIds = new Set(
                [acknowledgedItem.id, acknowledgedItem.clientRequestId, acknowledgedItem.clientUserMessageId].filter(
                  (value): value is string => Boolean(value)
                )
              );
              const currentItemIndex = currentQueue.items.findIndex((item) =>
                [item.id, item.clientRequestId, item.clientUserMessageId].some(
                  (value) => value && acknowledgedIds.has(value)
                )
              );
              const items = [...currentQueue.items];
              if (currentItemIndex === -1) {
                items.push(acknowledgedItem);
              } else {
                items[currentItemIndex] = acknowledgedItem;
              }
              applyQueuePayloadToSession(sessionId, sessionProfileId, {
                ...currentQueue,
                items,
                updatedAt: Math.max(Number(currentQueue.updatedAt ?? 0), Number(queue.updatedAt ?? 0)) || null,
                enqueueAccepted: queue.enqueueAccepted,
                enqueueItemId: queue.enqueueItemId
              });
            } else {
              applyQueuePayloadToSession(sessionId, sessionProfileId, queue);
            }
          } else {
            applyQueuePayloadToSession(sessionId, sessionProfileId, queue);
          }
        }
        if (selectedSessionBindingMatches(sessionId, sessionProfileId)) {
          noticeText = m.queue_notice();
        }
      })
      .catch((error) => {
        const pendingEnqueue = pendingEnqueuesByOptimisticId.get(optimisticQueueId);
        const wasDeleted = pendingEnqueue?.deleted === true;
        pendingEnqueuesByOptimisticId.delete(optimisticQueueId);
        removeOptimisticQueuedItem(sessionId, sessionProfileId, optimisticQueueId);
        if (
          !wasDeleted &&
          !preserveComposer &&
          selectedSessionBindingMatches(sessionId, sessionProfileId) &&
          !draft.trim() &&
          draftAttachments.length === 0
        ) {
          draft = draftText;
          draftAttachments = attachmentSnapshot;
          scheduleComposerTextareaResize();
        }
        if (selectedSessionBindingMatches(sessionId, sessionProfileId)) {
          noticeText = "";
          errorText = describeError(error);
        }
      })
      .finally(() => {
        finishComposerMutation(mutationSignature);
        const remainingCount = Math.max(0, (queuedMessageRequestCountsBySessionId[scopeKey] ?? 1) - 1);
        if (remainingCount === 0) {
          const remainingRequests = { ...queuedMessageRequestCountsBySessionId };
          delete remainingRequests[scopeKey];
          queuedMessageRequestCountsBySessionId = remainingRequests;
          return;
        }

        queuedMessageRequestCountsBySessionId = {
          ...queuedMessageRequestCountsBySessionId,
          [scopeKey]: remainingCount
        };
      });
  }

  async function sendSteerPrompt(prompt: string, clearComposer = false) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    const selectedBinding = ensureSelectedSessionBinding();
    if (!selectedBinding || !running || !prompt.trim() || sending) {
      return;
    }

    const sessionId = selectedBinding.sessionId;
    const sessionProfileId = profileIdForSession(sessionId);
    const normalizedPrompt = prompt.trim();
    const steerAttachmentSnapshot =
      clearComposer || draft.trim() === normalizedPrompt ? [...draftAttachments] : [];
    const attachmentIds = steerAttachmentSnapshot.map((attachment) => attachment.id);
    const selectedSkillsSnapshot = [...selectedBinding.state.selectedSkills];
    const activeTurnId = selectedBinding.state.activeTurnId ?? getConversationLiveTurn(selectedBinding.state)?.id ?? null;
    const clientUserMessageId = createClientUserMessageId();
    const mutationSignature = buildComposerMutationSignature("steer", sessionId, normalizedPrompt, selectedSkillsSnapshot, attachmentIds);
    if (!beginComposerMutation(mutationSignature)) {
      return;
    }
    sending = true;
    sendIntent = "steer";
    errorText = "";
    noticeText = "";
    setOptimisticMessageState(
      sessionId,
      sessionProfileId,
      clientUserMessageId,
      normalizedPrompt,
      selectedSkillsSnapshot,
      steerAttachmentSnapshot.map((attachment) => attachment.originalName),
      selectedBinding.state
    );
    try {
      await api.steerTurn(
        sessionId,
        normalizedPrompt,
        attachmentIds,
        selectedSkillsSnapshot,
        activeTurnId,
        clientUserMessageId,
        sessionProfileId
      );
      scheduleSessionRefresh(80);
      scheduleSelectedSessionStateRefresh(sessionId, 80);
      recordComposerHistory(normalizedPrompt);
      rememberLastComposerPromptChip(sessionId, normalizedPrompt);
      if (clearComposer || draft.trim() === normalizedPrompt) {
        draft = "";
        draftAttachments = [];
        closeFileMentionSearch();
        scheduleComposerTextareaResize();
        void api.clearSessionDraft(sessionId, sessionProfileId).catch(() => {});
      }
      pendingSteerResume = null;
    } catch (error) {
      clearOptimisticMessageState(sessionId, sessionProfileId, normalizedPrompt);
      errorText = describeError(error);
    } finally {
      finishComposerMutation(mutationSignature);
      sending = false;
      sendIntent = null;
    }
  }

  async function steerTurn() {
    await sendSteerPrompt(draft, true);
  }

  async function dispatchQueuedMessage(queueId: string, mode: "message" | "steer") {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    const selectedBinding = ensureSelectedSessionBinding();
    if (!selectedBinding || sending) {
      return;
    }

    const sessionId = selectedBinding.sessionId;
    const sessionProfileId = profileIdForSession(sessionId);
    const queuedItem = selectedBinding.state.queue.items.find((item) => item.id === queueId) ?? null;
    if (!queuedItem) {
      requestSelectedSessionResync(false);
      return;
    }
    sending = true;
    sendIntent = mode;
    errorText = "";
    noticeText = "";
    const clientUserMessageId = queuedItem.clientUserMessageId ?? queuedItem.clientRequestId ?? queuedItem.id;
    setOptimisticMessageState(
      sessionId,
      sessionProfileId,
      clientUserMessageId,
      queuedItem.prompt,
      queuedItem.skills,
      queuedItem.attachmentNames,
      selectedBinding.state
    );

    try {
      const activeTurnId =
        mode === "steer"
          ? selectedBinding.state.activeTurnId ?? getConversationLiveTurn(selectedBinding.state)?.id ?? null
          : null;
      const queue = await api.dispatchQueuedMessage(sessionId, queueId, mode, activeTurnId, sessionProfileId);
      applyQueuePayloadToSession(sessionId, sessionProfileId, queue);
      scheduleSessionRefresh(80);
      scheduleSelectedSessionStateRefresh(sessionId, 80);
      dismissedQueueResumeBySessionId = {
        ...dismissedQueueResumeBySessionId,
        [sessionStateKey(sessionId, sessionProfileId)]: false
      };
    } catch (error) {
      clearOptimisticMessageState(sessionId, sessionProfileId, queuedItem.prompt);
      errorText = describeError(error);
    } finally {
      sending = false;
      sendIntent = null;
    }
  }

  function updateQueueDragTarget(clientX: number, clientY: number) {
    if (!queueDragState || typeof document === "undefined") {
      return;
    }

    const nextTarget = document.elementFromPoint(clientX, clientY)?.closest("[data-queue-item-id]");
    if (!(nextTarget instanceof HTMLElement)) {
      queueDragState = {
        ...queueDragState,
        targetQueueId: null,
        targetPosition: null
      };
      return;
    }

    const targetQueueId = nextTarget.dataset.queueItemId ?? null;
    if (!targetQueueId) {
      queueDragState = {
        ...queueDragState,
        targetQueueId: null,
        targetPosition: null
      };
      return;
    }

    const rect = nextTarget.getBoundingClientRect();
    queueDragState = {
      ...queueDragState,
      targetQueueId,
      targetPosition: clientY < rect.top + rect.height / 2 ? "before" : "after"
    };
  }

  function startQueueDrag(event: PointerEvent, queueId: string) {
    const draggedItem = serverQueuedMessages.find((item) => item.id === queueId) ?? null;
    if (queueReorderBusy || serverQueuedMessages.length < 2 || !draggedItem || isOptimisticQueueItem(draggedItem)) {
      return;
    }

    event.preventDefault();
    event.stopPropagation();
    (event.currentTarget as HTMLElement | null)?.setPointerCapture?.(event.pointerId);
    queueDragState = {
      pointerId: event.pointerId,
      queueId,
      targetQueueId: null,
      targetPosition: null
    };
    updateQueueDragTarget(event.clientX, event.clientY);
  }

  function moveQueueDrag(event: PointerEvent) {
    if (!queueDragState || queueDragState.pointerId !== event.pointerId) {
      return;
    }

    event.preventDefault();
    updateQueueDragTarget(event.clientX, event.clientY);
  }

  async function endQueueDrag(event: PointerEvent) {
    if (!queueDragState || queueDragState.pointerId !== event.pointerId) {
      return;
    }

    event.preventDefault();
    const dragState = queueDragState;
    queueDragState = null;
    (event.currentTarget as HTMLElement | null)?.releasePointerCapture?.(event.pointerId);

    const selectedBinding = ensureSelectedSessionBinding(false);
    if (!selectedBinding || dragState.targetQueueId === null || dragState.targetPosition === null) {
      return;
    }

    const sessionId = selectedBinding.sessionId;
    const sessionProfileId = profileIdForSession(sessionId);
    if (!serverQueuedMessages.some((item) => item.id === dragState.targetQueueId)) {
      return;
    }

    const nextQueueIds = serverQueuedMessages
      .filter((item) => item.id !== dragState.queueId)
      .map((item) => item.id);
    const targetIndex = nextQueueIds.indexOf(dragState.targetQueueId);
    if (targetIndex < 0) {
      return;
    }

    nextQueueIds.splice(dragState.targetPosition === "before" ? targetIndex : targetIndex + 1, 0, dragState.queueId);
    const currentQueueIds = serverQueuedMessages.map((item) => item.id);
    if (
      nextQueueIds.length !== currentQueueIds.length ||
      nextQueueIds.every((id, index) => id === currentQueueIds[index])
    ) {
      return;
    }

    queueReorderBusy = true;
    errorText = "";
    noticeText = "";
    try {
      const queue = await api.reorderQueuedMessages(sessionId, nextQueueIds, sessionProfileId);
      applyQueuePayloadToSession(sessionId, sessionProfileId, queue);
    } catch (error) {
      errorText = describeError(error);
    } finally {
      queueReorderBusy = false;
    }
  }

  function cancelQueueDrag(event?: PointerEvent) {
    if (event && queueDragState && queueDragState.pointerId === event.pointerId) {
      (event.currentTarget as HTMLElement | null)?.releasePointerCapture?.(event.pointerId);
    }
    queueDragState = null;
  }

  function showQueueDropIndicator(itemId: string, position: "before" | "after") {
    return (
      queueDragState?.queueId !== itemId &&
      queueDragState?.targetQueueId === itemId &&
      queueDragState?.targetPosition === position
    );
  }

  async function removeQueuedMessage(queueId: string) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    const selectedBinding = ensureSelectedSessionBinding();
    if (!selectedBinding) {
      return;
    }

    const sessionId = selectedBinding.sessionId;
    const sessionProfileId = profileIdForSession(sessionId);
    if (isOptimisticQueueItem({ id: queueId } as SessionQueueItem)) {
      removeOptimisticQueuedItem(sessionId, sessionProfileId, queueId, true);
      if (editingQueueId === queueId) {
        editingQueueId = null;
        editingQueuePrompt = "";
      }
      return;
    }

    try {
      const queue = await api.removeQueuedMessage(sessionId, queueId, sessionProfileId);
      applyQueuePayloadToSession(sessionId, sessionProfileId, queue);
      if (editingQueueId === queueId) {
        editingQueueId = null;
        editingQueuePrompt = "";
      }
    } catch (error) {
      errorText = describeError(error);
    }
  }

  function beginQueuedMessageEdit(item: SessionQueueItem) {
    editingQueueId = item.id;
    editingQueuePrompt = item.prompt;
    errorText = "";
    noticeText = "";
  }

  function cancelQueuedMessageEdit() {
    editingQueueId = null;
    editingQueuePrompt = "";
  }

  async function saveQueuedMessage(queueId: string) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    const selectedBinding = ensureSelectedSessionBinding();
    if (!selectedBinding) {
      return;
    }
    const queuedItem = queuedMessages.find((item) => item.id === queueId);
    if (!queuedItem) {
      cancelQueuedMessageEdit();
      return;
    }

    const nextPrompt = editingQueuePrompt;
    if (nextPrompt.trim() === queuedItem.prompt.trim()) {
      cancelQueuedMessageEdit();
      return;
    }

    errorText = "";
    noticeText = "";
    if (isOptimisticQueueItem(queuedItem)) {
      const profileId = profileIdForSession(selectedBinding.sessionId);
      const scopeKey = sessionStateKey(selectedBinding.sessionId, profileId);
      const existing = optimisticQueuedItemsBySessionId[scopeKey] ?? [];
      const nextItem = {
        ...queuedItem,
        prompt: nextPrompt.trim()
      };
      optimisticQueuedItemsBySessionId = {
        ...optimisticQueuedItemsBySessionId,
        [scopeKey]: existing.map((item) => (item.id === queueId ? nextItem : item))
      };
      const pendingEnqueue = pendingEnqueuesByOptimisticId.get(queueId);
      if (pendingEnqueue) {
        pendingEnqueue.item = nextItem;
        pendingEnqueue.edited = true;
      }
      cancelQueuedMessageEdit();
      return;
    }

    try {
      const queue = await api.updateQueuedMessage(selectedBinding.sessionId, queueId, {
        prompt: nextPrompt,
        skills: queuedItem.skills,
        attachmentIds: queuedItem.attachmentIds,
        profileId: profileIdForSession(selectedBinding.sessionId)
      });
      applyQueuePayloadToSession(
        selectedBinding.sessionId,
        profileIdForSession(selectedBinding.sessionId),
        queue
      );
      noticeText = m.queued_followup_updated();
      cancelQueuedMessageEdit();
    } catch (error) {
      const message = describeError(error);
      errorText =
        message === "Internal Error"
          ? m.queued_followup_restart_required()
          : message;
    }
  }

  async function resumeQueuedMessages() {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    const selectedBinding = ensureSelectedSessionBinding();
    if (!selectedBinding) {
      return;
    }

    const sessionId = selectedBinding.sessionId;
    const sessionProfileId = profileIdForSession(sessionId);
    errorText = "";
    noticeText = "";

    try {
      const queue = await api.resumeSessionQueue(sessionId, sessionProfileId);
      applyQueuePayloadToSession(sessionId, sessionProfileId, queue);
      dismissedQueueResumeBySessionId = {
        ...dismissedQueueResumeBySessionId,
        [sessionStateKey(sessionId, sessionProfileId)]: false
      };
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function submitComposer() {
    if (composerPrimaryActionDisabled) {
      return;
    }

    submitComposerBusy = true;
    try {
      if (await handleSlashCommand(draft)) {
        return;
      }
      if (composerQueueModeActive) {
        await queueMessage();
        return;
      }
      await sendMessage();
    } finally {
      submitComposerBusy = false;
    }
  }

  function reuseLastComposerMessage() {
    if (!lastComposerHistoryPrompt) {
      return;
    }

    draft = lastComposerHistoryPrompt;
    resetComposerHistoryNavigation();
    scheduleComposerTextareaResize();
    composerTextareaElement?.focus();
  }

  async function resendLastComposerMessage() {
    if (recentComposerActionDisabled) {
      return;
    }

    if (composerQueueModeActive) {
      await queueMessage({
        promptText: lastComposerHistoryPrompt,
        attachmentSnapshot: [],
        preserveComposer: true
      });
      return;
    }

    await sendMessage({
      promptText: lastComposerHistoryPrompt,
      attachmentSnapshot: [],
      preserveComposer: true
    });
  }

  function promptAttachmentPicker() {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    if (uploading) {
      return;
    }
    filePickerElement?.click();
  }

  async function uploadFiles(files: FileList | null) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    if (!files || files.length === 0) {
      return;
    }

    uploading = true;
    errorText = "";
    noticeText = "";

    try {
      const materialized = await ensureSessionForComposer();
      if (!materialized) {
        return;
      }

      const response = await api.uploadAttachments(
        materialized.sessionId,
        Array.from(files),
        profileIdForSession(materialized.sessionId)
      );
      draftAttachments = [...draftAttachments, ...response.attachments];
    } catch (error) {
      errorText = describeError(error);
    } finally {
      if (filePickerElement) {
        filePickerElement.value = "";
      }
      uploading = false;
    }
  }

  async function removeDraftAttachment(attachmentId: string) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    const selectedBinding = ensureSelectedSessionBinding(false);
    if (!selectedBinding) {
      return;
    }
    try {
      await api.deleteAttachment(
        selectedBinding.sessionId,
        attachmentId,
        profileIdForSession(selectedBinding.sessionId)
      );
      draftAttachments = draftAttachments.filter((attachment) => attachment.id !== attachmentId);
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function interruptTurn() {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    const selectedBinding = ensureSelectedSessionBinding(false);
    if (!selectedBinding || !running) {
      return;
    }
    try {
      await api.abortTurn(selectedBinding.sessionId, profileIdForSession(selectedBinding.sessionId));
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function archiveSessionFromSidebar(session: SessionSummary) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    try {
      if (showArchivedSessions || session.archived) {
        const response = await api.unarchiveSession(session.id, profileIdForSession(session.id));
        applySessionSummaryUpdate(response.session);
        noticeText = m.session_restored_notice();
      } else {
        await api.archiveSession(session.id, profileIdForSession(session.id));
        applySessionSummaryUpdate({
          ...session,
          archived: true,
          updatedAt: Date.now()
        });
        noticeText = m.session_archived_notice();
      }
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function moveSessionProfileFromSidebar(session: SessionSummary, targetProfileId: string) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    const trimmedTargetProfileId = targetProfileId.trim();
    if (!trimmedTargetProfileId) {
      return;
    }
    const sourceSummaryKey = sessionSummaryKey(session);
    const requestedSourceProfileId = session.profileId ?? profileIdForSession(session.id);
    const selectedSessionMoveRequested =
      selectedSessionId === session.id &&
      (!selectedSessionProfileId || !requestedSourceProfileId || selectedSessionProfileId === requestedSourceProfileId);
    const previousDraftPersistencePaused = draftPersistencePaused;
    try {
      if (selectedSessionMoveRequested) {
        draftPersistencePaused = true;
        if (draftSaveTimer) {
          clearTimeout(draftSaveTimer);
          draftSaveTimer = null;
        }
        if (draft.trim()) {
          await api.saveSessionDraft(
            session.id,
            draft,
            composerQueueModeActive ? "queue" : "message",
            requestedSourceProfileId
          );
        }
      }
      const response = await api.moveSessionProfile(
        session.id,
        trimmedTargetProfileId,
        requestedSourceProfileId
      );
      const sourceProfileId = response.sourceProfileId || session.profileId || null;
      const targetProfile = config?.profiles.find((profile) => profile.id === response.targetProfileId) ?? null;
      const movedSummary: SessionSummary = {
        ...(response.session ?? session),
        profileId: response.targetProfileId,
        profileLabel: response.targetProfileLabel || targetProfile?.label || null,
        profileCodexHome: response.session?.profileCodexHome ?? targetProfile?.codexHome ?? null,
        accountEmail: response.session?.accountEmail ?? null,
        accountType: response.session?.accountType ?? null
      };
      const selectedSessionMoved =
        selectedSessionId === session.id &&
        (!selectedSessionProfileId || !sourceProfileId || selectedSessionProfileId === sourceProfileId);

      setSessionsStable(sessions.filter((entry) => sessionSummaryKey(entry) !== sourceSummaryKey));
      applySessionSummaryUpdate(movedSummary);

      if (selectedSessionMoved) {
        rebindSelectedSessionProfile(
          session.id,
          sourceProfileId,
          response.targetProfileId,
          movedSummary
        );
      }
      scheduleSessionRefresh(60);
      noticeText = getLocale().startsWith("ko")
        ? `세션을 ${response.targetProfileLabel || response.targetProfileId} 계정으로 이동했습니다.`
        : `Moved the session to ${response.targetProfileLabel || response.targetProfileId}.`;
    } catch (error) {
      errorText = describeError(error);
    } finally {
      if (selectedSessionMoveRequested && selectedSessionId === session.id) {
        draftPersistencePaused = previousDraftPersistencePaused;
      }
    }
  }

  function profileMoveOptionsForSession(session: SessionSummary | null) {
    if (!session || !config) {
      return [];
    }
    const currentProfileId = session.profileId ?? config.profiles.find((profile) => profile.active)?.id ?? null;
    return config.profiles.filter((profile) => profile.id !== currentProfileId);
  }

  function openSessionProfileMoveDialog(session: SessionSummary) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    const options = profileMoveOptionsForSession(session);
    if (options.length === 0) {
      return;
    }
    profileMoveDialogSession = session;
    profileMoveDialogTargetId = options[0]?.id ?? "";
  }

  function closeSessionProfileMoveDialog() {
    profileMoveDialogSession = null;
    profileMoveDialogTargetId = "";
  }

  async function confirmSessionProfileMoveDialog() {
    const session = profileMoveDialogSession;
    const targetProfileId = profileMoveDialogTargetId.trim();
    if (!session || !targetProfileId) {
      return;
    }
    closeSessionProfileMoveDialog();
    await moveSessionProfileFromSidebar(session, targetProfileId);
  }

  async function toggleSessionPinned(session: SessionSummary) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    try {
      const nextPinned = !session.pinned;
      const response = await api.updateSessionOrganization(
        session.id,
        {
          pinned: nextPinned
        },
        profileIdForSession(session.id)
      );
      applySessionSummaryUpdate({
        ...session,
        pinned: response.meta.pinned,
        tags: response.meta.tags
      });
      updateConfigSessionOrganization({
        knownTags: response.knownTags,
        sessionFolders: response.sessionFolders
      });
      noticeText = nextPinned ? m.session_pinned_notice() : m.session_unpinned_notice();
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function editSelectedSessionTags() {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    if (!selectedSessionId || !selectedSessionSummary) {
      return;
    }

    const initialValue = selectedSessionSummary.tags.join(", ");
    const nextValue = typeof window === "undefined" ? null : window.prompt(m.session_tags_prompt(), initialValue);
    if (nextValue === null) {
      return;
    }

    const nextTags = [...new Set(nextValue.split(",").map((entry) => entry.trim()).filter((entry) => entry.length > 0))];
    try {
      const response = await api.updateSessionOrganization(
        selectedSessionId,
        {
          tags: nextTags
        },
        profileIdForSession(selectedSessionId)
      );
      applySessionSummaryUpdate({
        ...selectedSessionSummary,
        pinned: response.meta.pinned,
        tags: response.meta.tags
      });
      updateConfigSessionOrganization({
        knownTags: response.knownTags,
        sessionFolders: response.sessionFolders
      });
      noticeText = m.session_tags_updated_notice();
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function archiveCurrentSession() {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    if (!selectedSessionId || showArchivedSessions) {
      return;
    }

    try {
      await api.archiveSession(selectedSessionId, profileIdForSession(selectedSessionId));
      showArchivedSessions = true;
      await refreshSessions();
      noticeText = m.session_archived_notice();
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function unarchiveCurrentSession() {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    if (!selectedSessionId || !showArchivedSessions) {
      return;
    }

    try {
      const response = await api.unarchiveSession(selectedSessionId, profileIdForSession(selectedSessionId));
      showArchivedSessions = false;
      await refreshSessions(response.session);
      noticeText = m.session_restored_notice();
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function logoutWebUi() {
    try {
      await api.logout();
    } finally {
      void clearSessionBrowserCache();
      loginPassword = "";
      loginMessage = "";
      clearWorkspaceForLoggedOut();
    }
  }

  async function handleLogin() {
    if (!loginPassword.trim()) {
      loginMessage = ui.enterPassword;
      return;
    }
    if (loginHcaptcha.enabled && !loginHcaptchaToken) {
      loginMessage = ui.completeHcaptcha;
      return;
    }

    loginBusy = true;
    loginMessage = "";

    try {
      const loginResult = await api.login(loginPassword.trim(), loginHcaptchaToken || null);
      resetRealtimeConnectionForAuthChange();
      loginPassword = "";
      authenticated = true;
      webRole = loginResult.role ?? "admin";
      loading = true;
      void bootstrap();
    } catch (error) {
      authenticated = false;
      loginMessage = error instanceof Error ? error.message : ui.loginFailed;
    } finally {
      if (loginHcaptcha.enabled && loginHcaptchaWidgetId !== null) {
        loginHcaptchaToken = "";
        window.hcaptcha?.reset?.(loginHcaptchaWidgetId);
      }
      loginBusy = false;
    }
  }

  async function startAccountLogin(type: "chatgpt" | "chatgptDeviceCode") {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    try {
      const response = await api.startAccountLogin(type);

      if (response.type === "chatgpt") {
        accountLoginFlow = {
          type: "chatgpt",
          loginId: response.loginId,
          authUrl: response.authUrl,
          busy: true,
          error: null
        };
        if (typeof window !== "undefined") {
          window.open(response.authUrl, "_blank", "noopener,noreferrer");
        }
        return;
      }

      if (response.type === "chatgptDeviceCode") {
        accountLoginFlow = {
          type: "chatgptDeviceCode",
          loginId: response.loginId,
          verificationUrl: response.verificationUrl,
          userCode: response.userCode,
          busy: true,
          error: null
        };
        return;
      }

      accountLoginFlow = null;
      await refreshAccountState(true);
      await refreshQuota(true);
      await refreshProfileAccounts(true);
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function importAccountCredentialsFromServer(
    path: string,
    options: { createProfile?: boolean; profileLabel?: string | null; profileId?: string | null } = {}
  ) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    const trimmedPath = path.trim();
    if (!trimmedPath) {
      errorText = getLocale().startsWith("ko") ? "서버 credentials JSON 경로를 입력하세요." : "Enter a server credentials JSON path.";
      return;
    }
    try {
      const response = await api.startAccountLogin("authJsonFile", null, trimmedPath, options);
      accountLoginFlow = null;
      await refreshAccountState(true);
      await refreshQuota(true);
      await refreshProfileAccounts(true);
      config = applyLocalComposerPreferencesToConfig(await api.getConfig());
      if (response.type === "authJsonFile" && response.restartRequired) {
        noticeText = getLocale().startsWith("ko")
          ? `프로필을 설정 파일에 추가했습니다. WebUI 재시작 후 사용할 수 있습니다.`
          : "Added the profile to the config file. Restart WebUI to use it.";
      }
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function cancelAccountLogin(loginId: string) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    try {
      await api.cancelAccountLogin(loginId);
      accountLoginFlow = null;
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function logoutAccount() {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    try {
      await api.logoutAccount();
      accountLoginFlow = null;
      await refreshAccountState(true);
      await refreshQuota(true);
      await refreshProfileAccounts(true);
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function selectAccountProfile(profileId: string) {
    if (!profileId.trim()) {
      return;
    }

    const currentProfileId = config?.profiles.find((profile) => profile.active)?.id ?? null;
    if (currentProfileId === profileId) {
      return;
    }

    const switchGeneration = ++accountProfileSwitchGeneration;
    try {
      const switchRequest = accountProfileSwitchQueue
        .catch(() => undefined)
        .then(() => api.selectAuthProfile(profileId));
      accountProfileSwitchQueue = switchRequest.then(
        () => undefined,
        () => undefined
      );
      const response = await switchRequest;
      if (switchGeneration !== accountProfileSwitchGeneration) {
        return;
      }
      activeProfileId = response.activeProfileId;
      const nextConfig = await api.getConfig();
      if (switchGeneration !== accountProfileSwitchGeneration) {
        return;
      }
      config = applyLocalComposerPreferencesToConfig(nextConfig);
      syncConfiguredTheme(config);
      syncStartupAlertModal(config);
      void refreshRuntimeStatus(true, { silent: true });
      void refreshQuota(true);
      void refreshResetTickets(true);
      void refreshAccountState(true);
      void refreshProfileAccounts(true);
    } catch (error) {
      if (switchGeneration === accountProfileSwitchGeneration) {
        errorText = describeError(error);
      }
    }
  }

  async function renameAccountProfile(profileId: string, currentLabel: string) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    const nextLabel = window.prompt(getLocale().startsWith("ko") ? "새 계정 라벨을 입력하세요." : "Enter a new account label.", currentLabel);
    if (nextLabel === null) {
      return;
    }
    const trimmed = nextLabel.trim();
    if (!trimmed || trimmed === currentLabel.trim()) {
      return;
    }
    try {
      const response = await api.updateAccountProfile(profileId, trimmed);
      if (config) {
        config = {
          ...config,
          profiles: config.profiles.map((profile) => (profile.id === profileId ? { ...profile, label: trimmed } : profile))
        };
      }
      profileAccounts = profileAccounts.map((profile) => (profile.profileId === profileId ? { ...profile, label: trimmed } : profile));
      if (response.restartRequired) {
        noticeText = getLocale().startsWith("ko")
          ? "계정 라벨을 설정 파일에 저장했습니다. WebUI 재시작 후 완전히 반영됩니다."
          : "Saved the account label to the config file. Restart WebUI to fully apply it.";
      }
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function deleteAccountProfile(profileId: string, label: string) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    const currentProfileId = config?.profiles.find((profile) => profile.active)?.id ?? activeProfileId;
    if (profileId === currentProfileId) {
      errorText = getLocale().startsWith("ko") ? "다른 계정으로 전환한 뒤 삭제하세요." : "Switch to another account before deleting it.";
      return;
    }
    const confirmed = window.confirm(
      getLocale().startsWith("ko")
        ? `"${label}" 계정을 목록에서 삭제할까요? 저장된 auth 파일은 기본적으로 남겨둡니다.`
        : `Remove "${label}" from the account list? Stored auth files are kept by default.`
    );
    if (!confirmed) {
      return;
    }
    try {
      const response = await api.deleteAccountProfile(profileId, false);
      if (config) {
        config = {
          ...config,
          profiles: config.profiles.filter((profile) => profile.id !== profileId)
        };
      }
      profileAccounts = profileAccounts.filter((profile) => profile.profileId !== profileId);
      if (response.restartRequired) {
        noticeText = getLocale().startsWith("ko")
          ? "계정을 설정 파일에서 제거했습니다. WebUI 재시작 후 완전히 반영됩니다."
          : "Removed the account from the config file. Restart WebUI to fully apply it.";
      }
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function refreshRuntimeStatus(checkForUpdate = false, options: { silent?: boolean } = {}) {
    const nextBusyAction: typeof runtimeBusyAction = checkForUpdate ? "check" : "status";
    if (runtimeBusyAction === "install" || runtimeBusyAction === "update") {
      return;
    }
    runtimeBusyAction = nextBusyAction;
    try {
      runtime = checkForUpdate ? await api.checkRuntimeUpdate() : await api.getRuntimeStatus();
    } catch (error) {
      if (!options.silent) {
        errorText = describeError(error);
      }
    } finally {
      if (runtimeBusyAction === nextBusyAction) {
        runtimeBusyAction = null;
      }
    }
  }

  async function refreshQuota(force = false) {
    if (quotaRefreshPromise) {
      quotaForceRefreshQueued = quotaForceRefreshQueued || force;
      return quotaRefreshPromise;
    }

    const runForce = force || quotaForceRefreshQueued;
    quotaForceRefreshQueued = false;
    if (runForce) {
      quotaBusy = true;
    }
    const quotaProfileId = activeProfileId;

    quotaRefreshPromise = (async () => {
      try {
        const nextQuota = await api.getQuota(runForce);
        if (quotaProfileId === activeProfileId) {
          quota = nextQuota;
        }
      } catch (error) {
        if (runForce) {
          errorText = describeError(error);
        }
      } finally {
        if (runForce) {
          quotaBusy = false;
        }
      }
    })();

    try {
      await quotaRefreshPromise;
    } finally {
      quotaRefreshPromise = null;
      if (quotaForceRefreshQueued) {
        void refreshQuota(true);
      }
    }
  }

  async function refreshAccountQuotaSurfaces() {
    // Refresh the active quota first. Otherwise the profile list can race the
    // same single-flight request and persist a temporary `refreshing` payload.
    await refreshQuota(true);
    await Promise.all([refreshProfileAccounts(true), refreshResetTickets(true)]);
  }

  async function refreshProfileAccounts(force = false) {
    if (readOnlyRole) {
      return;
    }
    if (profileAccountsRefreshPromise) {
      profileAccountsForceRefreshQueued = profileAccountsForceRefreshQueued || force;
      return profileAccountsRefreshPromise;
    }

    const runForce = force || profileAccountsForceRefreshQueued;
    profileAccountsForceRefreshQueued = false;
    profileAccountsBusy = true;
    profileAccountsRefreshPromise = (async () => {
      try {
        const payload = await api.getProfileAccounts(runForce);
        profileAccounts = payload.profiles;
      } catch (error) {
        if (runForce) {
          errorText = describeError(error);
        }
      } finally {
        profileAccountsBusy = false;
      }
    })();

    try {
      await profileAccountsRefreshPromise;
    } finally {
      profileAccountsRefreshPromise = null;
      if (profileAccountsForceRefreshQueued) {
        void refreshProfileAccounts(true);
      }
    }
  }

  async function refreshResetTickets(force = false) {
    if (resetTicketsRefreshPromise) {
      resetTicketsForceRefreshQueued = resetTicketsForceRefreshQueued || force;
      return resetTicketsRefreshPromise;
    }

    const runForce = force || resetTicketsForceRefreshQueued;
    resetTicketsForceRefreshQueued = false;
    resetTicketsBusy = true;
    const resetTicketProfileId = activeProfileId;

    resetTicketsRefreshPromise = (async () => {
      try {
        const nextResetTickets = await api.getResetTickets(runForce);
        if (resetTicketProfileId === activeProfileId) {
          resetTickets = nextResetTickets;
        }
      } catch (error) {
        errorText = describeError(error);
      } finally {
        resetTicketsBusy = false;
      }
    })();

    try {
      await resetTicketsRefreshPromise;
    } finally {
      resetTicketsRefreshPromise = null;
      if (resetTicketsForceRefreshQueued) {
        void refreshResetTickets(true);
      }
    }
  }

  async function useResetTicket(ticket: CodexResetTicket) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    if (!ticket.available || resetTicketUseBusyId) {
      return;
    }

    const label = ticket.label ?? ticket.limitName ?? ticket.limitId ?? ticket.id;
    const confirmed = window.confirm(
      getLocale().startsWith("ko")
        ? `리셋 티켓 "${label}"을 사용합니다. 이 작업은 되돌릴 수 없습니다. 계속할까요?`
        : `Use reset ticket "${label}"? This cannot be undone.`
    );
    if (!confirmed) {
      return;
    }

    resetTicketUseBusyId = ticket.id;
    try {
      const idempotencyKey =
        typeof crypto !== "undefined" && "randomUUID" in crypto
          ? crypto.randomUUID()
          : `reset-${Date.now()}-${Math.random().toString(36).slice(2)}`;
      const result = await api.useResetTicket(ticket.id, ticket.limitId, idempotencyKey);
      if (result.outcome === "nothingToReset") {
        noticeText = getLocale().startsWith("ko")
          ? "현재 리셋할 사용량 창이 없습니다."
          : "There is no eligible usage window to reset.";
      } else if (result.outcome === "noCredit") {
        noticeText = getLocale().startsWith("ko")
          ? "사용 가능한 리셋 티켓이 없습니다."
          : "No reset credits are available.";
      } else {
        noticeText = getLocale().startsWith("ko") ? "리셋 티켓을 사용했습니다." : "Reset ticket used.";
      }
      await Promise.all([refreshQuota(true), refreshResetTickets(true)]);
    } catch (error) {
      errorText = describeError(error);
    } finally {
      resetTicketUseBusyId = null;
    }
  }

  async function installCodex() {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    runtimeBusyAction = "install";
    errorText = "";

    try {
      const response = await api.installRuntime();
      runtime = response.runtime;
      noticeText = response.message;
      if (response.runtime.installed) {
        await refreshQuota(true);
        await bootstrap();
      }
    } catch (error) {
      errorText = describeError(error);
    } finally {
      runtimeBusyAction = null;
    }
  }

  async function installApp() {
    if (pwaInstalled) {
      noticeText = ui.appAlreadyInstalled;
      return;
    }

    if (deferredInstallPrompt) {
      pwaInstallBusy = true;
      errorText = "";

      try {
        await deferredInstallPrompt.prompt();
        const result = await deferredInstallPrompt.userChoice;
        deferredInstallPrompt = null;
        if (result.outcome === "dismissed") {
          noticeText = ui.appInstallPromptDismissed;
        }
      } catch (error) {
        errorText = describeError(error);
      } finally {
        pwaInstallBusy = false;
      }
      return;
    }

    if (pwaManualInstallOnly) {
      noticeText = ui.appInstallIosHint;
      return;
    }

    noticeText = ui.appInstallUnavailable;
  }

  async function updateCodex() {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    runtimeBusyAction = "update";
    errorText = "";

    try {
      const response = await api.updateRuntime();
      runtime = response.runtime;
      noticeText = response.message;
      await refreshQuota(true);
    } catch (error) {
      errorText = describeError(error);
    } finally {
      runtimeBusyAction = null;
    }
  }

  async function restartGateway() {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    if (gatewayRestartBusy) {
      return;
    }
    gatewayRestartBusy = true;
    errorText = "";

    try {
      await api.restartGateway();
      noticeText = ui.restartWebuiNotice;
      window.setTimeout(() => {
        api.reconnectNow();
      }, 2500);
    } catch (error) {
      errorText = describeError(error);
      gatewayRestartBusy = false;
    }
  }

  async function openBrowser(startPath: string | null) {
    browserOpen = true;
    await browseTo(startPath);
  }

  async function browseTo(currentPath: string | null) {
    browserBusy = true;
    try {
      directoryPayload = await api.browseDirectories(currentPath);
    } catch (error) {
      errorText = describeError(error);
    } finally {
      browserBusy = false;
    }
  }

  function chooseDirectory(nextPath: string) {
    setPreference("cwd", nextPath);
    browserOpen = false;
  }

  function handleRepoSelect(repoPath: string | null) {
    if (readOnlyRole) {
      viewerGitRepoPath = repoPath;
      return;
    }
    setPreference("gitRepoPath", repoPath);
  }

  function handleGitDiffTabRepoSelect(tabId: string, repoPath: string | null) {
    handleRepoSelect(repoPath);
    if (!repoPath) {
      return;
    }
    gitDiffTabs = gitDiffTabs.map((tab) => (tab.id === tabId ? { ...tab, repoPath } : tab));
  }

  function activateTab(tabId: WorkspaceTabId) {
    activeWorkspaceTabId = tabId;
    workspaceMenuOpen = false;
  }

  function openTasksTab() {
    tasksTabOpen = true;
    activeWorkspaceTabId = "tasks";
    workspaceMenuOpen = false;
  }

  function closeTasksTab() {
    tasksTabOpen = false;
    if (activeWorkspaceTabId === "tasks") {
      activeWorkspaceTabId = "chat";
    }
  }

  function openGitTab() {
    gitTabOpen = true;
    activeWorkspaceTabId = "git";
    workspaceMenuOpen = false;
  }

  function openSettingsTab(tab: typeof settingsInitialTab = "config") {
    settingsInitialTab = tab;
    settingsTabOpen = true;
    activeWorkspaceTabId = "settings";
    workspaceMenuOpen = false;
  }

  function closeGitTab() {
    gitTabOpen = false;
    if (activeWorkspaceTabId === "git") {
      activeWorkspaceTabId = "chat";
    }
  }

  function closeSettingsTab() {
    settingsTabOpen = false;
    if (activeWorkspaceTabId === "settings") {
      activeWorkspaceTabId = "chat";
    }
  }

  function openComputerTab() {
    computerTabOpen = true;
    activeWorkspaceTabId = "computer";
    workspaceMenuOpen = false;
  }

  function closeComputerTab() {
    computerTabOpen = false;
    if (activeWorkspaceTabId === "computer") {
      activeWorkspaceTabId = "chat";
    }
  }

  function openDiagnosticsTab() {
    diagnosticsTabOpen = true;
    activeWorkspaceTabId = "diagnostics";
    workspaceMenuOpen = false;
  }

  function closeDiagnosticsTab() {
    diagnosticsTabOpen = false;
    if (activeWorkspaceTabId === "diagnostics") {
      activeWorkspaceTabId = "chat";
    }
  }

  function openMemoryTab() {
    memoryTabOpen = true;
    activeWorkspaceTabId = "memory";
    workspaceMenuOpen = false;
  }

  function closeMemoryTab() {
    memoryTabOpen = false;
    if (activeWorkspaceTabId === "memory") {
      activeWorkspaceTabId = "chat";
    }
  }

  async function sendComputerInput(input: ComputerInputEvent) {
    if (!selectedSessionId || readOnlyRole || computerInputBusy) {
      return;
    }
    computerInputBusy = true;
    computerInputStatus = null;
    try {
      const result = await api.sendComputerInput(selectedSessionId, input, profileIdForSession(selectedSessionId));
      computerInputStatus = `${ui.computerInputDelivered} · ${result.routed}`;
    } catch (error) {
      computerInputStatus = ui.computerInputFailed;
      errorText = describeError(error);
    } finally {
      computerInputBusy = false;
    }
  }

  function handleComputerFrameClick(event: MouseEvent) {
    if (!selectedComputerFrame || readOnlyRole || computerInputBusy) {
      return;
    }
    const target = event.currentTarget as HTMLElement;
    const rect = target.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) {
      return;
    }
    void sendComputerInput({
      type: event.detail >= 2 ? "double_click" : "click",
      x: Math.min(Math.max((event.clientX - rect.left) / rect.width, 0), 1),
      y: Math.min(Math.max((event.clientY - rect.top) / rect.height, 0), 1),
      button: "left",
      coordinateSpace: "normalized",
      frameUpdatedAt: selectedComputerFrame.updatedAt
    });
  }

  function sendComputerTextInput() {
    const text = computerInputText.trim();
    if (!text || !selectedComputerFrame) {
      return;
    }
    computerInputText = "";
    void sendComputerInput({
      type: "text",
      text,
      frameUpdatedAt: selectedComputerFrame.updatedAt
    });
  }

  function sendComputerKeyInput(key: string, modifiers: string[] = []) {
    if (!selectedComputerFrame) {
      return;
    }
    void sendComputerInput({
      type: "key",
      key,
      modifiers,
      frameUpdatedAt: selectedComputerFrame.updatedAt
    });
  }

  function sendComputerScrollInput(deltaY: number) {
    if (!selectedComputerFrame) {
      return;
    }
    void sendComputerInput({
      type: "scroll",
      deltaX: 0,
      deltaY,
      coordinateSpace: "normalized",
      frameUpdatedAt: selectedComputerFrame.updatedAt
    });
  }

  function openGitDiffTab(repoPath: string, filePath: string) {
    const tabKey = `${repoPath}::${filePath}`;
    const nextRequest: GitOpenRequest = {
      repoPath,
      filePath,
      filePaths: null,
      title: baseName(filePath),
      requestId: Date.now()
    };
    const existing = gitDiffTabs.find((tab) => tab.id === `git-diff:${tabKey}`);

    if (existing) {
      gitDiffTabs = gitDiffTabs.map((tab) => (tab.id === existing.id ? { ...tab, request: nextRequest } : tab));
      activeWorkspaceTabId = existing.id;
      workspaceMenuOpen = false;
      return;
    }

    const nextTab: GitDiffTab = {
      id: `git-diff:${tabKey}`,
      repoPath,
      filePath,
      filePaths: null,
      label: baseName(filePath),
      request: nextRequest
    };
    gitDiffTabs = [...gitDiffTabs, nextTab];
    activeWorkspaceTabId = nextTab.id;
    workspaceMenuOpen = false;
  }

  async function openGitCommitDiffTab(repoPath: string, commit: GitCommit) {
    errorText = "";
    const payload = await api.getGitCommitDiff(repoPath, commit.hash);
    const views = parseAggregatedDiffViews(payload.diff);
    openCodeDiffTab(
      `git-commit:${repoPath}:${commit.hash}`,
      commit.shortHash,
      `${commit.shortHash} ${commit.subject}`,
      views.length > 0
        ? views
        : [
            {
              path: `${commit.shortHash}.diff`,
              kind: "update",
              movePath: null,
              diff: payload.diff,
              original: "",
              modified: payload.diff,
              renderable: false
            }
          ]
    );
  }

  function openCodeDiffTab(tabKey: string, label: string, title: string, views: FileChangeView[]) {
    const existing = codeDiffTabs.find((tab) => tab.id === `code-diff:${tabKey}`);

    if (existing) {
      codeDiffTabs = codeDiffTabs.map((tab) => (tab.id === existing.id ? { ...tab, label, title, views } : tab));
      activeWorkspaceTabId = existing.id;
      workspaceMenuOpen = false;
      return;
    }

    const nextTab: CodeDiffTab = {
      id: `code-diff:${tabKey}`,
      label,
      title,
      views
    };
    codeDiffTabs = [...codeDiffTabs, nextTab];
    activeWorkspaceTabId = nextTab.id;
    workspaceMenuOpen = false;
  }

  function openLiveDiffTab(turnId: string, diff: string) {
    const views = parseAggregatedDiffViews(diff);
    openCodeDiffTab(
      `live:${turnId}`,
      ui.aggregatedDiff,
      ui.aggregatedDiff,
      views.length > 0
        ? views
        : [
            {
              path: "aggregated.diff",
              kind: "update",
              movePath: null,
              diff,
              original: "",
              modified: diff,
              renderable: false
            }
          ]
    );
  }

  function closeGitDiffTab(tabId: string) {
    gitDiffTabs = gitDiffTabs.filter((tab) => tab.id !== tabId);
    if (activeWorkspaceTabId === tabId) {
      activeWorkspaceTabId = gitTabOpen ? "git" : "chat";
    }
  }

  function closeCodeDiffTab(tabId: string) {
    codeDiffTabs = codeDiffTabs.filter((tab) => tab.id !== tabId);
    if (activeWorkspaceTabId === tabId) {
      activeWorkspaceTabId = "chat";
    }
  }

  function fileTabId(filePath: string): `file:${string}` {
    return `file:${encodeURIComponent(filePath)}`;
  }

  function openFileTab(filePath: string) {
    const cleanPath = extractLocalFilePath(filePath);
    const id = fileTabId(cleanPath);
    const existing = fileTabs.find((tab) => tab.id === id);

    if (existing) {
      activeWorkspaceTabId = existing.id;
      workspaceMenuOpen = false;
      return;
    }

    const nextTab: FileTab = {
      id,
      path: cleanPath,
      label: baseName(cleanPath) || cleanPath
    };
    fileTabs = [...fileTabs, nextTab];
    activeWorkspaceTabId = nextTab.id;
    workspaceMenuOpen = false;
  }

  function closeFileTab(tabId: string) {
    fileTabs = fileTabs.filter((tab) => tab.id !== tabId);
    if (activeWorkspaceTabId === tabId) {
      activeWorkspaceTabId = "chat";
    }
  }

  async function createTerminalTab() {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    try {
      const snapshot = await api.createTerminal(conversation?.preferences.cwd ?? config?.defaults.cwd ?? null, null);
      terminals = [snapshot.terminal, ...terminals.filter((terminal) => terminal.id !== snapshot.terminal.id)];
      activeWorkspaceTabId = `terminal:${snapshot.terminal.id}`;
      workspaceMenuOpen = false;
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function closeTerminalTab(terminalId: string) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    try {
      await api.closeTerminal(terminalId);
      terminals = terminals.filter((terminal) => terminal.id !== terminalId);
      if (activeWorkspaceTabId === `terminal:${terminalId}`) {
        activeWorkspaceTabId = "chat";
      }
    } catch (error) {
      errorText = describeError(error);
    }
  }

  function attachTerminalContext(payload: TerminalContextPayload) {
    const nextAttachments = payload.attachments.filter(
      (attachment) => !draftAttachments.some((existingAttachment) => existingAttachment.id === attachment.id)
    );
    if (nextAttachments.length === 0) {
      return;
    }

    draftAttachments = [...draftAttachments, ...nextAttachments];
    if (conversation && selectedSessionId === conversation.thread.id) {
      conversation = {
        ...conversation,
        attachments: [...nextAttachments, ...conversation.attachments.filter((attachment) => !nextAttachments.some((nextAttachment) => nextAttachment.id === attachment.id))]
      };
      markConversationCacheDirty();
    }

    activeWorkspaceTabId = "chat";
    noticeText = m.terminal_context_attached({ name: nextAttachments[0].originalName });
  }

  function extractLocalFilePath(href: string) {
    const cleanHref = href.split("#")[0]?.split("?")[0] ?? href;
    const lineMatch = cleanHref.match(/^(\/.*?)(?::\d+)?$/u);
    return lineMatch?.[1] ?? cleanHref;
  }

  function openFileFromMessage(href: string) {
    openFileTab(extractLocalFilePath(href));
  }

  async function resolveGitFileForPath(filePath: string) {
    const resolved = await api.resolveGitFile(extractLocalFilePath(filePath));
    if ((viewerGitRepoPath ?? conversation?.preferences.gitRepoPath ?? null) !== resolved.repoPath) {
      handleRepoSelect(resolved.repoPath);
    }
    return resolved;
  }

  async function openGitWorkspaceFileFromPath(filePath: string) {
    try {
      const resolved = await resolveGitFileForPath(filePath);
      gitTabOpen = true;
      activeWorkspaceTabId = "git";
      workspaceMenuOpen = false;
      if (resolved.filePath) {
        gitOpenRequest = {
          repoPath: resolved.repoPath,
          filePath: resolved.filePath,
          filePaths: null,
          title: baseName(resolved.filePath),
          requestId: Date.now()
        };
      }
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function openGitDiffFromPath(filePath: string) {
    try {
      const resolved = await resolveGitFileForPath(filePath);
      if (resolved.filePath) {
        openGitDiffTab(resolved.repoPath, resolved.filePath);
        return;
      }
      openGitTab();
    } catch (error) {
      errorText = describeError(error);
    }
  }

  function updateSessionSearchQuery(query: string) {
    sessionSearchQuery = query;
    syncSessionListStateInUrl();
    scheduleSessionRefresh(query.trim() ? 180 : 60);
  }

  function updateSessionSearchScope(scope: SessionSearchScope) {
    sessionSearchScope = scope;
    syncSessionListStateInUrl();
    scheduleSessionRefresh(60);
  }

  function updateSessionFilter(patch: Partial<SessionSummaryFilter>) {
    sessionFilter = normalizeSessionFilterState({
      ...sessionFilter,
      ...patch
    });
    if (patch.untaggedOnly) {
      sessionFilter = normalizeSessionFilterState({
        ...sessionFilter,
        tags: []
      });
    } else if (patch.tags && patch.tags.length > 0) {
      sessionFilter = normalizeSessionFilterState({
        ...sessionFilter,
        untaggedOnly: false
      });
    }
    activeSessionFolder =
      !sessionFilter.untaggedOnly &&
      sessionFilter.tags.length === 1 &&
      (config?.sessionOrganization.sessionFolders ?? []).some((folder) => folder.name === sessionFilter.tags[0])
        ? sessionFilter.tags[0]
        : null;
    activeSavedSessionFilterId = null;
    syncSessionListStateInUrl();
    scheduleSessionRefresh(60);
  }

  function applySavedSessionFilter(filter: SavedSessionFilter | null) {
    sessionFilter = normalizeSessionFilterState(filter);
    activeSavedSessionFilterId = filter?.id ?? null;
    activeSessionFolder = null;
    syncSessionListStateInUrl();
    scheduleSessionRefresh(0);
  }

  function openSessionFolder(folderName: string | null) {
    activeSessionFolder = folderName;
    activeSavedSessionFilterId = null;
    sessionFilter = normalizeSessionFilterState({
      ...sessionFilter,
      tags: folderName ? [folderName] : [],
      untaggedOnly: false
    });
    showArchivedSessions = false;
    syncSessionListStateInUrl();
    scheduleSessionRefresh(0);
  }

  function openUnfiledSessions() {
    activeSessionFolder = null;
    activeSavedSessionFilterId = null;
    sessionFilter = normalizeSessionFilterState({
      ...sessionFilter,
      tags: [],
      untaggedOnly: true
    });
    showArchivedSessions = false;
    syncSessionListStateInUrl();
    scheduleSessionRefresh(0);
  }

  async function createSessionFolder() {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    const folderName = typeof window === "undefined" ? "" : window.prompt(m.session_folder_name_prompt(), "")?.trim() ?? "";
    if (!folderName) {
      return;
    }
    try {
      const response = await api.upsertSessionFolder(folderName, false);
      updateConfigSessionOrganization({
        knownTags: response.knownTags,
        sessionFolders: response.sessionFolders
      });
      openSessionFolder(response.folder.name);
      noticeText = m.session_folder_created_notice({ name: response.folder.name });
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function toggleSessionFolderPin(folder: SessionFolder) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    try {
      const response = await api.upsertSessionFolder(folder.name, !folder.pinned);
      updateConfigSessionOrganization({
        knownTags: response.knownTags,
        sessionFolders: response.sessionFolders
      });
      noticeText = response.folder.pinned
        ? m.session_folder_pinned_notice({ name: response.folder.name })
        : m.session_folder_unpinned_notice({ name: response.folder.name });
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function setSelectedSessionFolderMembership(folderName: string, inFolder: boolean) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    if (!selectedSessionId || !selectedSessionSummary) {
      return;
    }
    const nextTags = inFolder
      ? [...new Set([...selectedSessionSummary.tags, folderName])]
      : selectedSessionSummary.tags.filter((tag) => tag !== folderName);
    try {
      const response = await api.updateSessionOrganization(
        selectedSessionId,
        {
          tags: nextTags
        },
        profileIdForSession(selectedSessionId)
      );
      applySessionSummaryUpdate({
        ...selectedSessionSummary,
        pinned: response.meta.pinned,
        tags: response.meta.tags
      });
      updateConfigSessionOrganization({
        knownTags: response.knownTags,
        sessionFolders: response.sessionFolders
      });
      noticeText = inFolder
        ? m.session_folder_added_notice({ name: folderName })
        : m.session_folder_removed_notice({ name: folderName });
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function saveCurrentSessionFilter() {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    const filterName = typeof window === "undefined" ? "" : window.prompt(m.saved_filter_name_prompt(), "")?.trim() ?? "";
    if (!filterName) {
      return;
    }

    try {
      const savedFilter: SavedSessionFilter = {
        id: activeSavedSessionFilterId ?? crypto.randomUUID(),
        name: filterName,
        ...sessionFilter
      };
      const response = await api.saveSessionFilter(savedFilter);
      updateConfigSessionOrganization({
        savedFilters: response.savedFilters,
        knownTags: response.knownTags
      });
      activeSavedSessionFilterId = savedFilter.id;
      syncSessionListStateInUrl();
      noticeText = m.saved_filter_saved();
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function deleteSavedSessionFilter(filterId: string) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    try {
      const response = await api.deleteSessionFilter(filterId);
      updateConfigSessionOrganization({
        savedFilters: response.savedFilters,
        knownTags: response.knownTags
      });
      if (activeSavedSessionFilterId === filterId) {
        activeSavedSessionFilterId = null;
        syncSessionListStateInUrl();
      }
      noticeText = m.saved_filter_deleted();
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function savePromptPreset(preset: PromptPreset) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    try {
      const response = await api.savePromptPreset(preset);
      if (config) {
        config = {
          ...config,
          promptPresets: response.promptPresets
        };
      }
      noticeText = m.prompt_preset_saved();
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function deletePromptPreset(presetId: string) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    try {
      const response = await api.deletePromptPreset(presetId);
      if (config) {
        config = {
          ...config,
          promptPresets: response.promptPresets
        };
      }
      noticeText = m.prompt_preset_deleted();
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function saveAutomation(automation: AutomationDefinition) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    try {
      const response = await api.saveAutomation(automation);
      if (config) {
        config = {
          ...config,
          automations: {
            ...config.automations,
            items: response.automations
          }
        };
      }
      noticeText = m.automation_saved();
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function deleteAutomation(automationId: string) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    try {
      const response = await api.deleteAutomation(automationId);
      if (config) {
        config = {
          ...config,
          automations: {
            ...config.automations,
            items: response.automations
          }
        };
      }
      noticeText = m.automation_deleted();
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function runAutomation(automationId: string) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    try {
      const response = await api.runAutomation(automationId);
      applySessionSummaryUpdate(response.session);
      await selectSession(response.session.id);
      activeWorkspaceTabId = "chat";
      noticeText = m.automation_started();
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function cleanupAutomationWorktrees() {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    try {
      const response = await api.cleanupAutomationWorktrees(10, false);
      if (config) {
        config = applyLocalComposerPreferencesToConfig(await api.getConfig());
      }
      noticeText = m.automation_worktrees_cleanup_finished();
      if (response.failed > 0) {
        await refreshNotifications();
      }
    } catch (error) {
      errorText = describeError(error);
    }
  }

  function applySlashSuggestion(suggestion: SlashSuggestion) {
    draft = suggestion.value;
    scheduleComposerTextareaResize();
    composerTextareaElement?.focus();
  }

  function updateArchivedSessions(nextValue: boolean) {
    showArchivedSessions = nextValue;
    syncSessionListStateInUrl();
    scheduleSessionRefresh(0);
  }

  async function resolvePendingRequest(request: PendingServerRequest, result: unknown) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    const selectedBinding = ensureSelectedSessionBinding();
    if (!selectedBinding) {
      return;
    }
    try {
      await api.resolveRequest(selectedBinding.sessionId, request.id, result, profileIdForSession(selectedBinding.sessionId));
      if (conversation?.thread.id === selectedBinding.sessionId) {
        conversation = {
          ...conversation,
          pendingRequests: conversation.pendingRequests.filter((pending) => pending.id !== request.id)
        };
        markConversationCacheDirty();
      }
    } catch (error) {
      errorText = describeError(error);
    }
  }

  function setRequestAnswer(requestId: string, questionId: string, value: string) {
    requestAnswers = {
      ...requestAnswers,
      [requestId]: {
        ...(requestAnswers[requestId] ?? {}),
        [questionId]: value
      }
    };
  }

  async function submitRequestUserInput(request: PendingServerRequest) {
    const questions = Array.isArray(request.params.questions) ? (request.params.questions as Array<Record<string, unknown>>) : [];
    const answers = Object.fromEntries(
      questions.map((question) => {
        const questionId = String(question.id);
        return [
          questionId,
          {
            answers: [requestAnswers[request.id]?.[questionId] ?? ""]
          }
        ];
      })
    );
    await resolvePendingRequest(request, { answers });
  }

  async function submitRawRequestResponse(request: PendingServerRequest) {
    try {
      const payload = rawRequestResponses[request.id] ? JSON.parse(rawRequestResponses[request.id]) : {};
      await resolvePendingRequest(request, payload);
    } catch (error) {
      errorText = ui.invalidJsonResponse(describeError(error));
    }
  }

  function getUserText(item: Record<string, unknown>) {
    const content = Array.isArray(item.content) ? (item.content as Array<Record<string, unknown>>) : [];
    const fragments = content
      .map((entry) => formatValue(entry.text) || formatValue(entry.content) || formatValue(entry.value) || formatValue(entry))
      .filter((value) => value.trim().length > 0);

    const contentText = stripAttachmentPreamble(fragments.join("\n\n")).trim();
    if (contentText) {
      return contentText;
    }

    return stripAttachmentPreamble(
      formatValue(item.text) || formatValue(item.message) || formatValue(item.value) || formatValue(item)
    ).trim();
  }

  function getUserClientId(item: Record<string, unknown>) {
    return formatValue(item.clientUserMessageId) || formatValue(item.clientId);
  }

  function getUserAttachmentNames(item: Record<string, unknown>) {
    const names: string[] = [];
    const content = Array.isArray(item.content) ? (item.content as Array<Record<string, unknown>>) : [];
    for (const entry of content) {
      if (entry.type === "localImage" && typeof entry.path === "string") {
        names.push(baseName(entry.path));
      }
      if (entry.type === "text") {
        const textValue = formatValue(entry.text) || formatValue(entry.content) || "";
        if (textValue) {
          names.push(...extractAttachmentPaths(textValue).map((filePath) => baseName(filePath)));
        }
      }
    }
    return names;
  }

  function normalizeMessageForComparison(text: string) {
    return text.replace(/\s+/g, " ").trim();
  }

  function createClientUserMessageId() {
    if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
      return `cu_${crypto.randomUUID()}`;
    }
    return `cu_${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 10)}`;
  }

  function setOptimisticMessageState(
    sessionId: string,
    profileId: string | null,
    clientUserMessageId: string,
    prompt: string,
    skills: SelectedSkill[],
    attachmentNames: string[],
    currentConversation: ConversationState
  ) {
    const normalizedPrompt = prompt.trim();
    const normalizedAttachmentNames = attachmentNames.map((name) => name.trim()).filter(Boolean);
    if (!normalizedPrompt && normalizedAttachmentNames.length === 0) {
      return;
    }

    optimisticMessage = {
      sessionId,
      profileId,
      clientUserMessageId,
      prompt: normalizedPrompt,
      skills: normalizeSelectedSkills(skills),
      attachmentNames: normalizedAttachmentNames,
      createdAt: Date.now(),
      baselineTurnId: currentConversation.thread.turns.at(-1)?.id ?? null,
      baselineTurnCount: currentConversation.thread.turns.length
    };
    requestTranscriptBottomScroll(true);
  }

  function clearOptimisticMessageState(sessionId: string, profileId: string | null, prompt: string | null = null) {
    if (
      !optimisticMessage ||
      sessionStateKey(optimisticMessage.sessionId, optimisticMessage.profileId) !== sessionStateKey(sessionId, profileId)
    ) {
      return;
    }
    if (prompt !== null && normalizeMessageForComparison(optimisticMessage.prompt) !== normalizeMessageForComparison(prompt)) {
      return;
    }
    optimisticMessage = null;
  }

  function buildQueueItemSignature(prompt: string, skills: SelectedSkill[], attachmentIds: string[]) {
    return `${normalizeMessageForComparison(prompt)}\u0000${normalizeSelectedSkills(skills).map((skill) => skill.path).sort().join("\u0001")}\u0000${[...attachmentIds].sort().join("\u0001")}`;
  }

  function buildComposerMutationSignature(
    mode: "message" | "queue" | "steer",
    sessionId: string,
    prompt: string,
    skills: SelectedSkill[],
    attachmentIds: string[]
  ) {
    return `${mode}\u0000${sessionStateKey(sessionId, profileIdForSession(sessionId))}\u0000${buildQueueItemSignature(prompt, skills, attachmentIds)}`;
  }

  function composerCurrentDraftHasPendingMutation(sessionId: string, state: ConversationState) {
    pendingComposerMutationRevision;

    if (!draft.trim() && draftAttachments.length === 0) {
      return false;
    }

    const selectedSkillsSnapshot = [...state.selectedSkills];
    const attachmentIds = draftAttachments.map((attachment) => attachment.id);
    return (["message", "queue", "steer"] as const).some((mode) =>
      pendingComposerMutationSignatures.has(buildComposerMutationSignature(mode, sessionId, draft.trim(), selectedSkillsSnapshot, attachmentIds))
    );
  }

  function beginComposerMutation(signature: string) {
    if (pendingComposerMutationSignatures.has(signature)) {
      return false;
    }

    pendingComposerMutationSignatures.add(signature);
    pendingComposerMutationRevision += 1;
    return true;
  }

  function finishComposerMutation(signature: string | null) {
    if (signature && pendingComposerMutationSignatures.delete(signature)) {
      pendingComposerMutationRevision += 1;
    }
  }

  function addOptimisticQueuedItem(
    sessionId: string,
    profileId: string | null,
    clientUserMessageId: string,
    prompt: string,
    skills: SelectedSkill[],
    attachments: AttachmentRecord[]
  ) {
    const scopeKey = sessionStateKey(sessionId, profileId);
    const optimisticId = `optimistic:${normalizeSessionStateProfileId(profileId)}:${sessionId}:${clientUserMessageId}`;
    const item: SessionQueueItem = {
      id: optimisticId,
      prompt,
      skills: normalizeSelectedSkills(skills),
      attachmentIds: attachments.map((attachment) => attachment.id),
      attachmentNames: attachments.map((attachment) => attachment.originalName),
      createdAt: Date.now(),
      clientRequestId: optimisticId,
      clientUserMessageId
    };

    optimisticQueuedItemsBySessionId = {
      ...optimisticQueuedItemsBySessionId,
      [scopeKey]: [...(optimisticQueuedItemsBySessionId[scopeKey] ?? []), item]
    };
    pendingEnqueuesByOptimisticId.set(optimisticId, {
      sessionId,
      profileId,
      optimisticQueueId: optimisticId,
      item,
      deleted: false,
      edited: false
    });

    return optimisticId;
  }

  function removeOptimisticQueuedItem(
    sessionId: string,
    profileId: string | null,
    queueId: string,
    markDeleted = false
  ) {
    const scopeKey = sessionStateKey(sessionId, profileId);
    const existing = optimisticQueuedItemsBySessionId[scopeKey] ?? [];
    if (markDeleted) {
      const pendingEnqueue = pendingEnqueuesByOptimisticId.get(queueId);
      if (pendingEnqueue) {
        pendingEnqueue.deleted = true;
      }
    }
    if (existing.length === 0) {
      return;
    }

    const nextItems = existing.filter((item) => item.id !== queueId);
    if (nextItems.length === existing.length) {
      return;
    }

    if (nextItems.length === 0) {
      const remaining = { ...optimisticQueuedItemsBySessionId };
      delete remaining[scopeKey];
      optimisticQueuedItemsBySessionId = remaining;
      return;
    }

    optimisticQueuedItemsBySessionId = {
      ...optimisticQueuedItemsBySessionId,
      [scopeKey]: nextItems
    };
  }

  function reconcileOptimisticQueuedItems(sessionId: string, profileId: string | null, queueItems: SessionQueueItem[]) {
    const scopeKey = sessionStateKey(sessionId, profileId);
    const optimisticItems = optimisticQueuedItemsBySessionId[scopeKey] ?? [];
    if (optimisticItems.length === 0) {
      return;
    }

    const realIds = new Set<string>();
    const realCounts = new Map<string, number>();
    const realClientIds = new Set(
      queueItems
        .map((item) => item.clientUserMessageId ?? item.clientRequestId ?? "")
        .filter((value): value is string => value.length > 0)
    );
    for (const item of queueItems) {
      for (const value of [item.id, item.clientRequestId, item.clientUserMessageId]) {
        if (value) {
          realIds.add(value);
        }
      }
      const signature = buildQueueItemSignature(item.prompt, item.skills, item.attachmentIds);
      realCounts.set(signature, (realCounts.get(signature) ?? 0) + 1);
    }

    const nextItems = optimisticItems.filter((item) => {
      const pendingEnqueue = pendingEnqueuesByOptimisticId.get(item.id);
      const matchedRealItem = queueItems.find((candidate) =>
        [candidate.id, candidate.clientRequestId, candidate.clientUserMessageId].some(
          (value) => value && [item.id, item.clientRequestId, item.clientUserMessageId].includes(value)
        )
      );
      if (
        pendingEnqueue &&
        matchedRealItem &&
        buildQueueItemSignature(item.prompt, item.skills, item.attachmentIds) !==
          buildQueueItemSignature(matchedRealItem.prompt, matchedRealItem.skills, matchedRealItem.attachmentIds)
      ) {
        return true;
      }
      const clientUserMessageId = item.clientUserMessageId ?? item.clientRequestId;
      if (
        (clientUserMessageId && realClientIds.has(clientUserMessageId)) ||
        [item.id, item.clientRequestId, item.clientUserMessageId].some((value) => value && realIds.has(value))
      ) {
        return false;
      }
      const signature = buildQueueItemSignature(item.prompt, item.skills, item.attachmentIds);
      const remaining = realCounts.get(signature) ?? 0;
      if (remaining <= 0) {
        return true;
      }

      realCounts.set(signature, remaining - 1);
      return false;
    });

    if (nextItems.length === optimisticItems.length) {
      return;
    }

    if (nextItems.length === 0) {
      const remaining = { ...optimisticQueuedItemsBySessionId };
      delete remaining[scopeKey];
      optimisticQueuedItemsBySessionId = remaining;
      return;
    }

    optimisticQueuedItemsBySessionId = {
      ...optimisticQueuedItemsBySessionId,
      [scopeKey]: nextItems
    };
  }

  function isOptimisticQueueItem(item: SessionQueueItem) {
    return item.id.startsWith("optimistic:");
  }

  function hasConversationEchoedOptimisticMessage(
    currentConversation: ConversationState,
    optimistic: OptimisticMessageState
  ) {
    const targetPrompt = normalizeMessageForComparison(optimistic.prompt);
    const targetAttachments = new Set(optimistic.attachmentNames.map((name) => name.trim()).filter(Boolean));

    if (
      optimistic.clientUserMessageId &&
      currentConversation.thread.turns.some((turn) =>
        turn.items.some((item) => item.type === "userMessage" && getUserClientId(item) === optimistic.clientUserMessageId)
      )
    ) {
      return true;
    }

    return currentConversation.thread.turns.some((turn, turnIndex) => {
      if (turnIndex < optimistic.baselineTurnCount) {
        return false;
      }

      return turn.items.some((item) => {
        if (item.type !== "userMessage") {
          return false;
        }
        if (optimistic.clientUserMessageId && getUserClientId(item) === optimistic.clientUserMessageId) {
          return true;
        }

        const userText = normalizeMessageForComparison(getUserText(item));
        const attachmentNames = getUserAttachmentNames(item);
        const textMatches =
          targetPrompt.length > 0 &&
          (userText === targetPrompt ||
            userText.endsWith(` ${targetPrompt}`) ||
            (targetPrompt.length >= 24 && userText.includes(targetPrompt)));
        const attachmentMatches =
          targetAttachments.size > 0 && attachmentNames.some((attachmentName) => targetAttachments.has(attachmentName.trim()));

        if (targetPrompt.length === 0) {
          return attachmentMatches;
        }
        if (targetAttachments.size === 0) {
          return textMatches;
        }
        return textMatches || attachmentMatches;
      });
    });
  }

  $effect(() => {
    draft;
    scheduleComposerTextareaResize();
  });

  $effect(() => {
    const sessionId = selectedSessionId;
    if (rollbackTargetsResetSessionId === sessionId) {
      return;
    }
    rollbackTargetsResetSessionId = sessionId;
    rollbackTargetsOpen = false;
    rollbackTargetsPayload = null;
    rollbackTargetsSessionId = null;
    rollbackTargetsError = "";
    rollbackTargetsLoading = false;
  });

  async function copyMessageText(text: string) {
    if (typeof navigator === "undefined" || !text.trim()) {
      return;
    }
    try {
      await navigator.clipboard.writeText(text);
      noticeText = m.copied_to_clipboard();
    } catch (error) {
      errorText = describeError(error);
    }
  }

  function editMessageText(text: string) {
    draft = text;
    activeWorkspaceTabId = "chat";
    requestTranscriptBottomScroll(true);
  }

  function summarizeTurnForRollbackPreview(turn: CodexTurn) {
    const userText = turn.items.map((item) => (item.type === "userMessage" ? getUserText(item) : "")).find((value) => value.trim());
    const agentText = turn.items
      .map((item) => (item.type === "agentMessage" && typeof item.text === "string" ? item.text : ""))
      .find((value) => value.trim());
    const text = (userText || agentText || turn.id).replace(/\s+/gu, " ").trim();
    return text.length > 96 ? `${text.slice(0, 96).trimEnd()}...` : text;
  }

  function getLoadedAffectedTurnsForRollback(numTurns: number) {
    if (!conversation || numTurns <= 0) {
      return [];
    }
    return conversation.thread.turns.slice(Math.max(0, conversation.thread.turns.length - numTurns));
  }

  function getRollbackFilePreviews(turns: CodexTurn[]) {
    const filePreviews: Array<{ path: string; added: number; removed: number }> = [];
    for (const turn of turns) {
      for (const item of turn.items) {
        if (item.type !== "fileChange") {
          continue;
        }
        for (const change of getFileChangeViews(item)) {
          const stats = diffLineStats(change.diff);
          const path = change.movePath ? `${change.path} -> ${change.movePath}` : change.path;
          const existing = filePreviews.find((entry) => entry.path === path);
          if (existing) {
            existing.added += stats.added;
            existing.removed += stats.removed;
          } else {
            filePreviews.push({ path, added: stats.added, removed: stats.removed });
          }
        }
      }
    }
    return filePreviews;
  }

  async function loadRollbackTargets(force = false) {
    const sessionId = selectedSessionId;
    if (!sessionId) {
      return;
    }
    if (!force && rollbackTargetsPayload && rollbackTargetsSessionId === sessionId) {
      return;
    }
    rollbackTargetsLoading = true;
    rollbackTargetsError = "";
    try {
      const payload = await api.listRollbackTargets(sessionId, profileIdForSession(sessionId));
      if (selectedSessionId !== sessionId) {
        return;
      }
      rollbackTargetsPayload = payload;
      rollbackTargetsSessionId = sessionId;
    } catch (error) {
      if (selectedSessionId === sessionId) {
        rollbackTargetsError = describeError(error);
      }
    } finally {
      if (selectedSessionId === sessionId) {
        rollbackTargetsLoading = false;
      }
    }
  }

  async function toggleRollbackTargetsPanel() {
    rollbackTargetsOpen = !rollbackTargetsOpen;
    if (rollbackTargetsOpen) {
      await loadRollbackTargets();
    }
  }

  async function rollbackCurrentThreadToTarget(target: SessionRollbackTarget) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    if (!selectedSessionId || !conversation) {
      return;
    }
    const numTurns = target.numTurns;
    if (numTurns <= 0) {
      noticeText = m.rollback_no_later_turns();
      return;
    }
    const affectedTurns = getLoadedAffectedTurnsForRollback(numTurns);
    const affectedFullyLoaded = affectedTurns.length === numTurns;
    const previewLines = affectedTurns.slice(0, 6).map((turn, index) => `${index + 1}. ${summarizeTurnForRollbackPreview(turn)}`);
    if (affectedTurns.length > previewLines.length) {
      previewLines.push(`... +${affectedTurns.length - previewLines.length}`);
    }
    const filePreviews = getRollbackFilePreviews(affectedTurns);
    const filePreviewLines = filePreviews.slice(0, 8).map((entry) => {
      const stats = entry.added || entry.removed ? ` (+${entry.added}/-${entry.removed})` : "";
      return `- ${entry.path}${stats}`;
    });
    if (filePreviews.length > filePreviewLines.length) {
      filePreviewLines.push(`... +${filePreviews.length - filePreviewLines.length}`);
    }
    const affectedSection =
      previewLines.length > 0
        ? `\n\n${ui.rollbackTurnsCount} (${numTurns}):\n${previewLines.join("\n")}`
        : `\n\n${ui.rollbackTurnsCount}: ${numTurns}`;
    const incompleteSection = affectedFullyLoaded ? "" : `\n\n${ui.rollbackPreviewIncomplete}`;
    const filePreviewSection =
      filePreviews.length > 0
        ? `\n\nFile changes in loaded affected turns (${filePreviews.length}):\n${filePreviewLines.join("\n")}`
        : "\n\nFile changes in loaded affected turns: none";
    const confirmMessage = `${ui.rollbackConfirm}\n\nTarget: ${target.preview}${affectedSection}${incompleteSection}${filePreviewSection}`;
    if (!window.confirm(confirmMessage)) {
      return;
    }
    const sessionId = selectedSessionId;
    try {
      const response = await api.rollbackSession(sessionId, numTurns, profileIdForSession(sessionId));
      if (selectedSessionId === sessionId && conversation) {
        conversation = normalizeConversationExecutionState({
          ...conversation,
          thread: {
            ...conversation.thread,
            ...response.thread
          },
          activeTurnId: null
        });
        applySessionSummaryUpdate(buildSessionSummaryFromConversation(conversation));
      }
      rollbackTargetsPayload = null;
      rollbackTargetsSessionId = null;
      rollbackTargetsOpen = false;
      scheduleSessionRefresh(80);
      scheduleSelectedSessionStateRefresh(sessionId, 80);
      noticeText = m.rollback_complete();
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function forkCurrentThread(
    mode: "fork" | "handoff",
    options: {
      turnId?: string | null;
      messageText?: string | null;
    } = {}
  ) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    if (!selectedSessionId) {
      return;
    }
    try {
      const response = await api.forkSession(
        selectedSessionId,
        {
          mode,
          turnId: options.turnId ?? null,
          messageText: options.messageText ?? null
        },
        profileIdForSession(selectedSessionId)
      );
      upsertSessionSummary(response.session);
      await selectSession(response.session.id);
      draft = response.draft;
      scheduleComposerTextareaResize();
      noticeText = mode === "handoff" ? m.opened_handoff_thread() : m.opened_branch_thread();
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function rollbackCurrentThreadToTurn(turnId: string) {
    if (readOnlyRole) {
      errorText = m.error_forbidden_role();
      return;
    }
    if (!selectedSessionId || !conversation) {
      return;
    }
    const turnIndex = conversation.thread.turns.findIndex((turn) => turn.id === turnId);
    if (turnIndex === -1) {
      return;
    }
    const numTurns = conversation.thread.turns.length - turnIndex - 1;
    if (numTurns <= 0) {
      noticeText = m.rollback_no_later_turns();
      return;
    }
    const affectedTurns = conversation.thread.turns.slice(turnIndex + 1);
    const previewLines = affectedTurns.slice(0, 6).map((turn, index) => `${index + 1}. ${summarizeTurnForRollbackPreview(turn)}`);
    if (affectedTurns.length > previewLines.length) {
      previewLines.push(`... +${affectedTurns.length - previewLines.length}`);
    }
    const filePreviews = getRollbackFilePreviews(affectedTurns);
    const filePreviewLines = filePreviews.slice(0, 8).map((entry) => {
      const stats = entry.added || entry.removed ? ` (+${entry.added}/-${entry.removed})` : "";
      return `- ${entry.path}${stats}`;
    });
    if (filePreviews.length > filePreviewLines.length) {
      filePreviewLines.push(`... +${filePreviews.length - filePreviewLines.length}`);
    }
    const filePreviewSection =
      filePreviews.length > 0
        ? `\n\nFile changes in affected turns (${filePreviews.length}):\n${filePreviewLines.join("\n")}`
        : "\n\nFile changes in affected turns: none";
    const confirmMessage = `${ui.rollbackConfirm}\n\n${numTurns} turn${numTurns === 1 ? "" : "s"}:\n${previewLines.join("\n")}${filePreviewSection}`;
    if (!window.confirm(confirmMessage)) {
      return;
    }
    const sessionId = selectedSessionId;
    try {
      const response = await api.rollbackSession(sessionId, numTurns, profileIdForSession(sessionId));
      if (selectedSessionId === sessionId && conversation) {
        conversation = normalizeConversationExecutionState({
          ...conversation,
          thread: {
            ...conversation.thread,
            ...response.thread
          },
          activeTurnId: null
        });
        applySessionSummaryUpdate(buildSessionSummaryFromConversation(conversation));
      }
      scheduleSessionRefresh(80);
      scheduleSelectedSessionStateRefresh(sessionId, 80);
      noticeText = m.rollback_complete();
    } catch (error) {
      errorText = describeError(error);
    }
  }

  function updateRawRequestResponse(requestId: string, value: string) {
    rawRequestResponses = {
      ...rawRequestResponses,
      [requestId]: value
    };
  }

  function baseName(filePath: string) {
    return filePath.split(/[/\\]/).filter(Boolean).at(-1) ?? filePath;
  }

  function formatSize(size: number) {
    if (size > 1024 * 1024) {
      return `${(size / (1024 * 1024)).toFixed(1)} MB`;
    }
    if (size > 1024) {
      return `${Math.round(size / 1024)} KB`;
    }
    return `${size} B`;
  }

  function formatTokenCount(value: number | null | undefined) {
    if (typeof value !== "number" || !Number.isFinite(value)) {
      return "0";
    }
    return new Intl.NumberFormat("en-US").format(value);
  }

  function getContextUsageIndicator() {
    const tokenUsage = conversation?.tokenUsage;
    if (!tokenUsage?.modelContextWindow || tokenUsage.modelContextWindow <= 0) {
      return null;
    }

    const usedTokens = Math.max(0, tokenUsage.total.totalTokens ?? 0);
    const percent = Math.max(0, Math.min(100, Math.round((usedTokens / tokenUsage.modelContextWindow) * 100)));
    return {
      label: `${percent}%`,
      percent,
      tooltip: `${formatTokenCount(usedTokens)} / ${formatTokenCount(tokenUsage.modelContextWindow)} · ${percent}%`
    };
  }

  function summarizeQueueItem(item: SessionQueueItem) {
    const text = item.prompt.trim();
    if (text) {
      return text.length > 140 ? `${text.slice(0, 140).trimEnd()}…` : text;
    }
    if (item.attachmentNames.length > 0) {
      return item.attachmentNames.join(", ");
    }
    return "...";
  }

  function resetComposerHistoryNavigation() {
    composerHistoryIndex = -1;
    composerHistoryDraft = "";
  }

  function rememberLastComposerPromptChip(sessionId: string, prompt: string) {
    const normalizedPrompt = prompt.trim();
    if (!sessionId || !normalizedPrompt) {
      return;
    }

    lastComposerPromptChip = {
      sessionId,
      prompt: normalizedPrompt
    };
  }

  function dismissLastComposerPromptChip() {
    lastComposerPromptChip = null;
  }

  function recordComposerHistory(prompt: string) {
    const normalizedPrompt = prompt.trim();
    if (!normalizedPrompt) {
      return;
    }
    composerHistory =
      composerHistory.at(-1) === normalizedPrompt
        ? composerHistory
        : [...composerHistory.slice(-49), normalizedPrompt];
    resetComposerHistoryNavigation();
  }

  function resizeComposerTextarea() {
    const textarea = composerTextareaElement;
    if (!textarea || typeof window === "undefined") {
      return;
    }
    const minHeight = composerTextareaMinHeight;
    const maxHeight = 240;
    textarea.style.height = "0px";
    const nextHeight = Math.max(minHeight, Math.min(textarea.scrollHeight, maxHeight));
    textarea.style.height = `${nextHeight}px`;
    textarea.style.overflowY = textarea.scrollHeight > maxHeight ? "auto" : "hidden";
    syncTranscriptDockReserve();
  }

  function syncTranscriptDockReserve() {
    const dock = transcriptDockElement;
    if (!dock) {
      return;
    }

    const textareaHeight = composerTextareaElement?.offsetHeight ?? composerTextareaMinHeight;
    const overlap = Math.max(0, textareaHeight - composerTextareaMinHeight);
    const nextReserve = Math.max(152, Math.round(dock.offsetHeight - overlap));

    if (Math.abs(nextReserve - transcriptDockReservePx) <= 1) {
      return;
    }

    transcriptDockReservePx = nextReserve;
    if (!loadingOlderTurns && (stickTranscriptToBottom || forceTranscriptScroll)) {
      scheduleTranscriptScrollToBottom();
    }
  }

  function scheduleComposerTextareaResize() {
    if (typeof window === "undefined") {
      return;
    }
    if (composerTextareaResizeFrame !== null) {
      cancelAnimationFrame(composerTextareaResizeFrame);
    }
    composerTextareaResizeFrame = window.requestAnimationFrame(() => {
      composerTextareaResizeFrame = null;
      resizeComposerTextarea();
    });
  }

  function closeFileMentionSearch() {
    if (fileMentionSearchTimer !== null) {
      clearTimeout(fileMentionSearchTimer);
      fileMentionSearchTimer = null;
    }
    fileMentionTrigger = null;
    fileMentionResults = [];
    fileMentionBusy = false;
    fileMentionActiveIndex = 0;
  }

  function detectFileMentionTrigger(value: string, cursor: number): FileMentionTrigger | null {
    if (cursor < 1 || cursor > value.length) {
      return null;
    }

    let start = cursor;
    while (start > 0 && !/\s/u.test(value[start - 1] ?? "")) {
      start -= 1;
    }

    const token = value.slice(start, cursor);
    if (!token.startsWith("@") || token.includes("\n") || token.includes("\r")) {
      return null;
    }
    const previous = value[start - 1] ?? "";
    if (start > 0 && !/\s/u.test(previous) && !'([{"\'`,;'.includes(previous)) {
      return null;
    }

    const query = token.slice(1);
    if (query.includes("@")) {
      return null;
    }

    return {
      start,
      end: cursor,
      query
    };
  }

  function scheduleFileMentionSearch() {
    if (readOnlyRole || typeof window === "undefined") {
      closeFileMentionSearch();
      return;
    }

    window.queueMicrotask(() => {
      const textarea = composerTextareaElement;
      if (!textarea) {
        closeFileMentionSearch();
        return;
      }

      const selectionStart = textarea.selectionStart ?? draft.length;
      const selectionEnd = textarea.selectionEnd ?? draft.length;
      if (selectionStart !== selectionEnd) {
        closeFileMentionSearch();
        return;
      }

      const trigger = detectFileMentionTrigger(draft, selectionStart);
      if (!trigger) {
        closeFileMentionSearch();
        return;
      }

      fileMentionTrigger = trigger;
      fileMentionBusy = true;
      fileMentionActiveIndex = 0;
      if (fileMentionSearchTimer !== null) {
        clearTimeout(fileMentionSearchTimer);
      }

      const requestVersion = ++fileMentionRequestVersion;
      fileMentionSearchTimer = setTimeout(async () => {
        fileMentionSearchTimer = null;
        try {
          const payload = await api.searchFileMentions(
            trigger.query,
            conversation?.preferences.cwd ?? conversation?.thread.cwd ?? config?.defaults.cwd ?? null,
            12
          );
          if (requestVersion !== fileMentionRequestVersion) {
            return;
          }
          fileMentionResults = payload.entries;
          fileMentionActiveIndex = 0;
        } catch {
          if (requestVersion !== fileMentionRequestVersion) {
            return;
          }
          fileMentionResults = [];
        } finally {
          if (requestVersion === fileMentionRequestVersion) {
            fileMentionBusy = false;
          }
        }
      }, 120);
    });
  }

  async function insertFileMention(entry: FileMentionSearchEntry) {
    const trigger = fileMentionTrigger;
    if (!trigger) {
      return;
    }

    const insertion = `@${entry.relativePath} `;
    draft = `${draft.slice(0, trigger.start)}${insertion}${draft.slice(trigger.end)}`;
    closeFileMentionSearch();
    scheduleComposerTextareaResize();
    await tick();
    const cursor = trigger.start + insertion.length;
    composerTextareaElement?.focus();
    composerTextareaElement?.setSelectionRange(cursor, cursor);
  }

  function handleComposerInput() {
    scheduleComposerTextareaResize();
    scheduleFileMentionSearch();
    if (composerHistoryIndex !== -1) {
      resetComposerHistoryNavigation();
    }
  }

  function handleComposerHistoryNavigation(direction: "up" | "down") {
    if (composerHistory.length === 0) {
      return;
    }

    if (direction === "up") {
      if (composerHistoryIndex === -1) {
        composerHistoryDraft = draft;
        composerHistoryIndex = composerHistory.length - 1;
      } else if (composerHistoryIndex > 0) {
        composerHistoryIndex -= 1;
      }
      draft = composerHistory[composerHistoryIndex] ?? draft;
      return;
    }

    if (composerHistoryIndex === -1) {
      return;
    }

    if (composerHistoryIndex >= composerHistory.length - 1) {
      draft = composerHistoryDraft;
      resetComposerHistoryNavigation();
      return;
    }

    composerHistoryIndex += 1;
    draft = composerHistory[composerHistoryIndex] ?? draft;
  }

  function handleComposerKeydown(event: KeyboardEvent) {
    if (event.isComposing) {
      return;
    }

    if (fileMentionTrigger) {
      if (event.key === "Escape") {
        event.preventDefault();
        closeFileMentionSearch();
        return;
      }
      if (fileMentionResults.length > 0 && (event.key === "ArrowDown" || event.key === "ArrowUp")) {
        event.preventDefault();
        const delta = event.key === "ArrowDown" ? 1 : -1;
        fileMentionActiveIndex = (fileMentionActiveIndex + delta + fileMentionResults.length) % fileMentionResults.length;
        return;
      }
      if (fileMentionResults.length > 0 && (event.key === "Enter" || event.key === "Tab")) {
        event.preventDefault();
        void insertFileMention(fileMentionResults[fileMentionActiveIndex] ?? fileMentionResults[0]);
        return;
      }
    }

    if (event.key === "Enter" && event.repeat) {
      event.preventDefault();
      return;
    }

    if (
      conversation?.preferences.sendOnEnter &&
      event.key === "Enter" &&
      !event.altKey &&
      !event.shiftKey &&
      !event.ctrlKey &&
      !event.metaKey
    ) {
      event.preventDefault();
      if (composerQueueModeActive) {
        void queueMessage();
        return;
      }
      void submitComposer();
      return;
    }

    if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
      event.preventDefault();
      if (composerQueueModeActive) {
        void queueMessage();
        return;
      }
      void submitComposer();
      return;
    }

    if (event.altKey || event.shiftKey || event.ctrlKey || event.metaKey) {
      return;
    }

    if (event.key !== "ArrowUp" && event.key !== "ArrowDown") {
      return;
    }

    const textarea = event.currentTarget as HTMLTextAreaElement;
    const selectionStart = textarea.selectionStart ?? draft.length;
    const selectionEnd = textarea.selectionEnd ?? draft.length;
    if (selectionStart !== selectionEnd) {
      return;
    }

    const before = draft.slice(0, selectionStart);
    const after = draft.slice(selectionEnd);
    const isAtFirstLine = !before.includes("\n");
    const isAtLastLine = !after.includes("\n");

    if ((event.key === "ArrowUp" && !isAtFirstLine) || (event.key === "ArrowDown" && !isAtLastLine)) {
      return;
    }

    event.preventDefault();
    handleComposerHistoryNavigation(event.key === "ArrowUp" ? "up" : "down");
  }

  async function openSubagentThread(threadId: string) {
    try {
      if (typeof window !== "undefined") {
        const nextUrl = new URL(window.location.href);
        nextUrl.searchParams.set(sessionQueryParamKey, threadId);
        nextUrl.searchParams.delete(sessionNewParamKey);
        window.open(nextUrl.toString(), "_blank", "noopener,noreferrer");
        return;
      }

      await refreshSessions();
      await selectSession(threadId);
    } catch (error) {
      errorText = describeError(error);
    }
  }

  function describeError(value: unknown) {
    return describeUiError(value);
  }

  function formatValue(value: unknown, depth = 0): string {
    if (depth > 4) {
      return "";
    }

    if (typeof value === "string") {
      return value;
    }

    if (Array.isArray(value)) {
      const fragments = value.map((entry) => formatValue(entry, depth + 1)).filter((entry) => entry.trim().length > 0);
      return fragments.join("\n").trim();
    }

    if (!value || typeof value !== "object") {
      return "";
    }

    for (const key of ["text", "title", "value", "name", "message", "content"]) {
      const candidate = (value as Record<string, unknown>)[key];
      if (typeof candidate === "string") {
        return candidate;
      }
      const nested = formatValue(candidate, depth + 1);
      if (nested) {
        return nested;
      }
    }

    return "";
  }

  function summarizeCommand(item: Record<string, unknown>) {
    const command = item.command;
    if (Array.isArray(command)) {
      const fragments = command
        .map((entry) => (typeof entry === "string" ? entry : formatValue(entry)))
        .map((entry) => entry.trim())
        .filter((entry) => entry.length > 0);
      return fragments.join(" ").trim() || m.command();
    }

    return formatValue(command).replace(/\s+/g, " ").trim() || m.command();
  }

  function getParsedCommands(item: Record<string, unknown>) {
    const parsedCommands = Array.isArray(item.parsed_cmd)
      ? item.parsed_cmd
      : Array.isArray(item.parsedCmd)
        ? item.parsedCmd
        : [];
    return parsedCommands.filter((entry): entry is Record<string, unknown> => Boolean(entry) && typeof entry === "object");
  }

  function isReadOnlyShellCommand(item: Record<string, unknown>) {
    return getSpecialShellCommandKind(item) !== null;
  }

  function getSpecialShellCommandKind(item: Record<string, unknown>) {
    if (item.type !== "commandExecution") {
      return null;
    }

    const parsedCommands = getParsedCommands(item);
    if (parsedCommands.length > 0) {
      const parsedTypes = parsedCommands.map((entry) => String(entry.type ?? ""));
      if (parsedTypes.every((type) => readOnlyParsedCommandTypes.has(type))) {
        if (parsedTypes.some((type) => type === "list_files" || type === "search")) {
          return "search" as const;
        }
        return "read" as const;
      }
      if (parsedTypes.some((type) => type && type !== "unknown" && !readOnlyParsedCommandTypes.has(type))) {
        return null;
      }
    }

    const commandParts = Array.isArray(item.command)
      ? item.command
          .map((entry) => (typeof entry === "string" ? entry : formatValue(entry)))
          .map((entry) => entry.trim())
          .filter((entry) => entry.length > 0)
      : [];
    let command = summarizeCommand(item).trim();
    if (!command || />|>>|<<?|\btee\b|\brm\b|\bmv\b|\bcp\b|\btouch\b|\bmkdir\b|\bapply_patch\b|\bpython3?\b/iu.test(command)) {
      return null;
    }

    if (
      commandParts.length >= 3 &&
      /(?:^|\/)(?:bash|sh)$/iu.test(commandParts[0] ?? "") &&
      /^-l?c$/u.test(commandParts[1] ?? "")
    ) {
      command = commandParts.slice(2).join(" ").trim();
    } else {
      command = command
        .replace(/^(?:(?:\S+\/)?(?:bash|sh)|\/usr\/bin\/env\s+(?:\S+\/)?(?:bash|sh))\s+-l?c\s+/iu, "")
        .trim();
    }

    while (
      command.length >= 2 &&
      ((command.startsWith('"') && command.endsWith('"')) || (command.startsWith("'") && command.endsWith("'")))
    ) {
      command = command.slice(1, -1).trim();
    }

    if (!command || /(?:&&|\|\|)/u.test(command)) {
      return null;
    }

    const parts = command
      .split("|")
      .map((part) => part.trim())
      .filter(Boolean);

    if (parts.length === 0) {
      return null;
    }

    let sawGit = false;
    let sawSearch = false;
    let sawRead = false;

    for (const part of parts) {
      if (/^(?:\S+\/)?git\b/iu.test(part)) {
        const gitTokens = part.match(/"[^"]*"|'[^']*'|\S+/g) ?? [];
        let tokenIndex = 1;

        while (tokenIndex < gitTokens.length) {
          const token = gitTokens[tokenIndex] ?? "";
          if (token === "-C" || token === "-c" || token === "--git-dir" || token === "--work-tree") {
            tokenIndex += 2;
            continue;
          }
          if (token.startsWith("-")) {
            tokenIndex += 1;
            continue;
          }
          break;
        }

        const gitSubcommand = (gitTokens[tokenIndex] ?? "").replace(/^['"]|['"]$/g, "").toLowerCase();
        if (
          [
            "status",
            "diff",
            "show",
            "log",
            "reflog",
            "branch",
            "rev-parse",
            "remote",
            "ls-files",
            "symbolic-ref",
            "blame",
            "grep"
          ].includes(gitSubcommand)
        ) {
          sawGit = true;
          continue;
        }
        return null;
      }
      if (/^(pwd|sed|cat|head|tail|wc|sort|stat)\b/iu.test(part)) {
        sawRead = true;
        continue;
      }
      if (/^(ls|find|rg|tree)\b/iu.test(part)) {
        sawSearch = true;
        continue;
      }
      return null;
    }

    if (sawGit) {
      return "git" as const;
    }
    if (sawSearch) {
      return "search" as const;
    }
    if (sawRead) {
      return "read" as const;
    }
    return null;
  }

  function getReadOnlyCommandTarget(item: Record<string, unknown>) {
    const parsedCommands = getParsedCommands(item);
    const targets = parsedCommands
      .map((entry) => formatValue(entry.name) || formatValue(entry.path) || formatValue(entry.cmd))
      .filter((value) => value.trim().length > 0);
    return [...new Set(targets)];
  }

  function summarizeReadOnlyCommandGroup(items: CodexItem[]) {
    const targets = items.flatMap((item) => getReadOnlyCommandTarget(item));
    const uniqueTargets = [...new Set(targets)];
    if (uniqueTargets.length === 0) {
      return items.length === 1 ? summarizeCommand(items[0]) : m.read_commands_count({ count: String(items.length) });
    }
    if (uniqueTargets.length === 1) {
      return uniqueTargets[0];
    }
    if (uniqueTargets.length === 2) {
      return `${uniqueTargets[0]}, ${uniqueTargets[1]}`;
    }
    return `${uniqueTargets[0]}, ${uniqueTargets[1]} +${uniqueTargets.length - 2}`;
  }

  function getReadOnlyCommandGroupKind(items: CodexItem[]) {
    const specialKinds = items
      .map((item) => getSpecialShellCommandKind(item))
      .filter((kind): kind is "read" | "search" | "git" => Boolean(kind));
    if (specialKinds.length > 0 && specialKinds.every((kind) => kind === "git")) {
      return "git" as const;
    }
    if (specialKinds.length > 0 && specialKinds.every((kind) => kind === "search")) {
      return "search" as const;
    }
    if (specialKinds.length > 0 && specialKinds.every((kind) => kind === "read")) {
      return "read" as const;
    }

    const parsedTypes = items.flatMap((item) => getParsedCommands(item).map((entry) => String(entry.type ?? "")));
    if (parsedTypes.length > 0 && parsedTypes.every((type) => type === "read")) {
      return "read" as const;
    }
    if (parsedTypes.some((type) => type === "list_files" || type === "search")) {
      return "search" as const;
    }
    return "inspect" as const;
  }

  function getReadOnlyCommandGroupLabel(items: CodexItem[]) {
    const kind = getReadOnlyCommandGroupKind(items);
    if (kind === "git") {
      return m.git();
    }
    if (kind === "search") {
      return m.search();
    }
    if (kind === "read") {
      return m.read();
    }
    return m.inspect();
  }

  function getFileChangeSummaryEntries(item: Record<string, unknown>) {
    const changes = Array.isArray(item.changes) ? item.changes : [];
    const summaries: FileChangeSummaryEntry[] = [];

    for (const change of changes) {
      if (!change || typeof change !== "object") {
        continue;
      }
      const record = change as Record<string, unknown>;
      const kindRecord = record.kind && typeof record.kind === "object" ? (record.kind as Record<string, unknown>) : null;
      const kindType =
        typeof record.kind === "string"
          ? record.kind
          : typeof kindRecord?.type === "string"
            ? kindRecord.type
            : "update";

      summaries.push({
        path: typeof record.path === "string" && record.path.trim() ? record.path : m.code_edit(),
        kind: kindType === "add" || kindType === "delete" ? kindType : "update",
        movePath:
          typeof kindRecord?.movePath === "string"
            ? kindRecord.movePath
            : typeof kindRecord?.move_path === "string"
              ? kindRecord.move_path
              : null
      });
    }

    return summaries;
  }

  function getFileChangeViews(item: Record<string, unknown>) {
    const changes = Array.isArray(item.changes) ? item.changes : [];
    const views: FileChangeView[] = [];

    for (const change of changes) {
      if (!change || typeof change !== "object") {
        continue;
      }

      const record = change as Record<string, unknown>;
      const kindRecord = record.kind && typeof record.kind === "object" ? (record.kind as Record<string, unknown>) : null;
      const kindType =
        typeof record.kind === "string"
          ? record.kind
          : typeof kindRecord?.type === "string"
            ? kindRecord.type
            : "update";
      const kind = kindType === "add" || kindType === "delete" ? kindType : "update";
      const movePath =
        typeof kindRecord?.movePath === "string"
          ? kindRecord.movePath
          : typeof kindRecord?.move_path === "string"
            ? kindRecord.move_path
            : null;
      const diff =
        (
          typeof record.diff === "string"
            ? record.diff
            : typeof record.unifiedDiff === "string"
              ? record.unifiedDiff
              : typeof record.unified_diff === "string"
                ? record.unified_diff
                : typeof record.content === "string"
                  ? record.content
                  : ""
        ).replace(/\r\n/g, "\n");
      views.push(
        buildDiffView(typeof record.path === "string" && record.path.trim() ? record.path : m.code_edit(), kind, movePath, diff)
      );
    }

    return views;
  }

  function getFileChangeGroupSummaryEntries(items: CodexItem[]) {
    return items.flatMap((item) => (item.type === "fileChange" ? getFileChangeSummaryEntries(item) : []));
  }

  function getFileChangeGroupViews(items: CodexItem[]) {
    return items.flatMap((item) => (item.type === "fileChange" ? getFileChangeViews(item) : []));
  }

  function getFileChangeGroupLabel(items: CodexItem[]) {
    const count = getFileChangeGroupSummaryEntries(items).length;
    return count === 1 ? m.one_file_changed() : count > 1 ? m.files_changed({ count: String(count) }) : m.files_changed_fallback();
  }

  function summarizeFileChangeGroup(items: CodexItem[]) {
    const changes = getFileChangeGroupSummaryEntries(items);
    if (changes.length === 0) {
      return m.code_edit();
    }
    if (changes.length === 1) {
      return changes[0].path || m.code_edit();
    }
    if (changes.length === 2) {
      return `${changes[0].path}, ${changes[1].path}`;
    }
    return `${changes[0].path}, ${changes[1].path} +${changes.length - 2}`;
  }

  function summarizeFileChanges(item: Record<string, unknown>) {
    const changes = getFileChangeSummaryEntries(item);
    if (changes.length === 0) {
      return m.code_edit();
    }
    if (changes.length === 1) {
      return changes[0].path || m.code_edit();
    }
    if (changes.length === 2) {
      return `${changes[0].path}, ${changes[1].path}`;
    }
    return `${changes[0].path}, ${changes[1].path} +${changes.length - 2}`;
  }

  function getSubagentStates(item: Record<string, unknown>) {
    if (!item.agentsStates || typeof item.agentsStates !== "object") {
      return [] as Array<[string, { status?: string; message?: string | null }]>;
    }

    return Object.entries(item.agentsStates as Record<string, { status?: string; message?: string | null }>);
  }

  function getPrimarySubagentThreadId(item: Record<string, unknown>) {
    return Array.isArray(item.receiverThreadIds) && typeof item.receiverThreadIds[0] === "string"
      ? item.receiverThreadIds[0]
      : null;
  }

  function getRenderableTurnEntries(items: CodexItem[]): RenderableTurnEntry[] {
    const entries: RenderableTurnEntry[] = [];
    let readOnlyBuffer: CodexItem[] = [];
    let fileChangeBuffer: CodexItem[] = [];

    const flushReadOnlyBuffer = () => {
      if (readOnlyBuffer.length === 0) {
        return;
      }

      entries.push({
        kind: "readGroup",
        key: `read-group:${readOnlyBuffer[0]?.id ?? Math.random().toString(36)}:${readOnlyBuffer.at(-1)?.id ?? ""}`,
        items: readOnlyBuffer
      });
      readOnlyBuffer = [];
    };

    const flushFileChangeBuffer = () => {
      if (fileChangeBuffer.length === 0) {
        return;
      }

      entries.push({
        kind: "fileChangeGroup",
        key: `file-change-group:${fileChangeBuffer[0]?.id ?? Math.random().toString(36)}:${fileChangeBuffer.at(-1)?.id ?? ""}`,
        items: fileChangeBuffer
      });
      fileChangeBuffer = [];
    };

    for (const item of items) {
      if (isReadOnlyShellCommand(item)) {
        flushFileChangeBuffer();
        readOnlyBuffer.push(item);
        continue;
      }

      if (item.type === "fileChange") {
        flushReadOnlyBuffer();
        fileChangeBuffer.push(item);
        continue;
      }

      flushReadOnlyBuffer();
      flushFileChangeBuffer();
      entries.push({
        kind: "item",
        key: item.id,
        item
      });
    }

    flushReadOnlyBuffer();
    flushFileChangeBuffer();
    return entries;
  }

  const turnRenderModelCache = new WeakMap<CodexTurn, TurnRenderModel>();

  function getTurnRenderModel(turn: CodexTurn): TurnRenderModel {
    const cached = turnRenderModelCache.get(turn);
    if (cached) {
      return cached;
    }

    let finalAgentItem: CodexItem | null = null;
    const userItems: CodexItem[] = [];
    const collapsedItems: CodexItem[] = [];
    const summaryItems: CodexItem[] = [];

    for (const item of turn.items) {
      if (item.type === "userMessage") {
        userItems.push(item);
      }
      if (item.type === "agentMessage") {
        finalAgentItem = item;
      }
      if (item.type === "plan" || item.type === "fileChange" || item.type === "imageGeneration") {
        summaryItems.push(item);
      }
    }

    if (finalAgentItem) {
      for (const item of turn.items) {
        if (
          item.type !== "userMessage" &&
          !isInternalTranscriptItem(item) &&
          item.type !== "plan" &&
          item.type !== "fileChange" &&
          item.type !== "imageGeneration" &&
          item.id !== finalAgentItem.id
        ) {
          collapsedItems.push(item);
        }
      }
    }

    const model: TurnRenderModel = {
      userItems,
      finalAgentItem,
      collapsedItems,
      collapsedEntries: getRenderableTurnEntries(collapsedItems),
      visibleSummaryEntries: getRenderableTurnEntries(summaryItems),
      fullEntries: getRenderableTurnEntries(turn.items.filter((item) => item.type !== "userMessage" && !isInternalTranscriptItem(item)))
    };
    turnRenderModelCache.set(turn, model);
    return model;
  }

  function getTurnEntryRenderLimit(turnId: string) {
    return turnEntryRenderLimits[turnId] ?? initialTurnEntryRenderLimit;
  }

  function getVisibleTurnEntries(turnId: string, entries: RenderableTurnEntry[]) {
    const limit = getTurnEntryRenderLimit(turnId);
    if (entries.length <= limit) {
      return entries;
    }
    return entries.slice(entries.length - limit);
  }

  function getHiddenTurnEntryCount(turnId: string, entries: RenderableTurnEntry[]) {
    return Math.max(0, entries.length - getTurnEntryRenderLimit(turnId));
  }

  function showMoreTurnEntries(turnId: string, entries: RenderableTurnEntry[]) {
    const currentLimit = getTurnEntryRenderLimit(turnId);
    turnEntryRenderLimits = {
      ...turnEntryRenderLimits,
      [turnId]: Math.min(entries.length, currentLimit + turnEntryRenderIncrement)
    };
  }

  function handleTranscriptWheel() {
    noteTranscriptUserScrollIntent();
  }

  function handleTranscriptTouchMove() {
    noteTranscriptUserScrollIntent(1200);
  }

  function handleTranscriptPointerMove(event: PointerEvent) {
    if (event.buttons > 0) {
      noteTranscriptUserScrollIntent(1200);
    }
  }

  function handleTranscriptScroll() {
    if (!transcriptElement) {
      return;
    }

    refreshTranscriptTurnWindow();

    const userScrollIntent = hasTranscriptUserScrollIntent();
    const initialScrollPending = isInitialTranscriptScrollPending();
    if (isTranscriptAtBottom()) {
      transcriptAutoScrollSuspendedByUser = false;
      stickTranscriptToBottom = true;
      clearInitialTranscriptScrollPending();
      if (!loadingDetail && !loadingOlderTurns) {
        pendingTranscriptBottomScroll = false;
      }
    } else if (userScrollIntent || transcriptAutoScrollSuspendedByUser) {
      suspendTranscriptAutoScrollForUser(!userScrollIntent && Boolean(transcriptPinnedTurnId));
    } else if (initialScrollPending || forceTranscriptScroll || pendingTranscriptBottomScroll) {
      stickTranscriptToBottom = true;
    } else if (stickTranscriptToBottom) {
      scheduleTranscriptScrollToBottom();
      return;
    } else if (getTranscriptNow() > transcriptProgrammaticScrollUntil) {
      suspendTranscriptAutoScrollForUser();
    }

    if (
      !forceTranscriptScroll &&
      !pendingTranscriptBottomScroll &&
      !initialScrollPending &&
      !loadingDetail &&
      transcriptElement.scrollTop <= transcriptTopLoadThreshold &&
      sessionHydrationRemainingTurns > 0 &&
      olderTurnsAutoLoadEnabled &&
      !loadingOlderTurns
    ) {
      void maybeAutoLoadOlderTurns();
    }
  }

  function getItemKey(turnId: string, itemId: string) {
    return `${turnId}:${itemId}`;
  }

  function isItemExpanded(turnId: string, itemId: string) {
    return Boolean(expandedItems[getItemKey(turnId, itemId)]);
  }

  function isItemDetailLoading(turnId: string, itemId: string) {
    return Boolean(loadingItemDetails[getItemKey(turnId, itemId)]);
  }

  function getItemDetailError(turnId: string, itemId: string) {
    return itemDetailErrors[getItemKey(turnId, itemId)] ?? "";
  }

  function getToolItemLabel(item: CodexItem) {
    if (typeof item.title === "string" && item.title.trim()) {
      return item.title;
    }
    if (item.type === "commandExecution") {
      const specialKind = getSpecialShellCommandKind(item);
      if (specialKind === "git") {
        return m.git();
      }
      if (specialKind === "search") {
        return m.search();
      }
      if (specialKind === "read") {
        return m.read();
      }
      return m.run_command();
    }
    if (item.type === "fileChange") {
      const count = getFileChangeSummaryEntries(item).length;
      return count === 1 ? m.one_file_changed() : count > 1 ? m.files_changed({ count: String(count) }) : m.files_changed_fallback();
    }
    if (item.type === "mcpToolCall") {
      return m.mcp_call();
    }
    if (item.type === "dynamicToolCall") {
      return m.tool_call();
    }
    if (item.type === "webSearch") {
      return m.web_search();
    }
    if (item.type === "imageView") {
      return m.image_label();
    }
    if (item.type === "enteredReviewMode") {
      return m.review_started();
    }
    if (item.type === "exitedReviewMode") {
      return m.done();
    }
    if (item.type === "contextCompaction") {
      return ui.contextCompression;
    }
    return item.type;
  }

  function getToolItemIcon(item: CodexItem) {
    if (item.type === "commandExecution") {
      return isReadOnlyShellCommand(item) ? "search" : "terminal";
    }
    if (item.type === "fileChange") {
      return "difference";
    }
    if (item.type === "mcpToolCall") {
      return "hub";
    }
    if (item.type === "dynamicToolCall") {
      return "extension";
    }
    if (item.type === "webSearch") {
      return "travel_explore";
    }
    if (item.type === "imageView" || item.type === "imageGeneration") {
      return "image";
    }
    if (item.type === "enteredReviewMode" || item.type === "exitedReviewMode") {
      return "fact_check";
    }
    if (item.type === "reasoning") {
      return "psychology_alt";
    }
    if (item.type === "plan") {
      return "checklist";
    }
    if (item.type === "collabAgentToolCall") {
      return "smart_toy";
    }
    return "article";
  }

  function getToolItemSummary(item: CodexItem) {
    if (item.type === "commandExecution") {
      return isReadOnlyShellCommand(item) ? summarizeReadOnlyCommandGroup([item]) : summarizeCommand(item);
    }
    if (item.type === "fileChange") {
      return summarizeFileChanges(item);
    }
    if (typeof item.detailPreview === "string" && item.detailPreview.trim()) {
      return item.detailPreview;
    }
    if (item.type === "mcpToolCall" || item.type === "dynamicToolCall") {
      return formatValue(item.tool) || formatValue(item.toolName) || formatValue(item.server) || m.load_details();
    }
    if (item.type === "webSearch") {
      const queries = getWebSearchQueries(item);
      return queries[0] || getWebSearchUrl(item) || m.search_details();
    }
    if (item.type === "imageView") {
      return getImageViewPath(item) || m.load_details();
    }
    if (item.type === "enteredReviewMode" || item.type === "exitedReviewMode") {
      return getReviewText(item);
    }
    if (item.type === "contextCompaction") {
      const liveTurn = getConversationLiveTurn();
      return liveTurn && liveTurn.items.some((candidate) => candidate.id === item.id) && isContextCompactionRunning(liveTurn.id, item)
        ? ui.contextCompressionInProgress
        : ui.contextCompressionCompleted;
    }
    return m.load_details();
  }

  function updateConversationItem(turnId: string, itemId: string, nextItem: CodexItem) {
    if (!conversation) {
      return;
    }

    conversation = {
      ...conversation,
      thread: {
        ...conversation.thread,
        turns: conversation.thread.turns.map((turn) =>
          turn.id !== turnId
            ? turn
            : mergeConversationTurnState(turn, {
                ...turn,
                items: [{ ...nextItem, id: itemId, detailState: "loaded" }]
              })
        )
      }
    };
    markConversationCacheDirty();
  }

  function replaceConversationTurn(turnId: string, nextTurn: CodexTurn) {
    if (!conversation) {
      return;
    }

    conversation = {
      ...conversation,
      thread: {
        ...conversation.thread,
        turns: conversation.thread.turns.map((turn) =>
          turn.id === turnId ? mergeConversationTurnState(turn, nextTurn) : turn
        )
      }
    };
    markConversationCacheDirty();
  }

  function isTurnLoading(turnId: string) {
    return Boolean(loadingTurns[turnId]);
  }

  function getTurnLoadError(turnId: string) {
    return turnLoadErrors[turnId] ?? "";
  }

  async function loadTurnDetails(turnId: string) {
    if (!selectedSessionId || !conversation) {
      return;
    }

    const sessionId = selectedSessionId;
    const sessionProfileId = profileIdForSession(sessionId);
    const selectionVersion = sessionSelectionVersion;
    const turn = conversation.thread.turns.find((candidate) => candidate.id === turnId);
    const hasDeferredItems = Number(turn?.hiddenItemCount ?? 0) > 0;
    if (!turn || (turn.detailState === "full" && !hasDeferredItems) || loadingTurns[turnId]) {
      return;
    }

    loadingTurns = {
      ...loadingTurns,
      [turnId]: true
    };
    turnLoadErrors = {
      ...turnLoadErrors,
      [turnId]: ""
    };

    try {
      const response = await api.getSessionTurn(sessionId, turnId, sessionProfileId);
      if (!matchesSessionSelection(sessionId, sessionProfileId, selectionVersion) || !conversation) {
        return;
      }

      replaceConversationTurn(turnId, response.turn);
    } catch (error) {
      if (!matchesSessionSelection(sessionId, sessionProfileId, selectionVersion) || !conversation) {
        return;
      }
      turnLoadErrors = {
        ...turnLoadErrors,
        [turnId]: describeError(error)
      };
    } finally {
      if (!matchesSessionSelection(sessionId, sessionProfileId, selectionVersion) || !conversation) {
        return;
      }
      loadingTurns = {
        ...loadingTurns,
        [turnId]: false
      };
    }
  }

  async function applyOlderTurnPage(
    response: SessionTurnsPagePayload,
    previousHeight: number,
    previousTop: number,
    scrollAnchor: TranscriptScrollAnchor | null = null
  ) {
    if (!conversation) {
      return false;
    }

    const mergedTurns = [...response.turns, ...conversation.thread.turns].filter(
      (turn, index, collection) => collection.findIndex((candidate) => candidate.id === turn.id) === index
    );

    conversation = {
      ...conversation,
      thread: {
        ...conversation.thread,
        turns: mergedTurns
      },
      hydration: {
        ...conversation.hydration,
        loadedTurns: mergedTurns.length,
        totalTurns: response.totalTurns,
        remainingTurns: response.remainingTurns
      }
    };
    markConversationCacheDirty();

    await tick();
    if (await restoreTranscriptScrollAnchor(scrollAnchor)) {
      return true;
    }
    refreshTranscriptTurnWindow();
    await tick();
    if (transcriptElement) {
      const previousBehavior = transcriptElement.style.scrollBehavior;
      transcriptElement.style.scrollBehavior = "auto";
      transcriptElement.scrollTo({
        top: transcriptElement.scrollHeight - previousHeight + previousTop,
        behavior: "auto"
      });
      if (previousBehavior) {
        transcriptElement.style.scrollBehavior = previousBehavior;
      } else {
        transcriptElement.style.removeProperty("scroll-behavior");
      }
    }

    return true;
  }

  async function loadOlderTurns(mode: "auto" | "manual" = "manual") {
    if (!selectedSessionId || !conversation || loadingOlderTurns || conversation.hydration.remainingTurns <= 0) {
      return;
    }

    const sessionId = selectedSessionId;
    const sessionProfileId = profileIdForSession(sessionId);
    const selectionVersion = sessionSelectionVersion;
    const beforeTurnId = conversation.thread.turns[0]?.id;
    if (!beforeTurnId) {
      return;
    }

    const previousHeight = transcriptElement?.scrollHeight ?? 0;
    const previousTop = transcriptElement?.scrollTop ?? 0;
    const scrollAnchor = captureTranscriptScrollAnchor();
    loadingOlderTurns = true;
    if (mode === "manual") {
      olderTurnsAutoLoadPaused = false;
      olderTurnsAutoLoadEnabled = true;
      olderTurnsAutoTriggerTimestamps = [];
    }

    try {
      const response = await api.getSessionOlderTurns(sessionId, beforeTurnId, olderTurnPageSize, sessionProfileId);
      if (
        !matchesSessionSelection(sessionId, sessionProfileId, selectionVersion) ||
        !conversation ||
        conversation.thread.id !== sessionId
      ) {
        return;
      }

      await applyOlderTurnPage(response, previousHeight, previousTop, scrollAnchor);
    } catch (error) {
      if (matchesSessionSelection(sessionId, sessionProfileId, selectionVersion)) {
        errorText = describeError(error);
        olderTurnsAutoLoadPaused = true;
        olderTurnsAutoLoadEnabled = false;
      }
    } finally {
      if (matchesSessionSelection(sessionId, sessionProfileId, selectionVersion)) {
        loadingOlderTurns = false;
      }
    }
  }

  async function maybeAutoLoadOlderTurns() {
    const now = Date.now();
    const nextTimestamps = [...olderTurnsAutoTriggerTimestamps.filter((value) => now - value < olderTurnAutoLoadWindowMs), now];
    olderTurnsAutoTriggerTimestamps = nextTimestamps;

    if (nextTimestamps.length >= olderTurnAutoLoadBurstLimit) {
      olderTurnsAutoLoadEnabled = false;
      olderTurnsAutoLoadPaused = true;
      return;
    }

    await loadOlderTurns("auto");
  }

  function enableOlderTurnsAutoLoad() {
    olderTurnsAutoLoadEnabled = true;
    olderTurnsAutoLoadPaused = false;
    olderTurnsAutoTriggerTimestamps = [];
    if (transcriptElement && transcriptElement.scrollTop <= transcriptTopLoadThreshold) {
      void maybeAutoLoadOlderTurns();
    }
  }

  async function jumpToSessionSearchResult(match: SessionTurnSearchMatch) {
    if (!selectedSessionId || !conversation || sessionTurnSearchJumpingTurnId || loadingOlderTurns) {
      return;
    }

    const sessionId = selectedSessionId;
    const sessionProfileId = profileIdForSession(sessionId);
    const selectionVersion = sessionSelectionVersion;
    sessionTurnSearchJumpingTurnId = match.turnId;
    sessionTurnSearchError = "";

    try {
      while (conversation && conversation.thread.id === sessionId && !conversation.thread.turns.some((turn) => turn.id === match.turnId)) {
        if (conversation.hydration.remainingTurns <= 0) {
          break;
        }

        const totalTurns = conversation.hydration.totalTurns ?? conversation.thread.turns.length;
        const loadedStartIndex = Math.max(0, totalTurns - conversation.thread.turns.length);
        if (match.turnIndex >= loadedStartIndex) {
          break;
        }

        const beforeTurnId = conversation.thread.turns[0]?.id;
        if (!beforeTurnId) {
          break;
        }

        const previousHeight = transcriptElement?.scrollHeight ?? 0;
        const previousTop = transcriptElement?.scrollTop ?? 0;
        const scrollAnchor = captureTranscriptScrollAnchor();
        loadingOlderTurns = true;
        const response = await api.getSessionOlderTurns(
          sessionId,
          beforeTurnId,
          Math.min(100, Math.max(olderTurnPageSize, loadedStartIndex - match.turnIndex)),
          sessionProfileId
        );
        if (
          !matchesSessionSelection(sessionId, sessionProfileId, selectionVersion) ||
          !conversation ||
          conversation.thread.id !== sessionId
        ) {
          return;
        }
        await applyOlderTurnPage(response, previousHeight, previousTop, scrollAnchor);
        if (!matchesSessionSelection(sessionId, sessionProfileId, selectionVersion)) {
          return;
        }
        loadingOlderTurns = false;
      }

      if (!matchesSessionSelection(sessionId, sessionProfileId, selectionVersion)) {
        return;
      }

      if (
        !conversation ||
        conversation.thread.id !== sessionId ||
        !conversation.thread.turns.some((turn) => turn.id === match.turnId)
      ) {
        sessionTurnSearchError = describeUiError({
          code: "SESSION_SEARCH_RESULT_UNAVAILABLE",
          message: "Search result could not be located."
        });
        return;
      }

      if (match.requiresFullTurn) {
        await loadTurnDetails(match.turnId);
        if (!matchesSessionSelection(sessionId, sessionProfileId, selectionVersion)) {
          return;
        }
        expandedTurnLogs = {
          ...expandedTurnLogs,
          [match.turnId]: true
        };
      }

      if (match.itemId && match.requiresItemDetail) {
        expandedItems = {
          ...expandedItems,
          [getItemKey(match.turnId, match.itemId)]: true
        };
        await loadItemDetail(match.turnId, match.itemId);
        if (!matchesSessionSelection(sessionId, sessionProfileId, selectionVersion)) {
          return;
        }
      }

      suspendTranscriptAutoScrollForUser();
      pinTranscriptTurn(match.turnId, "center");
      refreshTranscriptTurnWindow(match.turnId, "center");
      await tick();
      if (!matchesSessionSelection(sessionId, sessionProfileId, selectionVersion)) {
        return;
      }
      const escapedTurnId =
        typeof CSS !== "undefined" && typeof CSS.escape === "function"
          ? CSS.escape(match.turnId)
          : match.turnId.replace(/"/g, '\\"');
      const target = transcriptContentElement?.querySelector<HTMLElement>(`[data-turn-id="${escapedTurnId}"]`);
      if (!target) {
        releaseTranscriptTurnPin(match.turnId);
        sessionTurnSearchError = describeUiError({
          code: "SESSION_SEARCH_RESULT_UNAVAILABLE",
          message: "Search result could not be located."
        });
        return;
      }

      target.scrollIntoView({
        block: "start",
        behavior: "smooth"
      });
      releaseTranscriptTurnPinWhenScrollEnds(match.turnId);
      sessionTurnSearchFocusedTurnId = match.turnId;
      if (sessionTurnSearchHighlightTimer) {
        clearTimeout(sessionTurnSearchHighlightTimer);
      }
      sessionTurnSearchHighlightTimer = setTimeout(() => {
        if (sessionTurnSearchFocusedTurnId === match.turnId) {
          sessionTurnSearchFocusedTurnId = null;
        }
      }, 2200);
    } catch (error) {
      releaseTranscriptTurnPin(match.turnId);
      if (matchesSessionSelection(sessionId, sessionProfileId, selectionVersion)) {
        sessionTurnSearchError = describeError(error);
      }
    } finally {
      if (matchesSessionSelection(sessionId, sessionProfileId, selectionVersion)) {
        loadingOlderTurns = false;
        if (sessionTurnSearchJumpingTurnId === match.turnId) {
          sessionTurnSearchJumpingTurnId = null;
        }
      }
    }
  }

  async function loadItemDetail(turnId: string, itemId: string, force = false) {
    if (!selectedSessionId || !conversation) {
      return;
    }

    const sessionId = selectedSessionId;
    const sessionProfileId = profileIdForSession(sessionId);
    const selectionVersion = sessionSelectionVersion;
    const itemKey = getItemKey(turnId, itemId);
    const turn = conversation.thread.turns.find((candidate) => candidate.id === turnId);
    const item = turn?.items.find((candidate) => candidate.id === itemId);
    if (!item) {
      return;
    }

    if (!force && item.detailState === "loaded") {
      return;
    }

    loadingItemDetails = {
      ...loadingItemDetails,
      [itemKey]: true
    };
    itemDetailErrors = {
      ...itemDetailErrors,
      [itemKey]: ""
    };

    try {
      const response = await api.getSessionItemDetail(sessionId, turnId, itemId, sessionProfileId);
      if (!matchesSessionSelection(sessionId, sessionProfileId, selectionVersion) || !conversation) {
        return;
      }
      updateConversationItem(turnId, itemId, response.item);
    } catch (error) {
      const message =
        error instanceof Error ? error.message : typeof error === "string" ? error : "";
      if (
        message.includes("Transcript item detail not found.") &&
        conversation &&
        matchesSessionSelection(sessionId, sessionProfileId, selectionVersion)
      ) {
        try {
          const response = await api.getSessionTurn(sessionId, turnId, sessionProfileId);
          if (!matchesSessionSelection(sessionId, sessionProfileId, selectionVersion) || !conversation) {
            return;
          }
          replaceConversationTurn(turnId, response.turn);
          expandedItems = {
            ...expandedItems,
            [itemKey]: false
          };
          itemDetailErrors = {
            ...itemDetailErrors,
            [itemKey]: ""
          };
          return;
        } catch {
          // Fall through to surface the original detail error.
        }
      }

      if (!matchesSessionSelection(sessionId, sessionProfileId, selectionVersion) || !conversation) {
        return;
      }
      itemDetailErrors = {
        ...itemDetailErrors,
        [itemKey]: describeError(error)
      };
    } finally {
      if (!matchesSessionSelection(sessionId, sessionProfileId, selectionVersion) || !conversation) {
        return;
      }
      loadingItemDetails = {
        ...loadingItemDetails,
        [itemKey]: false
      };
    }
  }

  async function toggleToolItem(turnId: string, itemId: string) {
    const itemKey = getItemKey(turnId, itemId);
    const nextExpanded = !expandedItems[itemKey];
    expandedItems = {
      ...expandedItems,
      [itemKey]: nextExpanded
    };

    if (nextExpanded) {
      await loadItemDetail(turnId, itemId);
    }
  }

  function scheduleExpandedItemRefresh(turnId: string, itemId: string) {
    const sessionId = selectedSessionId;
    const itemKey = getItemKey(turnId, itemId);
    if (!sessionId || !expandedItems[itemKey] || itemDetailRefreshTimers.has(itemKey)) {
      return;
    }

    itemDetailRefreshTimers.set(
      itemKey,
      setTimeout(() => {
        itemDetailRefreshTimers.delete(itemKey);
        if (selectedSessionId !== sessionId) {
          return;
        }
        void loadItemDetail(turnId, itemId, true);
      }, 240)
    );
  }

  function getDeferredToolBody(item: CodexItem) {
    if (item.type === "commandExecution") {
      return String(item.aggregatedOutput ?? "");
    }
    if (item.type === "fileChange") {
      return getFileChangeViews(item)
        .map((change) => {
          const moveLabel = change.movePath ? ` -> ${change.movePath}` : "";
          return `${change.kind}${moveLabel} · ${change.path}`;
        })
        .join("\n");
    }
    if (item.type === "dynamicToolCall") {
      const textItems = getDynamicToolContentItems(item)
        .map((entry) => {
          if (entry.type === "inputText" && typeof entry.text === "string") {
            return entry.text;
          }
          if (entry.type === "inputImage" && typeof entry.imageUrl === "string") {
            return entry.imageUrl;
          }
          return "";
        })
        .filter((value) => value.trim().length > 0);
      if (textItems.length > 0) {
        return textItems.join("\n\n");
      }
    }
    return JSON.stringify(item, null, 2);
  }

  function getDynamicToolContentItems(item: CodexItem): Array<Record<string, unknown>> {
    const contentItems = Array.isArray(item.contentItems)
      ? item.contentItems
      : Array.isArray(item.content_items)
        ? item.content_items
        : [];
    return contentItems.filter((entry): entry is Record<string, unknown> => Boolean(entry) && typeof entry === "object");
  }

  function getDynamicToolTextItems(item: CodexItem) {
    return getDynamicToolContentItems(item)
      .filter((entry) => entry.type === "inputText" && typeof entry.text === "string")
      .map((entry) => String(entry.text));
  }

  function getDynamicToolImageUrls(item: CodexItem) {
    return getDynamicToolContentItems(item)
      .filter((entry) => entry.type === "inputImage" && typeof entry.imageUrl === "string")
      .map((entry) => String(entry.imageUrl));
  }

  function getOutputPreviewKey(turnId: string, outputId: string) {
    return `${turnId}:${outputId}:output`;
  }

  function getHiddenOutputCharCount(output: string, outputKey: string, maxChars = toolOutputInitialChars) {
    if (expandedLargeOutputs[outputKey] || output.length <= maxChars) {
      return 0;
    }
    return output.length - maxChars;
  }

  function getCappedOutputText(output: string, outputKey: string, maxChars = toolOutputInitialChars) {
    const hiddenCount = getHiddenOutputCharCount(output, outputKey, maxChars);
    if (hiddenCount <= 0) {
      return output;
    }
    return `${ui.outputTruncatedPrefix(hiddenCount)}\n${output.slice(-maxChars)}`;
  }

  function expandLargeOutput(outputKey: string) {
    expandedLargeOutputs = {
      ...expandedLargeOutputs,
      [outputKey]: true
    };
  }

  function isTurnRunning(turnId: string) {
    return getConversationLiveTurn()?.id === turnId;
  }

  function isContextCompactionRunning(turnId: string, item: CodexItem) {
    if (item.type !== "contextCompaction") {
      return false;
    }
    if (String(item.lifecycleStatus ?? "") === "completed") {
      return false;
    }
    if (!isTurnRunning(turnId)) {
      return false;
    }

    const turn = conversation?.thread.turns.find((candidate) => candidate.id === turnId) ?? null;
    if (!turn) {
      return false;
    }

    const lastItem = turn.items.at(-1);
    if (!lastItem || lastItem.id !== item.id) {
      return false;
    }

    return true;
  }

  function getFinalAgentItem(turn: ConversationState["thread"]["turns"][number]) {
    return getTurnRenderModel(turn).finalAgentItem;
  }

  function getCollapsedTurnItems(turn: ConversationState["thread"]["turns"][number]) {
    return getTurnRenderModel(turn).collapsedItems;
  }

  function isInternalTranscriptItem(item: CodexItem) {
    return [
      "task_complete",
      "turn_aborted",
      "turn_started",
      "turn_completed",
      "agent_reasoning_section_break"
    ].includes(item.type);
  }

  function getVisibleSummaryEntries(turn: ConversationState["thread"]["turns"][number]) {
    if (!shouldCollapseTurnLogs(turn)) {
      return [] as RenderableTurnEntry[];
    }

    return getTurnRenderModel(turn).visibleSummaryEntries;
  }

  function getFileChangeEntryKey(turnId: string, itemId: string, change: FileChangeView) {
    return `${turnId}:${itemId}:${change.path}:${change.movePath ?? ""}`;
  }

  function isFileChangeEntryExpanded(turnId: string, itemId: string, change: FileChangeView) {
    return Boolean(expandedFileChangeEntries[getFileChangeEntryKey(turnId, itemId, change)]);
  }

  function toggleFileChangeEntry(turnId: string, itemId: string, change: FileChangeView) {
    const nextKey = getFileChangeEntryKey(turnId, itemId, change);
    expandedFileChangeEntries = {
      ...expandedFileChangeEntries,
      [nextKey]: !expandedFileChangeEntries[nextKey]
    };
  }

  function getCollapsedTurnEntries(turn: ConversationState["thread"]["turns"][number]) {
    return getTurnRenderModel(turn).collapsedEntries;
  }

  function getTurnEntries(turn: ConversationState["thread"]["turns"][number]) {
    return getTurnRenderModel(turn).fullEntries;
  }

  function getCollapsedTurnProgressCount(turn: ConversationState["thread"]["turns"][number]) {
    if (turn.detailState === "summary" && typeof turn.hiddenItemCount === "number") {
      return turn.hiddenItemCount + (conversation?.livePlans[turn.id] ? 1 : 0) + (conversation?.liveDiffs[turn.id] ? 1 : 0);
    }
    return (
      getTurnRenderModel(turn).collapsedEntries.length +
      (conversation?.livePlans[turn.id] ? 1 : 0) +
      (conversation?.liveDiffs[turn.id] ? 1 : 0)
    );
  }

  function shouldCollapseTurnLogs(turn: ConversationState["thread"]["turns"][number]) {
    const model = getTurnRenderModel(turn);
    return !isTurnRunning(turn.id) && Boolean(model.finalAgentItem) && getCollapsedTurnProgressCount(turn) > 0;
  }

  function isTurnLogExpanded(turnId: string) {
    return Boolean(expandedTurnLogs[turnId]);
  }

  function isLiveDiffExpanded(turnId: string) {
    return isItemExpanded(turnId, "live-diff-panel");
  }

  function toggleLiveDiff(turnId: string) {
    const itemKey = getItemKey(turnId, "live-diff-panel");
    expandedItems = {
      ...expandedItems,
      [itemKey]: !expandedItems[itemKey]
    };
  }

  async function toggleTurnLogs(turnId: string) {
    expandedTurnLogs = {
      ...expandedTurnLogs,
      [turnId]: !expandedTurnLogs[turnId]
    };

    if (!expandedTurnLogs[turnId]) {
      return;
    }

    await loadTurnDetails(turnId);
  }

  async function toggleReadOnlyCommandGroup(turnId: string, groupId: string, items: CodexItem[]) {
    const itemKey = getItemKey(turnId, groupId);
    const nextExpanded = !expandedItems[itemKey];
    expandedItems = {
      ...expandedItems,
      [itemKey]: nextExpanded
    };

    if (!nextExpanded) {
      return;
    }

    loadingItemDetails = {
      ...loadingItemDetails,
      [itemKey]: true
    };
    itemDetailErrors = {
      ...itemDetailErrors,
      [itemKey]: ""
    };

    try {
      await Promise.all(items.map((item) => loadItemDetail(turnId, item.id)));
      const firstError = items.map((item) => getItemDetailError(turnId, item.id)).find((value) => value.length > 0) ?? "";
      if (firstError) {
        itemDetailErrors = {
          ...itemDetailErrors,
          [itemKey]: firstError
        };
      }
    } finally {
      loadingItemDetails = {
        ...loadingItemDetails,
        [itemKey]: false
      };
    }
  }

  async function toggleFileChangeGroup(turnId: string, groupId: string, items: CodexItem[]) {
    const itemKey = getItemKey(turnId, groupId);
    const nextExpanded = !expandedItems[itemKey];
    expandedItems = {
      ...expandedItems,
      [itemKey]: nextExpanded
    };

    if (!nextExpanded) {
      return;
    }

    loadingItemDetails = {
      ...loadingItemDetails,
      [itemKey]: true
    };
    itemDetailErrors = {
      ...itemDetailErrors,
      [itemKey]: ""
    };

    try {
      await Promise.all(items.map((item) => loadItemDetail(turnId, item.id)));
      const firstError = items.map((item) => getItemDetailError(turnId, item.id)).find((value) => value.length > 0) ?? "";
      if (firstError) {
        itemDetailErrors = {
          ...itemDetailErrors,
          [itemKey]: firstError
        };
      }
    } finally {
      loadingItemDetails = {
        ...loadingItemDetails,
        [itemKey]: false
      };
    }
  }

  function getCodeDiffTabHeight(change: FileChangeView) {
    const lineCount = Math.max(change.original.split("\n").length, change.modified.split("\n").length, change.diff.split("\n").length);
    return Math.min(840, Math.max(440, lineCount * 18 + 140));
  }

  function openFileChangeInTab(change: FileChangeView, tabKey: string, label = baseName(change.movePath ?? change.path)) {
    openCodeDiffTab(tabKey, label, label, [change]);
  }

  function openFileChangeGroupInTab(changes: FileChangeView[], tabKey: string, title: string | null = null) {
    const label =
      title?.trim() || (changes.length === 1 ? baseName(changes[0]?.movePath ?? changes[0]?.path ?? "diff") : `${changes.length} ${m.diff()}`);
    openCodeDiffTab(tabKey, label, label, changes);
  }

  async function updateComposerSettingsPopoverPosition() {
    const triggerElement = composerSettingsAnchor === "security" ? composerSecurityTriggerElement : composerSettingsTriggerElement;

    if (!composerSettingsOpen || !triggerElement || !composerSettingsPopoverElement || typeof window === "undefined") {
      return;
    }

    await tick();
    composerSettingsPopoverStyle = anchoredPopoverStyle(triggerElement, composerSettingsPopoverElement, {
      align: "start",
      maxWidth: 360,
      minHeight: 96,
      minWidth: 272,
      placement: "above",
      preferredWidth: 304
    });
  }

  async function updateSessionTurnSearchPopoverPosition() {
    if (!sessionTurnSearchOpen || !sessionTurnSearchTriggerElement || !sessionTurnSearchPopoverElement || typeof window === "undefined") {
      return;
    }

    await tick();
    sessionTurnSearchPopoverStyle = anchoredPopoverStyle(sessionTurnSearchTriggerElement, sessionTurnSearchPopoverElement, {
      align: "end",
      maxWidth: 440,
      minHeight: 96,
      minWidth: 280,
      placement: "below",
      preferredWidth: 420
    });
  }

  function getComposerSettingsSummary() {
    if (!conversation) {
      return {
        model: ui.settings,
        speed: "auto" as SessionPreferences["speed"],
        indicators: [] as Array<{ key: string; icon: string; label: string; text: string | null }>
      };
    }

    const indicators: Array<{ key: string; icon: string; label: string; text: string | null }> = [
      {
        key: "reasoning",
        icon: "psychology_alt",
        label: `${m.reasoning()} ${conversation.preferences.effort ?? "auto"}`,
        text: conversation.preferences.effort ?? "auto"
      }
    ];

    if ((conversation.preferences.personality ?? "pragmatic") !== "pragmatic") {
      indicators.push({
        key: "personality",
        icon: conversation.preferences.personality === "friendly" ? "mood" : "horizontal_rule",
        label: getPersonalityOptionLabel(conversation.preferences.personality),
        text: null
      });
    }

    if (composerSelectedSkills.length > 0) {
      const [firstSkill] = composerSelectedSkills;
      indicators.push({
        key: "skills",
        icon: "auto_awesome",
        label:
          composerSelectedSkills.length > 1
            ? `${firstSkill.name} +${composerSelectedSkills.length - 1}`
            : firstSkill.name,
        text: null
      });
    }

    if (isPlanModeEnabled(conversation.preferences)) {
      indicators.push({
        key: "plan",
        icon: "checklist",
        label: m.plan_mode_enabled(),
        text: null
      });
    }

    if ((conversation.preferences.speed ?? "auto") !== "auto") {
      indicators.push({
        key: "speed",
        icon: conversation.preferences.speed === "fast" ? "bolt" : "swap_driving_apps",
        label: `Speed ${conversation.preferences.speed}`,
        text: null
      });
    }

    return {
      model: selectedModel?.displayName ?? m.auto_model(),
      speed: conversation.preferences.speed ?? "auto",
      indicators
    };
  }

  const composerSettingsSummary = $derived.by(() => {
    const _locale = $localeSignal;
    return getComposerSettingsSummary();
  });

  $effect(() => {
    if (composerSettingsOpen && composerSettingsTab === "skills") {
      void ensureCatalogLoaded();
    }
  });

  function getSpeedOptionLabel(option: SessionPreferences["speed"]) {
    if (option === "fast") {
      return ui.speedFast;
    }
    if (option === "flex") {
      return ui.speedFlex;
    }
    return ui.speedAuto;
  }

  function getPersonalityOptionLabel(option: SessionPreferences["personality"] | null | undefined) {
    if (option === "friendly") {
      return ui.personalityFriendly;
    }
    if (option === "none") {
      return ui.personalityNone;
    }
    return ui.personalityPragmatic;
  }

  function isFastSpeedMode(speed: SessionPreferences["speed"] | null | undefined) {
    return (speed ?? "auto") === "fast";
  }

  function getSecuritySettingsSummary() {
    if (!conversation) {
      return [] as Array<{ key: string; icon: string; label: string }>;
    }

    const indicators: Array<{ key: string; icon: string; label: string }> = [];
    if ((conversation.preferences.autoApproveMode ?? "manual") !== "manual") {
      indicators.push({
        key: "approve",
        icon: "verified_user",
        label:
          conversation.preferences.autoApproveMode === "session"
            ? m.auto_approve_session()
            : m.auto_approve_turn()
      });
    }
    if (conversation.preferences.networkAccess) {
      indicators.push({
        key: "network",
        icon: "wifi",
        label: m.network_enabled()
      });
    }
    return indicators;
  }

  function changeThemeMode(nextMode: ThemeMode) {
    const detail = applyThemeMode(nextMode);
    themeMode = detail.mode;
    resolvedTheme = detail.resolved;
  }
</script>

<svelte:head>
  <title>{ui.appTitle}</title>
</svelte:head>

{#if authenticated !== true}
  <div class="relative flex h-[100dvh] min-h-[100dvh] w-full overflow-hidden bg-[radial-gradient(circle_at_top,_rgba(251,191,36,0.18),_transparent_28%),linear-gradient(180deg,_#fffaf1_0%,_#ffffff_58%,_#f8fafc_100%)] text-gray-900">
    <div class={`flex h-full w-full transition-all duration-300 ${authenticated === false ? "scale-[0.995] blur-md saturate-[0.9]" : ""}`}>
      <aside class="hidden h-full w-[18.5rem] shrink-0 border-r border-white/70 bg-white/70 px-4 py-5 lg:flex lg:flex-col">
        <div class="mb-5 flex items-center justify-between gap-3">
          <div class="space-y-2">
            <div class="h-2.5 w-20 rounded-full bg-amber-200/80"></div>
            <div class="h-6 w-32 rounded-full bg-gray-200/80"></div>
          </div>
          <div class="h-10 w-10 rounded-2xl bg-amber-100/80"></div>
        </div>
        <div class="mb-4 h-10 rounded-2xl bg-white/90 shadow-sm ring-1 ring-gray-100"></div>
        <div class="mb-4 flex gap-2 rounded-2xl bg-white/70 p-1.5 shadow-sm ring-1 ring-gray-100">
          <div class="h-9 flex-1 rounded-xl bg-white"></div>
          <div class="h-9 flex-1 rounded-xl bg-gray-100/80"></div>
        </div>
        <div class="flex-1 space-y-2 overflow-hidden">
          {#each Array(8) as _, index (`auth-skeleton-sidebar-${index}`)}
            <div class="rounded-2xl bg-white/70 px-4 py-3 shadow-sm ring-1 ring-gray-100/80">
              <div class="h-3 w-[72%] rounded-full bg-gray-200"></div>
              <div class="mt-2 h-2.5 w-[46%] rounded-full bg-gray-100"></div>
            </div>
          {/each}
        </div>
      </aside>

      <div class="flex min-w-0 flex-1 flex-col">
        <div class="flex h-16 items-center justify-between border-b border-white/70 bg-white/65 px-4 shadow-sm shadow-amber-50/60 sm:px-6">
          <div class="flex items-center gap-3">
            <div class="h-9 w-9 rounded-2xl bg-amber-100/90"></div>
            <div class="space-y-2">
              <div class="h-2.5 w-20 rounded-full bg-gray-200"></div>
              <div class="h-3 w-36 rounded-full bg-gray-100"></div>
            </div>
          </div>
          <div class="hidden gap-2 sm:flex">
            <div class="h-9 w-28 rounded-2xl bg-white/90 shadow-sm ring-1 ring-gray-100"></div>
            <div class="h-9 w-10 rounded-2xl bg-white/90 shadow-sm ring-1 ring-gray-100"></div>
          </div>
        </div>

        <div class="flex min-h-0 flex-1 flex-col gap-5 px-4 py-5 sm:px-6 sm:py-6 lg:px-8">
          <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-5">
            <div class="space-y-4 pt-3">
              {#each Array(4) as _, index (`auth-skeleton-turn-${index}`)}
                <div class={`flex ${index % 2 === 0 ? "justify-start" : "justify-end"}`}>
                  <div class={`rounded-[1.75rem] border border-white/80 bg-white/82 p-5 shadow-sm ${index % 2 === 0 ? "w-full max-w-2xl" : "w-full max-w-xl"}`}>
                    <div class="space-y-3">
                      <div class="h-3 rounded-full bg-gray-200" style={`width:${index % 2 === 0 ? 78 : 68}%`}></div>
                      <div class="h-3 rounded-full bg-gray-100" style={`width:${index % 2 === 0 ? 54 : 48}%`}></div>
                      <div class="h-3 rounded-full bg-gray-100" style={`width:${index % 2 === 0 ? 60 : 38}%`}></div>
                    </div>
                  </div>
                </div>
              {/each}
            </div>

            <div class="mt-auto rounded-[1.75rem] border border-white/80 bg-white/85 p-4 shadow-[0_24px_60px_rgba(15,23,42,0.08)]">
              <div class="h-12 rounded-2xl bg-gray-100"></div>
              <div class="mt-3 flex items-center justify-between gap-3">
                <div class="flex items-center gap-2">
                  <div class="h-9 w-9 rounded-2xl bg-gray-100"></div>
                  <div class="h-9 w-28 rounded-2xl bg-gray-100"></div>
                </div>
                <div class="h-9 w-24 rounded-2xl bg-amber-100/90"></div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>

    {#if authenticated === false}
      <AuthLoginOverlay
        activeLocale={$activeLocale}
        bind:loginHcaptchaContainer={loginHcaptchaContainer}
        bind:loginPassword={loginPassword}
        {localeOptions}
        {loginBusy}
        {loginHcaptcha}
        {loginHcaptchaToken}
        {loginMessage}
        {ui}
        onLocaleChange={(locale) => updateLocale(locale as (typeof localeOptions)[number]["value"])}
        onSubmit={() => void handleLogin()}
      />
    {:else if errorText}
      <div class="pointer-events-none absolute inset-x-0 bottom-0 z-10 flex justify-center px-4 pb-5 sm:px-6">
        <p class="auth-dialog-message pointer-events-auto max-w-xl rounded-2xl border border-red-100 bg-red-50/95 px-4 py-3 text-sm text-red-700 shadow-lg shadow-red-100/70">
          {errorText}
        </p>
      </div>
    {/if}
  </div>
{:else}
<div class="flex h-[100dvh] min-h-[100dvh] w-full bg-white overflow-hidden font-sans text-gray-900" data-testid="workspace-shell">
  {#if showConnectionSnackbar || feedbackSnackbar}
    <div class="workspace-snackbar-stack pointer-events-none fixed inset-x-0 z-[110] flex justify-center px-3 sm:px-6">
      <div class="flex w-full max-w-xl flex-col gap-2">
        {#if showConnectionSnackbar}
          {#key `${connectionState}:${connectionBannerText}`}
            <div
              transition:fly={{ y: -18, duration: 220 }}
              data-tone={connectionSnackbarTone}
              class="snackbar-card pointer-events-auto flex w-full items-center gap-3 rounded-2xl border px-4 py-3 shadow-xl backdrop-blur-xl transition-all duration-300 {connectionSnackbarTone === 'error'
                ? 'border-red-200 bg-red-50/95 text-red-800 shadow-red-100/80'
                : connectionSnackbarTone === 'warning'
                  ? 'border-amber-200 bg-amber-50/95 text-amber-800 shadow-amber-100/80'
                  : 'border-sky-200 bg-sky-50/95 text-sky-800 shadow-sky-100/80'}"
              role="status"
            >
              {#if connectionState === "reconnecting"}
                <RefreshCw size={16} class="shrink-0 animate-spin" />
              {:else}
                <AlertCircle size={16} class="shrink-0" />
              {/if}
              <span class="min-w-0 text-sm font-medium leading-5">{connectionBannerText}</span>
            </div>
          {/key}
        {/if}

        {#if feedbackSnackbar}
          {#key `${feedbackSnackbar.tone}:${feedbackSnackbar.text}`}
            <div
              transition:fly={{ y: -14, duration: 220 }}
              data-tone={feedbackSnackbar.tone}
              class="snackbar-card pointer-events-auto flex w-full items-start gap-3 rounded-2xl border px-4 py-3 shadow-xl backdrop-blur-xl transition-all duration-300 {feedbackSnackbar.tone === 'error'
                ? 'border-red-200 bg-red-50/95 text-red-800 shadow-red-100/80'
                : feedbackSnackbar.tone === 'warning'
                  ? 'border-amber-200 bg-amber-50/95 text-amber-800 shadow-amber-100/80'
                : 'border-emerald-200 bg-emerald-50/95 text-emerald-800 shadow-emerald-100/80'}"
              role={feedbackSnackbar.tone === "error" ? "alert" : "status"}
            >
              {#if feedbackSnackbar.tone === "error"}
                <AlertCircle size={16} class="mt-0.5 shrink-0" />
              {:else if feedbackSnackbar.tone === "warning"}
                <RefreshCw size={16} class="mt-0.5 shrink-0 animate-spin" />
              {:else}
                <CheckCircle2 size={16} class="mt-0.5 shrink-0" />
              {/if}
              <span class="min-w-0 flex-1 text-sm font-medium leading-5">{feedbackSnackbar.text}</span>
              {#if feedbackSnackbar.dismissible !== false}
                <button
                  aria-label={ui.close}
                  class="ui-animated-button ui-animated-button--icon rounded-lg p-1 text-current/70 transition-colors hover:bg-black/5 hover:text-current"
                  onclick={dismissFeedbackSnackbar}
                  title={ui.close}
                  type="button"
                >
                  <X size={14} />
                </button>
              {/if}
            </div>
          {/key}
        {/if}
      </div>
    </div>
  {/if}

  <!-- Sidebar -->
  <aside
    class:hidden={!mobileSidebarOpen && isMobileLayout}
    class={[
      "h-full border-r border-gray-200 transition-all duration-300",
      isMobileLayout
        ? "fixed inset-y-0 left-0 z-[130] w-[min(22rem,calc(100vw-1.5rem))] max-w-[calc(100vw-1.5rem)] shadow-2xl"
        : "w-[22rem] min-w-[22rem] max-w-[24rem] flex-shrink-0"
    ]}
  >
    <SessionSidebar
      account={config?.account ?? null}
      {accountLoginFlow}
      webRole={webRole}
      readOnly={readOnlyRole}
      {notifications}
      notificationsBusy={notificationsBusy}
      notificationsUnreadCount={config?.notifications.unreadCount ?? 0}
      {quota}
      {quotaBusy}
      {profileAccounts}
      {profileAccountsBusy}
      {resetTickets}
      {resetTicketsBusy}
      {resetTicketUseBusyId}
      {runtime}
      {runtimeBusyAction}
      gatewayRestartAvailable={config?.gateway.restartAvailable ?? false}
      {gatewayRestartBusy}
      showPwaInstall={showPwaInstallAction}
      {pwaInstalled}
      {pwaInstallBusy}
      defaultLanguageBridgeEnabled={config?.defaults.languageBridgeEnabled ?? false}
      {defaultLanguageBridgeBusy}
      profiles={config?.profiles ?? []}
      systemShutdownArmed={config?.systemShutdown.armed ?? false}
      systemShutdownAvailable={config?.systemShutdown.available ?? false}
      systemShutdownDelaySeconds={config?.systemShutdown.delaySeconds ?? 0}
      {themeMode}
      {resolvedTheme}
      {sessions}
      {sessionHighlights}
      {sessionsBusy}
      {sessionsHasMore}
      {sessionsLoadingMore}
      sessionsLoadPercent={sessionLoadPercent}
      searchQuery={sessionSearchQuery}
      searchScope={sessionSearchScope}
	      {sessionFilter}
	      savedSessionFilters={config?.sessionOrganization.savedFilters ?? []}
	      knownSessionTags={config?.sessionOrganization.knownTags ?? []}
	      sessionFolders={config?.sessionOrganization.sessionFolders ?? []}
	      {activeSessionFolder}
	      {activeSavedSessionFilterId}
      showArchived={showArchivedSessions}
      showCloseButton={isMobileLayout}
      onArchivedChange={updateArchivedSessions}
      onCancelAccountLogin={(loginId) => {
        void cancelAccountLogin(loginId);
      }}
      onClose={closeMobileSidebar}
      onCreate={() => {
        void createSession();
      }}
      onLoadMore={() => {
        void loadMoreSessions();
      }}
      onLogoutAccount={() => {
        void logoutAccount();
      }}
      onLogoutWeb={() => {
        void logoutWebUi();
      }}
      onRefreshQuota={() => {
        void refreshQuota(true);
      }}
      onRefreshProfileAccounts={(force) => {
        void refreshProfileAccounts(force);
      }}
      onRefreshResetTickets={() => {
        void refreshResetTickets(true);
      }}
      onUseResetTicket={(ticket) => {
        void useResetTicket(ticket);
      }}
      onRefreshNotifications={() => {
        void refreshNotifications();
      }}
      onMarkNotificationsRead={(ids) => {
        void markNotificationsRead(ids);
      }}
      onClearNotifications={() => {
        void clearNotifications();
      }}
      onRefreshRuntime={() => {
        void refreshRuntimeStatus(true);
      }}
      onInstallApp={() => {
        void installApp();
      }}
      onDefaultLanguageBridgeChange={(enabled) => {
        void saveDefaultLanguageBridgeDefaults(enabled);
      }}
      onSystemShutdownArmedChange={(armed) => {
        void saveSystemShutdownAfterQueueCompletes(armed);
      }}
      onInstallRuntime={() => {
        void installCodex();
      }}
      onUpdateRuntime={() => {
        void updateCodex();
      }}
      onRestartGateway={() => {
        void restartGateway();
      }}
      onThemeModeChange={changeThemeMode}
      onSearchQueryChange={updateSessionSearchQuery}
      onSearchScopeChange={updateSessionSearchScope}
	      onSessionFilterChange={updateSessionFilter}
	      onSelectSessionFolder={openSessionFolder}
	      onSelectUnfiledSessions={openUnfiledSessions}
	      onCreateSessionFolder={() => {
	        void createSessionFolder();
	      }}
	      onToggleSessionFolderPin={(folder) => {
	        void toggleSessionFolderPin(folder);
	      }}
	      onAddSelectedSessionToFolder={(folderName) => {
	        void setSelectedSessionFolderMembership(folderName, true);
	      }}
	      onRemoveSelectedSessionFromFolder={(folderName) => {
	        void setSelectedSessionFolderMembership(folderName, false);
	      }}
	      onApplySavedFilter={applySavedSessionFilter}
      onSaveCurrentFilter={() => {
        void saveCurrentSessionFilter();
      }}
      onDeleteSavedFilter={(filterId) => {
        void deleteSavedSessionFilter(filterId);
      }}
      onSelect={(sessionId, profileId) => {
        void selectSession(sessionId, profileId ?? null);
      }}
      onTogglePin={(session) => {
        void toggleSessionPinned(session);
      }}
      onToggleArchive={(session) => {
        void archiveSessionFromSidebar(session);
      }}
      onRequestMoveSessionProfile={openSessionProfileMoveDialog}
      onStartAccountLogin={(type) => {
        void startAccountLogin(type);
      }}
      onImportAccountCredentials={(path, options) => {
        void importAccountCredentialsFromServer(path, options);
      }}
      onSelectProfile={(profileId) => {
        void selectAccountProfile(profileId);
      }}
      onRenameProfile={(profileId, label) => {
        void renameAccountProfile(profileId, label);
      }}
      onDeleteProfile={(profileId, label) => {
        void deleteAccountProfile(profileId, label);
      }}
      selectedId={selectedSessionId}
    />
  </aside>

  <!-- Main Content -->
  <main class="flex-1 flex flex-col h-full min-w-0 bg-white relative">
    <WorkspaceHeader
      activeWorkspaceTabId={activeWorkspaceTabId}
      bind:searchTriggerElement={sessionTurnSearchTriggerElement}
      bind:titleDraft={titleDraft}
      bind:titleInputElement={titleInputElement}
      bind:workspaceMenuOpen={workspaceMenuOpen}
      contextUsage={getContextUsageIndicator()}
      {isMobileLayout}
      {running}
      {selectedSessionId}
      {selectedSessionSummary}
      profiles={config?.profiles ?? []}
      searchOpenLabel={sessionSearchCopy.openSearch}
      sessionSearchOpen={sessionTurnSearchOpen}
      showArchivedSessions={showArchivedSessions}
      tokenCountLabel={conversation?.tokenUsage ? `${formatTokenCount(conversation.tokenUsage.total.totalTokens)} tok` : null}
      readOnly={readOnlyRole}
      {ui}
      onCreateSession={() => void createSession()}
      onCreateTerminalTab={() => void createTerminalTab()}
      onEditTags={() => void editSelectedSessionTags()}
      onRequestMoveSessionProfile={openSessionProfileMoveDialog}
      onForkHandoff={() => void forkCurrentThread("handoff")}
      onOpenComputerTab={openComputerTab}
      onOpenDiagnosticsTab={openDiagnosticsTab}
      onOpenGitTab={openGitTab}
      onOpenMemoryTab={openMemoryTab}
      onOpenMobileSidebar={openMobileSidebar}
      onOpenSettingsTab={openSettingsTab}
      onOpenTasksTab={openTasksTab}
      onSaveTitle={() => void saveTitle()}
      onToggleArchive={() => {
        if (showArchivedSessions) void unarchiveCurrentSession();
        else void archiveCurrentSession();
      }}
      onTogglePinned={() => {
        if (selectedSessionSummary) {
          void toggleSessionPinned(selectedSessionSummary);
        }
      }}
      onToggleSearch={toggleSessionTurnSearch}
    />

    <!-- Workspace Tabs -->
    {#if workspaceTabs.length > 1}
      <WorkspaceTabStrip
        activeTabId={activeWorkspaceTabId}
        tabs={workspaceTabs}
        onActivate={(tabId) => activateTab(tabId as WorkspaceTabId)}
        onClose={(tabId, kind) => {
          if (kind === "tasks") {
            closeTasksTab();
            return;
          }
          if (kind === "git") {
            closeGitTab();
            return;
          }
          if (kind === "settings") {
            closeSettingsTab();
            return;
          }
          if (kind === "computer") {
            closeComputerTab();
            return;
          }
          if (kind === "diagnostics") {
            closeDiagnosticsTab();
            return;
          }
          if (kind === "memory") {
            closeMemoryTab();
            return;
          }
          if (kind === "git-diff") {
            closeGitDiffTab(tabId as `git-diff:${string}`);
            return;
          }
          if (kind === "code-diff") {
            closeCodeDiffTab(tabId as `code-diff:${string}`);
            return;
          }
          if (kind === "file") {
            closeFileTab(tabId as `file:${string}`);
            return;
          }
          if (kind === "terminal") {
            void closeTerminalTab(tabId.replace(/^terminal:/u, ""));
          }
        }}
      />
    {/if}

    <div class="flex-1 overflow-hidden relative">
      {#if showTopLoadBar}
        <div class="pointer-events-none absolute inset-x-0 top-0 z-[58]">
          <div class="top-load-bar-track">
            <div class="top-load-bar-fill" style={`width:${Math.max(6, Math.min(100, topLoadPercent))}%`}></div>
          </div>
          {#if showTopLoadPill}
            <div class="top-load-pill">
              <RefreshCw size={11} class={topLoadKind === "sessionHydration" ? "" : "animate-spin"} />
              <span>{topLoadLabel}</span>
            </div>
          {/if}
        </div>
      {/if}
      {#if activeWorkspaceTabId === "chat"}
        <div class="h-full flex flex-col relative bg-white">
          <div
            bind:this={transcriptElement}
            class="chat-transcript flex-1 overflow-y-auto pt-8 pb-8"
            onscroll={handleTranscriptScroll}
            style={`padding-bottom: calc(${transcriptDockReservePx}px + env(safe-area-inset-bottom));`}
          >
            <div bind:this={transcriptContentElement} class="max-w-3xl mx-auto px-6 space-y-12">
              {#if loading || (loadingDetail && !conversation)}
                <div class="space-y-6 animate-pulse mt-8">
                  <div class="h-4 bg-gray-100 rounded w-1/3"></div>
                  <div class="space-y-3">
                    <div class="h-4 bg-gray-50 rounded w-full"></div>
                    <div class="h-4 bg-gray-50 rounded w-5/6"></div>
                  </div>
                </div>
              {:else if conversation}
                {#if conversation.goal}
                  <div class="goal-chip sticky top-3 z-[44] mb-6 flex min-w-0 items-center justify-between gap-2 rounded-2xl border border-amber-200/80 bg-white/95 px-3 py-2 shadow-sm backdrop-blur">
                    <div class="min-w-0 flex items-center gap-2">
                      <span class="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-xl border border-amber-100 bg-amber-50 text-amber-700">
                        <ListTodo size={14} />
                      </span>
                      <div class="min-w-0">
                        <p class="truncate text-[11px] font-bold text-gray-800">{conversation.goal.objective}</p>
                        <p class="mt-0.5 text-[10px] text-gray-500">
                          {conversation.goal.status} · {formatTokenCount(conversation.goal.tokensUsed)}
                          {#if conversation.goal.tokenBudget !== null}
                            / {formatTokenCount(conversation.goal.tokenBudget)}
                          {/if}
                        </p>
                      </div>
                    </div>
                    <div class="flex shrink-0 items-center gap-1">
                      {#if goalPrimaryAction(conversation.goal.status)}
                        <button
                          class="ui-animated-button ui-animated-button--soft rounded-lg border border-gray-200 bg-white px-2 py-1 text-[10px] font-bold text-gray-600 hover:bg-gray-50 disabled:opacity-50"
                          disabled={readOnlyRole}
                          onclick={() => void handleGoalPrimaryAction(conversation?.goal?.status)}
                          type="button"
                        >
                          {goalPrimaryActionLabel(conversation.goal.status)}
                        </button>
                      {/if}
                      <button
                        class="ui-animated-button ui-animated-button--soft rounded-lg border border-red-100 bg-red-50 px-2 py-1 text-[10px] font-bold text-red-600 hover:bg-red-100 disabled:opacity-50"
                        disabled={readOnlyRole}
                        onclick={() => void handleGoalSlashCommand("clear")}
                        type="button"
                      >
                        {m.clear_all()}
                      </button>
                    </div>
                  </div>
                {/if}

                {#if manualCompactPrompt && manualCompactPrompt.sessionId === conversation.thread.id}
                  <div
                    class="rounded-2xl border px-3 py-3 shadow-sm"
                    style="border-color: rgba(245, 158, 11, 0.38); background: color-mix(in srgb, var(--panel-strong) 88%, rgba(245, 158, 11, 0.12)); color: var(--ink-strong);"
                  >
                    <div class="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
                      <div class="min-w-0 flex items-start gap-3">
                        <span class="mt-0.5 inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-xl border border-amber-200 bg-amber-50 text-amber-700">
                          <AlertCircle size={15} />
                        </span>
                        <div class="min-w-0">
                          <p class="text-sm font-bold">{ui.manualCompactTitle}</p>
                          <p class="mt-1 line-clamp-2 text-xs leading-5" style="color: var(--muted);">
                            {manualCompactPrompt.message || ui.manualCompactDescription}
                          </p>
                        </div>
                      </div>
                      <div class="flex shrink-0 items-center gap-2">
                        <button
                          class="ui-animated-button ui-animated-button--soft inline-flex h-8 items-center gap-2 rounded-xl border border-amber-200 bg-amber-600 px-3 text-xs font-bold text-white shadow-sm hover:bg-amber-700 disabled:cursor-not-allowed disabled:opacity-55"
                          disabled={manualCompactPrompt.busy || readOnlyRole}
                          onclick={() => void startManualCompactFromPrompt()}
                          type="button"
                        >
                          <RefreshCw size={13} class={manualCompactPrompt.busy ? "animate-spin" : ""} />
                          {manualCompactPrompt.busy ? ui.manualCompactStarting : ui.manualCompactAction}
                        </button>
                        <button
                          aria-label={ui.close}
                          class="ui-animated-button ui-animated-button--icon inline-flex h-8 w-8 items-center justify-center rounded-xl text-gray-400 hover:bg-black/5 hover:text-gray-700"
                          onclick={dismissManualCompactPrompt}
                          type="button"
                        >
                          <X size={14} />
                        </button>
                      </div>
                    </div>
                  </div>
                {/if}

                {#if !readOnlyRole}
                  <div
                    class="rounded-2xl border px-3 py-2 shadow-sm"
                    style="border-color: var(--line); background: var(--panel-strong); color: var(--ink-strong);"
                  >
                    <div class="flex min-w-0 items-center justify-between gap-2">
                      <button
                        class="ui-animated-button ui-animated-button--soft flex min-w-0 flex-1 items-center gap-2 rounded-xl px-2 py-1.5 text-left text-xs font-bold"
                        style="color: var(--ink-strong);"
                        onclick={() => void toggleRollbackTargetsPanel()}
                        type="button"
                      >
                        <RotateCcw size={14} class="shrink-0 text-red-500" />
                        <span class="truncate">{ui.rollbackTargets}</span>
                        {#if rollbackTargetsPayload?.targets}
                          <span class="rounded-full px-1.5 py-0.5 text-[10px]" style="background: var(--panel-soft); color: var(--muted);">
                            {rollbackTargetsPayload.targets.length}
                          </span>
                        {/if}
                      </button>
                      <button
                        class="ui-animated-button ui-animated-button--soft rounded-lg px-2 py-1 text-[10px] font-bold disabled:opacity-50"
                        disabled={rollbackTargetsLoading}
                        onclick={() => void loadRollbackTargets(true)}
                        style="color: var(--muted);"
                        title={ui.refresh}
                        type="button"
                      >
                        <RefreshCw size={12} class={rollbackTargetsLoading ? "animate-spin" : ""} />
                      </button>
                      <ChevronDown size={14} class="shrink-0 text-gray-400 {rollbackTargetsOpen ? 'rotate-180' : ''} transition-transform" />
                    </div>

                    {#if rollbackTargetsOpen}
                      <div class="mt-2 space-y-2 border-t pt-2" style="border-color: var(--line);" transition:slide|local={{ duration: 180 }}>
                        <p class="text-[11px]" style="color: var(--muted);">{ui.rollbackTargetsHint}</p>
                        {#if rollbackTargetsLoading && !rollbackTargetsPayload}
                          <div class="flex items-center gap-2 rounded-xl px-3 py-2 text-xs" style="background: var(--panel-soft); color: var(--muted);">
                            <RefreshCw size={12} class="animate-spin" />
                            {ui.rollbackTargetsLoading}
                          </div>
                        {:else if rollbackTargetsError}
                          <div class="rounded-xl border border-red-200 bg-red-50 px-3 py-2 text-xs text-red-700">{rollbackTargetsError}</div>
                        {:else if !rollbackTargetsPayload || rollbackTargetsPayload.targets.length === 0}
                          <div class="rounded-xl px-3 py-2 text-xs" style="background: var(--panel-soft); color: var(--muted);">{ui.rollbackTargetsEmpty}</div>
                        {:else}
                          <div class="max-h-72 space-y-1.5 overflow-y-auto pr-1">
                            {#each rollbackTargetsPayload.targets as target (target.turnId ?? `${target.turnIndex}:${target.numTurns}`)}
                              <button
                                class="ui-animated-button ui-animated-button--soft flex w-full min-w-0 items-center gap-3 rounded-xl border px-3 py-2 text-left disabled:opacity-50"
                                disabled={rollbackTargetsLoading}
                                onclick={() => void rollbackCurrentThreadToTarget(target)}
                                style="border-color: var(--line); background: var(--panel-soft); color: var(--ink-strong);"
                                type="button"
                              >
                                <span class="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg border text-[10px] font-bold" style="border-color: var(--line); color: var(--muted);">
                                  {target.turnIndex + 1}
                                </span>
                                <span class="min-w-0 flex-1">
                                  <span class="block truncate text-xs font-semibold">{target.preview}</span>
                                  <span class="mt-0.5 block truncate text-[10px]" style="color: var(--muted);">
                                    {ui.rollbackTurnsCount}: {target.numTurns}
                                    {#if target.startedAt}
                                      · {formatTurnTimestamp(target.startedAt)}
                                    {/if}
                                  </span>
                                </span>
                                <RotateCcw size={13} class="shrink-0 text-red-500" />
                              </button>
                            {/each}
                          </div>
                        {/if}
                        {#if rollbackTargetsPayload?.truncatedBefore}
                          <p class="text-[10px]" style="color: var(--muted);">{ui.rollbackPreviewIncomplete}</p>
                        {/if}
                      </div>
                    {/if}
                  </div>
                {/if}

                {#if sessionHydrationRemainingTurns > 0 || loadingOlderTurns || olderTurnsAutoLoadPaused}
                  <div class="py-6 border-b border-gray-100 mb-12 flex flex-col items-center gap-4 text-center">
                    <div class="space-y-1">
                      <p class="text-sm font-bold text-gray-900">{loadingOlderTurns ? ui.loadingHistory : ui.historyAvailable}</p>
                      <p class="text-xs text-gray-500">{olderTurnsAutoLoadPaused ? ui.autoLoadPaused : m.older_turns_remaining({ count: String(sessionHydrationRemainingTurns) })}</p>
                    </div>
                    <div class="flex gap-2">
                      {#if olderTurnsAutoLoadPaused}<button class="px-4 py-2 bg-white border border-gray-200 rounded-xl text-xs font-bold text-gray-700 hover:bg-gray-50 shadow-sm" onclick={enableOlderTurnsAutoLoad}>{ui.resumeAutoLoad}</button>{/if}
                      {#if sessionHydrationRemainingTurns > 0}<button class="px-4 py-2 bg-amber-600 text-white rounded-xl text-xs font-bold hover:bg-amber-700 shadow-sm disabled:opacity-50" disabled={loadingOlderTurns} onclick={() => void loadOlderTurns("manual")}>{loadingOlderTurns ? m.loading() : ui.loadOlderTurns}</button>{/if}
                    </div>
                  </div>
                {/if}

                <div bind:this={transcriptTurnsElement} class="transcript-turn-window">
                  {#if transcriptTurnWindow.topSpacer > 0}
                    <div aria-hidden="true" style={`height:${transcriptTurnWindow.topSpacer}px`}></div>
                  {/if}
                {#each visibleTranscriptTurns as turn, visibleTurnIndex (turn.id)}
                  {@const turnModel = getTurnRenderModel(turn)}
                  {@const collapsedProgressCount = getCollapsedTurnProgressCount(turn)}
                  <div
                    class={`space-y-8 rounded-[1.75rem] px-3 py-3 transition-[background-color,box-shadow,border-color] duration-300 ${
                      sessionTurnSearchFocusedTurnId === turn.id
                        ? "border border-amber-200 bg-amber-50/50 shadow-[0_18px_40px_-32px_rgba(245,158,11,0.8)]"
                        : "border border-transparent"
                    }`}
                    data-turn-id={turn.id}
                    style={`margin-bottom:${transcriptTurnWindow.start + visibleTurnIndex < conversation.thread.turns.length - 1 ? transcriptTurnGap : 0}px`}
                    use:measureTranscriptTurn={turn.id}
                  >
                    {#each turnModel.userItems as item (item.id)}
                      <div class="flex flex-col items-end gap-2 max-w-[85%] ml-auto group/user-message">
                        <div class="flex items-center gap-1 opacity-0 group-hover/user-message:opacity-100 transition-opacity">
                          <button class="p-1.5 rounded-lg text-gray-400 hover:text-gray-700 hover:bg-gray-100 transition-colors" onclick={() => void copyMessageText(getUserText(item))} title={ui.copyMessage} type="button"><Copy size={13} /></button>
                          <button class="p-1.5 rounded-lg text-gray-400 hover:text-gray-700 hover:bg-gray-100 transition-colors" onclick={() => editMessageText(getUserText(item))} title={ui.editInComposer} type="button"><Pencil size={13} /></button>
                          <button class="p-1.5 rounded-lg text-gray-400 hover:text-amber-700 hover:bg-amber-50 transition-colors" onclick={() => void forkCurrentThread("fork", { turnId: turn.id, messageText: getUserText(item) })} title={ui.branchIntoNewThread} type="button"><GitBranch size={13} /></button>
                          <button class="p-1.5 rounded-lg text-gray-400 hover:text-amber-700 hover:bg-amber-50 transition-colors" onclick={() => void forkCurrentThread("handoff", { turnId: turn.id, messageText: getUserText(item) })} title={ui.handoffToNewThread} type="button"><ArrowRightLeft size={13} /></button>
                          <button class="p-1.5 rounded-lg text-gray-400 hover:text-red-700 hover:bg-red-50 transition-colors" onclick={() => void rollbackCurrentThreadToTurn(turn.id)} title={ui.rollbackToThisTurn} type="button"><RotateCcw size={13} /></button>
                        </div>
                        <div class="px-5 py-3 bg-gray-100 rounded-2xl text-gray-800 shadow-sm border border-gray-200/50">
                          <MarkdownMessage compact expandLabel={ui.showFullMessage} maxInitialChars={compactMarkdownInitialChars} on:openLocalPath={(event: CustomEvent<{ href: string }>) => openFileFromMessage(event.detail.href)} text={getUserText(item)} />
                          {#if getUserAttachmentNames(item).length > 0}
                            <div class="mt-3 flex flex-wrap gap-2">
                              {#each getUserAttachmentNames(item) as name}<span class="px-2 py-1 bg-white/80 rounded-lg border border-gray-200 text-[10px] font-bold text-gray-600 flex items-center gap-1.5"><FileText size={10} />{name}</span>{/each}
                            </div>
                          {/if}
                        </div>
                        {@render renderMessageTimestamp(turn.startedAt, ui.userMessageTimestamp, "right")}
                      </div>
                    {/each}

                    {#if optimisticAnchorTurnId === turn.id && visibleOptimisticMessage}
                      <div class="flex flex-col items-end gap-3 max-w-[85%] ml-auto opacity-70">
                        <div class="px-5 py-3 bg-gray-100 rounded-2xl text-gray-800 shadow-sm border border-gray-200/50">
                          <MarkdownMessage compact expandLabel={ui.showFullMessage} maxInitialChars={compactMarkdownInitialChars} text={visibleOptimisticMessage.prompt} />
                        </div>
                        {@render renderMessageTimestamp(visibleOptimisticMessage.createdAt, ui.userMessageTimestamp, "right")}
                      </div>
                    {/if}

                    <div class="flex gap-4">
                      <div class="flex-shrink-0 w-8 h-8 rounded-lg bg-amber-600 text-white flex items-center justify-center shadow-sm mt-1"><Bot size={18} /></div>
                      <div class="flex-1 min-w-0 space-y-6">
                        {#if shouldCollapseTurnLogs(turn)}
                          {#if collapsedProgressCount > 0}
                            <div class="turn-card-shell border border-gray-100 rounded-xl bg-gray-50/50 overflow-hidden">
                              <button
                                class="turn-card-header turn-card-header--neutral w-full flex items-center justify-between px-4 py-3 hover:bg-gray-100/50 transition-colors"
                                data-sticky-level="0"
                                onclick={() => void toggleTurnLogs(turn.id)}
                              >
                                <div class="flex items-center gap-3">
                                  <div class="p-1.5 bg-white border border-gray-200 rounded-lg text-gray-400"><History size={14} /></div>
                                  <span class="text-xs font-bold text-gray-600 tracking-tight uppercase">{m.work_steps_count({ count: String(collapsedProgressCount) })}</span>
                                </div>
                                <ChevronDown size={14} class="text-gray-400 {isTurnLogExpanded(turn.id) ? 'rotate-180' : ''} transition-transform" />
                              </button>
                              {#if isTurnLogExpanded(turn.id)}
                                <div class="turn-card-expand p-4 pt-0 space-y-4 bg-white/50 border-t border-gray-100" transition:slide|local={{ duration: 220 }}>
                                  {#if isTurnLoading(turn.id)}<div class="py-4 flex items-center justify-center gap-2 text-xs text-gray-400"><RefreshCw size={12} class="animate-spin" />{m.loading()}</div>
                                  {:else if getTurnLoadError(turn.id)}<div class="p-3 bg-red-50 text-red-600 rounded-xl text-xs">{getTurnLoadError(turn.id)}</div>
                                  {:else}
                                    {@const collapsedEntries = turnModel.collapsedEntries}
                                    {@const hiddenCollapsedEntryCount = getHiddenTurnEntryCount(turn.id, collapsedEntries)}
                                    {@const visibleCollapsedEntries = getVisibleTurnEntries(turn.id, collapsedEntries)}
                                    {@render renderHiddenTurnEntriesControl(turn.id, hiddenCollapsedEntryCount, collapsedEntries)}
                                    {#each visibleCollapsedEntries as entry (entry.key)}{@render renderTurnEntry(turn.id, entry, 1)}{/each}
                                  {/if}
                                </div>
                              {/if}
                            </div>
                          {/if}
                          {#each turnModel.visibleSummaryEntries as entry (entry.key)}{@render renderTurnEntry(turn.id, entry, 0)}{/each}
                          {#if turnModel.finalAgentItem}
                            {@render renderTurnItem(turn.id, turnModel.finalAgentItem, 0)}
                            {@render renderMessageTimestamp(turn.completedAt, ui.agentReplyTimestamp, "left")}
                          {/if}
                        {:else}
                          {@const fullEntries = turnModel.fullEntries}
                          {@const hiddenFullEntryCount = getHiddenTurnEntryCount(turn.id, fullEntries)}
                          {@const visibleFullEntries = getVisibleTurnEntries(turn.id, fullEntries)}
                          {@render renderHiddenTurnEntriesControl(turn.id, hiddenFullEntryCount, fullEntries)}
                          {#each visibleFullEntries as entry (entry.key)}{@render renderTurnEntry(turn.id, entry, 0)}{/each}
                          {#if turn.completedAt}
                            {@render renderMessageTimestamp(turn.completedAt, ui.agentReplyTimestamp, "left")}
                          {/if}
                        {/if}
                        
                        {#if conversation.livePlans[turn.id] && turn.id !== conversation.activeTurnId}
                          <div class="turn-card-shell w-full min-w-0 border border-amber-100 rounded-xl bg-amber-50/30 overflow-hidden">
                            <div class="turn-card-header turn-card-header--amber px-4 py-2 border-b border-amber-100 flex items-center gap-2 text-[10px] font-bold text-amber-700 uppercase tracking-widest" data-sticky-level="0"><ListTodo size={12} /><span>{ui.livePlan}</span></div>
                            <div class="p-4 text-sm text-gray-700 space-y-3">
                              {#if conversation.livePlans[turn.id].explanation}<p class="leading-relaxed">{conversation.livePlans[turn.id].explanation}</p>{/if}
                              <ul class="space-y-1.5 pl-2">
                                {#each conversation.livePlans[turn.id].plan as step (`${step.step}:${step.status}`)}
                                  <li class="flex items-start gap-2 text-xs">
                                    <span class="mt-1 flex-shrink-0 w-1.5 h-1.5 rounded-full {step.status === 'completed' ? 'bg-emerald-500' : 'bg-amber-400 animate-pulse'}"></span>
                                    <span class="font-medium {step.status === 'completed' ? 'text-gray-400 line-through' : 'text-gray-600'}">{step.step}</span>
                                  </li>
                                {/each}
                              </ul>
                            </div>
                          </div>
                        {/if}
                        {#if conversation.liveDiffs[turn.id] && turn.id !== conversation.activeTurnId}
                          {@const turnDiffStats = diffLineStats(conversation.liveDiffs[turn.id])}
                          <div class="turn-card-shell group bg-gray-50 border border-gray-200 rounded-xl overflow-hidden">
                            <button
                              class="turn-card-header turn-card-header--neutral flex w-full items-center justify-between px-4 py-2 text-[10px] font-bold text-gray-500 uppercase tracking-widest hover:bg-gray-100 transition-colors"
                              data-sticky-level="0"
                              onclick={() => toggleLiveDiff(turn.id)}
                              type="button"
                            >
                              <div class="flex items-center gap-2"><FileDiff size={12} /> {ui.aggregatedDiff}</div>
                              <div class="flex items-center gap-1.5">
                                <span class="rounded-full bg-emerald-50 px-2 py-1 text-emerald-700">+{turnDiffStats.added}</span>
                                <span class="rounded-full bg-red-50 px-2 py-1 text-red-700">-{turnDiffStats.removed}</span>
                                <ChevronDown size={12} class="{isLiveDiffExpanded(turn.id) ? 'rotate-180' : ''} transition-transform" />
                              </div>
                            </button>
                            {#if isLiveDiffExpanded(turn.id)}
                              <div class="turn-card-expand border-t border-gray-200 bg-white" transition:slide|local={{ duration: 220 }}>
                                <div class="flex items-center justify-end gap-3 px-4 py-2 bg-white">
                                  <button class="rounded-lg border border-gray-200 bg-white px-3 py-1.5 text-[10px] font-bold text-gray-700 hover:bg-gray-50 transition-colors" onclick={() => openLiveDiffTab(turn.id, conversation!.liveDiffs[turn.id]!)} type="button">{ui.openTab}</button>
                                </div>
                                {#if parseAggregatedDiffViews(conversation.liveDiffs[turn.id]).length > 0}
                                  <div class="border-t border-gray-200 bg-white">
                                    {#each parseAggregatedDiffViews(conversation.liveDiffs[turn.id]) as change}
                                      <div class="border-b border-gray-100 last:border-b-0">
                                        <button class="turn-card-header turn-card-header--neutral flex w-full items-center justify-between gap-3 px-4 py-2 text-left hover:bg-gray-50 transition-colors" data-sticky-level="1" onclick={() => toggleFileChangeEntry(turn.id, `live-diff:${turn.id}`, change)} type="button">
                                          <div class="min-w-0">
                                            <p class="truncate text-[11px] font-bold text-gray-700">{change.path}</p>
                                            <p class="mt-0.5 text-[10px] uppercase tracking-widest text-gray-400">{change.kind}</p>
                                          </div>
                                          <ChevronDown size={12} class="text-gray-300 {isFileChangeEntryExpanded(turn.id, `live-diff:${turn.id}`, change) ? 'rotate-180' : ''} transition-transform" />
                                        </button>
                                        {#if isFileChangeEntryExpanded(turn.id, `live-diff:${turn.id}`, change)}
                                          <div class="turn-card-expand border-t border-gray-100 bg-gray-50" transition:slide|local={{ duration: 180 }}>
                                            {#if change.renderable}
                                              <LazyMonacoDiffEditor fallbackText={change.diff} height={400} modified={change.modified} original={change.original} path={change.path} />
                                            {:else}
                                              <pre class="p-4 text-xs font-mono overflow-x-auto text-gray-600">{change.diff}</pre>
                                            {/if}
                                          </div>
                                        {/if}
                                      </div>
                                    {/each}
                                  </div>
                                {:else}
                                  <pre class="border-t border-gray-200 bg-gray-50/30 p-4 text-xs font-mono overflow-x-auto text-gray-600">{conversation.liveDiffs[turn.id]}</pre>
                                {/if}
                              </div>
                            {/if}
                          </div>
                        {/if}
                      </div>
                    </div>
                  </div>
                {/each}
                  {#if transcriptTurnWindow.bottomSpacer > 0}
                    <div aria-hidden="true" style={`height:${transcriptTurnWindow.bottomSpacer}px`}></div>
                  {/if}
                </div>

                {#if standaloneOptimisticMessage}
                  <div class="flex flex-col items-end gap-3 max-w-[85%] ml-auto opacity-70">
                    <div class="px-5 py-3 bg-gray-100 rounded-2xl text-gray-800 shadow-sm border border-gray-200/50">
                      <MarkdownMessage compact expandLabel={ui.showFullMessage} maxInitialChars={compactMarkdownInitialChars} text={standaloneOptimisticMessage.prompt} />
                    </div>
                  </div>
                {/if}

                {#if inlineGenerationState}
                  <div class="flex py-3">
                    <div class="thinking-indicator inline-flex items-center rounded-2xl border border-gray-200/80 bg-white/90 px-4 py-3 shadow-sm">
                      <span class="thinking-indicator__label text-sm font-medium">{inlineGenerationState.label}</span>
                    </div>
                  </div>
                {/if}

                {#if conversation.pendingRequests.length > 0}
                  <div class="space-y-4 pt-4">
                    {#each conversation.pendingRequests as request (request.id)}
                      <div class="bg-white border-2 border-amber-500/20 rounded-2xl shadow-xl overflow-hidden animate-in zoom-in-95 duration-300">
                        <div class="px-5 py-3 bg-amber-50 border-b border-amber-100 flex items-center justify-between">
                          <div class="flex items-center gap-2"><AlertCircle size={16} class="text-amber-600" /><span class="text-xs font-bold text-amber-900 uppercase tracking-widest">{ui.approvalRequired}</span></div>
                          <span class="text-[10px] font-mono text-amber-600/60">{request.id}</span>
                        </div>
                        <div class="p-5 space-y-4">
                          {#if request.method.includes('Approval')}
                            <p class="text-sm text-gray-700 leading-relaxed font-medium">{String(request.params.reason ?? "Codex is requesting approval.")}</p>
                            <div class="flex flex-wrap gap-2">
                              <button class="flex-1 px-4 py-2 bg-amber-600 text-white rounded-xl text-xs font-bold hover:bg-amber-700 shadow-sm transition-all" onclick={() => void resolvePendingRequest(request, { decision: "accept" })}>{m.approve()}</button>
                              <button class="flex-1 px-4 py-2 bg-amber-100 text-amber-700 border border-amber-200 rounded-xl text-xs font-bold hover:bg-amber-200 transition-all" onclick={() => void resolvePendingRequest(request, { decision: "acceptForSession" })}>{m.session()}</button>
                              <button class="flex-1 px-4 py-2 bg-gray-100 text-gray-600 rounded-xl text-xs font-bold hover:bg-gray-200 transition-all" onclick={() => void resolvePendingRequest(request, { decision: "decline" })}>{m.decline()}</button>
                            </div>
                          {:else if request.method === "item/tool/requestUserInput"}
                            <div class="space-y-4">
                              {#each Array.isArray(request.params.questions) ? request.params.questions : [] as question}
                                <div class="space-y-2">
                                  <div class="block text-xs font-bold text-gray-500 uppercase tracking-wider">{String(question.header ?? ui.inputRequired)}</div>
                                  <p class="text-sm font-medium text-gray-900">{String(question.question ?? "")}</p>
                                  {#if Array.isArray(question.options)}
                                    <div class="grid grid-cols-1 md:grid-cols-2 gap-2">
                                      {#each question.options as option}
                                        <button class="flex flex-col p-3 rounded-xl border transition-all text-left {requestAnswers[request.id]?.[String(question.id)] === String(option.label) ? 'bg-amber-50 border-amber-500 ring-1 ring-amber-500/20' : 'bg-white border-gray-200 hover:border-amber-300'}" onclick={() => setRequestAnswer(request.id, String(question.id), String(option.label))}>
                                          <span class="text-sm font-bold text-gray-900">{String(option.label)}</span>
                                          <span class="text-[10px] text-gray-500 mt-1">{String(option.description ?? "")}</span>
                                        </button>
                                      {/each}
                                    </div>
                                  {/if}
                                  <input class="w-full px-4 py-3 bg-gray-50 border border-gray-200 rounded-xl text-sm focus:outline-none focus:ring-2 focus:ring-amber-500/10 focus:border-amber-500 transition-all" oninput={(event) => setRequestAnswer(request.id, String(question.id), (event.currentTarget as HTMLInputElement).value)} placeholder={ui.typeYourResponse} type={question.isSecret ? "password" : "text"} value={requestAnswers[request.id]?.[String(question.id)] ?? ""} />
                                </div>
                              {/each}
                              <button class="w-full py-3 bg-amber-600 text-white rounded-xl text-sm font-bold hover:bg-amber-700 shadow-md transition-all active:scale-[0.98]" onclick={() => void submitRequestUserInput(request)}>{ui.submitResponse}</button>
                            </div>
                          {:else if request.method === "item/tool/call"}
                            <div class="space-y-3">
                              <div class="rounded-xl border border-gray-200 bg-gray-50 p-3">
                                <div class="flex flex-wrap items-center gap-2 text-xs font-bold text-gray-500">
                                  <span>{m.tool_call()}</span>
                                  <span class="rounded-full bg-white px-2 py-0.5 text-gray-700">
                                    {String(request.params.namespace ?? "tool")} / {String(request.params.tool ?? "unknown")}
                                  </span>
                                </div>
                                <pre class="mt-2 max-h-40 overflow-auto whitespace-pre-wrap break-words rounded-lg bg-white p-2 text-[11px] text-gray-600">{JSON.stringify(request.params.arguments ?? {}, null, 2)}</pre>
                              </div>
                              <textarea
                                class="min-h-24 w-full rounded-xl border border-gray-200 bg-gray-50 px-3 py-2 text-sm text-gray-800 focus:border-amber-500 focus:outline-none"
                                oninput={(event) => updateRawRequestResponse(request.id, (event.currentTarget as HTMLTextAreaElement).value)}
                                placeholder={ui.typeYourResponse}
                                value={rawRequestResponses[request.id] ?? ""}
                              ></textarea>
                              <div class="flex flex-wrap gap-2">
                                <button
                                  class="flex-1 rounded-xl bg-amber-600 px-4 py-2 text-xs font-bold text-white shadow-sm transition-all hover:bg-amber-700"
                                  onclick={() => void resolvePendingRequest(request, {
                                    contentItems: [{ type: "inputText", text: rawRequestResponses[request.id] ?? "" }],
                                    success: true
                                  })}
                                  type="button"
                                >
                                  {ui.submitResponse}
                                </button>
                                <button
                                  class="flex-1 rounded-xl bg-gray-100 px-4 py-2 text-xs font-bold text-gray-600 transition-all hover:bg-gray-200"
                                  onclick={() => void resolvePendingRequest(request, {
                                    contentItems: [{ type: "inputText", text: rawRequestResponses[request.id] || "Tool call declined." }],
                                    success: false
                                  })}
                                  type="button"
                                >
                                  Decline
                                </button>
                              </div>
                            </div>
                          {/if}
                        </div>
                      </div>
                    {/each}
                  </div>
                {/if}
              {/if}
            </div>
          </div>

          <!-- Bottom Area -->
          <div bind:this={transcriptDockElement} class="transcript-dock pointer-events-none absolute inset-x-0 bottom-0 z-[82] px-6 pb-6 pt-4">
            {#if showComposerSyncPill}
              <div class="pointer-events-none absolute inset-x-0 top-0 flex justify-center px-6">
                <div class="-translate-y-[calc(100%+0.5rem)]">
                  <div class="dock-sync-pill" transition:fly={{ y: 8, duration: 180 }}>
                    <RefreshCw size={12} class={topLoadKind === "sessionHydration" ? "" : "animate-spin"} />
                    <span>{sessionSyncLabel}</span>
                  </div>
                </div>
              </div>
            {/if}
            <div class="pointer-events-auto max-w-3xl mx-auto w-full space-y-4">
              {#if pendingSteerResume && selectedSessionBindingMatches(pendingSteerResume.sessionId, pendingSteerResume.profileId)}
                <div class="p-4 bg-amber-600 text-white rounded-2xl shadow-xl flex flex-col md:flex-row items-center gap-4 animate-in slide-in-from-bottom-8 duration-500">
                  <div class="flex-1"><p class="text-sm font-bold flex items-center gap-2"><Clock size={16} /> {ui.savedDraftFound}</p><p class="text-xs opacity-90 mt-0.5">{ui.resumeSavedSteeringPrompt}</p></div>
                  <div class="flex gap-2"><button class="px-4 py-1.5 bg-white text-amber-700 rounded-lg text-xs font-bold hover:bg-amber-50 shadow-sm" onclick={() => void resumeSavedSteer()}>{ui.resume}</button><button class="px-4 py-1.5 bg-amber-700/50 hover:bg-amber-700 text-white rounded-lg text-xs font-bold transition-colors" onclick={keepSavedDraftInComposer}>{ui.keepDraft}</button><button class="p-1.5 text-white/70 hover:text-white rounded-lg transition-colors" onclick={() => void discardSavedDraft()}><X size={16} /></button></div>
                </div>
              {/if}

              {#if showQueueResumeBanner && selectedSessionQueue}
                <div class="queue-resume-banner flex flex-col items-center gap-3 rounded-xl bg-gray-900 p-3 text-white shadow-xl animate-in slide-in-from-bottom-8 duration-500 md:flex-row">
                  <div class="flex-1"><p class="flex items-center gap-2 text-[13px] font-bold"><RefreshCw size={15} /> {ui.queuedWorkPaused}</p><p class="mt-0.5 text-[11px] opacity-80">{m.tasks_waiting({ count: String(selectedSessionQueue.items.length) })}</p></div>
                  <div class="flex gap-1.5"><button class="rounded-lg bg-amber-600 px-3 py-1.25 text-[11px] font-bold text-white shadow-sm hover:bg-amber-700" onclick={() => void resumeQueuedMessages()}>{ui.resumeQueue}</button><button class="queue-resume-ignore rounded-lg bg-gray-800 px-3 py-1.25 text-[11px] font-bold text-white transition-colors hover:bg-gray-700" onclick={() => { if (!selectedSessionId) return; dismissedQueueResumeBySessionId = { ...dismissedQueueResumeBySessionId, [sessionStateKey(selectedSessionId, selectedSessionProfileId)]: true }; }}>{ui.ignore}</button></div>
                </div>
              {/if}

              {#if queuedMessages.length > 0}
                <div class="overflow-hidden rounded-xl border border-gray-200 bg-gray-50/80 shadow-sm">
                  <button
                    class={`flex w-full items-center justify-between gap-2.5 bg-white/80 px-2.5 py-1.5 text-left transition-colors hover:bg-white ${queuedFollowupsExpanded ? "border-b border-gray-200" : ""}`}
                    onclick={() => (queuedFollowupsExpanded = !queuedFollowupsExpanded)}
                    type="button"
                  >
                    <div class="flex min-w-0 items-center gap-2">
                      <p class="truncate text-[11px] font-bold uppercase tracking-widest text-gray-900">{ui.queuedFollowups}</p>
                      <span class="inline-flex min-w-[1.35rem] items-center justify-center rounded-full bg-gray-900/6 px-1.5 py-0.5 text-[10px] font-bold tabular-nums text-gray-600">
                        {queuedMessages.length}
                      </span>
                    </div>
                    <div class="flex items-center gap-2.5">
                      {#if conversation?.queue.resumeRequired}
                        <span class="px-2 py-1 rounded-full bg-amber-100 text-[10px] font-bold text-amber-700 uppercase tracking-widest">{ui.paused}</span>
                      {/if}
                      <ChevronDown size={15} class={`text-gray-400 transition-transform ${queuedFollowupsExpanded ? "rotate-180" : ""}`} />
                    </div>
                  </button>
                  {#if queuedFollowupsExpanded}
                    <div
                      class="max-h-48 overflow-y-auto divide-y divide-gray-200 overscroll-contain"
                      transition:slide|local={{ duration: 220 }}
                    >
                      {#each queuedMessages as item (item.id)}
                        <div
                          class={`relative px-2.5 py-1.5 transition-colors ${queueDragState?.queueId === item.id ? "bg-amber-50/60" : ""}`}
                          data-queue-item-id={item.id}
                        >
                          {#if showQueueDropIndicator(item.id, "before")}
                            <div class="pointer-events-none absolute inset-x-3 top-0 h-0.5 rounded-full bg-amber-400 shadow-[0_0_0_1px_rgba(251,191,36,0.18)]"></div>
                          {/if}
                          {#if showQueueDropIndicator(item.id, "after")}
                            <div class="pointer-events-none absolute inset-x-3 bottom-0 h-0.5 rounded-full bg-amber-400 shadow-[0_0_0_1px_rgba(251,191,36,0.18)]"></div>
                          {/if}
                          <div class="flex items-start gap-2">
                            <button
                              aria-label={ui.reorderQueue ?? "Reorder queued message"}
                              class={`mt-0.5 inline-flex h-6.5 w-6.5 shrink-0 items-center justify-center rounded-md border border-gray-200 bg-white text-gray-400 transition-colors ${queueReorderBusy || serverQueuedMessages.length < 2 || isOptimisticQueueItem(item) ? "cursor-not-allowed opacity-45" : queueDragState?.queueId === item.id ? "cursor-grabbing border-amber-300 bg-amber-50 text-amber-700 shadow-sm" : "cursor-grab hover:bg-gray-100 hover:text-gray-700"}`}
                              disabled={queueReorderBusy || serverQueuedMessages.length < 2 || isOptimisticQueueItem(item)}
                              onlostpointercapture={cancelQueueDrag}
                              onpointercancel={cancelQueueDrag}
                              onpointerdown={(event) => startQueueDrag(event, item.id)}
                              onpointermove={moveQueueDrag}
                              onpointerup={endQueueDrag}
                              style="touch-action: none;"
                              title={ui.reorderQueue ?? "Reorder queued message"}
                              type="button"
                            >
                              <GripVertical size={12} />
                            </button>
                            <div class="min-w-0 flex-1">
                              <div class={`grid min-w-0 gap-1.5 ${editingQueueId === item.id ? "grid-cols-1" : "sm:grid-cols-[minmax(0,1fr)_auto] sm:items-center"}`}>
                                <div class="min-w-0 space-y-0.75">
                                  {#if editingQueueId === item.id}
                                    <div class="space-y-1.5">
                                      <textarea
                                        bind:value={editingQueuePrompt}
                                        class="w-full min-h-[3.5rem] rounded-lg border border-gray-200 bg-white px-2.5 py-1.5 text-[13px] text-gray-700 shadow-sm focus:border-amber-500 focus:outline-none focus:ring-2 focus:ring-amber-100"
                                        onkeydown={(event) => {
                                          if (event.key === "Escape") {
                                            event.preventDefault();
                                            cancelQueuedMessageEdit();
                                          }
                                          if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
                                            event.preventDefault();
                                            void saveQueuedMessage(item.id);
                                          }
                                        }}
                                        placeholder={m.edit_queued_followup()}
                                      ></textarea>
                                      {#if item.attachmentNames.length > 0}
                                        <div class="flex flex-wrap gap-1">
                                          {#each item.attachmentNames as attachmentName}
                                            <span class="rounded-full border border-gray-200 bg-white px-1.75 py-0.5 text-[10px] text-gray-500">{attachmentName}</span>
                                          {/each}
                                        </div>
                                      {/if}
                                      <p class="text-[10px] text-gray-400">{m.attached_files_stay()}</p>
                                    </div>
                                  {:else}
                                    <div class="flex flex-wrap items-center gap-1.5">
                                      <p class="min-w-0 flex-1 text-[11px] leading-4 text-gray-700 break-words sm:truncate">{summarizeQueueItem(item)}</p>
                                    </div>
                                    {#if item.attachmentNames.length > 0}
                                      <div class="flex flex-wrap gap-1">
                                        <span class="inline-flex items-center gap-1 rounded-full border border-gray-200 bg-white px-1.75 py-0.5 text-[9px] font-semibold text-gray-500">
                                          <Paperclip size={9} />
                                          <span>{m.attached_files_count({ count: String(item.attachmentNames.length) })}</span>
                                        </span>
                                      </div>
                                    {/if}
                                  {/if}
                                </div>
                                <div class={`flex flex-wrap items-center gap-1 ${editingQueueId === item.id ? "" : "sm:justify-end"}`}>
                                  {#if editingQueueId === item.id}
                                    <button class="inline-flex h-6.5 items-center justify-center rounded-md border border-emerald-200 bg-emerald-50 px-1.75 text-[10px] font-bold text-emerald-700 transition-colors hover:bg-emerald-100 disabled:opacity-50" disabled={queueReorderBusy} onclick={() => void saveQueuedMessage(item.id)} type="button">{ui.queueSave}</button>
                                    <button class="inline-flex h-6.5 items-center justify-center rounded-md border border-gray-200 bg-white px-1.75 text-[10px] font-bold text-gray-700 transition-colors hover:bg-gray-100 disabled:opacity-50" onclick={cancelQueuedMessageEdit} type="button">{ui.cancel}</button>
                                  {:else}
                                    <button class="inline-flex h-6.5 items-center justify-center rounded-md border border-amber-200 bg-amber-50 px-1.75 text-[10px] font-bold text-amber-700 transition-colors hover:bg-amber-100 disabled:opacity-50" disabled={sending || queueReorderBusy || hasPendingQueueRequests || isOptimisticQueueItem(item)} onclick={() => void dispatchQueuedMessage(item.id, "steer")} type="button">{ui.steerNow}</button>
                                    <button class="inline-flex h-6.5 items-center justify-center rounded-md border border-gray-200 bg-white px-1.75 text-[10px] font-bold text-gray-700 transition-colors hover:bg-gray-100 disabled:opacity-50" disabled={sending || queueReorderBusy || hasPendingQueueRequests || isOptimisticQueueItem(item)} onclick={() => void dispatchQueuedMessage(item.id, "message")} type="button">{ui.sendNow}</button>
                                    <button
                                      aria-label={ui.edit}
                                      class="inline-flex h-6.5 w-6.5 items-center justify-center rounded-md border border-gray-200 bg-white p-0 text-gray-500 transition-colors hover:bg-gray-100 hover:text-gray-700 disabled:opacity-50"
                                      disabled={queueReorderBusy}
                                      onclick={() => beginQueuedMessageEdit(item)}
                                      title={ui.edit}
                                      type="button"
                                    >
                                      <Pencil size={14} />
                                    </button>
                                  {/if}
                                  <button class="inline-flex h-6.5 w-6.5 items-center justify-center rounded-md p-0 text-gray-400 transition-colors hover:bg-red-50 hover:text-red-600 disabled:opacity-50" aria-label={m.remove_queued_message()} disabled={queueReorderBusy} onclick={() => void removeQueuedMessage(item.id)} type="button"><Trash2 size={13} /></button>
                                </div>
                              </div>
                            </div>
                          </div>
                        </div>
                      {/each}
                    </div>
                  {/if}
                </div>
              {/if}

              {#if activeLiveTurnId}
                <div class="live-turn-card turn-card-shell border border-amber-200 rounded-2xl bg-white shadow-sm overflow-hidden">
                  <button class="turn-card-header turn-card-header--amber w-full flex items-center justify-between gap-2.5 px-3 py-2 hover:bg-amber-50/40 transition-colors" data-sticky-level="0" onclick={() => (liveTurnCardExpanded = !liveTurnCardExpanded)} type="button">
                    <div class="min-w-0 flex items-center gap-2.5">
                      <div class="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-amber-50 text-amber-700 border border-amber-100">
                        <Zap size={13} />
                      </div>
                      <div class="min-w-0 text-left">
                        <div class="flex flex-wrap items-center gap-1.5">
                          <p class="text-[11px] font-bold uppercase tracking-widest text-amber-700">{ui.liveTurn}</p>
                          {#if activeLiveTurnPlan}<span class="rounded-full bg-amber-50 px-1.5 py-0.5 text-[9px] font-bold text-amber-700">{m.steps_count({ count: String(activeLiveTurnPlan.plan.length) })}</span>{/if}
                          {#if activeLiveTurnDiff}
                            <span class="rounded-full bg-gray-100 px-1.5 py-0.5 text-[9px] font-bold text-gray-600">
                              {m.files_count({ count: String(activeLiveTurnDiffViews.length > 0 ? activeLiveTurnDiffViews.length : 1) })}
                            </span>
                          {/if}
                          {#if activeLiveTurnSubagents.length > 0}
                            <span class="rounded-full bg-sky-50 px-1.5 py-0.5 text-[9px] font-bold text-sky-700">
                              {activeLiveTurnSubagents.length} {ui.tasks}
                            </span>
                          {/if}
                          {#if conversation?.goal}
                            <span class="rounded-full bg-amber-50 px-1.5 py-0.5 text-[9px] font-bold text-amber-700">
                              {conversation.goal.status}
                            </span>
                          {/if}
                        </div>
                      </div>
                    </div>
                    <div class="flex items-center gap-1 shrink-0">
                      {#if activeLiveTurnDiff}
                        <span class="rounded-full bg-emerald-50 px-1.5 py-0.5 text-[9px] font-bold text-emerald-700">+{diffLineStats(activeLiveTurnDiff).added}</span>
                        <span class="rounded-full bg-red-50 px-1.5 py-0.5 text-[9px] font-bold text-red-700">-{diffLineStats(activeLiveTurnDiff).removed}</span>
                      {/if}
                      <ChevronDown size={13} class="text-gray-400 {liveTurnCardExpanded ? 'rotate-180' : ''} transition-transform" />
                    </div>
                  </button>
                  {#if liveTurnCardExpanded}
                    <div class="turn-card-expand grid gap-2.5 border-t border-amber-100 bg-amber-50/20 p-2.5 {activeLiveTurnPlan && activeLiveTurnDiff && activeLiveTurnSubagents.length > 0 ? 'lg:grid-cols-2 xl:grid-cols-3' : 'lg:grid-cols-2'}" transition:slide|local={{ duration: 220 }}>
                      {#if activeLiveTurnDiff}
                        <div class="turn-card-shell rounded-xl border border-gray-200 bg-white/85 overflow-hidden lg:col-span-full">
                          <div class="turn-card-header turn-card-header--neutral flex items-center justify-between gap-2 border-b border-gray-200 px-2.5 py-2" data-sticky-level="1">
                            <div class="flex items-center gap-2 text-[10px] font-bold uppercase tracking-[0.18em] text-gray-500">
                              <FileDiff size={12} />
                              <span>{ui.aggregatedDiff}</span>
                            </div>
                            <button class="rounded-lg border border-gray-200 bg-white px-2.5 py-1 text-[10px] font-bold text-gray-700 hover:bg-gray-50 transition-colors" onclick={() => openLiveDiffTab(activeLiveTurnId, activeLiveTurnDiff)} type="button">{ui.openTab}</button>
                          </div>
                          {#if activeLiveTurnDiffViews.length > 0}
                            <div class="live-turn-scroll max-h-72 overflow-auto">
                              {#each activeLiveTurnDiffViews as change (`${change.path}:${change.kind}`)}
                                <div class="border-t border-gray-100 first:border-t-0">
                                  <button class="turn-card-header turn-card-header--neutral flex w-full items-center justify-between gap-3 px-2.5 py-2 text-left hover:bg-gray-50 transition-colors" data-sticky-level="2" onclick={() => toggleFileChangeEntry(activeLiveTurnId, "live-diff:active", change)} type="button">
                                    <div class="min-w-0">
                                      <p class="truncate text-[11px] font-bold text-gray-700">{change.path}</p>
                                      <p class="mt-0.5 text-[10px] uppercase tracking-[0.18em] text-gray-400">{change.kind}</p>
                                    </div>
                                    <ChevronDown size={12} class="text-gray-300 {isFileChangeEntryExpanded(activeLiveTurnId, 'live-diff:active', change) ? 'rotate-180' : ''} transition-transform" />
                                  </button>
                                  {#if isFileChangeEntryExpanded(activeLiveTurnId, "live-diff:active", change)}
                                    <div class="turn-card-expand border-t border-gray-100 bg-gray-50" transition:slide|local={{ duration: 180 }}>
                                      {#if change.renderable}
                                        <LazyMonacoDiffEditor fallbackText={change.diff} height={400} modified={change.modified} original={change.original} path={change.path} />
                                      {:else}
                                        <pre class="overflow-x-auto p-3 text-[11px] leading-relaxed text-gray-600">{change.diff}</pre>
                                      {/if}
                                    </div>
                                  {/if}
                                </div>
                              {/each}
                            </div>
                          {:else}
                            <pre class="max-h-36 overflow-auto p-3 text-[11px] leading-relaxed text-gray-600">{activeLiveTurnDiff}</pre>
                          {/if}
                        </div>
                      {/if}
                      {#if activeLiveTurnPlan}
                        <div class="turn-card-shell w-full min-w-0 rounded-xl border border-amber-100 bg-white/85 p-2.5 lg:col-span-full">
                          <div class="turn-card-header turn-card-header--amber mb-1.5 flex items-center gap-2 rounded-xl px-2 py-2 text-[10px] font-bold uppercase tracking-[0.18em] text-amber-700" data-sticky-level="1">
                            <ListTodo size={12} />
                            <span>{ui.livePlan}</span>
                          </div>
                          {#if activeLiveTurnPlan.explanation}
                            <p class="text-[11px] leading-relaxed text-gray-600">{activeLiveTurnPlan.explanation}</p>
                          {/if}
                          <ul class="mt-2 max-h-72 space-y-1.5 overflow-auto pr-1">
                            {#each activeLiveTurnPlan.plan as step (`${step.step}:${step.status}`)}
                              <li class="flex items-start gap-2 text-[11px] leading-4">
                                <span class="mt-1 h-1.5 w-1.5 flex-shrink-0 rounded-full {step.status === 'completed' ? 'bg-emerald-500' : 'bg-amber-400 animate-pulse'}"></span>
                                <span class="{step.status === 'completed' ? 'text-gray-400 line-through' : 'text-gray-700'}">{step.step}</span>
                              </li>
                            {/each}
                          </ul>
                        </div>
                      {/if}
                      {#if activeLiveTurnSubagents.length > 0}
                        <div class="turn-card-shell rounded-xl border border-sky-100 bg-white/85 overflow-hidden">
                          <div class="turn-card-header turn-card-header--neutral flex items-center justify-between gap-2 border-b border-sky-100 px-2.5 py-2" data-sticky-level="1">
                            <div class="flex items-center gap-2 text-[10px] font-bold uppercase tracking-[0.18em] text-sky-700">
                              <Bot size={12} />
                              <span>{ui.subagentActivities}</span>
                            </div>
                            <span class="rounded-full bg-sky-50 px-2 py-0.5 text-[10px] font-bold text-sky-700">
                              {activeLiveTurnSubagents.length}
                            </span>
                          </div>
                          <div class="live-turn-scroll max-h-72 overflow-auto">
                            {#each activeLiveTurnSubagents as task (task.key)}
                              <div class="flex items-start justify-between gap-2 border-t border-sky-50 px-2.5 py-2.5 first:border-t-0">
                                <div class="min-w-0 flex-1">
                                  <div class="flex flex-wrap items-center gap-1.5">
                                    <p class="truncate text-[11px] font-bold text-gray-800">{task.tool}</p>
                                    <span class="rounded-full px-1.5 py-0.5 text-[9px] font-bold uppercase tracking-[0.14em] {['completed', 'done', 'success'].includes(task.status.toLowerCase()) ? 'bg-emerald-50 text-emerald-700' : ['failed', 'error'].includes(task.status.toLowerCase()) ? 'bg-red-50 text-red-700' : ['queued', 'pending', 'waiting'].includes(task.status.toLowerCase()) ? 'bg-gray-100 text-gray-500' : 'bg-amber-50 text-amber-700'}">
                                      {task.status}
                                    </span>
                                  </div>
                                  <p class="mt-1 truncate text-[10px] text-gray-500">
                                    {task.prompt || `${task.model} · ${task.reasoningEffort}`}
                                  </p>
                                  {#if task.prompt}
                                    <p class="mt-1 truncate text-[10px] text-gray-400">{task.model} · {task.reasoningEffort}</p>
                                  {/if}
                                  {#if task.states.length > 0}
                                    <div class="mt-1.5 flex flex-wrap gap-1">
                                      {#each task.states.slice(0, 2) as [name, state]}
                                        <span class="rounded-full bg-gray-100 px-1.5 py-0.5 text-[9px] font-medium text-gray-500">
                                          {name}: {state.status ?? "running"}
                                        </span>
                                      {/each}
                                      {#if task.states.length > 2}
                                        <span class="rounded-full bg-gray-100 px-1.5 py-0.5 text-[9px] font-medium text-gray-500">+{task.states.length - 2}</span>
                                      {/if}
                                    </div>
                                  {/if}
                                </div>
                                {#if task.primaryThreadId}
                                  <button class="shrink-0 rounded-lg border border-sky-100 bg-white px-2.5 py-1 text-[10px] font-bold text-sky-700 hover:bg-sky-50 transition-colors" onclick={() => void openSubagentThread(task.primaryThreadId ?? "")} type="button">
                                    {ui.viewThread}
                                  </button>
                                {/if}
                              </div>
                            {/each}
                          </div>
                        </div>
                      {/if}
                    </div>
                  {/if}
                </div>
              {/if}

              <div class="relative group">
                {#if lastComposerHistoryPrompt}
                  <div class="mb-1.5 flex items-center gap-1.5 px-0.5">
                    <button
                      class="ui-animated-button ui-animated-button--soft flex min-w-0 flex-1 items-center gap-1.5 rounded-xl border border-gray-200/80 bg-white/80 px-2.5 py-1.25 text-left text-[10px] text-gray-500 shadow-sm backdrop-blur-sm transition-colors hover:border-amber-200 hover:bg-amber-50/70 hover:text-gray-700"
                      onclick={reuseLastComposerMessage}
                      title={ui.editInComposer}
                      type="button"
                    >
                      <Clock size={12} class="shrink-0 text-gray-400" />
                      <span class="truncate font-medium">{lastComposerHistoryPrompt}</span>
                    </button>
                    <button
                      aria-label={ui.close}
                      class="ui-animated-button ui-animated-button--icon flex h-7 w-7 shrink-0 items-center justify-center rounded-lg border border-gray-200/80 bg-white/80 text-gray-400 shadow-sm transition-colors hover:border-gray-300 hover:bg-gray-50 hover:text-gray-600"
                      onclick={dismissLastComposerPromptChip}
                      title={ui.close}
                      type="button"
                    >
                      <X size={14} />
                    </button>
                    <button
                      class="surface-contrast-button ui-animated-button ui-animated-button--soft flex h-7 shrink-0 items-center gap-1.25 rounded-lg px-2.5 text-[10px] font-bold shadow-sm"
                      disabled={recentComposerActionDisabled}
                      onclick={() => void resendLastComposerMessage()}
                      title={composerQueueModeActive ? ui.queue : ui.send}
                      type="button"
                    >
                      <span class="hidden sm:inline">{composerQueueModeActive ? ui.queue : ui.send}</span>
                      <Send size={13} />
                    </button>
                  </div>
                {/if}
                {#if slashSuggestions.length > 0}
                  <div class="mb-1.5 grid gap-1 rounded-xl border border-amber-100 bg-white/90 p-1.5 shadow-sm">
                    {#each slashSuggestions as suggestion (suggestion.key)}
                      <button
                        class="ui-animated-button ui-animated-button--soft flex items-start justify-between gap-2.5 rounded-lg px-2.5 py-1.5 text-left transition-colors hover:bg-amber-50/70"
                        onclick={() => applySlashSuggestion(suggestion)}
                        type="button"
                      >
                        <div class="min-w-0">
                          <p class="text-[11px] font-bold text-gray-800">{suggestion.title}</p>
                          <p class="mt-0.5 truncate text-[10px] text-gray-500">{suggestion.description}</p>
                        </div>
                        <span class="rounded-full bg-gray-100 px-2 py-0.5 text-[9px] font-bold uppercase tracking-[0.18em] text-gray-500">
                          {suggestion.command}
                        </span>
                      </button>
                    {/each}
                  </div>
                {/if}
                {#if fileMentionTrigger}
                  <div class="mb-1.5 overflow-hidden rounded-xl border border-gray-200 bg-white/95 p-1.5 shadow-lg shadow-gray-200/60">
                    <div class="flex items-center justify-between gap-2 px-2 py-1">
                      <div class="flex min-w-0 items-center gap-1.5 text-[10px] font-bold uppercase tracking-[0.18em] text-gray-400">
                        <FileText size={12} />
                        <span>{m.file_mentions()}</span>
                      </div>
                      {#if fileMentionBusy}
                        <RefreshCw size={12} class="shrink-0 animate-spin text-amber-500" />
                      {/if}
                    </div>
                    {#if fileMentionResults.length > 0}
                      <div class="grid gap-0.5">
                        {#each fileMentionResults as entry, index (entry.path)}
                          <button
                            class={`ui-animated-button ui-animated-button--soft flex items-center gap-2 rounded-lg px-2.5 py-1.5 text-left transition-colors ${
                              index === fileMentionActiveIndex ? "bg-amber-50 text-gray-900" : "text-gray-700 hover:bg-gray-50"
                            }`}
                            onmouseenter={() => (fileMentionActiveIndex = index)}
                            onmousedown={(event) => event.preventDefault()}
                            onclick={() => void insertFileMention(entry)}
                            type="button"
                          >
                            <FileText size={13} class={index === fileMentionActiveIndex ? "shrink-0 text-amber-600" : "shrink-0 text-gray-400"} />
                            <div class="min-w-0 flex-1">
                              <p class="truncate text-[11px] font-bold">{entry.name}</p>
                              <p class="truncate text-[10px] text-gray-400">{entry.displayPath}</p>
                            </div>
                          </button>
                        {/each}
                      </div>
                    {:else if !fileMentionBusy}
                      <div class="px-2.5 py-2 text-[11px] font-medium text-gray-400">{m.file_mentions_empty()}</div>
                    {/if}
                  </div>
                {/if}
                <form bind:this={composerPanelElement} class="composer-panel bg-white/95 border-2 border-gray-200 rounded-2xl shadow-2xl overflow-hidden transition-all duration-200 focus-within:-translate-y-0.5 focus-within:border-amber-400/70 focus-within:bg-white focus-within:shadow-[0_24px_60px_-34px_rgba(245,158,11,0.65)]" onsubmit={(event) => { event.preventDefault(); void submitComposer(); }}>
                  <textarea bind:this={composerTextareaElement} bind:value={draft} class="composer-textarea w-full min-h-[3rem] overflow-y-hidden border-none bg-transparent px-4 py-3 pr-12 text-sm leading-6 text-gray-800 placeholder-gray-400 outline-none transition-colors duration-150 focus:outline-none focus:ring-0 focus:placeholder:text-amber-500/70 resize-none sm:min-h-[3.25rem]" oninput={handleComposerInput} onkeydown={handleComposerKeydown} placeholder={composerQueueModeActive ? ui.queueFollowUpPlaceholder : ui.askCodex} readonly={readOnlyRole} rows="1"></textarea>
                  
                  {#if draftAttachments.length > 0}
                    <div class="flex flex-wrap gap-1.5 px-3.5 pb-1.5">
                      {#each draftAttachments as attachment (attachment.id)}<button class="flex items-center gap-1.5 rounded-lg border border-gray-200 bg-gray-50 px-2.5 py-1 text-[10px] font-bold text-gray-600 transition-all hover:border-red-200 hover:bg-red-50 hover:text-red-600 group disabled:cursor-not-allowed disabled:opacity-60" disabled={readOnlyRole} onclick={() => void removeDraftAttachment(attachment.id)} type="button"><FileText size={11} /><span>{attachment.originalName}</span><X size={11} class="opacity-0 transition-opacity group-hover:opacity-100" /></button>{/each}
                    </div>
                  {/if}

                  <div bind:this={composerToolbarElement} class={`composer-toolbar flex items-center gap-1.5 border-t border-gray-100 bg-gray-50/80 px-2.5 py-2 transition-colors duration-200 group-focus-within:border-amber-100 group-focus-within:bg-[linear-gradient(180deg,rgba(255,251,235,0.9),rgba(255,255,255,0.98))] sm:px-3 sm:py-2.5 ${composerToolbarCompact ? "flex-wrap" : "flex-nowrap"}`}>
                    <div class={`flex min-w-0 items-center gap-1.25 sm:gap-1.5 ${composerToolbarCompact ? "basis-full flex-1" : "flex-1"}`}>
                      <input bind:this={filePickerElement} disabled={readOnlyRole || uploading} hidden multiple onchange={(event) => void uploadFiles((event.currentTarget as HTMLInputElement).files)} type="file" />
                      <button class="ui-animated-button ui-animated-button--icon inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-lg p-0 text-gray-400 transition-all hover:bg-amber-50 hover:text-amber-600 group-focus-within:bg-white/90 group-focus-within:text-amber-700 disabled:cursor-not-allowed disabled:opacity-50 sm:h-8 sm:w-8" disabled={readOnlyRole || uploading} onclick={promptAttachmentPicker} title={ui.addAttachments} type="button">{#if uploading}<RefreshCw size={15} class="animate-spin" />{:else}<Paperclip size={15} />{/if}</button>
                      {#if conversation}
                        <div class="mx-0.5 hidden h-4 w-px bg-gray-200 sm:block"></div>
                        <button
                          bind:this={composerSettingsTriggerElement}
                          class={`composer-compact-trigger ui-animated-button ui-animated-button--soft flex h-7 min-w-0 max-w-[10rem] items-center gap-1.25 rounded-lg border px-2 text-[10px] font-bold transition-all group-focus-within:border-amber-100 group-focus-within:bg-white/90 group-focus-within:text-gray-700 sm:h-8 sm:max-w-[12.5rem] sm:gap-1.5 sm:px-2.5 sm:text-[11px] ${
                            composerSettingsOpen && composerSettingsAnchor === "session"
                              ? "border-amber-200 bg-white text-gray-900 shadow-sm"
                              : "border-transparent text-gray-500 hover:border-gray-200 hover:bg-white hover:text-gray-900"
                          }`}
                          onclick={() => {
                            if (composerSettingsOpen && composerSettingsAnchor === "session" && composerSettingsTab === "session") {
                              composerSettingsOpen = false;
                              return;
                            }

                            composerSettingsAnchor = "session";
                            composerSettingsTab = "session";
                            composerSettingsOpen = true;
                          }}
                          type="button"
                        >
                          {#if isFastSpeedMode(composerSettingsSummary.speed)}
                            <span class="inline-flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-amber-100 text-amber-700">
                              <Zap size={12} />
                            </span>
                          {:else}
                            <Cpu size={13} class="shrink-0 text-gray-400" />
                          {/if}
                          <span class="truncate">{composerSettingsSummary.model}</span>
                          {#if isPlanModeEnabled(conversation.preferences)}
                            <span class="inline-flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-amber-100 text-amber-700" title={m.plan_mode_enabled()}>
                              <ListTodo size={11} />
                            </span>
                          {/if}
                          {#if composerSettingsSummary.speed === "flex"}
                            <span class="hidden rounded-full bg-sky-50 px-1.5 py-0.5 text-[9px] font-bold uppercase tracking-widest text-sky-700 sm:inline-flex">
                              {ui.speedFlex}
                            </span>
                          {/if}
                          <ChevronDown size={14} class={`shrink-0 transition-transform ${composerSettingsOpen && composerSettingsAnchor === "session" ? "rotate-180" : ""}`} />
                        </button>
                        <button
                          bind:this={composerSecurityTriggerElement}
                          class={`composer-compact-trigger ui-animated-button ui-animated-button--soft flex h-7 shrink-0 items-center gap-1.25 rounded-lg border px-2 text-[10px] font-bold transition-all group-focus-within:border-amber-100 group-focus-within:bg-white/90 group-focus-within:text-gray-700 sm:h-8 sm:gap-1.5 sm:px-2.5 sm:text-[11px] ${
                            composerSettingsOpen && composerSettingsTab === "security"
                              ? "border-sky-200 bg-white text-sky-800 shadow-sm"
                              : "border-transparent text-gray-500 hover:border-gray-200 hover:bg-white hover:text-gray-900"
                          }`}
                          onclick={() => {
                            if (composerSettingsOpen && composerSettingsAnchor === "security" && composerSettingsTab === "security") {
                              composerSettingsOpen = false;
                              return;
                            }

                            composerSettingsAnchor = "security";
                            composerSettingsTab = "security";
                            composerSettingsOpen = true;
                          }}
                          title={ui.securitySession}
                          type="button"
                        >
                          <Shield size={14} class={composerSettingsOpen && composerSettingsTab === "security" ? "text-sky-600" : "text-gray-400"} />
                          <span class="hidden lg:inline">{ui.securitySession}</span>
                        </button>
                      {/if}
                    </div>
                    <div class={`flex items-center justify-end gap-1.25 sm:gap-1.5 ${composerToolbarCompact ? "basis-full" : "shrink-0"}`}>
                      {#if running}
                        <button class="ui-animated-button ui-animated-button--soft inline-flex h-7 items-center justify-center rounded-lg px-2.5 text-[10px] font-bold text-red-600 transition-all hover:bg-red-50 disabled:cursor-not-allowed disabled:opacity-50 sm:h-8 sm:px-3 sm:text-[11px]" disabled={readOnlyRole} onclick={interruptTurn} type="button">{ui.stop}</button>
                        <button class="ui-animated-button ui-animated-button--soft inline-flex h-7 items-center justify-center rounded-lg border border-amber-200 bg-amber-50 px-2.5 text-[10px] font-bold text-amber-700 transition-all hover:bg-amber-100 disabled:opacity-50 sm:h-8 sm:px-3 sm:text-[11px]" disabled={readOnlyRole || sending || (!draft.trim() && draftAttachments.length === 0)} onclick={steerTurn} type="button">{ui.steer}</button>
                      {/if}
                      <button class="surface-contrast-button ui-animated-button ui-animated-button--strong inline-flex h-7 items-center justify-center rounded-lg bg-gray-900 px-3 text-[10px] font-bold text-white shadow-lg shadow-gray-200 transition-all hover:bg-gray-800 disabled:opacity-50 disabled:shadow-none active:scale-[0.98] sm:h-8 sm:px-4 sm:text-[11px]" disabled={composerPrimaryActionDisabled} onclick={() => void submitComposer()} type="button"><div class="flex items-center gap-1.25 sm:gap-1.5"><span>{composerQueueModeActive ? ui.queue : ui.send}</span><Send size={13} /></div></button>
                    </div>
                  </div>
                </form>

                <!-- Settings Popovers -->
                {#if composerSettingsOpen && conversation}
                  <div
                    bind:this={composerSettingsPopoverElement}
                    use:portal
                    class="floating-popover composer-popover composer-settings-popover w-[19rem] max-w-[calc(100vw-1rem)] overflow-y-auto overscroll-contain rounded-xl border p-2.5 sm:rounded-2xl sm:p-3.5"
                    data-positioned={composerSettingsPopoverStyle.includes("top:")}
                    style={composerSettingsPopoverStyle || "opacity:0;pointer-events:none;"}
                  >
                    <div class="composer-settings-popover__header mb-2 flex items-center justify-between border-b border-gray-100 pb-2 sm:mb-2.5 sm:pb-2.5">
                      <div>
                        <h3 class="text-xs font-bold uppercase tracking-widest text-gray-400">{ui.settings}</h3>
                        <p class="mt-1 text-[11px] font-medium text-gray-500">
                          {composerSettingsTab === "session"
                            ? ui.composerSettings
                            : composerSettingsTab === "skills"
                              ? ui.settingsSkills
                              : ui.securitySession}
                        </p>
                      </div>
                      <button class="composer-settings-popover__close text-gray-400 hover:text-gray-600" onclick={() => (composerSettingsOpen = false)} type="button">
                        <X size={16} />
                      </button>
                    </div>
                    <div aria-label={ui.settings} class="composer-settings-tabs mb-2 grid grid-cols-3 gap-1 rounded-xl border border-gray-200 bg-gray-50 p-1 sm:mb-3 sm:rounded-2xl" role="tablist">
                      <button
                        aria-selected={composerSettingsTab === "session"}
                        class={`composer-settings-tab ui-animated-button ui-animated-button--soft flex items-center justify-center rounded-xl px-3 py-1.5 text-[10px] font-bold transition-all sm:text-[11px] ${
                          composerSettingsTab === "session"
                            ? "border border-amber-200 bg-white text-amber-700 shadow-sm"
                            : "border border-transparent text-gray-500 hover:bg-white hover:text-gray-700"
                        }`}
                        onclick={() => {
                          composerSettingsAnchor = "session";
                          composerSettingsTab = "session";
                        }}
                        role="tab"
                        type="button"
                      >
                        {ui.composerSettings}
                      </button>
                      <button
                        aria-selected={composerSettingsTab === "skills"}
                        class={`composer-settings-tab ui-animated-button ui-animated-button--soft flex items-center justify-center rounded-xl px-3 py-1.5 text-[10px] font-bold transition-all sm:text-[11px] ${
                          composerSettingsTab === "skills"
                            ? "border border-violet-200 bg-white text-violet-700 shadow-sm"
                            : "border border-transparent text-gray-500 hover:bg-white hover:text-gray-700"
                        }`}
                        onclick={() => {
                          composerSettingsAnchor = "session";
                          composerSettingsTab = "skills";
                        }}
                        role="tab"
                        type="button"
                      >
                        {ui.settingsSkills}
                      </button>
                      <button
                        aria-selected={composerSettingsTab === "security"}
                        class={`composer-settings-tab ui-animated-button ui-animated-button--soft flex items-center justify-center rounded-xl px-3 py-1.5 text-[10px] font-bold transition-all sm:text-[11px] ${
                          composerSettingsTab === "security"
                            ? "border border-sky-200 bg-white text-sky-700 shadow-sm"
                            : "border border-transparent text-gray-500 hover:bg-white hover:text-gray-700"
                        }`}
                        onclick={() => {
                          composerSettingsAnchor = "security";
                          composerSettingsTab = "security";
                        }}
                        role="tab"
                        type="button"
                      >
                        {ui.securitySession}
                      </button>
                    </div>
                    {#if composerSettingsTab === "session"}
                      <div class="grid grid-cols-1 gap-3" role="tabpanel">
                        <div class="space-y-1">
                          <label class="px-1 text-[10px] font-bold uppercase tracking-widest text-gray-400" for="composer-model-select">{ui.model}</label>
                          <select class="w-full rounded-xl border border-gray-200 bg-gray-50 px-3 py-2 text-sm transition-all focus:border-amber-500 focus:outline-none focus:ring-2 focus:ring-amber-500/10 disabled:cursor-not-allowed disabled:opacity-60" disabled={readOnlyRole} id="composer-model-select" onchange={(event) => setPreference("model", (event.currentTarget as HTMLSelectElement).value || null)} value={conversation.preferences.model ?? ""}>
                            <option value="">{ui.autoDefault}</option>
                            {#each config?.models ?? [] as model (model.id)}
                              <option value={model.id}>{model.displayName}</option>
                            {/each}
                          </select>
                        </div>
                        <div class="space-y-1">
                          <div class="flex items-center justify-between gap-2 px-1">
                            <span class="text-[10px] font-bold uppercase tracking-widest text-gray-400">model_context_window</span>
                            <span class="text-[10px] font-semibold text-gray-400">{isHundredMContextEnabled(conversation.preferences) ? "100M" : ui.speedAuto}</span>
                          </div>
                          <div class="composer-settings-segmented grid w-full grid-cols-2 gap-1 rounded-xl border border-gray-200 bg-white p-1 shadow-sm">
                            <button
                              class={`composer-settings-segmented__button ui-animated-button ui-animated-button--soft flex h-8 w-full items-center justify-center gap-1 rounded-lg border px-2 text-[10px] font-bold transition-all sm:text-[11px] ${
                                !isHundredMContextEnabled(conversation.preferences)
                                  ? "border-gray-900 bg-gray-900 text-white shadow-sm"
                                  : "border-transparent text-gray-500 hover:border-gray-200 hover:bg-gray-50 hover:text-gray-700"
                              }`}
                              data-selected={!isHundredMContextEnabled(conversation.preferences)}
                              data-tone="auto"
                              disabled={readOnlyRole}
                              onclick={() => setHundredMContextEnabled(false)}
                              type="button"
                            >
                              <span class="truncate">{ui.speedAuto}</span>
                            </button>
                            <button
                              class={`composer-settings-segmented__button ui-animated-button ui-animated-button--soft flex h-8 w-full items-center justify-center gap-1 rounded-lg border px-2 text-[10px] font-bold transition-all sm:text-[11px] ${
                                isHundredMContextEnabled(conversation.preferences)
                                  ? "border-amber-200 bg-amber-50 text-amber-700 shadow-sm"
                                  : "border-transparent text-gray-500 hover:border-gray-200 hover:bg-gray-50 hover:text-gray-700"
                              }`}
                              data-selected={isHundredMContextEnabled(conversation.preferences)}
                              data-tone="context"
                              disabled={readOnlyRole}
                              onclick={() => setHundredMContextEnabled(!isHundredMContextEnabled(conversation?.preferences))}
                              title="100,000,000"
                              type="button"
                            >
                              <span class="truncate">100M</span>
                            </button>
                          </div>
                        </div>
                        <div class="space-y-1">
                          <label class="px-1 text-[10px] font-bold uppercase tracking-widest text-gray-400" for="composer-effort-select">{m.reasoning()}</label>
                          <select class="w-full rounded-xl border border-gray-200 bg-gray-50 px-3 py-2 text-sm transition-all focus:border-amber-500 focus:outline-none focus:ring-2 focus:ring-amber-500/10 disabled:cursor-not-allowed disabled:opacity-60" disabled={readOnlyRole} id="composer-effort-select" onchange={(event) => setPreference("effort", (event.currentTarget as HTMLSelectElement).value as SessionPreferences["effort"])} value={conversation.preferences.effort ?? (reasoningOptions[0] ?? "medium")}>
                            {#each reasoningOptions as option (option)}
                              <option value={option}>{option}</option>
                            {/each}
                          </select>
                        </div>
                        {#if selectedModel?.supportsPersonality ?? true}
                          <div class="space-y-1">
                            <label class="px-1 text-[10px] font-bold uppercase tracking-widest text-gray-400" for="composer-personality-select">{ui.personality}</label>
                            <select
                              class="w-full rounded-xl border border-gray-200 bg-gray-50 px-3 py-2 text-sm transition-all focus:border-amber-500 focus:outline-none focus:ring-2 focus:ring-amber-500/10 disabled:cursor-not-allowed disabled:opacity-60"
                              disabled={readOnlyRole}
                              id="composer-personality-select"
                              onchange={(event) =>
                                setPreference("personality", (event.currentTarget as HTMLSelectElement).value as SessionPreferences["personality"])}
                              value={conversation.preferences.personality ?? "pragmatic"}
                            >
                              {#each personalityOptions as option (option)}
                                <option value={option}>{getPersonalityOptionLabel(option)}</option>
                              {/each}
                            </select>
                          </div>
                        {/if}
                        <div class="composer-settings-card flex flex-col gap-2.5 rounded-xl border border-gray-200/80 bg-gray-50/80 p-2.5">
                          <div class="flex min-w-0 items-start gap-2">
                            <span class={`composer-settings-card__icon inline-flex h-6 w-6 shrink-0 items-center justify-center rounded-lg border ${
                              conversation.preferences.sendOnEnter
                                ? "border-amber-200 bg-amber-100 text-amber-700"
                                : "border-gray-200 bg-white text-gray-500"
                            }`} data-active={conversation.preferences.sendOnEnter}>
                              <Keyboard size={12} />
                            </span>
                            <div class="min-w-0 space-y-0.5">
                              <p class="composer-settings-card__eyebrow text-[10px] font-bold uppercase tracking-widest text-gray-400">{ui.sendShortcut}</p>
                              <p class="composer-settings-card__value text-[11px] text-gray-500">{conversation.preferences.sendOnEnter ? ui.sendShortcutEnter : ui.sendShortcutCtrlEnter}</p>
                              <p class="text-[10px] leading-4 text-gray-400">{ui.sendShortcutDescription}</p>
                            </div>
                          </div>
                          <div class="grid w-full grid-cols-1 gap-1.5 sm:grid-cols-2 sm:gap-2">
                            <button
                              class={`ui-animated-button ui-animated-button--soft flex min-h-[2.25rem] w-full items-center justify-between gap-2 rounded-xl border px-2.5 py-1.5 text-left text-[11px] font-bold transition-all sm:min-h-[2.75rem] sm:gap-3 sm:px-3 sm:py-2 ${
                                !conversation.preferences.sendOnEnter
                                  ? "border-gray-900 bg-gray-900 text-white shadow-sm"
                                  : "border-transparent text-gray-500 hover:border-gray-200 hover:bg-gray-50 hover:text-gray-700"
                              }`}
                              disabled={readOnlyRole}
                              onclick={() => setPreference("sendOnEnter", false)}
                              type="button"
                            >
                              <span class="truncate">{ui.sendShortcutCtrlEnter}</span>
                              <span class={`shrink-0 rounded-full px-2 py-0.5 text-[10px] font-bold ${
                                !conversation.preferences.sendOnEnter
                                  ? "bg-white/15 text-white"
                                  : "bg-gray-100 text-gray-500"
                              }`}>{ui.send}</span>
                            </button>
                            <button
                              class={`ui-animated-button ui-animated-button--soft flex min-h-[2.25rem] w-full items-center justify-between gap-2 rounded-xl border px-2.5 py-1.5 text-left text-[11px] font-bold transition-all sm:min-h-[2.75rem] sm:gap-3 sm:px-3 sm:py-2 ${
                                conversation.preferences.sendOnEnter
                                  ? "border-amber-200 bg-amber-50 text-amber-700 shadow-sm"
                                  : "border-transparent text-gray-500 hover:border-gray-200 hover:bg-gray-50 hover:text-gray-700"
                              }`}
                              disabled={readOnlyRole}
                              onclick={() => setPreference("sendOnEnter", true)}
                              type="button"
                            >
                              <span class="truncate">{ui.sendShortcutEnter}</span>
                              <span class={`shrink-0 rounded-full px-2 py-0.5 text-[10px] font-bold ${
                                conversation.preferences.sendOnEnter
                                  ? "bg-amber-100 text-amber-700"
                                  : "bg-gray-100 text-gray-500"
                              }`}>{ui.send}</span>
                            </button>
                          </div>
                        </div>
                        <label
                          class:checkbox-card--disabled={readOnlyRole}
                          class="checkbox-card checkbox-card--compact w-full"
                          title={isPlanModeEnabled(conversation.preferences) ? m.plan_mode_enabled() : m.slash_plan_disabled()}
                        >
                          <input
                            checked={isPlanModeEnabled(conversation.preferences)}
                            class="checkbox-input"
                            disabled={readOnlyRole}
                            onchange={(event) => setPlanModePreference((event.currentTarget as HTMLInputElement).checked)}
                            type="checkbox"
                          />
                          <span class="checkbox-control"></span>
                          <span class="checkbox-copy min-w-0">
                            <span class="checkbox-title inline-flex items-center gap-1.5">
                              <ListTodo size={12} class={isPlanModeEnabled(conversation.preferences) ? "text-amber-700" : "text-gray-400"} />
                              <span>{ui.planMode}</span>
                            </span>
                            <span class="checkbox-description">
                              {isPlanModeEnabled(conversation.preferences) ? m.plan_mode_enabled() : ui.autoDefault}
                            </span>
                          </span>
                        </label>
                        <div class="composer-settings-card flex flex-col gap-2 rounded-xl border border-gray-200/80 bg-gray-50/80 p-2.5">
                          <label class:checkbox-card--disabled={readOnlyRole} class="checkbox-card checkbox-card--compact w-full">
                            <input
                              checked={conversation.preferences.languageBridgeEnabled ?? false}
                              class="checkbox-input"
                              disabled={readOnlyRole}
                              onchange={(event) => setPreference("languageBridgeEnabled", (event.currentTarget as HTMLInputElement).checked)}
                              type="checkbox"
                            />
                            <span class="checkbox-control"></span>
                            <span class="checkbox-copy min-w-0">
                              <span class="checkbox-title inline-flex items-center gap-1.5">
                                <ArrowRightLeft size={12} class={(conversation.preferences.languageBridgeEnabled ?? false) ? "text-amber-700" : "text-gray-400"} />
                                <span>{ui.languageBridge}</span>
                              </span>
                              <span class="checkbox-description">{ui.languageBridgeDescription}</span>
                            </span>
                          </label>
                          {#if conversation.preferences.languageBridgeEnabled}
                            <label class="space-y-1 px-1">
                              <span class="text-[10px] font-bold uppercase tracking-widest text-gray-400">{ui.languageBridgeOutput}</span>
                              <input
                                class="w-full rounded-xl border border-gray-200 bg-white px-3 py-2 text-sm text-gray-800 transition-all focus:border-amber-500 focus:outline-none focus:ring-2 focus:ring-amber-500/10 disabled:cursor-not-allowed disabled:opacity-60"
                                disabled={readOnlyRole}
                                onblur={(event) => {
                                  const value = (event.currentTarget as HTMLInputElement).value.trim() || "auto";
                                  setPreference("languageBridgeOutputLanguage", value);
                                }}
                                onkeydown={(event) => {
                                  if (event.key === "Enter") {
                                    event.preventDefault();
                                    (event.currentTarget as HTMLInputElement).blur();
                                  }
                                }}
                                placeholder={ui.languageBridgeAuto}
                                type="text"
                                value={conversation.preferences.languageBridgeOutputLanguage ?? "auto"}
                              />
                            </label>
                          {/if}
                        </div>
                        <div class="composer-settings-card flex flex-col gap-2.5 rounded-xl border border-gray-200/80 bg-gray-50/80 p-2.5">
                          <div class="flex min-w-0 items-start gap-2">
                            <span class={`composer-settings-card__icon inline-flex h-6 w-6 shrink-0 items-center justify-center rounded-lg border ${
                              isFastSpeedMode(conversation.preferences.speed)
                                ? "border-amber-200 bg-amber-100 text-amber-700"
                                : "border-gray-200 bg-white text-gray-500"
                            }`} data-active={isFastSpeedMode(conversation.preferences.speed)}>
                              {#if isFastSpeedMode(conversation.preferences.speed)}
                                <Zap size={12} />
                              {:else}
                                <Cpu size={12} />
                              {/if}
                            </span>
                            <div class="min-w-0">
                              <p class="composer-settings-card__eyebrow text-[10px] font-bold uppercase tracking-widest text-gray-400">{ui.speed}</p>
                              <p class="composer-settings-card__value text-[11px] text-gray-500">{getSpeedOptionLabel(conversation.preferences.speed ?? "auto")}</p>
                            </div>
                          </div>
                          <label class:checkbox-card--disabled={readOnlyRole || !speedOptions.includes("fast")} class="checkbox-card checkbox-card--compact w-full">
                            <input
                              checked={isFastSpeedMode(conversation.preferences.speed)}
                              class="checkbox-input"
                              disabled={readOnlyRole || !speedOptions.includes("fast")}
                              onchange={(event) => {
                                const target = event.currentTarget as HTMLInputElement;
                                setSpeedPreference(target.checked ? "fast" : "auto");
                              }}
                              type="checkbox"
                            />
                            <span class="checkbox-control"></span>
                            <span class="checkbox-copy min-w-0">
                              <span class="checkbox-title">{ui.speedFast}</span>
                              <span class="checkbox-description">
                                {isFastSpeedMode(conversation.preferences.speed)
                                  ? ui.speedFast
                                  : ui.speedAuto}
                              </span>
                            </span>
                          </label>
                          {#if speedOptions.includes("flex") && !isFastSpeedMode(conversation.preferences.speed)}
                            <div class="composer-settings-segmented grid w-full grid-cols-2 gap-1 rounded-xl border border-gray-200 bg-white p-1 shadow-sm">
                              {#each ["auto", "flex"] as option (option)}
                                <button
                                  class={`composer-settings-segmented__button ui-animated-button ui-animated-button--soft flex h-8 w-full items-center justify-center gap-1 rounded-lg border px-2 text-[10px] font-bold transition-all sm:text-[11px] ${
                                    (conversation.preferences.speed ?? "auto") === option
                                      ? option === "flex"
                                        ? "border-sky-200 bg-sky-50 text-sky-700 shadow-sm"
                                        : "border-gray-900 bg-gray-900 text-white shadow-sm"
                                      : "border-transparent text-gray-500 hover:border-gray-200 hover:bg-gray-50 hover:text-gray-700"
                                  }`}
                                  data-selected={(conversation.preferences.speed ?? "auto") === option}
                                  data-tone={option}
                                  disabled={readOnlyRole}
                                  onclick={() => setSpeedPreference(option as SessionPreferences["speed"])}
                                  type="button"
                                >
                                  {#if option === "flex"}
                                    <Cpu size={12} class="shrink-0 opacity-80" />
                                  {/if}
                                  <span class="truncate">{getSpeedOptionLabel(option as SessionPreferences["speed"])}</span>
                                </button>
                              {/each}
                            </div>
                          {/if}
                        </div>
                      </div>
                    {:else if composerSettingsTab === "skills"}
                      <div class="space-y-3" role="tabpanel">
                        <label class="space-y-1">
                          <span class="px-1 text-[10px] font-bold uppercase tracking-widest text-gray-400">{ui.installedSkills}</span>
                          <div class="search-popover__header flex items-center gap-2 rounded-xl border border-gray-200 bg-gray-50 px-3 py-2">
                            <Search size={14} class="text-gray-400" />
                            <input
                              bind:value={composerSkillQuery}
                              class="w-full border-none bg-transparent p-0 text-sm text-gray-800 placeholder-gray-400 focus:outline-none focus:ring-0"
                              placeholder={m.search()}
                              type="search"
                            />
                          </div>
                        </label>
                        {#if composerSelectedSkills.length > 0}
                          <div class="flex flex-wrap gap-1.5">
                            {#each composerSelectedSkills as skill (skill.path)}
                              <button
                                class="inline-flex items-center gap-1.5 rounded-full border border-violet-200 bg-violet-50 px-2.5 py-1 text-[10px] font-bold text-violet-700 transition-colors hover:bg-violet-100"
                                onclick={() => setSelectedSkills(composerSelectedSkills.filter((entry) => entry.path !== skill.path))}
                                type="button"
                              >
                                <span class="max-w-[12rem] truncate">{skill.name}</span>
                                <X size={11} />
                              </button>
                            {/each}
                          </div>
                        {/if}
                        {#if catalogLoading}
                          <div class="flex items-center justify-center gap-2 rounded-xl border border-gray-200 bg-gray-50 px-4 py-6 text-xs text-gray-500">
                            <RefreshCw size={14} class="animate-spin" />
                            <span>{m.loading()}</span>
                          </div>
                        {:else if filteredComposerSkills.length === 0}
                          <div class="rounded-xl border border-dashed border-gray-200 bg-gray-50 px-4 py-6 text-center text-xs text-gray-500">
                            {ui.noSkills}
                          </div>
                        {:else}
                          <div class="max-h-80 space-y-1.5 overflow-y-auto pr-1">
                            {#each filteredComposerSkills as skill (skill.path)}
                              {@const selected = composerSelectedSkills.some((entry) => entry.path === skill.path)}
                              <button
                                class={`w-full rounded-xl border px-3 py-2.5 text-left transition-colors ${
                                  selected
                                    ? "border-violet-200 bg-violet-50/70"
                                    : "border-gray-200 bg-white hover:border-gray-300 hover:bg-gray-50"
                                }`}
                                onclick={() => toggleComposerSkill(skill)}
                                type="button"
                              >
                                <div class="flex items-start justify-between gap-3">
                                  <div class="min-w-0">
                                    <p class="truncate text-sm font-bold text-gray-900">{skill.name}</p>
                                    <p class="mt-0.5 line-clamp-2 text-[11px] leading-relaxed text-gray-500">
                                      {skill.description || skill.path}
                                    </p>
                                    <div class="mt-1 flex flex-wrap items-center gap-1.5 text-[10px] text-gray-400">
                                      <span class="rounded-full bg-gray-100 px-2 py-0.5 font-bold uppercase tracking-widest">
                                        {skill.source}
                                      </span>
                                      {#if skill.pluginName}
                                        <span class="truncate">{skill.pluginName}</span>
                                      {/if}
                                    </div>
                                  </div>
                                  <span
                                    class={`mt-0.5 inline-flex h-5 min-w-[2.75rem] items-center justify-center rounded-full px-2 text-[10px] font-bold uppercase tracking-widest ${
                                      selected ? "bg-violet-600 text-white" : "bg-gray-100 text-gray-400"
                                    }`}
                                  >
                                    {selected ? "ON" : "OFF"}
                                  </span>
                                </div>
                              </button>
                            {/each}
                          </div>
                        {/if}
                      </div>
                    {:else}
                      <div class="space-y-4" role="tabpanel">
                        <div class="space-y-1">
                          <label class="px-1 text-[10px] font-bold uppercase tracking-widest text-gray-400" for="composer-approval-select">{ui.approvalMode}</label>
                          <select class="w-full rounded-xl border border-gray-200 bg-gray-50 px-3 py-2 text-sm transition-all focus:border-sky-500 focus:outline-none focus:ring-2 focus:ring-sky-500/10 disabled:cursor-not-allowed disabled:opacity-60" disabled={readOnlyRole} id="composer-approval-select" onchange={(event) => setPreference("autoApproveMode", (event.currentTarget as HTMLSelectElement).value as SessionPreferences["autoApproveMode"])} value={conversation.preferences.autoApproveMode ?? "manual"}>
                            <option value="manual">{ui.manual}</option>
                            <option value="turn">{ui.autoOnce}</option>
                            <option value="session">{ui.autoSession}</option>
                          </select>
                        </div>
                        <label class="checkbox-card" for="network-access">
                          <input
                            class="checkbox-input"
                            checked={conversation.preferences.networkAccess ?? false}
                            disabled={readOnlyRole}
                            onchange={(event) => setPreference("networkAccess", (event.currentTarget as HTMLInputElement).checked)}
                            type="checkbox"
                            id="network-access"
                          />
                          <span aria-hidden="true" class="checkbox-control"></span>
                          <span class="checkbox-copy">
                            <span class="checkbox-title">{ui.allowNetworkAccess}</span>
                          </span>
                        </label>
                      </div>
                    {/if}
                  </div>
                {/if}
              </div>
            </div>
          </div>
        </div>
      {:else if activeWorkspaceTabId === "tasks"}
        <div class="h-full overflow-y-auto bg-gray-50/30 p-8">
          <div class="max-w-4xl mx-auto space-y-8">
            <div class="flex items-end justify-between border-b border-gray-200 pb-6"><div><h2 class="text-2xl font-bold text-gray-900">{ui.taskCenter}</h2><p class="text-sm text-gray-500 mt-1">{ui.subagentActivities}</p></div><div class="px-3 py-1 bg-white border border-gray-200 rounded-lg text-xs font-bold text-gray-500 uppercase tracking-widest shadow-sm">{subagentTasks.length} {ui.tasks}</div></div>
            {#if ArenaWorkspaceView}
              <ArenaWorkspaceView
                currentPreferences={conversation?.preferences ?? config?.defaults ?? null}
                models={config?.models ?? []}
                readOnly={readOnlyRole}
                onOpenSession={async (sessionId, profileId = null) => {
                  activeWorkspaceTabId = "chat";
                  await selectSession(sessionId, profileId);
                }}
                onUseResponse={async (contestant) => {
                  activeWorkspaceTabId = "chat";
                  draft = contestant.response ?? "";
                  await tick();
                  composerTextareaElement?.focus();
                }}
              />
            {:else}
              <div class="workspace-loading-card">
                <RefreshCw size={16} class="animate-spin text-gray-300" />
                <span>{getWorkspaceLoadingLabel()}</span>
              </div>
            {/if}
            <section class="space-y-4">
              <div class="flex items-center justify-between">
                <h3 class="text-sm font-bold uppercase tracking-[0.22em] text-gray-500">{ui.subagentActivities}</h3>
                <span class="rounded-full border border-gray-200 bg-white px-3 py-1 text-[11px] font-bold uppercase tracking-[0.18em] text-gray-400 shadow-sm">{subagentTasks.length}</span>
              </div>
              {#if subagentTasks.length === 0}<div class="py-16 text-center"><div class="w-14 h-14 bg-gray-100 rounded-3xl flex items-center justify-center mx-auto mb-4 text-gray-300"><History size={28} /></div><p class="text-gray-500">{ui.noActiveTasks}</p></div>
              {:else}<div class="grid grid-cols-1 gap-4">{#each subagentTasks as task (task.key)}<div class="bg-white border border-gray-200 rounded-2xl p-5 shadow-sm hover:shadow-md transition-all group flex items-start justify-between gap-4"><div class="flex items-center gap-4"><div class="w-10 h-10 bg-amber-50 text-amber-600 rounded-xl flex items-center justify-center shadow-inner"><Bot size={20} /></div><div><h4 class="font-bold text-gray-900">{task.tool}</h4><p class="text-[10px] font-bold text-amber-600 uppercase tracking-widest mt-0.5">{task.status}</p></div></div>{#if task.primaryThreadId}<button class="px-4 py-2 bg-white border border-gray-200 rounded-xl text-xs font-bold text-gray-700 hover:bg-gray-50 transition-all shadow-sm" onclick={() => void openSubagentThread(task.primaryThreadId ?? "")}>{ui.openThread}</button>{/if}</div>{/each}</div>{/if}
            </section>
          </div>
        </div>
      {:else if activeWorkspaceTabId === "settings"}
        <div class="h-full overflow-y-auto bg-gray-50/30 p-5 sm:p-8">
          <div class="mx-auto w-full max-w-7xl">
            {#if SettingsWorkspaceView}
              <SettingsWorkspaceView
                codexHome={config?.paths.codexHome ?? ""}
                configFilePath={config?.paths.configFilePath ?? ""}
                autostart={config?.autostart ?? null}
                runtime={runtime}
                defaults={config?.defaults ?? null}
                notificationSettings={config?.notifications.settings ?? null}
                promptPresets={config?.promptPresets ?? []}
                automations={config?.automations.items ?? []}
                automationRuns={config?.automations.recentRuns ?? []}
                themeSettings={(config as ThemedConfigPayload | null)?.theme ?? null}
                themeMode={themeMode}
                resolvedTheme={resolvedTheme}
                initialTab={settingsInitialTab}
                webRole={webRole}
                readOnly={readOnlyRole}
                onConfigSaved={async () => {
                  config = applyLocalComposerPreferencesToConfig(await api.getConfig());
                  syncConfiguredTheme(config);
                }}
                onAutostartSaved={async (enabled) => {
                  await saveAutostartEnabled(enabled);
                }}
                onSaveDefaultLanguageBridge={async (enabled, outputLanguage) => {
                  await saveDefaultLanguageBridgeDefaults(enabled, outputLanguage);
                }}
                onSaveThemeSettings={async (theme) => {
                  await saveThemeSettings(theme);
                }}
                onNotificationSettingsSaved={async (settings) => {
                  await saveNotificationSettings(settings);
                }}
                onSavePromptPreset={async (preset) => {
                  await savePromptPreset(preset);
                }}
                onDeletePromptPreset={async (presetId) => {
                  await deletePromptPreset(presetId);
                }}
                onSaveAutomation={async (automation) => {
                  await saveAutomation(automation);
                }}
                onDeleteAutomation={async (automationId) => {
                  await deleteAutomation(automationId);
                }}
                onRunAutomation={async (automationId) => {
                  await runAutomation(automationId);
                }}
                onCleanupAutomationWorktrees={async () => {
                  await cleanupAutomationWorktrees();
                }}
                onOpenSession={async (sessionId, profileId = null) => {
                  activeWorkspaceTabId = "chat";
                  await selectSession(sessionId, profileId);
                }}
              />
            {:else if lazyWorkspaceLoadErrors.settings}
              <div class="workspace-loading-card">
                <AlertCircle size={16} class="text-rose-400" />
                <span>{lazyWorkspaceLoadErrors.settings}</span>
                <button
                  class="rounded-lg border px-2 py-1 text-xs font-bold"
                  style="border-color: var(--line); color: var(--ink);"
                  type="button"
                  onclick={() => void ensureLazyWorkspaceLoaded("settings")}
                >
                  {getLocale().startsWith("ko") ? "다시 시도" : "Retry"}
                </button>
              </div>
            {:else}
              <div class="workspace-loading-card">
                <RefreshCw size={16} class="animate-spin text-gray-300" />
                <span>{getWorkspaceLoadingLabel()}</span>
              </div>
            {/if}
          </div>
        </div>
      {:else if activeWorkspaceTabId === "computer"}
        <div class="flex h-full min-h-0 flex-col gap-4 overflow-hidden p-4 sm:p-6" data-testid="computer-workspace" style="background: var(--bg); color: var(--ink);">
          <div class="flex shrink-0 items-center justify-between gap-3 rounded-2xl border px-4 py-3" style="border-color: var(--line); background: var(--panel-strong);">
            <div class="min-w-0">
              <p class="text-[10px] font-bold uppercase tracking-[0.2em]" style="color: var(--muted);">{ui.computerSnapshotStream}</p>
              <h2 class="mt-1 truncate text-lg font-bold" style="color: var(--ink-strong);">{ui.computer}</h2>
            </div>
            {#if selectedComputerFrame}
              <div class="flex shrink-0 flex-col items-end gap-1 text-right">
                <span class="rounded-full border px-2 py-0.5 text-[10px] font-bold uppercase tracking-[0.16em]" style="border-color: var(--line); background: var(--panel-soft); color: var(--muted);">
                  {selectedComputerFrame.mimeType ?? "image"} · {selectedComputerFrame.transport}
                </span>
                <span class="text-[10px] font-semibold" style="color: var(--muted);">
                  {new Date(selectedComputerFrame.updatedAt).toLocaleTimeString()}
                </span>
              </div>
            {/if}
          </div>

          <div class="flex min-h-0 flex-1 items-center justify-center overflow-hidden rounded-3xl border p-3 shadow-sm" style="border-color: var(--line); background: color-mix(in srgb, var(--panel-strong) 92%, #020617 8%);">
            {#if selectedComputerFrame}
              <figure class="flex h-full w-full flex-col overflow-hidden rounded-2xl border" style="border-color: var(--line); background: #020617;">
                <button
                  aria-label={ui.computerClickHint}
                  class={`min-h-0 flex-1 overflow-hidden border-0 bg-transparent p-0 ${readOnlyRole || computerInputBusy ? "" : "cursor-crosshair"}`}
                  data-testid="computer-frame-image"
                  disabled={readOnlyRole || computerInputBusy}
                  onclick={handleComputerFrameClick}
                  type="button"
                >
                  <img
                    alt={ui.computer}
                    class="h-full w-full object-contain"
                    decoding="async"
                    src={selectedComputerFrame.imageUrl}
                  />
                </button>
                <figcaption class="flex shrink-0 flex-wrap items-center justify-between gap-2 border-t px-3 py-2 text-[11px] font-semibold" style="border-color: rgba(148, 163, 184, 0.22); color: #cbd5e1;">
                  <span class="truncate">{selectedComputerFrame.tool ?? ui.computerFrameUpdated}</span>
                  <span class="shrink-0">{selectedComputerFrame.frameMode} · {selectedComputerFrame.fpsHint ?? 1} fps</span>
                </figcaption>
              </figure>
            {:else}
              <div class="max-w-md rounded-2xl border px-5 py-8 text-center shadow-sm" style="border-color: var(--line); background: var(--panel-strong); color: var(--muted);">
                <Monitor class="mx-auto mb-3" size={30} />
                <p class="text-sm font-semibold">{ui.computerNoFrames}</p>
              </div>
            {/if}
          </div>
          <div class="grid shrink-0 gap-3 rounded-2xl border p-3 sm:grid-cols-[1fr_auto]" style="border-color: var(--line); background: var(--panel-strong);">
            <div class="min-w-0">
              <p class="mb-2 text-[10px] font-bold uppercase tracking-[0.16em]" style="color: var(--muted);">{ui.computerInputHint}</p>
              <div class="flex min-w-0 items-center gap-2">
                <input
                  bind:value={computerInputText}
                  class="min-w-0 flex-1 rounded-xl border px-3 py-2 text-sm outline-none transition focus:ring-2 focus:ring-amber-500/20"
                  data-testid="computer-input-text"
                  disabled={readOnlyRole || computerInputBusy || !selectedComputerFrame}
                  onkeydown={(event) => {
                    if (event.key !== "Enter") {
                      return;
                    }
                    event.preventDefault();
                    sendComputerTextInput();
                  }}
                  placeholder={ui.computerClickHint}
                  style="border-color: var(--line); background: var(--panel-soft); color: var(--ink-strong);"
                  type="text"
                />
                <button
                  class="surface-contrast-button ui-animated-button ui-animated-button--strong rounded-xl px-3 py-2 text-xs font-bold disabled:cursor-not-allowed disabled:opacity-50"
                  data-testid="computer-input-send"
                  disabled={readOnlyRole || computerInputBusy || !selectedComputerFrame || !computerInputText.trim()}
                  onclick={sendComputerTextInput}
                  type="button"
                >
                  {ui.sendComputerInput}
                </button>
              </div>
              {#if computerInputStatus}
                <p class="mt-2 text-[11px] font-semibold" style="color: var(--muted);">{computerInputStatus}</p>
              {/if}
            </div>
            <div class="flex flex-wrap items-end gap-2">
              <button class="ui-animated-button ui-animated-button--soft rounded-xl border px-3 py-2 text-xs font-bold disabled:opacity-50" data-testid="computer-key-enter" disabled={readOnlyRole || computerInputBusy || !selectedComputerFrame} onclick={() => sendComputerKeyInput("Enter")} style="border-color: var(--line); color: var(--ink);" type="button">Enter</button>
              <button class="ui-animated-button ui-animated-button--soft rounded-xl border px-3 py-2 text-xs font-bold disabled:opacity-50" disabled={readOnlyRole || computerInputBusy || !selectedComputerFrame} onclick={() => sendComputerKeyInput("Escape")} style="border-color: var(--line); color: var(--ink);" type="button">Esc</button>
              <button class="ui-animated-button ui-animated-button--soft rounded-xl border px-3 py-2 text-xs font-bold disabled:opacity-50" disabled={readOnlyRole || computerInputBusy || !selectedComputerFrame} onclick={() => sendComputerScrollInput(-640)} style="border-color: var(--line); color: var(--ink);" type="button">{ui.scrollUp}</button>
              <button class="ui-animated-button ui-animated-button--soft rounded-xl border px-3 py-2 text-xs font-bold disabled:opacity-50" disabled={readOnlyRole || computerInputBusy || !selectedComputerFrame} onclick={() => sendComputerScrollInput(640)} style="border-color: var(--line); color: var(--ink);" type="button">{ui.scrollDown}</button>
            </div>
          </div>
        </div>
      {:else if activeWorkspaceTabId === "diagnostics"}
        <div class="h-full overflow-y-auto" style="background: var(--bg);">
          {#if DiagnosticsWorkspaceView}
            <DiagnosticsWorkspaceView
              connectionState={connectionState}
              notifications={notifications}
              runtime={runtime}
              sessions={sessions}
              selectedSessionId={selectedSessionId}
              webRole={webRole}
              onOpenSession={async (sessionId, profileId = null) => {
                activeWorkspaceTabId = "chat";
                await selectSession(sessionId, profileId);
              }}
            />
          {:else}
            <div class="workspace-loading-card h-full">
              <RefreshCw size={16} class="animate-spin text-gray-300" />
              <span>{getWorkspaceLoadingLabel()}</span>
            </div>
          {/if}
        </div>
      {:else if activeWorkspaceTabId === "memory"}
        <div class="h-full overflow-y-auto" style="background: var(--bg);">
          {#if MemoryWorkspaceView}
            <MemoryWorkspaceView
              selectedSessionId={selectedSessionId}
              selectedSessionProfileId={profileIdForSession(selectedSessionId)}
              webRole={webRole}
            />
          {:else}
            <div class="workspace-loading-card h-full">
              <RefreshCw size={16} class="animate-spin text-gray-300" />
              <span>{getWorkspaceLoadingLabel()}</span>
            </div>
          {/if}
        </div>
      {:else if activeWorkspaceTabId === "git"}
        {#if GitWorkspaceView}
          <GitWorkspaceView
            openRequest={gitOpenRequest}
            onOpenCommitDiff={openGitCommitDiffTab}
            onOpenDiffTab={openGitDiffTab}
            onSelectRepo={handleRepoSelect}
            readOnly={readOnlyRole}
            selectedRepoPath={readOnlyRole ? (viewerGitRepoPath ?? conversation?.preferences.gitRepoPath ?? null) : (conversation?.preferences.gitRepoPath ?? null)}
          />
        {:else}
          <div class="workspace-loading-card h-full">
            <RefreshCw size={16} class="animate-spin text-gray-300" />
            <span>{getWorkspaceLoadingLabel()}</span>
          </div>
        {/if}
      {:else if activeGitDiffTab}
        {#if GitWorkspaceView}
          <GitWorkspaceView
            onOpenCommitDiff={openGitCommitDiffTab}
            onOpenDiffTab={openGitDiffTab}
            onSelectRepo={(repoPath) => handleGitDiffTabRepoSelect(activeGitDiffTab.id, repoPath)}
            openRequest={activeGitDiffTab.request}
            readOnly={readOnlyRole}
            selectedRepoPath={activeGitDiffTab.repoPath}
          />
        {:else}
          <div class="workspace-loading-card h-full">
            <RefreshCw size={16} class="animate-spin text-gray-300" />
            <span>{getWorkspaceLoadingLabel()}</span>
          </div>
        {/if}
      {:else if activeCodeDiffTab}
        {#if CodeDiffWorkspaceView}
          <CodeDiffWorkspaceView onClose={() => closeCodeDiffTab(activeCodeDiffTab.id)} title={activeCodeDiffTab.title} views={activeCodeDiffTab.views} />
        {:else}
          <div class="workspace-loading-card h-full">
            <RefreshCw size={16} class="animate-spin text-gray-300" />
            <span>{getWorkspaceLoadingLabel()}</span>
          </div>
        {/if}
      {:else if activeFileTab}
        {#if FileWorkspaceView}
          <FileWorkspaceView
            filePath={activeFileTab.path}
            readOnly={readOnlyRole}
            onClose={() => closeFileTab(activeFileTab.id)}
            onOpenDiff={(filePath) => void openGitDiffFromPath(filePath)}
            onOpenGit={(filePath) => void openGitWorkspaceFileFromPath(filePath)}
            onOpenLocalPath={(href) => openFileFromMessage(href)}
          />
        {:else}
          <div class="workspace-loading-card h-full">
            <RefreshCw size={16} class="animate-spin text-gray-300" />
            <span>{getWorkspaceLoadingLabel()}</span>
          </div>
        {/if}
      {:else}
        {#if TerminalWorkspaceView}
          <TerminalWorkspaceView
            terminalId={activeWorkspaceTabId.replace(/^terminal:/u, "")}
            selectedSessionId={selectedSessionId}
            selectedSessionProfileId={profileIdForSession(selectedSessionId)}
            readOnly={readOnlyRole}
            onAttachContext={(payload) => {
              attachTerminalContext(payload);
            }}
          />
        {:else}
          <div class="workspace-loading-card h-full">
            <RefreshCw size={16} class="animate-spin text-gray-300" />
            <span>{getWorkspaceLoadingLabel()}</span>
          </div>
        {/if}
      {/if}
    </div>
  </main>
</div>

{#if selectedSessionId && activeWorkspaceTabId === "chat" && sessionTurnSearchOpen}
  <SessionTurnSearchPopover
    bind:inputElement={sessionTurnSearchInputElement}
    bind:popoverElement={sessionTurnSearchPopoverElement}
    busy={sessionTurnSearchBusy}
    closeLabel={ui.close}
    contextCompressionLabel={ui.contextCompression}
    cursor={sessionTurnSearchCursor}
    error={sessionTurnSearchError}
    jumpingTurnId={sessionTurnSearchJumpingTurnId}
    loadingMore={sessionTurnSearchLoadingMore}
    onInput={() => scheduleSessionTurnSearch(true)}
    onJump={jumpToSessionSearchResult}
    onLoadMore={loadMoreSessionTurnSearchResults}
    onReset={resetSessionTurnSearch}
    planModeLabel={ui.planMode}
    bind:query={sessionTurnSearchQuery}
    results={sessionTurnSearchResults}
    searchCopy={sessionSearchCopy}
    style={sessionTurnSearchPopoverStyle || "opacity:0;pointer-events:none;"}
    totalMatches={sessionTurnSearchTotalMatches}
  />
{/if}

{#if profileMoveDialogSession}
  {@const profileMoveOptions = profileMoveOptionsForSession(profileMoveDialogSession)}
  <div aria-modal="true" class="fixed inset-0 flex items-center justify-center px-4 py-6" role="dialog" style="z-index:var(--z-modal);">
    <button
      aria-label={ui.close}
      class="absolute inset-0 bg-slate-950/45 backdrop-blur-sm"
      onclick={closeSessionProfileMoveDialog}
      type="button"
    ></button>
    <section
      aria-labelledby="session-profile-move-title"
      class="relative w-full max-w-md overflow-hidden rounded-3xl border shadow-2xl"
      style="border-color: var(--line); background: var(--panel-strong); color: var(--ink-strong);"
    >
      <div class="flex items-start justify-between gap-4 border-b px-5 py-4" style="border-color: var(--line);">
        <div class="min-w-0">
          <p class="mb-1 inline-flex items-center gap-1.5 rounded-full px-2 py-0.5 text-[10px] font-bold uppercase tracking-[0.18em]" style="background: var(--panel-soft); color: var(--muted);">
            <UserCog size={11} />
            {ui.moveSessionToAccount}
          </p>
          <h2 id="session-profile-move-title" class="truncate text-base font-bold">
            {profileMoveDialogSession.name || profileMoveDialogSession.preview || ui.newThread}
          </h2>
          <p class="mt-1 text-xs" style="color: var(--muted);">{ui.moveSessionToAccountDescription}</p>
        </div>
        <button
          class="rounded-xl p-2 transition-colors hover:bg-black/5"
          onclick={closeSessionProfileMoveDialog}
          title={ui.close}
          type="button"
        >
          <X size={16} />
        </button>
      </div>

      <div class="space-y-4 px-5 py-4">
        <div class="rounded-2xl border px-3 py-2.5 text-xs" style="border-color: var(--line); background: var(--panel-soft);">
          <p class="mb-1 font-bold uppercase tracking-[0.16em]" style="color: var(--muted);">{ui.currentAccount}</p>
          <p class="truncate font-semibold">
            {profileMoveDialogSession.profileLabel || profileMoveDialogSession.accountEmail || profileMoveDialogSession.profileId || activeProfileId || "default"}
          </p>
          {#if profileMoveDialogSession.profileCodexHome}
            <p class="mt-1 truncate text-[11px]" style="color: var(--muted);">{profileMoveDialogSession.profileCodexHome}</p>
          {/if}
        </div>

        <label class="block space-y-2">
          <span class="text-[10px] font-bold uppercase tracking-[0.18em]" style="color: var(--muted);">{ui.targetAccount}</span>
          <select
            bind:value={profileMoveDialogTargetId}
            class="w-full rounded-2xl border px-3 py-2.5 text-sm font-semibold outline-none transition-colors focus:border-amber-300 focus:ring-2 focus:ring-amber-200/60"
            style="border-color: var(--line); background: var(--panel); color: var(--ink-strong);"
          >
            {#each profileMoveOptions as profile (profile.id)}
              <option value={profile.id}>{profile.label || profile.id} · {profile.codexHome}</option>
            {/each}
          </select>
        </label>
      </div>

      <div class="flex items-center justify-end gap-2 border-t px-5 py-4" style="border-color: var(--line); background: var(--panel-soft);">
        <button
          class="ui-animated-button ui-animated-button--soft rounded-xl border px-3 py-2 text-xs font-bold"
          style="border-color: var(--line); color: var(--ink);"
          onclick={closeSessionProfileMoveDialog}
          type="button"
        >
          {ui.close}
        </button>
        <button
          class="ui-animated-button ui-animated-button--strong rounded-xl px-3 py-2 text-xs font-bold disabled:cursor-not-allowed disabled:opacity-50"
          disabled={!profileMoveDialogTargetId || profileMoveOptions.length === 0}
          onclick={() => void confirmSessionProfileMoveDialog()}
          type="button"
        >
          {ui.moveSession}
        </button>
      </div>
    </section>
  </div>
{/if}

{#if sessionRecoveryPrompt}
  <SessionRecoveryModal
    getIssueLabel={getSessionRecoveryIssueLabel}
    onDismiss={dismissSessionRecoveryPrompt}
    onRecover={recoverSessionHistoryPrompt}
    prompt={sessionRecoveryPrompt}
  />
{/if}

{#if startupAlertModalOpen && config && (startupPausedQueues.length > 0 || startupScheduledShutdown)}
  <StartupAlertModal
    fallbackThreadTitle={getDefaultThreadTitle()}
    onDismiss={dismissStartupAlertModal}
    onOpenSession={openStartupAlertSession}
    pausedQueues={startupPausedQueues}
    scheduledShutdown={startupScheduledShutdown}
    scheduledShutdownThreadLabel={startupScheduledShutdownThreadLabel}
    shutdownDelaySeconds={config.systemShutdown.delaySeconds}
    shutdownRemainingSeconds={startupShutdownRemainingSeconds}
    {ui}
  />
{/if}

{#if isMobileLayout && mobileSidebarOpen}
  <button
    aria-label={ui.closeThreadList}
    class="ui-scrim ui-scrim--soft fixed inset-0 z-[120] transition-all"
    onclick={closeMobileSidebar}
    type="button"
  ></button>
{/if}

{#if browserOpen}
  <FolderBrowserDialog
    busy={browserBusy}
    closeLabel={m.close_folder_picker()}
    confirmLabel={ui.selectFolder}
    {directoryPayload}
    loadingLabel={m.scanning_folders()}
    onBrowse={browseTo}
    onClose={() => (browserOpen = false)}
    subtitle={m.allowed_root_paths()}
    title={m.select_working_folder()}
  />
{/if}
{/if}

<style>
  @keyframes thinking-chip-sheen {
    0% {
      transform: translateX(-132%);
      opacity: 0;
    }

    18% {
      opacity: 0.34;
    }

    58% {
      opacity: 0.18;
    }

    100% {
      transform: translateX(168%);
      opacity: 0;
    }
  }

  @keyframes diff-loading-bar {
    0% {
      transform: translateX(-68%) scaleX(0.55);
    }

    45% {
      transform: translateX(8%) scaleX(0.82);
    }

    75% {
      transform: translateX(72%) scaleX(1);
    }

    100% {
      transform: translateX(164%) scaleX(0.62);
    }
  }

  .thinking-indicator {
    position: relative;
    overflow: hidden;
    isolation: isolate;
    backdrop-filter: blur(10px);
  }

  .thinking-indicator::after {
    content: "";
    position: absolute;
    inset: 0;
    width: 42%;
    background: linear-gradient(90deg, rgba(255, 255, 255, 0), rgba(251, 191, 36, 0.18), rgba(255, 255, 255, 0));
    transform: translateX(-132%);
    animation: thinking-chip-sheen 2.6s cubic-bezier(0.22, 1, 0.36, 1) infinite;
    pointer-events: none;
    z-index: 0;
  }

  .thinking-indicator__label {
    position: relative;
    z-index: 1;
    color: rgba(17, 24, 39, 0.92);
    letter-spacing: -0.01em;
  }

  .diff-loading-bar {
    width: 46%;
    transform-origin: left center;
    animation: diff-loading-bar 1.45s cubic-bezier(0.22, 1, 0.36, 1) infinite;
    will-change: transform;
  }

  .snackbar-card {
    will-change: transform, opacity, box-shadow;
  }

  .workspace-snackbar-stack {
    top: calc(env(safe-area-inset-top) + 4rem);
  }

  .ui-animated-button {
    will-change: transform, box-shadow, background-color, border-color, color, opacity;
    transition:
      transform 180ms cubic-bezier(0.22, 1, 0.36, 1),
      box-shadow 220ms cubic-bezier(0.22, 1, 0.36, 1),
      background-color 180ms ease,
      border-color 180ms ease,
      color 180ms ease,
      opacity 180ms ease;
  }

  .ui-animated-button:hover:not(:disabled) {
    transform: translateY(-1px);
  }

  .ui-animated-button:active:not(:disabled) {
    transform: translateY(0) scale(0.98);
    transition-duration: 110ms;
  }

  .ui-animated-button--soft:hover:not(:disabled) {
    box-shadow: 0 14px 28px -24px rgba(15, 23, 42, 0.5);
  }

  .ui-animated-button--strong:hover:not(:disabled) {
    box-shadow: 0 22px 36px -26px rgba(15, 23, 42, 0.45);
  }

  .ui-animated-button--icon:hover:not(:disabled) {
    transform: translateY(-1px) scale(1.03);
  }

  .ui-animated-button:disabled {
    transform: none;
    box-shadow: none;
  }

  .workspace-loading-card {
    display: flex;
    min-height: 14rem;
    align-items: center;
    justify-content: center;
    gap: 0.75rem;
    border: 1px solid rgba(148, 163, 184, 0.18);
    border-radius: 1.5rem;
    background: color-mix(in srgb, var(--panel) 92%, transparent);
    color: rgba(100, 116, 139, 0.95);
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.38);
  }

  .top-load-bar-track {
    height: 0.18rem;
    width: 100%;
    overflow: hidden;
    background: color-mix(in srgb, var(--panel-soft) 72%, transparent);
  }

  .top-load-bar-fill {
    height: 100%;
    border-radius: 999px;
    background: linear-gradient(90deg, color-mix(in srgb, var(--accent) 76%, white 6%), color-mix(in srgb, var(--accent) 54%, var(--ink) 10%));
    transition: width 180ms cubic-bezier(0.22, 1, 0.36, 1);
    box-shadow: 0 0 20px color-mix(in srgb, var(--accent) 24%, transparent);
  }

  .top-load-pill {
    position: absolute;
    top: 0.75rem;
    left: 50%;
    translate: -50% 0;
    display: inline-flex;
    max-width: min(22rem, calc(100vw - 2rem));
    align-items: center;
    gap: 0.45rem;
    border: 1px solid color-mix(in srgb, var(--line) 78%, transparent);
    border-radius: 999px;
    background: color-mix(in srgb, var(--panel-strong) 92%, transparent);
    color: color-mix(in srgb, var(--ink) 76%, transparent);
    padding: 0.38rem 0.7rem;
    font-size: 0.68rem;
    font-weight: 600;
    letter-spacing: -0.01em;
    box-shadow: 0 18px 40px -34px rgba(15, 23, 42, 0.42);
    backdrop-filter: blur(14px);
  }

  .top-load-pill span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .dock-sync-pill {
    display: inline-flex;
    max-width: min(24rem, calc(100vw - 3rem));
    align-items: center;
    gap: 0.45rem;
    border: 1px solid color-mix(in srgb, var(--line) 80%, transparent);
    border-radius: 999px;
    background: color-mix(in srgb, var(--panel-strong) 94%, transparent);
    color: color-mix(in srgb, var(--ink) 74%, transparent);
    padding: 0.36rem 0.72rem;
    font-size: 0.68rem;
    font-weight: 650;
    letter-spacing: -0.01em;
    box-shadow: 0 18px 44px -34px rgba(15, 23, 42, 0.48);
    backdrop-filter: blur(16px);
  }

  .dock-sync-pill span {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .chat-transcript {
    overflow-anchor: none;
  }

  .transcript-dock {
    position: absolute;
    inset-inline: 0;
    bottom: 0;
    background: linear-gradient(
      180deg,
      rgba(255, 255, 255, 0) 0%,
      rgba(255, 255, 255, 0.92) 22%,
      rgba(255, 255, 255, 0.97) 58%,
      rgba(255, 255, 255, 0.99) 100%
    );
  }

  .composer-panel {
    backdrop-filter: blur(18px);
  }

  .ui-scrim {
    backdrop-filter: blur(14px);
  }

  .ui-scrim--soft {
    background: linear-gradient(180deg, rgba(15, 23, 42, 0.2), rgba(15, 23, 42, 0.36));
  }

  .ui-scrim--strong {
    background: linear-gradient(180deg, rgba(15, 23, 42, 0.36), rgba(15, 23, 42, 0.58));
  }

  .ui-scrim--modal {
    background: linear-gradient(180deg, rgba(15, 23, 42, 0.48), rgba(15, 23, 42, 0.72));
  }

  .search-popover {
    background: linear-gradient(180deg, rgba(255, 255, 255, 0.98), rgba(248, 250, 252, 0.98));
    box-shadow: 0 26px 60px -34px rgba(15, 23, 42, 0.34);
  }

  .search-popover__header {
    background: linear-gradient(180deg, rgba(255, 255, 255, 0.86), rgba(248, 250, 252, 0.96));
  }

  .search-popover__item {
    transition:
      background-color 160ms ease,
      color 160ms ease;
  }

  .search-popover__badge {
    transition:
      background-color 160ms ease,
      color 160ms ease;
  }

  .composer-popover[data-positioned="true"] {
    transform-origin: bottom left;
    animation: composer-popover-enter 220ms cubic-bezier(0.22, 1, 0.36, 1);
    will-change: transform, opacity;
  }

  .composer-settings-popover,
  .composer-settings-popover__header,
  .composer-settings-popover__close,
  .composer-settings-tabs,
  .composer-settings-tab,
  .composer-settings-card,
  .composer-settings-card__icon,
  .composer-settings-card__eyebrow,
  .composer-settings-card__value,
  .composer-settings-segmented,
  .composer-settings-segmented__button {
    transition:
      background-color 160ms ease,
      border-color 160ms ease,
      color 160ms ease,
      box-shadow 160ms ease;
  }

  @keyframes composer-popover-enter {
    0% {
      opacity: 0;
      transform: translateY(10px) scale(0.985);
    }

    100% {
      opacity: 1;
      transform: translateY(0) scale(1);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .composer-popover[data-positioned="true"] {
      animation: none;
    }
  }

  :global(:root[data-theme="dark"]) .transcript-dock {
    background: linear-gradient(
      180deg,
      rgba(11, 18, 32, 0) 0%,
      rgba(11, 18, 32, 0.82) 20%,
      rgba(11, 18, 32, 0.95) 56%,
      rgba(11, 18, 32, 0.99) 100%
    );
  }

  :global(:root[data-theme="dark"]) .ui-scrim--soft {
    background: linear-gradient(180deg, rgba(2, 6, 23, 0.34), rgba(2, 6, 23, 0.56));
  }

  :global(:root[data-theme="dark"]) .ui-scrim--strong {
    background: linear-gradient(180deg, rgba(2, 6, 23, 0.5), rgba(2, 6, 23, 0.72));
  }

  :global(:root[data-theme="dark"]) .ui-scrim--modal {
    background: linear-gradient(180deg, rgba(2, 6, 23, 0.64), rgba(2, 6, 23, 0.82));
  }

  :global(:root[data-theme="dark"]) .search-popover {
    border-color: rgba(71, 85, 105, 0.5) !important;
    background: linear-gradient(180deg, rgba(17, 24, 39, 0.98), rgba(11, 18, 32, 1)) !important;
    box-shadow: 0 34px 72px -40px rgba(2, 6, 23, 0.94) !important;
  }

  :global(:root[data-theme="dark"]) .search-popover__header {
    border-color: rgba(71, 85, 105, 0.36) !important;
    background: linear-gradient(180deg, rgba(17, 24, 39, 0.92), rgba(15, 23, 42, 0.98)) !important;
  }

  :global(:root[data-theme="dark"]) .search-popover__input {
    color: #f8fafc !important;
  }

  :global(:root[data-theme="dark"]) .search-popover__input::placeholder {
    color: #64748b !important;
  }

  :global(:root[data-theme="dark"]) .search-popover__meta,
  :global(:root[data-theme="dark"]) .search-popover__empty {
    color: #94a3b8 !important;
  }

  :global(:root[data-theme="dark"]) .search-popover__list {
    border-color: rgba(71, 85, 105, 0.24) !important;
  }

  :global(:root[data-theme="dark"]) .search-popover__item {
    color: #e2e8f0 !important;
  }

  :global(:root[data-theme="dark"]) .search-popover__item:hover {
    background: rgba(245, 158, 11, 0.12) !important;
  }

  :global(:root[data-theme="dark"]) .search-popover__badge {
    background: rgba(51, 65, 85, 0.82) !important;
    color: #cbd5e1 !important;
  }

  :global(:root[data-theme="dark"]) .search-popover__close:hover {
    background: rgba(51, 65, 85, 0.78) !important;
    color: #f8fafc !important;
  }

  :global(:root[data-theme="dark"]) .thinking-indicator {
    border-color: rgba(71, 85, 105, 0.44) !important;
    background: rgba(15, 23, 42, 0.88) !important;
    box-shadow: 0 18px 42px -30px rgba(2, 6, 23, 0.84) !important;
  }

  :global(:root[data-theme="dark"]) .thinking-indicator::after {
    background: linear-gradient(90deg, rgba(255, 255, 255, 0), rgba(251, 191, 36, 0.2), rgba(255, 255, 255, 0));
  }

  :global(:root[data-theme="dark"]) .thinking-indicator__label {
    color: rgba(248, 250, 252, 0.94);
  }

  :global(:root[data-theme="dark"]) .auth-dialog-card {
    border-color: rgba(71, 85, 105, 0.42) !important;
    background: linear-gradient(180deg, rgba(17, 24, 39, 0.94), rgba(11, 18, 32, 0.98)) !important;
    box-shadow: 0 34px 84px -42px rgba(2, 6, 23, 0.95) !important;
  }

  :global(:root[data-theme="dark"]) .auth-dialog-card .text-gray-950 {
    color: #f8fafc !important;
  }

  :global(:root[data-theme="dark"]) .auth-dialog-card .text-gray-500,
  :global(:root[data-theme="dark"]) .auth-dialog-card .text-gray-700,
  :global(:root[data-theme="dark"]) .auth-dialog-card .text-gray-400 {
    color: #94a3b8 !important;
  }

  :global(:root[data-theme="dark"]) .auth-dialog-select,
  :global(:root[data-theme="dark"]) .auth-dialog-input,
  :global(:root[data-theme="dark"]) .auth-dialog-hcaptcha {
    border-color: rgba(71, 85, 105, 0.44) !important;
    background: rgba(15, 23, 42, 0.9) !important;
    color: #f8fafc !important;
    box-shadow: inset 0 0 0 1px rgba(148, 163, 184, 0.06);
  }

  :global(:root[data-theme="dark"]) .auth-dialog-input::placeholder {
    color: #64748b !important;
  }

  :global(:root[data-theme="dark"]) .auth-dialog-message {
    border-color: rgba(248, 113, 113, 0.34) !important;
    background: rgba(127, 29, 29, 0.34) !important;
    color: #fecaca !important;
    box-shadow: 0 18px 42px -30px rgba(127, 29, 29, 0.82) !important;
  }

  :global(:root[data-theme="dark"]) .snackbar-card {
    box-shadow: 0 22px 48px -28px rgba(2, 6, 23, 0.94) !important;
  }

  :global(:root[data-theme="dark"]) .snackbar-card[data-tone="error"] {
    border-color: rgba(248, 113, 113, 0.38) !important;
    background: linear-gradient(180deg, rgba(69, 10, 10, 0.95), rgba(39, 11, 11, 0.98)) !important;
    color: #fee2e2 !important;
    box-shadow: 0 24px 52px -30px rgba(127, 29, 29, 0.72) !important;
  }

  :global(:root[data-theme="dark"]) .snackbar-card[data-tone="warning"] {
    border-color: rgba(245, 158, 11, 0.38) !important;
    background: linear-gradient(180deg, rgba(69, 39, 10, 0.95), rgba(45, 28, 10, 0.98)) !important;
    color: #fef3c7 !important;
    box-shadow: 0 24px 52px -30px rgba(146, 64, 14, 0.72) !important;
  }

  :global(:root[data-theme="dark"]) .snackbar-card[data-tone="success"],
  :global(:root[data-theme="dark"]) .snackbar-card[data-tone="info"] {
    border-color: rgba(52, 211, 153, 0.3) !important;
    background: linear-gradient(180deg, rgba(7, 47, 38, 0.95), rgba(5, 30, 24, 0.98)) !important;
    color: #d1fae5 !important;
    box-shadow: 0 24px 52px -30px rgba(6, 95, 70, 0.7) !important;
  }

  :global(:root[data-theme="dark"]) .snackbar-card button:hover {
    background: rgba(255, 255, 255, 0.08) !important;
  }

  :global(:root[data-theme="dark"]) .startup-alert-card,
  :global(:root[data-theme="dark"]) .folder-dialog-card {
    border-color: rgba(71, 85, 105, 0.4) !important;
    background: linear-gradient(180deg, rgba(17, 24, 39, 0.96), rgba(11, 18, 32, 0.99)) !important;
    box-shadow: 0 36px 90px -48px rgba(2, 6, 23, 0.98) !important;
  }

  :global(:root[data-theme="dark"]) .startup-alert-card__hero,
  :global(:root[data-theme="dark"]) .folder-dialog-card__header {
    border-color: rgba(71, 85, 105, 0.36) !important;
    background: linear-gradient(180deg, rgba(30, 41, 59, 0.92), rgba(17, 24, 39, 0.98)) !important;
  }

  :global(:root[data-theme="dark"]) .folder-dialog-card__footer {
    border-color: rgba(71, 85, 105, 0.36) !important;
    background: rgba(15, 23, 42, 0.9) !important;
  }

  :global(:root[data-theme="dark"]) .startup-alert-card .text-gray-950,
  :global(:root[data-theme="dark"]) .startup-alert-card .text-gray-900,
  :global(:root[data-theme="dark"]) .folder-dialog-card .text-gray-900 {
    color: #f8fafc !important;
  }

  :global(:root[data-theme="dark"]) .startup-alert-card .text-gray-600,
  :global(:root[data-theme="dark"]) .startup-alert-card .text-gray-500,
  :global(:root[data-theme="dark"]) .startup-alert-card .text-gray-400,
  :global(:root[data-theme="dark"]) .folder-dialog-card .text-gray-600,
  :global(:root[data-theme="dark"]) .folder-dialog-card .text-gray-500,
  :global(:root[data-theme="dark"]) .folder-dialog-card .text-gray-400 {
    color: #94a3b8 !important;
  }

  :global(:root[data-theme="dark"]) .startup-alert-card__section {
    border-color: rgba(71, 85, 105, 0.34) !important;
    background: rgba(15, 23, 42, 0.78) !important;
  }

  :global(:root[data-theme="dark"]) .startup-alert-card__section--accent {
    border-color: rgba(245, 158, 11, 0.28) !important;
    background: linear-gradient(180deg, rgba(69, 39, 10, 0.3), rgba(30, 41, 59, 0.82)) !important;
  }

  :global(:root[data-theme="dark"]) .startup-alert-card__queue,
  :global(:root[data-theme="dark"]) .startup-alert-card__callout {
    border-color: rgba(71, 85, 105, 0.34) !important;
    background: rgba(17, 24, 39, 0.88) !important;
  }

  :global(:root[data-theme="dark"]) .queue-resume-banner {
    background: linear-gradient(135deg, rgba(17, 24, 39, 0.96), rgba(15, 23, 42, 0.98)) !important;
    border: 1px solid rgba(71, 85, 105, 0.55);
    box-shadow: 0 26px 54px -34px rgba(2, 6, 23, 0.82) !important;
  }

  :global(:root[data-theme="dark"]) .queue-resume-ignore {
    background-color: rgba(51, 65, 85, 0.92) !important;
    color: #f8fafc !important;
  }

  :global(:root[data-theme="dark"]) .queue-resume-ignore:hover {
    background-color: rgba(71, 85, 105, 0.98) !important;
  }

  :global(:root[data-theme="dark"]) .composer-panel {
    background: linear-gradient(180deg, rgba(17, 24, 39, 0.96), rgba(11, 18, 32, 0.985)) !important;
    border-color: rgba(71, 85, 105, 0.58) !important;
    box-shadow: 0 30px 72px -38px rgba(2, 6, 23, 0.88) !important;
  }

  :global(:root[data-theme="dark"]) .composer-panel:focus-within {
    background: linear-gradient(180deg, rgba(17, 24, 39, 0.98), rgba(11, 18, 32, 1)) !important;
    border-color: rgba(245, 158, 11, 0.45) !important;
    box-shadow: 0 30px 72px -36px rgba(245, 158, 11, 0.28) !important;
  }

  :global(:root[data-theme="dark"]) .composer-textarea {
    color: #f8fafc !important;
  }

  :global(:root[data-theme="dark"]) .composer-toolbar {
    border-color: rgba(71, 85, 105, 0.42) !important;
    background: linear-gradient(180deg, rgba(17, 24, 39, 0.9), rgba(11, 18, 32, 0.97)) !important;
  }

  :global(:root[data-theme="dark"]) .composer-compact-trigger {
    color: #cbd5e1 !important;
  }

  :global(:root[data-theme="dark"]) .composer-compact-trigger:hover,
  :global(:root[data-theme="dark"]) .composer-panel:focus-within .composer-compact-trigger {
    background-color: rgba(255, 255, 255, 0.06) !important;
    border-color: rgba(148, 163, 184, 0.22) !important;
    color: #f8fafc !important;
  }

  :global(:root[data-theme="dark"]) .composer-settings-popover {
    border-color: rgba(71, 85, 105, 0.52) !important;
    background: linear-gradient(180deg, rgba(17, 24, 39, 0.98), rgba(11, 18, 32, 1)) !important;
    box-shadow: 0 34px 76px -42px rgba(2, 6, 23, 0.94) !important;
  }

  :global(:root[data-theme="dark"]) .composer-settings-popover__header {
    border-color: rgba(71, 85, 105, 0.34) !important;
  }

  :global(:root[data-theme="dark"]) .composer-settings-popover .text-gray-400 {
    color: #94a3b8 !important;
  }

  :global(:root[data-theme="dark"]) .composer-settings-popover .text-gray-500 {
    color: #a8b3c7 !important;
  }

  :global(:root[data-theme="dark"]) .composer-settings-popover .text-gray-600,
  :global(:root[data-theme="dark"]) .composer-settings-popover .text-gray-700,
  :global(:root[data-theme="dark"]) .composer-settings-popover .text-gray-900 {
    color: #f8fafc !important;
  }

  :global(:root[data-theme="dark"]) .composer-settings-popover__close:hover {
    border-radius: 0.75rem;
    background: rgba(51, 65, 85, 0.74) !important;
    color: #f8fafc !important;
  }

  :global(:root[data-theme="dark"]) .composer-settings-tabs {
    border-color: rgba(71, 85, 105, 0.34) !important;
    background: rgba(15, 23, 42, 0.84) !important;
  }

  :global(:root[data-theme="dark"]) .composer-settings-tab[aria-selected="false"] {
    color: #94a3b8 !important;
  }

  :global(:root[data-theme="dark"]) .composer-settings-tab[aria-selected="false"]:hover {
    border-color: rgba(148, 163, 184, 0.18) !important;
    background: rgba(255, 255, 255, 0.06) !important;
    color: #f8fafc !important;
  }

  :global(:root[data-theme="dark"]) .composer-settings-tab[aria-selected="true"] {
    background: rgba(30, 41, 59, 0.94) !important;
    box-shadow: 0 16px 28px -24px rgba(2, 6, 23, 0.72) !important;
  }

  :global(:root[data-theme="dark"]) .composer-settings-popover select {
    border-color: rgba(71, 85, 105, 0.42) !important;
    background: rgba(15, 23, 42, 0.88) !important;
    color: #f8fafc !important;
    box-shadow: inset 0 0 0 1px rgba(148, 163, 184, 0.05);
  }

  :global(:root[data-theme="dark"]) .composer-settings-card {
    border-color: rgba(71, 85, 105, 0.34) !important;
    background: rgba(15, 23, 42, 0.72) !important;
  }

  :global(:root[data-theme="dark"]) .composer-settings-card__icon[data-active="false"] {
    border-color: rgba(71, 85, 105, 0.42) !important;
    background: rgba(17, 24, 39, 0.92) !important;
    color: #94a3b8 !important;
  }

  :global(:root[data-theme="dark"]) .composer-settings-card__icon[data-active="true"] {
    border-color: rgba(245, 158, 11, 0.34) !important;
    background: rgba(245, 158, 11, 0.16) !important;
    color: #fbbf24 !important;
    box-shadow: inset 0 0 0 1px rgba(251, 191, 36, 0.08);
  }

  :global(:root[data-theme="dark"]) .composer-settings-card__eyebrow {
    color: #94a3b8 !important;
  }

  :global(:root[data-theme="dark"]) .composer-settings-card__value {
    color: #e2e8f0 !important;
  }

  :global(:root[data-theme="dark"]) .composer-settings-segmented {
    border-color: rgba(71, 85, 105, 0.36) !important;
    background: rgba(15, 23, 42, 0.92) !important;
    box-shadow:
      inset 0 0 0 1px rgba(148, 163, 184, 0.04),
      0 18px 34px -28px rgba(2, 6, 23, 0.82) !important;
  }

  :global(:root[data-theme="dark"]) .composer-settings-segmented__button[data-selected="false"] {
    color: #94a3b8 !important;
  }

  :global(:root[data-theme="dark"]) .composer-settings-segmented__button[data-selected="false"]:hover {
    border-color: rgba(148, 163, 184, 0.18) !important;
    background: rgba(255, 255, 255, 0.06) !important;
    color: #f8fafc !important;
  }

  :global(:root[data-theme="dark"]) .composer-settings-segmented__button[data-selected="true"][data-tone="default"],
  :global(:root[data-theme="dark"]) .composer-settings-segmented__button[data-selected="true"][data-tone="auto"] {
    border-color: rgba(100, 116, 139, 0.46) !important;
    background: linear-gradient(180deg, rgba(71, 85, 105, 0.96), rgba(51, 65, 85, 1)) !important;
    color: #f8fafc !important;
    box-shadow: 0 14px 24px -20px rgba(2, 6, 23, 0.76) !important;
  }

  :global(:root[data-theme="dark"]) .composer-settings-segmented__button[data-selected="true"][data-tone="fast"] {
    border-color: rgba(245, 158, 11, 0.34) !important;
    background: linear-gradient(180deg, rgba(146, 64, 14, 0.3), rgba(120, 53, 15, 0.42)) !important;
    color: #fcd34d !important;
    box-shadow: 0 14px 24px -20px rgba(146, 64, 14, 0.62) !important;
  }

  :global(:root[data-theme="dark"]) .composer-settings-segmented__button[data-selected="true"][data-tone="flex"] {
    border-color: rgba(56, 189, 248, 0.34) !important;
    background: linear-gradient(180deg, rgba(12, 74, 110, 0.34), rgba(14, 116, 144, 0.28)) !important;
    color: #7dd3fc !important;
    box-shadow: 0 14px 24px -20px rgba(14, 116, 144, 0.52) !important;
  }

  :global(:root[data-theme="dark"]) .composer-settings-segmented__button[data-selected="true"][data-tone="context"] {
    border-color: rgba(245, 158, 11, 0.34) !important;
    background: linear-gradient(180deg, rgba(146, 64, 14, 0.3), rgba(120, 53, 15, 0.42)) !important;
    color: #fcd34d !important;
    box-shadow: 0 14px 24px -20px rgba(146, 64, 14, 0.62) !important;
  }

  :global(:root[data-theme="dark"]) .surface-contrast-button {
    background: linear-gradient(180deg, rgba(51, 65, 85, 0.96), rgba(30, 41, 59, 0.98)) !important;
    color: #f8fafc !important;
    border: 1px solid rgba(148, 163, 184, 0.18);
    box-shadow: 0 18px 34px -26px rgba(2, 6, 23, 0.72) !important;
  }

  :global(:root[data-theme="dark"]) .surface-contrast-button:hover {
    background: linear-gradient(180deg, rgba(71, 85, 105, 0.98), rgba(51, 65, 85, 1)) !important;
    color: #f8fafc !important;
  }

  .turn-card-shell {
    position: relative;
    isolation: auto;
    overflow: visible !important;
  }

  @supports (overflow: clip) {
    .turn-card-shell {
      overflow: clip !important;
    }
  }

  .turn-card-expand {
    overflow: visible;
    transform-origin: top center;
  }

  .turn-card-header {
    position: sticky;
    top: 0.45rem;
    z-index: 26;
    backdrop-filter: blur(14px);
    box-shadow: 0 14px 28px -26px rgba(15, 23, 42, 0.38);
  }

  .turn-card-header[data-sticky-level="1"] {
    top: 3.7rem;
    z-index: 25;
  }

  .turn-card-header[data-sticky-level="2"] {
    top: 6.95rem;
    z-index: 24;
  }

  .turn-card-header[data-sticky-level="3"] {
    top: 10.2rem;
    z-index: 23;
  }

  .live-turn-card .turn-card-header {
    position: relative;
    top: auto;
    z-index: 1;
  }

  .live-turn-card .live-turn-scroll .turn-card-header {
    position: sticky;
    top: 0;
    z-index: 3;
  }

  .turn-card-shell > :first-child {
    border-top-left-radius: inherit;
    border-top-right-radius: inherit;
  }

  .turn-card-shell > :last-child {
    border-bottom-left-radius: inherit;
    border-bottom-right-radius: inherit;
  }

  .turn-card-header--neutral {
    background: linear-gradient(180deg, rgba(255, 255, 255, 0.985), rgba(249, 250, 251, 0.95));
  }

  .turn-card-header--amber {
    background: linear-gradient(180deg, rgba(255, 251, 235, 0.985), rgba(255, 247, 237, 0.95));
  }

  :global(:root[data-theme="dark"]) .turn-card-header--neutral {
    background: linear-gradient(180deg, rgba(17, 24, 39, 0.985), rgba(15, 23, 42, 0.95));
    box-shadow: 0 16px 32px -26px rgba(2, 6, 23, 0.74);
  }

  :global(:root[data-theme="dark"]) .turn-card-header--amber {
    background: linear-gradient(180deg, rgba(69, 39, 10, 0.98), rgba(45, 28, 10, 0.94));
    box-shadow: 0 16px 32px -26px rgba(2, 6, 23, 0.74);
  }

  :global(:root[data-theme="dark"]) .turn-card-load-more {
    border-color: rgba(71, 85, 105, 0.42) !important;
    background: rgba(15, 23, 42, 0.76) !important;
    color: #94a3b8 !important;
  }

  :global(:root[data-theme="dark"]) .turn-card-load-more:hover {
    border-color: rgba(245, 158, 11, 0.34) !important;
    background: rgba(69, 39, 10, 0.28) !important;
    color: #fbbf24 !important;
  }

  .message-timestamp {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    max-width: 100%;
    color: color-mix(in srgb, var(--muted) 86%, transparent);
    font-size: 0.65rem;
    font-weight: 700;
    letter-spacing: 0.02em;
    line-height: 1;
    white-space: nowrap;
  }

  .message-timestamp--right {
    justify-content: flex-end;
    align-self: flex-end;
    padding-right: 0.35rem;
  }

  .message-timestamp--left {
    justify-content: flex-start;
    align-self: flex-start;
    padding-left: 0.15rem;
  }

  :global(:root[data-theme="dark"]) .message-timestamp {
    color: color-mix(in srgb, var(--muted) 92%, white 4%);
  }

  :global(:root[data-theme="dark"]) .large-output-preview pre {
    border-color: rgba(71, 85, 105, 0.54) !important;
    background: rgba(15, 23, 42, 0.82) !important;
    color: #cbd5e1 !important;
  }

  :global(:root[data-theme="dark"]) .large-output-preview > div {
    border-color: rgba(71, 85, 105, 0.42) !important;
    background: rgba(15, 23, 42, 0.64) !important;
  }

  :global(:root[data-theme="dark"]) .large-output-expand {
    border-color: rgba(71, 85, 105, 0.58) !important;
    background: rgba(15, 23, 42, 0.82) !important;
    color: #cbd5e1 !important;
  }

  :global(:root[data-theme="dark"]) .large-output-expand:hover {
    border-color: rgba(245, 158, 11, 0.38) !important;
    background: rgba(69, 39, 10, 0.34) !important;
    color: #fbbf24 !important;
  }

  @media (prefers-reduced-motion: reduce) {
    .thinking-indicator::after {
      animation: none;
    }

    .ui-animated-button,
    .snackbar-card {
      transition-duration: 0.01ms;
    }

    .turn-card-header {
      backdrop-filter: none;
    }
  }

</style>

{#snippet renderHiddenTurnEntriesControl(turnId: string, hiddenCount: number, entries: RenderableTurnEntry[])}
  {#if hiddenCount > 0}
    <button
      class="turn-card-load-more flex w-full items-center justify-center gap-2 rounded-xl border border-dashed border-gray-200 bg-white/70 px-3 py-2 text-[11px] font-bold text-gray-500 transition-colors hover:border-amber-200 hover:bg-amber-50/70 hover:text-amber-700"
      onclick={() => showMoreTurnEntries(turnId, entries)}
      type="button"
    >
      <ChevronDown size={13} />
      <span>{ui.showOlderWorkItems(hiddenCount)}</span>
    </button>
  {/if}
{/snippet}

{#snippet renderCappedOutput(output: string, outputKey: string, compact = false)}
  {@const hiddenOutputCharCount = getHiddenOutputCharCount(output, outputKey)}
  <div class="large-output-preview">
    {#if compact}
      <pre class="max-h-60 overflow-x-auto rounded-lg border border-gray-200 bg-white p-2.5 text-[10px] font-mono leading-relaxed text-gray-600">{getCappedOutputText(output, outputKey)}</pre>
    {:else}
      <pre class="bg-gray-50 p-3 text-[11px] font-mono leading-relaxed text-gray-700 overflow-x-auto">{getCappedOutputText(output, outputKey)}</pre>
    {/if}
    {#if hiddenOutputCharCount > 0}
      <div class="border-t border-gray-100 bg-white/80 px-3 py-2">
        <button
          class="large-output-expand inline-flex items-center gap-2 rounded-lg border border-gray-200 bg-white px-2.5 py-1.5 text-[10px] font-bold text-gray-600 transition-colors hover:border-amber-200 hover:bg-amber-50 hover:text-amber-700"
          onclick={() => expandLargeOutput(outputKey)}
          type="button"
        >
          <ChevronDown size={12} />
          <span>{ui.showFullOutput}</span>
        </button>
      </div>
    {/if}
  </div>
{/snippet}

{#snippet renderDiffLoadingCard(label: string)}
  <div class="space-y-3 p-4 sm:p-5">
    <div class="flex items-center justify-between gap-3">
      <div class="flex items-center gap-2 text-[10px] font-semibold uppercase tracking-[0.2em] text-gray-500">
        <RefreshCw size={12} class="animate-spin text-amber-500" />
        <span>{label}</span>
      </div>
      <span class="text-[10px] font-medium text-gray-400">{m.loading()}</span>
    </div>
    <div
      aria-hidden="true"
      class="overflow-hidden rounded-full bg-gray-200/80"
    >
      <div class="diff-loading-bar h-1.5 rounded-full bg-gradient-to-r from-amber-300 via-amber-500 to-orange-400"></div>
    </div>
  </div>
{/snippet}

{#snippet renderMessageTimestamp(value: number | null | undefined, label: string, align: "left" | "right")}
  {@const timestamp = formatTurnTimestamp(value)}
  {#if timestamp}
    <time
      class={`message-timestamp message-timestamp--${align}`}
      datetime={turnTimestampIso(value)}
      title={`${label} · ${formatTurnTimestamp(value, true)}`}
    >
      <span>{label}</span>
      <span>{timestamp}</span>
    </time>
  {/if}
{/snippet}

{#snippet renderWebSearchDetails(item: CodexItem)}
  {@const queries = getWebSearchQueries(item)}
  {@const actionType = getWebSearchActionType(item)}
  {@const actionUrl = getWebSearchUrl(item)}
  {@const actionPattern = getWebSearchPattern(item)}
  {@const resultSummary = getWebSearchSummary(item)}
  {@const results = getWebSearchResults(item)}
  {@const sources = getWebSearchSources(item)}
  <div class="space-y-3 p-3">
    <div class="grid gap-2 sm:grid-cols-2">
      <div class="rounded-xl border border-gray-100 bg-gray-50 px-3 py-2 dark:border-white/10 dark:bg-white/5">
        <p class="text-[9px] font-bold uppercase tracking-widest text-gray-400 dark:text-gray-500">{m.status_label()}</p>
        <p class="mt-1 text-xs font-semibold text-gray-700 dark:text-gray-200">{getWebSearchStatus(item)}</p>
      </div>
      {#if actionType}
        <div class="rounded-xl border border-gray-100 bg-gray-50 px-3 py-2 dark:border-white/10 dark:bg-white/5">
          <p class="text-[9px] font-bold uppercase tracking-widest text-gray-400 dark:text-gray-500">{m.action_label()}</p>
          <p class="mt-1 text-xs font-semibold text-gray-700 dark:text-gray-200">{actionType}</p>
        </div>
      {/if}
    </div>
    {#if resultSummary}
      <div class="rounded-xl border border-blue-100 bg-blue-50/80 px-3 py-2.5 dark:border-blue-300/20 dark:bg-blue-400/10">
        <p class="text-[9px] font-bold uppercase tracking-widest text-blue-500 dark:text-blue-300">{m.summary_label()}</p>
        <p class="mt-1 text-xs leading-relaxed text-blue-950 dark:text-blue-100">{resultSummary}</p>
      </div>
    {/if}
    {#if queries.length > 0}
      <div class="rounded-xl border border-gray-100 bg-white px-3 py-2.5 dark:border-white/10 dark:bg-white/[0.03]">
        <p class="text-[9px] font-bold uppercase tracking-widest text-gray-400 dark:text-gray-500">{m.queries_label()}</p>
        <ul class="mt-2 space-y-1.5">
          {#each queries as query (`${item.id}:query:${query}`)}
            <li class="rounded-lg bg-gray-50 px-2.5 py-1.5 text-xs font-medium text-gray-700 dark:bg-white/5 dark:text-gray-200">{query}</li>
          {/each}
        </ul>
      </div>
    {/if}
    {#if results.length > 0}
      <div class="rounded-xl border border-gray-100 bg-white px-3 py-2.5 dark:border-white/10 dark:bg-white/[0.03]">
        <p class="text-[9px] font-bold uppercase tracking-widest text-gray-400 dark:text-gray-500">{m.results_label()}</p>
        <ul class="mt-2 space-y-2">
          {#each results as result, index (`${item.id}:result:${result.url || result.title || index}`)}
            <li class="rounded-xl bg-gray-50 px-3 py-2 dark:bg-white/5">
              {#if result.url}
                <a class="block truncate text-xs font-semibold text-blue-700 hover:underline dark:text-blue-300" href={result.url} target="_blank" rel="noreferrer">{result.title || result.url}</a>
              {:else}
                <p class="truncate text-xs font-semibold text-gray-800 dark:text-gray-100">{result.title}</p>
              {/if}
              {#if result.snippet}
                <p class="mt-1 line-clamp-2 text-[11px] leading-relaxed text-gray-500 dark:text-gray-400">{result.snippet}</p>
              {/if}
            </li>
          {/each}
        </ul>
      </div>
    {/if}
    {#if sources.length > 0}
      <div class="rounded-xl border border-gray-100 bg-white px-3 py-2.5 dark:border-white/10 dark:bg-white/[0.03]">
        <p class="text-[9px] font-bold uppercase tracking-widest text-gray-400 dark:text-gray-500">{m.sources_label()}</p>
        <div class="mt-2 flex flex-wrap gap-1.5">
          {#each sources as source, index (`${item.id}:source:${source.url || source.title || index}`)}
            {#if source.url}
              <a class="max-w-full truncate rounded-full border border-gray-200 bg-gray-50 px-2.5 py-1 text-[11px] font-medium text-gray-700 hover:border-blue-200 hover:text-blue-700 dark:border-white/10 dark:bg-white/5 dark:text-gray-200 dark:hover:border-blue-300/40 dark:hover:text-blue-300" href={source.url} target="_blank" rel="noreferrer">{source.title || source.url}</a>
            {:else}
              <span class="max-w-full truncate rounded-full border border-gray-200 bg-gray-50 px-2.5 py-1 text-[11px] font-medium text-gray-700 dark:border-white/10 dark:bg-white/5 dark:text-gray-200">{source.title || source.snippet}</span>
            {/if}
          {/each}
        </div>
      </div>
    {/if}
    {#if actionUrl || actionPattern}
      <div class="rounded-xl border border-gray-100 bg-white px-3 py-2.5 dark:border-white/10 dark:bg-white/[0.03]">
        {#if actionUrl}
          <p class="truncate text-xs font-semibold text-gray-700 dark:text-gray-200">{actionUrl}</p>
        {/if}
        {#if actionPattern}
          <p class="mt-1 truncate text-[11px] text-gray-500 dark:text-gray-400">{actionPattern}</p>
        {/if}
      </div>
    {/if}
  </div>
{/snippet}

{#snippet renderReviewModeItem(item: CodexItem, stickyLevel = 0)}
  {@const findings = getReviewFindings(item)}
  <div class="turn-card-shell overflow-hidden rounded-2xl border border-orange-100 bg-white shadow-sm">
    <div class="turn-card-header turn-card-header--amber flex items-center justify-between gap-3 border-b border-orange-100 px-4 py-3" data-sticky-level={stickyLevel}>
      <div class="flex min-w-0 items-center gap-3">
        <div class="flex h-8 w-8 shrink-0 items-center justify-center rounded-xl border border-orange-100 bg-orange-50 text-orange-700">
          <Shield size={15} />
        </div>
        <div class="min-w-0">
          <h4 class="truncate text-[10px] font-bold uppercase tracking-widest text-orange-700">{getToolItemLabel(item)}</h4>
          <p class="mt-1 line-clamp-2 text-xs text-gray-600">{getReviewText(item)}</p>
        </div>
      </div>
      {#if findings.length > 0}
        <span class="shrink-0 rounded-full bg-orange-50 px-2 py-1 text-[10px] font-bold uppercase tracking-widest text-orange-700">{findings.length}</span>
      {/if}
    </div>
    {#if findings.length > 0}
      <div class="space-y-2 bg-orange-50/35 p-3">
        {#each findings as finding, index (`${item.id}:finding:${index}`)}
          {@const location = getReviewFindingLocation(finding)}
          {@const findingPath = getReviewFindingPath(finding)}
          <article class="rounded-xl border border-orange-100 bg-white px-3 py-2.5">
            <div class="flex items-start justify-between gap-2">
              <h5 class="min-w-0 text-xs font-bold text-gray-800">{formatValue(finding.title) || `Finding ${index + 1}`}</h5>
              {#if formatValue(finding.priority)}
                <span class="shrink-0 rounded-full bg-gray-100 px-1.5 py-0.5 text-[9px] font-bold uppercase text-gray-500">P{formatValue(finding.priority)}</span>
              {/if}
            </div>
            {#if location}
              <div class="mt-1 flex min-w-0 flex-wrap items-center gap-1.5">
                <button class="min-w-0 max-w-full truncate rounded-md border border-orange-100 bg-orange-50 px-2 py-1 text-left font-mono text-[10px] font-semibold text-orange-700 hover:bg-orange-100" onclick={() => openFileTab(findingPath || location)} type="button">{location}</button>
                {#if findingPath}
                  <button class="rounded-md border border-gray-200 bg-white px-2 py-1 text-[10px] font-bold text-gray-600 hover:bg-gray-50" onclick={() => void openGitDiffFromPath(findingPath)} type="button">{m.diff()}</button>
                {/if}
              </div>
            {/if}
            {#if formatValue(finding.body)}
              <p class="mt-2 text-xs leading-relaxed text-gray-600">{formatValue(finding.body)}</p>
            {/if}
          </article>
        {/each}
      </div>
    {/if}
  </div>
{/snippet}

{#snippet renderTurnItem(turnId: string, item: CodexItem, stickyLevel = 0)}
  {#if item.type === "agentMessage"}
    <div class="space-y-2 group/agent-message">
      <div class="flex justify-end opacity-0 group-hover/agent-message:opacity-100 transition-opacity">
        <button class="p-1.5 rounded-lg text-gray-400 hover:text-gray-700 hover:bg-gray-100 transition-colors" onclick={() => void copyMessageText(String(item.text ?? ""))} title={ui.copyReply} type="button"><Copy size={13} /></button>
      </div>
      <div class="prose prose-sm max-w-none text-gray-800 leading-relaxed animate-in fade-in slide-in-from-left-2 duration-700"><MarkdownMessage expandLabel={ui.showFullMessage} maxInitialChars={largeMarkdownInitialChars} on:openLocalPath={(event: CustomEvent<{ href: string }>) => openFileFromMessage(event.detail.href)} text={String(item.text ?? "")} /></div>
    </div>
  {:else if item.type === "imageGeneration"}
    {@const imageSrc = getImageGenerationSource(item)}
    {@const imagePrompt = getImageGenerationPrompt(item)}
    {@const savedPath = getImageGenerationSavedPath(item)}
    <div class="turn-card-shell overflow-hidden rounded-2xl border border-sky-100 bg-white shadow-sm">
      <div class="turn-card-header turn-card-header--neutral flex items-center justify-between gap-3 border-b border-sky-100 px-4 py-3" data-sticky-level={stickyLevel}>
        <div class="flex min-w-0 items-center gap-3">
          <div class="flex h-8 w-8 shrink-0 items-center justify-center rounded-xl border border-sky-100 bg-sky-50 text-sky-700">
            <Layout size={15} />
          </div>
          <div class="min-w-0">
            <h4 class="truncate text-[10px] font-bold uppercase tracking-widest text-sky-700">{item.title ?? m.generated_image()}</h4>
            {#if imagePrompt}
              <p class="mt-1 truncate text-xs text-gray-500">{imagePrompt}</p>
            {:else if formatValue(item.status)}
              <p class="mt-1 truncate text-xs text-gray-500">{formatValue(item.status)}</p>
            {/if}
          </div>
        </div>
        {#if imageSrc}
          <div class="flex shrink-0 items-center gap-1.5">
            <a
              class="inline-flex items-center gap-1.5 rounded-lg border border-gray-200 bg-white px-2.5 py-1.5 text-[10px] font-bold text-gray-700 transition-colors hover:bg-gray-50"
              download={getImageGenerationDownloadName(item)}
              href={imageSrc}
            >
              <Download size={12} />
              <span>{m.save()}</span>
            </a>
            <button
              class="inline-flex items-center gap-1.5 rounded-lg border border-gray-200 bg-white px-2.5 py-1.5 text-[10px] font-bold text-gray-700 transition-colors hover:bg-gray-50"
              onclick={() => window.open(imageSrc, "_blank", "noopener,noreferrer")}
              type="button"
            >
              <ExternalLink size={12} />
              <span>{ui.openInNewTab}</span>
            </button>
          </div>
        {/if}
      </div>
      <div class="space-y-3 bg-sky-50/40 p-3">
        {#if imageSrc}
          <div class="overflow-hidden rounded-2xl border border-sky-100 bg-white">
            <img alt={imagePrompt ?? m.generated_image()} class="block h-auto max-h-[34rem] w-full object-contain" loading="lazy" src={imageSrc} />
          </div>
        {:else if item.detailState === "deferred" || item.resultOmitted}
          <div class="rounded-xl border border-dashed border-sky-200 bg-white px-4 py-5 text-center text-xs text-gray-500">
            <p>{getToolItemSummary(item)}</p>
            {#if getItemDetailError(turnId, item.id)}
              <p class="mt-2 text-red-600">{getItemDetailError(turnId, item.id)}</p>
            {/if}
            <button
              class="mt-3 inline-flex items-center gap-1.5 rounded-lg border border-sky-200 bg-sky-50 px-3 py-1.5 text-[10px] font-bold text-sky-700 transition-colors hover:bg-sky-100 disabled:opacity-60"
              disabled={isItemDetailLoading(turnId, item.id)}
              onclick={() => void loadItemDetail(turnId, item.id)}
              type="button"
            >
              {#if isItemDetailLoading(turnId, item.id)}
                <RefreshCw size={12} class="animate-spin" />
              {:else}
                <ChevronDown size={12} />
              {/if}
              <span>{m.load_details()}</span>
            </button>
          </div>
        {:else}
          <div class="rounded-xl border border-dashed border-sky-200 bg-white px-4 py-6 text-center text-xs text-gray-500">
            {ui.noAdditionalOutput}
          </div>
        {/if}
        {#if savedPath}
          <p class="truncate px-1 text-[11px] text-gray-500">{savedPath}</p>
        {/if}
      </div>
    </div>
  {:else if item.type === "imageView"}
    {@const imagePath = getImageViewPath(item)}
    <div class="turn-card-shell overflow-hidden rounded-2xl border border-sky-100 bg-white shadow-sm">
      <div class="turn-card-header turn-card-header--neutral flex items-center justify-between gap-3 border-b border-sky-100 px-4 py-3" data-sticky-level={stickyLevel}>
        <div class="flex min-w-0 items-center gap-3">
          <div class="flex h-8 w-8 shrink-0 items-center justify-center rounded-xl border border-sky-100 bg-sky-50 text-sky-700">
            <Layout size={15} />
          </div>
          <div class="min-w-0">
            <h4 class="truncate text-[10px] font-bold uppercase tracking-widest text-sky-700">{getToolItemLabel(item)}</h4>
            {#if imagePath}
              <p class="mt-1 truncate text-xs text-gray-500">{imagePath}</p>
            {/if}
          </div>
        </div>
        {#if imagePath}
          <button
            class="inline-flex shrink-0 items-center gap-1.5 rounded-lg border border-gray-200 bg-white px-2.5 py-1.5 text-[10px] font-bold text-gray-700 transition-colors hover:bg-gray-50"
            onclick={() => openFileTab(imagePath)}
            type="button"
          >
            <FileText size={12} />
            <span>{m.open()}</span>
          </button>
        {/if}
      </div>
    </div>
  {:else if item.type === "enteredReviewMode" || item.type === "exitedReviewMode"}
    {@render renderReviewModeItem(item, stickyLevel)}
  {:else if item.type === "reasoning"}
    <div class="turn-card-shell overflow-hidden rounded-2xl border border-amber-100 bg-amber-50/40 shadow-sm">
      <div class="turn-card-header turn-card-header--amber flex items-start gap-3 border-b border-amber-100 px-4 py-3" data-sticky-level={stickyLevel}>
        <div class="mt-0.5 flex h-8 w-8 flex-shrink-0 items-center justify-center rounded-lg border border-amber-100 bg-white text-amber-600">
          <Zap size={16} />
        </div>
        <div class="min-w-0">
          <h4 class="text-[10px] font-bold uppercase tracking-widest leading-none text-amber-700">{m.reasoning()}</h4>
          {#if Array.isArray(item.summary) && item.summary.length > 0}
            <p class="mt-1 text-xs leading-relaxed text-amber-700/80 break-words">
              {m.steps_count({ count: String(item.summary.length) })}
            </p>
          {/if}
        </div>
      </div>
      <div class="space-y-3 bg-white/85 px-4 py-3">
        {#if String(item.text ?? "").trim()}
          <div class="rounded-xl border border-amber-100 bg-white px-3 py-3">
            <MarkdownMessage compact expandLabel={ui.showFullMessage} maxInitialChars={compactMarkdownInitialChars} on:openLocalPath={(event: CustomEvent<{ href: string }>) => openFileFromMessage(event.detail.href)} text={String(item.text ?? "")} />
          </div>
        {/if}
        {#if Array.isArray(item.summary) && item.summary.length > 0}
          <div class="space-y-2">
            {#each item.summary as summaryEntry, index (`${item.id}:summary:${index}`)}
              <div class="rounded-xl border border-amber-100/70 bg-amber-50/60 px-3 py-2.5 text-sm leading-relaxed text-gray-700">
                <MarkdownMessage compact expandLabel={ui.showFullMessage} maxInitialChars={compactMarkdownInitialChars} on:openLocalPath={(event: CustomEvent<{ href: string }>) => openFileFromMessage(event.detail.href)} text={summaryEntry} />
              </div>
            {/each}
          </div>
        {/if}
      </div>
    </div>
  {:else if item.type === "plan"}
    <div class="turn-card-shell w-full min-w-0 overflow-hidden rounded-2xl border border-amber-100 bg-amber-50/25 shadow-sm">
      <div class="turn-card-header turn-card-header--amber flex items-center gap-3 border-b border-amber-100 px-4 py-2.5" data-sticky-level={stickyLevel}>
        <ListTodo size={14} class="text-amber-700" />
        <span class="text-[10px] font-bold uppercase tracking-widest text-amber-700">{ui.plannedStrategy}</span>
      </div>
      <div class="bg-white/80 px-4 py-3 text-gray-700">
        <MarkdownMessage compact expandLabel={ui.showFullMessage} maxInitialChars={compactMarkdownInitialChars} on:openLocalPath={(event: CustomEvent<{ href: string }>) => openFileFromMessage(event.detail.href)} text={String(item.text ?? "")} />
      </div>
    </div>
  {:else if item.type === "contextCompaction"}
    {@const contextCompressionRunning = isContextCompactionRunning(turnId, item)}
    <div class="turn-card-shell overflow-hidden rounded-2xl border border-amber-200 bg-gradient-to-br from-amber-50/80 via-white to-white shadow-sm">
      <div class="turn-card-header turn-card-header--amber flex items-center justify-between gap-3 border-b border-amber-100 px-4 py-3" data-sticky-level={stickyLevel}>
        <div class="flex min-w-0 items-center gap-3">
          <div class={`flex h-8 w-8 flex-shrink-0 items-center justify-center rounded-xl border ${contextCompressionRunning ? "border-amber-200 bg-amber-100 text-amber-700" : "border-emerald-200 bg-emerald-50 text-emerald-700"}`}>
            {#if contextCompressionRunning}
              <RefreshCw size={15} class="animate-spin" />
            {:else}
              <CheckCircle2 size={15} />
            {/if}
          </div>
          <div class="min-w-0">
            <h4 class="text-[10px] font-bold uppercase tracking-widest text-amber-700">{ui.contextCompression}</h4>
            <p class="mt-1 text-xs leading-relaxed text-gray-600">
              {contextCompressionRunning ? ui.contextCompressionInProgress : ui.contextCompressionCompleted}
            </p>
          </div>
        </div>
        <span class={`rounded-full px-2 py-1 text-[10px] font-bold uppercase tracking-widest ${contextCompressionRunning ? "bg-amber-100 text-amber-700" : "bg-emerald-50 text-emerald-700"}`}>
          {contextCompressionRunning ? ui.executing : m.done()}
        </span>
      </div>
    </div>
  {:else if ["commandExecution", "fileChange", "mcpToolCall", "dynamicToolCall", "webSearch"].includes(item.type)}
    <div class="turn-card-shell border border-gray-200 rounded-2xl bg-white overflow-hidden shadow-sm hover:shadow-md transition-shadow">
      <button class="turn-card-header turn-card-header--neutral flex w-full items-center justify-between gap-2.5 px-3 py-2.25 hover:bg-gray-50 transition-colors" data-sticky-level={stickyLevel} onclick={() => void toggleToolItem(turnId, item.id)}>
        <div class="flex min-w-0 flex-1 items-center gap-2.5">
          <div class="shrink-0 rounded-lg bg-gray-100 p-1.5 text-gray-500">
            {#if item.type === 'commandExecution'}
              <Terminal size={14} />
            {:else if item.type === 'fileChange'}
              <FileDiff size={14} />
            {:else if item.type === 'webSearch'}
              <Layout size={14} />
            {:else}
              <Zap size={14} />
            {/if}
          </div>
          <div class="min-w-0 flex-1 text-left">
            <h4 class="truncate text-[11px] font-bold leading-tight text-gray-900">{getToolItemLabel(item)}</h4>
            <p class="mt-0.5 truncate text-[10px] font-medium text-gray-500">{getToolItemSummary(item) || ui.executing}</p>
          </div>
        </div>
        <div class="flex shrink-0 items-center gap-2">
          {#if item.type === "commandExecution" && item.exitCode !== null}
            <span class="px-1.5 py-0.5 bg-gray-100 text-[9px] font-bold text-gray-500 rounded uppercase tracking-tighter">{m.exit_label()} {item.exitCode}</span>
          {/if}
          <ChevronDown size={14} class="text-gray-400 {isItemExpanded(turnId, item.id) ? 'rotate-180' : ''} transition-transform" />
        </div>
      </button>
      {#if isItemExpanded(turnId, item.id)}
        <div class="turn-card-expand p-0 border-t border-gray-100" transition:slide|local={{ duration: 220 }}>
          {#if isItemDetailLoading(turnId, item.id)}
            {#if item.type === "fileChange"}
              {@render renderDiffLoadingCard(ui.computingDiffs)}
            {:else}
              <div class="flex items-center justify-center gap-2 p-6 text-xs italic text-gray-400"><RefreshCw size={15} class="animate-spin" />{ui.fetching}</div>
            {/if}
          {:else if getItemDetailError(turnId, item.id)}<div class="p-4 bg-red-50 text-red-600 text-xs border-t border-red-100">{getItemDetailError(turnId, item.id)}</div>
          {:else if item.type === "fileChange" && getFileChangeViews(item).length > 0}
            <div class="p-0 space-y-0">{#each getFileChangeViews(item) as change}<div class="border-b border-gray-100 last:border-0"><button class="turn-card-header turn-card-header--neutral w-full flex items-center justify-between px-4 py-1.75 hover:bg-gray-50 transition-colors" data-sticky-level={stickyLevel + 1} onclick={() => toggleFileChangeEntry(turnId, item.id, change)}><div class="flex items-center gap-2"><span class="text-[10px] font-mono font-bold text-gray-600">{change.path}</span><span class="px-1.5 py-0.5 bg-gray-100 text-[9px] font-bold text-gray-400 rounded uppercase tracking-tighter">{change.kind}</span></div><ChevronDown size={12} class="text-gray-300 {isFileChangeEntryExpanded(turnId, item.id, change) ? 'rotate-180' : ''} transition-transform" /></button>{#if isFileChangeEntryExpanded(turnId, item.id, change)}<div class="turn-card-expand bg-gray-50 p-0 border-t border-gray-100" transition:slide|local={{ duration: 180 }}>{#if change.renderable}<LazyMonacoDiffEditor fallbackText={change.diff} height={400} modified={change.modified} original={change.original} path={change.path} />{:else}<pre class="p-3 text-[10px] font-mono text-gray-600 overflow-x-auto">{change.diff}</pre>{/if}</div>{/if}</div>{/each}</div>
          {:else if item.type === "dynamicToolCall" && (getDynamicToolTextItems(item).length > 0 || getDynamicToolImageUrls(item).length > 0)}
            <div class="space-y-3 p-3">
              {#each getDynamicToolTextItems(item) as text, index (`${item.id}:text:${index}`)}
                <pre class="overflow-x-auto whitespace-pre-wrap break-words rounded-xl border border-gray-100 bg-gray-50 p-3 text-[11px] leading-relaxed text-gray-700">{text}</pre>
              {/each}
              {#each getDynamicToolImageUrls(item) as imageUrl, index (`${item.id}:image:${index}`)}
                <figure class="overflow-hidden rounded-xl border border-gray-100 bg-gray-50">
                  <img alt={`${getToolItemLabel(item)} ${index + 1}`} class="block h-auto max-h-[34rem] w-full object-contain" loading="lazy" src={imageUrl} />
                </figure>
              {/each}
            </div>
          {:else if item.type === "webSearch"}
            {@render renderWebSearchDetails(item)}
          {:else if getDeferredToolBody(item)}{@render renderCappedOutput(getDeferredToolBody(item), getOutputPreviewKey(turnId, item.id))}
          {:else}<div class="p-4 text-gray-400 text-xs italic text-center">{ui.noAdditionalOutput}</div>{/if}
        </div>
      {/if}
    </div>
  {:else if item.type === "collabAgentToolCall"}
    <div class="turn-card-shell border border-amber-200 rounded-2xl bg-white overflow-hidden shadow-sm group">
      <div class="turn-card-header turn-card-header--amber flex items-center justify-between gap-3 px-3 py-2.25" data-sticky-level={stickyLevel}>
        <div class="flex items-center gap-2.5"><div class="rounded-lg border border-amber-200 bg-white p-1.5 text-amber-600 shadow-sm transition-all group-hover:bg-amber-600 group-hover:text-white"><Bot size={15} /></div><div><h4 class="text-[11px] font-bold leading-tight text-gray-900">{ui.subagentInvocation}</h4><div class="mt-0.5 flex items-center gap-1.5"><span class="text-[10px] font-bold uppercase tracking-widest text-amber-600">{item.tool}</span><span class="h-1 w-1 rounded-full bg-amber-200"></span><span class="text-[10px] font-medium uppercase tracking-tighter text-gray-500">{item.status}</span></div></div></div>
        {#if getPrimarySubagentThreadId(item)}<button class="rounded-md border border-amber-200 bg-white px-2.5 py-1 text-[10px] font-bold text-amber-700 shadow-sm transition-all hover:bg-amber-600 hover:text-white" onclick={() => void openSubagentThread(getPrimarySubagentThreadId(item) ?? "")}>{ui.viewThread}</button>{/if}
      </div>
      {#if item.prompt}<div class="border-t border-amber-100 bg-white p-3"><p class="mb-1.5 text-[10px] font-bold uppercase tracking-widest text-gray-400">{ui.instructions}</p><pre class="text-[11px] font-mono italic leading-relaxed text-gray-600 whitespace-pre-wrap">{String(item.prompt)}</pre></div>{/if}
    </div>
  {/if}
{/snippet}

{#snippet renderTurnEntry(turnId: string, entry: RenderableTurnEntry, stickyLevel = 0)}
  {#if entry.kind === "item"}{@render renderTurnItem(turnId, entry.item, stickyLevel)}
  {:else if entry.kind === "readGroup"}
    <div class="turn-card-shell border border-gray-200 rounded-2xl bg-white overflow-hidden shadow-sm">
      <button class="turn-card-header turn-card-header--neutral flex w-full items-center justify-between gap-2.5 px-3 py-2.25 hover:bg-gray-50 transition-colors" data-sticky-level={stickyLevel} onclick={() => void toggleReadOnlyCommandGroup(turnId, entry.key, entry.items)}><div class="flex min-w-0 flex-1 items-center gap-2.5"><div class="shrink-0 rounded-lg bg-gray-100 p-1.5 text-gray-400">{#if getReadOnlyCommandGroupKind(entry.items) === "git"}<GitBranch size={14} />{:else}<Search size={14} />{/if}</div><div class="min-w-0 flex-1 text-left"><h4 class="truncate text-[11px] font-bold leading-tight text-gray-900">{getReadOnlyCommandGroupLabel(entry.items)}</h4><p class="mt-0.5 truncate text-[10px] font-medium text-gray-500">{summarizeReadOnlyCommandGroup(entry.items)}</p></div></div><div class="flex shrink-0 items-center gap-2"><span class="px-1.5 py-0.5 bg-gray-50 text-[9px] font-bold text-gray-400 rounded uppercase tracking-tighter">{ui.opsCount(entry.items.length)}</span><ChevronDown size={14} class="text-gray-400 {isItemExpanded(turnId, entry.key) ? 'rotate-180' : ''} transition-transform" /></div></button>
      {#if isItemExpanded(turnId, entry.key)}
        <div class="turn-card-expand border-t border-gray-100 bg-gray-50/30" transition:slide|local={{ duration: 220 }}>{#if isItemDetailLoading(turnId, entry.key)}<div class="flex justify-center p-5 text-xs italic text-gray-400 animate-pulse">{ui.readingFileData}</div>
          {:else}<div class="p-0">{#each entry.items as commandItem}<div class="border-b border-gray-100 p-3 last:border-0"><div class="mb-1.5 flex items-center justify-between"><span class="text-[10px] font-bold uppercase tracking-widest text-gray-500">{summarizeCommand(commandItem)}</span>{#if commandItem.exitCode !== null}<span class="text-[9px] font-mono text-gray-400">{m.exit_label()} {commandItem.exitCode}</span>{/if}</div>{@render renderCappedOutput(String(commandItem.aggregatedOutput ?? ""), getOutputPreviewKey(turnId, `${entry.key}:${commandItem.id}`), true)}</div>{/each}</div>{/if}</div>
      {/if}
    </div>
  {:else}
    <div class="turn-card-shell border border-gray-200 rounded-2xl bg-white overflow-hidden shadow-sm">
      <div class="turn-card-header turn-card-header--neutral flex items-center gap-2 pr-3" data-sticky-level={stickyLevel}>
        <button class="min-w-0 flex-1 flex items-center justify-between px-3 py-2.25 hover:bg-gray-50 transition-colors" onclick={() => void toggleFileChangeGroup(turnId, entry.key, entry.items)} type="button"><div class="flex min-w-0 items-center gap-2.5"><div class="shrink-0 rounded-lg bg-gray-100 p-1.5 text-gray-400"><FileDiff size={14} /></div><div class="min-w-0 text-left"><h4 class="text-[11px] font-bold leading-tight text-gray-900">{getFileChangeGroupLabel(entry.items)}</h4><p class="mt-0.5 truncate text-[10px] font-medium text-gray-500">{summarizeFileChangeGroup(entry.items)}</p></div></div><div class="flex shrink-0 items-center gap-2"><span class="px-1.5 py-0.5 bg-gray-50 text-[9px] font-bold text-gray-400 rounded uppercase tracking-tighter">{getFileChangeGroupSummaryEntries(entry.items).length} {m.files_count_label()}</span><ChevronDown size={14} class="text-gray-400 {isItemExpanded(turnId, entry.key) ? 'rotate-180' : ''} transition-transform" /></div></button>
        {#if getFileChangeGroupViews(entry.items).length > 0}
          <button
            class="shrink-0 rounded-md border border-gray-200 bg-white px-2.5 py-1 text-[10px] font-bold text-gray-700 transition-colors hover:bg-gray-50"
            onclick={(event) => {
              event.stopPropagation();
              openFileChangeGroupInTab(getFileChangeGroupViews(entry.items), `group:${turnId}:${entry.key}`, getFileChangeGroupLabel(entry.items));
            }}
            type="button"
          >
            {ui.openInNewTab}
          </button>
        {/if}
      </div>
      {#if isItemExpanded(turnId, entry.key)}
        <div class="turn-card-expand border-t border-gray-100" transition:slide|local={{ duration: 220 }}>{#if isItemDetailLoading(turnId, entry.key)}{@render renderDiffLoadingCard(ui.computingDiffs)}
          {:else}<div class="p-0">{#each getFileChangeGroupViews(entry.items) as change}<div class="border-b border-gray-100 last:border-0"><button class="turn-card-header turn-card-header--neutral w-full flex items-center justify-between px-5 py-2 hover:bg-gray-50 transition-colors" data-sticky-level={stickyLevel + 1} onclick={() => toggleFileChangeEntry(turnId, entry.key, change)}><span class="text-[10px] font-mono font-bold text-gray-600">{change.path}</span><ChevronDown size={12} class="text-gray-300 {isFileChangeEntryExpanded(turnId, entry.key, change) ? 'rotate-180' : ''} transition-transform" /></button>{#if isFileChangeEntryExpanded(turnId, entry.key, change)}<div class="turn-card-expand bg-gray-50 border-t border-gray-100" transition:slide|local={{ duration: 180 }}>{#if change.renderable}<LazyMonacoDiffEditor fallbackText={change.diff} height={400} modified={change.modified} original={change.original} path={change.path} />{:else}<pre class="p-4 text-[10px] font-mono text-gray-600 overflow-x-auto">{change.diff}</pre>{/if}</div>{/if}</div>{/each}</div>{/if}</div>
      {/if}
    </div>
  {/if}
{/snippet}
