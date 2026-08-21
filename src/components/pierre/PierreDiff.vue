<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import { FileDiff, parsePatchFiles, type FileDiffMetadata } from "@pierre/diffs";
import { PIERRE_THEMES, usePierreDark } from "./use-pierre-theme";

/**
 * @pierre/diffs 的 Vue 薄包装:单文件 diff 渲染(unified patch 输入)。
 * 实例生命周期由本组件持有;patch / 布局 / 词级高亮变化经 render 重放,
 * 亮暗切换走 setThemeType 缓存通道(不重排 DOM)。
 * 未变更区域折叠、点击展开、split 两侧留白对齐等均为库内行为。
 * 库没有公开的行定位 API,差异导航经 open shadow root 内的 data-line 行元素实现。
 */
const props = defineProps<{
  /** unified diff patch 文本(libgit2 patch.to_buf 输出,含 diff --git 头) */
  patch: string;
  /** 文件路径(语言推断与 cacheKey 用) */
  filePath: string;
  /** 并排(左右分栏)/ 逐行(单栏) */
  split: boolean;
  /** 行内词级差异高亮 */
  wordDiff: boolean;
  /** 截断 diff(超长大文件):内容不完整,unchanged 区域直接全量渲染、不可展开 */
  truncated: boolean;
}>();

const host = ref<HTMLElement | null>(null);
const isDark = usePierreDark();
/** 差异导航状态(工具条按钮禁用态):渲染完成与滚动时刷新 */
const hasPrevChange = ref(false);
const hasNextChange = ref(false);

let view: FileDiff | null = null;
let metadata: FileDiffMetadata | null = null;
/** 当前 patch 各变更块的首行(新文件行号),差异导航用 */
let changeLines: number[] = [];

function currentOptions() {
  return {
    theme: PIERRE_THEMES,
    themeType: (isDark.value ? "dark" : "light") as "dark" | "light",
    diffStyle: (props.split ? "split" : "unified") as "split" | "unified",
    lineDiffType: (props.wordDiff ? "word" : "none") as "word" | "none",
    // 工具条(文件路径/按钮)由外层 DiffViewer 自带,库内文件头关闭
    disableFileHeader: true,
    overflow: "scroll" as const,
    hunkSeparators: "line-info" as const,
    expandUnchanged: props.truncated,
  };
}

/** 库在 containerWrapper 内自建带 open shadow root 的容器元素 */
function shadowRootOf(): ShadowRoot | null {
  const el = host.value;
  if (!el) {
    return null;
  }
  for (const child of Array.from(el.children)) {
    if (child.shadowRoot) {
      return child.shadowRoot;
    }
  }
  return null;
}

/** 新文件行号对应的可见行元素(split 模式下优先 additions 列) */
function lineElement(line: number): HTMLElement | null {
  const root = shadowRootOf();
  if (!root) {
    return null;
  }
  const rows = Array.from(root.querySelectorAll<HTMLElement>(`[data-code][data-line="${line}"]`));
  return rows.find((r) => r.closest("[data-additions]")) ?? rows[0] ?? null;
}

/** 视口顶/底缘处可见行的新文件行号(shadowRoot.elementFromPoint 穿透 shadow DOM) */
function visibleLineAt(yRatio: number): number | null {
  const el = host.value;
  const root = shadowRootOf();
  if (!el || !root) {
    return null;
  }
  const rect = el.getBoundingClientRect();
  const hit = root.elementFromPoint(rect.left + 48, rect.top + rect.height * yRatio);
  const row = hit?.closest("[data-line]");
  const n = row?.getAttribute("data-line");
  return n ? Number(n) : null;
}

function refreshNavState() {
  const top = visibleLineAt(0.02);
  const bottom = visibleLineAt(0.98);
  if (top == null || !changeLines.length) {
    hasPrevChange.value = false;
    hasNextChange.value = changeLines.length > 0;
    return;
  }
  hasPrevChange.value = changeLines.some((l) => l < top - 1);
  hasNextChange.value = changeLines.some((l) => l > (bottom ?? top) + 1);
}

function parseMetadata(): FileDiffMetadata | null {
  const patches = parsePatchFiles(props.patch, props.filePath);
  return patches[0]?.files[0] ?? null;
}

function render() {
  if (!view || !host.value) {
    return;
  }
  metadata = parseMetadata();
  changeLines = (metadata?.hunks ?? [])
    .filter((h) => h.additionLines > 0 || h.deletionLines > 0)
    .map((h) => Math.max(h.additionStart, 1))
    .sort((a, b) => a - b);
  if (!metadata) {
    return;
  }
  view.render({ containerWrapper: host.value, fileDiff: metadata });
  // DOM 异步落地,下一帧再算导航态
  requestAnimationFrame(() => requestAnimationFrame(refreshNavState));
}

/** 上/下一个差异块:相对视口顶缘行号找前/后变更块首行,滚动到该行的 data-line 元素 */
function stepChange(dir: 1 | -1) {
  if (!changeLines.length) {
    return;
  }
  const top = visibleLineAt(0.02);
  if (top == null) {
    return;
  }
  const target =
    dir === 1
      ? changeLines.find((l) => l > top + 1)
      : [...changeLines].reverse().find((l) => l < top - 1);
  if (target == null) {
    return;
  }
  lineElement(target)?.scrollIntoView({ block: "center" });
}

onMounted(() => {
  view = new FileDiff(currentOptions());
  render();
  // scroll 不冒泡,capture 监听捕获后代(含 shadow 内 retarget 到宿主)滚动
  host.value?.addEventListener("scroll", refreshNavState, true);
});

watch(isDark, (dark) => view?.setThemeType(dark ? "dark" : "light"));

watch(
  () => [props.patch, props.filePath, props.truncated],
  () => {
    view?.setOptions(currentOptions());
    render();
  },
);

watch(
  () => [props.split, props.wordDiff],
  () => {
    // setOptions 只更新配置不重渲染(库行为),布局/词级变化需显式 rerender
    view?.setOptions(currentOptions());
    view?.rerender();
    requestAnimationFrame(() => requestAnimationFrame(refreshNavState));
  },
);

onBeforeUnmount(() => {
  host.value?.removeEventListener("scroll", refreshNavState, true);
  view?.cleanUp();
  view = null;
  metadata = null;
});

defineExpose({ stepChange, hasPrevChange, hasNextChange });
</script>

<template>
  <div ref="host" class="pierre-diff min-h-0 min-w-0 flex-1 overflow-auto" />
</template>

<style>
/* 库渲染在 shadow DOM 内,宿主只需提供确定高度;代码字体经 CSS 变量桥接雅黑回退
   (WebView2 zh-CN 下泛型 monospace 会把 CJK 回退到 NSimSun,见原 DiffViewer 注释) */
.pierre-diff {
  --diffs-font-family:
    ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New",
    "Microsoft YaHei", monospace;
  /* 与原 diff 视图一致:text-xs + 行高 20px */
  --diffs-font-size: 12px;
  --diffs-line-height: 20px;
}
</style>
