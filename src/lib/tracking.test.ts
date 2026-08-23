import { describe, expect, it } from "vitest";
import { matchesTrackingProject } from "@/lib/tracking";
import type { Project } from "@/types";

const project = {
  name: "RepoMeow",
  description: "桌面项目管理中心",
  path: "D:\\code\\project-dev",
  tags: [
    { id: 1, name: "Vue", color: "#42b883" },
    { id: 2, name: "Tauri", color: "#ffc131" },
  ],
} as Project;

describe("matchesTrackingProject", () => {
  it("匹配名称、简介、路径和标签", () => {
    expect(matchesTrackingProject(project, "meow")).toBe(true);
    expect(matchesTrackingProject(project, "管理中心")).toBe(true);
    expect(matchesTrackingProject(project, "project-dev")).toBe(true);
    expect(matchesTrackingProject(project, "tauri")).toBe(true);
  });

  it("多个搜索词按 AND 组合且不改变项目数据", () => {
    expect(matchesTrackingProject(project, "vue meow")).toBe(true);
    expect(matchesTrackingProject(project, "vue rust-only")).toBe(false);
    expect(project.tags.map((tag) => tag.name)).toEqual(["Vue", "Tauri"]);
  });
});
