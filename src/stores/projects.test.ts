import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { useProjectsStore } from "@/stores/projects";
import type { GitProjectChangedPayload, GitStatus, Project } from "@/types";

vi.mock("@/lib/tauri", () => ({ cmd: vi.fn() }));

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
