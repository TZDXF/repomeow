import { describe, expect, it } from "vitest";
import { themePreviewSplit } from "./theme-preview";

describe("themePreviewSplit", () => {
  it("根据选项自身的位置与宽度计算分割比例", () => {
    expect(themePreviewSplit(100, 100, 400)).toBe(0);
    expect(themePreviewSplit(200, 100, 400)).toBe(25);
    expect(themePreviewSplit(300, 100, 400)).toBe(50);
    expect(themePreviewSplit(500, 100, 400)).toBe(100);
  });

  it("将越界坐标限制在选项范围内", () => {
    expect(themePreviewSplit(50, 100, 400)).toBe(0);
    expect(themePreviewSplit(550, 100, 400)).toBe(100);
  });

  it("没有有效宽度时保持左右各半", () => {
    expect(themePreviewSplit(100, 100, 0)).toBe(50);
    expect(themePreviewSplit(100, 100, -1)).toBe(50);
  });
});
