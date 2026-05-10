export type ApprovalPolicy = "never" | "on-request" | "on-failure" | "untrusted";
export type SandboxMode = "read-only" | "workspace-write" | "danger-full-access";
export type CollaborationMode = "default" | "plan";
export type ReasoningEffort = "minimal" | "low" | "medium" | "high" | "xhigh";
export type ServiceSpeed = "auto" | "fast" | "flex";
export type Personality = "none" | "friendly" | "pragmatic";
export type AttachmentKind = "image" | "file";
export type AutoApproveMode = "manual" | "turn" | "session";
export type ItemDetailState = "inline" | "deferred" | "loaded";
export type SteeringResumeMode = "ask" | "auto";
export type UserRole = "owner" | "admin" | "viewer";
export type AutomationScheduleMode = "manual" | "interval";
export type AutomationExecutionTarget = "local" | "worktree";
export type AutomationRunStatus = "running" | "started" | "completed" | "failed" | "cancelled" | "skipped";
export type SessionForkMode = "fork" | "handoff";
export type AutostartProvider = "windows-startup" | "macos-launch-agent" | "linux-systemd-user" | "linux-xdg-autostart";

export type LoginHcaptchaConfig = {
  enabled: boolean;
  siteKey: string | null;
};

export type AuthSessionPayload = {
  authenticated: boolean;
  role: UserRole | null;
  activeProfileId: string | null;
  hcaptcha: LoginHcaptchaConfig;
};

export type SessionPreferences = {
  cwd: string;
  model: string | null;
  modelContextWindow: number | null;
  effort: ReasoningEffort | null;
  speed: ServiceSpeed;
  personality: Personality;
  mode: CollaborationMode;
  sendOnEnter: boolean;
  sandboxMode: SandboxMode;
  approvalPolicy: ApprovalPolicy;
  networkAccess: boolean;
  autoApproveMode: AutoApproveMode;
  steeringResumeMode: SteeringResumeMode;
  shutdownOnCompletion: boolean;
  gitRepoPath: string | null;
};

export type SessionSummary = {
  id: string;
  name: string | null;
  preview: string;
  queueCount: number;
  highlight: SessionSummaryHighlight | null;
  pinned: boolean;
  tags: string[];
  cwd: string;
  archived: boolean;
  createdAt: number;
  updatedAt: number;
  status: string;
  isSubagent: boolean;
  agentNickname: string | null;
  agentRole: string | null;
  preferences: SessionPreferences | null;
};

export type SessionSummaryFilter = {
  pinnedOnly: boolean;
  runningOnly: boolean;
  queuedOnly: boolean;
  highlight: "all" | "attention" | "completed";
  tags: string[];
};

export type SavedSessionFilter = SessionSummaryFilter & {
  id: string;
  name: string;
};

export type PromptPreset = {
  id: string;
  name: string;
  prompt: string;
  createdAt: number;
  updatedAt: number;
};

export type SelectedSkill = {
  id: string;
  name: string;
  path: string;
};

export type AutomationDefinition = {
  id: string;
  name: string;
  prompt: string;
  skills: SelectedSkill[];
  enabled: boolean;
  scheduleMode: AutomationScheduleMode;
  intervalMinutes: number | null;
  target: AutomationExecutionTarget;
  repoPath: string | null;
  cwd: string | null;
  model: string | null;
  effort: ReasoningEffort | null;
  speed: ServiceSpeed | null;
  mode: CollaborationMode | null;
  createdAt: number;
  updatedAt: number;
  lastRunAt: number | null;
  nextRunAt: number | null;
};

export type AutomationRun = {
  id: string;
  automationId: string;
  automationName: string;
  status: AutomationRunStatus;
  trigger: "manual" | "schedule";
  sessionId: string | null;
  repoPath: string | null;
  cwd: string | null;
  worktreePath: string | null;
  startedAt: number;
  completedAt: number | null;
  error: string | null;
  worktreeRemovedAt?: number | null;
  worktreeCleanupError?: string | null;
};

export type AutomationWorktreeCleanupPayload = {
  ok: true;
  dryRun: boolean;
  keepRecent: number;
  candidates: number;
  removed: number;
  failed: number;
  skippedActive: number;
  worktrees: Array<{
    runId: string;
    automationId: string;
    repoPath: string;
    worktreePath: string;
  }>;
  errors: Array<{
    runId: string;
    automationId: string;
    worktreePath: string;
    status: number;
    message: string;
  }>;
};

export type SessionSummaryHighlight = {
  kind: "completed" | "attention";
  at: number;
};

export type SessionListPayload = {
  sessions: SessionSummary[];
  nextCursor: string | null;
  sessionIds?: string[];
  summaryVersions?: Record<string, string>;
  stateHash?: string;
  cacheVersion: string;
  notModified?: false;
};

export type CacheValidationPayload = {
  cacheVersion: string;
  notModified: true;
};

export type SessionListPatchPayload = {
  cacheVersion: string;
  notModified: false;
  patch: {
    baseCacheVersion: string;
    baseStateHash: string;
    finalCacheVersion: string;
    finalStateHash: string;
    sessionIds: string[];
    summaryVersions: Record<string, string>;
    upserts: SessionSummary[];
    removes: string[];
    nextCursor: string | null;
  };
};

export type SessionListResponse = SessionListPayload | CacheValidationPayload | SessionListPatchPayload;

export type AttachmentRecord = {
  id: string;
  originalName: string;
  path: string;
  mimeType: string;
  size: number;
  kind: AttachmentKind;
  createdAt: string;
};

export type CodexItem = {
  id: string;
  type: string;
  detailState?: ItemDetailState;
  detailPreview?: string | null;
  title?: string | null;
  [key: string]: unknown;
};

export type CodexTurn = {
  id: string;
  items: CodexItem[];
  status: string;
  error: { message?: string } | null;
  startedAt: number | null;
  completedAt: number | null;
  durationMs: number | null;
  detailState?: "summary" | "full";
  hiddenItemCount?: number;
};

export type CodexThread = {
  id: string;
  preview: string;
  name: string | null;
  cwd: string;
  status: string;
  createdAt: number;
  updatedAt: number;
  isSubagent: boolean;
  agentNickname: string | null;
  agentRole: string | null;
  turns: CodexTurn[];
};

export type TokenUsageBreakdown = {
  totalTokens: number;
  inputTokens: number;
  cachedInputTokens: number;
  outputTokens: number;
  reasoningOutputTokens: number;
};

export type ThreadTokenUsage = {
  total: TokenUsageBreakdown;
  last: TokenUsageBreakdown;
  modelContextWindow: number | null;
};

export type ThreadGoalStatus = "active" | "paused" | "budgetLimited" | "complete";

export type ThreadGoal = {
  threadId: string;
  objective: string;
  status: ThreadGoalStatus;
  tokenBudget: number | null;
  tokensUsed: number;
  timeUsedSeconds: number;
  createdAt: number;
  updatedAt: number;
};

export type PendingServerRequest = {
  id: string;
  method: string;
  params: Record<string, unknown>;
  createdAt: string;
};

export type DirectoryEntry = {
  name: string;
  path: string;
  isDirectory: boolean;
};

export type DirectoryPayload = {
  allowedRoots: DirectoryEntry[];
  currentPath: string | null;
  parentPath: string | null;
  entries: DirectoryEntry[];
};

export type ModelOption = {
  id: string;
  displayName: string;
  description: string;
  defaultReasoningEffort: string;
  supportedReasoningEfforts: string[];
  additionalSpeedTiers: string[];
  inputModalities: string[];
  supportsPersonality: boolean;
  isDefault: boolean;
};

export type CollaborationModeOption = {
  name: string;
  mode: CollaborationMode | null;
  model: string | null;
  reasoning_effort: string | null;
};

export type StartupPausedQueueAlert = {
  sessionId: string;
  name: string | null;
  cwd: string;
  pendingCount: number;
  updatedAt: number | null;
};

export type StartupScheduledShutdownAlert = {
  sessionId: string | null;
  scheduledFor: number;
  delaySeconds: number;
};

export type StartupDataRecoveryEvent = {
  id: string;
  kind: string;
  at: number;
  path: string;
  backupPath: string;
  sourceBackupPath: string | null;
  restoredFromBackup: boolean;
};

export type NotificationEventType = "sessionCompleted" | "sessionAttention" | "queueDispatchFailed" | "shutdownScheduled";

export type NotificationSettings = {
  enabledEventTypes: NotificationEventType[];
  slackWebhookUrl: string | null;
  webhookUrl: string | null;
};

export type AppNotification = {
  id: string;
  type: NotificationEventType;
  createdAt: number;
  readAt: number | null;
  sessionId: string | null;
  sessionName: string | null;
  payload: Record<string, unknown>;
};

export type AuditLogEntry = {
  id: string;
  at: number;
  role: UserRole | "anonymous";
  method: string;
  target: string | null;
  ok: boolean;
  error: string | null;
};

export type AppConfigPayload = {
  models: ModelOption[];
  collaborationModes: CollaborationModeOption[];
  profiles: Array<{
    id: string;
    label: string;
    codexHome: string;
    active: boolean;
  }>;
  allowedRoots: DirectoryEntry[];
  defaults: SessionPreferences;
  paths: {
    codexHome: string;
    configFilePath: string;
  };
  git: {
    discoveryDepth: number;
  };
  autostart: {
    available: boolean;
    enabled: boolean;
    provider: AutostartProvider | null;
    location: string | null;
  };
  gateway: {
    restartAvailable: boolean;
    restartCommandConfigured: boolean;
  };
  systemShutdown: {
    available: boolean;
    delaySeconds: number;
    armed: boolean;
  };
  startup: {
    pausedQueues: StartupPausedQueueAlert[];
    scheduledShutdown: StartupScheduledShutdownAlert | null;
    scheduledShutdownBlockedReason?: "queuedWork" | "activeWork" | string | null;
    dataRecoveryEvents?: StartupDataRecoveryEvent[];
  };
  notifications: {
    unreadCount: number;
    settings: NotificationSettings;
  };
  sessionOrganization: {
    savedFilters: SavedSessionFilter[];
    knownTags: string[];
  };
  promptPresets: PromptPreset[];
  automations: {
    items: AutomationDefinition[];
    recentRuns: AutomationRun[];
  };
  account: {
    type: "apiKey" | "chatgpt" | null;
    email: string | null;
    planType: string | null;
    requiresOpenaiAuth: boolean;
  };
};

export type NotificationListPayload = {
  notifications: AppNotification[];
  unreadCount: number;
};

export type AuditLogPayload = {
  entries: AuditLogEntry[];
};

export type SessionSearchScope = "summary" | "full";

export type CodexAccountLoginResponse =
  | {
      type: "apiKey";
    }
  | {
      type: "chatgpt";
      loginId: string;
      authUrl: string;
    }
  | {
      type: "chatgptDeviceCode";
      loginId: string;
      verificationUrl: string;
      userCode: string;
    }
  | {
      type: "chatgptAuthTokens";
    };

export type CodexAccountLoginFlow =
  | {
      type: "chatgpt";
      loginId: string;
      authUrl: string;
      busy: boolean;
      error: string | null;
    }
  | {
      type: "chatgptDeviceCode";
      loginId: string;
      verificationUrl: string;
      userCode: string;
      busy: boolean;
      error: string | null;
    };

export type SessionDetailPayload = {
  thread: CodexThread;
  preferences: SessionPreferences;
  selectedSkills: SelectedSkill[];
  goal: ThreadGoal | null;
  attachments: AttachmentRecord[];
  queue: SessionQueuePayload;
  pendingRequests: PendingServerRequest[];
  activeTurnId: string | null;
  tokenUsage: ThreadTokenUsage | null;
  hydration: {
    state: "idle" | "loading" | "complete" | "error";
    loadedTurns: number;
    totalTurns: number | null;
    remainingTurns: number;
    message: string | null;
    recovery: {
      available: boolean;
      issue: string | null;
      totalLines: number | null;
      recoverableLines: number | null;
      skippedLines: number | null;
    };
  };
  turnIds?: string[];
  turnVersions?: Record<string, string>;
  metadataVersion?: string;
  stateHash?: string;
  cacheVersion: string;
  notModified?: false;
};

export type SessionDetailPatchPayload = {
  cacheVersion: string;
  notModified: false;
  patch: {
    baseCacheVersion: string;
    baseStateHash: string;
    finalCacheVersion: string;
    finalStateHash: string;
    metadataVersion: string;
    turnIds: string[];
    turnVersions: Record<string, string>;
    turnUpserts: CodexTurn[];
    turnRemoves: string[];
    thread: Omit<CodexThread, "turns"> & { turns: [] };
    preferences: SessionPreferences;
    selectedSkills: SelectedSkill[];
    goal: ThreadGoal | null;
    attachments: AttachmentRecord[];
    queue: SessionQueuePayload;
    pendingRequests: PendingServerRequest[];
    activeTurnId: string | null;
    tokenUsage: ThreadTokenUsage | null;
    hydration: SessionDetailPayload["hydration"];
  };
};

export type SessionDetailResponse = SessionDetailPayload | CacheValidationPayload | SessionDetailPatchPayload;

export type SessionRolloutRecoveryPayload = {
  ok: true;
  sessionId: string;
  backupPath: string;
  recoveredAt: number;
  totalLines: number;
  recoveredLines: number;
  skippedLines: number;
};

export type SessionTurnsPagePayload = {
  turns: CodexTurn[];
  loadedTurns: number;
  totalTurns: number | null;
  remainingTurns: number;
};

export type SessionTurnPayload = {
  turn: CodexTurn;
};

export type SessionForkPayload = {
  session: SessionSummary;
  draft: string;
  mode: SessionForkMode;
};

export type SessionTurnSearchMatch = {
  turnId: string;
  turnIndex: number;
  itemId: string | null;
  itemType: string | null;
  preview: string;
  startedAt: number | null;
  requiresFullTurn: boolean;
  requiresItemDetail: boolean;
};

export type SessionTurnSearchPayload = {
  matches: SessionTurnSearchMatch[];
  nextCursor: string | null;
  totalMatches: number;
};

export type SessionItemDetailPayload = {
  item: CodexItem;
};

export type SessionDraftPayload = {
  sessionId: string;
  draft: string;
  intent: "message" | "steer" | "queue" | null;
  updatedAt: number | null;
};

export type SessionQueueItem = {
  id: string;
  prompt: string;
  skills: SelectedSkill[];
  attachmentIds: string[];
  attachmentNames: string[];
  createdAt: number;
  clientRequestId?: string | null;
};

export type SessionQueuePayload = {
  sessionId: string;
  items: SessionQueueItem[];
  resumeRequired: boolean;
  updatedAt: number | null;
  enqueueAccepted?: boolean;
  enqueueItemId?: string | null;
};

export type StreamEvent =
  | {
      kind: "notification";
      method: string;
      params: Record<string, unknown>;
    }
  | {
      kind: "serverRequest";
      id: string;
      method: string;
      params: Record<string, unknown>;
    };

export type ComputerFramePayload = {
  threadId: string;
  turnId: string | null;
  itemId: string | null;
  imageUrl: string;
  mimeType: string | null;
  tool: string | null;
  transport: "websocket" | string;
  frameMode: "snapshot" | string;
  fpsHint: number | null;
  updatedAt: number;
};

export type ComputerInputEvent =
  | {
      type: "click" | "double_click";
      x: number;
      y: number;
      button: "left" | "middle" | "right";
      coordinateSpace: "normalized";
      frameUpdatedAt: number | null;
      server?: string;
      tool?: string;
    }
  | {
      type: "scroll";
      deltaX: number;
      deltaY: number;
      coordinateSpace: "normalized";
      frameUpdatedAt: number | null;
      server?: string;
      tool?: string;
    }
  | {
      type: "key";
      key: string;
      modifiers: string[];
      frameUpdatedAt: number | null;
      server?: string;
      tool?: string;
    }
  | {
      type: "text";
      text: string;
      frameUpdatedAt: number | null;
      server?: string;
      tool?: string;
    };

export type ComputerInputPayload = {
  ok: true;
  routed: "pendingDynamicTool" | "mcpServerTool" | "turnSteer" | "threadInject" | "local" | string;
  upstream: Record<string, unknown> | null;
};

export type GlobalStreamEvent = {
  kind: "notification";
  method: string;
  params: Record<string, unknown>;
};

export type TerminalSummary = {
  id: string;
  title: string;
  cwd: string;
  createdAt: number;
  lastActivityAt: number;
  status: "running" | "exited";
  exitCode: number | null;
};

export type TerminalListPayload = {
  terminals: TerminalSummary[];
};

export type TerminalSnapshotPayload = {
  terminal: TerminalSummary;
  snapshot: string;
};

export type TerminalContextPayload = {
  terminal: TerminalSummary;
  attachments: AttachmentRecord[];
  excerpt: string;
};

export type TerminalEvent =
  | {
      kind: "notification";
      method: "terminal/output";
      params: {
        text: string;
      };
    }
  | {
      kind: "notification";
      method: "terminal/exit";
      params: {
        exitCode: number | null;
      };
    };

export type GitFileReferencePayload = {
  repoPath: string;
  filePath: string | null;
};

export type GitOpenRequest = {
  repoPath: string;
  filePath: string | null;
  filePaths?: string[] | null;
  title?: string | null;
  requestId: number;
};

export type WsConnectionState = "idle" | "connecting" | "connected" | "reconnecting" | "disconnected";

export type CodexRuntimeStatus = {
  installed: boolean;
  configuredBin: string;
  resolvedBinPath: string | null;
  npmAvailable: boolean;
  version: string | null;
  latestVersion: string | null;
  updateAvailable: boolean | null;
  installCommand: string;
  updateCommand: string;
  lastCheckedAt: string | null;
  webuiVersion: string;
  webuiBuildVersion: string;
  webuiBuildCommit: string;
  webuiBuildCommitShort: string;
  webuiBuildDirty: boolean;
  webuiBuildTimestamp: string;
  issues: string[];
};

export type CodexRuntimeActionPayload = {
  ok: true;
  message: string;
  runtime: CodexRuntimeStatus;
};

export type GatewayRestartPayload = {
  ok: true;
  activeAppServerProcesses: number;
  appServerClients: number;
  handoffPrepared: boolean;
  handoffProxyProcesses: number;
  restartScheduled: boolean;
  stdioAppServerProcesses: number;
  mode: "command" | "current-binary" | string;
};

export type CodexQuotaWindow = {
  usedPercent: number;
  remainingPercent: number;
  resetAfterSeconds: number | null;
  resetAt: number | null;
};

export type CodexQuotaStatus = {
  available: boolean;
  source: "backend-api" | null;
  fetchedAt: number | null;
  account: string | null;
  plan: string | null;
  fiveHour: CodexQuotaWindow | null;
  weekly: CodexQuotaWindow | null;
  error: string | null;
};

export type GitRepository = {
  path: string;
  name: string;
  rootPath: string;
  relativePath: string;
  currentBranch: string | null;
};

export type GitBranch = {
  name: string;
  current: boolean;
  upstream: string | null;
};

export type GitWorktree = {
  path: string;
  branch: string | null;
  head: string | null;
  bare: boolean;
  detached: boolean;
  locked: boolean;
  prunable: boolean;
  current: boolean;
};

export type GitWorktreePayload = {
  repoPath: string;
  worktrees: GitWorktree[];
};

export type GitCommit = {
  hash: string;
  shortHash: string;
  author: string;
  authoredAt: string;
  subject: string;
};

export type GitCommitDiffPayload = {
  repoPath: string;
  commitHash: string;
  diff: string;
};

export type GitHubRepositoryInfo = {
  host: string;
  owner: string;
  name: string;
  remoteName: string;
  url: string;
};

export type GitHubPullRequestSummary = {
  number: number;
  title: string;
  state: "open" | "closed" | "merged";
  isDraft: boolean;
  url: string;
  author: string | null;
  authorUrl: string | null;
  baseRefName: string;
  headRefName: string;
  updatedAt: string | null;
  additions: number;
  deletions: number;
  changedFiles: number;
  labels: string[];
};

export type GitHubPullRequestListPayload = {
  repository: GitHubRepositoryInfo;
  pullRequests: GitHubPullRequestSummary[];
};

export type GitHubPullRequestFile = {
  path: string;
  previousPath: string | null;
  status: string;
  additions: number;
  deletions: number;
  patch: string | null;
};

export type GitHubPullRequestDetailPayload = {
  repository: GitHubRepositoryInfo;
  pullRequest: GitHubPullRequestSummary & {
    body: string;
    reviewDecision: string | null;
    mergeStateStatus: string | null;
    commits: number;
    files: GitHubPullRequestFile[];
    filesLoaded: number;
    filesTruncated: boolean;
  };
};

export type GitFileStatus = {
  path: string;
  originalPath: string | null;
  stagedCode: string;
  unstagedCode: string;
  stagedLabel: string;
  unstagedLabel: string;
  hasStagedChanges: boolean;
  hasUnstagedChanges: boolean;
  isUntracked: boolean;
};

export type GitStatusPayload = {
  repo: GitRepository;
  branch: string | null;
  ahead: number;
  behind: number;
  clean: boolean;
  files: GitFileStatus[];
  branches: GitBranch[];
  commits: GitCommit[];
};

export type GitFilePayload = {
  repoPath: string;
  filePath: string;
  originalPath: string | null;
  originalContent: string;
  modifiedContent: string;
  language: string;
  isBinary: boolean;
  status: GitFileStatus | null;
};

export type EditableFilePayload = {
  path: string;
  displayName: string;
  content: string;
  language: string;
  writable: boolean;
};

export type PluginCatalogEntry = {
  name: string;
  displayName: string;
  description: string;
  version: string | null;
  developerName: string | null;
  category: string | null;
  path: string;
  mentionPath?: string | null;
  marketplaceName?: string | null;
  marketplacePath?: string | null;
  pluginId?: string | null;
  installed?: boolean;
  enabled?: boolean;
  installPolicy?: string | null;
  authPolicy?: string | null;
  availability?: string | null;
  capabilities?: string[];
  skills: string[];
};

export type CodexAppInfo = {
  id: string;
  name: string;
  description: string | null;
  logoUrl: string | null;
  logoUrlDark: string | null;
  distributionChannel: string | null;
  installUrl: string | null;
  isAccessible: boolean;
  isEnabled: boolean;
  pluginDisplayNames: string[];
  [key: string]: unknown;
};

export type CodexAppsListPayload = {
  data: CodexAppInfo[];
  nextCursor: string | null;
};

export type SkillCatalogEntry = {
  id: string;
  name: string;
  description: string;
  path: string;
  source: "local" | "system" | "plugin" | "codex-plugin";
  pluginName: string | null;
};

export type CatalogPayload = {
  plugins: PluginCatalogEntry[];
  skills: SkillCatalogEntry[];
};
