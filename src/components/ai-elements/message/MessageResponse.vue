<script setup lang="ts">
import type { HTMLAttributes } from "vue";
import { cn } from "@/lib/utils";
import { computed, useSlots } from "vue";
import { useI18n } from "vue-i18n";
import { Markdown } from "vue-stream-markdown";
import "vue-stream-markdown/index.css";
import { createBeforeDownload } from "@/lib/markdown-download";
import type { SupportedLocale } from "@/i18n";

interface Props {
  content?: string;
  /**
   * 渲染模式:已完成的消息一律 static;仅流式累积中的气泡传 streaming。
   * streaming 的逐字动画会把文本拆成逐字/逐词的 inline-block span,
   * 在表格单元格内会破坏列宽计算(窄面板下列被压碎成逐字竖排),
   * 且历史消息每次渲染都会重放动画。
   */
  mode?: "static" | "streaming";
  class?: HTMLAttributes["class"];
}

const props = withDefaults(defineProps<Props>(), { mode: "static" });

const { t, locale } = useI18n();

// 主题变量读取隔离(与 wiki/report 的用法一致):传一个游离元素,库的
// useTailwindV3Theme 从它读不到任何 shadcn 变量,就不会把变量快照内联写回
// body 下共享的 overlay 容器——hex 格式的皮肤变量(island/glass)会被库误包成
// 非法的 hsl(#…),导致表格/代码块的全屏弹层背景全透明。
const detachedThemeEl = document.createElement("div");
const themeElement = () => detachedThemeEl;

const beforeDownload = createBeforeDownload(t);

const slots = useSlots();
const slotContent = computed<string | undefined>(() => {
  const nodes = slots.default?.();
  if (!Array.isArray(nodes)) {
    return undefined;
  }
  let text = "";
  for (const node of nodes) {
    if (typeof node.children === "string") text += node.children;
  }
  return text || undefined;
});

const md = computed(() => (slotContent.value ?? props.content ?? "") as string);
</script>

<template>
  <Markdown
    :content="md"
    :mode="mode"
    :theme-element="themeElement"
    :locale="locale as SupportedLocale"
    :before-download="beforeDownload"
    :class="cn('size-full [&>*:first-child]:mt-0! [&>*:last-child]:mb-0!', props.class)"
    v-bind="$attrs"
  />
</template>
