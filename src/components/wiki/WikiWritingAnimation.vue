<script setup lang="ts">
import { computed, onUnmounted, ref, watch, type Component } from "vue";
import { useI18n } from "vue-i18n";
import { FileSearch, FileText, FolderSearch, PenLine, Sparkles, Wrench } from "@lucide/vue";

/**
 * Wiki 生成等待动画:一份正在被书写的文档(逐行打字机书写 + 闪烁光标)。
 * agent 后端的工具调用不再逐行展示,而是化作「素材飞入文档」的粒子、
 * 文档辉光脉冲与右上角计数徽标参与动画,全部由 toolCalls 计数驱动。
 */
const props = defineProps<{
  /** 本轮生成累计的工具调用次数(单调递增;增量触发飞入粒子与辉光) */
  toolCalls: number;
}>();

const { t } = useI18n();

// ── 逐行书写(小步进定时器驱动,形成打字机节奏) ───────────────────────────

/** 各行目标宽度(容器宽度的百分比) */
const LINE_WIDTHS = [88, 100, 72, 94, 60, 82];
/** 行高 6px + 间距 10px,对应模板中的 h-1.5 + space-y-2.5 */
const LINE_PITCH = 16;
/** 每行书写所需 tick、整页写满后的停留 tick、整页淡去 tick */
const WRITE_TICKS = 14;
const HOLD_TICKS = 18;
const FADE_TICKS = 8;
const TICK_MS = 90;
const WRITING_TICKS = LINE_WIDTHS.length * WRITE_TICKS;

const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
const tick = ref(0);
const lineProgress = ref<number[]>(LINE_WIDTHS.map(() => (reducedMotion ? 1 : 0)));
const pageOpacity = ref(1);

let timer: number | undefined;
if (!reducedMotion) {
  timer = window.setInterval(() => {
    tick.value += 1;
    if (tick.value <= WRITING_TICKS) {
      const line = Math.floor((tick.value - 1) / WRITE_TICKS);
      const within = ((tick.value - 1) % WRITE_TICKS) + 1;
      lineProgress.value[line] = within / WRITE_TICKS;
    } else if (tick.value <= WRITING_TICKS + HOLD_TICKS) {
      // 整页写满,停留片刻供「阅读」
    } else if (tick.value <= WRITING_TICKS + HOLD_TICKS + FADE_TICKS) {
      pageOpacity.value = 1 - (tick.value - WRITING_TICKS - HOLD_TICKS) / FADE_TICKS;
    } else {
      tick.value = 0;
      lineProgress.value = LINE_WIDTHS.map(() => 0);
      pageOpacity.value = 1;
    }
  }, TICK_MS);
}

/** 正在书写的行序号(整页停留/淡去阶段为 -1,隐藏光标) */
const activeLine = computed(() =>
  tick.value >= 1 && tick.value <= WRITING_TICKS ? Math.floor((tick.value - 1) / WRITE_TICKS) : -1,
);
const caretStyle = computed(() => {
  const line = activeLine.value;
  if (line < 0) return { opacity: "0" };
  return {
    top: `${line * LINE_PITCH - 3}px`,
    left: `${lineProgress.value[line] * LINE_WIDTHS[line]}%`,
  };
});

// ── 工具调用参与动画:素材飞入粒子 + 辉光脉冲 ─────────────────────────────

const SPARK_ICONS: Component[] = [FileSearch, FolderSearch, FileText, Sparkles, Wrench];
const SPARK_LIFETIME_MS = 1600;

interface Spark {
  id: number;
  icon: Component;
  /** 起点水平位置(文档宽度的百分比) */
  x: number;
  delayMs: number;
}
const sparks = ref<Spark[]>([]);
const sparkTimers = new Set<number>();
let sparkSeq = 0;

/** 文档辉光脉冲序号:以 :key 重建元素来重放一次性动画 */
const glowPulse = ref(0);

watch(
  () => props.toolCalls,
  (next, prev) => {
    const delta = next - (prev ?? 0);
    if (delta <= 0) return;
    glowPulse.value += 1;
    // 活动按批量上报,单次增量可能多于一个;限制并发粒子数避免拥挤
    for (let i = 0; i < Math.min(delta, 4); i += 1) {
      const id = ++sparkSeq;
      const delayMs = i * 140;
      sparks.value.push({
        id,
        icon: SPARK_ICONS[id % SPARK_ICONS.length],
        x: 15 + Math.random() * 70,
        delayMs,
      });
      // 用定时器回收而非 animationend,保证 prefers-reduced-motion 下也能清理
      const timerId = window.setTimeout(() => {
        sparkTimers.delete(timerId);
        const index = sparks.value.findIndex((s) => s.id === id);
        if (index >= 0) sparks.value.splice(index, 1);
      }, delayMs + SPARK_LIFETIME_MS);
      sparkTimers.add(timerId);
    }
  },
);

onUnmounted(() => {
  if (timer !== undefined) window.clearInterval(timer);
  for (const timerId of sparkTimers) window.clearTimeout(timerId);
});
</script>

<template>
  <div class="relative" role="img" :aria-label="t('wiki.writing')">
    <div class="relative h-44 w-36 overflow-hidden rounded-lg border bg-card shadow-sm">
      <!-- 一次性辉光脉冲(每次工具调用以 :key 重建重放) -->
      <span
        v-if="glowPulse > 0"
        :key="glowPulse"
        class="wiki-writing-glow pointer-events-none absolute inset-0 rounded-lg"
      />
      <!-- 页面内容:标题行 + 正文行(整页写满后一起淡去,循环往复) -->
      <div class="px-3.5 pt-3.5" :style="{ opacity: pageOpacity }">
        <div class="h-2 rounded-full bg-primary/45" style="width: 55%" />
        <div class="relative mt-3 space-y-2.5">
          <div v-for="(width, index) in LINE_WIDTHS" :key="index" class="h-1.5">
            <div
              class="h-full rounded-full bg-muted-foreground/25 transition-[width] duration-100 ease-linear"
              :style="{ width: `${lineProgress[index] * width}%` }"
            />
          </div>
          <!-- 跟随书写位置的闪烁光标 -->
          <span
            class="wiki-writing-caret absolute h-3 w-0.5 rounded-full bg-primary/80 transition-[top,left] duration-100 ease-linear"
            :style="caretStyle"
          />
        </div>
      </div>
      <!-- 光泽扫过 -->
      <span class="wiki-writing-sheen pointer-events-none absolute inset-0" />
      <!-- 执笔图标 -->
      <PenLine class="wiki-writing-pen absolute right-3 bottom-3 h-4 w-4 text-primary/80" />
      <!-- 工具调用素材飞入粒子 -->
      <span
        v-for="spark in sparks"
        :key="spark.id"
        class="wiki-writing-spark pointer-events-none absolute bottom-8 flex h-6 w-6 items-center justify-center rounded-full border bg-background text-primary shadow-sm"
        :style="{ left: `${spark.x}%`, animationDelay: `${spark.delayMs}ms` }"
      >
        <component :is="spark.icon" class="h-3.5 w-3.5" />
      </span>
    </div>
    <!-- 工具调用计数徽标 -->
    <div
      v-if="toolCalls > 0"
      class="wiki-writing-badge absolute -top-2.5 -right-3 flex items-center gap-1 rounded-full border bg-background px-1.5 py-0.5 text-[10px] font-medium tabular-nums text-muted-foreground shadow-sm"
      :title="t('wiki.progress.toolCalls', { count: toolCalls })"
    >
      <Wrench class="h-3 w-3 text-primary" />
      {{ toolCalls }}
    </div>
  </div>
</template>

<style scoped>
/* 光标闪烁 */
@keyframes wiki-writing-caret {
  0%,
  45% {
    opacity: 1;
  }
  50%,
  95% {
    opacity: 0;
  }
  100% {
    opacity: 1;
  }
}

.wiki-writing-caret {
  animation: wiki-writing-caret 0.9s steps(1) infinite;
}

/* 执笔小幅起伏,模拟运笔 */
@keyframes wiki-writing-pen {
  0%,
  100% {
    transform: translate(0, 0) rotate(-6deg);
  }
  30% {
    transform: translate(-2px, 1px) rotate(-2deg);
  }
  60% {
    transform: translate(-4px, 0) rotate(-8deg);
  }
}

.wiki-writing-pen {
  animation: wiki-writing-pen 1.1s ease-in-out infinite;
}

/* 光泽从左向右扫过纸面 */
@keyframes wiki-writing-sheen {
  0% {
    transform: translateX(-130%);
  }
  55%,
  100% {
    transform: translateX(260%);
  }
}

.wiki-writing-sheen {
  background: linear-gradient(
    100deg,
    transparent 30%,
    color-mix(in oklab, var(--primary) 10%, transparent) 50%,
    transparent 70%
  );
  animation: wiki-writing-sheen 3.8s ease-in-out infinite;
}

/* 工具调用时的一圈外扩辉光 */
@keyframes wiki-writing-glow {
  0% {
    box-shadow: 0 0 0 0 color-mix(in oklab, var(--primary) 35%, transparent);
  }
  100% {
    box-shadow: 0 0 0 16px transparent;
  }
}

.wiki-writing-glow {
  animation: wiki-writing-glow 0.9s ease-out forwards;
}

/* 素材粒子:从文档底部浮起,升入纸面后淡出 */
@keyframes wiki-writing-spark {
  0% {
    transform: translate(-50%, 14px) scale(0.4);
    opacity: 0;
  }
  20% {
    transform: translate(-50%, 0) scale(1);
    opacity: 1;
  }
  65% {
    opacity: 1;
  }
  100% {
    transform: translate(-50%, -64px) scale(0.65);
    opacity: 0;
  }
}

.wiki-writing-spark {
  animation: wiki-writing-spark 1.5s ease-out forwards;
}

/* 计数徽标首次出现的弹性落位 */
@keyframes wiki-writing-badge {
  from {
    transform: scale(0.5);
    opacity: 0;
  }
  to {
    transform: scale(1);
    opacity: 1;
  }
}

.wiki-writing-badge {
  animation: wiki-writing-badge 0.35s cubic-bezier(0.34, 1.56, 0.64, 1);
}

@media (prefers-reduced-motion: reduce) {
  .wiki-writing-caret,
  .wiki-writing-pen,
  .wiki-writing-sheen,
  .wiki-writing-glow,
  .wiki-writing-badge {
    animation: none;
  }

  .wiki-writing-spark {
    animation: none;
    opacity: 0;
  }
}
</style>
