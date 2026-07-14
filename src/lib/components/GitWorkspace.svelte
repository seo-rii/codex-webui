<script lang="ts">
  import { onMount } from "svelte";
  import { ChevronDown, ChevronRight, ExternalLink, FileText, Folder, FolderOpen, GitPullRequest, RefreshCw } from "lucide-svelte";

  import { api } from "$lib/api";
  import MarkdownMessage from "$lib/components/MarkdownMessage.svelte";
  import LazyMonacoDiffEditor from "$lib/components/LazyMonacoDiffEditor.svelte";
  import MonacoTextEditor from "$lib/components/MonacoTextEditor.svelte";
  import { localeSignal } from "$lib/i18n";
  import { m } from "$lib/paraglide/messages.js";
  import { getLocale } from "$lib/paraglide/runtime.js";
  import { describeUiError } from "$lib/ui-errors";
  import type {
    GitCommit,
    GitFilePayload,
    GitFileStatus,
    GitHubPullRequestDetailPayload,
    GitHubPullRequestSummary,
    GitHubRepositoryInfo,
    GitOpenRequest,
    GitRepository,
    GitStatusPayload,
    GitWorktree
  } from "$lib/types";

  type ChangeSectionId = "staged" | "changes";
  type GitChangeEntry = {
    key: string;
    sectionId: ChangeSectionId;
    file: GitFileStatus;
    path: string;
    fileName: string;
    directoryPath: string;
    originalPath: string | null;
    statusCode: string;
    statusLabel: string;
  };
  type GitChangeVisibleRow =
    | {
        type: "folder";
        key: string;
        depth: number;
        name: string;
        count: number;
      }
    | {
        type: "file";
        key: string;
        depth: number;
        entry: GitChangeEntry;
      };
  type GitChangeTreeNode =
    | {
        type: "folder";
        key: string;
        name: string;
        path: string;
        depth: number;
        count: number;
        children: GitChangeTreeNode[];
      }
    | {
        type: "file";
        key: string;
        depth: number;
        entry: GitChangeEntry;
      };

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
  let loadingPullRequests = $state(false);
  let loadingPullRequestDetail = $state(false);
  let savingFile = $state(false);
  let gitBusy = $state(false);
  let gitBusyAction = $state<"fetch" | "pull" | null>(null);
  let githubBusyAction = $state<"checkout" | null>(null);
  let githubBusyPullRequestNumber = $state<number | null>(null);
  let worktreeBusy = $state(false);
  let openingCommitHash = $state<string | null>(null);
  let errorText = $state("");
  let githubErrorText = $state("");
  let lastRepoPath: string | null = null;
  let lastOpenRequestId = $state<number | null>(null);
  let lastPullRequestQueryKey: string | null = null;
  let isMobileLayout = $state(false);
  let mobileSection = $state<"repository" | "worktrees" | "changes" | "pulls" | "commits" | "detail">("repository");
  let githubRepo = $state<GitHubRepositoryInfo | null>(null);
  let pullRequests = $state<GitHubPullRequestSummary[]>([]);
  let pullRequestState = $state<"open" | "closed" | "all">("open");
  let pullRequestDetail = $state<GitHubPullRequestDetailPayload | null>(null);
  let selectedPullRequestFilePath = $state<string | null>(null);
  let changeViewMode = $state<"tree" | "list">("tree");
  let collapsedChangeSections = $state<Record<ChangeSectionId, boolean>>({
    staged: false,
    changes: false
  });
  let collapsedChangeFolders = $state<Record<string, boolean>>({});
  const hasDetailPanel = $derived(groupedFilePayloads.length > 0 || Boolean(filePayload) || Boolean(pullRequestDetail));
  const selectedPullRequestFile = $derived.by(
    () => pullRequestDetail?.pullRequest.files.find((file) => file.path === selectedPullRequestFilePath) ?? null
  );
  const ui = $derived.by(() => {
    const _locale = $localeSignal;
    const locale = getLocale();

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
      gitErrorGeneric: m.git_error_generic(),
      pullRequests: m.pull_requests(),
      pullRequestsLoading: m.loading_pull_requests(),
      noPullRequests: m.no_pull_requests(),
      pullRequestsOpen: m.pull_requests_open(),
      pullRequestsClosed: m.pull_requests_closed(),
      pullRequestsAll: m.pull_requests_all(),
      checkoutPr: m.checkout_pr(),
      checkingOutPr: m.checking_out_pr(),
      openOnGitHub: m.open_on_github(),
      pullRequestOverview: m.pull_request_overview(),
      pullRequestFiles: m.pull_request_files(),
      pullRequestBodyEmpty: m.pull_request_body_empty(),
      pullRequestFilesTruncated:
        locale === "ko"
          ? "GitHub API 페이지 제한으로 일부 파일만 표시됩니다."
          : "Only part of this file list is shown because the GitHub API page cap was reached.",
      sourceControl: locale === "ko" ? "소스 제어" : "Source Control",
      stagedChanges: locale === "ko" ? "스테이징된 변경 사항" : "Staged Changes",
      workingChanges: locale === "ko" ? "변경 사항" : "Changes",
      treeView: locale === "ko" ? "트리" : "Tree",
      listView: locale === "ko" ? "목록" : "List",
      viewAs: locale === "ko" ? "보기" : "View",
      noStagedChanges: locale === "ko" ? "스테이징된 변경 사항이 없습니다." : "No staged changes.",
      noWorkingChanges: locale === "ko" ? "작업 트리 변경 사항이 없습니다." : "No working tree changes.",
      fileTree: locale === "ko" ? "파일 트리" : "File tree"
    };
  });

  function buildChangeEntries(files: GitFileStatus[], sectionId: ChangeSectionId) {
    const entries: GitChangeEntry[] = [];

    for (const file of files) {
      const include = sectionId === "staged" ? file.hasStagedChanges : file.hasUnstagedChanges || file.isUntracked;
      if (!include) {
        continue;
      }

      const segments = file.path.split("/").filter((segment) => segment.length > 0);
      const fileName = segments.at(-1) ?? file.path;
      const directoryPath = segments.length > 1 ? segments.slice(0, -1).join("/") : "";
      const rawStatusCode = (sectionId === "staged" ? file.stagedCode : file.unstagedCode).trim();
      const normalizedStatusCode = rawStatusCode.replace(/\?/g, "U");

      entries.push({
        key: `${sectionId}:${file.path}`,
        sectionId,
        file,
        path: file.path,
        fileName,
        directoryPath,
        originalPath: file.originalPath ?? null,
        statusCode: normalizedStatusCode || "M",
        statusLabel: sectionId === "staged" ? file.stagedLabel : file.unstagedLabel
      });
    }

    return entries.sort((left, right) => left.path.localeCompare(right.path));
  }

  function buildChangeTreeNodes(
    entries: GitChangeEntry[],
    sectionId: ChangeSectionId,
    parentPath = "",
    depth = 0
  ): GitChangeTreeNode[] {
    const folders = new Map<string, GitChangeEntry[]>();
    const fileNodes: GitChangeTreeNode[] = [];

    for (const entry of entries) {
      const relativePath = parentPath ? entry.path.slice(parentPath.length + 1) : entry.path;
      const segments = relativePath.split("/").filter((segment) => segment.length > 0);
      const firstSegment = segments[0] ?? "";
      if (!firstSegment) {
        continue;
      }

      if (segments.length === 1) {
        fileNodes.push({
          type: "file",
          key: entry.key,
          depth,
          entry
        });
        continue;
      }

      const bucket = folders.get(firstSegment) ?? [];
      bucket.push(entry);
      folders.set(firstSegment, bucket);
    }

    const folderNodes: GitChangeTreeNode[] = [...folders.entries()]
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([name, childEntries]) => {
        const folderPath = parentPath ? `${parentPath}/${name}` : name;
        return {
          type: "folder",
          key: `${sectionId}:folder:${folderPath}`,
          name,
          path: folderPath,
          depth,
          count: childEntries.length,
          children: buildChangeTreeNodes(childEntries, sectionId, folderPath, depth + 1)
        } satisfies GitChangeTreeNode;
      });

    fileNodes.sort((left, right) => {
      if (left.type !== "file" || right.type !== "file") {
        return 0;
      }
      return left.entry.path.localeCompare(right.entry.path);
    });

    return [...folderNodes, ...fileNodes];
  }

  const stagedChangeEntries = $derived.by(() => buildChangeEntries(status?.files ?? [], "staged"));
  const workingChangeEntries = $derived.by(() => buildChangeEntries(status?.files ?? [], "changes"));
  const changeSections = $derived.by(() => [
    {
      id: "staged" as const,
      label: ui.stagedChanges,
      entries: stagedChangeEntries,
      tree: buildChangeTreeNodes(stagedChangeEntries, "staged"),
      emptyLabel: ui.noStagedChanges
    },
    {
      id: "changes" as const,
      label: ui.workingChanges,
      entries: workingChangeEntries,
      tree: buildChangeTreeNodes(workingChangeEntries, "changes"),
      emptyLabel: ui.noWorkingChanges
    }
  ]);

  function flattenChangeTree(nodes: GitChangeTreeNode[]): GitChangeVisibleRow[] {
    const rows: GitChangeVisibleRow[] = [];

    const visit = (currentNodes: GitChangeTreeNode[]) => {
      for (const node of currentNodes) {
        if (node.type === "folder") {
          rows.push({
            type: "folder",
            key: node.key,
            depth: node.depth,
            name: node.name,
            count: node.count
          });

          if (isChangeFolderExpanded(node.key)) {
            visit(node.children);
          }
          continue;
        }

        rows.push({
          type: "file",
          key: node.key,
          depth: node.depth,
          entry: node.entry
        });
      }
    };

    visit(nodes);
    return rows;
  }

  function getChangeStatusTone(statusCode: string) {
    const code = statusCode.trim().toUpperCase();
    if (code.includes("D")) {
      return "deleted";
    }
    if (code.includes("A") || code.includes("U") || code.includes("?")) {
      return "added";
    }
    if (code.includes("R")) {
      return "renamed";
    }
    return "modified";
  }

  function getChangeSecondaryLabel(entry: GitChangeEntry) {
    if (entry.originalPath) {
      return entry.originalPath;
    }

    return changeViewMode === "list" ? entry.directoryPath : "";
  }

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
      pullRequests = [];
      githubRepo = null;
      pullRequestDetail = null;
      selectedPullRequestFilePath = null;
      githubErrorText = "";
      lastPullRequestQueryKey = null;
      return;
    }

    void refreshStatus(repoPath);
  });

  $effect(() => {
    if (!selectedRepoPath) {
      return;
    }

    const key = `${selectedRepoPath}:${pullRequestState}`;
    if (lastPullRequestQueryKey === key) {
      return;
    }

    lastPullRequestQueryKey = key;
    void refreshPullRequests(selectedRepoPath);
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

  async function refreshPullRequests(repoPath: string) {
    loadingPullRequests = true;
    githubErrorText = "";
    pullRequestDetail = null;
    selectedPullRequestFilePath = null;

    try {
      const payload = await api.listGitHubPullRequests(repoPath, pullRequestState, 20);
      githubRepo = payload.repository;
      pullRequests = payload.pullRequests;
    } catch (error) {
      githubRepo = null;
      pullRequests = [];
      githubErrorText = describeError(error);
    } finally {
      loadingPullRequests = false;
    }
  }

  async function selectRepository(repoPath: string) {
    onSelectRepo(repoPath || null);
    status = null;
    worktrees = [];
    filePayload = null;
  }

  function toggleChangeSection(sectionId: ChangeSectionId) {
    collapsedChangeSections = {
      ...collapsedChangeSections,
      [sectionId]: !collapsedChangeSections[sectionId]
    };
  }

  function isChangeSectionExpanded(sectionId: ChangeSectionId) {
    return !collapsedChangeSections[sectionId];
  }

  function toggleChangeFolder(nodeKey: string) {
    collapsedChangeFolders = {
      ...collapsedChangeFolders,
      [nodeKey]: !collapsedChangeFolders[nodeKey]
    };
  }

  function isChangeFolderExpanded(nodeKey: string) {
    return !collapsedChangeFolders[nodeKey];
  }

  function isActiveChangeEntry(entry: GitChangeEntry) {
    return filePayload?.filePath === entry.path && groupedFilePayloads.length === 0;
  }

  async function applyChangeEntry(entry: GitChangeEntry) {
    if (entry.sectionId === "staged") {
      await unstage(entry.path);
      return;
    }

    await stage(entry.path);
  }

  async function openChangeEntry(entry: GitChangeEntry, openInTab = false) {
    if (openInTab && selectedRepoPath && onOpenDiffTab) {
      onOpenDiffTab(selectedRepoPath, entry.path);
      return;
    }

    await openFileByPath(selectedRepoPath, entry.path);
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

  async function openPullRequest(pullRequestNumber: number) {
    if (!selectedRepoPath) {
      return;
    }

    loadingPullRequestDetail = true;
    githubErrorText = "";

    try {
      pullRequestDetail = await api.getGitHubPullRequest(selectedRepoPath, pullRequestNumber);
      selectedPullRequestFilePath = pullRequestDetail.pullRequest.files[0]?.path ?? null;
      mobileSection = "detail";
    } catch (error) {
      githubErrorText = describeError(error);
    } finally {
      loadingPullRequestDetail = false;
    }
  }

  async function checkoutPullRequest(pullRequestNumber: number) {
    if (!selectedRepoPath || readOnly) {
      return;
    }

    githubBusyAction = "checkout";
    githubBusyPullRequestNumber = pullRequestNumber;
    githubErrorText = "";

    try {
      status = await api.checkoutGitHubPullRequest(selectedRepoPath, pullRequestNumber);
      await bootstrap();
      await refreshPullRequests(selectedRepoPath);
      if (pullRequestDetail?.pullRequest.number === pullRequestNumber) {
        await openPullRequest(pullRequestNumber);
      }
    } catch (error) {
      githubErrorText = describeError(error);
    } finally {
      githubBusyAction = null;
      githubBusyPullRequestNumber = null;
    }
  }

  function clearDetailPanels() {
    filePayload = null;
    groupedFilePayloads = [];
    groupedFileTitle = "";
    editorValue = "";
    pullRequestDetail = null;
    selectedPullRequestFilePath = null;
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
    const nextMobileSection = pullRequestDetail ? "pulls" : "changes";
    clearDetailPanels();
    if (isMobileLayout) {
      mobileSection = nextMobileSection;
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
      await refreshPullRequests(selectedRepoPath);
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
      await refreshPullRequests(selectedRepoPath);
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
        <button class="ghost-button small" type="button" onclick={() => refreshStatus(selectedRepoPath)}>{ui.refresh}</button>
      </div>
    </div>

    {#if isMobileLayout}
      <div class="git-mobile-nav" aria-label={ui.git}>
        <button class:git-mobile-nav__button--active={mobileSection === "repository"} class="git-mobile-nav__button" type="button" onclick={() => (mobileSection = "repository")}>{ui.repository}</button>
        <button class:git-mobile-nav__button--active={mobileSection === "worktrees"} class="git-mobile-nav__button" type="button" onclick={() => (mobileSection = "worktrees")}>{ui.worktrees}</button>
        <button class:git-mobile-nav__button--active={mobileSection === "changes"} class="git-mobile-nav__button" type="button" onclick={() => (mobileSection = "changes")}>{ui.changes}</button>
        <button class:git-mobile-nav__button--active={mobileSection === "pulls"} class="git-mobile-nav__button" type="button" onclick={() => (mobileSection = "pulls")}>{ui.pullRequests}</button>
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
        <section class="panel panel--scm">
          <div class="panel__header">
            <div class="panel__title">
              <h3>{ui.sourceControl}</h3>
              <span>{status.files.length} files</span>
            </div>
            <div class="scm-toolbar">
              <div class="segmented-control" aria-label={ui.viewAs}>
                <button
                  class:segmented-control__button--active={changeViewMode === "tree"}
                  class="segmented-control__button"
                  type="button"
                  onclick={() => (changeViewMode = "tree")}
                >
                  <Folder size={13} />
                  <span>{ui.treeView}</span>
                </button>
                <button
                  class:segmented-control__button--active={changeViewMode === "list"}
                  class="segmented-control__button"
                  type="button"
                  onclick={() => (changeViewMode = "list")}
                >
                  <FileText size={13} />
                  <span>{ui.listView}</span>
                </button>
              </div>
            </div>
          </div>

          <div class="scm-commit-box">
            <textarea
              bind:value={commitMessage}
              class="scm-commit-box__input"
              disabled={readOnly}
              placeholder={ui.commitMessage}
              rows="3"
            ></textarea>
            <div class="scm-commit-box__actions">
              <div class="git-actions git-actions--compact">
                <button class="ghost-button small" disabled={readOnly || gitBusy} type="button" onclick={() => stage(null)}>{ui.stageAll}</button>
                <button class="ghost-button small" disabled={readOnly || gitBusy} type="button" onclick={() => unstage(null)}>{ui.unstageAll}</button>
              </div>
              <button class="solid-button" disabled={readOnly || !commitMessage.trim() || gitBusy} type="button" onclick={commit}>{ui.commit}</button>
            </div>
          </div>

          <div class="change-sections">
            {#each changeSections as section (section.id)}
              <section class="change-section">
                <button class="change-section__header" type="button" onclick={() => toggleChangeSection(section.id)}>
                  {#if isChangeSectionExpanded(section.id)}
                    <ChevronDown size={14} />
                  {:else}
                    <ChevronRight size={14} />
                  {/if}
                  <span>{section.label}</span>
                  <span class="change-section__count">{section.entries.length}</span>
                </button>

                {#if isChangeSectionExpanded(section.id)}
                  {#if section.entries.length === 0}
                    <p class="field-note field-note--dense">{section.emptyLabel}</p>
                  {:else}
                    <div class="scm-list">
                      {#each (changeViewMode === "tree" ? flattenChangeTree(section.tree) : section.entries) as row (row.key)}
                        {#if "type" in row && row.type === "folder"}
                          <button
                            class="scm-row scm-row--folder"
                            style={`--scm-depth:${row.depth}`}
                            type="button"
                            onclick={() => toggleChangeFolder(row.key)}
                          >
                            <span class="scm-row__primary">
                              <span class="scm-row__icon">
                                {#if isChangeFolderExpanded(row.key)}
                                  <ChevronDown size={13} />
                                  <FolderOpen size={14} />
                                {:else}
                                  <ChevronRight size={13} />
                                  <Folder size={14} />
                                {/if}
                              </span>
                              <span class="scm-row__copy">
                                <strong>{row.name}</strong>
                              </span>
                            </span>
                            <span class="scm-row__meta">{row.count}</span>
                          </button>
                        {:else}
                          {@const entry = "entry" in row ? row.entry : row}
                          {@const depth = "depth" in row ? row.depth : 0}
                          <article
                            class:scm-row--active={isActiveChangeEntry(entry)}
                            class="scm-row scm-row--file"
                            style={`--scm-depth:${depth}`}
                          >
                            <button class="scm-row__primary" type="button" onclick={() => void openChangeEntry(entry)}>
                              <span class="scm-row__icon">
                                <FileText size={13} />
                              </span>
                              <span class="scm-row__copy">
                                <strong>{entry.fileName}</strong>
                                {#if getChangeSecondaryLabel(entry)}
                                  <small>{getChangeSecondaryLabel(entry)}</small>
                                {/if}
                              </span>
                            </button>
                            <div class="scm-row__actions">
                              <span class={`scm-status scm-status--${getChangeStatusTone(entry.statusCode)}`} title={entry.statusLabel}>
                                {entry.statusCode}
                              </span>
                              <button
                                class="icon-button"
                                disabled={readOnly || gitBusy}
                                title={entry.sectionId === "staged" ? ui.unstage : ui.stage}
                                type="button"
                                onclick={(event) => {
                                  event.stopPropagation();
                                  void applyChangeEntry(entry);
                                }}
                              >
                                {entry.sectionId === "staged" ? "−" : "+"}
                              </button>
                              {#if selectedRepoPath && onOpenDiffTab}
                                <button
                                  class="icon-button"
                                  title={ui.openTab}
                                  type="button"
                                  onclick={(event) => {
                                    event.stopPropagation();
                                    void openChangeEntry(entry, true);
                                  }}
                                >
                                  <ExternalLink size={13} />
                                </button>
                              {/if}
                            </div>
                          </article>
                        {/if}
                      {/each}
                    </div>
                  {/if}
                {/if}
              </section>
            {/each}
          </div>
        </section>
      {/if}

      {#if !isMobileLayout || mobileSection === "pulls"}
        <section class="panel">
          <div class="panel__header">
            <div class="panel__title">
              <div class="panel__title-row">
                <GitPullRequest size={16} />
                <h3>{ui.pullRequests}</h3>
              </div>
              {#if githubRepo}
                <span>{githubRepo.owner}/{githubRepo.name}</span>
              {/if}
            </div>
            <div class="git-actions">
              <label class="field field--inline field--compact">
                <span>{ui.pullRequests}</span>
                <select bind:value={pullRequestState} disabled={loadingPullRequests || loadingPullRequestDetail}>
                  <option value="open">{ui.pullRequestsOpen}</option>
                  <option value="closed">{ui.pullRequestsClosed}</option>
                  <option value="all">{ui.pullRequestsAll}</option>
                </select>
              </label>
              <button class="ghost-button small" disabled={!selectedRepoPath || loadingPullRequests} type="button" onclick={() => selectedRepoPath && refreshPullRequests(selectedRepoPath)}>
                <RefreshCw class={loadingPullRequests ? "spin" : ""} size={14} />
              </button>
            </div>
          </div>

          {#if githubErrorText}
            <div class="field-note">{githubErrorText}</div>
          {:else if loadingPullRequests}
            <div class="placeholder-card">{ui.pullRequestsLoading}</div>
          {:else if pullRequests.length === 0}
            <p class="field-note">{ui.noPullRequests}</p>
          {:else}
            <div class="file-list">
              {#each pullRequests as pullRequest (pullRequest.number)}
                <article class="file-row file-row--stacked">
                  <button class="file-link file-link--stacked" type="button" onclick={() => void openPullRequest(pullRequest.number)}>
                    <strong>#{pullRequest.number} {pullRequest.title}</strong>
                    <small>{pullRequest.author ?? "unknown"} · {pullRequest.baseRefName} ← {pullRequest.headRefName}</small>
                  </button>
                  <div class="file-actions">
                    <span class={`meta-pill ${pullRequest.state === "merged" ? "" : "subtle"}`}>{pullRequest.state}</span>
                    {#if pullRequest.isDraft}
                      <span class="meta-pill subtle">draft</span>
                    {/if}
                    <button class="ghost-button small" disabled={readOnly || githubBusyAction === "checkout"} type="button" onclick={() => void checkoutPullRequest(pullRequest.number)}>
                      {githubBusyPullRequestNumber === pullRequest.number ? ui.checkingOutPr : ui.checkoutPr}
                    </button>
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
                <LazyMonacoDiffEditor
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
                <LazyMonacoDiffEditor height={340} modified={editorValue} original={filePayload.originalContent} path={filePayload.filePath} />
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
        {:else if loadingPullRequestDetail}
          <div class="placeholder-card">{ui.pullRequestsLoading}</div>
        {:else if pullRequestDetail && (!isMobileLayout || mobileSection === "detail")}
          {@const activePullRequest = pullRequestDetail.pullRequest}
          <div class="panel__header">
            <div class="panel__title">
              <h3>#{activePullRequest.number} {activePullRequest.title}</h3>
              <span>{activePullRequest.baseRefName} ← {activePullRequest.headRefName}</span>
            </div>
            <div class="git-actions">
              <a class="ghost-button" href={activePullRequest.url} rel="noreferrer noopener" target="_blank">
                <ExternalLink size={14} />
                <span>{ui.openOnGitHub}</span>
              </a>
              <button class="ghost-button" disabled={readOnly || githubBusyAction === "checkout"} type="button" onclick={() => void checkoutPullRequest(activePullRequest.number)}>
                {githubBusyPullRequestNumber === activePullRequest.number ? ui.checkingOutPr : ui.checkoutPr}
              </button>
              <button class="ghost-button" type="button" onclick={closeEditor}>{ui.close}</button>
            </div>
          </div>

          <div class="editor-stack">
            <section class="panel">
              <div class="panel__header">
                <h3>{ui.pullRequestOverview}</h3>
                <span>{activePullRequest.changedFiles} files · +{activePullRequest.additions} / -{activePullRequest.deletions}</span>
              </div>
              <div class="space-y-3">
                <div class="git-meta">
                  <div class="meta-pill">{activePullRequest.state}</div>
                  {#if activePullRequest.isDraft}
                    <div class="meta-pill subtle">draft</div>
                  {/if}
                  {#if activePullRequest.reviewDecision}
                    <div class="meta-pill subtle">{activePullRequest.reviewDecision}</div>
                  {/if}
                </div>
                {#if activePullRequest.body.trim()}
                  <div class="git-markdown-body">
                    <MarkdownMessage text={activePullRequest.body} />
                  </div>
                {:else}
                  <p class="field-note">{ui.pullRequestBodyEmpty}</p>
                {/if}
              </div>
            </section>

            <section class="panel">
              <div class="panel__header">
                <h3>{ui.pullRequestFiles}</h3>
                <span>{activePullRequest.filesLoaded ?? activePullRequest.files.length}</span>
              </div>
              {#if activePullRequest.filesTruncated}
                <p class="field-note">{ui.pullRequestFilesTruncated}</p>
              {/if}

              <div class="grouped-diff-list">
                <div class="file-list">
                  {#each activePullRequest.files as file (file.path)}
                    <article class="file-row">
                      <button class="file-link file-link--stacked" type="button" onclick={() => (selectedPullRequestFilePath = file.path)}>
                        <strong>{file.path}</strong>
                        <small>{file.status}{file.previousPath ? ` · ${file.previousPath}` : ""}</small>
                      </button>
                      <div class="file-actions">
                        <span class="meta-pill subtle">+{file.additions}</span>
                        <span class="meta-pill subtle">-{file.deletions}</span>
                      </div>
                    </article>
                  {/each}
                </div>

                {#if selectedPullRequestFile}
                  <section class="panel grouped-diff-panel">
                    <div class="panel__header">
                      <div class="panel__title">
                        <h3>{selectedPullRequestFile.path}</h3>
                        <span>{selectedPullRequestFile.status}</span>
                      </div>
                      <div class="git-actions">
                        <span class="meta-pill subtle">+{selectedPullRequestFile.additions}</span>
                        <span class="meta-pill subtle">-{selectedPullRequestFile.deletions}</span>
                      </div>
                    </div>

                    {#if selectedPullRequestFile.patch}
                      <MonacoTextEditor height={320} path={`${selectedPullRequestFile.path}.diff`} readonly value={selectedPullRequestFile.patch} />
                    {:else}
                      <div class="placeholder-card">{ui.binaryDiffNotPreviewable}</div>
                    {/if}
                  </section>
                {/if}
              </div>
            </section>
          </div>
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

  .field--compact {
    grid-template-columns: auto minmax(0, 1fr);
    min-width: 0;
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
    border: 1px solid var(--line);
    border-radius: 1rem;
    background: color-mix(in srgb, var(--panel-strong) 86%, transparent);
    color: var(--ink);
    padding: 0.68rem 0.82rem;
  }

  .meta-pill {
    border-radius: 999px;
    background: color-mix(in srgb, var(--panel-strong) 82%, transparent);
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
    grid-template-columns: minmax(18rem, 0.92fr) minmax(17rem, 0.88fr) minmax(0, 1.48fr);
  }

  .git-mobile-nav {
    display: none;
  }

  .panel {
    display: grid;
    gap: 0.7rem;
    border: 1px solid var(--line);
    border-radius: 1.15rem;
    background: color-mix(in srgb, var(--panel-strong) 76%, transparent);
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

  .panel__title {
    display: grid;
    gap: 0.22rem;
    min-width: 0;
  }

  .panel__title-row {
    display: flex;
    gap: 0.5rem;
    align-items: center;
  }

  .panel--scm {
    align-content: start;
  }

  .panel--scm .panel__header {
    align-items: flex-start;
  }

  .panel__title span {
    color: var(--muted);
    font-size: 0.75rem;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .file-list,
  .commit-list {
    display: grid;
    gap: 0.4rem;
    max-height: 20rem;
    overflow: auto;
  }

  .file-row,
  .commit-row {
    display: flex;
    gap: 0.48rem;
    align-items: center;
    border-radius: 1rem;
    background: color-mix(in srgb, var(--panel-soft) 82%, transparent);
    padding: 0.54rem 0.62rem;
  }

  .file-row--stacked {
    align-items: flex-start;
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

  .file-link--stacked {
    display: grid;
    gap: 0.22rem;
    align-items: flex-start;
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

  .scm-toolbar {
    display: flex;
    justify-content: flex-end;
    flex: 0 0 auto;
  }

  .segmented-control {
    display: inline-flex;
    gap: 0.2rem;
    padding: 0.2rem;
    border-radius: 999px;
    background: color-mix(in srgb, var(--panel-soft) 88%, transparent);
  }

  .segmented-control__button {
    display: inline-flex;
    gap: 0.36rem;
    align-items: center;
    border: 0;
    border-radius: 999px;
    background: transparent;
    color: var(--muted);
    padding: 0.42rem 0.72rem;
    font-size: 0.76rem;
    font-weight: 700;
    transition: background-color 140ms ease, color 140ms ease;
  }

  .segmented-control__button--active {
    background: color-mix(in srgb, var(--panel-strong) 98%, transparent);
    color: var(--ink-strong);
  }

  .scm-commit-box {
    display: grid;
    gap: 0.55rem;
    padding: 0.72rem;
    border-radius: 1rem;
    background: color-mix(in srgb, var(--panel-soft) 82%, transparent);
  }

  .scm-commit-box__input {
    width: 100%;
    min-height: 4.85rem;
    border: 1px solid rgba(83, 61, 42, 0.12);
    border-radius: 0.92rem;
    background: color-mix(in srgb, var(--panel-strong) 92%, transparent);
    color: var(--ink);
    padding: 0.72rem 0.82rem;
    resize: vertical;
  }

  .scm-commit-box__actions {
    display: flex;
    gap: 0.55rem;
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
  }

  .change-sections {
    display: grid;
    gap: 0.6rem;
    min-height: 0;
  }

  .change-section {
    display: grid;
    gap: 0.35rem;
    min-height: 0;
  }

  .change-section__header {
    display: flex;
    gap: 0.42rem;
    align-items: center;
    justify-content: flex-start;
    border: 0;
    background: transparent;
    color: var(--ink-strong);
    padding: 0.12rem 0.12rem 0.12rem 0;
    font-size: 0.82rem;
    font-weight: 700;
    text-align: left;
  }

  .change-section__count {
    margin-left: auto;
    color: var(--muted);
    font-size: 0.74rem;
    font-weight: 600;
  }

  .scm-list {
    display: grid;
    gap: 0.22rem;
    max-height: min(28rem, 52dvh);
    overflow: auto;
  }

  .scm-row {
    display: flex;
    gap: 0.45rem;
    align-items: center;
    min-width: 0;
    border-radius: 0.85rem;
    background: color-mix(in srgb, var(--panel-soft) 78%, transparent);
    padding: 0.38rem 0.46rem;
    padding-inline-start: calc(0.46rem + (var(--scm-depth, 0) * 0.78rem));
    transition: background-color 140ms ease, box-shadow 140ms ease;
  }

  .scm-row--file:hover,
  .scm-row--folder:hover {
    background: color-mix(in srgb, var(--accent) 8%, var(--panel-soft));
  }

  .scm-row--file {
    overflow: hidden;
  }

  .scm-row--folder {
    width: 100%;
    border: 0;
    color: inherit;
    cursor: pointer;
    text-align: left;
  }

  .scm-row--active {
    box-shadow: inset 0 0 0 1px rgba(214, 140, 69, 0.28);
    background: rgba(255, 248, 237, 0.98);
  }

  .scm-row__primary {
    display: flex;
    gap: 0.45rem;
    align-items: center;
    min-width: 0;
    flex: 1 1 auto;
    border: 0;
    background: transparent;
    color: inherit;
    cursor: pointer;
    padding: 0;
    text-align: left;
  }

  .scm-row__icon {
    display: inline-flex;
    gap: 0.14rem;
    align-items: center;
    color: var(--muted);
    flex: 0 0 auto;
  }

  .scm-row__copy {
    display: grid;
    gap: 0.04rem;
    min-width: 0;
  }

  .scm-row__copy strong,
  .scm-row__copy small,
  .scm-row__meta {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .scm-row__copy strong {
    color: var(--ink-strong);
    font-size: 0.79rem;
    font-weight: 700;
  }

  .scm-row__copy small,
  .scm-row__meta {
    color: var(--muted);
    font-size: 0.7rem;
  }

  .scm-row__actions {
    display: inline-flex;
    gap: 0.28rem;
    align-items: center;
    justify-content: flex-end;
    flex: 0 0 auto;
  }

  .icon-button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 1.72rem;
    height: 1.72rem;
    border: 1px solid rgba(83, 61, 42, 0.12);
    border-radius: 0.65rem;
    background: color-mix(in srgb, var(--panel-strong) 88%, transparent);
    color: var(--ink);
    padding: 0;
    transition: border-color 140ms ease, background-color 140ms ease, color 140ms ease;
  }

  .icon-button:hover:not(:disabled) {
    border-color: rgba(214, 140, 69, 0.24);
    background: color-mix(in srgb, var(--accent) 10%, var(--panel-strong));
    color: var(--ink-strong);
  }

  .icon-button:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }

  .scm-status {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 1.72rem;
    height: 1.72rem;
    border-radius: 0.65rem;
    background: color-mix(in srgb, var(--panel-strong) 92%, transparent);
    font-size: 0.7rem;
    font-weight: 800;
    letter-spacing: 0.02em;
  }

  .scm-status--added {
    color: #1f7a41;
  }

  .scm-status--modified {
    color: #b66a11;
  }

  .scm-status--deleted {
    color: #b13a3a;
  }

  .scm-status--renamed {
    color: #2869b6;
  }

  .field-note--dense {
    margin: 0;
    padding: 0.18rem 0 0.08rem 1.5rem;
    font-size: 0.76rem;
  }

  .git-markdown-body {
    border-radius: 1rem;
    background: color-mix(in srgb, var(--panel-soft) 78%, transparent);
    padding: 0.85rem 0.95rem;
  }

  .spin {
    animation: git-workspace-spin 0.9s linear infinite;
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

  @media (max-width: 1480px) {
    .panel-grid {
      grid-template-columns: minmax(19rem, 0.98fr) minmax(0, 1.3fr);
    }
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
      background: color-mix(in srgb, var(--panel-strong) 78%, transparent);
      color: var(--muted);
      padding: 0.55rem 0.9rem;
      font-size: 0.76rem;
      font-weight: 700;
      white-space: nowrap;
      transition: color 140ms ease, background-color 140ms ease, border-color 140ms ease;
    }

    .git-mobile-nav__button--active {
      background: color-mix(in srgb, var(--accent) 10%, var(--panel-strong));
      border-color: rgba(214, 140, 69, 0.28);
      color: var(--ink-strong);
    }

    .git-mobile-nav__button:disabled {
      opacity: 0.45;
    }

    .git-header,
    .git-meta,
    .panel__header,
    .scm-commit-box__actions,
    .inline-field,
    .toolbar-row,
    .file-row,
    .commit-row {
      flex-direction: column;
      align-items: stretch;
    }

    .field--inline {
      grid-template-columns: 1fr;
    }

    .segmented-control {
      width: 100%;
      justify-content: stretch;
    }

    .segmented-control__button {
      flex: 1 1 0;
      justify-content: center;
    }

    .scm-toolbar {
      width: 100%;
    }

    .scm-list {
      max-height: none;
    }

    .scm-row {
      align-items: flex-start;
    }

    .scm-row__actions {
      width: 100%;
      justify-content: flex-end;
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

  @keyframes git-workspace-spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }
</style>
