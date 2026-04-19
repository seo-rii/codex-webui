<script lang="ts">
  import { onMount } from "svelte";

  import { appPath } from "$lib/api";

  onMount(() => {
    window.location.replace(appPath("/"));
  });
</script>

<p>Redirecting…</p>
