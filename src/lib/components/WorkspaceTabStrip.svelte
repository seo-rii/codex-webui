<script lang="ts">
  import { FileDiff, FileText, GitBranch, History, Layout, MessageSquare, Settings, Terminal, X } from "lucide-svelte";

  type WorkspaceTab = {
    id: string;
    label: string;
    kind: "chat" | "tasks" | "git" | "settings" | "git-diff" | "code-diff" | "file" | "terminal";
  };

  let {
    tabs,
    activeTabId,
    onActivate,
    onClose
  }: {
    tabs: WorkspaceTab[];
    activeTabId: string;
    onActivate: (tabId: string) => void;
    onClose: (tabId: string, kind: WorkspaceTab["kind"]) => void | Promise<void>;
  } = $props();

  function closeTab(event: MouseEvent | KeyboardEvent, tab: WorkspaceTab) {
    event.preventDefault();
    event.stopPropagation();
    void onClose(tab.id, tab.kind);
  }
</script>

<div class="workspace-tab-strip flex items-center gap-1 overflow-x-auto border-b border-gray-200 bg-gray-50 px-4 py-1.5 scrollbar-none">
  {#each tabs as tab (tab.id)}
    <button
      class={`ui-animated-button ui-animated-button--soft flex items-center gap-2 whitespace-nowrap rounded-lg px-3 py-1.5 text-xs font-semibold transition-all ${
        activeTabId === tab.id
          ? "border border-gray-200 bg-white text-gray-900 shadow-sm"
          : "text-gray-500 hover:bg-gray-100/50 hover:text-gray-700"
      }`}
      onclick={() => onActivate(tab.id)}
      type="button"
    >
      {#if tab.kind === "chat"}
        <MessageSquare size={14} />
      {:else if tab.kind === "tasks"}
        <History size={14} />
      {:else if tab.kind === "git"}
        <GitBranch size={14} />
      {:else if tab.kind === "settings"}
        <Settings size={14} />
      {:else if tab.kind === "git-diff"}
        <FileDiff size={14} />
      {:else if tab.kind === "code-diff"}
        <Layout size={14} />
      {:else if tab.kind === "file"}
        <FileText size={14} />
      {:else if tab.kind === "terminal"}
        <Terminal size={14} />
      {/if}
      <span>{tab.label}</span>
      {#if tab.id !== "chat"}
        <span
          aria-label={`Close ${tab.label}`}
          class="ml-1 rounded p-0.5 transition-colors hover:bg-gray-200"
          onclick={(event) => closeTab(event, tab)}
          onkeydown={(event) => {
            if (event.key !== "Enter" && event.key !== " ") {
              return;
            }
            closeTab(event, tab);
          }}
          role="button"
          tabindex="0"
        >
          <X size={10} />
        </span>
      {/if}
    </button>
  {/each}
</div>
