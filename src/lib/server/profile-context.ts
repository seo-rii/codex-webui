import { AsyncLocalStorage } from "node:async_hooks";

const profileStorage = new AsyncLocalStorage<string | null>();

export function runWithProfile<T>(profileId: string | null, callback: () => T) {
  return profileStorage.run(profileId, callback);
}

export function getCurrentProfileId() {
  return profileStorage.getStore() ?? null;
}
