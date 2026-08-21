<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import { File as PierreFile } from "@pierre/diffs";
import { PIERRE_THEMES, usePierreDark } from "./use-pierre-theme";
import { buildFindRegExp, collectMatches, type FindQuery, type TextRange } from "@/lib/text-search";

/**
 * @pierre/diffs 的 Vue 薄包装:单文件只读代码查看(文件预览专用)。
 * 语法高亮(Shiki)/ 行号 / 滚动均由库承担;文件内查找在 props.text 上跑
 * (与全文搜索同一套 text-search 辅助),当前匹配行经 setSelectedLines 强调并滚动定位。
 */
const props = defineProps<{ text: string; path: string; wrap: boolean }>();

const host = ref<HTMLElement | null>(null);
const isDark = usePierreDark();

let view: PierreFile | null = null;

function currentOptions() {
  return {
    theme: PIERRE_THEMES,
    themeType: (isDark.value ? "dark" : "light") as "dark" | "light",
    disableFileHeader: true,
    overflow: (props.wrap ? "wrap" : "scroll") as "wrap" | "scroll",
  };
}

function render() {
  if (!view || !host.value) {
    return;
  }
  view.render({
    containerWrapper: host.value,
    file: { name: props.path, contents: props.text, cacheKey: props.path },
  });
}

/** 滚动到 1-based 行号并居中:库没有公开行定位 API,
 *  经 open shadow root 内的 data-line 行元素 scrollIntoView */
function scrollToLine(line: number) {
  const el = host.value;
  if (!el) {
    return;
  }
  let root: ShadowRoot | null = null;
  for (const child of Array.from(el.children)) {
    if (child.shadowRoot) {
      root = child.shadowRoot;
      break;
    }
  }
  root
    ?.querySelector<HTMLElement>(`[data-code][data-line="${line}"]`)
    ?.scrollIntoView({ block: "center" });
}

// ── 文件内查找(Ctrl+F 查找条驱动):匹配计算在原文本上,定位/强调走库的行 API ──
let findRanges: TextRange[] = [];
let findCursor = -1;

/** 在当前文本上执行查找;返回带行号的全部匹配(空/非法查询为空数组) */
function runFind(query: FindQuery): TextRange[] {
  findRanges = [];
  findCursor = -1;
  const re = buildFindRegExp(query);
  if (re) {
    findRanges = collectMatches(props.text, re);
    if (findRanges.length) {
      findCursor = 0;
    }
  }
  return findRanges;
}

/** 当前匹配索引(runFind 后读取,-1 表示无) */
function getFindCursor(): number {
  return findCursor;
}

/** 清除查找状态与行强调 */
function clearFind() {
  findRanges = [];
  findCursor = -1;
  view?.setSelectedLines(null);
}

/** 跳到第 index 个匹配(循环取模、行强调并滚动),返回实际索引;无匹配返回 -1 */
function gotoMatch(index: number): number {
  if (!findRanges.length) {
    return -1;
  }
  const i = ((index % findRanges.length) + findRanges.length) % findRanges.length;
  const line = findRanges[i].line;
  findCursor = i;
  view?.setSelectedLines({ start: line, end: line });
  scrollToLine(line);
  return i;
}

/** 定位到 1-based 行号(全文搜索跳转用) */
function revealLine(line: number) {
  view?.setSelectedLines({ start: line, end: line });
  scrollToLine(line);
}

onMounted(() => {
  view = new PierreFile(currentOptions());
  render();
});

watch(isDark, (dark) => view?.setThemeType(dark ? "dark" : "light"));

watch(
  () => [props.text, props.path],
  () => {
    clearFind();
    render();
  },
);

watch(
  () => props.wrap,
  () => {
    // setOptions 只更新配置不重渲染(库行为),换行模式变化需显式 rerender
    view?.setOptions(currentOptions());
    view?.rerender();
  },
);

onBeforeUnmount(() => {
  view?.cleanUp();
  view = null;
});

defineExpose({ runFind, clearFind, gotoMatch, getFindCursor, revealLine });
</script>

<template>
  <div ref="host" class="pierre-file min-h-0 min-w-0 flex-1 overflow-auto" />
</template>

<style>
/* 库渲染在 shadow DOM 内,CSS 变量可穿透;字体栈桥接雅黑回退
   (WebView2 zh-CN 下泛型 monospace 会把 CJK 回退到 NSimSun) */
.pierre-file {
  --diffs-font-family:
    ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New",
    "Microsoft YaHei", monospace;
  --diffs-font-size: 12px;
  --diffs-line-height: 20px;
}
</style>
