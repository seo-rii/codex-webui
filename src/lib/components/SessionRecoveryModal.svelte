<script lang="ts">
  import { AlertCircle, RefreshCw, X } from "lucide-svelte";

  import { m } from "$lib/paraglide/messages.js";

  type SessionRecoveryPromptState = {
    sessionId: string;
    message: string;
    issue: string | null;
    totalLines: number | null;
    recoverableLines: number | null;
    skippedLines: number | null;
    busy: boolean;
  };

  let {
    prompt,
    getIssueLabel,
    onDismiss,
    onRecover
  }: {
    prompt: SessionRecoveryPromptState;
    getIssueLabel: (issue: string | null) => string;
    onDismiss: () => void;
    onRecover: () => void | Promise<void>;
  } = $props();
</script>

<div
  aria-labelledby="session-recovery-title"
  aria-modal="true"
  class="ui-scrim ui-scrim--modal fixed inset-0 z-[116] overflow-y-auto"
  role="dialog"
>
  <div class="flex min-h-full items-center justify-center p-4 sm:p-8">
    <div class="auth-dialog-card w-full max-w-2xl rounded-[2rem] border border-white/70 bg-white/92 p-6 shadow-[0_32px_90px_rgba(15,23,42,0.24)] backdrop-blur-2xl sm:p-8">
      <div class="flex items-start justify-between gap-4">
        <div class="space-y-3">
          <div class="inline-flex h-11 w-11 items-center justify-center rounded-2xl bg-amber-100 text-amber-700 shadow-sm">
            <AlertCircle size={18} />
          </div>
          <div>
            <h2 id="session-recovery-title" class="text-xl font-bold tracking-tight text-gray-950">
              {m.session_history_recovery_title()}
            </h2>
            <p class="mt-2 text-sm leading-relaxed text-gray-600">
              {m.session_history_recovery_description()}
            </p>
          </div>
        </div>
        <button
          aria-label={m.close()}
          class="inline-flex h-10 w-10 items-center justify-center rounded-2xl border border-gray-200 text-gray-500 transition-colors hover:bg-gray-50 hover:text-gray-900"
          onclick={onDismiss}
          type="button"
        >
          <X size={18} />
        </button>
      </div>

      <div class="mt-6 rounded-3xl border border-amber-200 bg-amber-50/80 p-4">
        <p class="text-sm font-semibold text-amber-900">
          {getIssueLabel(prompt.issue)}
        </p>
        <p class="mt-2 text-sm leading-relaxed text-amber-800/90">
          {prompt.message || m.session_history_recovery_generic_message()}
        </p>
      </div>

      <div class="mt-4 grid gap-3 sm:grid-cols-3">
        <div class="rounded-2xl border border-gray-200 bg-gray-50/80 px-4 py-3">
          <div class="text-[11px] font-bold uppercase tracking-[0.18em] text-gray-400">{m.session_history_recovery_total_lines()}</div>
          <div class="mt-1 text-lg font-semibold text-gray-900">{prompt.totalLines ?? "—"}</div>
        </div>
        <div class="rounded-2xl border border-gray-200 bg-gray-50/80 px-4 py-3">
          <div class="text-[11px] font-bold uppercase tracking-[0.18em] text-gray-400">{m.session_history_recovery_recoverable_lines()}</div>
          <div class="mt-1 text-lg font-semibold text-gray-900">{prompt.recoverableLines ?? "—"}</div>
        </div>
        <div class="rounded-2xl border border-gray-200 bg-gray-50/80 px-4 py-3">
          <div class="text-[11px] font-bold uppercase tracking-[0.18em] text-gray-400">{m.session_history_recovery_skipped_lines()}</div>
          <div class="mt-1 text-lg font-semibold text-gray-900">{prompt.skippedLines ?? "—"}</div>
        </div>
      </div>

      <p class="mt-4 text-sm leading-relaxed text-gray-600">
        {m.session_history_recovery_backup_notice()}
      </p>

      <div class="mt-6 flex flex-col-reverse gap-3 sm:flex-row sm:justify-end">
        <button
          class="ui-animated-button ui-animated-button--soft inline-flex items-center justify-center rounded-2xl border border-gray-200 bg-white px-4 py-2.5 text-sm font-semibold text-gray-700 shadow-sm transition-colors hover:bg-gray-50"
          onclick={onDismiss}
          type="button"
        >
          {m.not_now()}
        </button>
        <button
          class="ui-animated-button inline-flex items-center justify-center gap-2 rounded-2xl bg-amber-600 px-4 py-2.5 text-sm font-semibold text-white shadow-lg shadow-amber-500/20 transition-colors hover:bg-amber-700 disabled:cursor-not-allowed disabled:opacity-60"
          disabled={prompt.busy}
          onclick={() => void onRecover()}
          type="button"
        >
          {#if prompt.busy}
            <RefreshCw size={15} class="animate-spin" />
          {/if}
          <span>{m.session_history_recovery_action()}</span>
        </button>
      </div>
    </div>
  </div>
</div>
