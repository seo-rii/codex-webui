<script lang="ts">
  import { base } from "$app/paths";
  import { browser } from "$app/environment";
  import { onMount } from "svelte";

  import { syncLocale, localeSignal } from "$lib/i18n";
  import { m } from "$lib/paraglide/messages.js";
  import { initThemeRuntime } from "$lib/theme";
  import "../app.css";

  let { children } = $props();
  const i18n = $derived.by(() => {
    const _locale = $localeSignal;
    return {
      appTitle: m.app_title()
    };
  });

  onMount(() => {
    initThemeRuntime();
    syncLocale();
    if (!import.meta.env.DEV && "serviceWorker" in navigator) {
      void navigator.serviceWorker.register(`${base}/service-worker.js`, {
        scope: base ? `${base}/` : "/"
      }).catch(() => {});
    }
  });

  $effect(() => {
    if (!browser) {
      return;
    }

    syncLocale();
  });
</script>

<svelte:head>
  <title>{i18n.appTitle}</title>
  <meta name="application-name" content={i18n.appTitle} />
  <meta name="theme-color" content="#ffffff" />
  <meta name="mobile-web-app-capable" content="yes" />
  <meta name="apple-mobile-web-app-capable" content="yes" />
  <meta name="apple-mobile-web-app-status-bar-style" content="black-translucent" />
  <meta name="apple-mobile-web-app-title" content={i18n.appTitle} />
  <link rel="manifest" href={`${base}/manifest.webmanifest`} />
  <link rel="apple-touch-icon" href={`${base}/apple-touch-icon.png`} />
  <link rel="icon" href={`${base}/icon-192.png`} />
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
