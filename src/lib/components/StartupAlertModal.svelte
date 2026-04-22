<script lang="ts">
  import { AlertCircle, Clock } from "lucide-svelte";

  import { m } from "$lib/paraglide/messages.js";

  type StartupQueueAlert = {
    sessionId: string;
    name: string | null;
    cwd: string | null;
    pendingCount: number;
    updatedAt: number | null;
  };

  type ScheduledShutdown = {
    sessionId: string | null;
    scheduledFor: number;
  };

  type UiCopy = {
    startupAlertTitle: string;
    startupAlertDescription: string;
    startupAlertContinue: string;
    startupAlertPausedQueues: string;
    startupAlertPausedQueuesDescription: string;
    startupAlertPendingTasks: (count: number) => string;
    startupAlertOpenThread: string;
    startupAlertScheduledShutdown: string;
    startupAlertShutdownCountdown: (seconds: number) => string;
  };

  let {
    pausedQueues,
    scheduledShutdown,
    shutdownRemainingSeconds,
    scheduledShutdownThreadLabel,
    shutdownDelaySeconds,
    fallbackThreadTitle,
    ui,
    onDismiss,
    onOpenSession
  }: {
    pausedQueues: StartupQueueAlert[];
    scheduledShutdown: ScheduledShutdown | null;
    shutdownRemainingSeconds: number | null;
    scheduledShutdownThreadLabel: string | null;
    shutdownDelaySeconds: number;
    fallbackThreadTitle: string;
    ui: UiCopy;
    onDismiss: () => void;
    onOpenSession: (sessionId: string) => void | Promise<void>;
  } = $props();
</script>

<div
  aria-labelledby="startup-alert-title"
  aria-modal="true"
  class="ui-scrim ui-scrim--modal fixed inset-0 z-[115] overflow-y-auto"
  role="dialog"
>
  <div class="flex min-h-full items-center justify-center p-4 sm:p-8">
    <div class="startup-alert-card w-full max-w-4xl overflow-hidden rounded-[2rem] border border-white/10 bg-white shadow-2xl">
      <div class="startup-alert-card__hero border-b border-gray-200 bg-gradient-to-br from-amber-50 via-white to-white px-6 py-6 sm:px-8">
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
            onclick={onDismiss}
            type="button"
          >
            {ui.startupAlertContinue}
          </button>
        </div>
      </div>

      <div class={`grid gap-6 p-6 sm:p-8 ${pausedQueues.length > 0 && scheduledShutdown ? "sm:grid-cols-[minmax(0,1.2fr)_minmax(0,0.8fr)]" : ""}`}>
        {#if pausedQueues.length > 0}
          <div class="space-y-4">
            <div class="startup-alert-card__section rounded-3xl border border-gray-200 bg-gray-50/70 p-5 shadow-sm">
              <div class="flex items-start justify-between gap-4">
                <div>
                  <h3 class="text-sm font-bold text-gray-900">{ui.startupAlertPausedQueues}</h3>
                  <p class="mt-1 text-sm leading-relaxed text-gray-500">
                    {ui.startupAlertPausedQueuesDescription}
                  </p>
                </div>
                <span class="inline-flex items-center rounded-full bg-amber-100 px-3 py-1 text-[10px] font-bold uppercase tracking-[0.2em] text-amber-700">
                  {pausedQueues.length}
                </span>
              </div>

              <div class="mt-5 space-y-3">
                {#each pausedQueues as queueAlert (queueAlert.sessionId)}
                  <div class="startup-alert-card__queue rounded-2xl border border-gray-200 bg-white p-4 shadow-sm">
                    <div class="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
                      <div class="min-w-0">
                        <div class="truncate text-sm font-semibold text-gray-900">
                          {queueAlert.name || fallbackThreadTitle}
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
                        onclick={() => void onOpenSession(queueAlert.sessionId)}
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

        {#if scheduledShutdown}
          <div class="space-y-4">
            <div class="startup-alert-card__section startup-alert-card__section--accent rounded-3xl border border-amber-200 bg-gradient-to-br from-amber-50 to-white p-5 shadow-sm">
              <div class="flex items-center gap-3">
                <div class="flex h-10 w-10 items-center justify-center rounded-2xl bg-amber-100 text-amber-700">
                  <Clock size={18} />
                </div>
                <div>
                  <h3 class="text-sm font-bold text-gray-900">{ui.startupAlertScheduledShutdown}</h3>
                  {#if shutdownRemainingSeconds !== null}
                    <p class="mt-1 text-sm text-gray-600">
                      {ui.startupAlertShutdownCountdown(shutdownRemainingSeconds)}
                    </p>
                  {/if}
                </div>
              </div>
              {#if scheduledShutdown.sessionId && scheduledShutdownThreadLabel}
                <div class="startup-alert-card__callout mt-4 rounded-2xl border border-white/80 bg-white/80 px-4 py-3 text-sm text-gray-600 shadow-sm">
                  {scheduledShutdownThreadLabel}
                </div>
              {/if}
              <p class="mt-4 text-xs leading-relaxed text-gray-500">
                {m.shutdown_wait_description({ seconds: String(shutdownDelaySeconds) })}
              </p>
            </div>
          </div>
        {/if}
      </div>
    </div>
  </div>
</div>
