<script setup lang="ts">
import type { CSSProperties, HTMLAttributes } from "vue";
import { cn } from "@/lib/utils";
import { computed, useSlots } from "vue";

// 移植自 ai-elements-vue shimmer(官方依赖 motion-v 做位移动画;
// 这里用纯 CSS keyframes 实现同样的背景扫光,避免引入新依赖)。
interface Props {
  class?: HTMLAttributes["class"];
  /** 扫光一遍的时长(秒) */
  duration?: number;
  /** 高亮区宽度系数(按文本长度放大) */
  spread?: number;
}

const props = withDefaults(defineProps<Props>(), {
  duration: 2,
  spread: 2,
});

const slots = useSlots();

const textContent = computed(() => {
  const nodes = slots.default?.();
  if (!Array.isArray(nodes)) return "";
  return nodes.map((node) => (typeof node.children === "string" ? node.children : "")).join("");
});

const style = computed(
  (): CSSProperties => ({
    "--shimmer-spread": `${textContent.value.length * props.spread}px`,
    animationDuration: `${props.duration}s`,
  }),
);
</script>

<template>
  <span
    :class="cn('shimmer inline-block bg-clip-text text-transparent', props.class)"
    :style="style"
  >
    <slot />
  </span>
</template>

<style scoped>
.shimmer {
  background-image:
    linear-gradient(
      90deg,
      transparent calc(50% - var(--shimmer-spread)),
      var(--color-foreground) 50%,
      transparent calc(50% + var(--shimmer-spread))
    ),
    linear-gradient(var(--color-muted-foreground), var(--color-muted-foreground));
  background-size:
    250% 100%,
    auto;
  background-repeat: no-repeat;
  animation-name: shimmer-slide;
  animation-iteration-count: infinite;
  animation-timing-function: linear;
}

@keyframes shimmer-slide {
  from {
    background-position: 100% center;
  }
  to {
    background-position: 0% center;
  }
}
</style>
