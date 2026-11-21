import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// 回归守卫(node 环境无法做真实渲染,但可以从主题令牌解析出实际生效的填充/文字色):
// 玻璃拟态 / 玻璃拟态 Pro 下 bg-accent(白 14% / 薰衣草 18%)与未选中底几乎同价,
// 标题栏项目 tab、终端页签、资源库 tab、分段控件选中项都看不出当前项,激活态改用
// --tab-active-* 主题令牌标记。这里锁住四件事:
// 1) 令牌亮/暗各有一套,且落在两种玻璃皮肤作用域内;
// 2) 统一激活规则同时覆盖 ARIA 页签、分段控件与 data-tab-active 挂点;
// 3) 两种主题 × 亮/暗四种组合,普通态与悬停态的「文字 vs 填充」对比度均 ≥ 4.5:1;
// 4) 悬停不换底:只加强描边环与内高光,否则对比度会被拉低(见下方注释)。
const glassCss = readFileSync(new URL("./glassmorphism.css", import.meta.url), "utf8"),
  glassProCss = readFileSync(new URL("./glass-pro.css", import.meta.url), "utf8");

const componentSources = {
  "标题栏 tab / 「更多」弹层": "./../components/common/TitleTabs.vue",
  终端页签: "./../components/terminal/TerminalPanel.vue",
  语义关系面板: "./../components/files/SemanticRelationPanel.vue",
  语义影响面板: "./../components/semantic/SemanticImpactPanel.vue",
} as const;

/** WCAG 相对亮度 */
function luminance([r, g, b]: number[]): number {
  const channel = (value: number) => {
    const v = value / 255;
    return v <= 0.039_28 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(r!) + 0.7152 * channel(g!) + 0.0722 * channel(b!);
}

/** WCAG 对比度 */
function contrast(foreground: number[], background: number[]): number {
  const [l1, l2] = [luminance(foreground), luminance(background)].sort((a, b) => b - a);
  return (l1! + 0.05) / (l2! + 0.05);
}

/** 从 CSS 里取出「选择器含 marker、且含 .dark 与否」的声明块 */
function tokenBlocks(css: string, marker: string, dark: boolean): string[] {
  return [...css.matchAll(/(?<selector>[^{}]+)\{(?<body>[^{}]*)\}/g)]
    .filter(
      ({ groups }) =>
        groups!.selector!.includes(marker) && groups!.selector!.includes(".dark") === dark,
    )
    .map(({ groups }) => groups!.body!);
}

/** 取 --name 的声明值;后出现的块覆盖前面的(与浏览器层叠一致) */
function declarations(blocks: string[]): Map<string, string> {
  const map = new Map<string, string>();
  for (const block of blocks) {
    for (const { groups } of block.matchAll(/(?<name>--[a-z0-9-]+)\s*:\s*(?<value>[^;]+);/g)) {
      map.set(groups!.name!, groups!.value!.trim());
    }
  }
  return map;
}

/** 按顶层逗号切分参数(忽略括号内的逗号),用于解析 color-mix 的内层函数 */
function splitTopLevel(text: string): string[] {
  const out: string[] = [];
  let depth = 0,
    current = "";
  for (const char of text) {
    if (char === "(") {
      depth += 1;
    } else if (char === ")") {
      depth -= 1;
    }
    if (char === "," && depth === 0) {
      out.push(current);
      current = "";
      continue;
    }
    current += char;
  }
  out.push(current);
  return out;
}

const NAMED_COLORS: Record<string, number[]> = {
  transparent: [0, 0, 0],
  white: [255, 255, 255],
  black: [0, 0, 0],
};

/**
 * 解析 hex / rgb() / var() / color-mix(),返回已叠到不透明的 [r, g, b]。
 * known:已被外部解析过的变量名 → 颜色,用于悬停规则里复用 --tab-active-surface 的情况。
 */
function resolve(
  value: string,
  vars: Map<string, string>,
  known = new Map<string, number[]>(),
  depth = 0,
): number[] {
  if (depth > 12) {
    throw new Error(`变量解析层级过深: ${value}`);
  }
  const text = value.trim();

  const variable = text.match(/^var\((?<name>--[a-z0-9-]+)/);
  if (variable) {
    const name = variable.groups!.name!,
      cached = known.get(name);
    if (cached) {
      return cached;
    }
    const declared = vars.get(name);
    if (!declared) {
      throw new Error(`未定义的变量 ${name}`);
    }
    return resolve(declared, vars, known, depth + 1);
  }

  if (text.startsWith("color-mix(")) {
    // 内层 var()/rgb() 自带括号,不能靠正则切逗号:按括号深度切出顶层参数
    const parts = splitTopLevel(text.slice("color-mix(".length, -1));
    if (parts.length !== 3 || !parts[0]!.trim().startsWith("in srgb")) {
      throw new Error(`无法解析的 color-mix: ${text}`);
    }
    const firstPart = parts[1]!.trim(),
      weighted = firstPart.match(/^(?<color>.*?)(?<weight>\d+(?:\.\d+)?)%$/);
    if (!weighted) {
      throw new Error(`color-mix 缺少百分比权重: ${firstPart}`);
    }
    const weight = Number(weighted.groups!.weight) / 100,
      first = resolve(weighted.groups!.color!.trim(), vars, known, depth + 1),
      second = resolve(parts[2]!.trim(), vars, known, depth + 1);
    return first.map((channel, index) => channel * weight + (second[index] ?? 0) * (1 - weight));
  }

  const hex = text.match(/^#(?<hex>[0-9a-f]{6})$/i);
  if (hex) {
    const raw = hex.groups!.hex!;
    return [0, 2, 4].map((offset) => Number.parseInt(raw.slice(offset, offset + 2), 16));
  }

  const rgb = text.match(
    /^rgb\(\s*(?<r>\d+)\s+(?<g>\d+)\s+(?<b>\d+)(?:\s*\/\s*(?<a>\d+)%?)?\s*\)$/,
  );
  if (rgb) {
    const [r, g, b] = [rgb.groups!.r, rgb.groups!.g, rgb.groups!.b].map(Number) as [
      number,
      number,
      number,
    ];
    let alpha = 1;
    if (rgb.groups!.a !== undefined) {
      // rgb 的 alpha 可能写成百分比(22%)或 0–1 小数,两种都接受
      const raw = Number(rgb.groups!.a);
      alpha = raw > 1 ? raw / 100 : raw;
    }
    // 主题里带 alpha 的颜色都叠在不透明面上;alpha 只出现在描边环与内高光,不参与本断言
    return [
      r * alpha + 255 * (1 - alpha),
      g * alpha + 255 * (1 - alpha),
      b * alpha + 255 * (1 - alpha),
    ];
  }

  const named = NAMED_COLORS[text.toLowerCase()];
  if (named) {
    return named;
  }
  throw new Error(`无法解析的颜色值: ${value}`);
}

/** 两套主题 × 亮/暗的生效令牌(glass-pro.css 在 glassmorphism.css 之后导入,覆盖前者) */
const themeVars = {
  玻璃拟态亮: declarations(tokenBlocks(glassCss, '[data-theme="glassmorphism"]', false)),
  玻璃拟态暗: declarations(tokenBlocks(glassCss, '[data-theme="glassmorphism"]', true)),
  "玻璃拟态 Pro 亮": declarations([
    ...tokenBlocks(glassCss, '[data-theme="glass-pro"]', false),
    ...tokenBlocks(glassProCss, '[data-theme="glass-pro"]', false),
  ]),
  "玻璃拟态 Pro 暗": declarations([
    ...tokenBlocks(glassCss, '[data-theme="glass-pro"]', true),
    ...tokenBlocks(glassProCss, '[data-theme="glass-pro"]', true),
  ]),
};

const hoverRule = glassCss.slice(
  glassCss.indexOf("/* 激活项悬停:"),
  glassCss.indexOf("/* 高光阴影不应吞掉"),
);

/** 悬停规则里对激活项实际声明的 background-color / color(没有则沿用普通态) */
const hoverDeclarations = new Map(
  [...hoverRule.matchAll(/(?<property>background-color|color)\s*:\s*(?<value>[^;]+);/g)].map(
    ({ groups }) => [groups!.property!, groups!.value!.trim()],
  ),
);

describe("玻璃主题 Tab 激活态", () => {
  it("亮色与暗色各定义一套 --tab-active-* 令牌", () => {
    // 声明必须正好两次(亮色块 + .dark 块),否则暗色会退回亮色取值
    for (const token of [
      "--tab-active-surface",
      "--tab-active-foreground",
      "--tab-active-ring",
      "--tab-active-highlight",
      "--tab-active-ring-hover",
      "--tab-active-inner-glow",
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

  it("四种主题/明暗组合:普通态与悬停态文字对比度均 ≥ 4.5:1", () => {
    for (const [label, vars] of Object.entries(themeVars)) {
      const known = new Map<string, number[]>(),
        surface = resolve(vars.get("--tab-active-surface")!, vars, known),
        foreground = resolve(vars.get("--tab-active-foreground")!, vars, known);
      // 悬停规则可能复用 --tab-active-surface / --tab-active-foreground,先登记普通态取值
      known.set("--tab-active-surface", surface);
      known.set("--tab-active-foreground", foreground);

      for (const state of ["普通", "悬停"] as const) {
        // 悬停态把悬停规则里真实声明的 background-color/color 叠上去(没声明就沿用普通态),
        // 这样「不换底」与「换底并同步调前景色」两种实现都会被按实际取值校验
        const hovered = state === "悬停",
          stateSurface =
            hovered && hoverDeclarations.has("background-color")
              ? resolve(hoverDeclarations.get("background-color")!, vars, known)
              : surface,
          stateForeground =
            hovered && hoverDeclarations.has("color")
              ? resolve(hoverDeclarations.get("color")!, vars, known)
              : foreground,
          ratio = contrast(stateForeground, stateSurface);
        expect(ratio, `${label} ${state}态文字对比度 ${ratio.toFixed(2)}:1`).toBeGreaterThanOrEqual(
          4.5,
        );
      }
    }
  });

  it("悬停沿用普通态的填充与文字令牌,只加强描边环与内高光", () => {
    // 悬停若把主色再混进激活底,文字对比度会掉到 4.5:1 以下(四种组合无一例外);
    // 因此悬停允许重申 background-color/color(抵消组件自带的 hover:bg-accent 工具类),
    // 但取值必须与普通态令牌完全一致,不能重新混合。
    expect(hoverDeclarations.get("background-color")).toBe("var(--tab-active-surface)");
    expect(hoverDeclarations.get("color")).toBe("var(--tab-active-foreground)");
    for (const value of hoverDeclarations.values()) {
      expect(value, `悬停取值不应重新混合: ${value}`).not.toMatch(/color-mix\(/);
    }
    expect(hoverRule).toMatch(/box-shadow:/);
    expect(hoverRule).toMatch(/var\(--tab-active-ring-hover\)/);
    expect(hoverRule).toMatch(/var\(--tab-active-inner-glow\)/);

    // 悬停用到的两个新令牌在两种主题的亮/暗块里都必须定义
    for (const token of ["--tab-active-ring-hover", "--tab-active-inner-glow"]) {
      for (const [label, vars] of Object.entries(themeVars)) {
        expect(vars.has(token), `${label} 缺少 ${token}`).toBe(true);
      }
    }
  });
});
