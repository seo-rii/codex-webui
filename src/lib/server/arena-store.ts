import fsp from "node:fs/promises";
import path from "node:path";

import type { ArenaRun } from "$lib/arena-types";

import { getCurrentRuntimeProfile } from "./env";
import { ensureDataDirectories, pathExists } from "./fs";

type ArenaState = {
  runs: ArenaRun[];
};

function getArenaStoreFilePath() {
  return path.join(getCurrentRuntimeProfile().dataDir, "arena-runs.json");
}

class ArenaStore {
  private state: ArenaState | null = null;
  private writeChain = Promise.resolve();

  private async load() {
    if (this.state) {
      return this.state;
    }

    await ensureDataDirectories();
    const filePath = getArenaStoreFilePath();
    if (!(await pathExists(filePath))) {
      this.state = {
        runs: []
      };
      return this.state;
    }

    try {
      const raw = await fsp.readFile(filePath, "utf8");
      const parsed = JSON.parse(raw) as Partial<ArenaState>;
      this.state = {
        runs: Array.isArray(parsed.runs) ? parsed.runs : []
      };
    } catch {
      this.state = {
        runs: []
      };
      await this.flush();
    }

    return this.state;
  }

  private async flush() {
    await ensureDataDirectories();
    await fsp.writeFile(getArenaStoreFilePath(), JSON.stringify(this.state, null, 2), "utf8");
  }

  async getRuns() {
    const state = await this.load();
    return state.runs.map((run) => ({
      ...run,
      contestants: run.contestants.map((contestant) => ({ ...contestant }))
    }));
  }

  async saveRun(run: ArenaRun) {
    this.writeChain = this.writeChain.then(async () => {
      const state = await this.load();
      state.runs = [run, ...state.runs.filter((entry) => entry.id !== run.id)].slice(0, 60);
      await this.flush();
    });
    await this.writeChain;
    return run;
  }

  async updateRun(runId: string, updater: (run: ArenaRun | null) => ArenaRun | null) {
    let updated: ArenaRun | null = null;
    this.writeChain = this.writeChain.then(async () => {
      const state = await this.load();
      const index = state.runs.findIndex((entry) => entry.id === runId);
      const next = updater(index >= 0 ? state.runs[index] : null);
      if (!next) {
        if (index >= 0) {
          state.runs.splice(index, 1);
          await this.flush();
        }
        return;
      }
      if (index >= 0) {
        state.runs[index] = next;
      } else {
        state.runs.unshift(next);
      }
      updated = next;
      await this.flush();
    });
    await this.writeChain;
    return updated;
  }

  async getHiddenSessionIds() {
    const runs = await this.getRuns();
    return new Set(runs.flatMap((run) => run.contestants.map((contestant) => contestant.sessionId)));
  }
}

export const arenaStore = new ArenaStore();
