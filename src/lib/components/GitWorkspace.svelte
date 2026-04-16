<script lang="ts">
  import { onMount } from "svelte";

  import { api } from "$lib/api";
  import MonacoDiffEditor from "$lib/components/MonacoDiffEditor.svelte";
  import MonacoTextEditor from "$lib/components/MonacoTextEditor.svelte";
  import { localeSignal } from "$lib/i18n";
  import { m } from "$lib/paraglide/messages.js";
  import { getLocale } from "$lib/paraglide/runtime.js";
  import type { GitCommit, GitFilePayload, GitFileStatus, GitOpenRequest, GitRepository, GitStatusPayload, GitWorktree } from "$lib/types";

  let {
    selectedRepoPath,
    onSelectRepo,
    openRequest = null,
    onOpenDiffTab = null,
    onOpenCommitDiff = null
  }: {
    selectedRepoPath: string | null;
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
  let worktreeBusy = $state(false);
  let openingCommitHash = $state<string | null>(null);
  let errorText = $state("");
  let lastRepoPath: string | null = null;
  let lastOpenRequestId = $state<number | null>(null);
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
      gitErrorGeneric: m.git_error_generic()
    };
  });

  function getDateLocale() {
    return getLocale() === "ko" ? "ko-KR" : "en-US";
  }

  onMount(() => {
    void bootstrap();
  });

  $effect(() => {
    const repoPath = selectedRepoPath;
    if (repoPath === lastRepoPath) {
      return;
    }

    lastRepoPath = repoPath;
    clearDetailPanels();

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
    if (selectedRepoPath && onOpenDiffTab) {
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
    } catch (error) {
      errorText = describeError(error);
    } finally {
      loadingFile = false;
    }
  }

  async function saveFile() {
    if (!selectedRepoPath || !filePayload || filePayload.isBinary) {
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
    if (!selectedRepoPath) {
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
    if (!selectedRepoPath) {
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
    if (!selectedRepoPath || !commitMessage.trim()) {
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
    if (!selectedRepoPath || !branchName) {
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
    if (!selectedRepoPath || !newBranchName.trim()) {
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
    if (!selectedRepoPath || !newWorktreePath.trim()) {
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
    if (!selectedRepoPath) {
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
  }

  function describeError(value: unknown) {
    if (value instanceof Error) {
      return value.message;
    }
    return ui.gitErrorGeneric;
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
      <button class="ghost-button" type="button" onclick={() => refreshStatus(selectedRepoPath)}>{ui.refresh}</button>
    </div>

    <div class="toolbar-row toolbar-row--fields">
      <label class="field field--inline field--grow">
        <span>{ui.worktreePath}</span>
        <input bind:value={newWorktreePath} placeholder="/path/to/worktree" type="text" />
      </label>
      <label class="field field--inline field--grow">
        <span>{ui.worktreeBranch}</span>
        <input bind:value={newWorktreeBranch} disabled={detachWorktree} placeholder={detachWorktree ? ui.detachedHeadShort : ui.branchOrNewBranch} type="text" />
      </label>
      <label class:checkbox-card--disabled={detachWorktree} class="checkbox-card checkbox-card--compact">
        <input bind:checked={createWorktreeBranch} class="checkbox-input" disabled={detachWorktree} type="checkbox" />
        <span aria-hidden="true" class="checkbox-control"></span>
        <span class="checkbox-copy">
          <span class="checkbox-title">{ui.createBranchOption}</span>
        </span>
      </label>
      <label class="checkbox-card checkbox-card--compact">
        <input bind:checked={detachWorktree} class="checkbox-input" type="checkbox" />
        <span aria-hidden="true" class="checkbox-control"></span>
        <span class="checkbox-copy">
          <span class="checkbox-title">{ui.detachHeadOption}</span>
        </span>
      </label>
      <button class="solid-button" disabled={!newWorktreePath.trim() || (!detachWorktree && !newWorktreeBranch.trim()) || worktreeBusy} type="button" onclick={createWorktree}>
        {worktreeBusy ? ui.creating : ui.addWorktree}
      </button>
    </div>

    <div class="git-actions toolbar-row">
      <button class="ghost-button" disabled={gitBusy} type="button" onclick={() => stage(null)}>{ui.stageAll}</button>
      <button class="ghost-button" disabled={gitBusy} type="button" onclick={() => unstage(null)}>{ui.unstageAll}</button>
    </div>

    <div class="toolbar-row toolbar-row--fields">
      <label class="field field--inline field--grow">
        <span>{ui.switchBranch}</span>
        <select onchange={(event) => void switchBranch((event.currentTarget as HTMLSelectElement).value)} value={status.branch ?? ""}>
          <option value="">{ui.switchBranch}</option>
          {#each status.branches as branch (branch.name)}
            <option value={branch.name}>{branch.name}{branch.current ? ` · ${ui.current}` : ""}</option>
          {/each}
        </select>
      </label>

      <div class="inline-field inline-field--compact">
        <input bind:value={newBranchName} placeholder={ui.newBranchName} type="text" />
        <button class="ghost-button" disabled={!newBranchName.trim() || gitBusy} type="button" onclick={createBranch}>{ui.create}</button>
      </div>
    </div>

    <div class="toolbar-row toolbar-row--fields">
      <label class="field field--inline field--grow">
        <span>{ui.commit}</span>
        <input bind:value={commitMessage} placeholder={ui.commitMessage} type="text" />
      </label>
      <button class="solid-button" disabled={!commitMessage.trim() || gitBusy} type="button" onclick={commit}>{ui.commit}</button>
    </div>

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
                  <button class="ghost-button small" disabled={worktreeBusy} type="button" onclick={() => void removeWorktree(worktree.path)}>{ui.remove}</button>
                {/if}
              </div>
            </article>
          {/each}
        </div>
      {/if}
    </section>

    <div class="panel-grid">
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
                  <button class="ghost-button small" type="button" onclick={() => stage(file.path)}>{ui.stage}</button>
                  <button class="ghost-button small" type="button" onclick={() => unstage(file.path)}>{ui.unstage}</button>
                </div>
              </article>
            {/each}
          </div>
        {/if}
      </section>

      <section class="panel panel--detail">
        {#if loadingFile}
          <div class="placeholder-card">{ui.loadingFile}</div>
        {:else if groupedFilePayloads.length > 0}
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
                    <button class="ghost-button" type="button" onclick={() => groupedFile && stage(groupedFile.filePath)}>{ui.stage}</button>
                    <button class="ghost-button" type="button" onclick={() => groupedFile && unstage(groupedFile.filePath)}>{ui.unstage}</button>
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
        {:else if filePayload}
          <div class="panel__header">
            <div>
              <h3>{filePayload.filePath}</h3>
              <span>{filePayload.status?.stagedLabel ?? "clean"} / {filePayload.status?.unstagedLabel ?? "clean"}</span>
            </div>
            <div class="git-actions">
              <button class="ghost-button" type="button" onclick={() => filePayload && stage(filePayload.filePath)}>{ui.stage}</button>
              <button class="ghost-button" type="button" onclick={() => filePayload && unstage(filePayload.filePath)}>{ui.unstage}</button>
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
                  <button class="solid-button" disabled={savingFile} type="button" onclick={saveFile}>
                    {savingFile ? ui.saving : ui.saveFile}
                  </button>
                </div>
                <MonacoTextEditor bind:value={editorValue} height={340} path={filePayload.filePath} />
              </section>
            </div>
          {/if}
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
    </div>
  {/if}
</section>

<style>
  .git-shell {
    display: grid;
    gap: 1rem;
    min-height: 0;
    overflow: auto;
    padding: 1rem;
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
    gap: 0.75rem;
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
    padding: 0.85rem 0.95rem;
  }

  .meta-pill {
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.82);
    color: var(--ink);
    padding: 0.45rem 0.8rem;
    font-size: 0.8rem;
  }

  .meta-pill.subtle {
    color: var(--muted);
  }

  .panel-grid,
  .editor-stack,
  .grouped-diff-list {
    display: grid;
    gap: 1rem;
  }

  .panel-grid {
    grid-template-columns: minmax(20rem, 0.92fr) minmax(0, 1.45fr);
  }

  .panel {
    display: grid;
    gap: 0.8rem;
    border: 1px solid rgba(83, 61, 42, 0.1);
    border-radius: 1.15rem;
    background: rgba(255, 255, 255, 0.76);
    padding: 1rem;
  }

  .panel--detail {
    min-height: 0;
    align-content: start;
  }

  .grouped-diff-panel {
    padding: 0.85rem;
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
    gap: 0.75rem;
    max-height: 20rem;
    overflow: auto;
  }

  .file-row,
  .commit-row {
    display: flex;
    gap: 0.75rem;
    align-items: center;
    border-radius: 1rem;
    background: rgba(249, 245, 239, 0.75);
    padding: 0.8rem;
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
    gap: 0.5rem;
    flex-wrap: wrap;
    justify-content: flex-end;
    flex: 0 0 auto;
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
    padding: 0.45rem 0.8rem;
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
    .git-header,
    .git-meta,
    .git-actions,
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
