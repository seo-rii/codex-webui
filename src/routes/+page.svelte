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
    RefreshCw,
    Search,
    Copy
  } from "lucide-svelte";
  import { onMount, tick } from "svelte";
  import { extractAttachmentPaths, stripAttachmentPreamble } from "$lib/attachments";
  import { api } from "$lib/api";
  import { applyStreamEvent, createConversationState, type ConversationState } from "$lib/chat-state";
  import GitWorkspace from "$lib/components/GitWorkspace.svelte";
  import MarkdownMessage from "$lib/components/MarkdownMessage.svelte";
  import MonacoDiffEditor from "$lib/components/MonacoDiffEditor.svelte";
  import SessionSidebar from "$lib/components/SessionSidebar.svelte";
  import SettingsWorkspace from "$lib/components/SettingsWorkspace.svelte";
  import TerminalWorkspace from "$lib/components/TerminalWorkspace.svelte";
  import { activeLocale, localeOptions, localeSignal, updateLocale } from "$lib/i18n";
  import { m } from "$lib/paraglide/messages.js";
  import {
    applyThemeMode,
    getResolvedTheme,
    readThemeMode,
    subscribeThemeChange,
    type ResolvedTheme,
    type ThemeMode
  } from "$lib/theme";
  import type {
    AppConfigPayload,
    AttachmentRecord,
    CodexAccountLoginFlow,
    CodexItem,
    CodexQuotaStatus,
    CodexRuntimeStatus,
    DirectoryPayload,
    GitCommit,
    GitOpenRequest,
    GlobalStreamEvent,
    PendingServerRequest,
    SessionListPayload,
    SessionPreferences,
    SessionQueueItem,
    SessionSearchScope,
    SessionSummary,
    StreamEvent,
    TerminalSummary,
    WsConnectionState
  } from "$lib/types";

  type WorkspaceTabId = "chat" | "tasks" | "git" | "settings" | `git-diff:${string}` | `code-diff:${string}` | `terminal:${string}`;
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
    prompt: string;
    attachmentNames: string[];
    createdAt: number;
    baselineTurnId: string | null;
    baselineTurnCount: number;
  };

  let config = $state<AppConfigPayload | null>(null);
  let quota = $state<CodexQuotaStatus | null>(null);
  let runtime = $state<CodexRuntimeStatus | null>(null);
  let sessions = $state<SessionSummary[]>([]);
  let sessionsCursor = $state<string | null>(null);
  let sessionsHasMore = $state(false);
  let sessionsLoadingMore = $state(false);
  let conversation = $state<ConversationState | null>(null);
  let selectedSessionId = $state<string | null>(null);
  let authenticated = $state<boolean | null>(null);
  let loading = $state(true);
  let loadingDetail = $state(false);
  let sessionsBusy = $state(false);
  let sending = $state(false);
  let startingMessage = $state(false);
  let uploading = $state(false);
  let errorText = $state("");
  let noticeText = $state("");
  let loginPassword = $state("");
  let loginBusy = $state(false);
  let loginMessage = $state("");
  let draft = $state("");
  let draftAttachments = $state<AttachmentRecord[]>([]);
  let titleDraft = $state("");
  let browserOpen = $state(false);
  let browserBusy = $state(false);
  let runtimeBusyAction = $state<"install" | "update" | "check" | null>(null);
  let quotaBusy = $state(false);
  let directoryPayload = $state<DirectoryPayload | null>(null);
  let requestAnswers = $state<Record<string, Record<string, string>>>({});
  let rawRequestResponses = $state<Record<string, string>>({});
  let pendingSessionEvents = $state<Record<string, StreamEvent[]>>({});
  let expandedItems = $state<Record<string, boolean>>({});
  let expandedFileChangeEntries = $state<Record<string, boolean>>({});
  let loadingItemDetails = $state<Record<string, boolean>>({});
  let itemDetailErrors = $state<Record<string, string>>({});
  let expandedTurnLogs = $state<Record<string, boolean>>({});
  let loadingTurns = $state<Record<string, boolean>>({});
  let turnLoadErrors = $state<Record<string, string>>({});
  let sessionSearchQuery = $state("");
  let sessionSearchScope = $state<SessionSearchScope>("summary");
  let showArchivedSessions = $state(false);
  let accountLoginFlow = $state<CodexAccountLoginFlow | null>(null);
  let composerSettingsOpen = $state(false);
  let composerSecurityOpen = $state(false);
  let connectionState = $state<WsConnectionState>("idle");
  let themeMode = $state<ThemeMode>("system");
  let resolvedTheme = $state<ResolvedTheme>("light");
  let loadingOlderTurns = $state(false);
  let olderTurnsAutoLoadEnabled = $state(true);
  let olderTurnsAutoLoadPaused = $state(false);
  let olderTurnsAutoTriggerTimestamps = $state<number[]>([]);
  let terminals = $state<TerminalSummary[]>([]);
  let activeWorkspaceTabId = $state<WorkspaceTabId>("chat");
  let workspaceMenuOpen = $state(false);
  let gitTabOpen = $state(false);
  let settingsTabOpen = $state(false);
  let gitDiffTabs = $state<GitDiffTab[]>([]);
  let codeDiffTabs = $state<CodeDiffTab[]>([]);
  let pendingSteerResume = $state<{ sessionId: string; draft: string; updatedAt: number | null } | null>(null);
  let dismissedQueueResumeBySessionId = $state<Record<string, boolean>>({});
  let draftSaveTimer: ReturnType<typeof setTimeout> | null = null;
  let draftPersistencePaused = $state(false);
  let mobileSidebarOpen = $state(false);
  let isMobileLayout = $state(false);
  let optimisticMessage = $state<OptimisticMessageState | null>(null);
  let pendingQueueModeSessionId = $state<string | null>(null);
  let liveTurnCardExpanded = $state(false);
  let sendIntent = $state<"message" | "steer" | "queue" | null>(null);
  let editingQueueId = $state<string | null>(null);
  let editingQueuePrompt = $state("");
  let queuedFollowupsExpanded = $state(true);
  let sessionHighlights = $state<Record<string, { kind: "completed" | "attention"; at: number }>>({});
  let startupAlertModalOpen = $state(false);
  let startupAlertDismissed = $state(false);
  let startupAlertNow = $state(Date.now());

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

  const rawRequestPlaceholder = '{"action":"decline","content":null}';
  const readOnlyParsedCommandTypes = new Set(["read", "list_files", "search"]);
  const scrollBottomThreshold = 48;
  const transcriptTopLoadThreshold = 96;
  const sessionPageSize = 20;
  const olderTurnPageSize = 20;
  const olderTurnAutoLoadWindowMs = 1500;
  const olderTurnAutoLoadBurstLimit = 3;
  const sessionQueryParamKey = "session";
  const notificationPromptStorageKey = "codex-webui.notifications.permission-prompted";

  const ui = $derived.by(() => {
    const _locale = $localeSignal;

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
      newTerminal: m.new_terminal(),
      threadTitle: m.thread_title(),
      restoreThread: m.restore_thread(),
      archiveThread: m.archive_thread(),
      open: m.open(),
      loadingHistory: m.loading_history(),
      historyAvailable: m.history_available(),
      autoLoadPaused: m.auto_load_paused(),
      resumeAutoLoad: m.resume_auto_load(),
      loadOlderTurns: m.load_older_turns(),
      copyMessage: m.copy_message(),
      editInComposer: m.edit_in_composer(),
      branchIntoNewThread: m.branch_into_new_thread(),
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
      followUp: m.follow_up(),
      edit: m.edit(),
      save: m.save_file(),
      cancel: m.close(),
      steerNow: m.steer_now(),
      sendNow: m.send_now(),
      liveTurn: m.live_turn(),
      composerSettings: m.composer_settings(),
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
      autoDefault: m.auto_default(),
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
      stopped: m.stopped()
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

    let candidate =
      normalized.split(/\r?\n/u, 1)[0]?.split(/(?<=[.?!])\s+/u, 1)[0]?.split(/\s[-:|]\s/u, 1)[0]?.trim() ?? normalized;

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
      candidate = normalized;
    }

    return candidate.length > 60 ? `${candidate.slice(0, 60).trimEnd()}...` : candidate;
  }

  function getDisplayThreadTitle(name: string | null | undefined, preview: string | null | undefined) {
    if (!isPlaceholderThreadTitle(name)) {
      return formatValue(name);
    }
    return inferDisplayThreadTitle(formatValue(preview));
  }

  function createDraftConversation(preferences: SessionPreferences, title: string | null = null): ConversationState {
    const now = Math.floor(Date.now() / 1000);
    return {
      thread: {
        id: "",
        preview: "",
        name: title,
        cwd: preferences.cwd,
        status: "idle",
        createdAt: now,
        updatedAt: now,
        isSubagent: false,
        agentNickname: null,
        agentRole: null,
        turns: []
      },
      preferences: {
        ...preferences
      },
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
        message: null
      },
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
    clearHydrationRefresh();
    disconnectStream();
    selectedSessionId = null;
    conversation = createDraftConversation(preferences, options.title ?? null);
    draftPersistencePaused = false;
    pendingSessionEvents = {};
    expandedItems = {};
    expandedFileChangeEntries = {};
    loadingItemDetails = {};
    itemDetailErrors = {};
    expandedTurnLogs = {};
    loadingTurns = {};
    turnLoadErrors = {};
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
    composerSecurityOpen = false;
    activeWorkspaceTabId = "chat";
    syncSelectedSessionInUrl(null);
    queueMicrotask(() => {
      scheduleComposerTextareaResize();
    });
  }

  async function ensureSessionForComposer() {
    if (selectedSessionId && conversation) {
      return {
        sessionId: selectedSessionId,
        state: conversation
      };
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
    const nextTitle = draftTitleSnapshot && !isPlaceholderThreadTitle(draftTitleSnapshot) ? draftTitleSnapshot : null;

    const created = await api.createSession(draftState.preferences, nextTitle);
    upsertSessionSummary(created);
    const restored = await selectSession(created.id);
    if (!restored || !conversation || selectedSessionId !== created.id) {
      activateDraftSession(draftState.preferences, {
        draftText: draftTextSnapshot,
        draftAttachments: draftAttachmentSnapshot,
        title: draftTitleSnapshot
      });
      throw new Error("Failed to open the created session.");
    }

    draft = draftTextSnapshot;
    draftAttachments = draftAttachmentSnapshot;
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
    if (!hasStartupAlerts(nextConfig)) {
      startupAlertModalOpen = false;
      startupAlertDismissed = false;
      return;
    }

    if (forceOpen || !startupAlertDismissed) {
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
  let releaseConnectionStateListener: (() => void) | null = null;
  let releaseThemeListener: (() => void) | null = null;
  let saveTimer: ReturnType<typeof setTimeout> | null = null;
  let hydrationRefreshTimer: ReturnType<typeof setTimeout> | null = null;
  let sessionRefreshTimer: ReturnType<typeof setTimeout> | null = null;
  let sessionListRequestVersion = 0;
  const itemDetailRefreshTimers = new Map<string, ReturnType<typeof setTimeout>>();
  let transcriptElement = $state<HTMLDivElement | undefined>(undefined);
  let transcriptContentElement = $state<HTMLDivElement | undefined>(undefined);
  let stickTranscriptToBottom = $state(true);
  let forceTranscriptScroll = $state(false);
  let composerSettingsTriggerElement = $state<HTMLButtonElement | undefined>(undefined);
  let composerSettingsPopoverElement = $state<HTMLDivElement | undefined>(undefined);
  let composerSettingsPopoverStyle = $state("");
  let composerSecurityTriggerElement = $state<HTMLButtonElement | undefined>(undefined);
  let composerSecurityPopoverElement = $state<HTMLDivElement | undefined>(undefined);
  let composerSecurityPopoverStyle = $state("");
  let titleInputElement = $state<HTMLInputElement | undefined>(undefined);
  let composerTextareaElement = $state<HTMLTextAreaElement | undefined>(undefined);
  let filePickerElement = $state<HTMLInputElement | undefined>(undefined);
  let fakeTopLoadPercent = $state(0);
  let transcriptScrollFrame: number | null = null;
  let composerTextareaResizeFrame: number | null = null;
  let transcriptResizeObserver: ResizeObserver | null = null;
  let transcriptScrollGeneration = 0;
  let composerHistory = $state<string[]>([]);
  let composerHistoryIndex = $state(-1);
  let composerHistoryDraft = $state("");
  let notificationPermissionRequested = false;
  const handledResumeDraftKeys = new Set<string>();
  let lastLoadedConversationId = $state<string | null>(null);
  let lastActiveLiveTurnId = $state<string | null>(null);

  function canQueueComposerMessage(currentConversation: ConversationState | null = conversation) {
    if (!currentConversation || !selectedSessionId) {
      return false;
    }
    if (pendingQueueModeSessionId === selectedSessionId) {
      return true;
    }
    if (currentConversation.thread.status === "running" || currentConversation.thread.status === "active") {
      return true;
    }
    return currentConversation.thread.turns.some((turn) => String(turn.status ?? "") === "inProgress");
  }

  const running = $derived.by(() => {
    const currentConversation = conversation;
    if (!currentConversation?.activeTurnId) {
      return false;
    }
    if (currentConversation.thread.status === "running" || currentConversation.thread.status === "active") {
      return true;
    }
    return currentConversation.thread.turns.some(
      (turn) => turn.id === currentConversation.activeTurnId && String(turn.status ?? "") === "inProgress"
    );
  });
  const queueModeActive = $derived.by(() => canQueueComposerMessage());
  const selectedSessionSummary = $derived(sessions.find((session) => session.id === selectedSessionId) ?? null);
  const queuedMessages = $derived(conversation?.queue.items ?? []);
  const activeTurn = $derived.by(() => {
    const turnId = conversation?.activeTurnId;
    if (!turnId || !conversation) {
      return null;
    }
    return conversation.thread.turns.find((turn) => turn.id === turnId) ?? null;
  });
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
  const activeLiveTurnSummary = $derived.by(() => {
    const explanation = activeLiveTurnPlan?.explanation?.trim();
    if (explanation) {
      return explanation;
    }

    if (activeLiveTurnSubagents.length > 0) {
      const prompt = activeLiveTurnSubagents[0]?.prompt?.trim();
      if (prompt) {
        return prompt;
      }

      const tool = activeLiveTurnSubagents[0]?.tool?.trim();
      if (tool) {
        return tool;
      }
    }

    return null;
  });
  const visibleOptimisticMessage = $derived.by(() => {
    if (!optimisticMessage || optimisticMessage.sessionId !== selectedSessionId) {
      return null;
    }
    if (!conversation || conversation.thread.id !== optimisticMessage.sessionId) {
      return optimisticMessage;
    }
    return hasConversationEchoedOptimisticMessage(conversation, optimisticMessage) ? null : optimisticMessage;
  });
  const showQueueResumeBanner = $derived.by(
    () =>
      Boolean(
        selectedSessionId &&
          conversation?.queue.resumeRequired &&
          !dismissedQueueResumeBySessionId[selectedSessionId]
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
  const sessionHydrationState = $derived(conversation?.hydration.state ?? "idle");
  const sessionHydrationLoadedTurns = $derived(conversation?.hydration.loadedTurns ?? 0);
  const sessionHydrationTotalTurns = $derived(conversation?.hydration.totalTurns ?? null);
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
    if (loadingDetail && !conversation) {
      return "sessionDetail";
    }
    if (sessionHydrationState === "loading") {
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
    if (loadingDetail && !conversation) {
      return ui.loadingSessionBasics;
    }
    if (sessionHydrationState === "loading") {
      return sessionHydrationTotalTurns
        ? `${ui.loadingConversationProgressive} (${sessionHydrationLoadedTurns}/${sessionHydrationTotalTurns})`
        : sessionHydrationLoadedTurns > 0
          ? `${ui.loadingConversationProgressive} (${sessionHydrationLoadedTurns})`
          : ui.loadingConversationProgressive;
    }
    return "";
  });
  const showTopLoadBar = $derived(Boolean(topLoadLabel));
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
        text: errorText
      };
    }
    if (noticeText) {
      return {
        tone: "success" as const,
        text: noticeText
      };
    }
    return null;
  });
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
  const workspaceTabs = $derived.by(() => {
    const _locale = $localeSignal;
    const tabs: Array<{ id: WorkspaceTabId; label: string; kind: "chat" | "tasks" | "git" | "settings" | "git-diff" | "code-diff" | "terminal" }> = [
      { id: "chat", label: ui.chat, kind: "chat" }
    ];
    if (subagentTasks.length > 0) {
      tabs.push({
        id: "tasks",
        label: `${ui.tasks} ${subagentTasks.length}`,
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

  function dismissFeedbackSnackbar() {
    errorText = "";
    noticeText = "";
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
    releaseConnectionStateListener = api.onConnectionState((state) => {
      connectionState = state;
    });
    const handleViewportChange = () => {
      if (composerSettingsOpen) {
        void updateComposerSettingsPopoverPosition();
      }
      if (composerSecurityOpen) {
        void updateComposerSecurityPopoverPosition();
      }
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
    mobileQuery.addEventListener("change", syncMobileLayout);
    window.addEventListener("resize", handleViewportChange);
    window.addEventListener("scroll", handleViewportChange, true);
    window.addEventListener("pointerdown", requestNotificationPermissionFromGesture, true);
    window.addEventListener("keydown", requestNotificationPermissionFromGesture, true);
    void bootstrap();

    return () => {
      disconnectStream();
      clearHydrationRefresh();
      if (sessionRefreshTimer) {
        clearTimeout(sessionRefreshTimer);
      }
      if (saveTimer) {
        clearTimeout(saveTimer);
      }
      if (draftSaveTimer) {
        clearTimeout(draftSaveTimer);
      }
      if (transcriptScrollFrame !== null) {
        cancelAnimationFrame(transcriptScrollFrame);
      }
      if (composerTextareaResizeFrame !== null) {
        cancelAnimationFrame(composerTextareaResizeFrame);
      }
      transcriptResizeObserver?.disconnect();
      transcriptResizeObserver = null;
      for (const timer of itemDetailRefreshTimers.values()) {
        clearTimeout(timer);
      }
      itemDetailRefreshTimers.clear();
      releaseGlobalStream?.();
      releaseGlobalStream = null;
      releaseReconnectListener?.();
      releaseReconnectListener = null;
      releaseConnectionStateListener?.();
      releaseConnectionStateListener = null;
      releaseThemeListener?.();
      releaseThemeListener = null;
      mobileQuery.removeEventListener("change", syncMobileLayout);
      window.removeEventListener("resize", handleViewportChange);
      window.removeEventListener("scroll", handleViewportChange, true);
      window.removeEventListener("pointerdown", requestNotificationPermissionFromGesture, true);
      window.removeEventListener("keydown", requestNotificationPermissionFromGesture, true);
      api.disconnect();
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
    const nextDisplayTitle = getDisplayThreadTitle(conversation?.thread.name, conversation?.thread.preview) ?? "";
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
    if (!conversation || !transcriptElement || (!stickTranscriptToBottom && !forceTranscriptScroll)) {
      return;
    }
    scheduleTranscriptScrollToBottom();
  });

  $effect(() => {
    if (!transcriptElement || !conversation) {
      return;
    }
    if (!inlineGenerationState && !loadingDetail && !sending) {
      return;
    }
    forceTranscriptScroll = true;
    scheduleTranscriptScrollToBottom();
  });

  $effect(() => {
    if (typeof window === "undefined" || !transcriptContentElement) {
      transcriptResizeObserver?.disconnect();
      transcriptResizeObserver = null;
      return;
    }

    transcriptResizeObserver?.disconnect();
    transcriptResizeObserver = new ResizeObserver(() => {
      if (!transcriptElement || loadingOlderTurns || (!stickTranscriptToBottom && !forceTranscriptScroll)) {
        return;
      }
      scheduleTranscriptScrollToBottom();
    });
    transcriptResizeObserver.observe(transcriptContentElement);

    return () => {
      transcriptResizeObserver?.disconnect();
      transcriptResizeObserver = null;
    };
  });

  $effect(() => {
    if (!composerSettingsOpen) {
      composerSettingsPopoverStyle = "";
      return;
    }
    void updateComposerSettingsPopoverPosition();
  });

  $effect(() => {
    if (!composerSecurityOpen) {
      composerSecurityPopoverStyle = "";
      return;
    }
    void updateComposerSecurityPopoverPosition();
  });

  $effect(() => {
    if (!optimisticMessage) {
      return;
    }
    if (selectedSessionId !== optimisticMessage.sessionId) {
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
    if (!pendingQueueModeSessionId) {
      return;
    }
    if (conversation?.thread.id !== pendingQueueModeSessionId) {
      return;
    }
    if (conversation.activeTurnId || conversation.thread.status === "running" || conversation.thread.status === "active") {
      pendingQueueModeSessionId = null;
      return;
    }
    if (!sending && !visibleOptimisticMessage) {
      pendingQueueModeSessionId = null;
    }
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
    if (activeWorkspaceTabId === "tasks" && subagentTasks.length === 0) {
      activeWorkspaceTabId = "chat";
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
      sessionDetail: 72,
      sessionHydration: 88
    };
    const starts: Record<string, number> = {
      olderTurns: 18,
      sessionsInitial: 14,
      sessionsRefresh: 28,
      sessionsMore: 42,
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

  function compareSessions(left: SessionSummary, right: SessionSummary) {
    const priorityDifference = getSessionSortPriority(right.status) - getSessionSortPriority(left.status);
    if (priorityDifference !== 0) {
      return priorityDifference;
    }

    const updatedDifference = (right.updatedAt || 0) - (left.updatedAt || 0);
    if (updatedDifference !== 0) {
      return updatedDifference;
    }

    const createdDifference = (right.createdAt || 0) - (left.createdAt || 0);
    if (createdDifference !== 0) {
      return createdDifference;
    }

    return 0;
  }

  function sortSessions(items: SessionSummary[]) {
    return [...items].sort(compareSessions);
  }

  function highlightSession(sessionId: string, kind: "completed" | "attention") {
    if (!sessionId || sessionId === selectedSessionId) {
      return;
    }
    sessionHighlights = {
      ...sessionHighlights,
      [sessionId]: {
        kind,
        at: Date.now()
      }
    };
  }

  function clearSessionHighlight(sessionId: string | null) {
    if (!sessionId || !sessionHighlights[sessionId]) {
      return;
    }
    const nextHighlights = { ...sessionHighlights };
    delete nextHighlights[sessionId];
    sessionHighlights = nextHighlights;
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

  function scheduleTranscriptScrollToBottom() {
    if (!transcriptElement) {
      return;
    }

    const scrollTranscript = (top: number) => {
      if (!transcriptElement) {
        return;
      }

      const previousBehavior = transcriptElement.style.scrollBehavior;
      transcriptElement.style.scrollBehavior = "auto";
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
        if (loadingOlderTurns || (!stickTranscriptToBottom && !forceTranscriptScroll)) {
          transcriptScrollFrame = null;
          forceTranscriptScroll = false;
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

        if (stableFrames >= 2 || window.performance.now() - startedAt >= 700) {
          transcriptScrollFrame = null;
          forceTranscriptScroll = false;
          return;
        }

        transcriptScrollFrame = window.requestAnimationFrame(step);
      };

      transcriptScrollFrame = window.requestAnimationFrame(step);
    });
  }

  function mergeSessionPage(payload: SessionListPayload, pinnedSession: SessionSummary | null, append = false) {
    const visiblePayloadSessions = payload.sessions.filter((session) => !isSubagentSessionSummary(session));
    const baseSessions = append ? sessions : [];
    const deduped = [...baseSessions, ...visiblePayloadSessions].filter(
      (session, index, collection) => collection.findIndex((candidate) => candidate.id === session.id) === index
    );

    if (shouldPinSession(pinnedSession) && pinnedSession && !deduped.some((session) => session.id === pinnedSession.id)) {
      deduped.unshift(pinnedSession);
    }

    sessions = sortSessions(deduped);
    sessionsCursor = payload.nextCursor;
    sessionsHasMore = Boolean(payload.nextCursor);
  }

  function shouldPinSession(session: SessionSummary | null) {
    return Boolean(session && !isSubagentSessionSummary(session) && !sessionSearchQuery.trim() && session.archived === showArchivedSessions);
  }

  function upsertSessionSummary(summary: SessionSummary) {
    if (isSubagentSessionSummary(summary)) {
      sessions = sortSessions(sessions.filter((session) => session.id !== summary.id));
      return;
    }
    sessions = sortSessions([summary, ...sessions.filter((session) => session.id !== summary.id)]);
  }

  function applySessionSummaryUpdate(summary: SessionSummary) {
    if (isSubagentSessionSummary(summary)) {
      const nextSessions = sessions.filter((session) => session.id !== summary.id);
      if (nextSessions.length !== sessions.length) {
        sessions = sortSessions(nextSessions);
      }
      return;
    }

    if (sessionSearchQuery.trim()) {
      scheduleSessionRefresh(60);
      return;
    }

    if (summary.archived !== showArchivedSessions) {
      const nextSessions = sessions.filter((session) => session.id !== summary.id);
      if (nextSessions.length !== sessions.length) {
        sessions = sortSessions(nextSessions);
      }
      return;
    }

    upsertSessionSummary(summary);
  }

  function buildSessionSummaryFromConversation(state: ConversationState): SessionSummary {
    const hasLiveTurn =
      Boolean(state.activeTurnId) ||
      state.thread.status === "running" ||
      state.thread.status === "active" ||
      state.thread.turns.some((turn) => String(turn.status ?? "") === "inProgress");

    return {
      id: state.thread.id,
      name: getDisplayThreadTitle(state.thread.name, state.thread.preview),
      preview: state.thread.preview,
      queueCount: state.queue.items.length,
      cwd: state.thread.cwd,
      archived: selectedSessionSummary?.archived ?? showArchivedSessions,
      createdAt: state.thread.createdAt,
      updatedAt: Math.max(state.thread.updatedAt, Math.floor(Date.now() / 1000)),
      status: hasLiveTurn ? "running" : state.thread.status,
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

  function getRequestedSessionIdFromUrl() {
    if (typeof window === "undefined") {
      return null;
    }
    const value = new URL(window.location.href).searchParams.get(sessionQueryParamKey)?.trim() ?? "";
    return value || null;
  }

  function syncSelectedSessionInUrl(sessionId: string | null) {
    if (typeof window === "undefined") {
      return;
    }

    const url = new URL(window.location.href);
    if (sessionId) {
      url.searchParams.set(sessionQueryParamKey, sessionId);
    } else {
      url.searchParams.delete(sessionQueryParamKey);
    }

    const nextUrl = `${url.pathname}${url.search}${url.hash}`;
    const currentUrl = `${window.location.pathname}${window.location.search}${window.location.hash}`;
    if (nextUrl !== currentUrl) {
      window.history.replaceState(window.history.state, "", nextUrl);
    }
  }

  function resetWorkspaceState() {
    disconnectStream();
    clearHydrationRefresh();

    if (sessionRefreshTimer) {
      clearTimeout(sessionRefreshTimer);
      sessionRefreshTimer = null;
    }
    if (saveTimer) {
      clearTimeout(saveTimer);
      saveTimer = null;
    }

    for (const timer of itemDetailRefreshTimers.values()) {
      clearTimeout(timer);
    }
    itemDetailRefreshTimers.clear();

    config = null;
    quota = null;
    sessions = [];
    sessionsCursor = null;
    sessionsHasMore = false;
    sessionsLoadingMore = false;
    conversation = null;
    selectedSessionId = null;
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
    loadingTurns = {};
    turnLoadErrors = {};
    sessionSearchQuery = "";
    sessionSearchScope = "summary";
    showArchivedSessions = false;
    accountLoginFlow = null;
    composerSettingsOpen = false;
    composerSecurityOpen = false;
    composerSettingsPopoverStyle = "";
    composerSecurityPopoverStyle = "";
    loadingOlderTurns = false;
    olderTurnsAutoLoadEnabled = true;
    olderTurnsAutoLoadPaused = false;
    olderTurnsAutoTriggerTimestamps = [];
    terminals = [];
    activeWorkspaceTabId = "chat";
    workspaceMenuOpen = false;
    gitTabOpen = false;
    settingsTabOpen = false;
    gitDiffTabs = [];
    codeDiffTabs = [];
    pendingSteerResume = null;
    dismissedQueueResumeBySessionId = {};
    draftPersistencePaused = false;
    mobileSidebarOpen = false;
    optimisticMessage = null;
    pendingQueueModeSessionId = null;
    liveTurnCardExpanded = false;
    sendIntent = null;
    editingQueueId = null;
    editingQueuePrompt = "";
    sessionHighlights = {};
    startupAlertModalOpen = false;
    startupAlertDismissed = false;
    syncSelectedSessionInUrl(null);
  }

  function clearWorkspaceForLoggedOut() {
    resetWorkspaceState();
    authenticated = false;
    runtime = null;
    loginBusy = false;
    releaseGlobalStream?.();
    releaseGlobalStream = null;
    api.disconnect();
    connectionState = "idle";
  }

  function ensureGlobalStreamSubscription() {
    if (releaseGlobalStream) {
      return;
    }

    releaseGlobalStream = api.subscribeGlobal((event) => {
      handleGlobalEvent(event);
    });
  }

  async function bootstrap() {
    loading = true;
    errorText = "";
    noticeText = "";

    try {
      const authSession = await api.getAuthSession();
      if (!authSession.authenticated) {
        clearWorkspaceForLoggedOut();
        loading = false;
        return;
      }

      authenticated = true;
      loginMessage = "";
      ensureGlobalStreamSubscription();
      runtime = await api.getRuntimeStatus();
      if (!runtime.installed) {
        resetWorkspaceState();
        loading = false;
        return;
      }

      const requestedSessionId = getRequestedSessionIdFromUrl();
      config = await api.getConfig();
      syncStartupAlertModal(config);
      await refreshSessions();
      await refreshTerminals();
      void refreshQuota(false);
      void refreshAccountState(false);
      loading = false;

      if (requestedSessionId) {
        const restored = await selectSession(requestedSessionId);
        if (restored) {
          return;
        }
        syncSelectedSessionInUrl(null);
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

    try {
      const response = sessionSearchQuery.trim()
        ? await api.searchSessions(sessionSearchQuery.trim(), sessionSearchScope, showArchivedSessions, null, sessionPageSize)
        : await api.getSessions(showArchivedSessions, null, sessionPageSize);

      if (requestVersion !== sessionListRequestVersion) {
        return;
      }

      mergeSessionPage(response, pinnedSession, false);
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

  async function loadMoreSessions() {
    if (!sessionsHasMore || sessionsLoadingMore) {
      return;
    }

    const requestVersion = sessionListRequestVersion;
    const cursor = sessionsCursor;
    if (!cursor) {
      return;
    }

    sessionsLoadingMore = true;
    try {
      const response = sessionSearchQuery.trim()
        ? await api.searchSessions(sessionSearchQuery.trim(), sessionSearchScope, showArchivedSessions, cursor, sessionPageSize)
        : await api.getSessions(showArchivedSessions, cursor, sessionPageSize);

      if (requestVersion !== sessionListRequestVersion) {
        return;
      }

      mergeSessionPage(response, shouldPinSession(selectedSessionSummary) ? selectedSessionSummary : null, true);
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

  async function selectSession(sessionId: string) {
    if (selectedSessionId === sessionId && conversation) {
      syncSelectedSessionInUrl(sessionId);
      clearSessionHighlight(sessionId);
      return true;
    }

    clearHydrationRefresh();
    loadingDetail = true;
    selectedSessionId = sessionId;
    conversation = null;
    pendingSessionEvents = {
      [sessionId]: []
    };
    expandedItems = {};
    expandedFileChangeEntries = {};
    loadingItemDetails = {};
    itemDetailErrors = {};
    expandedTurnLogs = {};
    loadingTurns = {};
    turnLoadErrors = {};
    for (const timer of itemDetailRefreshTimers.values()) {
      clearTimeout(timer);
    }
    itemDetailRefreshTimers.clear();
    draft = "";
    draftAttachments = [];
    titleDraft = "";
    stickTranscriptToBottom = true;
    forceTranscriptScroll = true;
    olderTurnsAutoLoadEnabled = true;
    olderTurnsAutoLoadPaused = false;
    olderTurnsAutoTriggerTimestamps = [];
    loadingOlderTurns = false;
    pendingSteerResume = null;
    optimisticMessage = null;
    sendIntent = null;
    mobileSidebarOpen = false;
    composerSettingsOpen = false;
    composerSecurityOpen = false;
    activeWorkspaceTabId = "chat";
    clearSessionHighlight(sessionId);
    disconnectStream();
    connectStream(sessionId);

    try {
      const detail = await api.getSession(sessionId, olderTurnPageSize);
      if (selectedSessionId !== sessionId) {
        return false;
      }
      let nextConversation = createConversationState(detail);
      nextConversation = flushPendingSessionEvents(sessionId, nextConversation);
      conversation = nextConversation;
      if (
        pendingQueueModeSessionId === sessionId &&
        (nextConversation.activeTurnId || nextConversation.thread.status === "running" || nextConversation.thread.status === "active")
      ) {
        pendingQueueModeSessionId = null;
      }
      titleDraft = getDisplayThreadTitle(detail.thread.name, detail.thread.preview) ?? "";
      const existingSummary = sessions.find((session) => session.id === detail.thread.id) ?? null;
      upsertSessionSummary({
        ...buildSessionSummaryFromConversation(nextConversation),
        updatedAt: Math.max(nextConversation.thread.updatedAt, existingSummary?.updatedAt ?? 0)
      });
      await loadSavedDraft(detail.thread.id, nextConversation.activeTurnId, nextConversation.preferences.steeringResumeMode);
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
          if (selectedSessionId !== sessionId) {
            return;
          }
          void api.getSession(sessionId, olderTurnPageSize).then((retryDetail) => {
            if (selectedSessionId !== sessionId) {
              return;
            }
            let recoveredConversation = createConversationState(retryDetail);
            recoveredConversation = flushPendingSessionEvents(sessionId, recoveredConversation);
            conversation = recoveredConversation;
            titleDraft = getDisplayThreadTitle(retryDetail.thread.name, retryDetail.thread.preview) ?? "";
          }).catch(() => {});
        }, 250);
      } else {
        clearHydrationRefresh();
      }
      syncSelectedSessionInUrl(detail.thread.id);
      return true;
    } catch (error) {
      disconnectStream();
      clearHydrationRefresh();
      errorText = describeError(error);
      if (selectedSessionId === sessionId) {
        syncSelectedSessionInUrl(null);
      }
      return false;
    } finally {
      loadingDetail = false;
    }
  }

  function connectStream(sessionId: string) {
    disconnectStream();
    releaseSessionStream = api.subscribeSession(sessionId, (payload: StreamEvent) => {
      if (payload.kind === "notification" && payload.method === "codex-webui/shutdownScheduled") {
        noticeText = ui.shutdownScheduledNotice(Number(payload.params.delaySeconds ?? 0));
        if (config) {
          config = {
            ...config,
            startup: {
              ...config.startup,
              scheduledShutdown: {
                sessionId,
                scheduledFor: Number(payload.params.scheduledFor ?? Date.now()),
                delaySeconds: Number(payload.params.delaySeconds ?? config.systemShutdown.delaySeconds)
              }
            }
          };
          syncStartupAlertModal(config, true);
        }
      }

      if (payload.kind === "notification" && payload.method === "codex-webui/shutdownFailed") {
        errorText = m.shutdown_failed({ message: String(payload.params.message ?? m.unknown_error()) });
        if (config?.startup.scheduledShutdown?.sessionId === sessionId) {
          config = {
            ...config,
            startup: {
              ...config.startup,
              scheduledShutdown: null
            }
          };
          syncStartupAlertModal(config);
        }
      }

      if (payload.kind === "notification" && payload.method === "codex-webui/queueDispatchFailed") {
        errorText = m.queue_dispatch_failed({ message: String(payload.params.message ?? m.unknown_error()) });
      }

      if (payload.kind === "notification" && payload.method === "codex-webui/sessionHydrationFailed") {
        clearHydrationRefresh();
        errorText = m.session_history_failed({ message: String(payload.params.message ?? m.unknown_error()) });
      }

      if (payload.kind === "serverRequest") {
        highlightSession(sessionId, "attention");
        notifyBrowser(m.input_required_notification_title(), m.input_required_notification_body());
      }

      if (conversation?.thread.id === sessionId) {
        conversation = applyStreamEvent(conversation, payload);
        if (
          pendingQueueModeSessionId === sessionId &&
          payload.kind === "notification" &&
          (payload.method === "turn/started" ||
            ((payload.method === "thread/status/changed") &&
              (String(payload.params.status ?? "") === "running" || String(payload.params.status ?? "") === "active")))
        ) {
          pendingQueueModeSessionId = null;
        }

        if (
          payload.kind === "notification" &&
          ["turn/started", "turn/completed", "thread/name/updated", "thread/status/changed"].includes(payload.method)
        ) {
          applySessionSummaryUpdate(buildSessionSummaryFromConversation(conversation));
        }

        if (payload.kind === "notification" && payload.method === "codex-webui/queueUpdated") {
          dismissedQueueResumeBySessionId = {
            ...dismissedQueueResumeBySessionId,
            [sessionId]: false
          };
          if (config) {
            const nextPausedQueues = config.startup.pausedQueues.filter((entry) => entry.sessionId !== sessionId);
            if (conversation.queue.resumeRequired && conversation.queue.items.length > 0) {
              nextPausedQueues.unshift({
                sessionId,
                name: getDisplayThreadTitle(conversation.thread.name, conversation.thread.preview),
                cwd: conversation.thread.cwd,
                pendingCount: conversation.queue.items.length,
                updatedAt: conversation.queue.updatedAt
              });
            }
            config = {
              ...config,
              startup: {
                ...config.startup,
                pausedQueues: nextPausedQueues
              }
            };
            syncStartupAlertModal(config);
          }
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
        pendingSessionEvents = {
          ...pendingSessionEvents,
          [sessionId]: [...(pendingSessionEvents[sessionId] ?? []), payload]
        };
      }

      if (payload.kind === "notification" && payload.method === "turn/completed") {
        if (payload.method === "turn/completed") {
          highlightSession(sessionId, "completed");
          notifyBrowser(m.task_completed_notification_title(), m.task_completed_notification_body());
        }
      }
    });
  }

  function disconnectStream() {
    releaseSessionStream?.();
    releaseSessionStream = null;
  }

  function clearHydrationRefresh() {
    if (!hydrationRefreshTimer) {
      return;
    }
    clearTimeout(hydrationRefreshTimer);
    hydrationRefreshTimer = null;
  }

  function flushPendingSessionEvents(sessionId: string, nextConversation: ConversationState) {
    const queued = pendingSessionEvents[sessionId] ?? [];
    if (queued.length === 0) {
      return nextConversation;
    }

    const remaining = { ...pendingSessionEvents };
    delete remaining[sessionId];
    pendingSessionEvents = remaining;

    return queued.reduce((current, event) => applyStreamEvent(current, event), nextConversation);
  }

  async function recoverFromReconnect() {
    if (authenticated !== true) {
      return;
    }

    try {
      const authSession = await api.getAuthSession();
      if (!authSession.authenticated) {
        clearWorkspaceForLoggedOut();
        return;
      }

      ensureGlobalStreamSubscription();
      runtime = await api.getRuntimeStatus();
      if (!runtime.installed) {
        resetWorkspaceState();
        return;
      }

      config = await api.getConfig();
      syncStartupAlertModal(config);
      await refreshSessions(shouldPinSession(selectedSessionSummary) ? selectedSessionSummary : null);
      await refreshTerminals();
      void refreshQuota(false);
      void refreshAccountState(false);

      if (!selectedSessionId) {
        return;
      }

      const detail = await api.getSession(selectedSessionId, Math.max(conversation?.thread.turns.length ?? 0, olderTurnPageSize));
      if (selectedSessionId === detail.thread.id) {
        let nextConversation = createConversationState(detail);
        nextConversation = flushPendingSessionEvents(detail.thread.id, nextConversation);
        conversation = nextConversation;
        titleDraft = getDisplayThreadTitle(detail.thread.name, detail.thread.preview) ?? "";
        await loadSavedDraft(detail.thread.id, nextConversation.activeTurnId, nextConversation.preferences.steeringResumeMode);
        stickTranscriptToBottom = true;
        forceTranscriptScroll = true;
        clearHydrationRefresh();
      }
    } catch (error) {
      errorText = describeError(error);
    }
  }

  function handleGlobalEvent(event: GlobalStreamEvent) {
    if (event.kind !== "notification") {
      return;
    }

    if (event.method === "codex-webui/sessionAttention") {
      const sessionId = String(event.params.sessionId ?? "");
      const reason = String(event.params.reason ?? "");
      if (!sessionId) {
        return;
      }
      if (reason === "completed") {
        highlightSession(sessionId, "completed");
        void notifyBrowser(m.task_completed_notification_title(), m.task_completed_notification_body());
      } else {
        highlightSession(sessionId, "attention");
        void notifyBrowser(m.input_required_notification_title(), m.input_required_notification_body());
      }
      return;
    }

    if (event.method === "codex-webui/sessionSummaryUpdated") {
      const summary = event.params.session as SessionSummary | undefined;
      if (summary?.id) {
        applySessionSummaryUpdate(summary);
      } else {
        scheduleSessionRefresh(60);
      }
      return;
    }

    if (event.method === "codex-webui/sessionListsInvalidated") {
      scheduleSessionRefresh(60);
      return;
    }

    if (event.method === "codex-webui/configUpdated") {
      if (config) {
        config = {
          ...config,
          defaults: event.params.defaults as SessionPreferences
        };
      }
      return;
    }

    if (event.method === "codex-webui/accountUpdated") {
      accountLoginFlow = null;
      void refreshAccountState(false);
      void refreshQuota(true);
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
          void refreshQuota(true);
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
      void refreshQuota(true);
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
    const sessionId = selectedSessionId;
    const currentDraft = draft;
    const intent: "message" | "queue" = queueModeActive ? "queue" : "message";
    const hasPendingSteerResume = pendingSteerResume?.sessionId === sessionId && !currentDraft.trim();

    if (!sessionId || draftPersistencePaused || hasPendingSteerResume) {
      return;
    }

    if (draftSaveTimer) {
      clearTimeout(draftSaveTimer);
    }

    draftSaveTimer = setTimeout(async () => {
      draftSaveTimer = null;
      try {
        if (currentDraft.trim()) {
          await api.saveSessionDraft(sessionId, currentDraft, intent);
        } else {
          await api.clearSessionDraft(sessionId);
        }
      } catch (error) {
        if (selectedSessionId === sessionId) {
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

  function getResumeDraftKey(sessionId: string, updatedAt: number | null) {
    return `${sessionId}:${updatedAt ?? 0}`;
  }

  async function loadSavedDraft(sessionId: string, activeTurnId: string | null, resumeMode: SessionPreferences["steeringResumeMode"]) {
    draftPersistencePaused = true;

    try {
      const saved = await api.getSessionDraft(sessionId);
      if (selectedSessionId !== sessionId) {
        return;
      }

      const savedDraft = saved.draft.trim();
      if (!savedDraft) {
        pendingSteerResume = null;
        draft = "";
        return;
      }

      const resumeKey = getResumeDraftKey(sessionId, saved.updatedAt);
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
      errorText = describeError(error);
    } finally {
      queueMicrotask(() => {
        draftPersistencePaused = false;
      });
    }
  }

  function keepSavedDraftInComposer() {
    if (!pendingSteerResume || pendingSteerResume.sessionId !== selectedSessionId) {
      return;
    }

    handledResumeDraftKeys.add(getResumeDraftKey(pendingSteerResume.sessionId, pendingSteerResume.updatedAt));
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

    try {
      await api.clearSessionDraft(selectedSessionId);
      pendingSteerResume = null;
      if (!draft.trim()) {
        draft = "";
      }
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function resumeSavedSteer() {
    if (!pendingSteerResume || pendingSteerResume.sessionId !== selectedSessionId) {
      return;
    }

    handledResumeDraftKeys.add(getResumeDraftKey(pendingSteerResume.sessionId, pendingSteerResume.updatedAt));
    draft = pendingSteerResume.draft;
    const steerDraft = pendingSteerResume.draft;
    pendingSteerResume = null;
    await sendSteerPrompt(steerDraft, true);
  }

  function openMobileSidebar() {
    workspaceMenuOpen = false;
    mobileSidebarOpen = true;
  }

  function closeMobileSidebar() {
    mobileSidebarOpen = false;
  }

  async function createSession() {
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
    mobileSidebarOpen = false;
    activateDraftSession(config.defaults);
  }

  function setPreference<Key extends keyof SessionPreferences>(key: Key, value: SessionPreferences[Key]) {
    if (!conversation) {
      return;
    }

    conversation = {
      ...conversation,
      preferences: {
        ...conversation.preferences,
        [key]: value
      }
    };
    schedulePreferenceSave();
  }

  function schedulePreferenceSave() {
    if (!selectedSessionId || !conversation) {
      return;
    }
    if (saveTimer) {
      clearTimeout(saveTimer);
    }
    saveTimer = setTimeout(async () => {
      try {
        const saved = await api.savePreferences(selectedSessionId!, conversation!.preferences);
        const nextConfig = await api.getConfig();
        if (conversation?.thread.id === selectedSessionId) {
          conversation = {
            ...conversation,
            preferences: saved
          };
        }
        config = nextConfig;
        await refreshSessions(shouldPinSession(selectedSessionSummary) ? selectedSessionSummary : null);
      } catch (error) {
        errorText = describeError(error);
      }
    }, 350);
  }

  async function saveTitle() {
    if (!titleDraft.trim()) {
      return;
    }

    const nextTitle = titleDraft.trim();
    const currentDisplayTitle = getDisplayThreadTitle(conversation?.thread.name, conversation?.thread.preview) ?? "";
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
      }
      titleDraft = nextTitle;
      return;
    }

    try {
      await api.renameSession(selectedSessionId, nextTitle);
      const archived = selectedSessionSummary?.archived ?? showArchivedSessions;
      upsertSessionSummary({
        id: selectedSessionId,
        name: nextTitle,
        preview: conversation?.thread.preview ?? "",
        queueCount: conversation?.queue.items.length ?? selectedSessionSummary?.queueCount ?? 0,
        cwd: conversation?.thread.cwd ?? config?.defaults.cwd ?? "",
        archived,
        createdAt: conversation?.thread.createdAt ?? Math.floor(Date.now() / 1000),
        updatedAt: Math.floor(Date.now() / 1000),
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
      }
      scheduleSessionRefresh(80);
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function sendMessage() {
    if (!conversation || sending || startingMessage || uploading || queueModeActive) {
      return;
    }

    const prompt = draft.trim();
    const draftText = draft;
    const attachmentNames = draftAttachments.map((attachment) => attachment.originalName);
    const attachmentSnapshot = [...draftAttachments];
    startingMessage = true;
    errorText = "";
    noticeText = "";

    try {
      const materialized = await ensureSessionForComposer();
      if (!materialized) {
        return;
      }

      const sessionId = materialized.sessionId;
      const activeConversation = materialized.state;
      const attachmentIds = draftAttachments.map((attachment) => attachment.id);
      const preferences = activeConversation.preferences;
      optimisticMessage = {
        sessionId,
        prompt,
        attachmentNames,
        createdAt: Date.now(),
        baselineTurnId: activeConversation.thread.turns.at(-1)?.id ?? null,
        baselineTurnCount: activeConversation.thread.turns.length
      };
      stickTranscriptToBottom = true;
      forceTranscriptScroll = true;
      pendingQueueModeSessionId = sessionId;
      recordComposerHistory(prompt);
      draft = "";
      draftAttachments = [];
      scheduleComposerTextareaResize();
      composerSettingsOpen = false;
      composerSecurityOpen = false;

      void api
        .sendMessage(sessionId, {
          prompt: draftText,
          attachmentIds,
          preferences
        })
      .then(() => {
        scheduleSessionRefresh(80);
      })
        .catch((error) => {
          if (pendingQueueModeSessionId === sessionId) {
            pendingQueueModeSessionId = null;
          }
          if (optimisticMessage?.sessionId === sessionId && optimisticMessage.prompt === prompt) {
            optimisticMessage = null;
          }
          if (selectedSessionId === sessionId && !draft.trim() && draftAttachments.length === 0) {
            draft = draftText;
            draftAttachments = attachmentSnapshot;
            scheduleComposerTextareaResize();
          }
          errorText = describeError(error);
        })
        .finally(() => {
          startingMessage = false;
        });
    } catch (error) {
      errorText = describeError(error);
      startingMessage = false;
    }
  }

  async function queueMessage() {
    if (!selectedSessionId || !conversation || sending || uploading || !canQueueComposerMessage(conversation) || (!draft.trim() && draftAttachments.length === 0)) {
      return;
    }

    const prompt = draft.trim();
    sending = true;
    sendIntent = "queue";
    errorText = "";
    noticeText = "";

    try {
      const queue = await api.enqueueSessionMessage(selectedSessionId, {
        prompt: draft,
        attachmentIds: draftAttachments.map((attachment) => attachment.id)
      });
      if (conversation?.thread.id === selectedSessionId) {
        conversation = {
          ...conversation,
          queue
        };
      }

      const enqueueConfirmed =
        queue.enqueueAccepted === true &&
        typeof queue.enqueueItemId === "string" &&
        queue.items.some((item) => item.id === queue.enqueueItemId);

      if (!enqueueConfirmed) {
        errorText = m.queue_enqueue_failed();
        return;
      }

      recordComposerHistory(prompt);
      draft = "";
      draftAttachments = [];
      scheduleComposerTextareaResize();
      composerSettingsOpen = false;
      composerSecurityOpen = false;
      noticeText = m.queue_notice();
    } catch (error) {
      errorText = describeError(error);
    } finally {
      sending = false;
      sendIntent = null;
    }
  }

  async function sendSteerPrompt(prompt: string, clearComposer = false) {
    if (!selectedSessionId || !running || !prompt.trim() || sending) {
      return;
    }

    const normalizedPrompt = prompt.trim();
    sending = true;
    sendIntent = "steer";
    try {
      await api.steerTurn(
        selectedSessionId,
        normalizedPrompt,
        clearComposer || draft.trim() === normalizedPrompt ? draftAttachments.map((attachment) => attachment.id) : []
      );
      recordComposerHistory(normalizedPrompt);
      if (clearComposer || draft.trim() === normalizedPrompt) {
        draft = "";
        draftAttachments = [];
        scheduleComposerTextareaResize();
      }
      pendingSteerResume = null;
    } catch (error) {
      errorText = describeError(error);
    } finally {
      sending = false;
      sendIntent = null;
    }
  }

  async function steerTurn() {
    await sendSteerPrompt(draft, true);
  }

  async function dispatchQueuedMessage(queueId: string, mode: "message" | "steer") {
    if (!selectedSessionId || sending) {
      return;
    }

    sending = true;
    sendIntent = mode === "steer" ? "steer" : "queue";
    errorText = "";
    noticeText = "";

    try {
      const queue = await api.dispatchQueuedMessage(selectedSessionId, queueId, mode);
      if (conversation?.thread.id === selectedSessionId) {
        conversation = {
          ...conversation,
          queue
        };
      }
      dismissedQueueResumeBySessionId = {
        ...dismissedQueueResumeBySessionId,
        [selectedSessionId]: false
      };
    } catch (error) {
      errorText = describeError(error);
    } finally {
      sending = false;
      sendIntent = null;
    }
  }

  async function removeQueuedMessage(queueId: string) {
    if (!selectedSessionId || sending) {
      return;
    }

    try {
      const queue = await api.removeQueuedMessage(selectedSessionId, queueId);
      if (conversation?.thread.id === selectedSessionId) {
        conversation = {
          ...conversation,
          queue
        };
      }
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
    if (!selectedSessionId || sending) {
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

    sending = true;
    sendIntent = "queue";
    errorText = "";
    noticeText = "";

    try {
      const queue = await api.updateQueuedMessage(selectedSessionId, queueId, {
        prompt: nextPrompt,
        attachmentIds: queuedItem.attachmentIds
      });
      if (conversation?.thread.id === selectedSessionId) {
        conversation = {
          ...conversation,
          queue
        };
      }
      noticeText = m.queued_followup_updated();
      cancelQueuedMessageEdit();
    } catch (error) {
      const message = describeError(error);
      errorText =
        message === "Internal Error"
          ? m.queued_followup_restart_required()
          : message;
    } finally {
      sending = false;
      sendIntent = null;
    }
  }

  async function resumeQueuedMessages() {
    if (!selectedSessionId || sending) {
      return;
    }

    sending = true;
    sendIntent = "queue";
    errorText = "";
    noticeText = "";

    try {
      const queue = await api.resumeSessionQueue(selectedSessionId);
      if (conversation?.thread.id === selectedSessionId) {
        conversation = {
          ...conversation,
          queue
        };
      }
      dismissedQueueResumeBySessionId = {
        ...dismissedQueueResumeBySessionId,
        [selectedSessionId]: false
      };
    } catch (error) {
      errorText = describeError(error);
    } finally {
      sending = false;
      sendIntent = null;
    }
  }

  async function submitComposer() {
    if (queueModeActive) {
      await queueMessage();
      return;
    }
    await sendMessage();
  }

  function promptAttachmentPicker() {
    if (uploading) {
      return;
    }
    filePickerElement?.click();
  }

  async function uploadFiles(files: FileList | null) {
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

      const response = await api.uploadAttachments(materialized.sessionId, Array.from(files));
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
    if (!selectedSessionId) {
      return;
    }
    try {
      await api.deleteAttachment(selectedSessionId, attachmentId);
      draftAttachments = draftAttachments.filter((attachment) => attachment.id !== attachmentId);
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function interruptTurn() {
    if (!selectedSessionId || !running) {
      return;
    }
    try {
      await api.abortTurn(selectedSessionId);
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function archiveSessionFromSidebar(session: SessionSummary) {
    try {
      if (showArchivedSessions || session.archived) {
        const response = await api.unarchiveSession(session.id);
        applySessionSummaryUpdate(response.session);
        noticeText = m.session_restored_notice();
      } else {
        await api.archiveSession(session.id);
        applySessionSummaryUpdate({
          ...session,
          archived: true,
          updatedAt: Math.floor(Date.now() / 1000)
        });
        noticeText = m.session_archived_notice();
      }
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function archiveCurrentSession() {
    if (!selectedSessionId || showArchivedSessions) {
      return;
    }

    try {
      await api.archiveSession(selectedSessionId);
      showArchivedSessions = true;
      await refreshSessions();
      noticeText = m.session_archived_notice();
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function unarchiveCurrentSession() {
    if (!selectedSessionId || !showArchivedSessions) {
      return;
    }

    try {
      const response = await api.unarchiveSession(selectedSessionId);
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

    loginBusy = true;
    loginMessage = "";

    try {
      await api.login(loginPassword.trim());
      loginPassword = "";
      await bootstrap();
    } catch (error) {
      authenticated = false;
      loginMessage = error instanceof Error ? error.message : ui.loginFailed;
    } finally {
      loginBusy = false;
    }
  }

  async function startAccountLogin(type: "chatgpt" | "chatgptDeviceCode") {
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
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function cancelAccountLogin(loginId: string) {
    try {
      await api.cancelAccountLogin(loginId);
      accountLoginFlow = null;
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function logoutAccount() {
    try {
      await api.logoutAccount();
      accountLoginFlow = null;
      await refreshAccountState(true);
      await refreshQuota(true);
    } catch (error) {
      errorText = describeError(error);
    }
  }

  async function refreshRuntimeStatus(checkForUpdate = false) {
    runtimeBusyAction = checkForUpdate ? "check" : null;
    try {
      runtime = checkForUpdate ? await api.checkRuntimeUpdate() : await api.getRuntimeStatus();
    } catch (error) {
      errorText = describeError(error);
    } finally {
      if (runtimeBusyAction === "check") {
        runtimeBusyAction = null;
      }
    }
  }

  async function refreshQuota(force = false) {
    if (force) {
      quotaBusy = true;
    }

    try {
      quota = await api.getQuota(force);
    } catch (error) {
      if (force) {
        errorText = describeError(error);
      }
    } finally {
      if (force) {
        quotaBusy = false;
      }
    }
  }

  async function installCodex() {
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

  async function updateCodex() {
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

  function openGitTab() {
    gitTabOpen = true;
    activeWorkspaceTabId = "git";
    workspaceMenuOpen = false;
  }

  function openSettingsTab() {
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

  async function createTerminalTab() {
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

  function extractLocalFilePath(href: string) {
    const cleanHref = href.split("#")[0]?.split("?")[0] ?? href;
    const lineMatch = cleanHref.match(/^(\/.*?)(?::\d+)?$/u);
    return lineMatch?.[1] ?? cleanHref;
  }

  async function openGitFileFromMessage(href: string) {
    try {
      const resolved = await api.resolveGitFile(extractLocalFilePath(href));
      if (conversation?.preferences.gitRepoPath !== resolved.repoPath) {
        handleRepoSelect(resolved.repoPath);
      }
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
    scheduleSessionRefresh(query.trim() ? 180 : 60);
  }

  function updateSessionSearchScope(scope: SessionSearchScope) {
    sessionSearchScope = scope;
    scheduleSessionRefresh(60);
  }

  function updateArchivedSessions(nextValue: boolean) {
    showArchivedSessions = nextValue;
    scheduleSessionRefresh(0);
  }

  async function resolvePendingRequest(request: PendingServerRequest, result: unknown) {
    if (!selectedSessionId || !conversation) {
      return;
    }
    try {
      await api.resolveRequest(selectedSessionId, request.id, result);
      conversation = {
        ...conversation,
        pendingRequests: conversation.pendingRequests.filter((pending) => pending.id !== request.id)
      };
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

  function hasConversationEchoedOptimisticMessage(
    currentConversation: ConversationState,
    optimistic: OptimisticMessageState
  ) {
    const targetPrompt = normalizeMessageForComparison(optimistic.prompt);
    const targetAttachments = new Set(optimistic.attachmentNames.map((name) => name.trim()).filter(Boolean));

    return currentConversation.thread.turns.some((turn, turnIndex) => {
      if (turnIndex < optimistic.baselineTurnCount) {
        return false;
      }

      return turn.items.some((item) => {
        if (item.type !== "userMessage") {
          return false;
        }

        const userText = normalizeMessageForComparison(getUserText(item));
        const attachmentNames = getUserAttachmentNames(item);
        const textMatches = targetPrompt.length > 0 && userText === targetPrompt;
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

  $effect(() => {
    draft;
    scheduleComposerTextareaResize();
  });
  }

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
    stickTranscriptToBottom = true;
    forceTranscriptScroll = true;
  }

  async function branchFromMessage(text: string) {
    if (!config) {
      return;
    }
    try {
      const created = await api.createSession(config.defaults, inferBranchTitle(text));
      await refreshSessions(created);
      await selectSession(created.id);
      draft = text;
      noticeText = m.opened_branch_thread();
    } catch (error) {
      errorText = describeError(error);
    }
  }

  function inferBranchTitle(text: string) {
    const title = inferDisplayThreadTitle(text);
    if (!title) {
      return getDefaultThreadTitle();
    }
    return title;
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

  function formatThreadStatusLabel(status: string | null | undefined) {
    const normalized = String(status ?? "").trim().toLowerCase();
    if (!normalized || normalized === "idle") {
      return "idle";
    }
    if (normalized === "running" || normalized === "active") {
      return ui.active;
    }
    if (["completed", "done", "success"].includes(normalized)) {
      return ui.done;
    }
    if (normalized === "stopped") {
      return ui.stopped;
    }
    return normalized;
  }

  function getThreadStatusTextClass(status: string | null | undefined) {
    const normalized = String(status ?? "").trim().toLowerCase();
    if (normalized === "running" || normalized === "active") {
      return "text-amber-600";
    }
    if (["completed", "done", "success"].includes(normalized)) {
      return "text-emerald-600";
    }
    if (normalized === "stopped" || normalized === "idle") {
      return "text-gray-500";
    }
    if (["failed", "error", "systemerror"].includes(normalized)) {
      return "text-red-600";
    }
    return "text-gray-500";
  }

  function formatContextUsage() {
    const tokenUsage = conversation?.tokenUsage;
    if (!tokenUsage?.modelContextWindow || tokenUsage.modelContextWindow <= 0) {
      return null;
    }

    if (tokenUsage.total.totalTokens <= 0 || tokenUsage.total.totalTokens >= tokenUsage.modelContextWindow) {
      return null;
    }

    const percent = Math.min(99, Math.round((tokenUsage.total.totalTokens / tokenUsage.modelContextWindow) * 100));
    if (percent <= 0) {
      return null;
    }

    return `${percent}% of context`;
  }

  function summarizeQueueItem(item: SessionQueueItem) {
    const text = item.prompt.trim();
    if (text) {
      return text.length > 140 ? `${text.slice(0, 140).trimEnd()}…` : text;
    }
    if (item.attachmentNames.length > 0) {
      return item.attachmentNames.join(", ");
    }
    return "Queued follow-up";
  }

  function resetComposerHistoryNavigation() {
    composerHistoryIndex = -1;
    composerHistoryDraft = "";
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
    const minHeight = 52;
    const maxHeight = 240;
    textarea.style.height = "0px";
    const nextHeight = Math.max(minHeight, Math.min(textarea.scrollHeight, maxHeight));
    textarea.style.height = `${nextHeight}px`;
    textarea.style.overflowY = textarea.scrollHeight > maxHeight ? "auto" : "hidden";
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

  function handleComposerInput() {
    scheduleComposerTextareaResize();
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

    if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
      event.preventDefault();
      if (canQueueComposerMessage()) {
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
    if (value instanceof Error) {
      const message = value.message.trim();
      if (message.startsWith("{")) {
        try {
          const parsed = JSON.parse(message) as { message?: unknown };
          if (typeof parsed.message === "string" && parsed.message.trim()) {
            return parsed.message.trim();
          }
        } catch {
          // Fall through to the raw message.
        }
      }
      return message;
    }
    return m.unknown_error();
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
        readOnlyBuffer = [...readOnlyBuffer, item];
        continue;
      }

      if (item.type === "fileChange") {
        flushReadOnlyBuffer();
        fileChangeBuffer = [...fileChangeBuffer, item];
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

  function handleTranscriptScroll() {
    if (!transcriptElement) {
      return;
    }

    const remaining = transcriptElement.scrollHeight - transcriptElement.scrollTop - transcriptElement.clientHeight;
    stickTranscriptToBottom = remaining <= scrollBottomThreshold;

    if (
      !forceTranscriptScroll &&
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
      return formatValue(item.query) || m.search_details();
    }
    if (item.type === "contextCompaction") {
      return isContextCompactionRunning(conversation?.activeTurnId ?? "", item)
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
            : {
                ...turn,
                items: turn.items.map((item) => (item.id !== itemId ? item : { ...item, ...nextItem, detailState: "loaded" }))
              }
        )
      }
    };
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

    const turn = conversation.thread.turns.find((candidate) => candidate.id === turnId);
    if (!turn || turn.detailState === "full" || loadingTurns[turnId]) {
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
      const response = await api.getSessionTurn(selectedSessionId, turnId);
      if (!conversation || conversation.thread.id !== selectedSessionId) {
        return;
      }

      conversation = {
        ...conversation,
        thread: {
          ...conversation.thread,
          turns: conversation.thread.turns.map((candidate) => (candidate.id === turnId ? response.turn : candidate))
        }
      };
    } catch (error) {
      turnLoadErrors = {
        ...turnLoadErrors,
        [turnId]: describeError(error)
      };
    } finally {
      loadingTurns = {
        ...loadingTurns,
        [turnId]: false
      };
    }
  }

  async function loadOlderTurns(mode: "auto" | "manual" = "manual") {
    if (!selectedSessionId || !conversation || loadingOlderTurns || conversation.hydration.remainingTurns <= 0) {
      return;
    }

    const beforeTurnId = conversation.thread.turns[0]?.id;
    if (!beforeTurnId) {
      return;
    }

    const previousHeight = transcriptElement?.scrollHeight ?? 0;
    const previousTop = transcriptElement?.scrollTop ?? 0;
    loadingOlderTurns = true;
    if (mode === "manual") {
      olderTurnsAutoLoadPaused = false;
      olderTurnsAutoLoadEnabled = true;
      olderTurnsAutoTriggerTimestamps = [];
    }

    try {
      const response = await api.getSessionOlderTurns(selectedSessionId, beforeTurnId, olderTurnPageSize);
      if (!conversation || conversation.thread.id !== selectedSessionId) {
        return;
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
    } catch (error) {
      errorText = describeError(error);
      olderTurnsAutoLoadPaused = true;
      olderTurnsAutoLoadEnabled = false;
    } finally {
      loadingOlderTurns = false;
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

  async function loadItemDetail(turnId: string, itemId: string, force = false) {
    if (!selectedSessionId || !conversation) {
      return;
    }

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
      const response = await api.getSessionItemDetail(selectedSessionId, turnId, itemId);
      updateConversationItem(turnId, itemId, response.item);
    } catch (error) {
      itemDetailErrors = {
        ...itemDetailErrors,
        [itemKey]: describeError(error)
      };
    } finally {
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
    const itemKey = getItemKey(turnId, itemId);
    if (!expandedItems[itemKey] || itemDetailRefreshTimers.has(itemKey)) {
      return;
    }

    itemDetailRefreshTimers.set(
      itemKey,
      setTimeout(() => {
        itemDetailRefreshTimers.delete(itemKey);
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
    return JSON.stringify(item, null, 2);
  }

  function isTurnRunning(turnId: string) {
    if (!conversation?.activeTurnId || conversation.activeTurnId !== turnId) {
      return false;
    }

    if (conversation.thread.status === "running" || conversation.thread.status === "active") {
      return true;
    }

    return conversation.thread.turns.some((turn) => turn.id === turnId && String(turn.status ?? "") === "inProgress");
  }

  function isContextCompactionRunning(turnId: string, item: CodexItem) {
    if (item.type !== "contextCompaction") {
      return false;
    }
    if (String(item.lifecycleStatus ?? "") === "completed") {
      return false;
    }
    return String(item.lifecycleStatus ?? "") === "inProgress" || isTurnRunning(turnId);
  }

  function getFinalAgentItem(turn: ConversationState["thread"]["turns"][number]) {
    for (let index = turn.items.length - 1; index >= 0; index -= 1) {
      const item = turn.items[index];
      if (item.type === "agentMessage") {
        return item;
      }
    }
    return null;
  }

  function getCollapsedTurnItems(turn: ConversationState["thread"]["turns"][number]) {
    const finalAgentItem = getFinalAgentItem(turn);
    if (!finalAgentItem) {
      return [] as CodexItem[];
    }

    return turn.items.filter((item) => item.type !== "userMessage" && item.type !== "fileChange" && item.id !== finalAgentItem.id);
  }

  function getVisibleSummaryEntries(turn: ConversationState["thread"]["turns"][number]) {
    if (!shouldCollapseTurnLogs(turn)) {
      return [] as RenderableTurnEntry[];
    }

    return getRenderableTurnEntries(turn.items.filter((item) => item.type === "fileChange"));
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
    return getRenderableTurnEntries(getCollapsedTurnItems(turn));
  }

  function getTurnEntries(turn: ConversationState["thread"]["turns"][number]) {
    return getRenderableTurnEntries(turn.items.filter((item) => item.type !== "userMessage"));
  }

  function getCollapsedTurnProgressCount(turn: ConversationState["thread"]["turns"][number]) {
    if (turn.detailState === "summary" && typeof turn.hiddenItemCount === "number") {
      return turn.hiddenItemCount + (conversation?.livePlans[turn.id] ? 1 : 0) + (conversation?.liveDiffs[turn.id] ? 1 : 0);
    }
    return (
      getCollapsedTurnEntries(turn).length +
      (conversation?.livePlans[turn.id] ? 1 : 0) +
      (conversation?.liveDiffs[turn.id] ? 1 : 0)
    );
  }

  function shouldCollapseTurnLogs(turn: ConversationState["thread"]["turns"][number]) {
    return !isTurnRunning(turn.id) && Boolean(getFinalAgentItem(turn)) && getCollapsedTurnProgressCount(turn) > 0;
  }

  function isTurnLogExpanded(turnId: string) {
    return Boolean(expandedTurnLogs[turnId]);
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
    if (!composerSettingsOpen || !composerSettingsTriggerElement || !composerSettingsPopoverElement || typeof window === "undefined") {
      return;
    }

    await tick();
    const margin = 12;
    const triggerRect = composerSettingsTriggerElement.getBoundingClientRect();
    const popoverRect = composerSettingsPopoverElement.getBoundingClientRect();
    const width =
      window.innerWidth <= 760
        ? Math.min(window.innerWidth - margin * 2, 420)
        : Math.min(Math.max(popoverRect.width || 420, Math.min(triggerRect.width, 420)), window.innerWidth - margin * 2);
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

    composerSettingsPopoverStyle = `top:${Math.round(top)}px;left:${Math.round(left)}px;width:${Math.round(width)}px;max-height:${Math.max(220, window.innerHeight - margin * 2)}px;opacity:1;pointer-events:auto;`;
  }

  async function updateComposerSecurityPopoverPosition() {
    if (!composerSecurityOpen || !composerSecurityTriggerElement || !composerSecurityPopoverElement || typeof window === "undefined") {
      return;
    }

    await tick();
    const margin = 12;
    const triggerRect = composerSecurityTriggerElement.getBoundingClientRect();
    const popoverRect = composerSecurityPopoverElement.getBoundingClientRect();
    const width =
      window.innerWidth <= 760
        ? Math.min(window.innerWidth - margin * 2, 440)
        : Math.min(Math.max(popoverRect.width || 440, Math.min(triggerRect.width + 80, 440)), window.innerWidth - margin * 2);
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

    composerSecurityPopoverStyle = `top:${Math.round(top)}px;left:${Math.round(left)}px;width:${Math.round(width)}px;max-height:${Math.max(240, window.innerHeight - margin * 2)}px;opacity:1;pointer-events:auto;`;
  }

  function getComposerSettingsSummary() {
    if (!conversation) {
      return {
        model: ui.settings,
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

    if ((conversation.preferences.mode ?? "default") === "plan") {
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
      indicators
    };
  }

  const composerSettingsSummary = $derived.by(() => {
    const _locale = $localeSignal;
    return getComposerSettingsSummary();
  });

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
    if (conversation.preferences.shutdownOnCompletion ?? false) {
      indicators.push({
        key: "shutdown",
        icon: "power_settings_new",
        label: m.shutdown_after_completion()
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
      <div class="absolute inset-0 bg-gray-950/30 backdrop-blur-sm"></div>
      <div class="absolute inset-0 z-10 flex items-center justify-center p-4 sm:p-6">
        <div class="w-full max-w-xl rounded-[2rem] border border-white/70 bg-white/92 p-6 shadow-[0_32px_90px_rgba(15,23,42,0.24)] backdrop-blur-2xl sm:p-8">
          <div class="flex flex-col gap-5 sm:flex-row sm:items-start sm:justify-between">
            <div class="space-y-3">
              <p class="text-[11px] font-bold uppercase tracking-[0.28em] text-amber-700">{ui.privateGateway}</p>
              <div>
                <h1 class="text-3xl font-semibold tracking-tight text-gray-950 sm:text-4xl">{ui.appTitle}</h1>
                <p class="mt-3 max-w-md text-sm leading-7 text-gray-500">{ui.loginLede}</p>
              </div>
            </div>
            <div class="flex flex-wrap gap-2" role="group" aria-label={ui.language}>
              {#each localeOptions as option (option.value)}
                <button
                  class="rounded-full border px-3 py-1.5 text-xs font-semibold transition-colors {$activeLocale === option.value
                    ? 'border-amber-200 bg-amber-50 text-amber-700'
                    : 'border-gray-200 bg-white text-gray-500 hover:border-amber-200 hover:text-amber-700'}"
                  onclick={() => updateLocale(option.value)}
                  type="button"
                >
                  {option.label}
                </button>
              {/each}
            </div>
          </div>

          <form class="mt-8 space-y-5" onsubmit={(event) => {
            event.preventDefault();
            void handleLogin();
          }}>
            <label class="block space-y-2">
              <span class="text-sm font-semibold text-gray-700">{ui.password}</span>
              <input
                bind:value={loginPassword}
                autocomplete="current-password"
                class="w-full rounded-2xl border border-gray-200 bg-white px-4 py-3 text-sm text-gray-900 shadow-sm outline-none transition focus:border-amber-500 focus:ring-4 focus:ring-amber-100"
                placeholder={ui.password}
                type="password"
              />
            </label>

            <button
              class="inline-flex min-w-32 items-center justify-center rounded-2xl bg-amber-600 px-5 py-3 text-sm font-semibold text-white shadow-lg shadow-amber-200/70 transition hover:bg-amber-700 disabled:cursor-not-allowed disabled:bg-amber-300"
              disabled={loginBusy}
              type="submit"
            >
              {loginBusy ? ui.signingIn : ui.signIn}
            </button>
          </form>

          {#if loginMessage}
            <p class="mt-4 rounded-2xl border border-red-100 bg-red-50 px-4 py-3 text-sm text-red-700">{loginMessage}</p>
          {/if}
        </div>
      </div>
    {:else if errorText}
      <div class="pointer-events-none absolute inset-x-0 bottom-0 z-10 flex justify-center px-4 pb-5 sm:px-6">
        <p class="pointer-events-auto max-w-xl rounded-2xl border border-red-100 bg-red-50/95 px-4 py-3 text-sm text-red-700 shadow-lg shadow-red-100/70">
          {errorText}
        </p>
      </div>
    {/if}
  </div>
{:else}
<div class="flex h-[100dvh] min-h-[100dvh] w-full bg-white overflow-hidden font-sans text-gray-900">
  {#if showConnectionSnackbar || feedbackSnackbar}
    <div class="pointer-events-none fixed inset-x-0 top-0 z-[110] flex justify-center px-3 pt-3 sm:px-6">
      <div class="flex w-full max-w-xl flex-col gap-2">
        {#if showConnectionSnackbar}
          <div
            class="pointer-events-auto flex w-full items-center gap-3 rounded-2xl border px-4 py-3 shadow-xl backdrop-blur-xl transition-all duration-300 {connectionSnackbarTone === 'error'
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
        {/if}

        {#if feedbackSnackbar}
          <div
            class="pointer-events-auto flex w-full items-start gap-3 rounded-2xl border px-4 py-3 shadow-xl backdrop-blur-xl transition-all duration-300 {feedbackSnackbar.tone === 'error'
              ? 'border-red-200 bg-red-50/95 text-red-800 shadow-red-100/80'
              : 'border-emerald-200 bg-emerald-50/95 text-emerald-800 shadow-emerald-100/80'}"
            role={feedbackSnackbar.tone === "error" ? "alert" : "status"}
          >
            {#if feedbackSnackbar.tone === "error"}
              <AlertCircle size={16} class="mt-0.5 shrink-0" />
            {:else}
              <CheckCircle2 size={16} class="mt-0.5 shrink-0" />
            {/if}
            <span class="min-w-0 flex-1 text-sm font-medium leading-5">{feedbackSnackbar.text}</span>
            <button
              aria-label={ui.close}
              class="rounded-lg p-1 text-current/70 transition-colors hover:bg-black/5 hover:text-current"
              onclick={dismissFeedbackSnackbar}
              title={ui.close}
              type="button"
            >
              <X size={14} />
            </button>
          </div>
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
        ? "fixed inset-y-0 left-0 z-40 max-w-[calc(100vw-1.5rem)] shadow-2xl"
        : "flex-shrink-0 z-30"
    ]}
  >
    <SessionSidebar
      account={config?.account ?? null}
      {accountLoginFlow}
      {quota}
      {quotaBusy}
      {runtime}
      {runtimeBusyAction}
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
      onRefreshRuntime={() => {
        void refreshRuntimeStatus(true);
      }}
      onInstallRuntime={() => {
        void installCodex();
      }}
      onUpdateRuntime={() => {
        void updateCodex();
      }}
      onThemeModeChange={changeThemeMode}
      onSearchQueryChange={updateSessionSearchQuery}
      onSearchScopeChange={updateSessionSearchScope}
      onSelect={(sessionId) => {
        void selectSession(sessionId);
      }}
      onToggleArchive={(session) => {
        void archiveSessionFromSidebar(session);
      }}
      onStartAccountLogin={(type) => {
        void startAccountLogin(type);
      }}
      selectedId={selectedSessionId}
    />
  </aside>

  <!-- Main Content -->
  <main class="flex-1 flex flex-col h-full min-w-0 bg-white relative">
    <header class="flex items-center justify-between px-6 py-3 border-b border-gray-100 bg-white/80 backdrop-blur-md z-20 sticky top-0">
      <div class="flex items-center gap-4 min-w-0">
        {#if isMobileLayout}
          <button class="p-2 -ml-2 text-gray-500 hover:text-gray-900 hover:bg-gray-100 rounded-lg transition-colors" onclick={openMobileSidebar}>
            <Menu size={20} />
          </button>
        {/if}
        
        <div class="flex flex-col min-w-0">
          <input
            bind:this={titleInputElement}
            bind:value={titleDraft}
            class="text-sm font-semibold bg-transparent border-none p-0 focus:ring-0 placeholder-gray-400 truncate w-full max-w-md"
            onblur={saveTitle}
            onkeydown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                void saveTitle();
              }
            }}
            placeholder={ui.threadTitle}
          />
          {#if selectedModel}
            <div class="flex items-center gap-1.5 mt-0.5">
              <span class="text-[10px] font-bold text-gray-400 uppercase tracking-widest leading-none">{selectedModel.displayName}</span>
              <span class="w-1 h-1 rounded-full bg-gray-300"></span>
              <span class={`text-[10px] font-medium uppercase tracking-widest leading-none ${getThreadStatusTextClass(conversation?.thread.status)}`}>
                {formatThreadStatusLabel(conversation?.thread.status)}
              </span>
            </div>
          {/if}
        </div>
      </div>

      <div class="flex items-center gap-2">
        {#if conversation?.tokenUsage}
          <div class="hidden md:flex items-center gap-2 mr-2">
            <span class="px-2 py-0.5 bg-gray-50 text-[10px] font-bold text-gray-500 rounded border border-gray-100 uppercase tracking-tight">
              {formatTokenCount(conversation.tokenUsage.total.totalTokens)} tok
            </span>
            {#if formatContextUsage()}
              <span class="px-2 py-0.5 bg-amber-50 text-[10px] font-bold text-amber-700 rounded border border-amber-100 uppercase tracking-tight">
                {formatContextUsage()}
              </span>
            {/if}
          </div>
        {/if}

        {#if selectedSessionId}
          <button
            class="p-2 text-gray-400 hover:text-amber-600 hover:bg-amber-50 rounded-lg transition-all"
            onclick={() => {
              if (showArchivedSessions) void unarchiveCurrentSession();
              else void archiveCurrentSession();
            }}
            title={showArchivedSessions ? ui.restoreThread : ui.archiveThread}
            type="button"
          >
            {#if showArchivedSessions}<RotateCcw size={18} />{:else}<Archive size={18} />{/if}
          </button>
        {/if}

        <div class="w-px h-4 bg-gray-200 mx-1"></div>

        <div class="relative">
          <button 
            class="flex items-center gap-1.5 px-3 py-1.5 bg-gray-900 text-white rounded-lg text-xs font-bold hover:bg-gray-800 transition-all shadow-sm active:scale-95"
            onclick={() => (workspaceMenuOpen = !workspaceMenuOpen)}
          >
            <Plus size={14} />
            <span>{ui.open}</span>
            <ChevronDown size={12} class={workspaceMenuOpen ? 'rotate-180' : ''} />
          </button>

          {#if workspaceMenuOpen}
            <div class="absolute top-10 right-0 w-56 bg-white border border-gray-200 rounded-xl shadow-2xl p-1 z-50">
              <button class="w-full flex items-center gap-3 px-3 py-2 text-sm text-gray-700 hover:bg-gray-50 rounded-lg transition-colors group" disabled={subagentTasks.length === 0} onclick={() => { activateTab("tasks"); workspaceMenuOpen = false; }} type="button">
                <History size={16} class="text-gray-400 group-hover:text-amber-600" />
                <span>{ui.tasks}</span>
              </button>
              <button class="w-full flex items-center gap-3 px-3 py-2 text-sm text-gray-700 hover:bg-gray-50 rounded-lg transition-colors group" onclick={() => { openGitTab(); workspaceMenuOpen = false; }} type="button">
                <GitBranch size={16} class="text-gray-400 group-hover:text-amber-600" />
                <span>{ui.gitWorkspace}</span>
              </button>
              <button class="w-full flex items-center gap-3 px-3 py-2 text-sm text-gray-700 hover:bg-gray-50 rounded-lg transition-colors group" onclick={() => { openSettingsTab(); workspaceMenuOpen = false; }} type="button">
                <Settings size={16} class="text-gray-400 group-hover:text-amber-600" />
                <span>{ui.settingsSkills}</span>
              </button>
              <button class="w-full flex items-center gap-3 px-3 py-2 text-sm text-gray-700 hover:bg-gray-50 rounded-lg transition-colors group" onclick={() => { void createTerminalTab(); workspaceMenuOpen = false; }} type="button">
                <Terminal size={16} class="text-gray-400 group-hover:text-amber-600" />
                <span>{ui.newTerminal}</span>
              </button>
            </div>
          {/if}
        </div>
      </div>
    </header>

    <!-- Workspace Tabs -->
    {#if workspaceTabs.length > 1}
      <div class="flex items-center gap-1 px-4 py-1.5 bg-gray-50 border-b border-gray-200 overflow-x-auto scrollbar-none">
        {#each workspaceTabs as tab (tab.id)}
          <button
            class="flex items-center gap-2 px-3 py-1.5 rounded-lg text-xs font-semibold whitespace-nowrap transition-all {activeWorkspaceTabId === tab.id ? 'bg-white text-gray-900 shadow-sm border border-gray-200' : 'text-gray-500 hover:text-gray-700 hover:bg-gray-100/50'}"
            onclick={() => activateTab(tab.id)}
            type="button"
          >
            {#if tab.kind === 'chat'}<MessageSquare size={14} />
            {:else if tab.kind === 'tasks'}<History size={14} />
            {:else if tab.kind === 'git'}<GitBranch size={14} />
            {:else if tab.kind === 'settings'}<Settings size={14} />
            {:else if tab.kind === 'git-diff'}<FileDiff size={14} />
            {:else if tab.kind === 'code-diff'}<Layout size={14} />
            {:else if tab.kind === 'terminal'}<Terminal size={14} />
            {/if}
            <span>{tab.label}</span>
            {#if tab.id !== 'chat'}
              <span
                aria-label={`Close ${tab.label}`}
                class="ml-1 p-0.5 hover:bg-gray-200 rounded transition-colors"
                onclick={(event) => {
                  event.stopPropagation();
                  if (tab.kind === "git") closeGitTab();
                  else if (tab.kind === "settings") closeSettingsTab();
                  else if (tab.kind === "git-diff") closeGitDiffTab(tab.id);
                  else if (tab.kind === "code-diff") closeCodeDiffTab(tab.id);
                  else if (tab.kind === "terminal") void closeTerminalTab(tab.id.replace(/^terminal:/u, ""));
                }}
                onkeydown={(event) => {
                  if (event.key !== "Enter" && event.key !== " ") {
                    return;
                  }
                  event.preventDefault();
                  event.stopPropagation();
                  if (tab.kind === "git") closeGitTab();
                  else if (tab.kind === "settings") closeSettingsTab();
                  else if (tab.kind === "git-diff") closeGitDiffTab(tab.id);
                  else if (tab.kind === "code-diff") closeCodeDiffTab(tab.id);
                  else if (tab.kind === "terminal") void closeTerminalTab(tab.id.replace(/^terminal:/u, ""));
                }}
                role="button"
                tabindex="0"
              >
                <X size={10} />
              </span>
            {/if}
          </button>
        {/each}
      </div>
    {/if}

    <div class="flex-1 overflow-hidden relative">
      {#if activeWorkspaceTabId === "chat"}
        <div class="h-full flex flex-col relative bg-white">
          <div bind:this={transcriptElement} class="flex-1 overflow-y-auto pt-8 pb-8" onscroll={handleTranscriptScroll}>
            <div bind:this={transcriptContentElement} class="max-w-3xl mx-auto px-6 space-y-12">
              {#if loading || loadingDetail}
                <div class="space-y-6 animate-pulse mt-8">
                  <div class="h-4 bg-gray-100 rounded w-1/3"></div>
                  <div class="space-y-3">
                    <div class="h-4 bg-gray-50 rounded w-full"></div>
                    <div class="h-4 bg-gray-50 rounded w-5/6"></div>
                  </div>
                </div>
              {:else if conversation}
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

                {#each conversation.thread.turns as turn (turn.id)}
                  <div class="space-y-8 animate-in fade-in slide-in-from-bottom-4 duration-500">
                    {#each turn.items.filter((item) => item.type === "userMessage") as item (item.id)}
                      <div class="flex flex-col items-end gap-2 max-w-[85%] ml-auto group/user-message">
                        <div class="flex items-center gap-1 opacity-0 group-hover/user-message:opacity-100 transition-opacity">
                          <button class="p-1.5 rounded-lg text-gray-400 hover:text-gray-700 hover:bg-gray-100 transition-colors" onclick={() => void copyMessageText(getUserText(item))} title={ui.copyMessage} type="button"><Copy size={13} /></button>
                          <button class="p-1.5 rounded-lg text-gray-400 hover:text-gray-700 hover:bg-gray-100 transition-colors" onclick={() => editMessageText(getUserText(item))} title={ui.editInComposer} type="button"><MessageSquare size={13} /></button>
                          <button class="p-1.5 rounded-lg text-gray-400 hover:text-amber-700 hover:bg-amber-50 transition-colors" onclick={() => void branchFromMessage(getUserText(item))} title={ui.branchIntoNewThread} type="button"><GitBranch size={13} /></button>
                        </div>
                        <div class="px-5 py-3 bg-gray-100 rounded-2xl text-gray-800 shadow-sm border border-gray-200/50">
                          <MarkdownMessage compact on:openLocalPath={(event: CustomEvent<{ href: string }>) => void openGitFileFromMessage(event.detail.href)} text={getUserText(item)} />
                          {#if getUserAttachmentNames(item).length > 0}
                            <div class="mt-3 flex flex-wrap gap-2">
                              {#each getUserAttachmentNames(item) as name}<span class="px-2 py-1 bg-white/80 rounded-lg border border-gray-200 text-[10px] font-bold text-gray-600 flex items-center gap-1.5"><FileText size={10} />{name}</span>{/each}
                            </div>
                          {/if}
                        </div>
                      </div>
                    {/each}

                    <div class="flex gap-4">
                      <div class="flex-shrink-0 w-8 h-8 rounded-lg bg-amber-600 text-white flex items-center justify-center shadow-sm mt-1"><Bot size={18} /></div>
                      <div class="flex-1 min-w-0 space-y-6">
                        {#if shouldCollapseTurnLogs(turn)}
                          {#if getCollapsedTurnProgressCount(turn) > 0}
                            <div class="border border-gray-100 rounded-xl bg-gray-50/50 overflow-hidden">
                              <button class="turn-card-header turn-card-header--neutral w-full flex items-center justify-between px-4 py-3 hover:bg-gray-100/50 transition-colors" onclick={() => void toggleTurnLogs(turn.id)}>
                                <div class="flex items-center gap-3">
                                  <div class="p-1.5 bg-white border border-gray-200 rounded-lg text-gray-400"><History size={14} /></div>
                                  <span class="text-xs font-bold text-gray-600 tracking-tight uppercase">{m.work_steps_count({ count: String(getCollapsedTurnProgressCount(turn)) })}</span>
                                </div>
                                <ChevronDown size={14} class="text-gray-400 {isTurnLogExpanded(turn.id) ? 'rotate-180' : ''} transition-transform" />
                              </button>
                              {#if isTurnLogExpanded(turn.id)}
                                <div class="p-4 pt-0 space-y-4 bg-white/50 border-t border-gray-100">
                                  {#if isTurnLoading(turn.id)}<div class="py-4 flex items-center justify-center gap-2 text-xs text-gray-400"><RefreshCw size={12} class="animate-spin" />{m.loading()}</div>
                                  {:else if getTurnLoadError(turn.id)}<div class="p-3 bg-red-50 text-red-600 rounded-xl text-xs">{getTurnLoadError(turn.id)}</div>
                                  {:else}{#each getCollapsedTurnEntries(turn) as entry (entry.key)}{@render renderTurnEntry(turn.id, entry)}{/each}{/if}
                                </div>
                              {/if}
                            </div>
                          {/if}
                          {#each getVisibleSummaryEntries(turn) as entry (entry.key)}{@render renderTurnEntry(turn.id, entry)}{/each}
                          {#if getFinalAgentItem(turn)}{@render renderTurnItem(turn.id, getFinalAgentItem(turn)!)}{/if}
                        {:else}{#each getTurnEntries(turn) as entry (entry.key)}{@render renderTurnEntry(turn.id, entry)}{/each}{/if}
                        
                        {#if conversation.livePlans[turn.id] && turn.id !== conversation.activeTurnId}
                          <div class="border border-amber-100 rounded-xl bg-amber-50/30 overflow-hidden">
                            <div class="turn-card-header turn-card-header--amber px-4 py-2 border-b border-amber-100 flex items-center gap-2 text-[10px] font-bold text-amber-700 uppercase tracking-widest"><ListTodo size={12} /><span>{ui.livePlan}</span></div>
                            <div class="p-4 text-sm text-gray-700 space-y-3">
                              {#if conversation.livePlans[turn.id].explanation}<p class="leading-relaxed">{conversation.livePlans[turn.id].explanation}</p>{/if}
                              <ul class="space-y-1.5 pl-2">
                                {#each conversation.livePlans[turn.id].plan as step}
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
                          <details class="group bg-gray-50 border border-gray-200 rounded-xl overflow-hidden">
                            <summary class="turn-card-header turn-card-header--neutral flex items-center justify-between px-4 py-2 text-[10px] font-bold text-gray-500 uppercase tracking-widest cursor-pointer hover:bg-gray-100 transition-colors">
                              <div class="flex items-center gap-2"><FileDiff size={12} /> {ui.aggregatedDiff}</div>
                              <ChevronDown size={12} class="group-open:rotate-180 transition-transform" />
                            </summary>
                            <div class="flex items-center justify-between gap-3 px-4 py-2 border-t border-gray-200 bg-white">
                              <div class="flex items-center gap-2 text-[10px] font-bold">
                                <span class="rounded-full bg-emerald-50 px-2 py-1 text-emerald-700">+{diffLineStats(conversation.liveDiffs[turn.id]).added}</span>
                                <span class="rounded-full bg-red-50 px-2 py-1 text-red-700">-{diffLineStats(conversation.liveDiffs[turn.id]).removed}</span>
                              </div>
                              <button class="rounded-lg border border-gray-200 bg-white px-3 py-1.5 text-[10px] font-bold text-gray-700 hover:bg-gray-50 transition-colors" onclick={() => openLiveDiffTab(turn.id, conversation!.liveDiffs[turn.id]!)} type="button">{ui.openTab}</button>
                            </div>
                            {#if parseAggregatedDiffViews(conversation.liveDiffs[turn.id]).length > 0}
                              <div class="border-t border-gray-200 bg-white">
                                {#each parseAggregatedDiffViews(conversation.liveDiffs[turn.id]) as change}
                                  <div class="border-b border-gray-100 last:border-b-0">
                                    <button class="flex w-full items-center justify-between gap-3 px-4 py-2 text-left hover:bg-gray-50 transition-colors" onclick={() => toggleFileChangeEntry(turn.id, `live-diff:${turn.id}`, change)} type="button">
                                      <div class="min-w-0">
                                        <p class="truncate text-[11px] font-bold text-gray-700">{change.path}</p>
                                        <p class="mt-0.5 text-[10px] uppercase tracking-widest text-gray-400">{change.kind}</p>
                                      </div>
                                      <ChevronDown size={12} class="text-gray-300 {isFileChangeEntryExpanded(turn.id, `live-diff:${turn.id}`, change) ? 'rotate-180' : ''} transition-transform" />
                                    </button>
                                    {#if isFileChangeEntryExpanded(turn.id, `live-diff:${turn.id}`, change)}
                                      <div class="border-t border-gray-100 bg-gray-50">
                                        {#if change.renderable}
                                          <MonacoDiffEditor fallbackText={change.diff} height={400} modified={change.modified} original={change.original} path={change.path} />
                                        {:else}
                                          <pre class="p-4 text-xs font-mono overflow-x-auto text-gray-600">{change.diff}</pre>
                                        {/if}
                                      </div>
                                    {/if}
                                  </div>
                                {/each}
                              </div>
                            {:else}
                              <pre class="p-4 text-xs font-mono overflow-x-auto text-gray-600 bg-gray-50/30 border-t border-gray-200">{conversation.liveDiffs[turn.id]}</pre>
                            {/if}
                          </details>
                        {/if}
                      </div>
                    </div>
                  </div>
                {/each}

                {#if visibleOptimisticMessage}
                  <div class="flex flex-col items-end gap-3 max-w-[85%] ml-auto opacity-70">
                    <div class="px-5 py-3 bg-gray-100 rounded-2xl text-gray-800 shadow-sm border border-gray-200/50">
                      <MarkdownMessage compact text={visibleOptimisticMessage.prompt} />
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
                              <button class="flex-1 px-4 py-2 bg-amber-600 text-white rounded-xl text-xs font-bold hover:bg-amber-700 shadow-sm transition-all" onclick={() => void resolvePendingRequest(request, { decision: "accept" })}>Approve</button>
                              <button class="flex-1 px-4 py-2 bg-amber-100 text-amber-700 border border-amber-200 rounded-xl text-xs font-bold hover:bg-amber-200 transition-all" onclick={() => void resolvePendingRequest(request, { decision: "acceptForSession" })}>Session</button>
                              <button class="flex-1 px-4 py-2 bg-gray-100 text-gray-600 rounded-xl text-xs font-bold hover:bg-gray-200 transition-all" onclick={() => void resolvePendingRequest(request, { decision: "decline" })}>Decline</button>
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
          <div class="flex-shrink-0 px-6 pb-6 pt-4 bg-gradient-to-t from-white via-white/95 to-transparent z-10">
            <div class="max-w-3xl mx-auto w-full space-y-4">
              {#if pendingSteerResume && pendingSteerResume.sessionId === selectedSessionId}
                <div class="p-4 bg-amber-600 text-white rounded-2xl shadow-xl flex flex-col md:flex-row items-center gap-4 animate-in slide-in-from-bottom-8 duration-500">
                  <div class="flex-1"><p class="text-sm font-bold flex items-center gap-2"><Clock size={16} /> {ui.savedDraftFound}</p><p class="text-xs opacity-90 mt-0.5">{ui.resumeSavedSteeringPrompt}</p></div>
                  <div class="flex gap-2"><button class="px-4 py-1.5 bg-white text-amber-700 rounded-lg text-xs font-bold hover:bg-amber-50 shadow-sm" onclick={() => void resumeSavedSteer()}>{ui.resume}</button><button class="px-4 py-1.5 bg-amber-700/50 hover:bg-amber-700 text-white rounded-lg text-xs font-bold transition-colors" onclick={keepSavedDraftInComposer}>{ui.keepDraft}</button><button class="p-1.5 text-white/70 hover:text-white rounded-lg transition-colors" onclick={() => void discardSavedDraft()}><X size={16} /></button></div>
                </div>
              {/if}

              {#if showQueueResumeBanner && conversation}
                <div class="p-4 bg-gray-900 text-white rounded-2xl shadow-xl flex flex-col md:flex-row items-center gap-4 animate-in slide-in-from-bottom-8 duration-500">
                  <div class="flex-1"><p class="text-sm font-bold flex items-center gap-2"><RefreshCw size={16} /> {ui.queuedWorkPaused}</p><p class="text-xs opacity-80 mt-0.5">{m.tasks_waiting({ count: String(conversation.queue.items.length) })}</p></div>
                  <div class="flex gap-2"><button class="px-4 py-1.5 bg-amber-600 text-white rounded-lg text-xs font-bold hover:bg-amber-700 shadow-sm" onclick={() => void resumeQueuedMessages()}>{ui.resumeQueue}</button><button class="px-4 py-1.5 bg-gray-800 hover:bg-gray-700 text-white rounded-lg text-xs font-bold transition-colors" onclick={() => { if (!selectedSessionId) return; dismissedQueueResumeBySessionId = { ...dismissedQueueResumeBySessionId, [selectedSessionId]: true }; }}>{ui.ignore}</button></div>
                </div>
              {/if}

              {#if queuedMessages.length > 0}
                <div class="border border-gray-200 rounded-2xl bg-gray-50/80 shadow-sm overflow-hidden">
                  <button
                    class={`flex w-full items-center justify-between gap-3 bg-white/80 px-3.5 py-2.5 text-left transition-colors hover:bg-white ${queuedFollowupsExpanded ? "border-b border-gray-200" : ""}`}
                    onclick={() => (queuedFollowupsExpanded = !queuedFollowupsExpanded)}
                    type="button"
                  >
                    <div>
                      <p class="text-[11px] font-bold text-gray-900 uppercase tracking-widest">{ui.queuedFollowups}</p>
                      <p class="mt-0.5 text-[10px] text-gray-500">{m.pending_count({ count: String(queuedMessages.length) })}</p>
                    </div>
                    <div class="flex items-center gap-2.5">
                      {#if conversation?.queue.resumeRequired}
                        <span class="px-2 py-1 rounded-full bg-amber-100 text-[10px] font-bold text-amber-700 uppercase tracking-widest">{ui.paused}</span>
                      {/if}
                      <ChevronDown size={15} class={`text-gray-400 transition-transform ${queuedFollowupsExpanded ? "rotate-180" : ""}`} />
                    </div>
                  </button>
                  {#if queuedFollowupsExpanded}
                    <div class="max-h-52 overflow-y-auto divide-y divide-gray-200 overscroll-contain">
                      {#each queuedMessages as item (item.id)}
                        <div class="px-3.5 py-2.5 flex flex-col gap-2 md:flex-row md:items-start md:justify-between">
                          <div class="min-w-0 flex-1 space-y-1">
                            <div class="flex items-center gap-2 text-[10px] font-bold uppercase tracking-widest text-gray-400">
                              <span>{ui.followUp}</span>
                              {#if item.attachmentNames.length > 0}
                                <span class="text-gray-300">•</span>
                                <span>{m.attached_files_count({ count: String(item.attachmentNames.length) })}</span>
                              {/if}
                            </div>
                            {#if editingQueueId === item.id}
                              <div class="space-y-2">
                                <textarea
                                  bind:value={editingQueuePrompt}
                                  class="w-full min-h-[4.25rem] rounded-xl border border-gray-200 bg-white px-3 py-2 text-sm text-gray-700 shadow-sm focus:border-amber-500 focus:outline-none focus:ring-2 focus:ring-amber-100"
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
                                  <div class="flex flex-wrap gap-1.5">
                                    {#each item.attachmentNames as attachmentName}
                                      <span class="rounded-full border border-gray-200 bg-white px-2 py-1 text-[11px] text-gray-500">{attachmentName}</span>
                                    {/each}
                                  </div>
                                {/if}
                                <p class="text-[11px] text-gray-400">{m.attached_files_stay()}</p>
                              </div>
                            {:else}
                              <p class="text-[13px] leading-5 text-gray-700 break-words">{summarizeQueueItem(item)}</p>
                            {/if}
                          </div>
                          <div class="flex flex-wrap items-center gap-2 md:justify-end">
                            {#if editingQueueId === item.id}
                              <button class="px-2.5 py-1.5 rounded-lg border border-emerald-200 bg-emerald-50 text-[11px] font-bold text-emerald-700 hover:bg-emerald-100 transition-colors disabled:opacity-50" disabled={sending} onclick={() => void saveQueuedMessage(item.id)} type="button">{ui.save}</button>
                              <button class="px-2.5 py-1.5 rounded-lg border border-gray-200 bg-white text-[11px] font-bold text-gray-700 hover:bg-gray-100 transition-colors disabled:opacity-50" disabled={sending} onclick={cancelQueuedMessageEdit} type="button">{ui.cancel}</button>
                            {:else}
                              <button class="px-2.5 py-1.5 rounded-lg border border-gray-200 bg-white text-[11px] font-bold text-gray-700 hover:bg-gray-100 transition-colors disabled:opacity-50" disabled={sending} onclick={() => beginQueuedMessageEdit(item)} type="button">{ui.edit}</button>
                              <button class="px-2.5 py-1.5 rounded-lg border border-amber-200 bg-amber-50 text-[11px] font-bold text-amber-700 hover:bg-amber-100 transition-colors disabled:opacity-50" disabled={sending} onclick={() => void dispatchQueuedMessage(item.id, "steer")} type="button">{ui.steerNow}</button>
                              <button class="px-2.5 py-1.5 rounded-lg border border-gray-200 bg-white text-[11px] font-bold text-gray-700 hover:bg-gray-100 transition-colors disabled:opacity-50" disabled={sending} onclick={() => void dispatchQueuedMessage(item.id, "message")} type="button">{ui.sendNow}</button>
                            {/if}
                            <button class="p-2 rounded-lg text-gray-400 hover:text-red-600 hover:bg-red-50 transition-colors disabled:opacity-50" aria-label={m.remove_queued_message()} disabled={sending} onclick={() => void removeQueuedMessage(item.id)} type="button"><Trash2 size={14} /></button>
                          </div>
                        </div>
                      {/each}
                    </div>
                  {/if}
                </div>
              {/if}

              {#if activeLiveTurnId}
                <div class="border border-amber-200 rounded-2xl bg-white shadow-sm overflow-hidden">
                  <button class="w-full flex items-start justify-between gap-3 px-3.5 py-2.5 hover:bg-amber-50/40 transition-colors" onclick={() => (liveTurnCardExpanded = !liveTurnCardExpanded)} type="button">
                    <div class="min-w-0 flex items-start gap-3">
                      <div class="flex h-8 w-8 shrink-0 items-center justify-center rounded-xl bg-amber-50 text-amber-700 border border-amber-100">
                        <Zap size={14} />
                      </div>
                      <div class="min-w-0 text-left">
                        <div class="flex flex-wrap items-center gap-1.5">
                          <p class="text-[11px] font-bold uppercase tracking-widest text-amber-700">{ui.liveTurn}</p>
                          {#if activeLiveTurnPlan}<span class="rounded-full bg-amber-50 px-2 py-0.5 text-[10px] font-bold text-amber-700">{m.steps_count({ count: String(activeLiveTurnPlan.plan.length) })}</span>{/if}
                          {#if activeLiveTurnDiff}
                            <span class="rounded-full bg-gray-100 px-2 py-0.5 text-[10px] font-bold text-gray-600">
                              {m.files_count({ count: String(activeLiveTurnDiffViews.length > 0 ? activeLiveTurnDiffViews.length : 1) })}
                            </span>
                          {/if}
                          {#if activeLiveTurnSubagents.length > 0}
                            <span class="rounded-full bg-sky-50 px-2 py-0.5 text-[10px] font-bold text-sky-700">
                              {activeLiveTurnSubagents.length} {ui.tasks}
                            </span>
                          {/if}
                        </div>
                        {#if activeLiveTurnSummary}
                          <p class="mt-0.5 truncate text-xs text-gray-500">{activeLiveTurnSummary}</p>
                        {/if}
                      </div>
                    </div>
                    <div class="flex items-center gap-1.5 shrink-0 self-center">
                      {#if activeLiveTurnDiff}
                        <span class="rounded-full bg-emerald-50 px-2 py-0.5 text-[10px] font-bold text-emerald-700">+{diffLineStats(activeLiveTurnDiff).added}</span>
                        <span class="rounded-full bg-red-50 px-2 py-0.5 text-[10px] font-bold text-red-700">-{diffLineStats(activeLiveTurnDiff).removed}</span>
                      {/if}
                      <ChevronDown size={14} class="text-gray-400 {liveTurnCardExpanded ? 'rotate-180' : ''} transition-transform" />
                    </div>
                  </button>
                  {#if liveTurnCardExpanded}
                    <div class="grid gap-2.5 border-t border-amber-100 bg-amber-50/20 p-2.5 {activeLiveTurnPlan && activeLiveTurnDiff && activeLiveTurnSubagents.length > 0 ? 'lg:grid-cols-2 xl:grid-cols-3' : 'lg:grid-cols-2'}">
                      {#if activeLiveTurnPlan}
                        <div class="rounded-xl border border-amber-100 bg-white/85 p-2.5">
                          <div class="mb-1.5 flex items-center gap-2 text-[10px] font-bold uppercase tracking-[0.18em] text-amber-700">
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
                      {#if activeLiveTurnDiff}
                        <div class="rounded-xl border border-gray-200 bg-white/85 overflow-hidden">
                          <div class="flex items-center justify-between gap-2 px-2.5 py-2 border-b border-gray-200">
                            <div class="flex items-center gap-2 text-[10px] font-bold uppercase tracking-[0.18em] text-gray-500">
                              <FileDiff size={12} />
                              <span>{ui.aggregatedDiff}</span>
                            </div>
                            <button class="rounded-lg border border-gray-200 bg-white px-2.5 py-1 text-[10px] font-bold text-gray-700 hover:bg-gray-50 transition-colors" onclick={() => openLiveDiffTab(activeLiveTurnId, activeLiveTurnDiff)} type="button">{ui.openTab}</button>
                          </div>
                          {#if activeLiveTurnDiffViews.length > 0}
                            <div class="max-h-72 overflow-auto">
                              {#each activeLiveTurnDiffViews as change (`${change.path}:${change.kind}`)}
                                <div class="border-t border-gray-100 first:border-t-0">
                                  <button class="flex w-full items-center justify-between gap-3 px-2.5 py-2 text-left hover:bg-gray-50 transition-colors" onclick={() => toggleFileChangeEntry(activeLiveTurnId, "live-diff:active", change)} type="button">
                                    <div class="min-w-0">
                                      <p class="truncate text-[11px] font-bold text-gray-700">{change.path}</p>
                                      <p class="mt-0.5 text-[10px] uppercase tracking-[0.18em] text-gray-400">{change.kind}</p>
                                    </div>
                                    <ChevronDown size={12} class="text-gray-300 {isFileChangeEntryExpanded(activeLiveTurnId, 'live-diff:active', change) ? 'rotate-180' : ''} transition-transform" />
                                  </button>
                                  {#if isFileChangeEntryExpanded(activeLiveTurnId, "live-diff:active", change)}
                                    <div class="border-t border-gray-100 bg-gray-50">
                                      {#if change.renderable}
                                        <MonacoDiffEditor fallbackText={change.diff} height={400} modified={change.modified} original={change.original} path={change.path} />
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
                      {#if activeLiveTurnSubagents.length > 0}
                        <div class="rounded-xl border border-sky-100 bg-white/85 overflow-hidden">
                          <div class="flex items-center justify-between gap-2 border-b border-sky-100 px-2.5 py-2">
                            <div class="flex items-center gap-2 text-[10px] font-bold uppercase tracking-[0.18em] text-sky-700">
                              <Bot size={12} />
                              <span>{ui.subagentActivities}</span>
                            </div>
                            <span class="rounded-full bg-sky-50 px-2 py-0.5 text-[10px] font-bold text-sky-700">
                              {activeLiveTurnSubagents.length}
                            </span>
                          </div>
                          <div class="max-h-72 overflow-auto">
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
                <form class="bg-white/95 border-2 border-gray-200 rounded-2xl shadow-2xl overflow-hidden transition-all duration-200 focus-within:-translate-y-0.5 focus-within:border-amber-400/70 focus-within:bg-white focus-within:shadow-[0_24px_60px_-34px_rgba(245,158,11,0.65)]" onsubmit={(event) => { event.preventDefault(); void submitComposer(); }}>
                  <textarea bind:this={composerTextareaElement} bind:value={draft} class="w-full min-h-[3rem] overflow-y-hidden border-none bg-transparent px-4 py-3 pr-12 text-sm leading-6 text-gray-800 placeholder-gray-400 transition-colors duration-150 focus:ring-0 focus:placeholder:text-amber-500/70 resize-none sm:min-h-[3.25rem]" oninput={handleComposerInput} onkeydown={handleComposerKeydown} placeholder={queueModeActive ? ui.queueFollowUpPlaceholder : ui.askCodex} rows="1"></textarea>
                  
                  {#if draftAttachments.length > 0}
                    <div class="px-4 pb-2 flex flex-wrap gap-2">
                      {#each draftAttachments as attachment (attachment.id)}<button class="px-3 py-1.5 bg-gray-50 border border-gray-200 rounded-xl text-[11px] font-bold text-gray-600 hover:bg-red-50 hover:text-red-600 hover:border-red-200 transition-all flex items-center gap-2 group" onclick={() => void removeDraftAttachment(attachment.id)} type="button"><FileText size={12} /><span>{attachment.originalName}</span><X size={12} class="opacity-0 group-hover:opacity-100 transition-opacity" /></button>{/each}
                    </div>
                  {/if}

                  <div class="flex flex-wrap items-center gap-2 border-t border-gray-100 bg-gray-50/80 px-3 py-2.5 transition-colors duration-200 group-focus-within:border-amber-100 group-focus-within:bg-[linear-gradient(180deg,rgba(255,251,235,0.9),rgba(255,255,255,0.98))] sm:px-4 sm:py-3">
                    <div class="flex min-w-0 flex-1 items-center gap-1.5 sm:gap-2">
                      <input bind:this={filePickerElement} disabled={uploading} hidden multiple onchange={(event) => void uploadFiles((event.currentTarget as HTMLInputElement).files)} type="file" />
                      <button class="rounded-xl p-1.5 text-gray-400 transition-all hover:bg-amber-50 hover:text-amber-600 group-focus-within:bg-white/90 group-focus-within:text-amber-700 sm:p-2" disabled={uploading} onclick={promptAttachmentPicker} title={ui.addAttachments} type="button">{#if uploading}<RefreshCw size={18} class="animate-spin" />{:else}<Paperclip size={18} />{/if}</button>
                      {#if conversation}
                        <div class="mx-0.5 hidden h-4 w-px bg-gray-200 sm:block"></div>
                        <button class="flex min-w-0 max-w-[8.5rem] items-center gap-1.5 rounded-xl border border-transparent px-2.5 py-1 text-[10px] font-bold text-gray-500 transition-all hover:border-gray-200 hover:bg-white hover:text-gray-900 group-focus-within:border-amber-100 group-focus-within:bg-white/90 group-focus-within:text-gray-700 sm:max-w-[11rem] sm:gap-2 sm:px-3 sm:py-1.5 sm:text-[11px]" onclick={() => { composerSettingsOpen = !composerSettingsOpen; composerSecurityOpen = false; }} type="button">
                          <span class="truncate">{composerSettingsSummary.model}</span>
                          <ChevronDown size={14} class={`shrink-0 transition-transform ${composerSettingsOpen ? "rotate-180" : ""}`} />
                        </button>
                        <button class="flex shrink-0 items-center gap-1.5 rounded-xl border border-transparent px-2.5 py-1 text-[10px] font-bold text-gray-500 transition-all hover:border-gray-200 hover:bg-white hover:text-gray-900 group-focus-within:border-amber-100 group-focus-within:bg-white/90 group-focus-within:text-gray-700 sm:gap-2 sm:px-3 sm:py-1.5 sm:text-[11px]" onclick={() => { composerSecurityOpen = !composerSecurityOpen; composerSettingsOpen = false; }} title={ui.securitySession} type="button"><Zap size={14} /><span class="hidden sm:inline">{ui.securitySession}</span></button>
                      {/if}
                    </div>
                    <div class="flex w-full items-center justify-end gap-1.5 sm:w-auto sm:gap-2">
                      {#if running}
                        <button class="rounded-xl px-3 py-1.5 text-[11px] font-bold text-red-600 transition-all hover:bg-red-50 sm:px-4 sm:py-2 sm:text-xs" onclick={interruptTurn} type="button">{ui.stop}</button>
                        <button class="rounded-xl border border-amber-200 bg-amber-50 px-3 py-1.5 text-[11px] font-bold text-amber-700 transition-all hover:bg-amber-100 disabled:opacity-50 sm:px-4 sm:py-2 sm:text-xs" disabled={sending || (!draft.trim() && draftAttachments.length === 0)} onclick={steerTurn} type="button">{ui.steer}</button>
                      {/if}
                      <button class="rounded-xl bg-gray-900 px-4 py-1.5 text-[11px] font-bold text-white shadow-lg shadow-gray-200 transition-all hover:bg-gray-800 disabled:opacity-50 disabled:shadow-none active:scale-[0.98] sm:px-6 sm:py-2 sm:text-xs" disabled={sending || (!draft.trim() && draftAttachments.length === 0)} onclick={() => void submitComposer()} type="button"><div class="flex items-center gap-1.5 sm:gap-2"><span>{queueModeActive ? ui.queue : ui.send}</span><Send size={14} /></div></button>
                    </div>
                  </div>
                </form>

                <!-- Settings Popovers -->
                {#if composerSettingsOpen && conversation}
                  <div class="absolute bottom-24 left-0 w-80 bg-white border border-gray-200 rounded-2xl shadow-2xl p-4 space-y-4 z-50 animate-in slide-in-from-bottom-4 duration-300">
                    <div class="flex items-center justify-between border-b border-gray-100 pb-3 mb-2"><h3 class="text-xs font-bold text-gray-400 uppercase tracking-widest">{ui.composerSettings}</h3><button class="text-gray-400 hover:text-gray-600" onclick={() => (composerSettingsOpen = false)}><X size={16} /></button></div>
                    <div class="grid grid-cols-1 gap-4">
                      <div class="space-y-1"><label class="text-[10px] font-bold text-gray-400 uppercase tracking-widest px-1" for="composer-model-select">{ui.model}</label><select class="w-full px-3 py-2 bg-gray-50 border border-gray-200 rounded-xl text-sm focus:outline-none focus:ring-2 focus:ring-amber-500/10 focus:border-amber-500 transition-all" id="composer-model-select" onchange={(event) => setPreference("model", (event.currentTarget as HTMLSelectElement).value || null)} value={conversation.preferences.model ?? ""}><option value="">{ui.autoDefault}</option>{#each config?.models ?? [] as model}<option value={model.id}>{model.displayName}</option>{/each}</select></div>
                      <div class="space-y-1"><label class="text-[10px] font-bold text-gray-400 uppercase tracking-widest px-1" for="composer-effort-select">{m.reasoning()}</label><select class="w-full px-3 py-2 bg-gray-50 border border-gray-200 rounded-xl text-sm focus:outline-none focus:ring-2 focus:ring-amber-500/10 focus:border-amber-500 transition-all" id="composer-effort-select" onchange={(event) => setPreference("effort", (event.currentTarget as HTMLSelectElement).value as SessionPreferences["effort"])} value={conversation.preferences.effort ?? (reasoningOptions[0] ?? "medium")}>{#each reasoningOptions as option}<option value={option}>{option}</option>{/each}</select></div>
                    </div>
                  </div>
                {/if}
                {#if composerSecurityOpen && conversation}
                  <div class="absolute bottom-24 left-0 w-80 bg-white border border-gray-200 rounded-2xl shadow-2xl p-4 space-y-4 z-50 animate-in slide-in-from-bottom-4 duration-300">
                    <div class="flex items-center justify-between border-b border-gray-100 pb-3 mb-2"><h3 class="text-xs font-bold text-gray-400 uppercase tracking-widest">{ui.securitySession}</h3><button class="text-gray-400 hover:text-gray-600" onclick={() => (composerSecurityOpen = false)}><X size={16} /></button></div>
                    <div class="space-y-4">
                      <div class="space-y-1"><label class="text-[10px] font-bold text-gray-400 uppercase tracking-widest px-1" for="composer-approval-select">{ui.approvalMode}</label><select class="w-full px-3 py-2 bg-gray-50 border border-gray-200 rounded-xl text-sm focus:outline-none focus:ring-2 focus:ring-amber-500/10 focus:border-amber-500 transition-all" id="composer-approval-select" onchange={(event) => setPreference("autoApproveMode", (event.currentTarget as HTMLSelectElement).value as SessionPreferences["autoApproveMode"])} value={conversation.preferences.autoApproveMode ?? "manual"}><option value="manual">{ui.manual}</option><option value="turn">{ui.autoOnce}</option><option value="session">{ui.autoSession}</option></select></div>
                      <label class="checkbox-card" for="network-access">
                        <input
                          class="checkbox-input"
                          checked={conversation.preferences.networkAccess ?? false}
                          onchange={(event) => setPreference("networkAccess", (event.currentTarget as HTMLInputElement).checked)}
                          type="checkbox"
                          id="network-access"
                        />
                        <span aria-hidden="true" class="checkbox-control"></span>
                        <span class="checkbox-copy">
                          <span class="checkbox-title">{ui.allowNetworkAccess}</span>
                        </span>
                      </label>
                      <label class:checkbox-card--disabled={!config?.systemShutdown.available} class="checkbox-card" for="shutdown-on-completion">
                        <input
                          class="checkbox-input"
                          checked={conversation.preferences.shutdownOnCompletion ?? false}
                          disabled={!config?.systemShutdown.available}
                          onchange={(event) => setPreference("shutdownOnCompletion", (event.currentTarget as HTMLInputElement).checked)}
                          type="checkbox"
                          id="shutdown-on-completion"
                        />
                        <span aria-hidden="true" class="checkbox-control"></span>
                        <span class="checkbox-copy">
                          <span class="checkbox-title">{ui.shutdownAfterQueueCompletes}</span>
                          <span class="checkbox-description">{m.shutdown_wait_description({ seconds: String(config?.systemShutdown.delaySeconds ?? 30) })}</span>
                        </span>
                      </label>
                    </div>
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
            {#if subagentTasks.length === 0}<div class="py-24 text-center"><div class="w-16 h-16 bg-gray-100 rounded-3xl flex items-center justify-center mx-auto mb-4 text-gray-300"><History size={32} /></div><p class="text-gray-500">{ui.noActiveTasks}</p></div>
            {:else}<div class="grid grid-cols-1 gap-4">{#each subagentTasks as task (task.key)}<div class="bg-white border border-gray-200 rounded-2xl p-5 shadow-sm hover:shadow-md transition-all group flex items-start justify-between gap-4"><div class="flex items-center gap-4"><div class="w-10 h-10 bg-amber-50 text-amber-600 rounded-xl flex items-center justify-center shadow-inner"><Bot size={20} /></div><div><h4 class="font-bold text-gray-900">{task.tool}</h4><p class="text-[10px] font-bold text-amber-600 uppercase tracking-widest mt-0.5">{task.status}</p></div></div>{#if task.primaryThreadId}<button class="px-4 py-2 bg-white border border-gray-200 rounded-xl text-xs font-bold text-gray-700 hover:bg-gray-50 transition-all shadow-sm" onclick={() => void openSubagentThread(task.primaryThreadId ?? "")}>{ui.openThread}</button>{/if}</div>{/each}</div>{/if}
          </div>
        </div>
      {:else if activeWorkspaceTabId === "settings"}
        <div class="h-full overflow-y-auto bg-gray-50/30 p-5 sm:p-8">
          <div class="mx-auto w-full max-w-7xl">
            <SettingsWorkspace
              codexHome={config?.paths.codexHome ?? ""}
              configFilePath={config?.paths.configFilePath ?? ""}
              onConfigSaved={async () => {
                config = await api.getConfig();
                syncStartupAlertModal(config);
              }}
            />
          </div>
        </div>
      {:else if activeWorkspaceTabId === "git"}
        <GitWorkspace
          onOpenCommitDiff={openGitCommitDiffTab}
          onOpenDiffTab={openGitDiffTab}
          onSelectRepo={handleRepoSelect}
          selectedRepoPath={conversation?.preferences.gitRepoPath ?? null}
        />
      {:else if activeGitDiffTab}
        <GitWorkspace
          onOpenCommitDiff={openGitCommitDiffTab}
          onOpenDiffTab={openGitDiffTab}
          onSelectRepo={(repoPath) => handleGitDiffTabRepoSelect(activeGitDiffTab.id, repoPath)}
          openRequest={activeGitDiffTab.request}
          selectedRepoPath={activeGitDiffTab.repoPath}
        />
      {:else if activeCodeDiffTab}
        <div class="h-full overflow-y-auto bg-gray-50/30 p-8"><div class="max-w-5xl mx-auto space-y-8"><div class="flex items-end justify-between"><div><h2 class="text-2xl font-bold text-gray-900">{activeCodeDiffTab.title}</h2><p class="text-sm text-gray-500 mt-1">{m.files_count({ count: String(activeCodeDiffTab.views.length) })}</p></div><button class="p-2 text-gray-400 hover:text-red-600 rounded-xl transition-all" onclick={() => closeCodeDiffTab(activeCodeDiffTab.id)}><X size={20} /></button></div><div class="space-y-6">{#each activeCodeDiffTab.views as change}<div class="bg-white border border-gray-200 rounded-2xl overflow-hidden shadow-sm"><div class="px-5 py-3 bg-gray-50 border-b border-gray-200 flex items-center justify-between"><div class="flex items-center gap-3"><span class="text-sm font-bold text-gray-900">{change.path}</span><span class="px-2 py-0.5 bg-amber-100 text-[10px] font-bold text-amber-700 rounded uppercase tracking-widest">{change.kind}</span></div></div><div class="p-0">{#if change.renderable}<MonacoDiffEditor fallbackText={change.diff} height={400} modified={change.modified} original={change.original} path={change.path} />{:else}<pre class="p-6 text-xs font-mono text-gray-600 bg-gray-50/50 overflow-x-auto">{change.diff}</pre>{/if}</div></div>{/each}</div></div></div>
      {:else}
        <TerminalWorkspace terminalId={activeWorkspaceTabId.replace(/^terminal:/u, "")} />
      {/if}
    </div>
  </main>
</div>

{#if startupAlertModalOpen && config && (startupPausedQueues.length > 0 || startupScheduledShutdown)}
  <div
    aria-labelledby="startup-alert-title"
    aria-modal="true"
    class="fixed inset-0 z-[115] overflow-y-auto bg-gray-950/70 backdrop-blur-xl"
    role="dialog"
  >
    <div class="flex min-h-full items-center justify-center p-4 sm:p-8">
      <div class="w-full max-w-4xl overflow-hidden rounded-[2rem] border border-white/10 bg-white shadow-2xl">
        <div class="border-b border-gray-200 bg-gradient-to-br from-amber-50 via-white to-white px-6 py-6 sm:px-8">
          <div class="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
            <div class="space-y-2">
              <div class="inline-flex h-10 w-10 items-center justify-center rounded-2xl bg-amber-100 text-amber-700 shadow-sm">
                <AlertCircle size={18} />
              </div>
              <div>
                <h2 id="startup-alert-title" class="text-2xl font-bold tracking-tight text-gray-950">
                  {ui.startupAlertTitle}
                </h2>
                <p class="mt-2 max-w-2xl text-sm leading-relaxed text-gray-600">
                  {ui.startupAlertDescription}
                </p>
              </div>
            </div>
            <button
              class="inline-flex items-center justify-center rounded-2xl border border-gray-200 px-4 py-2 text-sm font-semibold text-gray-700 transition-colors hover:bg-gray-50"
              onclick={dismissStartupAlertModal}
              type="button"
            >
              {ui.startupAlertContinue}
            </button>
          </div>
        </div>

        <div class="grid gap-6 p-6 sm:p-8 {(startupPausedQueues.length > 0 && startupScheduledShutdown) ? 'sm:grid-cols-[minmax(0,1.2fr)_minmax(0,0.8fr)]' : ''}">
          {#if startupPausedQueues.length > 0}
            <div class="space-y-4">
              <div class="rounded-3xl border border-gray-200 bg-gray-50/70 p-5 shadow-sm">
                <div class="flex items-start justify-between gap-4">
                  <div>
                    <h3 class="text-sm font-bold text-gray-900">{ui.startupAlertPausedQueues}</h3>
                    <p class="mt-1 text-sm leading-relaxed text-gray-500">
                      {ui.startupAlertPausedQueuesDescription}
                    </p>
                  </div>
                  <span class="inline-flex items-center rounded-full bg-amber-100 px-3 py-1 text-[10px] font-bold uppercase tracking-[0.2em] text-amber-700">
                    {startupPausedQueues.length}
                  </span>
                </div>

                <div class="mt-5 space-y-3">
                  {#each startupPausedQueues as queueAlert (queueAlert.sessionId)}
                    <div class="rounded-2xl border border-gray-200 bg-white p-4 shadow-sm">
                      <div class="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
                        <div class="min-w-0">
                          <div class="truncate text-sm font-semibold text-gray-900">
                            {queueAlert.name || getDefaultThreadTitle()}
                          </div>
                          <div class="mt-1 text-xs text-gray-500">
                            {ui.startupAlertPendingTasks(queueAlert.pendingCount)}
                          </div>
                          <div class="mt-2 truncate text-[11px] text-gray-400">
                            {queueAlert.cwd}
                          </div>
                        </div>
                        <button
                          class="inline-flex items-center justify-center rounded-2xl border border-amber-200 bg-amber-50 px-4 py-2 text-sm font-semibold text-amber-700 transition-colors hover:bg-amber-100"
                          onclick={() => void openStartupAlertSession(queueAlert.sessionId)}
                          type="button"
                        >
                          {ui.startupAlertOpenThread}
                        </button>
                      </div>
                    </div>
                  {/each}
                </div>
              </div>
            </div>
          {/if}

          {#if startupScheduledShutdown}
            <div class="space-y-4">
              <div class="rounded-3xl border border-amber-200 bg-gradient-to-br from-amber-50 to-white p-5 shadow-sm">
                <div class="flex items-center gap-3">
                  <div class="flex h-10 w-10 items-center justify-center rounded-2xl bg-amber-100 text-amber-700">
                    <Clock size={18} />
                  </div>
                  <div>
                    <h3 class="text-sm font-bold text-gray-900">{ui.startupAlertScheduledShutdown}</h3>
                    {#if startupShutdownRemainingSeconds !== null}
                      <p class="mt-1 text-sm text-gray-600">
                        {ui.startupAlertShutdownCountdown(startupShutdownRemainingSeconds)}
                      </p>
                    {/if}
                  </div>
                </div>
                {#if startupScheduledShutdown.sessionId}
                  <div class="mt-4 rounded-2xl border border-white/80 bg-white/80 px-4 py-3 text-sm text-gray-600 shadow-sm">
                    {ui.startupAlertShutdownThread(
                      sessions.find((session) => session.id === startupScheduledShutdown.sessionId)?.name ||
                        (conversation?.thread.id === startupScheduledShutdown.sessionId
                          ? getDisplayThreadTitle(conversation?.thread.name, conversation?.thread.preview) || getDefaultThreadTitle()
                          : startupScheduledShutdown.sessionId)
                    )}
                  </div>
                {/if}
                <p class="mt-4 text-xs leading-relaxed text-gray-500">
                  {m.shutdown_wait_description({ seconds: String(config.systemShutdown.delaySeconds) })}
                </p>
              </div>
            </div>
          {/if}
        </div>
      </div>
    </div>
  </div>
{/if}

{#if isMobileLayout && mobileSidebarOpen}
  <button
    aria-label={ui.closeThreadList}
    class="fixed inset-0 bg-gray-900/40 backdrop-blur-sm z-30 transition-all"
    onclick={closeMobileSidebar}
    type="button"
  ></button>
{/if}

{#if browserOpen}
  <div class="fixed inset-0 z-[100] flex items-center justify-center p-6">
    <button
      aria-label="Close folder picker"
      class="absolute inset-0 bg-gray-900/60 backdrop-blur-md"
      onclick={() => (browserOpen = false)}
      type="button"
    ></button>
    <div class="relative bg-white rounded-3xl shadow-2xl w-full max-w-2xl max-h-[80vh] overflow-hidden flex flex-col">
      <header class="px-8 py-6 border-b border-gray-100 flex items-center justify-between"><div><h2 class="text-xl font-bold text-gray-900">Select working folder</h2><p class="text-[10px] font-bold text-gray-400 uppercase tracking-widest mt-1">Allowed root paths</p></div><button class="p-2 text-gray-400 hover:text-gray-900 hover:bg-gray-100 rounded-xl transition-all" onclick={() => (browserOpen = false)}><X size={20} /></button></header>
      <div class="flex-1 overflow-y-auto p-6">
        {#if browserBusy}<div class="py-24 flex flex-col items-center gap-4 text-gray-400"><RefreshCw size={32} class="animate-spin" /><p class="text-sm font-medium">Scanning...</p></div>
        {:else if directoryPayload}
          <div class="space-y-4">
            <div class="flex items-center gap-2 mb-6"><button class="p-2 bg-gray-100 text-gray-600 rounded-lg hover:bg-gray-200 disabled:opacity-30 transition-all" disabled={!directoryPayload?.parentPath} onclick={() => directoryPayload?.parentPath && void browseTo(directoryPayload.parentPath)}><ChevronUp size={18} /></button><div class="flex-1 px-4 py-2 bg-gray-50 border border-gray-200 rounded-xl text-xs font-mono text-gray-600 truncate">{directoryPayload.currentPath}</div></div>
            <div class="grid grid-cols-1 gap-1">{#each directoryPayload.entries as entry (entry.path)}<button class="flex items-center justify-between p-3 rounded-xl hover:bg-gray-50 transition-all text-left group" onclick={() => void browseTo(entry.path)}><div class="flex items-center gap-3"><div class="p-2 bg-amber-50 text-amber-600 rounded-lg group-hover:bg-amber-100"><Layout size={16} /></div><div><span class="text-sm font-semibold text-gray-700">{entry.name}</span><p class="text-[10px] text-gray-400 font-mono mt-0.5">{entry.path}</p></div></div><ChevronDown size={14} class="-rotate-90 text-gray-300 group-hover:text-gray-500" /></button>{/each}</div>
          </div>
        {/if}
      </div>
      <div class="px-8 py-4 bg-gray-50 border-t border-gray-100 flex items-center justify-end"><button class="px-6 py-2 bg-amber-600 text-white rounded-xl text-xs font-bold hover:bg-amber-700 shadow-md transition-all active:scale-95" onclick={() => (browserOpen = false)}>{ui.selectFolder}</button></div>
    </div>
  </div>
{/if}
{/if}

<style>
  @keyframes thinking-shimmer {
    0% {
      background-position: 200% 50%;
      opacity: 0.72;
    }

    50% {
      opacity: 1;
    }

    100% {
      background-position: -15% 50%;
      opacity: 0.78;
    }
  }

  .thinking-indicator {
    backdrop-filter: blur(10px);
  }

  .thinking-indicator__label {
    background-image: linear-gradient(90deg, rgba(107, 114, 128, 0.62) 0%, rgba(17, 24, 39, 0.96) 32%, rgba(217, 119, 6, 0.88) 52%, rgba(17, 24, 39, 0.96) 70%, rgba(107, 114, 128, 0.62) 100%);
    background-size: 220% 100%;
    background-clip: text;
    -webkit-background-clip: text;
    color: transparent;
    animation: thinking-shimmer 1.9s linear infinite;
  }
  .turn-card-header {
    position: sticky;
    top: 0.35rem;
    z-index: 4;
    backdrop-filter: blur(12px);
  }

  .turn-card-header--neutral {
    background: linear-gradient(180deg, rgba(255, 255, 255, 0.98), rgba(249, 250, 251, 0.94));
  }

  .turn-card-header--amber {
    background: linear-gradient(180deg, rgba(255, 251, 235, 0.98), rgba(255, 247, 237, 0.94));
  }

</style>

{#snippet renderTurnItem(turnId: string, item: CodexItem)}
  {#if item.type === "agentMessage"}
    <div class="space-y-2 group/agent-message">
      <div class="flex justify-end opacity-0 group-hover/agent-message:opacity-100 transition-opacity">
        <button class="p-1.5 rounded-lg text-gray-400 hover:text-gray-700 hover:bg-gray-100 transition-colors" onclick={() => void copyMessageText(String(item.text ?? ""))} title={ui.copyReply} type="button"><Copy size={13} /></button>
      </div>
      <div class="prose prose-sm max-w-none text-gray-800 leading-relaxed animate-in fade-in slide-in-from-left-2 duration-700"><MarkdownMessage on:openLocalPath={(event: CustomEvent<{ href: string }>) => void openGitFileFromMessage(event.detail.href)} text={String(item.text ?? "")} /></div>
    </div>
  {:else if item.type === "reasoning"}
    <div class="overflow-hidden rounded-2xl border border-amber-100 bg-amber-50/40 shadow-sm">
      <div class="turn-card-header turn-card-header--amber flex items-start gap-3 border-b border-amber-100 px-4 py-3">
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
            <MarkdownMessage compact on:openLocalPath={(event: CustomEvent<{ href: string }>) => void openGitFileFromMessage(event.detail.href)} text={String(item.text ?? "")} />
          </div>
        {/if}
        {#if Array.isArray(item.summary) && item.summary.length > 0}
          <div class="space-y-2">
            {#each item.summary as summaryEntry, index (`${item.id}:summary:${index}`)}
              <div class="rounded-xl border border-amber-100/70 bg-amber-50/60 px-3 py-2.5 text-sm leading-relaxed text-gray-700">
                <MarkdownMessage compact on:openLocalPath={(event: CustomEvent<{ href: string }>) => void openGitFileFromMessage(event.detail.href)} text={summaryEntry} />
              </div>
            {/each}
          </div>
        {/if}
      </div>
    </div>
  {:else if item.type === "plan"}
    <div class="border border-amber-100 rounded-2xl bg-amber-50/20 overflow-hidden shadow-sm"><div class="turn-card-header turn-card-header--amber px-4 py-2.5 border-b border-amber-100 flex items-center gap-3"><ListTodo size={14} class="text-amber-700" /><span class="text-[10px] font-bold text-amber-700 uppercase tracking-widest">{ui.plannedStrategy}</span></div><pre class="p-5 text-xs font-mono text-gray-700 leading-relaxed whitespace-pre-wrap">{String(item.text ?? "")}</pre></div>
  {:else if item.type === "contextCompaction"}
    {@const contextCompressionRunning = isContextCompactionRunning(turnId, item)}
    <div class="overflow-hidden rounded-2xl border border-amber-200 bg-gradient-to-br from-amber-50/80 via-white to-white shadow-sm">
      <div class="turn-card-header turn-card-header--amber flex items-center justify-between gap-3 border-b border-amber-100 px-4 py-3">
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
    <div class="border border-gray-200 rounded-2xl bg-white overflow-hidden shadow-sm hover:shadow-md transition-shadow">
      <button class="turn-card-header turn-card-header--neutral w-full flex items-center justify-between px-4 py-3 hover:bg-gray-50 transition-colors" onclick={() => void toggleToolItem(turnId, item.id)}>
        <div class="flex items-center gap-3"><div class="p-2 bg-gray-100 text-gray-500 rounded-xl">{#if item.type === 'commandExecution'}<Terminal size={14} />{:else if item.type === 'fileChange'}<FileDiff size={14} />{:else if item.type === 'webSearch'}<Layout size={14} />{:else}<Zap size={14} />{/if}</div><div class="text-left"><h4 class="text-xs font-bold text-gray-900 leading-tight">{getToolItemLabel(item)}</h4><p class="text-[10px] text-gray-500 mt-0.5 font-medium">{getToolItemSummary(item) || ui.executing}</p></div></div>
        <div class="flex items-center gap-3">{#if item.type === "commandExecution" && item.exitCode !== null}<span class="px-1.5 py-0.5 bg-gray-100 text-[9px] font-bold text-gray-500 rounded uppercase tracking-tighter">Exit {item.exitCode}</span>{/if}<ChevronDown size={14} class="text-gray-400 {isItemExpanded(turnId, item.id) ? 'rotate-180' : ''} transition-transform" /></div>
      </button>
      {#if isItemExpanded(turnId, item.id)}
        <div class="p-0 border-t border-gray-100 animate-in slide-in-from-top-2 duration-300">
          {#if isItemDetailLoading(turnId, item.id)}<div class="p-8 flex items-center justify-center gap-2 text-gray-400 text-xs italic"><RefreshCw size={16} class="animate-spin" />{ui.fetching}</div>
          {:else if getItemDetailError(turnId, item.id)}<div class="p-4 bg-red-50 text-red-600 text-xs border-t border-red-100">{getItemDetailError(turnId, item.id)}</div>
          {:else if item.type === "fileChange" && getFileChangeViews(item).length > 0}
            <div class="p-0 space-y-0">{#each getFileChangeViews(item) as change}<div class="border-b border-gray-100 last:border-0"><button class="w-full flex items-center justify-between px-5 py-2 hover:bg-gray-50 transition-colors" onclick={() => toggleFileChangeEntry(turnId, item.id, change)}><div class="flex items-center gap-2"><span class="text-[10px] font-mono font-bold text-gray-600">{change.path}</span><span class="px-1.5 py-0.5 bg-gray-100 text-[9px] font-bold text-gray-400 rounded uppercase tracking-tighter">{change.kind}</span></div><ChevronDown size={12} class="text-gray-300 {isFileChangeEntryExpanded(turnId, item.id, change) ? 'rotate-180' : ''} transition-transform" /></button>{#if isFileChangeEntryExpanded(turnId, item.id, change)}<div class="bg-gray-50 p-0 border-t border-gray-100">{#if change.renderable}<MonacoDiffEditor fallbackText={change.diff} height={400} modified={change.modified} original={change.original} path={change.path} />{:else}<pre class="p-4 text-[10px] font-mono text-gray-600 overflow-x-auto">{change.diff}</pre>{/if}</div>{/if}</div>{/each}</div>
          {:else if getDeferredToolBody(item)}<pre class="p-4 bg-gray-50 text-[11px] font-mono text-gray-700 overflow-x-auto leading-relaxed">{getDeferredToolBody(item)}</pre>
          {:else}<div class="p-4 text-gray-400 text-xs italic text-center">{ui.noAdditionalOutput}</div>{/if}
        </div>
      {/if}
    </div>
  {:else if item.type === "collabAgentToolCall"}
    <div class="border border-amber-200 rounded-2xl bg-white overflow-hidden shadow-sm group">
      <div class="turn-card-header turn-card-header--amber px-4 py-3 flex items-center justify-between gap-4">
        <div class="flex items-center gap-3"><div class="p-2 bg-white border border-amber-200 text-amber-600 rounded-xl group-hover:bg-amber-600 group-hover:text-white transition-all shadow-sm"><Bot size={16} /></div><div><h4 class="text-xs font-bold text-gray-900 leading-tight">{ui.subagentInvocation}</h4><div class="flex items-center gap-2 mt-0.5"><span class="text-[10px] font-bold text-amber-600 uppercase tracking-widest">{item.tool}</span><span class="w-1 h-1 rounded-full bg-amber-200"></span><span class="text-[10px] text-gray-500 font-medium uppercase tracking-tighter">{item.status}</span></div></div></div>
        {#if getPrimarySubagentThreadId(item)}<button class="px-3 py-1.5 bg-white border border-amber-200 rounded-lg text-[10px] font-bold text-amber-700 hover:bg-amber-600 hover:text-white transition-all shadow-sm" onclick={() => void openSubagentThread(getPrimarySubagentThreadId(item) ?? "")}>{ui.viewThread}</button>{/if}
      </div>
      {#if item.prompt}<div class="p-4 border-t border-amber-100 bg-white"><p class="text-[10px] font-bold text-gray-400 uppercase tracking-widest mb-2">{ui.instructions}</p><pre class="text-[11px] font-mono text-gray-600 leading-relaxed whitespace-pre-wrap italic">{String(item.prompt)}</pre></div>{/if}
    </div>
  {/if}
{/snippet}

{#snippet renderTurnEntry(turnId: string, entry: RenderableTurnEntry)}
  {#if entry.kind === "item"}{@render renderTurnItem(turnId, entry.item)}
  {:else if entry.kind === "readGroup"}
    <div class="border border-gray-200 rounded-2xl bg-white overflow-hidden shadow-sm">
      <button class="turn-card-header turn-card-header--neutral w-full flex items-center justify-between px-4 py-3 hover:bg-gray-50 transition-colors" onclick={() => void toggleReadOnlyCommandGroup(turnId, entry.key, entry.items)}><div class="flex items-center gap-3"><div class="p-2 bg-gray-100 text-gray-400 rounded-xl"><Search size={14} /></div><div class="text-left"><h4 class="text-xs font-bold text-gray-900 leading-tight">{getReadOnlyCommandGroupLabel(entry.items)}</h4><p class="text-[10px] text-gray-500 mt-0.5 font-medium">{summarizeReadOnlyCommandGroup(entry.items)}</p></div></div><div class="flex items-center gap-3"><span class="px-1.5 py-0.5 bg-gray-50 text-[9px] font-bold text-gray-400 rounded uppercase tracking-tighter">{ui.opsCount(entry.items.length)}</span><ChevronDown size={14} class="text-gray-400 {isItemExpanded(turnId, entry.key) ? 'rotate-180' : ''} transition-transform" /></div></button>
      {#if isItemExpanded(turnId, entry.key)}
        <div class="border-t border-gray-100 bg-gray-50/30">{#if isItemDetailLoading(turnId, entry.key)}<div class="p-6 flex justify-center text-gray-400 italic text-xs animate-pulse">{ui.readingFileData}</div>
          {:else}<div class="p-0">{#each entry.items as commandItem}<div class="p-4 border-b border-gray-100 last:border-0"><div class="flex items-center justify-between mb-2"><span class="text-[10px] font-bold text-gray-500 uppercase tracking-widest">{summarizeCommand(commandItem)}</span>{#if commandItem.exitCode !== null}<span class="text-[9px] font-mono text-gray-400">Exit {commandItem.exitCode}</span>{/if}</div><pre class="p-3 bg-white border border-gray-200 rounded-lg text-[10px] font-mono text-gray-600 overflow-x-auto max-h-60 leading-relaxed">{String(commandItem.aggregatedOutput ?? "")}</pre></div>{/each}</div>{/if}</div>
      {/if}
    </div>
  {:else}
    <div class="border border-gray-200 rounded-2xl bg-white overflow-hidden shadow-sm">
      <div class="turn-card-header turn-card-header--neutral flex items-center gap-2 pr-3">
        <button class="min-w-0 flex-1 flex items-center justify-between px-4 py-3 hover:bg-gray-50 transition-colors" onclick={() => void toggleFileChangeGroup(turnId, entry.key, entry.items)} type="button"><div class="flex items-center gap-3 min-w-0"><div class="p-2 bg-gray-100 text-gray-400 rounded-xl shrink-0"><FileDiff size={14} /></div><div class="text-left min-w-0"><h4 class="text-xs font-bold text-gray-900 leading-tight">{getFileChangeGroupLabel(entry.items)}</h4><p class="text-[10px] text-gray-500 mt-0.5 font-medium truncate">{summarizeFileChangeGroup(entry.items)}</p></div></div><div class="flex items-center gap-3 shrink-0"><span class="px-1.5 py-0.5 bg-gray-50 text-[9px] font-bold text-gray-400 rounded uppercase tracking-tighter">{getFileChangeGroupSummaryEntries(entry.items).length} Files</span><ChevronDown size={14} class="text-gray-400 {isItemExpanded(turnId, entry.key) ? 'rotate-180' : ''} transition-transform" /></div></button>
        {#if getFileChangeGroupViews(entry.items).length > 0}
          <button
            class="rounded-lg border border-gray-200 bg-white px-3 py-1.5 text-[10px] font-bold text-gray-700 hover:bg-gray-50 transition-colors shrink-0"
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
        <div class="border-t border-gray-100">{#if isItemDetailLoading(turnId, entry.key)}<div class="p-6 flex justify-center text-gray-400 italic text-xs">{ui.computingDiffs}</div>
          {:else}<div class="p-0">{#each getFileChangeGroupViews(entry.items) as change}<div class="border-b border-gray-100 last:border-0"><button class="w-full flex items-center justify-between px-5 py-2 hover:bg-gray-50 transition-colors" onclick={() => toggleFileChangeEntry(turnId, entry.key, change)}><span class="text-[10px] font-mono font-bold text-gray-600">{change.path}</span><ChevronDown size={12} class="text-gray-300 {isFileChangeEntryExpanded(turnId, entry.key, change) ? 'rotate-180' : ''} transition-transform" /></button>{#if isFileChangeEntryExpanded(turnId, entry.key, change)}<div class="bg-gray-50 border-t border-gray-100">{#if change.renderable}<MonacoDiffEditor fallbackText={change.diff} height={400} modified={change.modified} original={change.original} path={change.path} />{:else}<pre class="p-4 text-[10px] font-mono text-gray-600 overflow-x-auto">{change.diff}</pre>{/if}</div>{/if}</div>{/each}</div>{/if}</div>
      {/if}
    </div>
  {/if}
{/snippet}
