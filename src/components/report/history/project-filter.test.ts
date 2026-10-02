import { describe, expect, it } from "vitest";
import { filterProjectsByKeyword } from "./project-filter";
import type { Project } from "@/types";

function makeProject(id: number, name: string, path: string): Project {
  return { id, name, path } as Project;
}

const projects = [
  makeProject(1, "RepoMeow", "C:\\code\\repomeow"),
  makeProject(2, "Docs Site", "/home/user/docs-site"),
  makeProject(3, "API", "D:\\services\\api-gateway"),
];

describe("filterProjectsByKeyword", () => {
  it("空关键词(含纯空白)返回全部项目", () => {
    expect(filterProjectsByKeyword(projects, "")).toHaveLength(3);
    expect(filterProjectsByKeyword(projects, "   ")).toHaveLength(3);
  });

  it("按名称匹配,忽略大小写", () => {
    expect(filterProjectsByKeyword(projects, "repomeow").map((p) => p.id)).toEqual([1]);
    expect(filterProjectsByKeyword(projects, "DOCS").map((p) => p.id)).toEqual([2]);
  });

  it("按路径匹配(Windows 反斜杠与 POSIX 路径均可)", () => {
    expect(filterProjectsByKeyword(projects, "api-gateway").map((p) => p.id)).toEqual([3]);
    expect(filterProjectsByKeyword(projects, "c:\\code").map((p) => p.id)).toEqual([1]);
  });

  it("名称或路径任一命中即保留", () => {
    expect(filterProjectsByKeyword(projects, "docs").map((p) => p.id)).toEqual([2]);
  });

  it("无命中返回空数组", () => {
    expect(filterProjectsByKeyword(projects, "nonexistent")).toEqual([]);
  });
});
