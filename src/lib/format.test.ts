import { describe, expect, it } from "vitest";
import { formatAuditDate, formatCompactNumber } from "./format";

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

describe("formatAuditDate", () => {
  it("解析英文审计日期并按当前语言本地化(默认 zh-CN)", () => {
    expect(formatAuditDate("Mar 15, 2026")).toBe("2026年3月15日");
    expect(formatAuditDate("January 1, 2026")).toBe("2026年1月1日");
  });

  it("全月名与多余空白也能解析", () => {
    expect(formatAuditDate("  December  25, 2025 ")).toBe("2025年12月25日");
  });

  it("无法解析或日期无效时回退原串", () => {
    expect(formatAuditDate("unknown")).toBe("unknown");
    expect(formatAuditDate("")).toBe("");
  });
});
