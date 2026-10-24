<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useStickToBottomContext } from "vue-stick-to-bottom";

/**
 * 消息级按需渲染壳:远离视口的消息只保留记录了最后高度的占位 shell,
 * 进入阈值(rootMargin 800px)后才真实渲染,长会话的滚动与流式渲染成本
 * 不再随消息总量线性增长。视口取自 Conversation 的 vue-stick-to-bottom
 * 上下文(本组件必须渲染在 Conversation 内部)。
 */
const props = withDefaults(
  defineProps<{
    /** 强制渲染:时间线末尾固定几条保持真实挂载,保证吸底定位与流式可见 */
    pinned?: boolean;
    /** 流式回合:始终渲染 */
    live?: boolean;
  }>(),
  { pinned: false, live: false },
);

const { scrollRef } = useStickToBottomContext();
const shell = ref<HTMLElement>();
const body = ref<HTMLElement>();
const nearby = ref(false);
const revealed = ref(false);
const focused = ref(false);
// 占位高度:取最后实测值,初始给一个常见气泡的估计值
const height = ref(160);
const rendered = computed(
  () => props.pinned || props.live || nearby.value || revealed.value || focused.value,
);

let intersection: IntersectionObserver | undefined;
let resize: ResizeObserver | undefined;
let disposed = false;

function measure() {
  const el = body.value;
  if (!el) return;
  const next = el.getBoundingClientRect().height;
  if (!next) return;
  const previous = height.value;
  height.value = next;
  const viewport = scrollRef.value;
  if (!viewport || !shell.value) return;
  // 视口上方的占位高度变化时补偿 scrollTop,保持阅读位置;吸底跟随交给 Conversation
  const above = shell.value.getBoundingClientRect().top + previous <= viewport.getBoundingClientRect().top;
  const atBottom = viewport.scrollHeight - viewport.clientHeight - viewport.scrollTop < 32;
  if (above && !atBottom) viewport.scrollTop += next - previous;
}

async function onFocusOut() {
  await nextTick();
  focused.value = !!shell.value?.contains(document.activeElement);
}

watch(
  rendered,
  async (visible) => {
    resize?.disconnect();
    if (!visible) return;
    await nextTick();
    if (disposed) return;
    if (body.value) resize?.observe(body.value);
    measure();
  },
  { flush: "post" },
);

onMounted(() => {
  resize = new ResizeObserver(measure);
  if (body.value) {
    resize.observe(body.value);
    measure();
  }
  watch(
    () => scrollRef.value,
    (viewport) => {
      intersection?.disconnect();
      if (!viewport || !shell.value) return;
      intersection = new IntersectionObserver(
        (entries) => {
          for (const entry of entries) {
            // 正被用户选中(划词复制)的内容不因滚出视口而卸载
            const selection = window.getSelection();
            const selecting =
              selection &&
              !selection.isCollapsed &&
              (shell.value?.contains(selection.anchorNode) ||
                shell.value?.contains(selection.focusNode));
            if (!entry.isIntersecting && selecting) continue;
            nearby.value = entry.isIntersecting;
            if (entry.isIntersecting) revealed.value = false;
          }
        },
        { root: viewport, rootMargin: "800px 0px" },
      );
      intersection.observe(shell.value);
    },
    { immediate: true, flush: "post" },
  );
});

onBeforeUnmount(() => {
  disposed = true;
  intersection?.disconnect();
  resize?.disconnect();
});
</script>

<template>
  <!-- 未渲染时保留可寻址的占位 shell(高度取最后实测值) -->
  <div
    ref="shell"
    class="min-w-0 shrink-0"
    :style="rendered ? undefined : { height: `${height}px` }"
    @virtual-reveal="revealed = true"
    @focusin="focused = true"
    @focusout="onFocusOut"
  >
    <div v-if="rendered" ref="body" class="flow-root min-w-0">
      <slot />
    </div>
  </div>
</template>
