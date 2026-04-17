import { browser } from "$app/environment";
import {
  cloneThemeSettings,
  DEFAULT_THEME_SETTINGS,
  deriveThemeRuntimeVariables,
  type ThemeSettings
} from "$lib/theme-customization";

export type ThemeMode = "system" | "light" | "dark";
export type ResolvedTheme = "light" | "dark";

export const THEME_MODE_STORAGE_KEY = "codex-webui.theme-mode";
const THEME_CHANGE_EVENT = "codex-webui:themechange";

let activeThemeSettings: ThemeSettings = cloneThemeSettings(DEFAULT_THEME_SETTINGS);

function normalizeThemeMode(value: string | null | undefined): ThemeMode {
  if (value === "light" || value === "dark") {
    return value;
  }
  return "system";
}

export function readThemeMode(): ThemeMode {
  if (!browser) {
    return "system";
  }
  return normalizeThemeMode(window.localStorage.getItem(THEME_MODE_STORAGE_KEY));
}

export function getSystemTheme(): ResolvedTheme {
  if (!browser || typeof window.matchMedia !== "function") {
    return "light";
  }
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

export function resolveTheme(mode: ThemeMode): ResolvedTheme {
  return mode === "system" ? getSystemTheme() : mode;
}

export function getResolvedTheme(): ResolvedTheme {
  if (!browser) {
    return "light";
  }
  const value = document.documentElement.dataset.theme;
  return value === "dark" ? "dark" : "light";
}

function updateThemeColorMeta(theme: ResolvedTheme) {
  const meta = document.querySelector('meta[name="theme-color"]');
  if (!meta) {
    return;
  }
  meta.setAttribute("content", activeThemeSettings[theme].bg);
}

function applyResolvedThemeSettings(resolved: ResolvedTheme) {
  if (!browser) {
    return;
  }

  const variables = deriveThemeRuntimeVariables(activeThemeSettings, resolved);
  for (const [name, value] of Object.entries(variables)) {
    document.documentElement.style.setProperty(name, value);
  }
}

function emitThemeChange(mode: ThemeMode, resolved: ResolvedTheme) {
  window.dispatchEvent(
    new CustomEvent(THEME_CHANGE_EVENT, {
      detail: {
        mode,
        resolved,
        settings: cloneThemeSettings(activeThemeSettings)
      }
    })
  );
}

export function getThemeSettings() {
  return cloneThemeSettings(activeThemeSettings);
}

export function applyThemeSettings(settings: ThemeSettings | null | undefined, mode: ThemeMode = readThemeMode()) {
  activeThemeSettings = cloneThemeSettings(settings ?? DEFAULT_THEME_SETTINGS);
  const resolved = resolveTheme(mode);

  if (!browser) {
    return {
      mode,
      resolved,
      settings: cloneThemeSettings(activeThemeSettings)
    };
  }

  applyResolvedThemeSettings(resolved);
  updateThemeColorMeta(resolved);
  emitThemeChange(mode, resolved);

  return {
    mode,
    resolved,
    settings: cloneThemeSettings(activeThemeSettings)
  };
}

export function applyThemeMode(mode: ThemeMode, persist = true) {
  const normalizedMode = normalizeThemeMode(mode);
  const resolved = resolveTheme(normalizedMode);

  if (!browser) {
    return {
      mode: normalizedMode,
      resolved,
      settings: cloneThemeSettings(activeThemeSettings)
    };
  }

  if (persist) {
    window.localStorage.setItem(THEME_MODE_STORAGE_KEY, normalizedMode);
  }

  document.documentElement.dataset.themeMode = normalizedMode;
  document.documentElement.dataset.theme = resolved;
  document.documentElement.style.colorScheme = resolved;
  applyResolvedThemeSettings(resolved);
  updateThemeColorMeta(resolved);
  emitThemeChange(normalizedMode, resolved);

  return {
    mode: normalizedMode,
    resolved,
    settings: cloneThemeSettings(activeThemeSettings)
  };
}

export function subscribeThemeChange(listener: (detail: { mode: ThemeMode; resolved: ResolvedTheme; settings: ThemeSettings }) => void) {
  if (!browser) {
    return () => {};
  }

  const handleEvent = (event: Event) => {
    const detail = (event as CustomEvent<{ mode: ThemeMode; resolved: ResolvedTheme; settings: ThemeSettings }>).detail;
    if (!detail) {
      return;
    }
    listener(detail);
  };

  window.addEventListener(THEME_CHANGE_EVENT, handleEvent);

  return () => {
    window.removeEventListener(THEME_CHANGE_EVENT, handleEvent);
  };
}

export function initThemeRuntime() {
  if (!browser) {
    return () => {};
  }

  let mode = readThemeMode();
  const mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");

  applyThemeSettings(activeThemeSettings, mode);
  applyThemeMode(mode, false);

  const handleMediaChange = () => {
    if (mode !== "system") {
      return;
    }
    applyThemeMode(mode, false);
  };

  const handleStorage = (event: StorageEvent) => {
    if (event.key !== THEME_MODE_STORAGE_KEY) {
      return;
    }
    mode = readThemeMode();
    applyThemeMode(mode, false);
  };

  const handleThemeChange = (event: Event) => {
    const detail = (event as CustomEvent<{ mode: ThemeMode; resolved: ResolvedTheme; settings: ThemeSettings }>).detail;
    if (!detail) {
      return;
    }
    mode = detail.mode;
  };

  mediaQuery.addEventListener("change", handleMediaChange);
  window.addEventListener("storage", handleStorage);
  window.addEventListener(THEME_CHANGE_EVENT, handleThemeChange);

  return () => {
    mediaQuery.removeEventListener("change", handleMediaChange);
    window.removeEventListener("storage", handleStorage);
    window.removeEventListener(THEME_CHANGE_EVENT, handleThemeChange);
  };
}
