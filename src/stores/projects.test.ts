import { beforeEach, describe, expect, it, vi, type Mock } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { useProjectsStore } from "@/stores/projects";
import type { GitProjectChangedPayload, GitStatus, Project } from "@/types";

// node 环境无 Tauri IPC,mock 掉前后端桥
vi.mock("@/lib/tauri", () => ({ cmd: vi.fn() }));

import { cmd } from "@/lib/tauri";
const cmdMock = cmd as unknown as Mock;

const status: GitStatus = {
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
