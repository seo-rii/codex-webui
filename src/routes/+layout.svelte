<script lang="ts">
  import { browser } from "$app/environment";
  import { onMount } from "svelte";

  import { syncLocale, localeSignal } from "$lib/i18n";
  import { m } from "$lib/paraglide/messages.js";
  import { initThemeRuntime } from "$lib/theme";
  import "../app.css";

  let { children, data } = $props();
  const i18n = $derived.by(() => {
    const _locale = $localeSignal;
    return {
      appTitle: m.app_title()
    };
  });

  onMount(() => {
    initThemeRuntime();
    syncLocale(data.locale);
  });

  $effect(() => {
    if (!browser) {
      return;
    }

    syncLocale(data.locale);
  });
</script>

<svelte:head>
  <title>{i18n.appTitle}</title>
  <meta name="theme-color" content="#ffffff" />
  <script>
    {`
      (() => {
        try {
          const mode = localStorage.getItem("codex-webui.theme-mode");
          const normalizedMode = mode === "light" || mode === "dark" ? mode : "system";
          const resolved = normalizedMode === "dark" || (normalizedMode === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches)
            ? "dark"
            : "light";
          document.documentElement.dataset.themeMode = normalizedMode;
          document.documentElement.dataset.theme = resolved;
          document.documentElement.style.colorScheme = resolved;
          const meta = document.querySelector('meta[name="theme-color"]');
          if (meta) {
            meta.setAttribute("content", resolved === "dark" ? "#0b1220" : "#f8fafc");
          }
        } catch {}
      })();
    `}
  </script>
</svelte:head>

{@render children()}
