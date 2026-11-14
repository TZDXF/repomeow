import { describe, expect, it } from "vitest";
import { hasBranch, resolveFilterTarget, type BranchScope } from "./git-graph-filter";

const scope = (remote: string[] = ["origin/main"], local: string[] = ["main"]): BranchScope => ({
  local,
  remote,
});
describe("当前分支筛选目标校验", () => {
  it("选中的远程分支仍存在时保持选择", () => {
    expect(
      resolveFilterTarget(scope(["origin/main", "origin/feature"]), "current", "origin/feature"),
    ).toEqual({ branch: "origin/feature", dropped: null });
  });

  it("远程分支被 prune 后清空失效选择并回退当前分支", () => {
    expect(resolveFilterTarget(scope(), "current", "origin/feature")).toEqual({
      branch: "",
      dropped: "origin/feature",
    });
  });

  it("本地分支被删除后同样回退", () => {
    expect(resolveFilterTarget(scope(), "current", "topic")).toEqual({
      branch: "",
      dropped: "topic",
    });
  });

  it("全部分支筛选不受影响(不涉及修订范围)", () => {
    expect(resolveFilterTarget(scope(), "all", "origin/feature")).toEqual({
      branch: "origin/feature",
      dropped: null,
    });
  });

  it("未选择分支时不做校正(回退到当前检出分支由调用方处理)", () => {
    expect(resolveFilterTarget(scope(), "current", "")).toEqual({
      branch: "",
      dropped: null,
    });
  });
});

describe("分支存在性判定", () => {
  it("本地与远程分支都算", () => {
    expect(hasBranch(scope(["origin/dev"]), "origin/dev")).toBe(true);
    expect(hasBranch(scope(["origin/dev"]), "main")).toBe(true);
    expect(hasBranch(scope(["origin/dev"]), "origin/feature")).toBe(false);
  });
});
