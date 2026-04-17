<script lang="ts">
  import { onMount } from "svelte";
  import { BarChart3, ExternalLink, Play, RefreshCw, Sparkles } from "lucide-svelte";

  import { api } from "$lib/api";
  import MarkdownMessage from "$lib/components/MarkdownMessage.svelte";
  import { localeSignal } from "$lib/i18n";
  import { getLocale } from "$lib/paraglide/runtime.js";
  import type { ArenaContestant, ArenaRun } from "$lib/arena-types";
  import type { ModelOption, SessionPreferences } from "$lib/types";

  let {
    models = [],
    currentPreferences = null,
    readOnly = false,
    onOpenSession = null,
    onUseResponse = null
  }: {
    models?: ModelOption[];
    currentPreferences?: Partial<SessionPreferences> | null;
    readOnly?: boolean;
    onOpenSession?: ((sessionId: string) => void | Promise<void>) | null;
    onUseResponse?: ((contestant: ArenaContestant) => void | Promise<void>) | null;
  } = $props();

  let prompt = $state("");
  let selectedModelIds = $state<string[]>([]);
  let runs = $state<ArenaRun[]>([]);
  let loading = $state(true);
  let starting = $state(false);
  let errorText = $state("");
  let lastDefaultsKey = $state("");
  let pollingTimer: ReturnType<typeof setInterval> | null = null;

  const copy = $derived.by(() => {
    const _locale = $localeSignal;
    const locale = getLocale();
    if (locale === "ko") {
      return {
        title: "Best-of-N Arena",
        subtitle: "같은 프롬프트를 여러 모델에 동시에 보내고 결과를 비교합니다.",
        prompt: "Arena 프롬프트",
        chooseModels: "비교할 모델",
        run: "Arena 실행",
        refresh: "새로고침",
        loading: "Arena 실행 결과를 불러오는 중입니다…",
        empty: "아직 실행한 Arena 비교가 없습니다.",
        needPrompt: "프롬프트를 입력하고 최소 두 개의 모델을 선택하세요.",
        running: "실행 중",
        completed: "완료",
        useResponse: "응답 가져오기",
        openThread: "스레드 열기",
        noResponse: "아직 최종 응답이 없습니다.",
        contestants: "비교 대상"
      };
    }

    return {
      title: "Best-of-N Arena",
      subtitle: "Send one prompt to several models at once and compare the final answers.",
      prompt: "Arena prompt",
      chooseModels: "Models to compare",
      run: "Run arena",
      refresh: "Refresh",
      loading: "Loading arena runs…",
      empty: "No arena comparisons have been started yet.",
      needPrompt: "Enter a prompt and choose at least two models.",
      running: "Running",
      completed: "Completed",
      useResponse: "Use response",
      openThread: "Open thread",
      noResponse: "No final response yet.",
      contestants: "Contestants"
    };
  });

  const running = $derived.by(() => runs.some((run) => run.status === "running"));

  onMount(() => {
    initializeModelSelection();
    void refreshRuns();
  });

  $effect(() => {
    const availableIds = models.map((model) => model.id).join("|");
    if (availableIds === lastDefaultsKey) {
      return;
    }
    lastDefaultsKey = availableIds;
    initializeModelSelection();
  });

  $effect(() => {
    if (typeof window === "undefined") {
      return;
    }
    if (!running) {
      if (pollingTimer) {
        clearInterval(pollingTimer);
        pollingTimer = null;
      }
      return;
    }

    pollingTimer = setInterval(() => {
      void refreshRuns(true);
    }, 2500);

    return () => {
      if (pollingTimer) {
        clearInterval(pollingTimer);
        pollingTimer = null;
      }
    };
  });

  function initializeModelSelection() {
    const available = models.map((model) => model.id);
    if (available.length === 0) {
      selectedModelIds = [];
      return;
    }

    const preferred = [
      currentPreferences?.model ?? null,
      ...available.filter((modelId) => modelId !== currentPreferences?.model)
    ].filter((modelId): modelId is string => Boolean(modelId && available.includes(modelId)));

    selectedModelIds = [...new Set(preferred)].slice(0, Math.min(3, Math.max(2, preferred.length || 2)));
    if (selectedModelIds.length < 2) {
      selectedModelIds = available.slice(0, Math.min(2, available.length));
    }
  }

  async function refreshRuns(silent = false) {
    if (!silent) {
      loading = true;
    }
    errorText = "";

    try {
      const payload = await api.listArenaRuns();
      runs = payload.runs;
    } catch (error) {
      errorText = error instanceof Error ? error.message : copy.loading;
    } finally {
      loading = false;
    }
  }

  function toggleModel(modelId: string, enabled: boolean) {
    if (enabled) {
      selectedModelIds = [...new Set([...selectedModelIds, modelId])].slice(0, 4);
      return;
    }
    selectedModelIds = selectedModelIds.filter((entry) => entry !== modelId);
  }

  async function startArena() {
    if (readOnly || starting) {
      return;
    }
    if (!prompt.trim() || selectedModelIds.length < 2) {
      errorText = copy.needPrompt;
      return;
    }

    starting = true;
    errorText = "";

    try {
      const contestants = selectedModelIds
        .map((modelId) => {
          const model = models.find((entry) => entry.id === modelId);
          if (!model) {
            return null;
          }
          return {
            model: model.id,
            label: model.displayName || model.id
          };
        })
        .filter((entry): entry is { model: string; label: string } => Boolean(entry));

      await api.startArenaRun(prompt.trim(), contestants, currentPreferences ?? {});
      prompt = "";
      await refreshRuns(true);
    } catch (error) {
      errorText = error instanceof Error ? error.message : copy.needPrompt;
    } finally {
      starting = false;
    }
  }

  function formatTimestamp(value: number) {
    return new Date(value).toLocaleString(getLocale() === "ko" ? "ko-KR" : "en-US");
  }
</script>

<section class="arena-shell surface">
  <div class="arena-header">
    <div>
      <p class="eyebrow">{copy.title}</p>
      <h2>{copy.title}</h2>
      <p class="arena-subtitle">{copy.subtitle}</p>
    </div>
    <button class="ghost-button" disabled={loading} type="button" onclick={() => void refreshRuns()}>
      <RefreshCw class={loading ? "spin" : ""} size={15} />
      <span>{copy.refresh}</span>
    </button>
  </div>

  {#if errorText}
    <div class="error-banner small">{errorText}</div>
  {/if}

  <section class="arena-composer">
    <label class="field">
      <span>{copy.prompt}</span>
      <textarea bind:value={prompt} disabled={readOnly || starting} placeholder={copy.prompt} rows="4"></textarea>
    </label>

    <div class="field">
      <span>{copy.chooseModels}</span>
      <div class="arena-model-grid">
        {#each models as model (model.id)}
          <label class:checkbox-card--disabled={readOnly} class="checkbox-card">
            <input
              checked={selectedModelIds.includes(model.id)}
              class="checkbox-input"
              disabled={readOnly}
              onchange={(event) => toggleModel(model.id, (event.currentTarget as HTMLInputElement).checked)}
              type="checkbox"
            />
            <span aria-hidden="true" class="checkbox-control"></span>
            <span class="checkbox-copy">
              <span class="checkbox-title">{model.displayName}</span>
              <span class="checkbox-caption">{model.id}</span>
            </span>
          </label>
        {/each}
      </div>
    </div>

    <div class="arena-actions">
      <span class="field-note">{selectedModelIds.length} {copy.contestants}</span>
      <button class="solid-button" disabled={readOnly || starting || !prompt.trim() || selectedModelIds.length < 2} type="button" onclick={() => void startArena()}>
        <Play size={15} />
        <span>{starting ? copy.running : copy.run}</span>
      </button>
    </div>
  </section>

  {#if loading}
    <div class="placeholder-card">{copy.loading}</div>
  {:else if runs.length === 0}
    <div class="placeholder-card">{copy.empty}</div>
  {:else}
    <div class="arena-run-list">
      {#each runs as run (run.id)}
        <article class="arena-run-card">
          <div class="arena-run-header">
            <div class="arena-run-title">
              <div class="arena-run-title-row">
                <BarChart3 size={16} />
                <h3>{run.prompt}</h3>
              </div>
              <span>{formatTimestamp(run.createdAt)}</span>
            </div>
            <div class={`meta-pill ${run.status === "running" ? "subtle" : ""}`}>
              {run.status === "running" ? copy.running : copy.completed}
            </div>
          </div>

          <div class="arena-contestant-grid">
            {#each run.contestants as contestant (contestant.id)}
              <section class="arena-contestant-card">
                <div class="arena-contestant-header">
                  <div>
                    <h4>{contestant.label}</h4>
                    <span>{contestant.status}</span>
                  </div>
                  <Sparkles size={15} class="arena-contestant-icon" />
                </div>

                <div class="arena-response">
                  {#if contestant.response}
                    <MarkdownMessage text={contestant.response} />
                  {:else}
                    <p class="field-note">{copy.noResponse}</p>
                  {/if}
                </div>

                <div class="arena-contestant-actions">
                  <button class="ghost-button small" disabled={!contestant.response} type="button" onclick={() => void onUseResponse?.(contestant)}>
                    {copy.useResponse}
                  </button>
                  <button class="ghost-button small" type="button" onclick={() => void onOpenSession?.(contestant.sessionId)}>
                    <ExternalLink size={14} />
                    <span>{copy.openThread}</span>
                  </button>
                </div>
              </section>
            {/each}
          </div>
        </article>
      {/each}
    </div>
  {/if}
</section>

<style>
  .arena-shell {
    display: grid;
    gap: 0.9rem;
    padding: 1rem;
    border-radius: 1.5rem;
    border: 1px solid rgba(83, 61, 42, 0.1);
    background: rgba(255, 255, 255, 0.8);
  }

  .arena-header,
  .arena-actions,
  .arena-run-header,
  .arena-contestant-header,
  .arena-contestant-actions {
    display: flex;
    gap: 0.75rem;
    align-items: center;
    justify-content: space-between;
  }

  .arena-header h2,
  .arena-run-title h3,
  .arena-contestant-header h4 {
    margin: 0.15rem 0 0;
    color: var(--ink-strong);
    font: 600 1.1rem/1.12 var(--font-display);
  }

  .arena-subtitle {
    margin: 0.45rem 0 0;
    color: var(--muted);
    font-size: 0.86rem;
  }

  .arena-composer,
  .arena-run-list,
  .arena-contestant-grid {
    display: grid;
    gap: 0.85rem;
  }

  .arena-model-grid {
    display: grid;
    gap: 0.6rem;
    grid-template-columns: repeat(auto-fit, minmax(12rem, 1fr));
  }

  .arena-run-card,
  .arena-contestant-card {
    display: grid;
    gap: 0.7rem;
    border-radius: 1.2rem;
    border: 1px solid rgba(83, 61, 42, 0.1);
    background: rgba(249, 245, 239, 0.72);
    padding: 0.9rem;
  }

  .arena-run-title,
  .arena-contestant-header > div {
    display: grid;
    gap: 0.24rem;
    min-width: 0;
  }

  .arena-run-title-row {
    display: flex;
    gap: 0.55rem;
    align-items: center;
  }

  .arena-run-title span,
  .arena-contestant-header span {
    color: var(--muted);
    font-size: 0.74rem;
  }

  .arena-contestant-grid {
    grid-template-columns: repeat(auto-fit, minmax(17rem, 1fr));
  }

  .arena-response {
    min-height: 5rem;
    max-height: 18rem;
    overflow: auto;
    border-radius: 1rem;
    background: rgba(255, 255, 255, 0.85);
    padding: 0.8rem 0.9rem;
  }

  .arena-contestant-icon {
    color: var(--muted);
  }

  textarea {
    width: 100%;
    min-height: 6.5rem;
    resize: vertical;
    border: 1px solid rgba(83, 61, 42, 0.14);
    border-radius: 1.1rem;
    background: rgba(255, 255, 255, 0.86);
    color: var(--ink);
    padding: 0.82rem 0.92rem;
  }

  .spin {
    animation: arena-spin 0.9s linear infinite;
  }

  @media (max-width: 720px) {
    .arena-header,
    .arena-actions,
    .arena-run-header,
    .arena-contestant-header,
    .arena-contestant-actions {
      flex-direction: column;
      align-items: stretch;
    }
  }

  @keyframes arena-spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }
</style>
