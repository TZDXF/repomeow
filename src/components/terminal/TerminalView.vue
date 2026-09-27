<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { resizeCommandSession, writeCommandSession } from "@/lib/terminal";
import { useTerminalStore } from "@/stores/terminal";

/**
 * 单会话 xterm 视图:挂载时回放 store 输出缓存,此后经 onOutput 增量写入。
 * 会话重启(started_at 变化)时重新渲染;由父组件 :key 控制会话切换时的重建。
 */
const props = defineProps<{ sessionId: number }>();
const store = useTerminalStore();

const container = ref<HTMLDivElement | null>(null);
// 容器留白区域(内边距)需与终端自绘背景同色,沿用面板背景会在边缘露出色差
const termBg = ref<string>("transparent");
let term: Terminal | null = null;
let fit: FitAddon | null = null;
let detachOutput: (() => void) | null = null;
let resizeObserver: ResizeObserver | null = null;
let themeObserver: MutationObserver | null = null;
// PTY 尺寸去重:与上次同步值相同则跳过(重启后归零强制重发)
let syncedCols = 0;
let syncedRows = 0;

// 终端配色直接取自应用主题变量(xterm 需要具体色值,不读 CSS 变量),
// 亮/暗色与 pixel / glassmorphism / island 等皮肤都能自动适配。
let colorProbe: HTMLDivElement | null = null;

/** 用探针元素把 var(--xxx)(含 oklch)解析成 xterm 可消费的 rgb() 字符串 */
function resolveThemeColor(varName: string): string | undefined {
  if (!colorProbe) {
    colorProbe = document.createElement("div");
    colorProbe.style.display = "none";
    document.body.appendChild(colorProbe);
  }
  colorProbe.style.color = `var(${varName})`;
  return getComputedStyle(colorProbe).color || undefined;
}

/** 给 rgb() 颜色叠加透明度(选区底色用前景色冲淡,适配任意主题) */
function withAlpha(rgb: string, alpha: number): string {
  const m = rgb.match(/rgba?\(([^)]+)\)/);
  return m ? `rgba(${m[1].split(",").slice(0, 3).join(",")},${alpha})` : rgb;
}

function currentTheme() {
  const isDark = document.documentElement.classList.contains("dark");
  const background = resolveThemeColor("--background") ?? (isDark ? "#0d1117" : "#ffffff");
  const foreground = resolveThemeColor("--foreground") ?? (isDark ? "#e6edf3" : "#1f2328");
  const mutedForeground = resolveThemeColor("--muted-foreground") ?? "#8b949e";
  return {
    background,
    foreground,
    cursor: foreground,
    cursorAccent: background,
    selectionBackground: withAlpha(foreground, 0.25),
    // ANSI 亮黑(常作暗灰提示色)对齐主题的弱化文字色
    brightBlack: mutedForeground,
  };
}

/** 解析当前主题配色,并把背景色同步给容器留白区域 */
function syncTheme() {
  const theme = currentTheme();
  termBg.value = theme.background;
  return theme;
}

function sessionOf(id: number) {
  return store.sessions.find((s) => s.id === id);
}

/** 把 xterm 当前尺寸同步给交互式会话的 PTY(命令会话是管道,无尺寸概念) */
function syncPtySize() {
  if (!term || !sessionOf(props.sessionId)?.interactive) return;
  if (term.cols === syncedCols && term.rows === syncedRows) return;
  syncedCols = term.cols;
  syncedRows = term.rows;
  void resizeCommandSession(props.sessionId, term.rows, term.cols).catch(() => {});
}

/** 清空并回放当前缓存(挂载/重启时调用;缓存为空即只是清屏) */
function renderBuffer() {
  term?.reset();
  const buffered = store.outputOf(props.sessionId);
  if (buffered) term?.write(buffered);
}

onMounted(() => {
  if (!container.value) return;
  term = new Terminal({
    // 命令会话是管道输出(只有 \n):交给 xterm 转成 CRLF 才能正确换行;
    // 交互式会话走 ConPTY,输出已含 \r\n,不能重复转换
    convertEol: !sessionOf(props.sessionId)?.interactive,
    fontSize: 13,
    cursorBlink: false,
    scrollback: 5000,
    theme: syncTheme(),
  });
  fit = new FitAddon();
  term.loadAddon(fit);
  term.open(container.value);
  fit.fit();
  syncPtySize();
  renderBuffer();
  term.focus();

  detachOutput = store.onOutput((id, chunk) => {
    if (id === props.sessionId) term?.write(chunk);
  });

  /** 键盘输入与剪贴板粘贴共用的写入逻辑 */
  function handleInput(data: string) {
    const session = sessionOf(props.sessionId);
    if (session?.status !== "running") return;
    // 交互式 PTY 会话原样透传:回显/行编辑/^C 展示全部由 shell 自绘;
    // 命令会话(管道)无回显,本地补显键入字符,便于向运行中的进程回答提示。
    void writeCommandSession(props.sessionId, data).catch(() => {});
    if (!session.interactive) {
      if (data === "\r") term?.write("\r\n");
      else if (data >= " ") term?.write(data);
    }
  }

  term.onData(handleInput);

  // 复制粘贴:有选区时 Ctrl/Cmd+C 复制选区(无选区保持 ^C 语义);
  // Ctrl/Cmd+V 读取剪贴板走与键盘输入相同的写入路径(支持多行逐行提交)。
  term.attachCustomKeyEventHandler((e) => {
    if (e.type !== "keydown" || !(e.ctrlKey || e.metaKey)) return true;
    const key = e.key.toLowerCase();
    if (key === "c" && term?.hasSelection()) {
      void navigator.clipboard.writeText(term.getSelection()).catch(() => {});
      return false;
    }
    if (key === "v") {
      void navigator.clipboard
        .readText()
        .then((text) => {
          if (text) handleInput(text.replace(/\r\n/g, "\n"));
        })
        .catch(() => {});
      return false;
    }
    return true;
  });

  resizeObserver = new ResizeObserver(() => {
    fit?.fit();
    syncPtySize();
  });
  resizeObserver.observe(container.value);

  themeObserver = new MutationObserver(() => {
    if (term) term.options.theme = syncTheme();
  });
  themeObserver.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ["class", "data-theme"],
  });
});

// 会话重启:store 已按 started_at 变化清空缓存,这里整屏重渲染,
// 并把尺寸重发给重新拉起的 PTY(归零去重标记强制重发)
watch(
  () => sessionOf(props.sessionId)?.started_at,
  () => {
    syncedCols = 0;
    syncedRows = 0;
    syncPtySize();
    renderBuffer();
  },
);

onBeforeUnmount(() => {
  detachOutput?.();
  resizeObserver?.disconnect();
  themeObserver?.disconnect();
  term?.dispose();
  colorProbe?.remove();
  colorProbe = null;
});
</script>

<template>
  <div
    ref="container"
    class="h-full w-full overflow-hidden p-2"
    :style="{ backgroundColor: termBg }"
  />
</template>
