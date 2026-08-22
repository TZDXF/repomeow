import { describe, expect, it } from "vitest";
import { formatCompactNumber } from "./format";

describe("formatCompactNumber", () => {
  it("小于 1000 时显示完整数字", () => {
    expect(formatCompactNumber(0)).toBe("0");
    expect(formatCompactNumber(999)).toBe("999");
  });

  it("使用 K、M、B 缩写并保留一位有效小数", () => {
    expect(formatCompactNumber(1_000)).toBe("1K");
    expect(formatCompactNumber(12_500)).toBe("12.5K");
    expect(formatCompactNumber(1_250_000)).toBe("1.3M");
    expect(formatCompactNumber(5_600_000_000)).toBe("5.6B");
  });

  it("舍入到单位上界时提升到下一级", () => {
    expect(formatCompactNumber(999_999)).toBe("1M");
    expect(formatCompactNumber(999_999_999)).toBe("1B");
  });
});
