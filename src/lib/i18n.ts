import { browser } from "$app/environment";
import { get, writable } from "svelte/store";

import { getLocale, getTextDirection, setLocale, type Locale } from "$lib/paraglide/runtime.js";

export const localeSignal = writable(0);
export const activeLocale = writable<Locale>("en");

export const localeOptions = [
  { value: "en", label: "English" },
  { value: "ko", label: "한국어" },
  { value: "zh-Hans", label: "简体中文" },
  { value: "zh-Hant", label: "繁體中文" },
  { value: "ja", label: "日本語" },
  { value: "fr", label: "français" },
  { value: "es", label: "español" },
  { value: "de", label: "Deutsch" },
  { value: "it", label: "italiano" },
  { value: "pt-BR", label: "português (Brasil)" },
  { value: "ru", label: "Русский" }
] as const satisfies ReadonlyArray<{ value: Locale; label: string }>;

function applyLocaleToDocument(locale: Locale) {
  if (!browser) {
    return;
  }

  document.documentElement.lang = locale;
  document.documentElement.dir = getTextDirection(locale);
}

export function syncLocale(locale?: string | null) {
  const nextLocale = ((locale?.trim() || getLocale()) as Locale) ?? "en";
  activeLocale.set(nextLocale);
  applyLocaleToDocument(nextLocale);
  localeSignal.update((value) => value + 1);
}

export function getClientLocale() {
  return get(activeLocale);
}

export function updateLocale(locale: Locale) {
  void setLocale(locale, { reload: false });
  syncLocale(locale);
}
