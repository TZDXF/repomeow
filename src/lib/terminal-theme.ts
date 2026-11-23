import type { ITheme } from "@xterm/xterm";

// 除 16 色调色板外,也校正命令指定的 256 色 / RGB 文字与实际背景的对比度。
export const TERMINAL_MINIMUM_CONTRAST_RATIO = 4.5;

/** 终端底始终透明,让外层面板背景与皮肤直接透出 */
export const TERMINAL_TRANSPARENT_BACKGROUND = "#00000000";

/**
 * overview ruler 在本应用只用来给滚动条留出宽度(不注册任何终端装饰);
 * 但 xterm 总会用 `overviewRulerBorder` 在它左侧画 1px 描边,未设置时回退到
 * DEFAULT_FOREGROUND(#ffffff),表现为透明终端右侧一条白线。置为完全透明,让画布不可见。
 */
export const TERMINAL_TRANSPARENT_OVERVIEW_RULER_BORDER = "#00000000";

/** 已解析成 rgb() 的主题令牌(组件用探针把 CSS 变量换算成具体色值后传入) */
export interface TerminalChromeColors {
  foreground: string;
  background: string;
  scrollbar?: string;
  scrollbarHover?: string;
}

/** 给 rgb() 颜色叠加透明度(选区底色用前景色冲淡,适配任意主题) */
export function withAlpha(rgb: string, alpha: number): string {
  const m = rgb.match(/rgba?\((?<channels>[^)]+)\)/);
  return m ? `rgba(${m.groups!.channels.split(",").slice(0, 3).join(",")},${alpha})` : rgb;
}

/** 终端非 ANSI 的 chrome 配色:透明底、皮肤前景与滚动条、不可见 ruler 描边 */
export function terminalChromeTheme({
  foreground,
  background,
  scrollbar,
  scrollbarHover,
}: TerminalChromeColors): ITheme {
  return {
    background: TERMINAL_TRANSPARENT_BACKGROUND,
    foreground,
    cursor: foreground,
    cursorAccent: background,
    selectionBackground: withAlpha(foreground, 0.25),
    scrollbarSliderBackground: scrollbar,
    // hover/active 保持旧语义:只认 --scrollbar-thumb-hover,缺省时留 undefined 交给 xterm
    // 回退(前景色 40% / 50% 透明度),不要回退到普通滑块色,否则悬停/按下反馈会消失。
    scrollbarSliderHoverBackground: scrollbarHover,
    scrollbarSliderActiveBackground: scrollbarHover,
    overviewRulerBorder: TERMINAL_TRANSPARENT_OVERVIEW_RULER_BORDER,
  };
}

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
