<script lang="ts">
  import { onMount } from "svelte";
  import { Plug, RefreshCw, Save, Settings2, Sparkles } from "lucide-svelte";

  import { api } from "$lib/api";
  import MonacoTextEditor from "$lib/components/MonacoTextEditor.svelte";
  import { localeSignal } from "$lib/i18n";
  import { m } from "$lib/paraglide/messages.js";
  import type { CatalogPayload, EditableFilePayload, NotificationEventType, NotificationSettings } from "$lib/types";

  let {
    codexHome,
    configFilePath,
    notificationSettings = null,
    onNotificationSettingsSaved = null,
    onConfigSaved = null
  }: {
    codexHome: string;
    configFilePath: string;
    notificationSettings?: NotificationSettings | null;
    onNotificationSettingsSaved?: ((settings: Partial<NotificationSettings>) => void | Promise<void>) | null;
    onConfigSaved?: (() => void | Promise<void>) | null;
  } = $props();

  let configFile = $state<EditableFilePayload | null>(null);
  let catalog = $state<CatalogPayload | null>(null);
  let editorValue = $state("");
  let errorText = $state("");
  let notificationSlackWebhookUrl = $state("");
  let notificationWebhookUrl = $state("");
  let notificationEventTypes = $state<NotificationEventType[]>([]);
  let loading = $state(true);
  let saving = $state(false);
  let reloading = $state(false);
  let savingNotifications = $state(false);
  const dirty = $derived(Boolean(configFile && editorValue !== configFile.content));
  const notificationDirty = $derived.by(() => {
    const current = notificationSettings;
    if (!current) {
      return false;
    }
    const left = [...notificationEventTypes].sort().join(",");
    const right = [...current.enabledEventTypes].sort().join(",");
    return (
      left !== right ||
      notificationSlackWebhookUrl.trim() !== (current.slackWebhookUrl ?? "") ||
      notificationWebhookUrl.trim() !== (current.webhookUrl ?? "")
    );
  });
  const ui = $derived.by(() => {
    const _locale = $localeSignal;

    return {
      settings: m.settings(),
      workspace: m.codex_workspace(),
      reload: m.reload(),
      saving: m.saving(),
      saveConfig: m.save_config_toml(),
      failedLoad: m.failed_to_load_settings_workspace(),
      failedSave: m.failed_to_save_config_toml(),
      loadingWorkspace: m.loading_settings_workspace(),
      configTitle: m.codex_config_toml(),
      unsaved: m.unsaved(),
      editableFile: m.editable_file(),
      notifications: m.notifications(),
      notificationCenter: m.notification_center(),
      notificationEvents: m.notification_events(),
      slackWebhookUrl: m.slack_webhook_url(),
      genericWebhookUrl: m.generic_webhook_url(),
      saveNotificationSettings: m.save_notification_settings(),
      notificationSessionCompleted: m.notification_session_completed(),
      notificationInputRequired: m.notification_input_required(),
      notificationQueueFailed: m.notification_queue_failed(),
      notificationShutdownScheduled: m.notification_shutdown_scheduled(),
      installedPlugins: m.installed_plugins(),
      noPlugins: m.no_installed_plugins(),
      noDescription: m.no_description(),
      installedSkills: m.installed_skills(),
      noSkills: m.no_local_skills()
    };
  });

  onMount(() => {
    void bootstrap();
  });

  $effect(() => {
    const current = notificationSettings;
    if (!current) {
      notificationSlackWebhookUrl = "";
      notificationWebhookUrl = "";
      notificationEventTypes = [];
      return;
    }

    notificationSlackWebhookUrl = current.slackWebhookUrl ?? "";
    notificationWebhookUrl = current.webhookUrl ?? "";
    notificationEventTypes = [...current.enabledEventTypes];
  });

  function toggleNotificationEvent(eventType: NotificationEventType, enabled: boolean) {
    if (enabled) {
      notificationEventTypes = [...new Set([...notificationEventTypes, eventType])];
      return;
    }
    notificationEventTypes = notificationEventTypes.filter((entry) => entry !== eventType);
  }

  function notificationEventLabel(eventType: NotificationEventType) {
    if (eventType === "sessionCompleted") {
      return ui.notificationSessionCompleted;
    }
    if (eventType === "sessionAttention") {
      return ui.notificationInputRequired;
    }
    if (eventType === "queueDispatchFailed") {
      return ui.notificationQueueFailed;
    }
    return ui.notificationShutdownScheduled;
  }

  async function saveNotifications() {
    if (!notificationDirty) {
      return;
    }

    savingNotifications = true;
    errorText = "";

    try {
      await onNotificationSettingsSaved?.({
        enabledEventTypes: notificationEventTypes,
        slackWebhookUrl: notificationSlackWebhookUrl.trim() || null,
        webhookUrl: notificationWebhookUrl.trim() || null
      });
    } catch (error) {
      errorText = error instanceof Error ? error.message : ui.failedSave;
    } finally {
      savingNotifications = false;
    }
  }

  async function bootstrap() {
    loading = true;
    errorText = "";

    try {
      const [nextFile, nextCatalog] = await Promise.all([api.getEditableFile(configFilePath), api.getCatalog()]);
      configFile = nextFile;
      editorValue = nextFile.content;
      catalog = nextCatalog;
    } catch (error) {
      errorText = error instanceof Error ? error.message : ui.failedLoad;
    } finally {
      loading = false;
      reloading = false;
    }
  }

  async function reloadConfigFile() {
    reloading = true;
    await bootstrap();
  }

  async function saveConfigFile() {
    if (!configFile || !dirty) {
      return;
    }

    saving = true;
    errorText = "";

    try {
      configFile = await api.saveEditableFile(configFile.path, editorValue);
      editorValue = configFile.content;
      await onConfigSaved?.();
    } catch (error) {
      errorText = error instanceof Error ? error.message : ui.failedSave;
    } finally {
      saving = false;
    }
  }
</script>

<section class="settings-shell surface">
  <div class="settings-shell__header">
    <div>
      <p class="eyebrow">{ui.settings}</p>
      <h2>{ui.workspace}</h2>
    </div>
    <div class="settings-shell__actions">
      <button class="ghost-button" disabled={reloading || loading} onclick={() => void reloadConfigFile()} type="button">
        <RefreshCw size={14} class={reloading ? "animate-spin" : ""} />
        <span>{ui.reload}</span>
      </button>
      <button class="solid-button" disabled={!dirty || saving || !configFile} onclick={() => void saveConfigFile()} type="button">
        <Save size={14} />
        <span>{saving ? ui.saving : ui.saveConfig}</span>
      </button>
    </div>
  </div>

  {#if errorText}
    <div class="error-banner small">{errorText}</div>
  {/if}

  {#if loading}
    <div class="placeholder-card">{ui.loadingWorkspace}</div>
  {:else if configFile}
    <div class="settings-grid">
      <section class="panel">
        <div class="panel__header">
          <div>
            <h3>{ui.configTitle}</h3>
            <span>{configFile.path}</span>
          </div>
          {#if dirty}
            <span class="meta-pill subtle">{ui.unsaved}</span>
          {/if}
        </div>
        <div class="settings-meta">
          <div class="meta-card">
            <span>CODEX_HOME</span>
            <strong>{codexHome}</strong>
          </div>
          <div class="meta-card">
            <span>{ui.editableFile}</span>
            <strong>{configFile.displayName}</strong>
          </div>
        </div>
        <MonacoTextEditor bind:value={editorValue} height={460} path={configFile.path} />
      </section>

      <section class="settings-column">
        <section class="panel">
          <div class="panel__header">
            <div class="panel-title">
              <Settings2 size={16} />
              <h3>{ui.notificationCenter}</h3>
            </div>
            {#if notificationDirty}
              <span class="meta-pill subtle">{ui.unsaved}</span>
            {/if}
          </div>
          <div class="catalog-list">
            <div class="settings-meta">
              <label class="field-block">
                <span>{ui.slackWebhookUrl}</span>
                <input bind:value={notificationSlackWebhookUrl} class="field-input" placeholder="https://hooks.slack.com/services/..." type="url" />
              </label>
              <label class="field-block">
                <span>{ui.genericWebhookUrl}</span>
                <input bind:value={notificationWebhookUrl} class="field-input" placeholder="https://example.com/codex-webhook" type="url" />
              </label>
            </div>
            <div class="catalog-list">
              <p class="field-note">{ui.notificationEvents}</p>
              {#each ["sessionCompleted", "sessionAttention", "queueDispatchFailed", "shutdownScheduled"] as eventType (eventType)}
                <label class="checkbox-card checkbox-card--compact">
                  <input
                    class="checkbox-input"
                    checked={notificationEventTypes.includes(eventType as NotificationEventType)}
                    onchange={(event) => toggleNotificationEvent(eventType as NotificationEventType, (event.currentTarget as HTMLInputElement).checked)}
                    type="checkbox"
                  />
                  <span aria-hidden="true" class="checkbox-control"></span>
                  <span class="checkbox-copy">
                    <span class="checkbox-title">{notificationEventLabel(eventType as NotificationEventType)}</span>
                  </span>
                </label>
              {/each}
            </div>
            <div class="settings-shell__actions">
              <button class="solid-button" disabled={!notificationDirty || savingNotifications} onclick={() => void saveNotifications()} type="button">
                <Save size={14} />
                <span>{savingNotifications ? ui.saving : ui.saveNotificationSettings}</span>
              </button>
            </div>
          </div>
        </section>

        <section class="panel">
          <div class="panel__header">
            <div class="panel-title">
              <Settings2 size={16} />
              <h3>{ui.installedPlugins}</h3>
            </div>
            <span>{catalog?.plugins.length ?? 0}</span>
          </div>
          <div class="catalog-list">
            {#if (catalog?.plugins.length ?? 0) === 0}
              <p class="field-note">{ui.noPlugins}</p>
            {:else}
              {#each catalog?.plugins ?? [] as plugin (plugin.path)}
                <article class="catalog-card">
                  <div class="catalog-card__title">
                    <Plug size={14} />
                    <strong>{plugin.displayName}</strong>
                  </div>
                  <p>{plugin.description || ui.noDescription}</p>
                  <small>{plugin.name}{plugin.version ? ` · ${plugin.version}` : ""}{plugin.developerName ? ` · ${plugin.developerName}` : ""}</small>
                  {#if plugin.skills.length > 0}
                    <div class="tag-row">
                      {#each plugin.skills as skillName (skillName)}
                        <span class="meta-pill subtle">{skillName}</span>
                      {/each}
                    </div>
                  {/if}
                </article>
              {/each}
            {/if}
          </div>
        </section>

        <section class="panel">
          <div class="panel__header">
            <div class="panel-title">
              <Sparkles size={16} />
              <h3>{ui.installedSkills}</h3>
            </div>
            <span>{catalog?.skills.length ?? 0}</span>
          </div>
          <div class="catalog-list">
            {#if (catalog?.skills.length ?? 0) === 0}
              <p class="field-note">{ui.noSkills}</p>
            {:else}
              {#each catalog?.skills ?? [] as skill (skill.path)}
                <article class="catalog-card">
                  <div class="catalog-card__title">
                    <Sparkles size={14} />
                    <strong>{skill.name}</strong>
                  </div>
                  <p>{skill.description || ui.noDescription}</p>
                  <small>{skill.source}{skill.pluginName ? ` · ${skill.pluginName}` : ""}</small>
                </article>
              {/each}
            {/if}
          </div>
        </section>
      </section>
    </div>
  {/if}
</section>

<style>
  .settings-shell {
    display: grid;
    gap: 1.25rem;
    width: 100%;
    min-height: 0;
  }

  .settings-shell__header,
  .settings-shell__actions,
  .panel-title,
  .settings-meta,
  .tag-row {
    display: flex;
    gap: 0.75rem;
    align-items: center;
  }

  .settings-shell__header,
  .panel__header {
    justify-content: space-between;
  }

  .settings-shell__actions,
  .tag-row {
    flex-wrap: wrap;
  }

  .settings-shell__header {
    border: 1px solid rgba(83, 61, 42, 0.1);
    border-radius: 1.4rem;
    background:
      linear-gradient(180deg, rgba(255, 255, 255, 0.96), rgba(255, 250, 244, 0.92)),
      var(--panel);
    padding: 1.1rem 1.25rem;
    box-shadow: 0 20px 36px rgba(58, 39, 20, 0.08);
  }

  .settings-shell__header > div:first-child {
    min-width: 0;
  }

  .settings-shell__header h2,
  .panel__header h3 {
    margin: 0.15rem 0 0;
    color: var(--ink-strong);
    font: 600 1.2rem/1.1 var(--font-display);
  }

  .settings-shell__header p {
    margin: 0;
  }

  .settings-grid {
    display: grid;
    grid-template-columns: minmax(0, 1.45fr) minmax(22rem, 0.95fr);
    gap: 1.15rem;
    min-height: 0;
    align-items: start;
  }

  .settings-column,
  .catalog-list {
    display: grid;
    gap: 1rem;
  }

  .field-block {
    display: grid;
    gap: 0.45rem;
    min-width: 0;
    flex: 1 1 100%;
  }

  .field-block span {
    color: var(--muted);
    font-size: 0.72rem;
    font-weight: 700;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  .field-input {
    width: 100%;
    border: 1px solid rgba(148, 163, 184, 0.28);
    border-radius: 0.95rem;
    background: rgba(255, 255, 255, 0.92);
    color: var(--ink-strong);
    padding: 0.75rem 0.9rem;
    font: inherit;
    outline: none;
    transition: border-color 160ms ease, box-shadow 160ms ease, background-color 160ms ease;
  }

  .field-input:focus {
    border-color: rgba(245, 158, 11, 0.65);
    box-shadow: 0 0 0 4px rgba(245, 158, 11, 0.12);
    background: rgba(255, 255, 255, 1);
  }

  .panel {
    display: grid;
    gap: 0.9rem;
    border: 1px solid rgba(83, 61, 42, 0.1);
    border-radius: 1.35rem;
    background: rgba(255, 255, 255, 0.88);
    padding: 1.05rem;
    box-shadow: 0 16px 30px rgba(58, 39, 20, 0.07);
  }

  .panel__header {
    display: flex;
    gap: 0.75rem;
    align-items: center;
    min-width: 0;
  }

  .panel__header span {
    color: var(--muted);
    font-size: 0.8rem;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .settings-meta {
    flex-wrap: wrap;
  }

  .meta-card {
    display: grid;
    gap: 0.35rem;
    border-radius: 1rem;
    background: rgba(249, 245, 239, 0.75);
    padding: 0.85rem 1rem;
  }

  .meta-card span {
    color: var(--muted);
    font-size: 0.72rem;
    text-transform: uppercase;
    letter-spacing: 0.08em;
  }

  .meta-card strong {
    color: var(--ink-strong);
    font-size: 0.92rem;
    word-break: break-all;
  }

  .catalog-card {
    display: grid;
    gap: 0.55rem;
    border-radius: 1rem;
    background: rgba(249, 245, 239, 0.75);
    padding: 0.9rem 1rem;
  }

  .catalog-card__title {
    display: flex;
    gap: 0.55rem;
    align-items: center;
    color: var(--ink-strong);
  }

  .catalog-card p,
  .catalog-card small {
    margin: 0;
  }

  .catalog-card p {
    color: var(--ink);
    font-size: 0.9rem;
    line-height: 1.45;
  }

  .catalog-card small {
    color: var(--muted);
  }

  @media (max-width: 1120px) {
    .settings-grid {
      grid-template-columns: 1fr;
    }

    .settings-column {
      grid-template-columns: repeat(2, minmax(0, 1fr));
      align-items: start;
    }
  }

  @media (max-width: 720px) {
    .settings-shell,
    .panel {
      padding: 0.85rem;
    }

    .settings-shell__header {
      flex-direction: column;
      align-items: stretch;
    }

    .settings-column {
      grid-template-columns: 1fr;
    }
  }
</style>
