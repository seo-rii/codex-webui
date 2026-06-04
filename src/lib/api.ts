import { base } from "$app/paths";
import type { ArenaListPayload, ArenaRun } from "$lib/arena-types";
import type { ThemeSettings } from "$lib/theme-customization";

import type {
  AutomationDefinition,
  AutomationRun,
  AutomationWorktreeCleanupPayload,
  AppConfigPayload,
  AuditLogPayload,
  AuthSessionPayload,
  AttachmentRecord,
  CatalogPayload,
  CodexAccountLoginResponse,
  CodexAppsListPayload,
  CodexHooksListPayload,
  CodexMemoryResetPayload,
  CodexMemoryStatusPayload,
  ComputerInputEvent,
  ComputerInputPayload,
  CodexQuotaStatus,
  CodexRuntimeActionPayload,
  CodexRuntimeProcessKillPayload,
  CodexRuntimeProcessesPayload,
  CodexRuntimeStatus,
  CodexSkillsListPayload,
  DirectoryPayload,
  EditableFilePayload,
  FileMentionSearchPayload,
  GlobalStreamEvent,
  GatewayRestartPayload,
  GitCommitDiffPayload,
  GitFilePayload,
  GitFileReferencePayload,
  GitHubPullRequestDetailPayload,
  GitHubPullRequestListPayload,
  GitRepository,
  GitStatusPayload,
  GitWorktreePayload,
  McpServerStatusPayload,
  NotificationListPayload,
  NotificationSettings,
  ParserDiagnosticsPayload,
  PromptPreset,
  SavedSessionFilter,
  SelectedSkill,
  SessionDetailPayload,
  SessionDetailResponse,
  SessionDraftPayload,
  SessionItemDetailPayload,
  SessionForkPayload,
  SessionListResponse,
  SessionMemoryModePayload,
  SessionPreferences,
  SessionReviewStartPayload,
  SessionReviewTarget,
  SessionRollbackPayload,
  SessionRollbackTargetsPayload,
  SessionRolloutRecoveryPayload,
  SessionSearchScope,
  SessionSummaryFilter,
  SessionSummary,
  TerminalEvent,
  TerminalContextPayload,
  TerminalListPayload,
  TerminalSnapshotPayload,
  ThreadGoal,
  SessionTurnSearchPayload,
  SessionTurnPayload,
  SessionTurnsPagePayload,
  StreamEvent,
  WsConnectionState
} from "$lib/types";
import { WebSocketRpcClient } from "$lib/ws-client";

function appPath(pathname: string) {
  const normalized = pathname.startsWith("/") ? pathname : `/${pathname}`;
  return `${base}${normalized}` || "/";
}

function apiPath(pathname: string) {
  const normalized = pathname.startsWith("/") ? pathname : `/${pathname}`;
  return `${base}/api${normalized}`;
}

function browserBaseUrl() {
  if (typeof window === "undefined") {
    return null;
  }
  const url = new URL(base ? `${base}/` : "/", window.location.origin);
  return url.toString().replace(/\/$/, "");
}

function readCookie(name: string) {
  if (typeof document === "undefined") {
    return null;
  }
  const encodedName = `${encodeURIComponent(name)}=`;
  return (
    document.cookie
      .split(";")
      .map((entry) => entry.trim())
      .find((entry) => entry.startsWith(encodedName))
      ?.slice(encodedName.length) ?? null
  );
}

async function request<T>(input: string, init?: RequestInit) {
  const headers = new Headers(init?.headers ?? {});
  if (!(init?.body instanceof FormData) && !headers.has("content-type")) {
    headers.set("content-type", "application/json");
  }
  const method = (init?.method ?? "GET").toUpperCase();
  if (
    ["POST", "PUT", "PATCH", "DELETE"].includes(method) &&
    !input.includes("/api/auth/login") &&
    !headers.has("x-codex-webui-csrf")
  ) {
    const token = readCookie("codex_webui_csrf");
    if (token) {
      headers.set("x-codex-webui-csrf", decodeURIComponent(token));
    }
  }

  const response = await fetch(input, {
    ...init,
    credentials: "include",
    headers
  });

  if (!response.ok) {
    const message = await response.text();
    throw new Error(message || `${response.status} ${response.statusText}`);
  }

  return (await response.json()) as T;
}

async function downloadRequest(input: string) {
  const response = await fetch(input, {
    credentials: "include",
    method: "GET"
  });

  if (!response.ok) {
    const message = await response.text();
    throw new Error(message || `${response.status} ${response.statusText}`);
  }

  const disposition = response.headers.get("content-disposition") ?? "";
  const filenameMatch = /filename="([^"]+)"/iu.exec(disposition);
  return {
    blob: await response.blob(),
    filename: filenameMatch?.[1] ?? null
  };
}

const ws = new WebSocketRpcClient();

export const api = {
  getAuthSession() {
    return request<AuthSessionPayload>(apiPath("/auth/session"), {
      method: "GET"
    });
  },

  login(password: string, hcaptchaToken: string | null = null) {
    return request<{ ok: true; role?: AuthSessionPayload["role"] }>(apiPath("/auth/login"), {
      method: "POST",
      body: JSON.stringify({ password, hcaptchaToken })
    });
  },

  async logout() {
    const response = await request<{ ok: true }>(apiPath("/auth/logout"), {
      method: "POST",
      body: JSON.stringify({})
    });
    ws.disconnect("Logged out.");
    return response;
  },

  selectAuthProfile(profileId: string) {
    return request<{ ok: true; activeProfileId: string }>(apiPath("/auth/profile"), {
      method: "POST",
      body: JSON.stringify({ profileId })
    });
  },

  disconnect() {
    ws.disconnect();
  },

  onReconnect(listener: () => void) {
    return ws.onReconnect(listener);
  },

  onConnectionState(listener: (state: WsConnectionState) => void) {
    return ws.onConnectionState(listener);
  },

  reconnectNow() {
    ws.reconnectNow();
  },

  listArenaRuns() {
    return ws.request<ArenaListPayload>("arena/list");
  },

  startArenaRun(
    prompt: string,
    contestants: Array<{
      model: string;
      label: string;
    }>,
    preferences: Partial<SessionPreferences>
  ) {
    return ws.request<ArenaRun>("arena/start", {
      prompt,
      contestants,
      preferences
    });
  },

  subscribeGlobal(listener: (event: GlobalStreamEvent) => void) {
    return ws.subscribeGlobal(listener);
  },

  subscribeSession(sessionId: string, listener: (event: StreamEvent) => void) {
    return ws.subscribeSession(sessionId, listener);
  },

  subscribeTerminal(terminalId: string, listener: (event: TerminalEvent) => void) {
    return ws.subscribeTerminal(terminalId, listener);
  },

  getConfig() {
    return ws.request<AppConfigPayload>("config/get");
  },

  saveSystemShutdownAfterQueueCompletes(armed: boolean) {
    return ws.request<AppConfigPayload>("config/update", {
      systemShutdown: {
        armed
      }
    });
  },

  saveAutostartEnabled(enabled: boolean) {
    return ws.request<AppConfigPayload>("config/update", {
      autostart: {
        enabled
      }
    });
  },

  saveThemeSettings(theme: ThemeSettings) {
    return ws.request<AppConfigPayload & { theme: ThemeSettings }>("config/update", {
      theme
    });
  },

  saveDefaultSessionPreferences(preferences: Partial<SessionPreferences>) {
    return ws.request<AppConfigPayload>("config/update", {
      defaults: preferences
    });
  },

  getNotifications(limit = 80) {
    return ws.request<NotificationListPayload>("notifications/list", { limit });
  },

  getAuditLog(limit = 120) {
    return ws.request<AuditLogPayload>("audit/list", { limit });
  },

  markNotificationsRead(ids: string[] | null = null) {
    return ws.request<NotificationListPayload>("notifications/markRead", { ids });
  },

  clearNotifications() {
    return ws.request<NotificationListPayload>("notifications/clear");
  },

  saveAutomation(automation: AutomationDefinition) {
    return ws.request<{ automations: AutomationDefinition[] }>("automations/save", {
      automation
    });
  },

  deleteAutomation(automationId: string) {
    return ws.request<{ automations: AutomationDefinition[] }>("automations/delete", {
      automationId
    });
  },

  runAutomation(automationId: string, trigger: "manual" | "schedule" = "manual") {
    return ws.request<{ ok: true; session: SessionSummary; run: AutomationRun | null }>("automations/run", {
      automationId,
      trigger
    });
  },

  cleanupAutomationWorktrees(keepRecent = 10, dryRun = false) {
    return ws.request<AutomationWorktreeCleanupPayload>("automations/worktrees/cleanup", {
      keepRecent,
      dryRun
    });
  },

  updateNotificationSettings(settings: Partial<NotificationSettings>) {
    return ws.request<{ settings: NotificationSettings; unreadCount: number }>("notifications/settings/update", settings);
  },

  getRuntimeStatus() {
    return ws.request<CodexRuntimeStatus>("runtime/status");
  },

  getRuntimeProcesses() {
    return ws.request<CodexRuntimeProcessesPayload>("runtime/processes/list");
  },

  getMemoryStatus(sessionId: string | null = null) {
    return ws.request<CodexMemoryStatusPayload>("memory/status", { sessionId });
  },

  resetMemory() {
    return ws.request<CodexMemoryResetPayload>("memory/reset");
  },

  setSessionMemoryMode(sessionId: string, mode: "enabled" | "disabled") {
    return ws.request<SessionMemoryModePayload>("session/memoryMode/set", {
      sessionId,
      mode
    });
  },

  compareParserWithNativeSession(sessionId: string, limit = 5) {
    return ws.request<ParserDiagnosticsPayload>("diagnostics/parser/compare", {
      sessionId,
      limit
    });
  },

  killRuntimeProcess(profileId: string, pid: number) {
    return ws.request<CodexRuntimeProcessKillPayload>("runtime/process/kill", {
      profileId,
      pid
    });
  },

  getCatalog() {
    return ws.request<CatalogPayload>("catalog/get");
  },

  listCodexFeatures(params: Record<string, unknown> = {}) {
    return ws.request<Record<string, unknown>>("codex/features/list", params);
  },

  setCodexFeatureEnablement(enablement: Record<string, boolean>) {
    return ws.request<Record<string, unknown>>("codex/features/set", { enablement });
  },

  listCodexPlugins(params: Record<string, unknown> = {}) {
    return ws.request<Record<string, unknown>>("codex/plugins/list", params);
  },

  readCodexPlugin(params: Record<string, unknown>) {
    return ws.request<Record<string, unknown>>("codex/plugins/read", params);
  },

  installCodexPlugin(params: Record<string, unknown>) {
    return ws.request<Record<string, unknown>>("codex/plugins/install", params);
  },

  uninstallCodexPlugin(pluginId: string) {
    return ws.request<Record<string, unknown>>("codex/plugins/uninstall", { pluginId });
  },

  addCodexMarketplace(params: Record<string, unknown>) {
    return ws.request<Record<string, unknown>>("codex/marketplaces/add", params);
  },

  removeCodexMarketplace(params: Record<string, unknown>) {
    return ws.request<Record<string, unknown>>("codex/marketplaces/remove", params);
  },

  upgradeCodexMarketplace(params: Record<string, unknown>) {
    return ws.request<Record<string, unknown>>("codex/marketplaces/upgrade", params);
  },

  listCodexSkills(params: Record<string, unknown> = {}) {
    return ws.request<CodexSkillsListPayload>("codex/skills/list", params);
  },

  listCodexHooks(params: Record<string, unknown> = {}) {
    return ws.request<CodexHooksListPayload>("codex/hooks/list", params);
  },

  listCodexApps(params: Record<string, unknown> = {}) {
    return ws.request<CodexAppsListPayload>("codex/apps/list", params);
  },

  listMcpServers(params: Record<string, unknown> = {}) {
    return ws.request<McpServerStatusPayload>("codex/mcp/status/list", params);
  },

  refreshMcpServers() {
    return ws.request<Record<string, unknown>>("codex/mcp/refresh", {});
  },

  startMcpOauthLogin(params: { name: string; scopes?: string[] | null; timeoutSecs?: number | null }) {
    return ws.request<{ authorizationUrl: string }>("codex/mcp/oauth/login", params);
  },

  listRealtimeVoices() {
    return ws.request<{ voices: unknown }>("codex/realtime/listVoices", {});
  },

  startRealtimeSession(
    threadId: string,
    params: {
      outputModality?: "text" | "audio";
      prompt?: string | null;
      realtimeSessionId?: string | null;
      transport?: Record<string, unknown> | null;
      voice?: string | null;
    } = {}
  ) {
    return ws.request<Record<string, unknown>>("codex/realtime/start", {
      threadId,
      outputModality: params.outputModality ?? "text",
      prompt: params.prompt ?? null,
      realtimeSessionId: params.realtimeSessionId ?? null,
      transport: params.transport ?? null,
      voice: params.voice ?? null
    });
  },

  appendRealtimeText(threadId: string, text: string) {
    return ws.request<Record<string, unknown>>("codex/realtime/appendText", { threadId, text });
  },

  stopRealtimeSession(threadId: string) {
    return ws.request<Record<string, unknown>>("codex/realtime/stop", { threadId });
  },

  sendComputerInput(sessionId: string, input: ComputerInputEvent) {
    return ws.request<ComputerInputPayload>("computer/input", { sessionId, input });
  },

  getEditableFile(filePath: string) {
    return ws.request<EditableFilePayload>("editor/file/get", { filePath });
  },

  downloadEditableFile(filePath: string) {
    return downloadRequest(apiPath(`/editor/download?filePath=${encodeURIComponent(filePath)}`));
  },

  saveEditableFile(filePath: string, content: string) {
    return ws.request<EditableFilePayload>("editor/file/save", { filePath, content });
  },

  getQuota(refresh = false) {
    return ws.request<CodexQuotaStatus>("runtime/quota", { refresh });
  },

  checkRuntimeUpdate() {
    return ws.request<CodexRuntimeStatus>("runtime/checkUpdate");
  },

  installRuntime() {
    return ws.request<CodexRuntimeActionPayload>("runtime/install");
  },

  updateRuntime() {
    return ws.request<CodexRuntimeActionPayload>("runtime/update");
  },

  restartGateway() {
    return ws.request<GatewayRestartPayload>("gateway/restart");
  },

  getSessions(
    archived = false,
    cursor: string | null = null,
    limit = 20,
    filter: SessionSummaryFilter | null = null,
    knownVersion: string | null = null,
    knownSummaryVersions: Record<string, string> | null = null,
    knownStateHash: string | null = null
  ) {
    return ws.request<SessionListResponse>("sessions/list", {
      archived,
      cursor,
      limit,
      filter,
      knownVersion,
      knownSummaryVersions,
      knownStateHash
    });
  },

  searchSessions(
    query: string,
    scope: SessionSearchScope,
    archived = false,
    cursor: string | null = null,
    limit = 20,
    filter: SessionSummaryFilter | null = null,
    knownVersion: string | null = null,
    knownSummaryVersions: Record<string, string> | null = null,
    knownStateHash: string | null = null
  ) {
    return ws.request<SessionListResponse>("sessions/search", {
      query,
      scope,
      archived,
      cursor,
      limit,
      filter,
      knownVersion,
      knownSummaryVersions,
      knownStateHash
    });
  },

  createSession(preferences: Partial<SessionPreferences>, name: string | null = null, selectedSkills: SelectedSkill[] = []) {
    return ws.request<SessionSummary>("session/create", { preferences, name, selectedSkills });
  },

  getSession(
    sessionId: string,
    limit = 20,
    knownVersion: string | null = null,
    knownTurnVersions: Record<string, string> | null = null,
    knownStateHash: string | null = null
  ) {
    return ws.request<SessionDetailResponse>("session/get", {
      sessionId,
      limit,
      knownVersion,
      knownTurnVersions,
      knownStateHash
    });
  },

  recoverSessionRollout(sessionId: string) {
    return ws.request<SessionRolloutRecoveryPayload>("session/recovery", { sessionId });
  },

  forkSession(
    sessionId: string,
    payload: {
      mode: "fork" | "handoff";
      turnId?: string | null;
      messageText?: string | null;
    }
  ) {
    return ws.request<SessionForkPayload>("session/fork", {
      sessionId,
      ...payload
    });
  },

  startReview(
    sessionId: string,
    payload: {
      target: SessionReviewTarget;
      delivery?: "inline" | "detached" | null;
    }
  ) {
    return ws.request<SessionReviewStartPayload>("session/review/start", {
      sessionId,
      target: payload.target,
      delivery: payload.delivery ?? null
    });
  },

  rollbackSession(sessionId: string, numTurns: number) {
    return ws.request<SessionRollbackPayload>("session/rollback", {
      sessionId,
      numTurns
    });
  },

  listRollbackTargets(sessionId: string) {
    return ws.request<SessionRollbackTargetsPayload>("session/rollbackTargets/list", {
      sessionId
    });
  },

  searchSessionTurns(sessionId: string, query: string, cursor: string | null = null, limit = 20) {
    return ws.request<SessionTurnSearchPayload>("session/search", { sessionId, query, cursor, limit });
  },

  getSessionDraft(sessionId: string) {
    return ws.request<SessionDraftPayload>("session/draft/get", { sessionId });
  },

  saveSessionDraft(sessionId: string, draft: string, intent: "message" | "steer" | "queue") {
    return ws.request<SessionDraftPayload>("session/draft/save", { sessionId, draft, intent });
  },

  getSessionQueue(sessionId: string) {
    return ws.request<SessionDetailPayload["queue"]>("session/queue/get", { sessionId });
  },

  enqueueSessionMessage(
    sessionId: string,
    payload: {
      prompt: string;
      skills?: SelectedSkill[];
      attachmentIds: string[];
      clientRequestId?: string;
      clientUserMessageId?: string;
    }
  ) {
    return ws.request<SessionDetailPayload["queue"]>("session/queue/enqueue", {
      sessionId,
      prompt: payload.prompt,
      clientRequestId: payload.clientRequestId ?? null,
      clientUserMessageId: payload.clientUserMessageId ?? null,
      skills: payload.skills ?? [],
      attachmentIds: payload.attachmentIds
    });
  },

  resumeSessionQueue(sessionId: string) {
    return ws.request<SessionDetailPayload["queue"]>("session/queue/resume", { sessionId });
  },

  removeQueuedMessage(sessionId: string, queueId: string) {
    return ws.request<SessionDetailPayload["queue"]>("session/queue/remove", { sessionId, queueId });
  },

  updateQueuedMessage(sessionId: string, queueId: string, payload: { prompt: string; skills?: SelectedSkill[]; attachmentIds?: string[] }) {
    return ws.request<SessionDetailPayload["queue"]>("session/queue/update", {
      sessionId,
      queueId,
      prompt: payload.prompt,
      skills: payload.skills ?? [],
      attachmentIds: payload.attachmentIds
    });
  },

  reorderQueuedMessages(sessionId: string, queueIds: string[]) {
    return ws.request<SessionDetailPayload["queue"]>("session/queue/reorder", {
      sessionId,
      queueIds
    });
  },

  dispatchQueuedMessage(sessionId: string, queueId: string, mode: "message" | "steer", activeTurnId: string | null = null) {
    return ws.request<SessionDetailPayload["queue"]>("session/queue/dispatch", {
      sessionId,
      queueId,
      mode,
      activeTurnId
    });
  },

  clearSessionDraft(sessionId: string) {
    return ws.request<SessionDraftPayload>("session/draft/clear", { sessionId });
  },

  getSessionOlderTurns(sessionId: string, beforeTurnId: string, limit = 20) {
    return ws.request<SessionTurnsPagePayload>("session/olderTurns/get", { sessionId, beforeTurnId, limit });
  },

  getSessionTurn(sessionId: string, turnId: string) {
    return ws.request<SessionTurnPayload>("session/turn/get", { sessionId, turnId });
  },

  getSessionItemDetail(sessionId: string, turnId: string, itemId: string) {
    return ws.request<SessionItemDetailPayload>("session/itemDetail/get", { sessionId, turnId, itemId });
  },

  getSessionGoal(sessionId: string) {
    return ws.request<{ goal: ThreadGoal | null }>("session/goal/get", { sessionId });
  },

  setSessionGoal(
    sessionId: string,
    payload: {
      objective?: string | null;
      status?: ThreadGoal["status"] | null;
      tokenBudget?: number | null;
    }
  ) {
    return ws.request<{ goal: ThreadGoal | null }>("session/goal/set", {
      sessionId,
      ...payload
    });
  },

  clearSessionGoal(sessionId: string) {
    return ws.request<{ goal: null; cleared: boolean }>("session/goal/clear", { sessionId });
  },

  savePreferences(sessionId: string, preferences: Partial<SessionPreferences>) {
    return ws.request<SessionPreferences>("session/savePreferences", { sessionId, preferences });
  },

  saveSessionSkills(sessionId: string, skills: SelectedSkill[]) {
    return ws.request<SelectedSkill[]>("session/skills/save", { sessionId, skills });
  },

  renameSession(sessionId: string, name: string) {
    return ws.request<{ ok: true }>("session/rename", { sessionId, name });
  },

  updateSessionOrganization(sessionId: string, patch: Partial<{ pinned: boolean; tags: string[] }>) {
    return ws.request<{
      meta: { pinned: boolean; tags: string[] };
      knownTags: string[];
      sessionFolders: AppConfigPayload["sessionOrganization"]["sessionFolders"];
    }>("session/organization/update", {
      sessionId,
      ...patch
    });
  },

  upsertSessionFolder(name: string, pinned: boolean | null = null) {
    return ws.request<{
      folder: AppConfigPayload["sessionOrganization"]["sessionFolders"][number];
      knownTags: string[];
      sessionFolders: AppConfigPayload["sessionOrganization"]["sessionFolders"];
    }>("sessionFolders/upsert", { name, pinned });
  },

  deleteSessionFolder(name: string, removeFromSessions = false) {
    return ws.request<{
      removed: string;
      knownTags: string[];
      sessionFolders: AppConfigPayload["sessionOrganization"]["sessionFolders"];
    }>("sessionFolders/delete", { name, removeFromSessions });
  },

  saveSessionFilter(filter: SavedSessionFilter) {
    return ws.request<{ savedFilters: SavedSessionFilter[]; knownTags: string[] }>("sessionFilters/save", { filter });
  },

  deleteSessionFilter(filterId: string) {
    return ws.request<{ savedFilters: SavedSessionFilter[]; knownTags: string[] }>("sessionFilters/delete", { filterId });
  },

  savePromptPreset(preset: PromptPreset) {
    return ws.request<{ promptPresets: PromptPreset[] }>("promptPresets/save", { preset });
  },

  deletePromptPreset(presetId: string) {
    return ws.request<{ promptPresets: PromptPreset[] }>("promptPresets/delete", { presetId });
  },

  archiveSession(sessionId: string) {
    return ws.request<{ ok: true }>("session/archive", { sessionId });
  },

  unarchiveSession(sessionId: string) {
    return ws.request<{ ok: true; session: SessionSummary }>("session/unarchive", { sessionId });
  },

  getAccount() {
    return ws.request<{
      account: Record<string, unknown>;
      requiresOpenaiAuth: boolean;
    }>("account/get");
  },

  startAccountLogin(type: "chatgpt" | "chatgptDeviceCode" | "apiKey", apiKey?: string | null) {
    return ws.request<CodexAccountLoginResponse>("account/login/start", {
      type,
      apiKey: apiKey ?? null,
      browserBaseUrl: type === "chatgpt" ? browserBaseUrl() : null
    });
  },

  cancelAccountLogin(loginId: string) {
    return ws.request<{ status: "canceled" | "notFound" }>("account/login/cancel", { loginId });
  },

  logoutAccount() {
    return ws.request<Record<string, never>>("account/logout");
  },

  sendMessage(
    sessionId: string,
    payload: {
      prompt: string;
      skills?: SelectedSkill[];
      attachmentIds: string[];
      preferences: Partial<SessionPreferences>;
      clientUserMessageId?: string;
    }
  ) {
    return ws.request<{ ok: true }>("turn/send", {
      sessionId,
      prompt: payload.prompt,
      skills: payload.skills ?? [],
      attachmentIds: payload.attachmentIds,
      preferences: payload.preferences,
      clientUserMessageId: payload.clientUserMessageId ?? null
    });
  },

  steerTurn(
    sessionId: string,
    prompt: string,
    attachmentIds: string[] = [],
    skills: SelectedSkill[] = [],
    activeTurnId: string | null = null,
    clientUserMessageId: string | null = null
  ) {
    return ws.request<{ ok: true }>("turn/steer", {
      sessionId,
      prompt,
      skills,
      attachmentIds,
      activeTurnId,
      clientUserMessageId
    });
  },

  async uploadAttachments(sessionId: string, files: File[]) {
    const formData = new FormData();
    for (const file of files) {
      formData.append("files", file);
    }
    return request<{ attachments: AttachmentRecord[] }>(apiPath(`/sessions/${sessionId}/attachments`), {
      method: "POST",
      body: formData,
      credentials: "include"
    });
  },

  deleteAttachment(sessionId: string, attachmentId: string) {
    return ws.request<{ ok: true }>("attachments/delete", {
      sessionId,
      attachmentId
    });
  },

  resolveRequest(sessionId: string, requestId: string, result: unknown) {
    return ws.request<{ ok: true }>("approval/resolve", {
      sessionId,
      requestId,
      result
    });
  },

  abortTurn(sessionId: string) {
    return ws.request<{ interrupted: boolean }>("turn/abort", {
      sessionId
    });
  },

  browseDirectories(currentPath: string | null) {
    return ws.request<DirectoryPayload>("directories/browse", { currentPath });
  },

  searchFileMentions(query: string, cwd: string | null, limit = 12) {
    return ws.request<FileMentionSearchPayload>("files/search", { query, cwd, limit });
  },

  listRepositories() {
    return ws.request<{ repositories: GitRepository[] }>("git/repositories/list");
  },

  getGitStatus(repoPath: string) {
    return ws.request<GitStatusPayload>("git/status", { repoPath });
  },

  getGitWorktrees(repoPath: string) {
    return ws.request<GitWorktreePayload>("git/worktrees/list", { repoPath });
  },

  createGitWorktree(repoPath: string, payload: { worktreePath: string; branchName: string | null; createBranch: boolean; detach: boolean }) {
    return ws.request<GitWorktreePayload>("git/worktrees/create", {
      repoPath,
      worktreePath: payload.worktreePath,
      branchName: payload.branchName,
      createBranch: payload.createBranch,
      detach: payload.detach
    });
  },

  removeGitWorktree(repoPath: string, worktreePath: string, force = false) {
    return ws.request<GitWorktreePayload>("git/worktrees/remove", {
      repoPath,
      worktreePath,
      force
    });
  },

  getGitFile(repoPath: string, filePath: string) {
    return ws.request<GitFilePayload>("git/file/get", { repoPath, filePath });
  },

  getGitCommitDiff(repoPath: string, commitHash: string) {
    return ws.request<GitCommitDiffPayload>("git/commit/diff", { repoPath, commitHash });
  },

  listGitHubPullRequests(repoPath: string, state: "open" | "closed" | "all" = "open", limit = 20) {
    return ws.request<GitHubPullRequestListPayload>("git/github/pulls", { repoPath, state, limit });
  },

  getGitHubPullRequest(repoPath: string, number: number) {
    return ws.request<GitHubPullRequestDetailPayload>("git/github/pull", { repoPath, number });
  },

  checkoutGitHubPullRequest(repoPath: string, number: number) {
    return ws.request<GitStatusPayload>("git/github/pull/checkout", { repoPath, number });
  },

  resolveGitFile(filePath: string) {
    return ws.request<GitFileReferencePayload>("git/file/resolve", { filePath });
  },

  saveGitFile(repoPath: string, filePath: string, content: string) {
    return ws.request<GitFilePayload>("git/file/save", {
      repoPath,
      filePath,
      content
    });
  },

  stageGitFile(repoPath: string, filePath: string | null = null) {
    return ws.request<GitStatusPayload>("git/stage", {
      repoPath,
      filePath
    });
  },

  unstageGitFile(repoPath: string, filePath: string | null = null) {
    return ws.request<GitStatusPayload>("git/unstage", {
      repoPath,
      filePath
    });
  },

  fetchGitRepository(repoPath: string) {
    return ws.request<GitStatusPayload>("git/fetch", {
      repoPath
    });
  },

  pullGitRepository(repoPath: string) {
    return ws.request<GitStatusPayload>("git/pull", {
      repoPath
    });
  },

  commitGit(repoPath: string, message: string) {
    return ws.request<GitStatusPayload>("git/commit", {
      repoPath,
      message
    });
  },

  checkoutGitBranch(repoPath: string, branchName: string, create = false) {
    return ws.request<GitStatusPayload>("git/checkout", {
      repoPath,
      branchName,
      create
    });
  },

  listTerminals() {
    return ws.request<TerminalListPayload>("terminal/list");
  },

  createTerminal(cwd: string | null = null, title: string | null = null) {
    return ws.request<TerminalSnapshotPayload>("terminal/create", {
      cwd,
      title
    });
  },

  readTerminal(terminalId: string) {
    return ws.request<TerminalSnapshotPayload>("terminal/read", { terminalId });
  },

  attachTerminalContext(sessionId: string, terminalId: string, maxBytes = 24_000) {
    return ws.request<TerminalContextPayload>("terminal/context/attach", {
      sessionId,
      terminalId,
      maxBytes
    });
  },

  sendTerminalInput(terminalId: string, data: string) {
    return ws.request<{ ok: true }>("terminal/input", { terminalId, data });
  },

  closeTerminal(terminalId: string) {
    return ws.request<{ ok: true }>("terminal/close", { terminalId });
  }
};

export { appPath, apiPath };
