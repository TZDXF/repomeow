import { describe, expect, it } from "vitest";
import { terminalAnsiColors, TERMINAL_MINIMUM_CONTRAST_RATIO } from "./terminal-theme";

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
];
const ansiKeys = [
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
      const foregroundLuminance = luminance(color);
      const contrast =
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
