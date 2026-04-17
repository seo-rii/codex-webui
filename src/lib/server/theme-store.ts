import fsp from "node:fs/promises";
import path from "node:path";

import { cloneThemeSettings, normalizeThemeSettings, type ThemeSettings } from "$lib/theme-customization";

import { getCurrentRuntimeProfile, type RuntimeProfileConfig } from "./env";
import { ensureDataDirectories, pathExists } from "./fs";

function getThemeStorePath(profile: RuntimeProfileConfig) {
  return path.join(profile.dataDir, "theme-settings.json");
}

class ThemeSettingsStore {
  private state: ThemeSettings | null = null;
  private writeChain = Promise.resolve();

  constructor(private readonly profile: RuntimeProfileConfig) {}

  private async load() {
    if (this.state) {
      return this.state;
    }

    await ensureDataDirectories(this.profile);
    const storePath = getThemeStorePath(this.profile);

    if (!(await pathExists(storePath))) {
      this.state = normalizeThemeSettings(null);
      return this.state;
    }

    try {
      const raw = await fsp.readFile(storePath, "utf8");
      this.state = normalizeThemeSettings(JSON.parse(raw));
    } catch {
      this.state = normalizeThemeSettings(null);
      await this.flush();
    }

    return this.state;
  }

  private async flush() {
    if (!this.state) {
      return;
    }
    await ensureDataDirectories(this.profile);
    await fsp.writeFile(getThemeStorePath(this.profile), JSON.stringify(this.state, null, 2), "utf8");
  }

  async get() {
    return cloneThemeSettings(await this.load());
  }

  async update(nextSettings: ThemeSettings) {
    let updated = normalizeThemeSettings(nextSettings);
    this.writeChain = this.writeChain.then(async () => {
      this.state = normalizeThemeSettings(nextSettings);
      updated = cloneThemeSettings(this.state);
      await this.flush();
    });
    await this.writeChain;
    return updated;
  }

  async reset() {
    return this.update(normalizeThemeSettings(null));
  }
}

const themeSettingsStores = new Map<string, ThemeSettingsStore>();

function getThemeSettingsStore(profile = getCurrentRuntimeProfile()) {
  const existing = themeSettingsStores.get(profile.id);
  if (existing) {
    return existing;
  }

  const created = new ThemeSettingsStore(profile);
  themeSettingsStores.set(profile.id, created);
  return created;
}

export function getStoredThemeSettings(profile?: RuntimeProfileConfig) {
  return getThemeSettingsStore(profile).get();
}

export function updateStoredThemeSettings(nextSettings: ThemeSettings, profile?: RuntimeProfileConfig) {
  return getThemeSettingsStore(profile).update(nextSettings);
}

export function resetStoredThemeSettings(profile?: RuntimeProfileConfig) {
  return getThemeSettingsStore(profile).reset();
}
