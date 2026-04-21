<script lang="ts">
  import { onMount } from "svelte";
  import { BellRing, Clock3, History, Palette, Pencil, Play, Plug, Power, RefreshCw, RotateCcw, Save, Settings2, Sparkles, Trash2, Wand2 } from "lucide-svelte";
  import { fade } from "svelte/transition";

  import { api } from "$lib/api";
  import MonacoTextEditor from "$lib/components/MonacoTextEditor.svelte";
  import { localeSignal } from "$lib/i18n";
  import { m } from "$lib/paraglide/messages.js";
  import { getLocale } from "$lib/paraglide/runtime.js";
  import type { ResolvedTheme, ThemeMode } from "$lib/theme";
  import {
    cloneThemeSettings,
    DEFAULT_THEME_SETTINGS,
    deriveThemeRuntimeVariables,
    THEME_SURFACES,
    THEME_TOKEN_KEYS,
    type ThemeSettings,
    type ThemeSurface,
    type ThemeTokenKey
  } from "$lib/theme-customization";
  import type {
    AutomationDefinition,
    AutomationRun,
    AuditLogEntry,
    AutostartProvider,
    AppConfigPayload,
    CatalogPayload,
    EditableFilePayload,
    NotificationEventType,
    NotificationSettings,
    PromptPreset,
    SelectedSkill,
    UserRole
  } from "$lib/types";

  type SettingsTabId =
    | "config"
    | "startup"
    | "audit"
    | "theme"
    | "notifications"
    | "presets"
    | "automations"
    | "plugins"
    | "skills";

  let {
    codexHome,
    configFilePath,
    autostart = null,
    notificationSettings = null,
    promptPresets = [],
    automations = [],
    automationRuns = [],
    themeSettings = null,
    themeMode = "system",
    resolvedTheme = "light",
    webRole = "admin",
    readOnly = false,
    onAutostartSaved = null,
    onSaveThemeSettings = null,
    onNotificationSettingsSaved = null,
    onSavePromptPreset = null,
    onDeletePromptPreset = null,
    onSaveAutomation = null,
    onDeleteAutomation = null,
    onRunAutomation = null,
    onOpenSession = null,
    onConfigSaved = null
  }: {
    codexHome: string;
    configFilePath: string;
    autostart?: AppConfigPayload["autostart"] | null;
    notificationSettings?: NotificationSettings | null;
    promptPresets?: PromptPreset[];
    automations?: AutomationDefinition[];
    automationRuns?: AutomationRun[];
    themeSettings?: ThemeSettings | null;
    themeMode?: ThemeMode;
    resolvedTheme?: ResolvedTheme;
    webRole?: UserRole | null;
    readOnly?: boolean;
    onAutostartSaved?: ((enabled: boolean) => void | Promise<void>) | null;
    onSaveThemeSettings?: ((settings: ThemeSettings) => void | Promise<void>) | null;
    onNotificationSettingsSaved?: ((settings: Partial<NotificationSettings>) => void | Promise<void>) | null;
    onSavePromptPreset?: ((preset: PromptPreset) => void | Promise<void>) | null;
    onDeletePromptPreset?: ((presetId: string) => void | Promise<void>) | null;
    onSaveAutomation?: ((automation: AutomationDefinition) => void | Promise<void>) | null;
    onDeleteAutomation?: ((automationId: string) => void | Promise<void>) | null;
    onRunAutomation?: ((automationId: string) => void | Promise<void>) | null;
    onOpenSession?: ((sessionId: string) => void | Promise<void>) | null;
    onConfigSaved?: (() => void | Promise<void>) | null;
  } = $props();

  let configFile = $state<EditableFilePayload | null>(null);
  let catalog = $state<CatalogPayload | null>(null);
  let editorValue = $state("");
  let errorText = $state("");
  let notificationSlackWebhookUrl = $state("");
  let notificationWebhookUrl = $state("");
  let notificationEventTypes = $state<NotificationEventType[]>([]);
  let presetId = $state<string | null>(null);
  let presetName = $state("");
  let presetPrompt = $state("");
  let automationId = $state<string | null>(null);
  let automationName = $state("");
  let automationPrompt = $state("");
  let automationEnabled = $state(true);
  let automationScheduleMode = $state<AutomationDefinition["scheduleMode"]>("manual");
  let automationIntervalMinutes = $state("60");
  let automationTarget = $state<AutomationDefinition["target"]>("local");
  let automationRepoPath = $state("");
  let automationCwd = $state("");
  let automationModel = $state("");
  let automationEffort = $state("");
  let automationSpeed = $state("");
  let automationMode = $state("");
  let automationSkills = $state<SelectedSkill[]>([]);
  let auditEntries = $state<AuditLogEntry[]>([]);
  let themeDraft = $state<ThemeSettings>(cloneThemeSettings(DEFAULT_THEME_SETTINGS));
  let themeBaseFingerprint = $state(JSON.stringify(cloneThemeSettings(DEFAULT_THEME_SETTINGS)));
  let loading = $state(true);
  let saving = $state(false);
  let reloading = $state(false);
  let savingNotifications = $state(false);
  let savingAutostart = $state(false);
  let savingPreset = $state(false);
  let savingTheme = $state(false);
  let auditLoadedForAdmin = $state(false);
  let activeTab = $state<SettingsTabId>("config");
  const dirty = $derived(Boolean(configFile && editorValue !== configFile.content));
  const themeDirty = $derived(themeBaseFingerprint !== JSON.stringify(themeDraft));
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
  const selectedPromptPreset = $derived.by(() => promptPresets.find((preset) => preset.id === presetId) ?? null);
  const promptPresetDirty = $derived.by(() => {
    if (!presetName.trim() && !presetPrompt.trim()) {
      return false;
    }
    if (!selectedPromptPreset) {
      return true;
    }
    return presetName.trim() !== selectedPromptPreset.name || presetPrompt !== selectedPromptPreset.prompt;
  });
  const selectedAutomation = $derived.by(() => automations.find((automation) => automation.id === automationId) ?? null);
  const automationDirty = $derived.by(() => {
    if (!automationName.trim() && !automationPrompt.trim()) {
      return false;
    }

    const interval = automationScheduleMode === "interval" ? String(Math.max(1, Math.round(Number(automationIntervalMinutes) || 0))) : "";
    if (!selectedAutomation) {
      return true;
    }

    return (
      automationName.trim() !== selectedAutomation.name ||
      automationPrompt !== selectedAutomation.prompt ||
      automationEnabled !== selectedAutomation.enabled ||
      automationScheduleMode !== selectedAutomation.scheduleMode ||
      interval !== String(selectedAutomation.intervalMinutes ?? "") ||
      automationTarget !== selectedAutomation.target ||
      automationRepoPath.trim() !== (selectedAutomation.repoPath ?? "") ||
      automationCwd.trim() !== (selectedAutomation.cwd ?? "") ||
      automationModel.trim() !== (selectedAutomation.model ?? "") ||
      automationEffort.trim() !== (selectedAutomation.effort ?? "") ||
      automationSpeed.trim() !== (selectedAutomation.speed ?? "") ||
      automationMode.trim() !== (selectedAutomation.mode ?? "") ||
      JSON.stringify(automationSkills.map((skill) => skill.path).sort()) !==
        JSON.stringify((selectedAutomation.skills ?? []).map((skill) => skill.path).sort())
    );
  });
  const ui = $derived.by(() => {
    const _locale = $localeSignal;

    return {
      settings: m.settings(),
      account: m.account(),
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
      startup: m.startup(),
      autostartTitle: m.autostart_title(),
      autostartDescription: m.autostart_description(),
      autostartEnable: m.autostart_enable(),
      autostartUnavailable: m.autostart_unavailable(),
      autostartMethod: m.autostart_method(),
      autostartLocation: m.autostart_location(),
      autostartMethodWindows: m.autostart_method_windows(),
      autostartMethodMacos: m.autostart_method_macos(),
      autostartMethodLinuxSystemd: m.autostart_method_linux_systemd(),
      autostartMethodLinuxXdg: m.autostart_method_linux_xdg(),
      notificationCenter: m.notification_center(),
      notificationEvents: m.notification_events(),
      slackWebhookUrl: m.slack_webhook_url(),
      genericWebhookUrl: m.generic_webhook_url(),
      saveNotificationSettings: m.save_notification_settings(),
      notificationSessionCompleted: m.notification_session_completed(),
      notificationInputRequired: m.notification_input_required(),
      notificationQueueFailed: m.notification_queue_failed(),
      notificationShutdownScheduled: m.notification_shutdown_scheduled(),
      auditLog: m.audit_log(),
      auditLogEmpty: m.audit_log_empty(),
      readOnlyMode: m.read_only_mode(),
      roleAdmin: m.role_admin(),
      roleViewer: m.role_viewer(),
      promptPresets: m.prompt_presets(),
      automations: m.automations(),
      automationName: m.automation_name(),
      automationPrompt: m.automation_prompt(),
      automationTarget: m.automation_target(),
      automationSchedule: m.automation_schedule(),
      automationEnabled: m.automation_enabled(),
      automationIntervalMinutes: m.automation_interval_minutes(),
      automationRepoPath: m.automation_repo_path(),
      automationWorkingDirectory: m.automation_working_directory(),
      automationModelOverride: m.automation_model_override(),
      automationEffortOverride: m.automation_effort_override(),
      automationSpeedOverride: m.automation_speed_override(),
      automationModeOverride: m.automation_mode_override(),
      automationManual: m.automation_manual(),
      automationInterval: m.automation_interval(),
      automationLocalWorkspace: m.automation_local_workspace(),
      automationManagedWorktree: m.automation_managed_worktree(),
      saveAutomation: m.save_automation(),
      runAutomation: m.run_automation(),
      noAutomations: m.no_automations(),
      newAutomation: m.new_automation(),
      recentAutomationRuns: m.recent_automation_runs(),
      noAutomationRuns: m.no_automation_runs(),
      presetName: m.preset_name(),
      presetPrompt: m.preset_prompt(),
      newPreset: m.new_preset(),
      savePreset: m.save_preset(),
      noPromptPresets: m.no_prompt_presets(),
      installedPlugins: m.installed_plugins(),
      noPlugins: m.no_installed_plugins(),
      noDescription: m.no_description(),
      installedSkills: m.installed_skills(),
      noSkills: m.no_local_skills(),
      openThread: m.open_thread(),
      themeEditor: m.theme_editor(),
      themeEditorDescription: m.theme_editor_description(),
      saveTheme: m.save_theme(),
      resetTheme: m.reset_theme(),
      themeSaved: m.theme_saved(),
      lightPalette: m.light_palette(),
      darkPalette: m.dark_palette(),
      currentThemeMode: m.current_theme_mode(),
      currentResolvedTheme: m.current_resolved_theme(),
      themePreview: m.theme_preview(),
      themeTokenBackground: m.theme_token_background(),
      themeTokenSidebar: m.theme_token_sidebar(),
      themeTokenAccentSurface: m.theme_token_accent_surface(),
      themeTokenStrongPanel: m.theme_token_strong_panel(),
      themeTokenSoftPanel: m.theme_token_soft_panel(),
      themeTokenPrimaryText: m.theme_token_primary_text(),
      themeTokenSecondaryText: m.theme_token_secondary_text(),
      themeTokenMutedText: m.theme_token_muted_text(),
      themeTokenAccent: m.theme_token_accent(),
      themeTokenBorder: m.theme_token_border(),
      remove: m.remove()
    };
  });

  const settingsTabs = $derived.by(() => {
    const tabs: Array<{
      id: SettingsTabId;
      label: string;
      icon: typeof Settings2;
      meta: string | null;
      alert: boolean;
    }> = [
      {
        id: "config",
        label: ui.configTitle,
        icon: Settings2,
        meta: dirty ? ui.unsaved : null,
        alert: dirty
      },
      {
        id: "startup",
        label: ui.autostartTitle,
        icon: Power,
        meta: null,
        alert: false
      },
      {
        id: "theme",
        label: ui.themeEditor,
        icon: Palette,
        meta: themeDirty ? ui.unsaved : null,
        alert: themeDirty
      },
      {
        id: "notifications",
        label: ui.notificationCenter,
        icon: BellRing,
        meta: notificationDirty ? ui.unsaved : notificationEventTypes.length > 0 ? String(notificationEventTypes.length) : null,
        alert: notificationDirty
      },
      {
        id: "presets",
        label: ui.promptPresets,
        icon: Pencil,
        meta: promptPresets.length > 0 ? String(promptPresets.length) : null,
        alert: promptPresetDirty
      },
      {
        id: "automations",
        label: ui.automations,
        icon: Wand2,
        meta: automations.length > 0 ? String(automations.length) : null,
        alert: automationDirty
      },
      {
        id: "plugins",
        label: ui.installedPlugins,
        icon: Plug,
        meta: (catalog?.plugins.length ?? 0) > 0 ? String(catalog?.plugins.length ?? 0) : null,
        alert: false
      },
      {
        id: "skills",
        label: ui.installedSkills,
        icon: Sparkles,
        meta: (catalog?.skills.length ?? 0) > 0 ? String(catalog?.skills.length ?? 0) : null,
        alert: false
      }
    ];

    if (webRole === "admin") {
      tabs.splice(2, 0, {
        id: "audit",
        label: ui.auditLog,
        icon: History,
        meta: auditEntries.length > 0 ? String(auditEntries.length) : null,
        alert: false
      });
    }

    return tabs;
  });

  const activeSettingsTab = $derived.by(() => settingsTabs.find((tab) => tab.id === activeTab) ?? settingsTabs[0]);

  onMount(() => {
    void bootstrap();
  });

  $effect(() => {
    if (!settingsTabs.some((tab) => tab.id === activeTab)) {
      activeTab = settingsTabs[0]?.id ?? "config";
    }
  });

  $effect(() => {
    const nextTheme = cloneThemeSettings(themeSettings ?? DEFAULT_THEME_SETTINGS);
    const nextFingerprint = JSON.stringify(nextTheme);
    if (nextFingerprint === themeBaseFingerprint) {
      return;
    }
    themeBaseFingerprint = nextFingerprint;
    themeDraft = nextTheme;
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

  $effect(() => {
    if (webRole !== "admin") {
      auditLoadedForAdmin = false;
      return;
    }

    if (loading || reloading || auditLoadedForAdmin) {
      return;
    }

    let cancelled = false;

    void api
      .getAuditLog(120)
      .then((nextAudit) => {
        if (cancelled) {
          return;
        }

        auditEntries = nextAudit.entries;
        auditLoadedForAdmin = true;
      })
      .catch((error) => {
        if (cancelled) {
          return;
        }

        errorText = error instanceof Error ? error.message : ui.failedLoad;
      });

    return () => {
      cancelled = true;
    };
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

  function autostartProviderLabel(provider: AutostartProvider | null) {
    if (provider === "windows-startup") {
      return ui.autostartMethodWindows;
    }
    if (provider === "macos-launch-agent") {
      return ui.autostartMethodMacos;
    }
    if (provider === "linux-systemd-user") {
      return ui.autostartMethodLinuxSystemd;
    }
    if (provider === "linux-xdg-autostart") {
      return ui.autostartMethodLinuxXdg;
    }
    return ui.autostartUnavailable;
  }

  async function updateAutostart(enabled: boolean) {
    if (readOnly || !autostart?.available) {
      return;
    }

    savingAutostart = true;
    errorText = "";

    try {
      await onAutostartSaved?.(enabled);
    } catch (error) {
      errorText = error instanceof Error ? error.message : ui.failedSave;
    } finally {
      savingAutostart = false;
    }
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
      const [nextFile, nextCatalog, nextAudit] = await Promise.all([
        api.getEditableFile(configFilePath),
        api.getCatalog(),
        webRole === "admin" ? api.getAuditLog(120) : Promise.resolve({ entries: [] as AuditLogEntry[] })
      ]);
      configFile = nextFile;
      editorValue = nextFile.content;
      catalog = nextCatalog;
      auditEntries = nextAudit.entries;
      auditLoadedForAdmin = webRole === "admin";
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
    if (!configFile || !dirty || readOnly) {
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

  function getThemeSurfaceLabel(surface: ThemeSurface) {
    return surface === "dark" ? ui.darkPalette : ui.lightPalette;
  }

  function getThemeModeLabel(mode: ThemeMode | ResolvedTheme) {
    if (mode === "dark") {
      return m.dark();
    }
    if (mode === "light") {
      return m.light();
    }
    return m.system();
  }

  function getThemeTokenLabel(token: ThemeTokenKey) {
    if (token === "bg") {
      return ui.themeTokenBackground;
    }
    if (token === "bgSidebar") {
      return ui.themeTokenSidebar;
    }
    if (token === "bgAccent") {
      return ui.themeTokenAccentSurface;
    }
    if (token === "panelStrong") {
      return ui.themeTokenStrongPanel;
    }
    if (token === "panelSoft") {
      return ui.themeTokenSoftPanel;
    }
    if (token === "inkStrong") {
      return ui.themeTokenPrimaryText;
    }
    if (token === "ink") {
      return ui.themeTokenSecondaryText;
    }
    if (token === "muted") {
      return ui.themeTokenMutedText;
    }
    if (token === "accent") {
      return ui.themeTokenAccent;
    }
    return ui.themeTokenBorder;
  }

  function setThemeColor(surface: ThemeSurface, token: ThemeTokenKey, value: string) {
    themeDraft = {
      ...themeDraft,
      [surface]: {
        ...themeDraft[surface],
        [token]: value
      }
    };
  }

  function getThemePreviewStyle(surface: ThemeSurface) {
    return Object.entries(deriveThemeRuntimeVariables(themeDraft, surface))
      .map(([name, value]) => `${name}:${value}`)
      .join(";");
  }

  function resetThemeDraft() {
    themeDraft = cloneThemeSettings(DEFAULT_THEME_SETTINGS);
  }

  async function saveTheme() {
    if (readOnly || !themeDirty) {
      return;
    }

    savingTheme = true;
    errorText = "";

    try {
      const nextTheme = cloneThemeSettings(themeDraft);
      await onSaveThemeSettings?.(nextTheme);
    } catch (error) {
      errorText = error instanceof Error ? error.message : ui.failedSave;
    } finally {
      savingTheme = false;
    }
  }

  function startNewPreset() {
    presetId = null;
    presetName = "";
    presetPrompt = "";
  }

  function startNewAutomation() {
    automationId = null;
    automationName = "";
    automationPrompt = "";
    automationEnabled = true;
    automationScheduleMode = "manual";
    automationIntervalMinutes = "60";
    automationTarget = "local";
    automationRepoPath = "";
    automationCwd = "";
    automationModel = "";
    automationEffort = "";
    automationSpeed = "";
    automationMode = "";
    automationSkills = [];
  }

  function editAutomation(automation: AutomationDefinition) {
    automationId = automation.id;
    automationName = automation.name;
    automationPrompt = automation.prompt;
    automationEnabled = automation.enabled;
    automationScheduleMode = automation.scheduleMode;
    automationIntervalMinutes = String(automation.intervalMinutes ?? 60);
    automationTarget = automation.target;
    automationRepoPath = automation.repoPath ?? "";
    automationCwd = automation.cwd ?? "";
    automationModel = automation.model ?? "";
    automationEffort = automation.effort ?? "";
    automationSpeed = automation.speed ?? "";
    automationMode = automation.mode ?? "";
    automationSkills = [...(automation.skills ?? [])];
  }

  function normalizeSelectedSkills(skills: SelectedSkill[]) {
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

  function toggleAutomationSkill(skill: CatalogPayload["skills"][number]) {
    const exists = automationSkills.some((entry) => entry.path === skill.path);
    automationSkills = exists
      ? automationSkills.filter((entry) => entry.path !== skill.path)
      : normalizeSelectedSkills([
          ...automationSkills,
          {
            id: skill.id,
            name: skill.name,
            path: skill.path
          }
        ]);
  }

  function editPromptPreset(preset: PromptPreset) {
    presetId = preset.id;
    presetName = preset.name;
    presetPrompt = preset.prompt;
  }

  async function savePromptPreset() {
    if (!presetName.trim() || !presetPrompt.trim() || readOnly) {
      return;
    }

    savingPreset = true;
    errorText = "";
    try {
      await onSavePromptPreset?.({
        id: presetId ?? crypto.randomUUID(),
        name: presetName.trim(),
        prompt: presetPrompt,
        createdAt: selectedPromptPreset?.createdAt ?? Date.now(),
        updatedAt: selectedPromptPreset?.updatedAt ?? Date.now()
      });
      startNewPreset();
    } catch (error) {
      errorText = error instanceof Error ? error.message : ui.failedSave;
    } finally {
      savingPreset = false;
    }
  }

  async function removePromptPreset() {
    if (!presetId || readOnly) {
      return;
    }

    savingPreset = true;
    errorText = "";
    try {
      await onDeletePromptPreset?.(presetId);
      startNewPreset();
    } catch (error) {
      errorText = error instanceof Error ? error.message : ui.failedSave;
    } finally {
      savingPreset = false;
    }
  }

  async function saveAutomation() {
    if (!automationName.trim() || !automationPrompt.trim() || readOnly) {
      return;
    }

    savingPreset = true;
    errorText = "";
    try {
      await onSaveAutomation?.({
        id: automationId ?? crypto.randomUUID(),
        name: automationName.trim(),
        prompt: automationPrompt,
        enabled: automationEnabled,
        scheduleMode: automationScheduleMode,
        intervalMinutes: automationScheduleMode === "interval" ? Math.max(1, Math.round(Number(automationIntervalMinutes) || 0)) : null,
        target: automationTarget,
        repoPath: automationRepoPath.trim() || null,
        cwd: automationCwd.trim() || null,
        skills: normalizeSelectedSkills(automationSkills),
        model: automationModel.trim() || null,
        effort: (automationEffort.trim() || null) as AutomationDefinition["effort"],
        speed: (automationSpeed.trim() || null) as AutomationDefinition["speed"],
        mode: (automationMode.trim() || null) as AutomationDefinition["mode"],
        createdAt: selectedAutomation?.createdAt ?? Date.now(),
        updatedAt: selectedAutomation?.updatedAt ?? Date.now(),
        lastRunAt: selectedAutomation?.lastRunAt ?? null,
        nextRunAt: selectedAutomation?.nextRunAt ?? null
      });
      startNewAutomation();
    } catch (error) {
      errorText = error instanceof Error ? error.message : ui.failedSave;
    } finally {
      savingPreset = false;
    }
  }

  async function removeAutomation() {
    if (!automationId || readOnly) {
      return;
    }

    savingPreset = true;
    errorText = "";
    try {
      await onDeleteAutomation?.(automationId);
      startNewAutomation();
    } catch (error) {
      errorText = error instanceof Error ? error.message : ui.failedSave;
    } finally {
      savingPreset = false;
    }
  }

  async function runAutomationNow() {
    if (!automationId || readOnly) {
      return;
    }

    savingPreset = true;
    errorText = "";
    try {
      await onRunAutomation?.(automationId);
    } catch (error) {
      errorText = error instanceof Error ? error.message : ui.failedSave;
    } finally {
      savingPreset = false;
    }
  }
</script>

<section class="settings-shell surface">
  <div class="settings-shell__header">
    <div>
      <p class="eyebrow">{ui.settings}</p>
      <h2>{activeSettingsTab?.label ?? ui.settings}</h2>
    </div>
    <div class="settings-shell__actions">
      <span class={`meta-pill ${readOnly ? "" : "subtle"}`}>
        {webRole === "viewer" ? ui.roleViewer : ui.roleAdmin}
        {#if readOnly}
          · {ui.readOnlyMode}
        {/if}
      </span>
      <button class="ghost-button" disabled={reloading || loading} onclick={() => void reloadConfigFile()} type="button">
        <RefreshCw size={14} class={reloading ? "animate-spin" : ""} />
        <span>{ui.reload}</span>
      </button>
      {#if activeTab === "config"}
        <button class="solid-button" disabled={readOnly || !dirty || saving || !configFile} onclick={() => void saveConfigFile()} type="button">
          <Save size={14} />
          <span>{saving ? ui.saving : ui.saveConfig}</span>
        </button>
      {/if}
    </div>
  </div>

  {#if errorText}
    <div class="error-banner small">{errorText}</div>
  {/if}

  {#if loading}
    <div class="placeholder-card">{ui.loadingWorkspace}</div>
  {:else if configFile}
    <div aria-label={ui.settings} class="settings-tabs" role="tablist">
      {#each settingsTabs as tab (tab.id)}
        {@const Icon = tab.icon}
        <button
          aria-controls={`settings-panel-${tab.id}`}
          aria-selected={activeTab === tab.id}
          class={`settings-tab ${activeTab === tab.id ? "settings-tab--active" : ""}`}
          id={`settings-tab-${tab.id}`}
          onclick={() => (activeTab = tab.id)}
          role="tab"
          type="button"
        >
          <span class="settings-tab__icon">
            <Icon size={14} />
          </span>
          <span class="settings-tab__label">{tab.label}</span>
          {#if tab.meta}
            <span class={`settings-tab__meta ${tab.alert ? "settings-tab__meta--alert" : ""}`}>{tab.meta}</span>
          {/if}
        </button>
      {/each}
    </div>

    {#if activeTab === "config"}
      <div
        aria-labelledby="settings-tab-config"
        class="settings-tab-panel"
        id="settings-panel-config"
        role="tabpanel"
        transition:fade={{ duration: 160 }}
      >
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
            <div class="meta-card">
              <span>{ui.account}</span>
              <strong>{webRole === "viewer" ? ui.roleViewer : ui.roleAdmin}</strong>
            </div>
          </div>
          {#if readOnly}
            <div class="field-note field-note--read-only">{ui.readOnlyMode}</div>
          {/if}
          <MonacoTextEditor bind:value={editorValue} height={460} path={configFile.path} readonly={readOnly} />
        </section>
      </div>
    {:else if activeTab === "startup"}
      <div
        aria-labelledby="settings-tab-startup"
        class="settings-tab-panel"
        id="settings-panel-startup"
        role="tabpanel"
        transition:fade={{ duration: 160 }}
      >
        <section class="panel">
          <div class="panel__header">
            <div class="panel-title">
              <Power size={16} />
              <h3>{ui.autostartTitle}</h3>
            </div>
            {#if savingAutostart}
              <span class="meta-pill subtle">{ui.saving}</span>
            {/if}
          </div>
          <p class="field-note">{ui.autostartDescription}</p>
          <div class="settings-meta">
            <div class="meta-card">
              <span>{ui.startup}</span>
              <strong>{autostart?.available ? autostartProviderLabel(autostart.provider) : ui.autostartUnavailable}</strong>
            </div>
            {#if autostart?.location}
              <div class="meta-card">
                <span>{ui.autostartLocation}</span>
                <strong>{autostart.location}</strong>
              </div>
            {/if}
          </div>
          <label class:checkbox-card--disabled={readOnly || !autostart?.available} class="checkbox-card checkbox-card--compact">
            <input
              class="checkbox-input"
              checked={Boolean(autostart?.enabled)}
              disabled={readOnly || !autostart?.available || savingAutostart}
              onchange={(event) => void updateAutostart((event.currentTarget as HTMLInputElement).checked)}
              type="checkbox"
            />
            <span aria-hidden="true" class="checkbox-control"></span>
            <span class="checkbox-copy">
              <span class="checkbox-title">{ui.autostartEnable}</span>
              <span>{autostart?.available ? `${ui.autostartMethod}: ${autostartProviderLabel(autostart.provider)}` : ui.autostartUnavailable}</span>
            </span>
          </label>
        </section>
      </div>
    {:else if activeTab === "audit"}
      <div
        aria-labelledby="settings-tab-audit"
        class="settings-tab-panel"
        id="settings-panel-audit"
        role="tabpanel"
        transition:fade={{ duration: 160 }}
      >
        <section class="panel">
          <div class="panel__header">
            <div class="panel-title">
              <History size={16} />
              <h3>{ui.auditLog}</h3>
            </div>
            <span>{auditEntries.length}</span>
          </div>
          <div class="catalog-list">
            {#if auditEntries.length === 0}
              <p class="field-note">{ui.auditLogEmpty}</p>
            {:else}
              <div class="audit-log-list">
                {#each auditEntries as entry (entry.id)}
                  <article class="audit-log-entry">
                    <div class="audit-log-entry__header">
                      <strong>{entry.method}</strong>
                      <span class={`meta-pill subtle ${entry.ok ? "audit-log-entry__result--ok" : "audit-log-entry__result--error"}`}>
                        {entry.ok ? "OK" : "ERR"}
                      </span>
                    </div>
                    <div class="audit-log-entry__meta">
                      <span>{new Intl.DateTimeFormat(getLocale() === "ko" ? "ko-KR" : "en-US", {
                        month: "short",
                        day: "numeric",
                        hour: "2-digit",
                        minute: "2-digit"
                      }).format(new Date(entry.at))}</span>
                      <span>{entry.role === "viewer" ? ui.roleViewer : entry.role === "admin" ? ui.roleAdmin : entry.role}</span>
                      {#if entry.target}
                        <span class="truncate">{entry.target}</span>
                      {/if}
                    </div>
                    {#if entry.error}
                      <p class="audit-log-entry__error">{entry.error}</p>
                    {/if}
                  </article>
                {/each}
              </div>
            {/if}
          </div>
        </section>
      </div>
    {:else if activeTab === "theme"}
      <div
        aria-labelledby="settings-tab-theme"
        class="settings-tab-panel"
        id="settings-panel-theme"
        role="tabpanel"
        transition:fade={{ duration: 160 }}
      >
        <section class="panel">
          <div class="panel__header">
            <div class="panel-title">
              <Palette size={16} />
              <h3>{ui.themeEditor}</h3>
            </div>
            {#if themeDirty}
              <span class="meta-pill subtle">{ui.unsaved}</span>
            {/if}
          </div>
          <p class="field-note">{ui.themeEditorDescription}</p>
          <div class="settings-meta">
            <div class="meta-card">
              <span>{ui.currentThemeMode}</span>
              <strong>{getThemeModeLabel(themeMode)}</strong>
            </div>
            <div class="meta-card">
              <span>{ui.currentResolvedTheme}</span>
              <strong>{getThemeModeLabel(resolvedTheme)}</strong>
            </div>
          </div>
          <div class="theme-preview-grid">
            {#each THEME_SURFACES as surface (surface)}
              <article class="theme-preview-card" style={getThemePreviewStyle(surface)}>
                <div class="theme-preview-card__header">
                  <div>
                    <h4>{getThemeSurfaceLabel(surface)}</h4>
                    <p>{ui.themePreview}</p>
                  </div>
                  <span class="theme-preview-card__accent"></span>
                </div>
                <div class="theme-preview-shell">
                  <aside class="theme-preview-sidebar">
                    <span></span>
                    <span></span>
                    <span></span>
                  </aside>
                  <div class="theme-preview-main">
                    <div class="theme-preview-panel theme-preview-panel--strong">
                      <div class="theme-preview-line theme-preview-line--strong"></div>
                      <div class="theme-preview-line"></div>
                    </div>
                    <div class="theme-preview-panel theme-preview-panel--soft">
                      <div class="theme-preview-badge"></div>
                      <div class="theme-preview-line theme-preview-line--strong"></div>
                      <div class="theme-preview-line"></div>
                    </div>
                  </div>
                </div>
              </article>
            {/each}
          </div>
          <div class="theme-editor-grid">
            {#each THEME_SURFACES as surface (surface)}
              <section class="theme-surface-card">
                <div class="panel__header">
                  <h4>{getThemeSurfaceLabel(surface)}</h4>
                  <span>{themeDraft[surface].accent}</span>
                </div>
                <div class="theme-token-grid">
                  {#each THEME_TOKEN_KEYS as token (token)}
                    <label class="theme-token-field">
                      <span>{getThemeTokenLabel(token)}</span>
                      <div class="theme-token-control">
                        <input type="color" value={themeDraft[surface][token]} onchange={(event) => setThemeColor(surface, token, (event.currentTarget as HTMLInputElement).value)} />
                        <code>{themeDraft[surface][token]}</code>
                      </div>
                    </label>
                  {/each}
                </div>
              </section>
            {/each}
          </div>
          <div class="settings-shell__actions">
            <button class="ghost-button" disabled={readOnly || savingTheme} onclick={resetThemeDraft} type="button">
              <RotateCcw size={14} />
              <span>{ui.resetTheme}</span>
            </button>
            <button class="solid-button" disabled={readOnly || !themeDirty || savingTheme} onclick={() => void saveTheme()} type="button">
              <Save size={14} />
              <span>{savingTheme ? ui.saving : ui.saveTheme}</span>
            </button>
          </div>
        </section>
      </div>
    {:else if activeTab === "notifications"}
      <div
        aria-labelledby="settings-tab-notifications"
        class="settings-tab-panel"
        id="settings-panel-notifications"
        role="tabpanel"
        transition:fade={{ duration: 160 }}
      >
        <section class="panel">
          <div class="panel__header">
            <div class="panel-title">
              <BellRing size={16} />
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
                <input bind:value={notificationSlackWebhookUrl} class="field-input" disabled={readOnly} placeholder="https://hooks.slack.com/services/..." type="url" />
              </label>
              <label class="field-block">
                <span>{ui.genericWebhookUrl}</span>
                <input bind:value={notificationWebhookUrl} class="field-input" disabled={readOnly} placeholder="https://example.com/codex-webhook" type="url" />
              </label>
            </div>
            <div class="catalog-list">
              <p class="field-note">{ui.notificationEvents}</p>
              {#each ["sessionCompleted", "sessionAttention", "queueDispatchFailed", "shutdownScheduled"] as eventType (eventType)}
                <label class="checkbox-card checkbox-card--compact">
                  <input
                    class="checkbox-input"
                    checked={notificationEventTypes.includes(eventType as NotificationEventType)}
                    disabled={readOnly}
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
              <button class="solid-button" disabled={readOnly || !notificationDirty || savingNotifications} onclick={() => void saveNotifications()} type="button">
                <Save size={14} />
                <span>{savingNotifications ? ui.saving : ui.saveNotificationSettings}</span>
              </button>
            </div>
          </div>
        </section>
      </div>
    {:else if activeTab === "presets"}
      <div
        aria-labelledby="settings-tab-presets"
        class="settings-tab-panel"
        id="settings-panel-presets"
        role="tabpanel"
        transition:fade={{ duration: 160 }}
      >
        <section class="panel">
          <div class="panel__header">
            <div class="panel-title">
              <Pencil size={16} />
              <h3>{ui.promptPresets}</h3>
            </div>
            <button class="ghost-button" disabled={readOnly} onclick={startNewPreset} type="button">
              <Pencil size={14} />
              <span>{ui.newPreset}</span>
            </button>
          </div>
          <div class="settings-grid settings-grid--nested">
            <div class="catalog-list">
              {#if promptPresets.length === 0}
                <p class="field-note">{ui.noPromptPresets}</p>
              {:else}
                {#each promptPresets as preset (preset.id)}
                  <button
                    class={`catalog-card catalog-card--button ${presetId === preset.id ? "catalog-card--active" : ""}`}
                    onclick={() => editPromptPreset(preset)}
                    type="button"
                  >
                    <div class="catalog-card__title">
                      <Pencil size={14} />
                      <strong>{preset.name}</strong>
                    </div>
                    <p>{preset.prompt.split(/\r?\n/u, 1)[0]?.trim() || preset.prompt.trim()}</p>
                  </button>
                {/each}
              {/if}
            </div>
            <div class="settings-meta settings-meta--stack">
              <label class="field-block">
                <span>{ui.presetName}</span>
                <input bind:value={presetName} class="field-input" disabled={readOnly} placeholder={ui.presetName} type="text" />
              </label>
              <label class="field-block">
                <span>{ui.presetPrompt}</span>
                <textarea bind:value={presetPrompt} class="field-input field-textarea" disabled={readOnly} placeholder={ui.presetPrompt} rows="8"></textarea>
              </label>
              <div class="settings-shell__actions">
                <button class="solid-button" disabled={readOnly || !promptPresetDirty || savingPreset || !presetName.trim() || !presetPrompt.trim()} onclick={() => void savePromptPreset()} type="button">
                  <Save size={14} />
                  <span>{savingPreset ? ui.saving : ui.savePreset}</span>
                </button>
                {#if presetId}
                  <button class="ghost-button" disabled={readOnly || savingPreset} onclick={() => void removePromptPreset()} type="button">
                    <Trash2 size={14} />
                    <span>{ui.remove}</span>
                  </button>
                {/if}
              </div>
            </div>
          </div>
        </section>
      </div>
    {:else if activeTab === "automations"}
      <div
        aria-labelledby="settings-tab-automations"
        class="settings-tab-panel"
        id="settings-panel-automations"
        role="tabpanel"
        transition:fade={{ duration: 160 }}
      >
        <section class="panel">
          <div class="panel__header">
            <div class="panel-title">
              <Wand2 size={16} />
              <h3>{ui.automations}</h3>
            </div>
            <button class="ghost-button" disabled={readOnly} onclick={startNewAutomation} type="button">
              <Wand2 size={14} />
              <span>{ui.newAutomation}</span>
            </button>
          </div>
          <div class="settings-grid settings-grid--nested">
            <div class="catalog-list">
              {#if automations.length === 0}
                <p class="field-note">{ui.noAutomations}</p>
              {:else}
                {#each automations as automation (automation.id)}
                  <button
                    class={`catalog-card catalog-card--button ${automationId === automation.id ? "catalog-card--active" : ""}`}
                    onclick={() => editAutomation(automation)}
                    type="button"
                  >
                    <div class="catalog-card__title">
                      <Wand2 size={14} />
                      <strong>{automation.name}</strong>
                    </div>
                    <p>{automation.prompt.split(/\r?\n/u, 1)[0]?.trim() || automation.prompt.trim()}</p>
                    <small>
                      {automation.target === "worktree" ? ui.automationManagedWorktree : ui.automationLocalWorkspace}
                      ·
                      {automation.scheduleMode === "interval" ? `${ui.automationInterval} · ${automation.intervalMinutes ?? 0}m` : ui.automationManual}
                      {automation.enabled ? "" : " · paused"}
                    </small>
                  </button>
                {/each}
              {/if}
            </div>
            <div class="settings-meta settings-meta--stack">
              <label class="field-block">
                <span>{ui.automationName}</span>
                <input bind:value={automationName} class="field-input" disabled={readOnly} placeholder={ui.automationName} type="text" />
              </label>
              <label class="field-block">
                <span>{ui.automationPrompt}</span>
                <textarea bind:value={automationPrompt} class="field-input field-textarea" disabled={readOnly} placeholder={ui.automationPrompt} rows="8"></textarea>
              </label>
              <div class="field-block">
                <span>{ui.installedSkills}</span>
                {#if automationSkills.length > 0}
                  <div class="mt-2 flex flex-wrap gap-1.5">
                    {#each automationSkills as skill (skill.path)}
                      <button
                        class="inline-flex items-center gap-1.5 rounded-full border border-violet-200 bg-violet-50 px-2.5 py-1 text-[10px] font-bold text-violet-700 transition-colors hover:bg-violet-100"
                        disabled={readOnly}
                        onclick={() => (automationSkills = automationSkills.filter((entry) => entry.path !== skill.path))}
                        type="button"
                      >
                        <span class="max-w-[12rem] truncate">{skill.name}</span>
                        <Trash2 size={11} />
                      </button>
                    {/each}
                  </div>
                {/if}
                <div class="catalog-list mt-3">
                  {#if (catalog?.skills.length ?? 0) === 0}
                    <p class="field-note">{ui.noSkills}</p>
                  {:else}
                    {#each catalog?.skills ?? [] as skill (skill.path)}
                      {@const selected = automationSkills.some((entry) => entry.path === skill.path)}
                      <button
                        class={`catalog-card catalog-card--button ${selected ? "catalog-card--active" : ""}`}
                        disabled={readOnly}
                        onclick={() => toggleAutomationSkill(skill)}
                        type="button"
                      >
                        <div class="catalog-card__title">
                          <Sparkles size={14} />
                          <strong>{skill.name}</strong>
                        </div>
                        <p>{skill.description || skill.path}</p>
                        <small>
                          {skill.source}
                          {skill.pluginName ? ` · ${skill.pluginName}` : ""}
                        </small>
                      </button>
                    {/each}
                  {/if}
                </div>
              </div>
              <div class="settings-meta">
                <label class="field-block">
                  <span>{ui.automationTarget}</span>
                  <select bind:value={automationTarget} class="field-input" disabled={readOnly}>
                    <option value="local">{ui.automationLocalWorkspace}</option>
                    <option value="worktree">{ui.automationManagedWorktree}</option>
                  </select>
                </label>
                <label class="field-block">
                  <span>{ui.automationSchedule}</span>
                  <select bind:value={automationScheduleMode} class="field-input" disabled={readOnly}>
                    <option value="manual">{ui.automationManual}</option>
                    <option value="interval">{ui.automationInterval}</option>
                  </select>
                </label>
              </div>
              {#if automationScheduleMode === "interval"}
                <label class="field-block">
                  <span>{ui.automationIntervalMinutes}</span>
                  <input bind:value={automationIntervalMinutes} class="field-input" disabled={readOnly} min="1" step="1" type="number" />
                </label>
              {/if}
              {#if automationTarget === "worktree"}
                <label class="field-block">
                  <span>{ui.automationRepoPath}</span>
                  <input bind:value={automationRepoPath} class="field-input" disabled={readOnly} placeholder="/path/to/repository" type="text" />
                </label>
              {/if}
              <label class="field-block">
                <span>{ui.automationWorkingDirectory}</span>
                <input bind:value={automationCwd} class="field-input" disabled={readOnly} placeholder={codexHome} type="text" />
              </label>
              <div class="settings-meta">
                <label class="field-block">
                  <span>{ui.automationModelOverride}</span>
                  <input bind:value={automationModel} class="field-input" disabled={readOnly} placeholder="gpt-5.4" type="text" />
                </label>
                <label class="field-block">
                  <span>{ui.automationEffortOverride}</span>
                  <input bind:value={automationEffort} class="field-input" disabled={readOnly} placeholder="medium" type="text" />
                </label>
              </div>
              <div class="settings-meta">
                <label class="field-block">
                  <span>{ui.automationSpeedOverride}</span>
                  <input bind:value={automationSpeed} class="field-input" disabled={readOnly} placeholder="fast" type="text" />
                </label>
                <label class="field-block">
                  <span>{ui.automationModeOverride}</span>
                  <input bind:value={automationMode} class="field-input" disabled={readOnly} placeholder="plan" type="text" />
                </label>
              </div>
              <label class="checkbox-card checkbox-card--compact">
                <input bind:checked={automationEnabled} class="checkbox-input" disabled={readOnly} type="checkbox" />
                <span aria-hidden="true" class="checkbox-control"></span>
                <span class="checkbox-copy">
                  <span class="checkbox-title">{ui.automationEnabled}</span>
                </span>
              </label>
              <div class="settings-shell__actions">
                <button
                  class="solid-button"
                  disabled={readOnly || !automationDirty || savingPreset || !automationName.trim() || !automationPrompt.trim()}
                  onclick={() => void saveAutomation()}
                  type="button"
                >
                  <Save size={14} />
                  <span>{savingPreset ? ui.saving : ui.saveAutomation}</span>
                </button>
                {#if automationId}
                  <button class="ghost-button" disabled={readOnly || savingPreset} onclick={() => void runAutomationNow()} type="button">
                    <Play size={14} />
                    <span>{ui.runAutomation}</span>
                  </button>
                  <button class="ghost-button" disabled={readOnly || savingPreset} onclick={() => void removeAutomation()} type="button">
                    <Trash2 size={14} />
                    <span>{ui.remove}</span>
                  </button>
                {/if}
              </div>
            </div>
          </div>
          <div class="catalog-list">
            <div class="panel__header">
              <div class="panel-title">
                <Clock3 size={16} />
                <h3>{ui.recentAutomationRuns}</h3>
              </div>
              <span>{automationRuns.length}</span>
            </div>
            {#if automationRuns.length === 0}
              <p class="field-note">{ui.noAutomationRuns}</p>
            {:else}
              <div class="audit-log-list">
                {#each automationRuns as run (run.id)}
                  <article class="audit-log-entry">
                    <div class="audit-log-entry__header">
                      <strong>{run.automationName}</strong>
                      <div class="flex items-center gap-2">
                        <span class={`meta-pill subtle ${run.status === "failed" ? "audit-log-entry__result--error" : "audit-log-entry__result--ok"}`}>
                          {run.status}
                        </span>
                        {#if run.sessionId}
                          <button class="ghost-button" onclick={() => void onOpenSession?.(run.sessionId!)} type="button">
                            <Play size={13} />
                            <span>{ui.openThread}</span>
                          </button>
                        {/if}
                      </div>
                    </div>
                    <div class="audit-log-entry__meta">
                      <span>{run.trigger}</span>
                      <span>{new Intl.DateTimeFormat(getLocale() === "ko" ? "ko-KR" : "en-US", {
                        month: "short",
                        day: "numeric",
                        hour: "2-digit",
                        minute: "2-digit"
                      }).format(new Date(run.startedAt))}</span>
                      {#if run.cwd}
                        <span class="truncate">{run.cwd}</span>
                      {/if}
                    </div>
                    {#if run.error}
                      <p class="audit-log-entry__error">{run.error}</p>
                    {/if}
                  </article>
                {/each}
              </div>
            {/if}
          </div>
        </section>
      </div>
    {:else if activeTab === "plugins"}
      <div
        aria-labelledby="settings-tab-plugins"
        class="settings-tab-panel"
        id="settings-panel-plugins"
        role="tabpanel"
        transition:fade={{ duration: 160 }}
      >
        <section class="panel">
          <div class="panel__header">
            <div class="panel-title">
              <Plug size={16} />
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
      </div>
    {:else if activeTab === "skills"}
      <div
        aria-labelledby="settings-tab-skills"
        class="settings-tab-panel"
        id="settings-panel-skills"
        role="tabpanel"
        transition:fade={{ duration: 160 }}
      >
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
      </div>
    {/if}
  {/if}
</section>

<style>
  .settings-shell {
    display: grid;
    gap: 1.25rem;
    width: 100%;
    min-height: 0;
  }

  .settings-tabs {
    display: flex;
    gap: 0.7rem;
    align-items: stretch;
    overflow-x: auto;
    padding: 0.2rem 0.1rem 0.35rem;
    scrollbar-width: thin;
  }

  .settings-tab {
    display: inline-flex;
    align-items: center;
    gap: 0.55rem;
    min-height: 2.9rem;
    border: 1px solid rgba(83, 61, 42, 0.1);
    border-radius: 1rem;
    background: rgba(255, 255, 255, 0.82);
    color: var(--ink);
    padding: 0.7rem 0.9rem;
    white-space: nowrap;
    transition:
      border-color 160ms ease,
      background-color 160ms ease,
      transform 160ms ease,
      box-shadow 160ms ease,
      color 160ms ease;
  }

  .settings-tab:hover {
    border-color: rgba(245, 158, 11, 0.22);
    background: rgba(255, 251, 235, 0.82);
    color: var(--ink-strong);
    transform: translateY(-1px);
  }

  .settings-tab--active {
    border-color: rgba(245, 158, 11, 0.34);
    background: linear-gradient(180deg, rgba(255, 251, 235, 0.96), rgba(255, 247, 237, 0.92));
    color: var(--ink-strong);
    box-shadow: 0 14px 26px rgba(58, 39, 20, 0.08);
  }

  .settings-tab__icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 1.8rem;
    height: 1.8rem;
    border-radius: 0.8rem;
    background: rgba(245, 158, 11, 0.1);
    color: #c2410c;
    flex: 0 0 auto;
  }

  .settings-tab__label {
    font-size: 0.9rem;
    font-weight: 700;
  }

  .settings-tab__meta {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 1.45rem;
    border-radius: 999px;
    background: rgba(148, 163, 184, 0.16);
    color: var(--muted);
    padding: 0.18rem 0.48rem;
    font-size: 0.7rem;
    font-weight: 700;
    letter-spacing: 0.03em;
  }

  .settings-tab__meta--alert {
    background: rgba(245, 158, 11, 0.14);
    color: #c2410c;
  }

  .settings-tab-panel {
    display: grid;
    gap: 1rem;
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

  .settings-grid--nested {
    grid-template-columns: minmax(0, 0.9fr) minmax(0, 1.1fr);
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

  .field-textarea {
    min-height: 10rem;
    resize: vertical;
  }

  .field-note--read-only {
    border: 1px solid rgba(245, 158, 11, 0.18);
    border-radius: 0.95rem;
    background: rgba(255, 247, 237, 0.88);
    color: #9a6700;
    padding: 0.8rem 0.95rem;
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

  .audit-log-list {
    display: grid;
    gap: 0.7rem;
    max-height: 28rem;
    overflow: auto;
    padding-right: 0.15rem;
  }

  .audit-log-entry {
    display: grid;
    gap: 0.45rem;
    border: 1px solid rgba(148, 163, 184, 0.18);
    border-radius: 1rem;
    background: rgba(248, 250, 252, 0.8);
    padding: 0.8rem 0.9rem;
  }

  .audit-log-entry__header,
  .audit-log-entry__meta {
    display: flex;
    align-items: center;
    gap: 0.55rem;
    min-width: 0;
  }

  .audit-log-entry__header {
    justify-content: space-between;
  }

  .audit-log-entry__header strong {
    min-width: 0;
    color: var(--ink-strong);
    font-size: 0.86rem;
  }

  .audit-log-entry__meta {
    flex-wrap: wrap;
    color: var(--muted);
    font-size: 0.72rem;
  }

  .audit-log-entry__result--ok {
    color: #047857;
  }

  .audit-log-entry__result--error {
    color: #b91c1c;
  }

  .audit-log-entry__error {
    margin: 0;
    color: #b91c1c;
    font-size: 0.76rem;
    line-height: 1.45;
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

  .settings-meta--stack {
    display: grid;
    align-content: start;
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

  .catalog-card--button {
    width: 100%;
    border: 1px solid transparent;
    text-align: left;
    transition: border-color 160ms ease, background-color 160ms ease, transform 160ms ease;
  }

  .catalog-card--button:hover {
    border-color: rgba(245, 158, 11, 0.2);
    background: rgba(255, 255, 255, 0.95);
    transform: translateY(-1px);
  }

  .catalog-card--active {
    border-color: rgba(245, 158, 11, 0.35);
    background: rgba(255, 251, 235, 0.9);
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

  .theme-preview-grid,
  .theme-editor-grid,
  .theme-token-grid {
    display: grid;
    gap: 0.9rem;
  }

  .theme-preview-grid,
  .theme-editor-grid {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }

  .theme-preview-card,
  .theme-surface-card {
    display: grid;
    gap: 0.9rem;
    border: 1px solid rgba(148, 163, 184, 0.18);
    border-radius: 1.1rem;
    background: rgba(248, 250, 252, 0.78);
    padding: 0.95rem;
  }

  .theme-preview-card__header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
  }

  .theme-preview-card__header h4,
  .theme-surface-card h4 {
    margin: 0;
    color: var(--ink-strong);
    font-size: 0.9rem;
  }

  .theme-preview-card__header p {
    margin: 0.15rem 0 0;
    color: var(--muted);
    font-size: 0.72rem;
  }

  .theme-preview-card__accent {
    width: 0.95rem;
    height: 0.95rem;
    border-radius: 999px;
    background: var(--accent);
    box-shadow: 0 0 0 4px color-mix(in srgb, var(--accent) 18%, transparent);
  }

  .theme-preview-shell {
    display: grid;
    grid-template-columns: 4.4rem minmax(0, 1fr);
    gap: 0.75rem;
    min-height: 11.5rem;
    border-radius: 1rem;
    border: 1px solid color-mix(in srgb, var(--line) 92%, transparent);
    background: var(--bg);
    padding: 0.8rem;
  }

  .theme-preview-sidebar {
    display: grid;
    align-content: start;
    gap: 0.55rem;
    border-radius: 0.9rem;
    background: var(--bg-sidebar);
    padding: 0.8rem 0.65rem;
  }

  .theme-preview-sidebar span,
  .theme-preview-line {
    display: block;
    border-radius: 999px;
    background: color-mix(in srgb, var(--ink) 16%, transparent);
  }

  .theme-preview-sidebar span {
    height: 0.5rem;
  }

  .theme-preview-main {
    display: grid;
    align-content: start;
    gap: 0.75rem;
  }

  .theme-preview-panel {
    display: grid;
    gap: 0.55rem;
    border-radius: 0.95rem;
    border: 1px solid color-mix(in srgb, var(--line) 92%, transparent);
    padding: 0.85rem;
  }

  .theme-preview-panel--strong {
    background: var(--panel-strong);
  }

  .theme-preview-panel--soft {
    background: var(--panel-soft);
  }

  .theme-preview-line {
    height: 0.58rem;
  }

  .theme-preview-line--strong {
    width: 68%;
    background: color-mix(in srgb, var(--ink-strong) 24%, transparent);
  }

  .theme-preview-badge {
    width: 2.4rem;
    height: 0.78rem;
    border-radius: 999px;
    background: var(--accent);
  }

  .theme-surface-card .panel__header {
    margin-bottom: -0.1rem;
  }

  .theme-token-grid {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }

  .theme-token-field {
    display: grid;
    gap: 0.45rem;
  }

  .theme-token-field span {
    color: var(--muted);
    font-size: 0.72rem;
    font-weight: 700;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  .theme-token-control {
    display: flex;
    align-items: center;
    gap: 0.65rem;
    min-width: 0;
    border: 1px solid rgba(148, 163, 184, 0.24);
    border-radius: 0.9rem;
    background: rgba(255, 255, 255, 0.9);
    padding: 0.45rem 0.6rem;
  }

  .theme-token-control input[type="color"] {
    width: 2.2rem;
    height: 2rem;
    border: none;
    padding: 0;
    background: transparent;
    cursor: pointer;
  }

  .theme-token-control code {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--ink-strong);
    font-size: 0.8rem;
  }

  @media (max-width: 1120px) {
    .settings-grid {
      grid-template-columns: 1fr;
    }

    .settings-column {
      grid-template-columns: repeat(2, minmax(0, 1fr));
      align-items: start;
    }

    .theme-preview-grid,
    .theme-editor-grid {
      grid-template-columns: 1fr;
    }
  }

  @media (max-width: 720px) {
    .settings-shell,
    .panel {
      padding: 0.85rem;
    }

    .settings-tabs {
      gap: 0.55rem;
      margin-inline: -0.1rem;
      padding-inline: 0.1rem;
    }

    .settings-tab {
      min-height: 2.7rem;
      padding: 0.65rem 0.8rem;
    }

    .settings-tab__label {
      font-size: 0.82rem;
    }

    .settings-shell__header {
      flex-direction: column;
      align-items: stretch;
    }

    .settings-column {
      grid-template-columns: 1fr;
    }

    .theme-token-grid {
      grid-template-columns: 1fr;
    }
  }
</style>
