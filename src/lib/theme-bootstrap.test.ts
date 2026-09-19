import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";

const html = readFileSync(new URL("../../index.html", import.meta.url), "utf8");
const bootstrap = html.match(/<script>([\s\S]*?)<\/script>/)![1];

function applyCache(raw: string | null, systemDark = false) {
  const attributes = new Map<string, string>();
  let dark = false;
  runInNewContext(bootstrap, {
    localStorage: { getItem: () => raw },
    window: { matchMedia: () => ({ matches: systemDark }) },
    document: {
      documentElement: {
        classList: {
          toggle: (_name: string, value: boolean) => {
            dark = value;
          },
        },
        setAttribute: (name: string, value: string) => attributes.set(name, value),
      },
    },
  });
  return { attributes, dark };
}

describe("首帧主题缓存恢复", () => {
  it.each(["light", "dark", "system"])("恢复玻璃皮肤与 %s 模式", (theme) => {
    const result = applyCache(JSON.stringify({ theme, themeSkin: "glassmorphism" }), true);
    expect(result.attributes.get("data-theme")).toBe("glassmorphism");
    expect(result.dark).toBe(theme !== "light");
  });

  it.each(["island", "pixel"])("保留已有 %s 皮肤", (themeSkin) => {
    expect(applyCache(JSON.stringify({ themeSkin })).attributes.get("data-theme")).toBe(themeSkin);
  });

  it.each([null, "broken", '{"themeSkin":"unknown"}', '{"themeSkin":"default"}'])(
    "缓存无效或默认皮肤时不设置主题属性: %s",
    (raw) => {
      expect(applyCache(raw).attributes.has("data-theme")).toBe(false);
    },
  );
});
