<script lang="ts">
  import { onMount } from "svelte";

  import { api } from "$lib/api";
  import MonacoDiffEditor from "$lib/components/MonacoDiffEditor.svelte";
  import MonacoTextEditor from "$lib/components/MonacoTextEditor.svelte";
  import { localeSignal } from "$lib/i18n";
  import { m } from "$lib/paraglide/messages.js";
  import { getLocale } from "$lib/paraglide/runtime.js";
  import { describeUiError } from "$lib/ui-errors";
  import type { GitCommit, GitFilePayload, GitFileStatus, GitOpenRequest, GitRepository, GitStatusPayload, GitWorktree } from "$lib/types";

  let {
    selectedRepoPath,
    readOnly = false,
    onSelectRepo,
    openRequest = null,
    onOpenDiffTab = null,
    onOpenCommitDiff = null
  }: {
    selectedRepoPath: string | null;
    readOnly?: boolean;
    onSelectRepo: (repoPath: string | null) => void;
    openRequest?: GitOpenRequest | null;
    onOpenDiffTab?: ((repoPath: string, filePath: string) => void) | null;
    onOpenCommitDiff?: ((repoPath: string, commit: GitCommit) => Promise<void> | void) | null;
  } = $props();

  let repositories = $state<GitRepository[]>([]);
  let status = $state<GitStatusPayload | null>(null);
  let filePayload = $state<GitFilePayload | null>(null);
  let groupedFilePayloads = $state<GitFilePayload[]>([]);
  let groupedFileTitle = $state("");
  let worktrees = $state<GitWorktree[]>([]);
  let editorValue = $state("");
  let commitMessage = $state("");
  let newBranchName = $state("");
  let newWorktreePath = $state("");
  let newWorktreeBranch = $state("");
  let createWorktreeBranch = $state(true);
  let detachWorktree = $state(false);
  let loadingRepos = $state(true);
  let loadingStatus = $state(false);
  let loadingWorktrees = $state(false);
  let loadingFile = $state(false);
  let savingFile = $state(false);
  let gitBusy = $state(false);
  let gitBusyAction = $state<"fetch" | "pull" | null>(null);
  let worktreeBusy = $state(false);
  let openingCommitHash = $state<string | null>(null);
  let errorText = $state("");
  let lastRepoPath: string | null = null;
  let lastOpenRequestId = $state<number | null>(null);
  let isMobileLayout = $state(false);
  let mobileSection = $state<"repository" | "worktrees" | "changes" | "commits" | "detail">("repository");
  const hasDetailPanel = $derived(groupedFilePayloads.length > 0 || Boolean(filePayload));
  const ui = $derived.by(() => {
    const _locale = $localeSignal;

    return {
      git: m.git(),
      repository: m.repository(),
      refreshRepos: m.refresh_repos(),
      selectedRepo: m.select_repository(),
      detached: m.detached(),
      repoSelectionNote: m.repo_selection_note(),
      loadingRepositoryStatus: m.loading_repository_status(),
      detachedHead: m.detached_head(),
      worktreePath: m.worktree_path(),
      worktreeBranch: m.worktree_branch(),
      detachedHeadShort: m.detached_head_short(),
      branchOrNewBranch: m.branch_or_new_branch(),
      createBranchOption: m.create_branch_option(),
      detachHeadOption: m.detach_head_option(),
      creating: m.creating(),
      addWorktree: m.add_worktree(),
      stageAll: m.stage_all(),
      unstageAll: m.unstage_all(),
      fetch: m.fetch(),
      fetching: m.fetching(),
      pull: m.pull(),
      pulling: m.pulling(),
      switchBranch: m.switch_branch(),
      newBranchName: m.new_branch_name(),
      create: m.create(),
      commitMessage: m.commit_message(),
      commit: m.commit(),
      worktrees: m.worktrees({ count: String(worktrees.length) }),
      refresh: m.reload(),
      loadingWorktrees: m.loading_worktrees(),
      noLinkedWorktrees: m.no_linked_worktrees(),
      current: m.current(),
      open: m.open(),
      remove: m.remove(),
      changes: m.changes(),
      workingTreeClean: m.working_tree_clean(),
      stage: m.stage(),
      unstage: m.unstage(),
      loadingFile: m.loading_file(),
      close: m.close(),
      singleTab: m.single_tab(),
      binaryDiffNotPreviewable: m.binary_diff_not_previewable(),
      diff: m.diff(),
      edit: m.edit(),
      saveFile: m.save_file(),
      saving: m.saving(),
      recentCommits: m.recent_commits(),
      openTab: m.open_tab(),
      gitErrorGeneric: m.git_error_generic()
    };
  });

  function getDateLocale() {
    return getLocale() === "ko" ? "ko-KR" : "en-US";
  }

  onMount(() => {
    void bootstrap();
    if (typeof window === "undefined") {
      return;
    }

    const mobileQuery = window.matchMedia("(max-width: 720px)");
    const syncMobileLayout = () => {
      isMobileLayout = mobileQuery.matches;
      if (!mobileQuery.matches) {
        mobileSection = "repository";
      } else if (mobileSection === "detail" && !hasDetailPanel) {
        mobileSection = "changes";
      }
    };

    syncMobileLayout();
    mobileQuery.addEventListener("change", syncMobileLayout);

    return () => {
      mobileQuery.removeEventListener("change", syncMobileLayout);
    };
  });

  $effect(() => {
    const repoPath = selectedRepoPath;
    if (repoPath === lastRepoPath) {
      return;
    }

    lastRepoPath = repoPath;
    clearDetailPanels();
    mobileSection = "repository";

    if (!repoPath) {
      status = null;
      return;
    }

    void refreshStatus(repoPath);
  });

  async function bootstrap() {
    loadingRepos = true;
    errorText = "";

    try {
      const response = await api.listRepositories();
      repositories = response.repositories;

      if (selectedRepoPath && !repositories.some((repository) => repository.path === selectedRepoPath)) {
        onSelectRepo(null);
      }
    } catch (error) {
      errorText = describeError(error);
    } finally {
      loadingRepos = false;
    }
  }

  async function refreshStatus(repoPath: string) {
    loadingStatus = true;
    loadingWorktrees = true;
    errorText = "";

    try {
      const [nextStatus, nextWorktrees] = await Promise.all([api.getGitStatus(repoPath), api.getGitWorktrees(repoPath)]);
      status = nextStatus;
      worktrees = nextWorktrees.worktrees;
    } catch (error) {
      errorText = describeError(error);
    } finally {
      loadingStatus = false;
      loadingWorktrees = false;
    }
  }

  async function selectRepository(repoPath: string) {
    onSelectRepo(repoPath || null);
    status = null;
    worktrees = [];
    filePayload = null;
  }

  async function openFile(fileStatus: GitFileStatus) {
    if (!isMobileLayout && selectedRepoPath && onOpenDiffTab) {
      onOpenDiffTab(selectedRepoPath, fileStatus.path);
      return;
    }

    await openFileByPath(selectedRepoPath, fileStatus.path);
  }

  async function openCommit(commit: GitCommit) {
    if (!selectedRepoPath || !onOpenCommitDiff) {
      return;
    }

    openingCommitHash = commit.hash;
    errorText = "";

    try {
      await onOpenCommitDiff(selectedRepoPath, commit);
    } catch (error) {
      errorText = describeError(error);
    } finally {
      if (openingCommitHash === commit.hash) {
        openingCommitHash = null;
      }
    }
  }

  function clearDetailPanels() {
    filePayload = null;
    groupedFilePayloads = [];
    groupedFileTitle = "";
    editorValue = "";
  }

  async function openFileByPath(repoPath: string | null, filePath: string) {
    if (!repoPath) {
      return;
    }

    if (!selectedRepoPath) {
      onSelectRepo(repoPath);
    }

    loadingFile = true;
    errorText = "";
    clearDetailPanels();

    try {
      filePayload = await api.getGitFile(repoPath, filePath);
      editorValue = filePayload.modifiedContent;
      mobileSection = "detail";
    } catch (error) {
      errorText = describeError(error);
    } finally {
      loadingFile = false;
    }
  }

  async function openGroupedFilesByPath(repoPath: string | null, filePaths: string[], title: string | null = null) {
    if (!repoPath) {
      return;
    }

    const uniqueFilePaths = [...new Set(filePaths.filter((filePath) => filePath.trim().length > 0))];
    if (uniqueFilePaths.length === 0) {
      clearDetailPanels();
      return;
    }

    if (!selectedRepoPath) {
      onSelectRepo(repoPath);
    }

    loadingFile = true;
    errorText = "";
    clearDetailPanels();

    try {
      groupedFilePayloads = await Promise.all(uniqueFilePaths.map((filePath) => api.getGitFile(repoPath, filePath)));
      groupedFileTitle = title?.trim() || `${groupedFilePayloads.length} files changed`;
      mobileSection = "detail";
    } catch (error) {
      errorText = describeError(error);
    } finally {
      loadingFile = false;
    }
  }

  async function saveFile() {
    if (!selectedRepoPath || !filePayload || filePayload.isBinary || readOnly) {
      return;
    }

    savingFile = true;
    errorText = "";

    try {
      filePayload = await api.saveGitFile(selectedRepoPath, filePayload.filePath, editorValue);
      editorValue = filePayload.modifiedContent;
      await refreshStatus(selectedRepoPath);
    } catch (error) {
      errorText = describeError(error);
    } finally {
      savingFile = false;
    }
  }

  async function stage(filePath: string | null = null) {
    if (!selectedRepoPath || readOnly) {
      return;
    }

    gitBusy = true;
    errorText = "";

    try {
      status = await api.stageGitFile(selectedRepoPath, filePath);
    } catch (error) {
      errorText = describeError(error);
    } finally {
      gitBusy = false;
    }
  }

  async function unstage(filePath: string | null = null) {
    if (!selectedRepoPath || readOnly) {
      return;
    }

    gitBusy = true;
    errorText = "";

    try {
      status = await api.unstageGitFile(selectedRepoPath, filePath);
    } catch (error) {
      errorText = describeError(error);
    } finally {
      gitBusy = false;
    }
  }

  async function commit() {
    if (!selectedRepoPath || readOnly || !commitMessage.trim()) {
      return;
    }

    gitBusy = true;
    errorText = "";

    try {
      status = await api.commitGit(selectedRepoPath, commitMessage.trim());
      commitMessage = "";
    } catch (error) {
      errorText = describeError(error);
    } finally {
      gitBusy = false;
    }
  }

  async function switchBranch(branchName: string) {
    if (!selectedRepoPath || readOnly || !branchName) {
      return;
    }

    gitBusy = true;
    errorText = "";

    try {
      status = await api.checkoutGitBranch(selectedRepoPath, branchName);
    } catch (error) {
      errorText = describeError(error);
    } finally {
      gitBusy = false;
    }
  }

  async function createBranch() {
    if (!selectedRepoPath || readOnly || !newBranchName.trim()) {
      return;
    }

    gitBusy = true;
    errorText = "";

    try {
      status = await api.checkoutGitBranch(selectedRepoPath, newBranchName.trim(), true);
      newBranchName = "";
    } catch (error) {
      errorText = describeError(error);
    } finally {
      gitBusy = false;
    }
  }

  async function createWorktree() {
    if (!selectedRepoPath || readOnly || !newWorktreePath.trim()) {
      return;
    }

    worktreeBusy = true;
    errorText = "";

    try {
      const payload = await api.createGitWorktree(selectedRepoPath, {
        worktreePath: newWorktreePath.trim(),
        branchName: detachWorktree ? null : (newWorktreeBranch.trim() || null),
        createBranch: createWorktreeBranch && !detachWorktree,
        detach: detachWorktree
      });
      worktrees = payload.worktrees;
      newWorktreePath = "";
      newWorktreeBranch = "";
      createWorktreeBranch = true;
      detachWorktree = false;
      await bootstrap();
      await refreshStatus(selectedRepoPath);
    } catch (error) {
      errorText = describeError(error);
    } finally {
      worktreeBusy = false;
    }
  }

  async function removeWorktree(worktreePath: string) {
    if (!selectedRepoPath || readOnly) {
      return;
    }

    worktreeBusy = true;
    errorText = "";

    try {
      const payload = await api.removeGitWorktree(selectedRepoPath, worktreePath, true);
      worktrees = payload.worktrees;
      await bootstrap();
      await refreshStatus(selectedRepoPath);
    } catch (error) {
      errorText = describeError(error);
    } finally {
      worktreeBusy = false;
    }
  }

  function closeEditor() {
    clearDetailPanels();
    if (isMobileLayout) {
      mobileSection = "changes";
    }
  }

  function describeError(value: unknown) {
    const message = describeUiError(value);
    return message === m.unknown_error() ? ui.gitErrorGeneric : message;
  }

  async function fetchRepository() {
    if (!selectedRepoPath || readOnly) {
      return;
    }

    gitBusy = true;
    gitBusyAction = "fetch";
    errorText = "";

    try {
      status = await api.fetchGitRepository(selectedRepoPath);
    } catch (error) {
      errorText = describeError(error);
    } finally {
      gitBusy = false;
      gitBusyAction = null;
    }
  }

  async function pullRepository() {
    if (!selectedRepoPath || readOnly) {
      return;
    }

    gitBusy = true;
    gitBusyAction = "pull";
    errorText = "";

    try {
      status = await api.pullGitRepository(selectedRepoPath);
    } catch (error) {
      errorText = describeError(error);
    } finally {
      gitBusy = false;
      gitBusyAction = null;
    }
  }

  $effect(() => {
    const request = openRequest;
    if (!request || request.requestId === lastOpenRequestId) {
      return;
    }

    lastOpenRequestId = request.requestId;
    if (selectedRepoPath !== request.repoPath) {
      onSelectRepo(request.repoPath);
    }
    if (Array.isArray(request.filePaths) && request.filePaths.length > 0) {
      void openGroupedFilesByPath(request.repoPath, request.filePaths, request.title ?? null);
      return;
    }
    if (request.filePath) {
      void openFileByPath(request.repoPath, request.filePath);
    }
  });
</script>

<section class="git-shell surface">
  <div class="git-header">
    <div>
      <p class="eyebrow">{ui.git}</p>
      <h2>{ui.repository}</h2>
    </div>
    <button class="ghost-button" type="button" onclick={bootstrap}>{ui.refreshRepos}</button>
  </div>

  {#if errorText}
    <div class="error-banner small">{errorText}</div>
  {/if}

  <label class="field field--inline">
    <span>{ui.repository}</span>
    <select disabled={loadingRepos} onchange={(event) => void selectRepository((event.currentTarget as HTMLSelectElement).value)} value={selectedRepoPath ?? ""}>
      <option value="">{ui.selectedRepo}</option>
      {#each repositories as repository (repository.path)}
        <option value={repository.path}>{repository.relativePath} · {repository.currentBranch ?? ui.detached}</option>
      {/each}
    </select>
  </label>

  {#if !selectedRepoPath}
    <p class="field-note">{ui.repoSelectionNote}</p>
  {:else if loadingStatus}
    <div class="placeholder-card">{ui.loadingRepositoryStatus}</div>
  {:else if status}
    <div class="git-meta toolbar-row">
      <div class="meta-pill">{status.branch ?? ui.detachedHead}</div>
      <div class="meta-pill subtle">{m.ahead_behind({ ahead: String(status.ahead), behind: String(status.behind) })}</div>
      <div class="meta-pill subtle">{ui.worktrees}</div>
      <div class="git-actions git-actions--compact">
        <button class="ghost-button small" disabled={readOnly || gitBusy || loadingStatus} type="button" onclick={fetchRepository}>
          {gitBusyAction === "fetch" ? ui.fetching : ui.fetch}
        </button>
        <button class="ghost-button small" disabled={readOnly || gitBusy || loadingStatus} type="button" onclick={pullRepository}>
          {gitBusyAction === "pull" ? ui.pulling : ui.pull}
        </button>
        <button class="ghost-button small" disabled={readOnly || gitBusy} type="button" onclick={() => stage(null)}>{ui.stageAll}</button>
        <button class="ghost-button small" disabled={readOnly || gitBusy} type="button" onclick={() => unstage(null)}>{ui.unstageAll}</button>
        <button class="ghost-button small" type="button" onclick={() => refreshStatus(selectedRepoPath)}>{ui.refresh}</button>
      </div>
    </div>

    {#if isMobileLayout}
      <div class="git-mobile-nav" aria-label={ui.git}>
        <button class:git-mobile-nav__button--active={mobileSection === "repository"} class="git-mobile-nav__button" type="button" onclick={() => (mobileSection = "repository")}>{ui.repository}</button>
        <button class:git-mobile-nav__button--active={mobileSection === "worktrees"} class="git-mobile-nav__button" type="button" onclick={() => (mobileSection = "worktrees")}>{ui.worktrees}</button>
        <button class:git-mobile-nav__button--active={mobileSection === "changes"} class="git-mobile-nav__button" type="button" onclick={() => (mobileSection = "changes")}>{ui.changes}</button>
        <button class:git-mobile-nav__button--active={mobileSection === "commits"} class="git-mobile-nav__button" type="button" onclick={() => (mobileSection = "commits")}>{ui.recentCommits}</button>
        <button class:git-mobile-nav__button--active={mobileSection === "detail"} class="git-mobile-nav__button" disabled={!hasDetailPanel} type="button" onclick={() => (mobileSection = "detail")}>{ui.edit}</button>
      </div>
    {/if}

    {#if !isMobileLayout || mobileSection === "repository"}
      <div class="toolbar-row toolbar-row--fields">
        <label class="field field--inline field--grow">
          <span>{ui.worktreePath}</span>
          <input bind:value={newWorktreePath} disabled={readOnly} placeholder="/path/to/worktree" type="text" />
        </label>
        <label class="field field--inline field--grow">
          <span>{ui.worktreeBranch}</span>
          <input bind:value={newWorktreeBranch} disabled={readOnly || detachWorktree} placeholder={detachWorktree ? ui.detachedHeadShort : ui.branchOrNewBranch} type="text" />
        </label>
        <label class:checkbox-card--disabled={readOnly || detachWorktree} class="checkbox-card checkbox-card--compact">
          <input bind:checked={createWorktreeBranch} class="checkbox-input" disabled={readOnly || detachWorktree} type="checkbox" />
          <span aria-hidden="true" class="checkbox-control"></span>
          <span class="checkbox-copy">
            <span class="checkbox-title">{ui.createBranchOption}</span>
          </span>
        </label>
        <label class:checkbox-card--disabled={readOnly} class="checkbox-card checkbox-card--compact">
          <input bind:checked={detachWorktree} class="checkbox-input" disabled={readOnly} type="checkbox" />
          <span aria-hidden="true" class="checkbox-control"></span>
          <span class="checkbox-copy">
            <span class="checkbox-title">{ui.detachHeadOption}</span>
          </span>
        </label>
        <button class="solid-button" disabled={readOnly || !newWorktreePath.trim() || (!detachWorktree && !newWorktreeBranch.trim()) || worktreeBusy} type="button" onclick={createWorktree}>
          {worktreeBusy ? ui.creating : ui.addWorktree}
        </button>
      </div>

      <div class="toolbar-row toolbar-row--fields">
        <label class="field field--inline field--grow">
          <span>{ui.switchBranch}</span>
          <select disabled={readOnly} onchange={(event) => void switchBranch((event.currentTarget as HTMLSelectElement).value)} value={status.branch ?? ""}>
            <option value="">{ui.switchBranch}</option>
            {#each status.branches as branch (branch.name)}
              <option value={branch.name}>{branch.name}{branch.current ? ` · ${ui.current}` : ""}</option>
            {/each}
          </select>
        </label>

        <div class="inline-field inline-field--compact">
          <input bind:value={newBranchName} disabled={readOnly} placeholder={ui.newBranchName} type="text" />
          <button class="ghost-button" disabled={readOnly || !newBranchName.trim() || gitBusy} type="button" onclick={createBranch}>{ui.create}</button>
        </div>
      </div>

      <div class="toolbar-row toolbar-row--fields">
        <label class="field field--inline field--grow">
          <span>{ui.commit}</span>
          <input bind:value={commitMessage} disabled={readOnly} placeholder={ui.commitMessage} type="text" />
        </label>
        <button class="solid-button" disabled={readOnly || !commitMessage.trim() || gitBusy} type="button" onclick={commit}>{ui.commit}</button>
      </div>
    {/if}

    {#if !isMobileLayout || mobileSection === "worktrees"}
      <section class="panel">
        <div class="panel__header">
          <h3>{ui.worktrees}</h3>
          <span>{loadingWorktrees ? "..." : worktrees.length}</span>
        </div>

        {#if loadingWorktrees}
          <div class="placeholder-card">{ui.loadingWorktrees}</div>
        {:else if worktrees.length === 0}
          <p class="field-note">{ui.noLinkedWorktrees}</p>
        {:else}
          <div class="file-list">
            {#each worktrees as worktree (worktree.path)}
              <article class="file-row">
                <button class="file-link" type="button" onclick={() => void selectRepository(worktree.path)}>
                  <strong>{worktree.path}</strong>
                  <small>{worktree.branch ?? ui.detached}{worktree.current ? ` · ${ui.current}` : ""}</small>
                </button>
                <div class="file-actions">
                  <button class="ghost-button small" type="button" onclick={() => void selectRepository(worktree.path)}>{ui.open}</button>
                  {#if !worktree.current}
                    <button class="ghost-button small" disabled={readOnly || worktreeBusy} type="button" onclick={() => void removeWorktree(worktree.path)}>{ui.remove}</button>
                  {/if}
                </div>
              </article>
            {/each}
          </div>
        {/if}
      </section>
    {/if}

    <div class="panel-grid">
      {#if !isMobileLayout || mobileSection === "changes"}
        <section class="panel">
          <div class="panel__header">
            <h3>{ui.changes}</h3>
            <span>{status.files.length}</span>
          </div>

          {#if status.files.length === 0}
            <p class="field-note">{ui.workingTreeClean}</p>
          {:else}
            <div class="file-list">
              {#each status.files as file (file.path)}
                <article class="file-row">
                  <button class="file-link" type="button" onclick={() => openFile(file)}>
                    <strong>{file.path}</strong>
                    <small>{file.stagedLabel} / {file.unstagedLabel}</small>
                  </button>
                  <div class="file-actions">
                    <button class="ghost-button small" disabled={readOnly || gitBusy} type="button" onclick={() => stage(file.path)}>{ui.stage}</button>
                    <button class="ghost-button small" disabled={readOnly || gitBusy} type="button" onclick={() => unstage(file.path)}>{ui.unstage}</button>
                    {#if isMobileLayout && onOpenDiffTab && selectedRepoPath}
                      <button class="ghost-button small" type="button" onclick={() => onOpenDiffTab(selectedRepoPath, file.path)}>{ui.openTab}</button>
                    {/if}
                  </div>
                </article>
              {/each}
            </div>
          {/if}
        </section>
      {/if}

      {#if !isMobileLayout || mobileSection === "commits" || mobileSection === "detail"}
        <section class="panel panel--detail">
        {#if loadingFile}
          <div class="placeholder-card">{ui.loadingFile}</div>
        {:else if groupedFilePayloads.length > 0 && (!isMobileLayout || mobileSection === "detail")}
          <div class="panel__header">
            <div>
              <h3>{groupedFileTitle || `${groupedFilePayloads.length} files changed`}</h3>
              <span>{groupedFilePayloads.length} files</span>
            </div>
            <div class="git-actions">
              <button class="ghost-button" type="button" onclick={closeEditor}>{ui.close}</button>
            </div>
          </div>

          <div class="grouped-diff-list">
            {#each groupedFilePayloads as groupedFile (groupedFile.filePath)}
              <section class="panel grouped-diff-panel">
                <div class="panel__header">
                  <div>
                    <h3>{groupedFile.filePath}</h3>
                    <span>{groupedFile.status?.stagedLabel ?? "clean"} / {groupedFile.status?.unstagedLabel ?? "clean"}</span>
                  </div>
                  <div class="git-actions">
                    <button class="ghost-button" disabled={readOnly || gitBusy} type="button" onclick={() => groupedFile && stage(groupedFile.filePath)}>{ui.stage}</button>
                    <button class="ghost-button" disabled={readOnly || gitBusy} type="button" onclick={() => groupedFile && unstage(groupedFile.filePath)}>{ui.unstage}</button>
                    {#if onOpenDiffTab}
                      <button class="ghost-button" type="button" onclick={() => onOpenDiffTab(groupedFile.repoPath, groupedFile.filePath)}>{ui.singleTab}</button>
                    {/if}
                  </div>
                </div>

                {#if groupedFile.isBinary}
                  <div class="placeholder-card">{ui.binaryDiffNotPreviewable}</div>
                {:else}
                <MonacoDiffEditor
                  height={320}
                  modified={groupedFile.modifiedContent}
                    original={groupedFile.originalContent}
                    path={groupedFile.filePath}
                  />
                {/if}
              </section>
            {/each}
          </div>
        {:else if filePayload && (!isMobileLayout || mobileSection === "detail")}
          <div class="panel__header">
            <div>
              <h3>{filePayload.filePath}</h3>
              <span>{filePayload.status?.stagedLabel ?? "clean"} / {filePayload.status?.unstagedLabel ?? "clean"}</span>
            </div>
            <div class="git-actions">
              <button class="ghost-button" disabled={readOnly || gitBusy} type="button" onclick={() => filePayload && stage(filePayload.filePath)}>{ui.stage}</button>
              <button class="ghost-button" disabled={readOnly || gitBusy} type="button" onclick={() => filePayload && unstage(filePayload.filePath)}>{ui.unstage}</button>
              <button class="ghost-button" type="button" onclick={closeEditor}>{ui.close}</button>
            </div>
          </div>

          {#if filePayload.isBinary}
            <div class="placeholder-card">{ui.binaryDiffNotPreviewable}</div>
          {:else}
            <div class="editor-stack">
              <section class="panel">
                <div class="panel__header">
                  <h3>{ui.diff}</h3>
                  <span>{filePayload.status?.stagedLabel ?? "clean"} / {filePayload.status?.unstagedLabel ?? "clean"}</span>
                </div>
                <MonacoDiffEditor height={340} modified={editorValue} original={filePayload.originalContent} path={filePayload.filePath} />
              </section>

              <section class="panel">
                <div class="panel__header">
                  <h3>{ui.edit}</h3>
                  <button class="solid-button" disabled={readOnly || savingFile} type="button" onclick={saveFile}>
                    {savingFile ? ui.saving : ui.saveFile}
                  </button>
                </div>
                <MonacoTextEditor bind:value={editorValue} height={340} path={filePayload.filePath} readonly={readOnly} />
              </section>
            </div>
          {/if}
        {:else if isMobileLayout && mobileSection === "detail"}
          <div class="placeholder-card">{ui.loadingFile}</div>
        {:else}
          <div class="panel__header">
            <h3>{ui.recentCommits}</h3>
            <span>{status.commits.length}</span>
          </div>

          <div class="commit-list">
            {#each status.commits as commit (commit.hash)}
              <article class="commit-row">
                <button class="commit-link" disabled={openingCommitHash === commit.hash} type="button" onclick={() => void openCommit(commit)}>
                  <strong>{commit.shortHash}</strong>
                  <p>{commit.subject}</p>
                  <small>{commit.author} · {new Date(commit.authoredAt).toLocaleString(getDateLocale())}</small>
                </button>
              </article>
            {/each}
          </div>
        {/if}
        </section>
      {/if}
    </div>
  {/if}
</section>

<style>
  .git-shell {
    display: grid;
    gap: 0.8rem;
    min-height: 0;
    overflow: auto;
    padding: 0.9rem;
    background: var(--panel-strong);
  }

  .git-header,
  .git-meta,
  .git-actions,
  .inline-field {
    display: flex;
    gap: 0.75rem;
    align-items: center;
    justify-content: space-between;
  }

  .toolbar-row {
    display: flex;
    gap: 0.55rem;
    align-items: center;
    flex-wrap: wrap;
  }

  .toolbar-row--fields {
    align-items: stretch;
  }

  .git-header h2,
  .panel__header h3 {
    margin: 0.15rem 0 0;
    color: var(--ink-strong);
    font: 600 1.2rem/1.1 var(--font-display);
  }

  .field {
    display: grid;
    gap: 0.55rem;
  }

  .field--inline {
    display: grid;
    grid-template-columns: minmax(7rem, auto) minmax(0, 1fr);
    align-items: center;
    column-gap: 0.8rem;
    row-gap: 0.4rem;
  }

  .field--grow {
    flex: 1 1 28rem;
  }

  .field span {
    color: var(--muted);
    font-size: 0.82rem;
    white-space: nowrap;
  }

  .field select,
  .field input,
  .inline-field input {
    width: 100%;
    border: 1px solid rgba(83, 61, 42, 0.14);
    border-radius: 1rem;
    background: rgba(255, 255, 255, 0.86);
    color: var(--ink);
    padding: 0.68rem 0.82rem;
  }

  .meta-pill {
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.82);
    color: var(--ink);
    padding: 0.32rem 0.65rem;
    font-size: 0.74rem;
  }

  .meta-pill.subtle {
    color: var(--muted);
  }

  .panel-grid,
  .editor-stack,
  .grouped-diff-list {
    display: grid;
    gap: 0.8rem;
  }

  .panel-grid {
    grid-template-columns: minmax(20rem, 0.92fr) minmax(0, 1.45fr);
  }

  .git-mobile-nav {
    display: none;
  }

  .panel {
    display: grid;
    gap: 0.7rem;
    border: 1px solid rgba(83, 61, 42, 0.1);
    border-radius: 1.15rem;
    background: rgba(255, 255, 255, 0.76);
    padding: 0.88rem;
  }

  .panel--detail {
    min-height: 0;
    align-content: start;
  }

  .grouped-diff-panel {
    padding: 0.76rem;
  }

  .panel__header {
    display: flex;
    gap: 0.75rem;
    align-items: center;
    justify-content: space-between;
  }

  .file-list,
  .commit-list {
    display: grid;
    gap: 0.55rem;
    max-height: 20rem;
    overflow: auto;
  }

  .file-row,
  .commit-row {
    display: flex;
    gap: 0.55rem;
    align-items: center;
    border-radius: 1rem;
    background: rgba(249, 245, 239, 0.75);
    padding: 0.62rem 0.72rem;
  }

  .file-link {
    display: flex;
    gap: 0.75rem;
    align-items: center;
    min-width: 0;
    flex: 1 1 auto;
    border: 0;
    background: transparent;
    cursor: pointer;
    padding: 0;
    text-align: left;
  }

  .commit-link {
    width: 100%;
    display: grid;
    gap: 0.2rem;
    min-width: 0;
    border: 0;
    background: transparent;
    color: inherit;
    cursor: pointer;
    padding: 0;
    text-align: left;
  }

  .commit-link:disabled {
    cursor: progress;
    opacity: 0.6;
  }

  .file-link strong,
  .commit-row strong {
    color: var(--ink-strong);
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .file-link small,
  .commit-row small {
    color: var(--muted);
    flex: 0 0 auto;
    white-space: nowrap;
  }

  .file-actions {
    display: flex;
    gap: 0.4rem;
    flex-wrap: wrap;
    justify-content: flex-end;
    flex: 0 0 auto;
  }

  .git-actions--compact {
    flex: 1 1 auto;
    justify-content: flex-end;
  }

  .commit-row p {
    min-width: 0;
    margin: 0;
    flex: 1 1 auto;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .small {
    padding: 0.34rem 0.62rem;
  }

  .error-banner.small {
    margin: 0;
  }

  @media (max-width: 1120px) {
    .panel-grid {
      grid-template-columns: 1fr;
    }
  }

  @media (max-width: 720px) {
    .git-mobile-nav {
      display: flex;
      gap: 0.5rem;
      overflow-x: auto;
      padding-bottom: 0.2rem;
      margin: -0.1rem 0 0.1rem;
      scrollbar-width: none;
    }

    .git-mobile-nav::-webkit-scrollbar {
      display: none;
    }

    .git-mobile-nav__button {
      border: 1px solid rgba(83, 61, 42, 0.12);
      border-radius: 999px;
      background: rgba(255, 255, 255, 0.78);
      color: var(--muted);
      padding: 0.55rem 0.9rem;
      font-size: 0.76rem;
      font-weight: 700;
      white-space: nowrap;
      transition: color 140ms ease, background-color 140ms ease, border-color 140ms ease;
    }

    .git-mobile-nav__button--active {
      background: rgba(255, 248, 237, 0.96);
      border-color: rgba(214, 140, 69, 0.28);
      color: var(--ink-strong);
    }

    .git-mobile-nav__button:disabled {
      opacity: 0.45;
    }

    .git-header,
    .git-meta,
    .inline-field,
    .toolbar-row,
    .file-row,
    .commit-row,
    .file-link,
    .commit-link {
      flex-direction: column;
      align-items: stretch;
    }

    .field--inline {
      grid-template-columns: 1fr;
    }

    .file-link small,
    .commit-row small,
    .commit-row p,
    .file-link strong {
      white-space: normal;
      overflow: visible;
      text-overflow: clip;
    }

    .panel,
    .git-shell {
      padding: 0.85rem;
    }
  }
</style>
