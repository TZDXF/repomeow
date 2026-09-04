import { describe, expect, it } from "vitest";
import { createI18n } from "vue-i18n";
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

describe("i18n 消息编译", () => {
  // vue-i18n 消息里 @ / | / {} 是保留语法,裸字符只会在界面首次用到该词条时
  // 才抛 Invalid linked format(把弹窗/页面炸掉),这里全量编译提前拦截
  for (const [locale, messages] of [
    ["zh-CN", zhCN],
    ["en-US", enUS],
  ] as const) {
    it(`${locale} 全部词条均可通过 vue-i18n 消息编译`, () => {
      const i18n = createI18n({
        legacy: false,
        locale,
        missingWarn: false,
        warnHtmlMessage: false,
        messages: { [locale]: messages },
      });
      for (const key of keyPaths(messages as Record<string, unknown>)) {
        expect(() => i18n.global.t(key), key).not.toThrow();
      }
    });
  }
});
