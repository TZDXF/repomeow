import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const source = readFileSync(new URL("./ProjectResourceAddDialog.vue", import.meta.url), "utf8");

// 缓存页中的模态层必须显式卸载;只更新 open 仍会等待退出动画。
// node 环境检查此生命周期约束,实际 pointer-events 恢复需在 WebView 中回归。
describe("添加项目资源后跳转资源管理", () => {
  it("主动关闭时卸载 Dialog,不依赖退出动画清理模态层", () => {
    expect(source).toMatch(/<Dialog\s+v-if="open"\s+:open="open"/);
  });

  it("先关闭并等待卸载刷新,再导航到资源管理", () => {
    expect(source).toMatch(
      /async function manage\(\)\s*\{\s*open\.value = false;\s*\/\/[^\n]*\n\s*await nextTick\(\);\s*await router\.push\(\{ name: "settings", query: \{ category: "resources" \} \}\);\s*\}/,
    );
  });
});
