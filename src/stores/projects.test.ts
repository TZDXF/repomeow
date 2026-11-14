import { beforeEach, describe, expect, it, vi, type Mock } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { gitCheckCoordinator } from "@/lib/git-check-coordinator";
import { useProjectsStore } from "@/stores/projects";
import { cmd } from "@/lib/tauri";
import type { GitProjectChangedPayload, GitStatus, Project } from "@/types";

// node 环境无 Tauri IPC,mock 掉前后端桥
vi.mock("@/lib/tauri", () => ({ cmd: vi.fn() }));

const cmdMock = cmd as unknown as Mock,
  status: GitStatus = {
    is_repo: true,
    branch: "main",
    ahead: 0,
    behind: 0,
    staged: 0,
    modified: 1,
    untracked: 0,
    conflicted: 0,
    remote_ahead: 0,
    last_fetch_at: null,
    last_commit_at: 1,
  };

function project(): Project {
  return {
    id: 1,
    path: "D:\\repo",
    name: "demo",
    description: "",
    tags: [],
    git: null,
    path_exists: true,
    archived_at: null,
    favorited_at: null,
    auto_pull: false,
    wiki_auto_update: true,
    created_at: 1,
    updated_at: 1,
  };
}

function event(path = "D:\\repo"): GitProjectChangedPayload {
  return {
    project_id: 1,
    name: "demo",
    path,
    status,
    head_sha: "abc",
    head_changed: true,
    auto_pulled: false,
    pulled_commits: 0,
    source: "periodic",
    wiki_auto_update: true,
  };
}

function makeProject(id: number, name: string): Project {
  return {
    id,
    path: `D:/code/${name}`,
    name,
    description: "",
    tags: [],
    git: null,
    path_exists: true,
    archived_at: null,
    favorited_at: null,
    auto_pull: false,
    wiki_auto_update: false,
    created_at: 0,
    updated_at: 0,
  };
}

describe("projects store Git 统一事件", () => {
  beforeEach(() => setActivePinia(createPinia()));

  it("按项目 id 和路径更新主工作区状态", () => {
    const store = useProjectsStore();
    store.projects = [project()];
    store.applyGitProjectEvent(event());
    expect(store.projects[0]?.git).toEqual(status);
  });

  it("不使用同项目 id 的 worktree 状态覆盖主工作区", () => {
    const store = useProjectsStore();
    store.projects = [project()];
    store.applyGitProjectEvent(event("D:\\repo-worktree"));
    expect(store.projects[0]?.git).toBeNull();
  });
});

/** 回归:首页搜索/标签筛选只裁剪列表,不得裁剪按 id 查询的全量索引(标题栏 tab 名、详情页等依赖) */
describe("projects store 全量索引", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    cmdMock.mockReset();
  });

  it("搜索筛选裁剪列表后,getProjectById 仍能查到不在结果内的项目", async () => {
    const store = useProjectsStore();
    // 首次全量拉取
    cmdMock.mockResolvedValueOnce([makeProject(34, "repo-a"), makeProject(14, "repo-b")]);
    await store.fetchProjects();

    // 搜索"b"后列表只剩 repo-b
    cmdMock.mockResolvedValueOnce([makeProject(14, "repo-b")]);
    await store.setQuery("b");
    expect(store.projects.map((p) => p.id)).toEqual([14]);

    expect(store.getProjectById(34)?.name).toBe("repo-a");
    expect(store.getProjectById(14)?.name).toBe("repo-b");
    expect(store.getProjectById(99)).toBeUndefined();
  });

  it("ensureProjectLoaded 只在全量索引注入;筛选中不污染搜索结果列表", async () => {
    const store = useProjectsStore();
    cmdMock.mockResolvedValueOnce([makeProject(1, "repo-a")]);
    await store.fetchProjects();
    cmdMock.mockResolvedValueOnce([makeProject(1, "repo-a")]);
    await store.setQuery("a");

    cmdMock.mockResolvedValueOnce(makeProject(2, "repo-b"));
    const fresh = await store.ensureProjectLoaded(2);

    expect(fresh.name).toBe("repo-b");
    expect(store.getProjectById(2)?.name).toBe("repo-b");
    // 筛选中的列表不被注入
    expect(store.projects.map((p) => p.id)).toEqual([1]);

    // 无筛选时同步进列表
    cmdMock.mockResolvedValueOnce([makeProject(1, "repo-a")]);
    await store.setQuery("");
    cmdMock.mockResolvedValueOnce(makeProject(3, "repo-c"));
    await store.ensureProjectLoaded(3);
    expect(store.projects.map((p) => p.id)).toEqual([1, 3]);
  });

  it.each([
    { label: "搜索", query: "repo-a", tagIds: [], visible: false },
    { label: "标签筛选", query: "", tagIds: [7], visible: false },
    { label: "搜索与标签筛选", query: "repo-a", tagIds: [7], visible: false },
    { label: "匹配搜索条件", query: "repo", tagIds: [], visible: true },
    { label: "无筛选", query: "", tagIds: [], visible: true },
  ])("$label 时新增项目可按 id 查询,列表仍遵循筛选结果", async ({ query, tagIds, visible }) => {
    const store = useProjectsStore(),
      existing = makeProject(1, "repo-a"),
      added = makeProject(40, "repo-b");
    cmdMock.mockImplementation(async (command: string) => {
      switch (command) {
        case "add_project":
          return added;
        case "list_projects":
          return visible ? [existing, added] : [existing];
        case "check_git_status":
          return [];
        default:
          throw new Error(`Unexpected command: ${command}`);
      }
    });
    store.query = query;
    store.selectedTagIds = tagIds;

    const result = await store.addProject(added.path, added.name);

    expect(result).toEqual(added);
    expect(store.getProjectById(40)?.name).toBe("repo-b");
    expect(store.projects.map((p) => p.id)).toEqual(visible ? [1, 40] : [1]);
    expect(store.query).toBe(query);
    expect(store.selectedTagIds).toEqual(tagIds);
    expect(cmdMock).toHaveBeenCalledWith("list_projects", {
      query: query || null,
      tagIds: tagIds.length ? tagIds : null,
    });
  });

  it("归档/删除后从全量索引移除,刷新单项目同步索引", async () => {
    const store = useProjectsStore();
    cmdMock.mockResolvedValueOnce([makeProject(1, "repo-a"), makeProject(2, "repo-b")]);
    await store.fetchProjects();

    await store.archiveProject(1);
    expect(store.getProjectById(1)).toBeUndefined();
    expect(store.getProjectById(2)?.name).toBe("repo-b");

    cmdMock.mockResolvedValueOnce(makeProject(2, "repo-b-renamed"));
    await store.refreshProject(2);
    expect(store.getProjectById(2)?.name).toBe("repo-b-renamed");
  });
});

/** 回归:所有 Git 检查入口共享 15s 路径记录,大仓库不会被聚焦/进页重复扫描 */
describe("projects store Git 检查协调", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    cmdMock.mockReset();
    gitCheckCoordinator.reset();
  });

  it("15s 内第二次 refreshGitStatus(含 force)复用已有结果,不再发 IPC", async () => {
    const store = useProjectsStore(),
      p = project();
    store.projects = [p];
    cmdMock.mockResolvedValueOnce([{ path: p.path, status }]);
    await store.refreshGitStatus(p);
    expect(cmdMock).toHaveBeenCalledOnce();

    // 第二次带上 force:仍命中共享记录,不发起新的扫描
    await store.refreshGitStatus(p, { force: true });
    expect(cmdMock).toHaveBeenCalledOnce();
    expect(p.git).toEqual(status);
  });

  it("另一入口刚检查过时,refreshGitStatus 直接复用其结果", async () => {
    const store = useProjectsStore(),
      p = project();
    store.projects = [p];
    gitCheckCoordinator.record(p.path, status);
    await store.refreshGitStatus(p, { force: true });
    expect(cmdMock).not.toHaveBeenCalled();
    expect(p.git).toEqual(status);
  });

  it("同一路径的检查进行中时复用该请求,不叠加并发扫描", async () => {
    const store = useProjectsStore(),
      p = project();
    store.projects = [p];
    let release!: (items: unknown) => void;
    const deferred = new Promise<unknown>((resolve) => {
      release = resolve;
    });
    cmdMock.mockReturnValueOnce(deferred);
    const first = store.refreshGitStatus(p),
      // 检查仍在进行中时第二个入口发起同一路径请求:应复用而非再发一次 IPC
      second = store.refreshGitStatus(p, { force: true });
    expect(cmdMock).toHaveBeenCalledOnce();
    release([{ path: p.path, status }]);
    await Promise.all([first, second]);
    expect(cmdMock).toHaveBeenCalledOnce();
    expect(p.git).toEqual(status);
  });

  it("recheck 丢弃记录后真正重扫(路径变更/worktree 合回场景)", async () => {
    const store = useProjectsStore(),
      p = project();
    store.projects = [p];
    cmdMock.mockResolvedValue([{ path: p.path, status }]);
    await store.refreshGitStatus(p);
    await store.refreshGitStatus(p, { force: true, recheck: true });
    expect(cmdMock).toHaveBeenCalledTimes(2);
    gitCheckCoordinator.reset();
  });

  it("批量刷新把结果登记到共享记录,单项目入口紧随其后不再扫描", async () => {
    const store = useProjectsStore(),
      p = project();
    store.projects = [p];
    cmdMock.mockResolvedValueOnce([{ path: p.path, status }]);
    await store.refreshAllGitStatus();
    await store.refreshGitStatus(p);
    expect(cmdMock).toHaveBeenCalledOnce();
    expect(p.git).toEqual(status);
  });

  it("检查失败不写入记录,后续调用仍可重试", async () => {
    const store = useProjectsStore(),
      p = project();
    store.projects = [p];
    cmdMock.mockRejectedValueOnce(new Error("git unavailable"));
    cmdMock.mockResolvedValueOnce([{ path: p.path, status }]);
    await store.refreshGitStatus(p);
    // 失败路径会重试一次,第二次成功即写入记录
    expect(cmdMock).toHaveBeenCalledTimes(2);
    expect(p.git).toEqual(status);
    gitCheckCoordinator.reset();
  });
});
