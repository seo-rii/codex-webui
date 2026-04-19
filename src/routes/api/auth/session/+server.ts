import { json } from "@sveltejs/kit";

import { getLoginHcaptchaConfig } from "$lib/server/auth";

export function GET({ locals }) {
  const hcaptcha = getLoginHcaptchaConfig();
  return json({
    authenticated: locals.authenticated,
    role: locals.authenticated ? "admin" : null,
    activeProfileId: locals.profileId ?? null,
    hcaptcha: {
      enabled: hcaptcha.enabled,
      siteKey: hcaptcha.siteKey
    }
  });
}
