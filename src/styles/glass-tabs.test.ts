import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// 回归守卫(node 环境无法做真实渲染):
// 玻璃拟态 / 玻璃拟态 Pro 下 bg-accent(白 14% / 薰衣草 18%)与未选中底几乎同价,
// 标题栏项目 tab、终端页签、资源库 tab、分段控件选中项都看不出当前项。
// 这里锁住三件事:1) 激活态令牌亮/暗各有一套且落在两种玻璃皮肤作用域内;
// 2) 统一激活规则同时覆盖 ARIA 页签、分段控件与 data-tab-active 挂点;
// 3) 原生按钮页签确实带上了 data-tab-active 挂点,否则样式无处可依。
const glassCss = readFileSync(new URL("./glassmorphism.css", import.meta.url), "utf8");

const componentSources = {
  "标题栏 tab / 「更多」弹层": "./../components/common/TitleTabs.vue",
  终端页签: "./../components/terminal/TerminalPanel.vue",
  语义关系面板: "./../components/files/SemanticRelationPanel.vue",
  语义影响面板: "./../components/semantic/SemanticImpactPanel.vue",
} as const;

describe("玻璃主题 Tab 激活态", () => {
  it("亮色与暗色各定义一套 --tab-active-* 令牌", () => {
    // 声明必须正好两次(亮色块 + .dark 块),否则暗色会退回亮色取值
    for (const token of [
      "--tab-active-surface",
      "--tab-active-foreground",
      "--tab-active-ring",
      "--tab-active-highlight",
    ]) {
      expect(glassCss.match(new RegExp(`${token}:`, "g")) ?? [], token).toHaveLength(2);
    }
  });

  it("激活规则限定在两种玻璃皮肤作用域,不改其他主题", () => {
    const block = glassCss.slice(glassCss.indexOf("/* Tab 激活态统一规则"));
    expect(block).toMatch(
      /:is\(\[data-theme="glassmorphism"\], \[data-theme="glass-pro"\]\)\s*:is\(/,
    );
    expect(block).toMatch(/\[role="tab"\]\[aria-selected="true"\]/);
    expect(block).toMatch(/\.segmented-control\s*>\s*:is\(\[aria-pressed="true"\]/);
    expect(block).toMatch(/\[data-tab-active\]/);
    // 激活态必须由主题令牌驱动,不能回落到 accent
    expect(block).toMatch(/background-color: var\(--tab-active-surface\)/);
    expect(block).toMatch(/inset 0 0 0 1px var\(--tab-active-ring\)/);
  });

  it("无 ARIA 选中语义的按钮页签带 data-tab-active 挂点", () => {
    for (const [label, relative] of Object.entries(componentSources)) {
      const source = readFileSync(new URL(relative, import.meta.url), "utf8");
      expect(source, `${label} 缺少 data-tab-active 挂点`).toMatch(
        /:data-tab-active="[^"]+ \|\| undefined"/,
      );
    }
  });
});
