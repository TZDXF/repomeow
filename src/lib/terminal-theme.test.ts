import { describe, expect, it } from "vitest";
import {
  terminalAnsiColors,
  terminalChromeTheme,
  TERMINAL_MINIMUM_CONTRAST_RATIO,
  TERMINAL_TRANSPARENT_BACKGROUND,
  TERMINAL_TRANSPARENT_OVERVIEW_RULER_BORDER,
} from "./terminal-theme";

function luminance(hex: string): number {
  const channels = hex
    .slice(1)
    .match(/.{2}/g)!
    .map((channel) => {
      const value = parseInt(channel, 16) / 255;
      return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
    });
  return channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722;
}

const lightBackgrounds = [
    ["默认", "#ffffff"],
    ["玻璃", "#e8eef4"],
    ["岛屿", "#f8f8f0"],
    ["像素", "#f4f4f4"],
  ],
  ansiKeys = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "brightBlack",
    "brightRed",
    "brightGreen",
    "brightYellow",
    "brightBlue",
    "brightMagenta",
    "brightCyan",
    "brightWhite",
  ];

describe("终端 ANSI 主题", () => {
  it("亮色模式覆盖完整 16 色,不沿用皮肤中可能过浅的弱化文字色", () => {
    const colors = terminalAnsiColors(false, "#9f927d");
    expect(Object.keys(colors).sort()).toEqual([...ansiKeys].sort());
    expect(colors.brightBlack).not.toBe("#9f927d");
  });

  it.each(lightBackgrounds)("%s亮色皮肤的每种 ANSI 文字色都有足够对比度", (_, background) => {
    const backgroundLuminance = luminance(background);
    for (const [name, color] of Object.entries(terminalAnsiColors(false, "#9f927d"))) {
      const foregroundLuminance = luminance(color),
        contrast =
          (Math.max(backgroundLuminance, foregroundLuminance) + 0.05) /
          (Math.min(backgroundLuminance, foregroundLuminance) + 0.05);
      expect(contrast, name).toBeGreaterThanOrEqual(TERMINAL_MINIMUM_CONTRAST_RATIO);
    }
  });

  it("暗色模式保留默认 ANSI 配色与皮肤弱化文字色", () => {
    expect(terminalAnsiColors(true, "#b1bfd0")).toEqual({ brightBlack: "#b1bfd0" });
  });

  it("主题切换后不会残留或修改亮色调色板", () => {
    const first = terminalAnsiColors(false, "#9f927d");
    first.red = "#ffffff";
    expect(terminalAnsiColors(true, "#b1bfd0")).not.toHaveProperty("red");
    expect(terminalAnsiColors(false, "#9f927d").red).toBe("#b42318");
  });
});

// TZD-144:终端右侧竖直白线来自 xterm 的 overview ruler 左侧 1px 描边。
// 该 ruler 在本应用只为滚动条留宽度(overviewRuler: { width: 10 })、不注册任何装饰;
// overviewRulerBorder 未设置时 xterm 回退成 DEFAULT_FOREGROUND(#ffffff),
// 在透明终端上就是一条白线。下面锁住 chrome 配色一直把它置为全透明。
describe("终端外框配色", () => {
  const darkChrome = { foreground: "rgb(239, 230, 210)", background: "rgb(43, 36, 27)" },
    lightChrome = { foreground: "rgb(31, 35, 40)", background: "rgb(255, 255, 255)" };

  it.each([
    ["暗色", darkChrome],
    ["亮色", lightChrome],
  ])("%s皮肤把 overview ruler 描边置为全透明,不再出现白线", (_, chrome) => {
    // 透明色只能写成带 alpha 的 8 位十六进制/颜色函数;`#ffffff` 之类实色会让画布显形
    expect(terminalChromeTheme(chrome).overviewRulerBorder).toBe(
      TERMINAL_TRANSPARENT_OVERVIEW_RULER_BORDER,
    );
    expect(TERMINAL_TRANSPARENT_OVERVIEW_RULER_BORDER).toMatch(/^#00000000$/i);
  });

  it("终端底色保持全透明,由外层面板与皮肤透出", () => {
    expect(terminalChromeTheme(darkChrome).background).toBe(TERMINAL_TRANSPARENT_BACKGROUND);
    expect(terminalChromeTheme(lightChrome).background).toBe(TERMINAL_TRANSPARENT_BACKGROUND);
  });

  it("滚动条滑块取皮肤令牌,缺省时交给 xterm 自行回退", () => {
    const withTokens = terminalChromeTheme({
      ...darkChrome,
      scrollbar: "rgb(85, 72, 52)",
      scrollbarHover: "rgb(111, 95, 71)",
    });
    expect(withTokens.scrollbarSliderBackground).toBe("rgb(85, 72, 52)");
    expect(withTokens.scrollbarSliderHoverBackground).toBe("rgb(111, 95, 71)");
    expect(withTokens.scrollbarSliderActiveBackground).toBe("rgb(111, 95, 71)");

    const withoutTokens = terminalChromeTheme(darkChrome);
    expect(withoutTokens.scrollbarSliderBackground).toBeUndefined();
    expect(withoutTokens.scrollbarSliderHoverBackground).toBeUndefined();
  });

  it("chrome 配色不携带 ANSI 调色板,合并时不会覆盖皮肤文字色", () => {
    const chrome = terminalChromeTheme(darkChrome);
    expect(chrome).not.toHaveProperty("red");
    expect({ ...chrome, ...terminalAnsiColors(true, "rgb(177, 191, 208)") }).toMatchObject({
      brightBlack: "rgb(177, 191, 208)",
      overviewRulerBorder: TERMINAL_TRANSPARENT_OVERVIEW_RULER_BORDER,
    });
  });
});
