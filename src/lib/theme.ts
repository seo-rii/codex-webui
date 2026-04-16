import { browser } from "$app/environment";

export type ThemeMode = "system" | "light" | "dark";
export type ResolvedTheme = "light" | "dark";

export const THEME_MODE_STORAGE_KEY = "codex-webui.theme-mode";
const THEME_CHANGE_EVENT = "codex-webui:themechange";

const themeColorByMode: Record<ResolvedTheme, string> = {
  light: "#f8fafc",
  dark: "#0b1220"
};

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
  meta.setAttribute("content", themeColorByMode[theme]);
}

function emitThemeChange(mode: ThemeMode, resolved: ResolvedTheme) {
  window.dispatchEvent(
    new CustomEvent(THEME_CHANGE_EVENT, {
      detail: {
        mode,
        resolved
      }
    })
  );
}

export function applyThemeMode(mode: ThemeMode, persist = true) {
  const normalizedMode = normalizeThemeMode(mode);
  const resolved = resolveTheme(normalizedMode);

  if (!browser) {
    return {
      mode: normalizedMode,
      resolved
    };
  }

  if (persist) {
    window.localStorage.setItem(THEME_MODE_STORAGE_KEY, normalizedMode);
  }

  document.documentElement.dataset.themeMode = normalizedMode;
  document.documentElement.dataset.theme = resolved;
  document.documentElement.style.colorScheme = resolved;
  updateThemeColorMeta(resolved);
  emitThemeChange(normalizedMode, resolved);

  return {
    mode: normalizedMode,
    resolved
  };
}

export function subscribeThemeChange(listener: (detail: { mode: ThemeMode; resolved: ResolvedTheme }) => void) {
  if (!browser) {
    return () => {};
  }

  const handleEvent = (event: Event) => {
    const detail = (event as CustomEvent<{ mode: ThemeMode; resolved: ResolvedTheme }>).detail;
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
    const detail = (event as CustomEvent<{ mode: ThemeMode; resolved: ResolvedTheme }>).detail;
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
