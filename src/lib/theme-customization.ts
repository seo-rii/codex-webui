export const THEME_SURFACES = ["light", "dark"] as const;
export const THEME_TOKEN_KEYS = [
  "bg",
  "bgSidebar",
  "bgAccent",
  "panelStrong",
  "panelSoft",
  "inkStrong",
  "ink",
  "muted",
  "accent",
  "line"
] as const;

export type ThemeSurface = (typeof THEME_SURFACES)[number];
export type ThemeTokenKey = (typeof THEME_TOKEN_KEYS)[number];
export type ThemePalette = Record<ThemeTokenKey, string>;
export type ThemeSettings = Record<ThemeSurface, ThemePalette>;

export const DEFAULT_THEME_SETTINGS: ThemeSettings = {
  light: {
    bg: "#f8fafc",
    bgSidebar: "#f7f7f8",
    bgAccent: "#f3f4f6",
    panelStrong: "#ffffff",
    panelSoft: "#f8fafc",
    inkStrong: "#111827",
    ink: "#334155",
    muted: "#64748b",
    accent: "#d97706",
    line: "#e2e8f0"
  },
  dark: {
    bg: "#0b1220",
    bgSidebar: "#111827",
    bgAccent: "#172033",
    panelStrong: "#0f172a",
    panelSoft: "#111827",
    inkStrong: "#f8fafc",
    ink: "#cbd5e1",
    muted: "#94a3b8",
    accent: "#d97706",
    line: "#334155"
  }
};

function clonePalette(palette: ThemePalette): ThemePalette {
  return { ...palette };
}

function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
}

function normalizeHexColor(value: unknown, fallback: string) {
  if (typeof value !== "string") {
    return fallback;
  }

  const trimmed = value.trim();
  if (!trimmed) {
    return fallback;
  }

  const normalized = trimmed.startsWith("#") ? trimmed.slice(1) : trimmed;
  if (!/^[0-9a-f]{3}([0-9a-f]{3})?$/iu.test(normalized)) {
    return fallback;
  }

  if (normalized.length === 3) {
    return `#${normalized
      .split("")
      .map((entry) => `${entry}${entry}`)
      .join("")
      .toLowerCase()}`;
  }

  return `#${normalized.toLowerCase()}`;
}

function hexToRgb(value: string) {
  const normalized = normalizeHexColor(value, "#000000").slice(1);
  return {
    r: Number.parseInt(normalized.slice(0, 2), 16),
    g: Number.parseInt(normalized.slice(2, 4), 16),
    b: Number.parseInt(normalized.slice(4, 6), 16)
  };
}

export function mixHex(left: string, right: string, ratio = 0.5) {
  const amount = clamp(ratio, 0, 1);
  const start = hexToRgb(left);
  const end = hexToRgb(right);

  const mixChannel = (from: number, to: number) => Math.round(from + (to - from) * amount);
  const toHex = (channel: number) => clamp(channel, 0, 255).toString(16).padStart(2, "0");

  return `#${toHex(mixChannel(start.r, end.r))}${toHex(mixChannel(start.g, end.g))}${toHex(mixChannel(start.b, end.b))}`;
}

export function withAlpha(color: string, alpha: number) {
  const { r, g, b } = hexToRgb(color);
  const normalizedAlpha = clamp(alpha, 0, 1);
  return `rgba(${r}, ${g}, ${b}, ${normalizedAlpha.toFixed(3).replace(/0+$/u, "").replace(/\.$/u, "")})`;
}

export function normalizeThemePalette(value: unknown, fallback: ThemePalette): ThemePalette {
  const record = value && typeof value === "object" ? (value as Record<string, unknown>) : {};
  return {
    bg: normalizeHexColor(record.bg, fallback.bg),
    bgSidebar: normalizeHexColor(record.bgSidebar, fallback.bgSidebar),
    bgAccent: normalizeHexColor(record.bgAccent, fallback.bgAccent),
    panelStrong: normalizeHexColor(record.panelStrong, fallback.panelStrong),
    panelSoft: normalizeHexColor(record.panelSoft, fallback.panelSoft),
    inkStrong: normalizeHexColor(record.inkStrong, fallback.inkStrong),
    ink: normalizeHexColor(record.ink, fallback.ink),
    muted: normalizeHexColor(record.muted, fallback.muted),
    accent: normalizeHexColor(record.accent, fallback.accent),
    line: normalizeHexColor(record.line, fallback.line)
  };
}

export function normalizeThemeSettings(value: unknown): ThemeSettings {
  const record = value && typeof value === "object" ? (value as Record<string, unknown>) : {};
  return {
    light: normalizeThemePalette(record.light, DEFAULT_THEME_SETTINGS.light),
    dark: normalizeThemePalette(record.dark, DEFAULT_THEME_SETTINGS.dark)
  };
}

export function cloneThemeSettings(value: ThemeSettings | null | undefined) {
  const normalized = normalizeThemeSettings(value);
  return {
    light: clonePalette(normalized.light),
    dark: clonePalette(normalized.dark)
  } satisfies ThemeSettings;
}

export function deriveThemeRuntimeVariables(settings: ThemeSettings, surface: ThemeSurface) {
  const palette = settings[surface];
  const accentSoft = withAlpha(palette.accent, surface === "dark" ? 0.16 : 0.08);
  const brandSecondary = mixHex(palette.accent, surface === "dark" ? "#f8fafc" : "#ffffff", surface === "dark" ? 0.18 : 0.34);
  const scrollbarThumb = mixHex(palette.line, palette.ink, surface === "dark" ? 0.46 : 0.28);
  const scrollbarThumbHover = mixHex(palette.line, palette.inkStrong, surface === "dark" ? 0.62 : 0.45);

  return {
    "--bg": palette.bg,
    "--bg-sidebar": palette.bgSidebar,
    "--ink": palette.ink,
    "--ink-strong": palette.inkStrong,
    "--muted": palette.muted,
    "--accent": palette.accent,
    "--accent-soft": accentSoft,
    "--line": palette.line,
    "--panel-strong": palette.panelStrong,
    "--panel-soft": palette.panelSoft,
    "--scrollbar-thumb": scrollbarThumb,
    "--scrollbar-thumb-hover": scrollbarThumbHover,
    "--color-brand-primary": palette.accent,
    "--color-brand-secondary": brandSecondary,
    "--color-bg-main": palette.bg,
    "--color-bg-sidebar": palette.bgSidebar,
    "--color-bg-accent": palette.bgAccent,
    "--color-text-main": palette.inkStrong,
    "--color-text-muted": palette.muted,
    "--color-border-subtle": palette.line
  } satisfies Record<string, string>;
}
