<script lang="ts">
  import { onMount } from "svelte";
  import { Activity, AlertTriangle, CheckCircle2, Database, RefreshCw, Search, Server } from "lucide-svelte";

  import { api } from "$lib/api";
  import { localeSignal } from "$lib/i18n";
  import { getLocale } from "$lib/paraglide/runtime.js";
  import type {
    CodexRuntimeProcess,
    CodexRuntimeStatus,
    ParserDiagnosticsPayload,
    AppNotification,
    SessionSummary,
    UserRole,
    WsConnectionState
  } from "$lib/types";

  let {
    runtime = null,
    sessions = [],
    notifications = [],
    selectedSessionId = null,
    connectionState = "idle",
    webRole = "admin",
    onOpenSession = null
  }: {
    runtime?: CodexRuntimeStatus | null;
    sessions?: SessionSummary[];
    notifications?: AppNotification[];
    selectedSessionId?: string | null;
    connectionState?: WsConnectionState;
    webRole?: UserRole | null;
    onOpenSession?: ((sessionId: string, profileId?: string | null) => void | Promise<void>) | null;
  } = $props();

  let runtimeStatus = $state<CodexRuntimeStatus | null>(null);
  let runtimeProcesses = $state<CodexRuntimeProcess[]>([]);
  let overviewLoading = $state(false);
  let overviewError = $state("");
  let parserSessionId = $state("");
  let parserLimit = $state(5);
  let parserLoading = $state(false);
  let parserError = $state("");
  let parserResult = $state<ParserDiagnosticsPayload | null>(null);

  const canLoadAdminDiagnostics = $derived(webRole !== "viewer");
  const visibleSessions = $derived(sessions.slice(0, 200));
  const runningSessions = $derived(sessions.filter((session) => session.status === "running" || session.status === "starting"));
  const queuedSessions = $derived(sessions.filter((session) => session.queueCount > 0));
  const attentionSessions = $derived(sessions.filter((session) => session.highlight?.kind === "attention"));
  const recentRuntimeNotifications = $derived(
    notifications
      .filter((notification) => ["queueDispatchFailed", "sessionAttention", "sessionCompleted"].includes(notification.type))
      .slice(0, 6)
  );
  const ui = $derived.by(() => {
    const _locale = $localeSignal;
    const isKorean = getLocale() === "ko";
    return {
      title: isKorean ? "진단" : "Diagnostics",
      description: isKorean
        ? "게이트웨이 상태, Codex 프로세스, parser/native 비교를 필요한 때만 확인합니다."
        : "Inspect gateway status, Codex processes, and parser/native parity only when needed.",
      refresh: isKorean ? "새로고침" : "Refresh",
      runtime: isKorean ? "런타임" : "Runtime",
      resources: isKorean ? "리소스" : "Resources",
      websocket: isKorean ? "웹소켓" : "WebSocket",
      processes: isKorean ? "Codex 프로세스" : "Codex processes",
      noProcesses: isKorean ? "관리 중인 Codex 프로세스가 없습니다." : "No managed Codex processes are running.",
      activeSessions: isKorean ? "활성 세션" : "Active sessions",
      queuedSessions: isKorean ? "큐 대기" : "Queued sessions",
      attentionNeeded: isKorean ? "확인 필요" : "Needs attention",
      loadedSummaries: isKorean ? "로드된 요약" : "Loaded summaries",
      recentEvents: isKorean ? "최근 이벤트" : "Recent events",
      noRecentEvents: isKorean ? "표시할 최근 런타임 이벤트가 없습니다." : "No recent runtime events to show.",
      parserCompare: isKorean ? "Parser / native 비교" : "Parser / native comparison",
      parserDescription: isKorean
        ? "사이드바 hot path는 커스텀 parser를 유지합니다. 이 작업은 선택한 세션만 native Codex와 비교합니다."
        : "The sidebar hot path stays on the custom parser. This action compares only the selected session with native Codex.",
      selectedSession: isKorean ? "세션" : "Session",
      recentTurns: isKorean ? "최근 턴" : "Recent turns",
      compare: isKorean ? "비교 실행" : "Compare",
      openSession: isKorean ? "세션 열기" : "Open session",
      available: isKorean ? "사용 가능" : "Available",
      unavailable: isKorean ? "사용 불가" : "Unavailable",
      memoryCurrent: isKorean ? "현재 메모리" : "Memory current",
      memoryLimit: isKorean ? "메모리 한도" : "Memory limit",
      oomKills: isKorean ? "OOM kill" : "OOM kills",
      mismatches: isKorean ? "불일치" : "Mismatches",
      matched: isKorean ? "일치" : "Matched",
      viewerLimited: isKorean
        ? "viewer 권한에서는 민감한 프로세스 및 parser 비교 데이터가 제한됩니다."
        : "Viewer role receives limited process and parser diagnostics.",
      nativeUnavailable: isKorean ? "native 결과 없음" : "No native result",
      localUnavailable: isKorean ? "local parser 결과 없음" : "No local parser result"
    };
  });

  $effect(() => {
    if (!parserSessionId) {
      parserSessionId = selectedSessionId ?? visibleSessions[0]?.id ?? "";
    }
  });

  $effect(() => {
    if (runtime) {
      runtimeStatus = runtime;
    }
  });

  onMount(() => {
    void refreshOverview();
  });

  async function refreshOverview() {
    overviewLoading = true;
    overviewError = "";
    try {
      const nextRuntime = await api.getRuntimeStatus();
      runtimeStatus = nextRuntime;
      if (canLoadAdminDiagnostics) {
        const processPayload = await api.getRuntimeProcesses();
        runtimeProcesses = processPayload.processes;
      } else {
        runtimeProcesses = [];
      }
    } catch (error) {
      overviewError = error instanceof Error ? error.message : String(error);
    } finally {
      overviewLoading = false;
    }
  }

  function profileIdForSession(sessionId: string) {
    const loadedSession = sessions.find((session) => session.id === sessionId);
    if (loadedSession?.profileId) {
      return loadedSession.profileId;
    }

    const process = runtimeProcesses.find((entry) =>
      entry.sessions.some((session) => session.sessionId === sessionId)
    );
    return process?.profileId ?? null;
  }

  async function compareParserWithNative() {
    const sessionId = parserSessionId.trim();
    if (!sessionId || !canLoadAdminDiagnostics) {
      return;
    }
    parserLoading = true;
    parserError = "";
    parserResult = null;
    try {
      parserResult = await api.compareParserWithNativeSession(sessionId, parserLimit, profileIdForSession(sessionId));
    } catch (error) {
      parserError = error instanceof Error ? error.message : String(error);
    } finally {
      parserLoading = false;
    }
  }

  function sessionLabel(session: SessionSummary) {
    const title = session.name?.trim() || session.preview?.trim() || session.id;
    return `${title} · ${session.id}`;
  }

  function formatValue(value: unknown) {
    if (value === null || value === undefined) {
      return "null";
    }
    if (typeof value === "string") {
      return value;
    }
    try {
      return JSON.stringify(value, null, 2);
    } catch {
      return String(value);
    }
  }

  function formatBytes(value: number | null | undefined) {
    if (typeof value !== "number" || !Number.isFinite(value) || value <= 0) {
      return "-";
    }
    const units = ["B", "KiB", "MiB", "GiB", "TiB"];
    let next = value;
    let unitIndex = 0;
    while (next >= 1024 && unitIndex < units.length - 1) {
      next /= 1024;
      unitIndex += 1;
    }
    return `${next >= 10 || unitIndex === 0 ? next.toFixed(0) : next.toFixed(1)} ${units[unitIndex]}`;
  }
</script>

<div class="diagnostics-workspace">
  <header class="diagnostics-hero">
    <div>
      <p class="eyebrow">{ui.title}</p>
      <h2>{ui.title}</h2>
      <p>{ui.description}</p>
    </div>
    <button class="diagnostics-button diagnostics-button--primary" disabled={overviewLoading} onclick={() => void refreshOverview()} type="button">
      <RefreshCw size={15} class={overviewLoading ? "animate-spin" : ""} />
      <span>{ui.refresh}</span>
    </button>
  </header>

  {#if overviewError}
    <div class="diagnostics-alert">
      <AlertTriangle size={16} />
      <span>{overviewError}</span>
    </div>
  {/if}

  {#if !canLoadAdminDiagnostics}
    <div class="diagnostics-alert diagnostics-alert--muted">
      <AlertTriangle size={16} />
      <span>{ui.viewerLimited}</span>
    </div>
  {/if}

  <section class="diagnostics-grid">
    <article class="diagnostics-card">
      <div class="diagnostics-card__header">
        <div class="diagnostics-card__title">
          <Activity size={16} />
          <h3>{ui.runtime}</h3>
        </div>
        <span class="diagnostics-pill">{runtimeStatus?.installed ? ui.available : ui.unavailable}</span>
      </div>
      <dl class="diagnostics-kv">
        <div><dt>Codex</dt><dd>{runtimeStatus?.version ?? "-"}</dd></div>
        <div><dt>WebUI</dt><dd>{runtimeStatus?.webuiBuildVersion ?? runtimeStatus?.webuiVersion ?? "-"}</dd></div>
        <div><dt>Commit</dt><dd>{runtimeStatus?.webuiBuildCommitShort ?? "-"}</dd></div>
      </dl>
      {#if runtimeStatus?.issues?.length}
        <div class="diagnostics-issue-list">
          {#each runtimeStatus.issues as issue (issue)}
            <span>{issue}</span>
          {/each}
        </div>
      {/if}
    </article>

    <article class="diagnostics-card">
      <div class="diagnostics-card__header">
        <div class="diagnostics-card__title">
          <Database size={16} />
          <h3>{ui.resources}</h3>
        </div>
        <span class={`diagnostics-pill ${(runtimeStatus?.hostResources?.oomKillCount ?? 0) > 0 ? "diagnostics-pill--warn" : ""}`}>
          {(runtimeStatus?.hostResources?.oomKillCount ?? 0) > 0 ? `${runtimeStatus?.hostResources?.oomKillCount} OOM` : "ok"}
        </span>
      </div>
      <dl class="diagnostics-kv">
        <div><dt>{ui.memoryCurrent}</dt><dd>{formatBytes(runtimeStatus?.hostResources?.memoryCurrentBytes)}</dd></div>
        <div><dt>{ui.memoryLimit}</dt><dd>{formatBytes(runtimeStatus?.hostResources?.memoryMaxBytes ?? runtimeStatus?.hostResources?.procMemTotalBytes)}</dd></div>
        <div><dt>{ui.oomKills}</dt><dd>{runtimeStatus?.hostResources?.oomKillCount ?? 0}</dd></div>
      </dl>
    </article>
  </section>

  <section class="diagnostics-grid">
    <article class="diagnostics-card">
      <div class="diagnostics-card__header">
        <div class="diagnostics-card__title">
          <Server size={16} />
          <h3>{ui.websocket}</h3>
        </div>
        <span class="diagnostics-pill">{connectionState}</span>
      </div>
      <dl class="diagnostics-kv">
        <div><dt>{ui.processes}</dt><dd>{runtimeProcesses.length}</dd></div>
        <div><dt>{ui.selectedSession}</dt><dd>{selectedSessionId ?? "-"}</dd></div>
      </dl>
    </article>

    <article class="diagnostics-card">
      <div class="diagnostics-card__header">
        <div class="diagnostics-card__title">
          <Activity size={16} />
          <h3>{ui.activeSessions}</h3>
        </div>
        <span class="diagnostics-pill">{runningSessions.length}</span>
      </div>
      <dl class="diagnostics-kv">
        <div><dt>{ui.queuedSessions}</dt><dd>{queuedSessions.length}</dd></div>
        <div><dt>{ui.attentionNeeded}</dt><dd>{attentionSessions.length}</dd></div>
        <div><dt>{ui.loadedSummaries}</dt><dd>{sessions.length}</dd></div>
      </dl>
    </article>

    <article class="diagnostics-card">
      <div class="diagnostics-card__header">
        <div class="diagnostics-card__title">
          <AlertTriangle size={16} />
          <h3>{ui.recentEvents}</h3>
        </div>
        <span class="diagnostics-pill">{recentRuntimeNotifications.length}</span>
      </div>
      {#if recentRuntimeNotifications.length === 0}
        <div class="diagnostics-empty">{ui.noRecentEvents}</div>
      {:else}
        <div class="diagnostics-event-list">
          {#each recentRuntimeNotifications as notification (notification.id)}
            <article class="diagnostics-event">
              <strong>{notification.type}</strong>
              <span>{notification.sessionId ?? "-"}</span>
            </article>
          {/each}
        </div>
      {/if}
    </article>
  </section>

  <section class="diagnostics-card">
    <div class="diagnostics-card__header">
      <div class="diagnostics-card__title">
        <Server size={16} />
        <h3>{ui.processes}</h3>
      </div>
      <span class="diagnostics-pill">{runtimeProcesses.length}</span>
    </div>
    {#if runtimeProcesses.length === 0}
      <div class="diagnostics-empty">{overviewLoading ? ui.refresh : ui.noProcesses}</div>
    {:else}
      <div class="diagnostics-process-list">
        {#each runtimeProcesses as process (`${process.profileId}:${process.pid}:${process.clientKey}`)}
          <article class="diagnostics-process">
            <div class="diagnostics-process__top">
              <strong>{process.kind}</strong>
              <span>PID {process.pid}</span>
              <span>{process.pendingRequestCount} pending</span>
            </div>
            <code>{process.clientKey}</code>
            {#if process.sessions.length > 0}
              <div class="diagnostics-session-chips">
                {#each process.sessions as session (session.sessionId)}
                  <button class="diagnostics-chip" onclick={() => void onOpenSession?.(session.sessionId, process.profileId)} type="button">
                    {session.title ?? session.sessionId}
                    <span>{session.status}</span>
                  </button>
                {/each}
              </div>
            {/if}
          </article>
        {/each}
      </div>
    {/if}
  </section>

  <section class="diagnostics-card">
    <div class="diagnostics-card__header">
      <div class="diagnostics-card__title">
        <Database size={16} />
        <h3>{ui.parserCompare}</h3>
      </div>
      {#if parserResult}
        <span class={`diagnostics-pill ${parserResult.ok ? "diagnostics-pill--ok" : "diagnostics-pill--warn"}`}>
          {parserResult.ok ? ui.matched : `${parserResult.mismatchCount} ${ui.mismatches}`}
        </span>
      {/if}
    </div>
    <p class="diagnostics-note">{ui.parserDescription}</p>
    <div class="diagnostics-controls">
      <label>
        <span>{ui.selectedSession}</span>
        {#if visibleSessions.length > 0}
          <select bind:value={parserSessionId} disabled={!canLoadAdminDiagnostics || parserLoading}>
            {#each visibleSessions as session (session.id)}
              <option value={session.id}>{sessionLabel(session)}</option>
            {/each}
          </select>
        {:else}
          <input bind:value={parserSessionId} disabled={!canLoadAdminDiagnostics || parserLoading} placeholder="session id" />
        {/if}
      </label>
      <label>
        <span>{ui.recentTurns}</span>
        <input
          bind:value={parserLimit}
          disabled={!canLoadAdminDiagnostics || parserLoading}
          max="20"
          min="1"
          type="number"
        />
      </label>
      <button
        class="diagnostics-button diagnostics-button--primary"
        disabled={!canLoadAdminDiagnostics || parserLoading || !parserSessionId.trim()}
        onclick={() => void compareParserWithNative()}
        type="button"
      >
        <Search size={15} />
        <span>{parserLoading ? ui.refresh : ui.compare}</span>
      </button>
    </div>

    {#if parserError}
      <div class="diagnostics-alert">
        <AlertTriangle size={16} />
        <span>{parserError}</span>
      </div>
    {/if}

    {#if parserResult}
      <div class="diagnostics-result">
        <div class="diagnostics-result__summary">
          <span class={parserResult.local.available ? "ok" : "warn"}>
            {#if parserResult.local.available}<CheckCircle2 size={14} />{:else}<AlertTriangle size={14} />{/if}
            {parserResult.local.available ? "local" : ui.localUnavailable}
          </span>
          <span class={parserResult.native.available ? "ok" : "warn"}>
            {#if parserResult.native.available}<CheckCircle2 size={14} />{:else}<AlertTriangle size={14} />{/if}
            {parserResult.native.available ? "native" : ui.nativeUnavailable}
          </span>
        </div>

        {#if parserResult.mismatches.length === 0}
          <div class="diagnostics-empty">{ui.matched}</div>
        {:else}
          <div class="diagnostics-mismatch-list">
            {#each parserResult.mismatches as mismatch (`${mismatch.category}:${mismatch.field}`)}
              <article class="diagnostics-mismatch">
                <header>
                  <strong>{mismatch.category}</strong>
                  <span>{mismatch.field}</span>
                </header>
                <div class="diagnostics-mismatch__values">
                  <pre>{formatValue(mismatch.local)}</pre>
                  <pre>{formatValue(mismatch.native)}</pre>
                </div>
              </article>
            {/each}
          </div>
        {/if}
      </div>
    {/if}
  </section>
</div>

<style>
  .diagnostics-workspace {
    display: grid;
    gap: 1rem;
    min-height: 0;
    padding: 1.25rem;
    color: var(--ink);
  }

  .diagnostics-hero,
  .diagnostics-card,
  .diagnostics-process,
  .diagnostics-alert {
    border: 1px solid var(--line);
    background: var(--panel-strong);
    box-shadow: 0 18px 40px -32px rgba(15, 23, 42, 0.45);
  }

  .diagnostics-hero {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    border-radius: 1.5rem;
    padding: 1rem;
  }

  .diagnostics-hero h2 {
    margin: 0.15rem 0 0;
    color: var(--ink-strong);
    font-size: 1.25rem;
    font-weight: 800;
  }

  .diagnostics-hero p,
  .diagnostics-note {
    margin: 0.35rem 0 0;
    color: var(--muted);
    font-size: 0.84rem;
    line-height: 1.45;
  }

  .eyebrow {
    margin: 0;
    color: var(--muted);
    font-size: 0.64rem;
    font-weight: 800;
    letter-spacing: 0.18em;
    text-transform: uppercase;
  }

  .diagnostics-grid {
    display: grid;
    gap: 1rem;
    grid-template-columns: repeat(auto-fit, minmax(16rem, 1fr));
  }

  .diagnostics-card {
    border-radius: 1.25rem;
    padding: 1rem;
  }

  .diagnostics-card__header,
  .diagnostics-process__top,
  .diagnostics-result__summary {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
  }

  .diagnostics-card__title {
    display: flex;
    align-items: center;
    gap: 0.55rem;
    color: var(--ink-strong);
  }

  .diagnostics-card__title h3 {
    margin: 0;
    font-size: 0.95rem;
    font-weight: 800;
  }

  .diagnostics-pill,
  .diagnostics-chip span {
    border-radius: 999px;
    background: var(--panel-soft);
    color: var(--muted);
    padding: 0.22rem 0.55rem;
    font-size: 0.68rem;
    font-weight: 800;
    letter-spacing: 0.06em;
    text-transform: uppercase;
  }

  .diagnostics-pill--ok {
    background: rgba(16, 185, 129, 0.12);
    color: #059669;
  }

  .diagnostics-pill--warn {
    background: rgba(245, 158, 11, 0.14);
    color: #d97706;
  }

  .diagnostics-kv {
    display: grid;
    gap: 0.55rem;
    margin: 0.85rem 0 0;
  }

  .diagnostics-kv div {
    display: flex;
    justify-content: space-between;
    gap: 1rem;
    border-top: 1px solid var(--line);
    padding-top: 0.55rem;
  }

  .diagnostics-kv dt {
    color: var(--muted);
    font-size: 0.75rem;
    font-weight: 700;
  }

  .diagnostics-kv dd {
    margin: 0;
    max-width: 60%;
    overflow: hidden;
    color: var(--ink-strong);
    font-size: 0.78rem;
    font-weight: 800;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .diagnostics-button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 0.45rem;
    border-radius: 0.85rem;
    border: 1px solid var(--line);
    padding: 0.55rem 0.75rem;
    color: var(--ink);
    font-size: 0.78rem;
    font-weight: 800;
    transition:
      transform 140ms ease,
      background-color 140ms ease,
      opacity 140ms ease;
  }

  .diagnostics-button:hover:not(:disabled) {
    transform: translateY(-1px);
    background: var(--panel-soft);
  }

  .diagnostics-button:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }

  .diagnostics-button--primary {
    background: var(--ink-strong);
    color: var(--panel-strong);
  }

  .diagnostics-alert {
    display: flex;
    align-items: center;
    gap: 0.55rem;
    border-radius: 1rem;
    padding: 0.75rem 0.9rem;
    color: #b45309;
    font-size: 0.82rem;
    font-weight: 700;
  }

  .diagnostics-alert--muted {
    color: var(--muted);
  }

  .diagnostics-process-list,
  .diagnostics-event-list,
  .diagnostics-mismatch-list,
  .diagnostics-issue-list {
    display: grid;
    gap: 0.7rem;
    margin-top: 0.8rem;
  }

  .diagnostics-process {
    border-radius: 1rem;
    padding: 0.8rem;
  }

  .diagnostics-process code {
    display: block;
    margin-top: 0.45rem;
    overflow: hidden;
    color: var(--muted);
    font-size: 0.72rem;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .diagnostics-process__top {
    justify-content: flex-start;
    color: var(--ink-strong);
    font-size: 0.78rem;
    font-weight: 800;
  }

  .diagnostics-process__top span {
    color: var(--muted);
    font-size: 0.72rem;
  }

  .diagnostics-session-chips {
    display: flex;
    flex-wrap: wrap;
    gap: 0.45rem;
    margin-top: 0.7rem;
  }

  .diagnostics-event {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    border: 1px solid var(--line);
    border-radius: 0.9rem;
    padding: 0.58rem 0.7rem;
    color: var(--ink);
    font-size: 0.76rem;
    font-weight: 700;
  }

  .diagnostics-event span {
    min-width: 0;
    overflow: hidden;
    color: var(--muted);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .diagnostics-chip {
    display: inline-flex;
    max-width: 18rem;
    align-items: center;
    gap: 0.45rem;
    border-radius: 999px;
    border: 1px solid var(--line);
    background: var(--panel-soft);
    color: var(--ink);
    padding: 0.32rem 0.55rem;
    font-size: 0.72rem;
    font-weight: 700;
  }

  .diagnostics-controls {
    display: grid;
    align-items: end;
    gap: 0.75rem;
    grid-template-columns: minmax(0, 1fr) 7rem auto;
    margin-top: 0.9rem;
  }

  .diagnostics-controls label {
    display: grid;
    gap: 0.3rem;
    color: var(--muted);
    font-size: 0.68rem;
    font-weight: 800;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  .diagnostics-controls input,
  .diagnostics-controls select {
    min-width: 0;
    border: 1px solid var(--line);
    border-radius: 0.85rem;
    background: var(--panel-soft);
    color: var(--ink-strong);
    padding: 0.55rem 0.65rem;
    font-size: 0.8rem;
    outline: none;
  }

  .diagnostics-empty {
    margin-top: 0.8rem;
    border: 1px dashed var(--line);
    border-radius: 1rem;
    color: var(--muted);
    padding: 1rem;
    text-align: center;
    font-size: 0.82rem;
    font-weight: 700;
  }

  .diagnostics-result {
    margin-top: 1rem;
  }

  .diagnostics-result__summary {
    justify-content: flex-start;
  }

  .diagnostics-result__summary span {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    border-radius: 999px;
    padding: 0.25rem 0.55rem;
    font-size: 0.72rem;
    font-weight: 800;
  }

  .diagnostics-result__summary .ok {
    background: rgba(16, 185, 129, 0.12);
    color: #059669;
  }

  .diagnostics-result__summary .warn {
    background: rgba(245, 158, 11, 0.14);
    color: #d97706;
  }

  .diagnostics-mismatch {
    overflow: hidden;
    border: 1px solid var(--line);
    border-radius: 1rem;
  }

  .diagnostics-mismatch header {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    background: var(--panel-soft);
    padding: 0.6rem 0.75rem;
    color: var(--ink-strong);
    font-size: 0.78rem;
  }

  .diagnostics-mismatch header span {
    color: var(--muted);
  }

  .diagnostics-mismatch__values {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }

  .diagnostics-mismatch pre {
    max-height: 14rem;
    overflow: auto;
    margin: 0;
    border-top: 1px solid var(--line);
    padding: 0.75rem;
    color: var(--muted);
    font-size: 0.72rem;
    line-height: 1.45;
    white-space: pre-wrap;
  }

  .diagnostics-mismatch pre + pre {
    border-left: 1px solid var(--line);
  }

  .diagnostics-issue-list span {
    border-radius: 0.8rem;
    background: rgba(245, 158, 11, 0.1);
    color: #b45309;
    padding: 0.45rem 0.6rem;
    font-size: 0.76rem;
    font-weight: 700;
  }

  @media (max-width: 760px) {
    .diagnostics-workspace {
      padding: 0.9rem;
    }

    .diagnostics-hero,
    .diagnostics-card__header {
      align-items: stretch;
      flex-direction: column;
    }

    .diagnostics-controls,
    .diagnostics-mismatch__values {
      grid-template-columns: 1fr;
    }

    .diagnostics-mismatch pre + pre {
      border-left: 0;
    }
  }
</style>
