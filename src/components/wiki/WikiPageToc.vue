<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useElementSize } from "@vueuse/core";
import { ListTree } from "@lucide/vue";
import { Button } from "@/components/ui/button";

interface TocEntry {
  el: HTMLElement;
  text: string;
  level: number;
}

/** 视口顶部再往下这段距离内的最后一个标题视为「当前」(标题自带 mt-6 上边距) */
const ACTIVE_OFFSET_PX = 72;
/** 跳转后标题距视口顶部的留白 */
const SCROLL_MARGIN_PX = 12;
/** 只有标题的页面不值得展示目录 */
const MIN_ENTRIES = 2;
/**
 * 容器宽于该值时目录常驻展开,否则折叠为按钮、hover 浮出。
 * 取值依据:正文列 max-w-3xl(768px)居中,需再容纳面板 w-52(208px)+ 两侧间距,
 * 即 768 + 2 * (208 + 16 + 16) ≈ 1248px 时面板与正文列之间仍有空隙。
 */
const EXPAND_MIN_WIDTH = 1248;
/** 折叠态移出后延迟收起,容忍指针在按钮与浮层间隙间的短暂离开 */
const HOVER_CLOSE_DELAY_MS = 150;

const props = defineProps<{
  /** 渲染了 Markdown 正文的容器,标题直接从它的 DOM 中提取 */
  root: HTMLElement | null;
  /** 正文内容;变更后重新扫描标题 */
  content: string;
  /** 页面标识;切页后重置目录与高亮 */
  pageId: string;
}>();

const { t } = useI18n();

const host = ref<HTMLElement | null>(null);
const { width: containerWidth } = useElementSize(host);
const expanded = computed(() => containerWidth.value >= EXPAND_MIN_WIDTH);

const entries = ref<TocEntry[]>([]);
const activeEl = ref<HTMLElement | null>(null);
const hovering = ref(false);
let hoverTimer: ReturnType<typeof setTimeout> | null = null;

const minLevel = computed(() =>
  entries.value.length ? Math.min(...entries.value.map((entry) => entry.level)) : 1,
);

/** reka-ui ScrollArea 的实际滚动元素是内部 viewport,与 useWikiPreviewScroll 同源 */
function scrollViewport(): HTMLElement | null {
  return props.root?.closest('[data-slot="scroll-area-viewport"]') ?? null;
}

/** 从渲染后的 DOM 提取标题,天然排除代码块里的 # 注释等假标题 */
function scan() {
  const rootEl = props.root;
  if (!rootEl) {
    entries.value = [];
    return;
  }
  const found: TocEntry[] = [];
  for (const el of rootEl.querySelectorAll<HTMLElement>("h1, h2, h3, h4, h5")) {
    // 标题上可能挂着折叠/锚点等交互按钮,剔除后取纯文本
    const clone = el.cloneNode(true) as HTMLElement;
    clone.querySelectorAll("button").forEach((node) => node.remove());
    const text = (clone.textContent ?? "").replace(/\s+/g, " ").trim();
    if (text) {
      found.push({ el, text, level: Number(el.tagName[1]) });
    }
  }
  entries.value = found;
  if (!found.some((entry) => entry.el === activeEl.value)) {
    activeEl.value = found[0]?.el ?? null;
  }
  updateActive();
}

function updateActive() {
  const viewport = scrollViewport();
  if (!viewport || entries.value.length === 0) {
    return;
  }
  const viewportTop = viewport.getBoundingClientRect().top;
  let current = entries.value[0];
  for (const entry of entries.value) {
    if (entry.el.getBoundingClientRect().top - viewportTop > ACTIVE_OFFSET_PX) {
      break;
    }
    current = entry;
  }
  activeEl.value = current.el;
}

function jumpTo(entry: TocEntry) {
  const viewport = scrollViewport();
  if (!viewport) {
    return;
  }
  const viewportTop = viewport.getBoundingClientRect().top;
  const top =
    viewport.scrollTop + entry.el.getBoundingClientRect().top - viewportTop - SCROLL_MARGIN_PX;
  viewport.scrollTo({ top: Math.max(top, 0), behavior: "smooth" });
  activeEl.value = entry.el;
}

function onHoverEnter() {
  if (hoverTimer) {
    clearTimeout(hoverTimer);
    hoverTimer = null;
  }
  hovering.value = true;
}

function onHoverLeave() {
  if (hoverTimer) {
    clearTimeout(hoverTimer);
  }
  hoverTimer = setTimeout(() => {
    hovering.value = false;
    hoverTimer = null;
  }, HOVER_CLOSE_DELAY_MS);
}

onBeforeUnmount(() => {
  if (hoverTimer) {
    clearTimeout(hoverTimer);
  }
});

watch(
  () => [props.root, props.content, props.pageId] as const,
  async () => {
    await nextTick();
    scan();
  },
  { immediate: true },
);

watch(
  () => props.root,
  (element, _previous, onCleanup) => {
    const viewport = element?.closest('[data-slot="scroll-area-viewport"]');
    if (!element || !viewport) {
      return;
    }
    const onScroll = () => updateActive();
    viewport.addEventListener("scroll", onScroll, { passive: true });
    onCleanup(() => viewport.removeEventListener("scroll", onScroll));
  },
  { immediate: true },
);
</script>

<template>
  <!-- 横向铺满的悬浮层:pointer-events-none 让中间区域透传点击,仅按钮与面板可交互;
       flex justify-end 让按钮与面板统一贴右(Button 是行内元素,ml-auto 对其无效) -->
  <div
    v-if="entries.length >= MIN_ENTRIES"
    ref="host"
    class="pointer-events-none absolute inset-x-0 top-3 z-20 flex justify-end"
    @mouseenter="onHoverEnter"
    @mouseleave="onHoverLeave"
  >
    <template v-if="containerWidth > 0">
      <Button
        v-if="!expanded"
        variant="ghost"
        size="icon"
        class="pointer-events-auto ml-auto mr-4 h-7 w-7 rounded-md border bg-background/80 shadow-sm backdrop-blur"
        :title="t('wiki.toc')"
        :aria-label="t('wiki.toc')"
      >
        <ListTree class="h-4 w-4" />
      </Button>
      <div
        v-show="expanded || hovering"
        class="pointer-events-auto w-52 overflow-hidden rounded-md border bg-popover text-popover-foreground shadow-md"
        :class="expanded ? 'ml-auto mr-4' : 'absolute right-4 top-8'"
      >
        <div class="max-h-[60vh] overflow-y-auto p-1">
          <button
            v-for="(entry, index) in entries"
            :key="index"
            type="button"
            class="block w-full truncate rounded-sm py-1 pr-2 text-left text-xs transition-colors"
            :class="
              entry.el === activeEl
                ? 'bg-accent text-accent-foreground'
                : 'text-muted-foreground hover:bg-accent/60 hover:text-foreground'
            "
            :style="{ paddingLeft: `${8 + (entry.level - minLevel) * 12}px` }"
            :title="entry.text"
            @click="jumpTo(entry)"
          >
            {{ entry.text }}
          </button>
        </div>
      </div>
    </template>
  </div>
</template>
