import { base } from "$app/paths";
import type { ArenaListPayload, ArenaRun } from "$lib/arena-types";
import type { ThemeSettings } from "$lib/theme-customization";

import type {
  AutomationDefinition,
  AutomationRun,
  AppConfigPayload,
  AuditLogPayload,
  AuthSessionPayload,
  AttachmentRecord,
  CatalogPayload,
  CodexAccountLoginResponse,
  CodexQuotaStatus,
  CodexRuntimeActionPayload,
  CodexRuntimeStatus,
  DirectoryPayload,
  EditableFilePayload,
  GlobalStreamEvent,
  GitCommitDiffPayload,
  GitFilePayload,
  GitFileReferencePayload,
  GitHubPullRequestDetailPayload,
  GitHubPullRequestListPayload,
  GitRepository,
  GitStatusPayload,
  GitWorktreePayload,
  NotificationListPayload,
  NotificationSettings,
  PromptPreset,
  SavedSessionFilter,
  SelectedSkill,
  SessionDetailPayload,
  SessionDraftPayload,
  SessionItemDetailPayload,
  SessionForkPayload,
  SessionListPayload,
  SessionPreferences,
  SessionRolloutRecoveryPayload,
  SessionSearchScope,
  SessionSummaryFilter,
  SessionSummary,
  TerminalEvent,
  TerminalContextPayload,
  TerminalListPayload,
  TerminalSnapshotPayload,
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

async function request<T>(input: string, init?: RequestInit) {
  const headers = new Headers(init?.headers ?? {});
  if (!(init?.body instanceof FormData) && !headers.has("content-type")) {
    headers.set("content-type", "application/json");
  }

  const response = await fetch(input, {
    ...init,
    credentials: init?.credentials ?? "include",
    headers
  });

  if (!response.ok) {
    const message = await response.text();
    throw new Error(message || `${response.status} ${response.statusText}`);
  }

  return (await response.json()) as T;
}

const ws = new WebSocketRpcClient();

export const api = {
  getAuthSession() {
    return request<AuthSessionPayload>(apiPath("/auth/session"), {
      method: "GET"
    });
  },

  login(password: string, hcaptchaToken: string | null = null) {
    return request<{ ok: true }>(apiPath("/auth/login"), {
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

  updateNotificationSettings(settings: Partial<NotificationSettings>) {
    return ws.request<{ settings: NotificationSettings; unreadCount: number }>("notifications/settings/update", settings);
  },

  getRuntimeStatus() {
    return ws.request<CodexRuntimeStatus>("runtime/status");
  },

  getCatalog() {
    return ws.request<CatalogPayload>("catalog/get");
  },

  getEditableFile(filePath: string) {
    return ws.request<EditableFilePayload>("editor/file/get", { filePath });
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

  getSessions(archived = false, cursor: string | null = null, limit = 20, filter: SessionSummaryFilter | null = null) {
    return ws.request<SessionListPayload>("sessions/list", { archived, cursor, limit, filter });
  },

  searchSessions(
    query: string,
    scope: SessionSearchScope,
    archived = false,
    cursor: string | null = null,
    limit = 20,
    filter: SessionSummaryFilter | null = null
  ) {
    return ws.request<SessionListPayload>("sessions/search", { query, scope, archived, cursor, limit, filter });
  },

  createSession(preferences: Partial<SessionPreferences>, name: string | null = null, selectedSkills: SelectedSkill[] = []) {
    return ws.request<SessionSummary>("session/create", { preferences, name, selectedSkills });
  },

  getSession(sessionId: string, limit = 20) {
    return ws.request<SessionDetailPayload>("session/get", { sessionId, limit });
  },

  recoverSessionRollout(sessionId: string) {
    return request<SessionRolloutRecoveryPayload>(apiPath(`/sessions/${sessionId}/recovery`), {
      method: "POST",
      body: JSON.stringify({})
    });
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

  enqueueSessionMessage(sessionId: string, payload: { prompt: string; skills?: SelectedSkill[]; attachmentIds: string[] }) {
    return ws.request<SessionDetailPayload["queue"]>("session/queue/enqueue", {
      sessionId,
      prompt: payload.prompt,
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

  dispatchQueuedMessage(sessionId: string, queueId: string, mode: "message" | "steer") {
    return ws.request<SessionDetailPayload["queue"]>("session/queue/dispatch", {
      sessionId,
      queueId,
      mode
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
    return ws.request<{ meta: { pinned: boolean; tags: string[] }; knownTags: string[] }>("session/organization/update", {
      sessionId,
      ...patch
    });
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
    return ws.request<CodexAccountLoginResponse>("account/login/start", { type, apiKey: apiKey ?? null });
  },

  cancelAccountLogin(loginId: string) {
    return ws.request<{ status: "canceled" | "notFound" }>("account/login/cancel", { loginId });
  },

  logoutAccount() {
    return ws.request<Record<string, never>>("account/logout");
  },

  sendMessage(sessionId: string, payload: { prompt: string; skills?: SelectedSkill[]; attachmentIds: string[]; preferences: Partial<SessionPreferences> }) {
    return ws.request<{ ok: true }>("turn/send", {
      sessionId,
      prompt: payload.prompt,
      skills: payload.skills ?? [],
      attachmentIds: payload.attachmentIds,
      preferences: payload.preferences
    });
  },

  steerTurn(sessionId: string, prompt: string, attachmentIds: string[] = [], skills: SelectedSkill[] = []) {
    return ws.request<{ ok: true }>("turn/steer", {
      sessionId,
      prompt,
      skills,
      attachmentIds
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
