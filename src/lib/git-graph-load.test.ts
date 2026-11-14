import { describe, expect, it, vi } from "vitest";
import { prepareGraphLoad } from "./git-graph-load";

const emptyScope = { local: [] as string[], remote: [] as string[] },
  branches = (remote: string[], local: string[] = ["main"]) => ({ local, remote, tracking: [] });

describe("图谱加载准备:分支列表先于筛选校验", () => {
  it("列表到手后清除已删除的筛选目标", async () => {
    const result = await prepareGraphLoad(
      async () => branches(["origin/main"]),
      emptyScope,
      "current",
      "origin/feature",
    );
    expect(result.branches?.remote).toEqual(["origin/main"]);
    expect(result.correction).toEqual({ branch: "", dropped: "origin/feature" });
  });

  it("列表仍包含该分支时保持筛选", async () => {
    const result = await prepareGraphLoad(
      async () => branches(["origin/main", "origin/feature"]),
      emptyScope,
      "current",
      "origin/feature",
    );
    expect(result.correction).toEqual({ branch: "origin/feature", dropped: null });
  });

  it("列表请求失败不阻断加载:退回旧列表继续校验", async () => {
    const result = await prepareGraphLoad(
      vi.fn(async () => {
        throw new Error("list failed");
      }),
      branches(["origin/main", "origin/feature"]),
      "current",
      "origin/feature",
    );
    expect(result.branches).toBeNull();
    expect(result.correction).toEqual({ branch: "origin/feature", dropped: null });
  });

  it("全部更新清理后即便列表为空也不会保留坏修订", async () => {
    const result = await prepareGraphLoad(
      async () => branches([]),
      emptyScope,
      "current",
      "origin/x",
    );
    expect(result.correction.dropped).toBe("origin/x");
    expect(result.correction.branch).toBe("");
  });
});
