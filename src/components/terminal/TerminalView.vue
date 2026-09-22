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

// 随应用明暗主题切换的两套配色(xterm 需要具体色值,不读 CSS 变量)
const LIGHT_THEME = {
  background: "#ffffff",
  foreground: "#1f2328",
  cursor: "#1f2328",
  selectionBackground: "#b6d7ff",
};
const DARK_THEME = {
  background: "#0d1117",
  foreground: "#e6edf3",
  cursor: "#e6edf3",
  selectionBackground: "#264f78",
};

function currentTheme() {
  return document.documentElement.classList.contains("dark") ? DARK_THEME : LIGHT_THEME;
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

  detachOutput = store.onOutput((id, chunk) => {
    if (id === props.sessionId) term?.write(chunk);
  });

  term.onData((data) => {
    void writeCommandSession(props.sessionId, data).catch(() => {});
    // 管道 stdin 无 TTY 回显:本地回显可打印字符与回车,交互式输入才可见
    if (data === "\r") term?.write("\r\n");
    else if (data >= " ") term?.write(data);
  });

  resizeObserver = new ResizeObserver(() => fit?.fit());
  resizeObserver.observe(container.value);

  themeObserver = new MutationObserver(() => {
    if (term) term.options.theme = currentTheme();
  });
  themeObserver.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ["class"],
  });
});

// 会话重启:store 已按 started_at 变化清空缓存,这里整屏重渲染
watch(
  () => store.sessions.find((s) => s.id === props.sessionId)?.started_at,
  () => renderBuffer(),
);

onBeforeUnmount(() => {
  detachOutput?.();
  resizeObserver?.disconnect();
  themeObserver?.disconnect();
  term?.dispose();
});
</script>

<template>
  <div ref="container" class="h-full w-full overflow-hidden px-2 py-1" />
</template>
