<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { writeCommandSession } from "@/lib/terminal";
import { useTerminalStore } from "@/stores/terminal";

/**
 * 单会话 xterm 视图:挂载时回放 store 输出缓存,此后经 onOutput 增量写入。
 * 会话重启(started_at 变化)时重新渲染;由父组件 :key 控制会话切换时的重建。
 */
const props = defineProps<{ sessionId: number }>();
const store = useTerminalStore();

const container = ref<HTMLDivElement | null>(null);
let term: Terminal | null = null;
let fit: FitAddon | null = null;
let detachOutput: (() => void) | null = null;
let resizeObserver: ResizeObserver | null = null;
let themeObserver: MutationObserver | null = null;
let inputLine = "";

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

/** 清空并回放当前缓存(挂载/重启时调用;缓存为空即只是清屏) */
function renderBuffer() {
  term?.reset();
  const buffered = store.outputOf(props.sessionId);
  if (buffered) term?.write(buffered);
}

onMounted(() => {
  if (!container.value) return;
  term = new Terminal({
    // 子进程是管道而非 PTY,输出只含 \n:交给 xterm 转成 CRLF 才能正确换行
    convertEol: true,
    fontSize: 13,
    cursorBlink: false,
    scrollback: 5000,
    theme: currentTheme(),
  });
  fit = new FitAddon();
  term.loadAddon(fit);
  term.open(container.value);
  fit.fit();
  renderBuffer();
  term.focus();

  detachOutput = store.onOutput((id, chunk) => {
    if (id === props.sessionId) term?.write(chunk);
  });

  term.onData((data) => {
    const session = store.sessions.find((s) => s.id === props.sessionId);
    if (session?.status !== "running") return;
    if (session.interactive) {
      // 管道不是 PTY:在前端逐行编辑,回车时整行写入 Shell stdin。
      for (const ch of data) {
        if (ch === "\r" || ch === "\n") {
          void writeCommandSession(props.sessionId, `${inputLine}\n`).catch(() => {});
          inputLine = "";
          term?.write("\r\n");
        } else if (ch === "\x7f" || ch === "\b") {
          if (inputLine) {
            inputLine = Array.from(inputLine).slice(0, -1).join("");
            term?.write("\b \b");
          }
        } else if (ch === "\x03") {
          inputLine = "";
          term?.write("^C\r\n");
        } else if (ch >= " " && ch !== "\x7f") {
          inputLine += ch;
          term?.write(ch);
        }
      }
    } else {
      void writeCommandSession(props.sessionId, data).catch(() => {});
      // 一次性命令会话沿用即时写入,便于向运行中的进程回答提示。
      if (data === "\r") term?.write("\r\n");
      else if (data >= " ") term?.write(data);
    }
  });

  resizeObserver = new ResizeObserver(() => fit?.fit());
  resizeObserver.observe(container.value);

  themeObserver = new MutationObserver(() => {
    if (term) term.options.theme = currentTheme();
  });
  themeObserver.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ["class", "data-theme"],
  });
});

// 会话重启:store 已按 started_at 变化清空缓存,这里整屏重渲染
watch(
  () => store.sessions.find((s) => s.id === props.sessionId)?.started_at,
  () => {
    inputLine = "";
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
  <div ref="container" class="h-full w-full overflow-hidden px-2 py-1" />
</template>
