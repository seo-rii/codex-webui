<script lang="ts">
  import { resolve } from "$app/paths";
  import { goto } from "$app/navigation";

  import { api } from "$lib/api";
  import { activeLocale, localeOptions, localeSignal, updateLocale } from "$lib/i18n";
  import { m } from "$lib/paraglide/messages.js";

  let password = $state("");
  let loading = $state(false);
  let message = $state("");
  const ui = $derived.by(() => {
    const _locale = $localeSignal;

    return {
      title: m.login_page_title(),
      appTitle: m.app_title(),
      gateway: m.private_gateway(),
      lede: m.login_lede(),
      password: m.password(),
      signIn: m.sign_in(),
      signingIn: m.signing_in(),
      enterPassword: m.enter_password(),
      loginFailed: m.login_failed(),
      language: m.language()
    };
  });

  async function handleLogin() {
    if (!password.trim()) {
      message = ui.enterPassword;
      return;
    }

    loading = true;
    message = "";

    try {
      await api.login(password);
      await goto(resolve("/"));
    } catch (error) {
      message = error instanceof Error ? error.message : ui.loginFailed;
    } finally {
      loading = false;
    }
  }
</script>

<svelte:head>
  <title>{ui.title}</title>
</svelte:head>

<div class="login-shell">
  <section class="login-card surface">
    <div class="card-header">
      <div>
        <p class="eyebrow">{ui.gateway}</p>
        <h1>{ui.appTitle}</h1>
      </div>
      <div class="locale-switcher" role="group" aria-label={ui.language}>
        {#each localeOptions as option (option.value)}
          <button
            class:locale-switcher__button--active={$activeLocale === option.value}
            class="locale-switcher__button"
            onclick={() => updateLocale(option.value)}
            type="button"
          >
            {option.label}
          </button>
        {/each}
      </div>
    </div>
    <p class="lede">{ui.lede}</p>

    <label>
      <span>{ui.password}</span>
      <input bind:value={password} type="password" />
    </label>

    <button class="solid-button" disabled={loading} type="button" onclick={handleLogin}>
      {loading ? ui.signingIn : ui.signIn}
    </button>

    {#if message}
      <p class="message">{message}</p>
    {/if}
  </section>
</div>

<style>
  .login-shell {
    display: grid;
    min-height: 100vh;
    place-items: center;
    padding: 2rem;
  }

  .login-card {
    width: min(28rem, 100%);
    padding: 2rem;
    background: var(--panel-strong);
  }

  .card-header {
    align-items: flex-start;
    display: flex;
    gap: 1rem;
    justify-content: space-between;
  }

  .eyebrow {
    margin: 0;
    color: var(--muted);
    font-size: 0.75rem;
    letter-spacing: 0.2em;
    text-transform: uppercase;
  }

  h1 {
    margin: 0.35rem 0 0.65rem;
    color: var(--ink-strong);
    font: 600 2.2rem/1 var(--font-display);
  }

  .lede {
    margin: 0 0 1.4rem;
    color: var(--muted);
    line-height: 1.6;
  }

  .locale-switcher {
    display: inline-flex;
    gap: 0.35rem;
    padding: 0.3rem;
    border: 1px solid var(--line);
    border-radius: 999px;
    background: color-mix(in srgb, var(--panel-strong) 92%, transparent);
  }

  .locale-switcher__button {
    border: 0;
    border-radius: 999px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    font: 600 0.75rem/1 var(--font-body);
    padding: 0.45rem 0.7rem;
    transition:
      background-color 160ms ease,
      color 160ms ease;
  }

  .locale-switcher__button--active {
    background: color-mix(in srgb, var(--accent) 14%, white);
    color: var(--ink-strong);
  }

  label {
    display: grid;
    gap: 0.5rem;
    margin-bottom: 1rem;
  }

  input {
    border: 1px solid var(--line);
    border-radius: 1rem;
    background: color-mix(in srgb, var(--panel-strong) 88%, transparent);
    color: var(--ink-strong);
    padding: 0.95rem 1rem;
  }

  .message {
    margin: 1rem 0 0;
    color: #b34a2f;
  }
</style>
