import { computed, ref } from "vue";
import { defineStore } from "pinia";
import { cmd } from "@/lib/tauri";
import type {
  GitBranches,
  GitMergeResult,
  GitPullResult,
  GitRebaseResult,
  GitStatus,
  GitStatusItem,
  GitUpdatedPayload,
  GitWorktree,
  Project,
} from "@/types";

export const useProjectsStore = defineStore("projects", () => {
  const projects = ref<Project[]>([]);
  const archivedProjects = ref<Project[]>([]);
  const loading = ref(false);
  const query = ref("");
  const selectedTagIds = ref<number[]>([]);

  /**
   * 拉取项目列表。
   * withGit 为 false 时(搜索/筛选等高频操作)不重新拉取 git 状态,
   * 仅按项目 id 保留已有的 git 信息,避免频繁触发 git 更新。
   */
  async function fetchProjects(options: { withGit?: boolean } = {}) {
    const withGit = options.withGit ?? true;
    loading.value = true;
    try {
      const list = await cmd<Project[]>("list_projects", {
        query: query.value.trim() ? query.value.trim() : null,
        tagIds: selectedTagIds.value.length ? selectedTagIds.value : null,
      });
      if (withGit) {
        projects.value = list;
        // Git 状态后台补齐,不阻塞列表渲染(一次批量 IPC,后端带缓存)
        refreshAllGitStatus();
      } else {
        const prevGit = new Map(projects.value.map((p) => [p.id, p.git]));
        list.forEach((p) => {
          p.git = prevGit.get(p.id) ?? p.git;
        });
        projects.value = list;
      }
    } finally {
      loading.value = false;
    }
  }

  function setQuery(value: string) {
    query.value = value;
    fetchProjects({ withGit: false });
  }

  function toggleTagFilter(tagId: number) {
    selectedTagIds.value = selectedTagIds.value.includes(tagId)
      ? selectedTagIds.value.filter((id) => id !== tagId)
      : [...selectedTagIds.value, tagId];
    fetchProjects({ withGit: false });
  }

  function clearTagFilters() {
    if (!selectedTagIds.value.length) return;
    selectedTagIds.value = [];
    fetchProjects({ withGit: false });
  }

  /** 重新拉取单个项目(保留已有的 git 状态,后端不返回) */
  async function refreshProject(id: number) {
    const fresh = await cmd<Project>("get_project", { id });
    const idx = projects.value.findIndex((p) => p.id === id);
    if (idx >= 0) {
      fresh.git = projects.value[idx].git;
      projects.value[idx] = fresh;
    }
    return fresh;
  }

  /**
   * 兜底注入:托盘弹窗跳详情时,主窗口 store 可能因搜索/标签筛选被裁剪,
   * 导致 ProjectDetail 按 id 找不到而误报「项目不存在或已被删除」。
   * 这里强制按 id 单点拉取并写入列表(替换保留 git,新增直接 push)。
   * 后端真实找不到时向上抛错,调用方决定是否降级跳转到 not-found 页。
   */
  async function ensureProjectLoaded(id: number) {
    const fresh = await cmd<Project>("get_project", { id });
    const idx = projects.value.findIndex((p) => p.id === id);
    if (idx >= 0) {
      fresh.git = projects.value[idx].git ?? fresh.git;
      projects.value[idx] = fresh;
    } else {
      projects.value.push(fresh);
    }
    return fresh;
  }

  /**
   * 刷新单个项目的 git 状态。
   * 默认走后端 15s 缓存:详情页进入等高频场景直接命中,避免每次进大仓库都重跑
   * git status;缓存过期由后端 30s 后台刷新循环兜底保鲜,结果经 git://status-updated 推送。
   * git 写操作本身会返回最新状态并回填缓存,不经过此函数。
   * force: true 用于路径变更(重定向/移动目录)后必须拿到最新状态的场景。
   */
  async function refreshGitStatus(project: Project, options: { force?: boolean } = {}) {
    const force = options.force ?? false;
    const run = () => cmd<GitStatus>("get_git_status", { path: project.path, force });
    try {
      project.git = await run();
    } catch {
      // 失败重试一次;仍失败时保留旧值(原本就没有则保持 null),避免卡片 git 信息闪烁丢失
      try {
        project.git = await run();
      } catch {
        /* 保留旧值,等待下一轮后台刷新 */
      }
    }
  }

  /**
   * 批量刷新所有项目的 git 状态:一次 IPC 返回全部(后端带缓存)。
   * force 为 true 时绕过缓存强制重查(启动/用户主动刷新)
   */
  async function refreshAllGitStatus(force = false) {
    const paths = projects.value.map((p) => p.path);
    if (!paths.length) return;
    try {
      const items = await cmd<GitStatusItem[]>("refresh_all_git_status", { paths, force });
      applyGitStatusItems(items);
    } catch {
      // 失败静默:由后端事件推送/窗口聚焦兜底补充
    }
  }

  /** 按路径批量写入 git 状态(后端事件推送与批量命令共用) */
  function applyGitStatusItems(items: GitStatusItem[]) {
    if (!items.length) return;
    const byPath = new Map(items.map((i) => [i.path, i.status]));
    projects.value.forEach((p) => {
      const st = byPath.get(p.path);
      if (st) p.git = st;
    });
  }

  async function addProject(path: string, name: string, description?: string) {
    const project = await cmd<Project>("add_project", {
      path,
      name,
      description: description?.trim() ? description.trim() : null,
    });
    await fetchProjects();
    return project;
  }

  /** 克隆仓库到本地,返回克隆后的路径;可被 cancelClone 中断。
   *  accountId 传入时后端用绑定账号 token 克隆(成功后重置 origin 为干净 URL) */
  async function cloneProject(
    url: string,
    targetPath: string,
    jobId: string,
    accountId?: number,
  ): Promise<string> {
    return cmd<string>("git_clone", { url, targetPath, jobId, accountId: accountId ?? null });
  }

  /** 取消进行中的克隆(后端 kill 子进程并清理半成品目录) */
  async function cancelClone(jobId: string) {
    await cmd("cancel_git_clone", { jobId });
  }

  async function updateProject(id: number, name: string, description: string) {
    const project = await cmd<Project>("update_project", { id, name, description });
    const idx = projects.value.findIndex((p) => p.id === id);
    if (idx >= 0) projects.value[idx] = project;
    return project;
  }

  /** 重新指定项目目录(目录被移动后修复登记路径),成功后重新探测 git 状态 */
  async function updateProjectPath(id: number, path: string) {
    const project = await cmd<Project>("update_project_path", { id, path });
    const idx = projects.value.findIndex((p) => p.id === id);
    if (idx >= 0) projects.value[idx] = project;
    if (project.path_exists) {
      // 路径刚变更,绕过缓存强制重查新路径状态
      await refreshGitStatus(project, { force: true });
    }
    return project;
  }

  /** 应用内移动项目目录到新的父目录下(可改名),成功后重新探测 git 状态 */
  async function moveProjectDir(id: number, targetParent: string, dirName: string) {
    const project = await cmd<Project>("move_project_dir", { id, targetParent, dirName });
    const idx = projects.value.findIndex((p) => p.id === id);
    if (idx >= 0) projects.value[idx] = project;
    await refreshGitStatus(project, { force: true });
    return project;
  }

  /** 归档项目:软删除,历史数据保留;归档后不再展示、不再获取 git 状态 */
  async function archiveProject(id: number) {
    await cmd("archive_project", { id });
    projects.value = projects.value.filter((p) => p.id !== id);
  }

  /** 设置/取消收藏:成功后就地更新 favorited_at,列表排序由 computed 响应 */
  async function setFavorite(id: number, favorite: boolean) {
    await cmd("set_project_favorite", { id, favorite });
    const p = projects.value.find((x) => x.id === id);
    if (p) {
      p.favorited_at = favorite ? Math.floor(Date.now() / 1000) : null;
    }
  }

  /** 设置/取消「跟踪更新」:开启后远端有更新时后台自动快进拉取(无法快进即取消,不提醒) */
  async function setAutoPull(id: number, enabled: boolean) {
    await cmd("set_project_auto_pull", { id, enabled });
    const p = projects.value.find((x) => x.id === id);
    if (p) {
      p.auto_pull = enabled;
    }
  }

  /** 设置/取消项目级「Wiki 自动增量更新」:实际触发还需全局开关与「跟踪更新」开启 */
  async function setWikiAutoUpdate(id: number, enabled: boolean) {
    await cmd("set_project_wiki_auto_update", { id, enabled });
    const p = projects.value.find((x) => x.id === id);
    if (p) {
      p.wiki_auto_update = enabled;
    }
  }

  /** 拉取已归档项目列表(设置页归档管理用) */
  async function fetchArchivedProjects() {
    archivedProjects.value = await cmd<Project[]>("list_archived_projects");
  }

  /** 取消归档:恢复到项目列表 */
  async function unarchiveProject(id: number) {
    await cmd("unarchive_project", { id });
    archivedProjects.value = archivedProjects.value.filter((p) => p.id !== id);
    await fetchProjects();
  }

  /** 彻底删除项目(不可恢复,历史数据一并删除;不会删除磁盘文件) */
  async function deleteProject(id: number) {
    await cmd("delete_project", { id });
    archivedProjects.value = archivedProjects.value.filter((p) => p.id !== id);
  }

  /** 后台 fetch 完成后由 git://updated 事件调用 */
  function updateGitRemote(projectId: number, payload: GitUpdatedPayload) {
    const p = projects.value.find((x) => x.id === projectId);
    if (p?.git) {
      p.git.remote_ahead = payload.remote_ahead;
      p.git.last_fetch_at = payload.last_fetch_at;
    }
  }

  // --- Git 写操作:错误向上抛出由 UI toast,成功后用返回的最新状态就地更新 ---

  function listBranches(project: Project) {
    return cmd<GitBranches>("list_git_branches", { path: project.path });
  }

  /** 在项目目录初始化 git 仓库;branch 空回退 main,remoteUrl 非空时添加为 origin */
  async function initRepository(project: Project, branch: string, remoteUrl?: string) {
    project.git = await cmd<GitStatus>("git_init", {
      path: project.path,
      branch,
      remoteUrl: remoteUrl || null,
    });
  }

  /**
   * 切换分支。create: 创建并切换;remote: branch 形如 "origin/feature",
   * 本地无同名分支时自动创建跟踪分支;startPoint: create 时的基点(本地分支或 origin/xxx)
   */
  async function checkoutBranch(
    project: Project,
    branch: string,
    options: { create?: boolean; remote?: boolean; startPoint?: string } = {},
  ) {
    project.git = await cmd<GitStatus>("git_checkout", {
      path: project.path,
      branch,
      create: options.create ?? false,
      remote: options.remote ?? false,
      startPoint: options.startPoint ?? null,
    });
  }

  /** 提交更改(未暂存修改始终纳入;includeUntracked 控制是否包含未跟踪文件;
   * paths 为提交对话框勾选的文件子集,空/null 表示全量提交) */
  async function commitChanges(
    project: Project,
    message: string,
    includeUntracked: boolean,
    paths?: string[] | null,
  ) {
    project.git = await cmd<GitStatus>("git_commit", {
      path: project.path,
      message,
      includeUntracked,
      paths: paths ?? null,
    });
  }

  /** 拉取远端;返回冲突文件列表(非空表示产生了合并冲突)。branch 指定拉取其他本地分支(快进更新,不切工作区) */
  async function pullRepository(project: Project, branch?: string) {
    const result = await cmd<GitPullResult>("git_pull", {
      path: project.path,
      branch: branch ?? null,
    });
    project.git = result.status;
    return result.conflicts;
  }

  /** 推送分支(无 upstream 时后端自动 -u <远端>,优先 origin);branch 缺省推送当前分支 */
  async function pushRepository(project: Project, branch?: string) {
    project.git = await cmd<GitStatus>("git_push", {
      path: project.path,
      branch: branch ?? null,
    });
  }

  /** 删除本地分支;force=false 仅允许删除已合并分支(未合并报 git_branch_not_merged) */
  async function deleteBranch(project: Project, branch: string, force: boolean) {
    project.git = await cmd<GitStatus>("git_branch_delete", {
      path: project.path,
      branch,
      force,
    });
  }

  /** 删除远程分支(branch 形如 "origin/feature/x",后端 push --delete 移除远端引用) */
  async function deleteRemoteBranch(project: Project, branch: string) {
    project.git = await cmd<GitStatus>("git_remote_branch_delete", {
      path: project.path,
      branch,
    });
  }

  // --- worktree / 合并 / 变基 ---

  /** 列出仓库的全部 worktree(第一条为主工作区) */
  function listWorktrees(project: Project) {
    return cmd<GitWorktree[]>("list_git_worktrees", { path: project.path });
  }

  /**
   * 创建 worktree,返回最新 worktree 列表。
   * worktreePath 支持 `{branch}` 占位符与相对路径(后端基于主工作区根解析);
   * createBranch 为 true 时检出新分支(可选 startPoint 基点),
   * 为 false 时挂载已有分支(本地分支或 origin/xxx 远程分支;远程无本地同名时
   * 创建跟踪分支,同名时本地落后会先快进对齐,分叉报 git_branch_diverged)
   */
  function addWorktree(
    project: Project,
    worktreePath: string,
    branch: string,
    options: { createBranch?: boolean; startPoint?: string; baseBranch?: string } = {},
  ) {
    return cmd<GitWorktree[]>("git_worktree_add", {
      path: project.path,
      worktreePath,
      branch,
      createBranch: options.createBranch ?? true,
      startPoint: options.startPoint ?? null,
      baseBranch: options.baseBranch ?? null,
    });
  }

  /** 删除 worktree;force 强制(含未提交修改时),deleteBranch 同时删除其检出的本地分支;
   *  branch 为 worktree 检出分支名,供上次部分成功(登记已删、分支未删)后的强制重试使用 */
  function removeWorktree(
    project: Project,
    worktreePath: string,
    options: { force?: boolean; deleteBranch?: boolean; branch?: string | null } = {},
  ) {
    return cmd<GitWorktree[]>("git_worktree_remove", {
      path: project.path,
      worktreePath,
      force: options.force ?? false,
      deleteBranch: options.deleteBranch ?? false,
      branch: options.branch ?? null,
    });
  }

  /**
   * 将 branch 合并进目标分支(target 缺省为主工作区当前分支);squash 时只暂存不提交。
   * 目标分支检出在其它 worktree 时在该 worktree 内合并(不回写 project.git);
   * 未被任何 worktree 检出时仅快进。返回冲突文件列表(非空表示有冲突)与实际合并位置
   */
  async function mergeBranch(
    project: Project,
    branch: string,
    options: { squash?: boolean; target?: string } = {},
  ) {
    const result = await cmd<GitMergeResult>("git_merge", {
      path: project.path,
      branch,
      target: options.target ?? null,
      squash: options.squash ?? false,
    });
    // 仅当合并发生在主工作区(target 缺省或即主工作区当前分支)时回写状态
    if (!options.target || options.target === project.git?.branch) {
      project.git = result.status;
    }
    return { conflicts: result.conflicts, mergedIn: result.merged_in };
  }

  /** 中止进行中的合并(git merge --abort) */
  async function abortMerge(project: Project) {
    project.git = await cmd<GitStatus>("git_merge_abort", { path: project.path });
  }

  /**
   * 将 path 所在工作区的当前分支变基到 onto 之上(默认传项目路径;
   * worktree 场景传 worktree 路径,此时不回写 project.git)。返回冲突列表与是否中断
   */
  async function rebaseBranch(project: Project, onto: string, path?: string) {
    const result = await cmd<GitRebaseResult>("git_rebase", {
      path: path ?? project.path,
      onto,
    });
    if (!path) {
      project.git = result.status;
    }
    return { conflicts: result.conflicts, inProgress: result.in_progress };
  }

  /** 中止进行中的变基(git rebase --abort);path 缺省为项目路径 */
  async function abortRebase(project: Project, path?: string) {
    const status = await cmd<GitStatus>("git_rebase_abort", { path: path ?? project.path });
    if (!path) {
      project.git = status;
    }
  }

  const byId = computed(() => {
    return (id: number) => projects.value.find((p) => p.id === id);
  });

  return {
    projects,
    archivedProjects,
    loading,
    query,
    selectedTagIds,
    fetchProjects,
    setQuery,
    toggleTagFilter,
    clearTagFilters,
    refreshProject,
    ensureProjectLoaded,
    addProject,
    cloneProject,
    cancelClone,
    updateProject,
    updateProjectPath,
    moveProjectDir,
    archiveProject,
    setFavorite,
    setAutoPull,
    setWikiAutoUpdate,
    fetchArchivedProjects,
    unarchiveProject,
    deleteProject,
    refreshGitStatus,
    refreshAllGitStatus,
    applyGitStatusItems,
    updateGitRemote,
    listBranches,
    initRepository,
    checkoutBranch,
    commitChanges,
    pullRepository,
    pushRepository,
    deleteBranch,
    deleteRemoteBranch,
    listWorktrees,
    addWorktree,
    removeWorktree,
    mergeBranch,
    abortMerge,
    rebaseBranch,
    abortRebase,
    byId,
  };
});
