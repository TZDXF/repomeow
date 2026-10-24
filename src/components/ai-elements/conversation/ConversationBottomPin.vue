<script setup lang="ts">
import { onBeforeUnmount, watchEffect } from "vue";
import { useStickToBottomContext } from "vue-stick-to-bottom";

/**
 * 吸底同步修正:vue-stick-to-bottom 的 resize 修正在 requestAnimationFrame
 * 里应用,流式输出时每一帧都先以过期的 scrollTop 绘制再回弹(滚动条肉眼可见
 * 地跳动)。这里改在 ResizeObserver 回调(layout 后、paint 前)同步把 scrollTop
 * 钉在底部。仅在跟随底部且未被用户上滚解锁时生效。
 */
const { scrollRef, contentRef, isAtBottom, escapedFromLock } = useStickToBottomContext();

let observer: ResizeObserver | undefined;

watchEffect((onCleanup) => {
  const scroll = scrollRef.value;
  const content = contentRef.value;
  if (!scroll || !content) return;
  observer = new ResizeObserver(() => {
    if (!isAtBottom.value || escapedFromLock.value) return;
    const max = scroll.scrollHeight - scroll.clientHeight;
    if (max <= 0 || scroll.scrollTop >= max) return;
    scroll.scrollTop = max;
  });
  observer.observe(content);
  onCleanup(() => {
    observer?.disconnect();
    observer = undefined;
  });
});

onBeforeUnmount(() => observer?.disconnect());
</script>

<template>
  <div class="hidden" aria-hidden="true" />
</template>
