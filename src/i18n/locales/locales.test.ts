import { describe, expect, it } from "vitest";
import zhCN from "./zh-CN";
import enUS from "./en-US";

/** 收集嵌套词条的完整键路径;数组视为叶子值 */
function keyPaths(obj: Record<string, unknown>, prefix = ""): string[] {
  return Object.entries(obj).flatMap(([key, value]) =>
    value !== null && typeof value === "object" && !Array.isArray(value)
      ? keyPaths(value as Record<string, unknown>, `${prefix}${key}.`)
      : [`${prefix}${key}`],
  );
}

describe("i18n 词条对齐", () => {
  it("zh-CN 与 en-US 键集合完全一致", () => {
    const zh = keyPaths(zhCN as Record<string, unknown>);
    const en = keyPaths(enUS as Record<string, unknown>);
    expect(zh.filter((k) => !en.includes(k))).toEqual([]);
    expect(en.filter((k) => !zh.includes(k))).toEqual([]);
  });
});
