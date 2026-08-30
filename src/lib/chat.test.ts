import { describe, expect, it } from "vitest";
import { formatTokenCount } from "@/lib/chat";

describe("formatTokenCount", () => {
  it("小数值直接显示整数", () => {
    expect(formatTokenCount(0)).toBe("0");
    expect(formatTokenCount(-5)).toBe("0");
    expect(formatTokenCount(999)).toBe("999");
  });

  it("千位区间保留一位小数并去掉 .0 尾巴", () => {
    expect(formatTokenCount(1000)).toBe("1k");
    expect(formatTokenCount(1234)).toBe("1.2k");
    expect(formatTokenCount(100000)).toBe("100k");
    expect(formatTokenCount(131072)).toBe("131.1k");
  });

  it("百万位区间换算为 M", () => {
    expect(formatTokenCount(1000000)).toBe("1M");
    expect(formatTokenCount(1048576)).toBe("1M");
    expect(formatTokenCount(1280000)).toBe("1.3M");
  });

  it("非有限值回退 0", () => {
    expect(formatTokenCount(Number.NaN)).toBe("0");
    expect(formatTokenCount(Number.POSITIVE_INFINITY)).toBe("0");
  });
});
