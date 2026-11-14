import { describe, expect, it } from "vitest";
import { revealCommit, toggleDetailByCommit } from "./git-graph-detail";

describe("提交图谱详情收起/展开", () => {
  it("点另一条记录:选中并展开", () => {
    expect(toggleDetailByCommit({ selectedHash: "a", detailOpen: false }, "b")).toEqual({
      selectedHash: "b",
      detailOpen: true,
    });
    expect(toggleDetailByCommit({ selectedHash: null, detailOpen: false }, "a")).toEqual({
      selectedHash: "a",
      detailOpen: true,
    });
  });

  it("点已选中记录且详情展开:收起但保留选中(高亮与窄条仍在)", () => {
    expect(toggleDetailByCommit({ selectedHash: "a", detailOpen: true }, "a")).toEqual({
      selectedHash: "a",
      detailOpen: false,
    });
  });

  it("点已选中记录且详情已收起:再次点击展开", () => {
    expect(toggleDetailByCommit({ selectedHash: "a", detailOpen: false }, "a")).toEqual({
      selectedHash: "a",
      detailOpen: true,
    });
  });

  it("收起后点任意记录都展开", () => {
    for (const hash of ["a", "b"]) {
      expect(toggleDetailByCommit({ selectedHash: "a", detailOpen: false }, hash).detailOpen).toBe(
        true,
      );
    }
  });

  it("主动收起后定位(搜索/标签/?commit=)强制展开", () => {
    expect(revealCommit("a")).toEqual({ selectedHash: "a", detailOpen: true });
  });
});
