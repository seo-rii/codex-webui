import { error as svelteError } from "@sveltejs/kit";

import { createAppError, serializeAppError, type AppErrorCode } from "$lib/errors";

export function throwRouteError(status: number, code: AppErrorCode, message?: string): never {
  throw svelteError(
    status,
    serializeAppError({
      code,
      message,
      status
    })
  );
}

export function throwAppError(code: AppErrorCode, message?: string): never {
  throw createAppError(code, message);
}
