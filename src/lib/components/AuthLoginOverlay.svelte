<script lang="ts">
  import { ChevronDown } from "lucide-svelte";

  import type { LoginHcaptchaConfig } from "$lib/types";

  type LocaleOption = {
    value: string;
    label: string;
  };

  type UiCopy = {
    privateGateway: string;
    appTitle: string;
    loginLede: string;
    language: string;
    password: string;
    signingIn: string;
    signIn: string;
  };

  let {
    ui,
    localeOptions,
    activeLocale,
    loginPassword = $bindable(""),
    loginBusy,
    loginMessage,
    loginHcaptcha,
    loginHcaptchaToken,
    loginHcaptchaContainer = $bindable(null),
    onLocaleChange,
    onSubmit
  }: {
    ui: UiCopy;
    localeOptions: readonly LocaleOption[];
    activeLocale: string;
    loginPassword: string;
    loginBusy: boolean;
    loginMessage: string;
    loginHcaptcha: LoginHcaptchaConfig;
    loginHcaptchaToken: string;
    loginHcaptchaContainer: HTMLDivElement | null;
    onLocaleChange: (locale: string) => void;
    onSubmit: () => void | Promise<void>;
  } = $props();
</script>

<div class="ui-scrim ui-scrim--soft absolute inset-0"></div>
<div class="absolute inset-0 z-10 flex items-center justify-center p-4 sm:p-6">
  <div class="auth-dialog-card w-full max-w-xl rounded-[2rem] border border-white/70 bg-white/92 p-6 shadow-[0_32px_90px_rgba(15,23,42,0.24)] backdrop-blur-2xl sm:p-8">
    <div class="flex flex-col gap-5 sm:flex-row sm:items-start sm:justify-between">
      <div class="space-y-3">
        <p class="text-[11px] font-bold uppercase tracking-[0.28em] text-amber-700">{ui.privateGateway}</p>
        <div>
          <h1 class="text-3xl font-semibold tracking-tight text-gray-950 sm:text-4xl">{ui.appTitle}</h1>
          <p class="mt-3 max-w-md text-sm leading-7 text-gray-500">{ui.loginLede}</p>
        </div>
      </div>
      <label class="flex min-w-[12rem] flex-col gap-1.5">
        <span class="text-[10px] font-bold uppercase tracking-[0.18em] text-gray-400">{ui.language}</span>
        <div class="relative">
          <select
            aria-label={ui.language}
            class="auth-dialog-select w-full appearance-none rounded-2xl border border-gray-200 bg-white px-3.5 py-2.5 pr-9 text-sm font-semibold text-gray-700 shadow-sm outline-none transition focus:border-amber-400 focus:ring-4 focus:ring-amber-100"
            onchange={(event) => onLocaleChange((event.currentTarget as HTMLSelectElement).value)}
            value={activeLocale}
          >
            {#each localeOptions as option (option.value)}
              <option value={option.value}>{option.label}</option>
            {/each}
          </select>
          <div class="pointer-events-none absolute inset-y-0 right-3 flex items-center text-gray-400">
            <ChevronDown size={16} />
          </div>
        </div>
      </label>
    </div>

    <form
      class="mt-8 space-y-5"
      data-testid="login-form"
      onsubmit={(event) => {
        event.preventDefault();
        void onSubmit();
      }}
    >
      <label class="block space-y-2">
        <span class="text-sm font-semibold text-gray-700">{ui.password}</span>
        <input
          bind:value={loginPassword}
          autocomplete="current-password"
          class="auth-dialog-input w-full rounded-2xl border border-gray-200 bg-white px-4 py-3 text-sm text-gray-900 shadow-sm outline-none transition focus:border-amber-500 focus:ring-4 focus:ring-amber-100"
          data-testid="login-password"
          placeholder={ui.password}
          type="password"
        />
      </label>

      {#if loginHcaptcha.enabled && loginHcaptcha.siteKey}
        <div
          bind:this={loginHcaptchaContainer}
          class="auth-dialog-hcaptcha min-h-[82px] overflow-hidden rounded-2xl border border-gray-200 bg-white px-3 py-3 shadow-sm"
        ></div>
      {/if}

      <button
        class="inline-flex min-w-32 items-center justify-center rounded-2xl bg-amber-600 px-5 py-3 text-sm font-semibold text-white shadow-lg shadow-amber-200/70 transition hover:bg-amber-700 disabled:cursor-not-allowed disabled:bg-amber-300"
        data-testid="login-submit"
        disabled={loginBusy || (loginHcaptcha.enabled && !loginHcaptchaToken)}
        type="submit"
      >
        {loginBusy ? ui.signingIn : ui.signIn}
      </button>
    </form>

    {#if loginMessage}
      <p class="auth-dialog-message mt-4 rounded-2xl border border-red-100 bg-red-50 px-4 py-3 text-sm text-red-700">{loginMessage}</p>
    {/if}
  </div>
</div>
