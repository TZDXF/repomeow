import type { ITheme } from "@xterm/xterm";

// 除 16 色调色板外,也校正命令指定的 256 色 / RGB 文字与实际背景的对比度。
export const TERMINAL_MINIMUM_CONTRAST_RATIO = 4.5;

// 亮色模式不能沿用面向黑底的 ANSI 默认色,尤其是黄、青、白与 bright 系列。
// 保留颜色语义,同时让普通色与加粗时使用的 bright 色在浅色皮肤下都可读。
const LIGHT_ANSI_COLORS: ITheme = {
  black: "#1f2328",
  red: "#b42318",
  green: "#176b35",
  yellow: "#805500",
  blue: "#185abd",
  magenta: "#8f298f",
  cyan: "#086575",
  white: "#4b5563",
  brightBlack: "#59636e",
  brightRed: "#a31515",
  brightGreen: "#14632e",
  brightYellow: "#754d00",
  brightBlue: "#174ea6",
  brightMagenta: "#7e2288",
  brightCyan: "#075b61",
  brightWhite: "#374151",
};

export function terminalAnsiColors(isDark: boolean, mutedForeground: string): ITheme {
  // 暗色保留 xterm 默认调色板,弱化文字继续跟随应用皮肤。
  return isDark ? { brightBlack: mutedForeground } : { ...LIGHT_ANSI_COLORS };
}
