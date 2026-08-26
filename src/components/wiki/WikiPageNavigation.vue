<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { Check, Circle, LoaderCircle, X } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import type { WikiGenPhase } from "@/lib/wiki-generator";
import type { WikiNavItem } from "./wiki-navigation";

const props = defineProps<{
  items: WikiNavItem[];
  activeId: string | null;
  generating: boolean;
  phase?: WikiGenPhase;
  totalPageCount: number;
  processedPageCount: number;
}>();

const emit = defineEmits<{
  select: [id: string];
  cancel: [];
}>();

const { t } = useI18n();

const phaseText = computed(() => {
  const map: Record<WikiGenPhase, string> = {
    collecting: t("wiki.phase.collecting"),
    outlining: t("wiki.phase.outlining"),
    generating: t("wiki.phase.generating"),
    done: "",
    failed: "",
    cancelled: "",
  };
  return props.phase ? map[props.phase] : "";
});

const phaseSteps = computed(() => [
  t("wiki.progress.collecting"),
  t("wiki.progress.outlining"),
  t("wiki.progress.generating"),
]);

const activePhaseIndex = computed(() => {
  switch (props.phase) {
    case "outlining":
      return 1;
    case "generating":
      return 2;
    default:
      return 0;
  }
});

const pageProgressPercent = computed(() =>
  props.totalPageCount > 0
    ? Math.round((props.processedPageCount / props.totalPageCount) * 100)
    : 0,
);

const pageProgressStyle = computed(() => ({ width: `${pageProgressPercent.value}%` }));

/** 按 section 保序分组；无 section 的页面归入 null 组并扁平展示。 */
const groups = computed(() => {
  const result: { section: string | null; items: WikiNavItem[] }[] = [];
  for (const item of props.items) {
    const section = item.section ?? null;
    const last = result[result.length - 1];
    if (last && last.section === section) {
      last.items.push(item);
    } else {
      result.push({ section, items: [item] });
    }
  }
  return result;
});

/** importance H/M/L 徽标，颜色与文字双重编码，避免只靠色点辨认。 */
function importanceClass(importance: string): string {
  switch (importance) {
    case "high":
      return "border-violet-400/60 bg-violet-500/10 text-violet-600 dark:text-violet-300";
    case "low":
      return "border-sky-400/60 bg-sky-500/10 text-sky-600 dark:text-sky-300";
    default:
      return "border-amber-400/60 bg-amber-500/10 text-amber-700 dark:text-amber-300";
  }
}

function importanceCode(importance: string): "H" | "M" | "L" {
  if (importance === "high") return "H";
  if (importance === "low") return "L";
  return "M";
}

function importanceLabel(importance: string): string {
  switch (importance) {
    case "high":
      return t("wiki.importance.high");
    case "low":
      return t("wiki.importance.low");
    default:
      return t("wiki.importance.medium");
  }
}

function pageStatsText(item: WikiNavItem): string {
  if (item.durationMs === undefined) {
    return "";
  }
  const totalSeconds = Math.max(0, Math.round(item.durationMs / 1000));
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return `${minutes}:${String(seconds).padStart(2, "0")}`;
}

/** 生成中页面的实时字数;<1000 原样展示,否则缩写为 k,避免宽度抖动 */
function pageWordCountText(item: WikiNavItem): string {
  const count = item.wordCount ?? 0;
  if (count < 1000) return t("wiki.charCount", { count });
  return t("wiki.charCount", { count: `${(count / 1000).toFixed(1)}k` });
}
</script>

<template>
  <aside class="flex w-64 shrink-0 flex-col border-r">
    <section v-if="generating" class="shrink-0 border-b bg-muted/20 p-3" aria-live="polite">
      <div class="flex items-center gap-2">
        <LoaderCircle class="h-4 w-4 shrink-0 animate-spin text-primary" />
        <p class="min-w-0 flex-1 truncate text-xs font-medium">{{ phaseText }}</p>
        <span
          v-if="totalPageCount"
          class="shrink-0 text-xs font-semibold tabular-nums text-primary"
        >
          {{ pageProgressPercent }}%
        </span>
      </div>

      <div class="mt-3 grid grid-cols-3 gap-1" aria-hidden="true">
        <div v-for="(step, index) in phaseSteps" :key="step" class="text-center">
          <div class="mb-1 flex items-center">
            <span
              class="h-px flex-1"
              :class="index <= activePhaseIndex ? 'bg-primary/50' : 'bg-border'"
            />
            <span
              class="relative h-2.5 w-2.5 shrink-0 rounded-full border"
              :class="
                index < activePhaseIndex
                  ? 'border-primary bg-primary'
                  : index === activePhaseIndex
                    ? 'border-primary bg-background shadow-[0_0_0_3px] shadow-primary/15'
                    : 'border-border bg-background'
              "
            >
              <span
                v-if="index === activePhaseIndex"
                class="absolute inset-0 animate-ping rounded-full bg-primary/50"
              />
            </span>
            <span
              class="h-px flex-1"
              :class="index < activePhaseIndex ? 'bg-primary/50' : 'bg-border'"
            />
          </div>
          <span
            class="text-[10px]"
            :class="index <= activePhaseIndex ? 'text-foreground' : 'text-muted-foreground/60'"
          >
            {{ step }}
          </span>
        </div>
      </div>

      <div
        class="mt-3 h-1.5 overflow-hidden rounded-full bg-muted"
        role="progressbar"
        :aria-label="phaseText"
        :aria-valuemin="totalPageCount ? 0 : undefined"
        :aria-valuemax="totalPageCount ? 100 : undefined"
        :aria-valuenow="totalPageCount ? pageProgressPercent : undefined"
      >
        <div
          v-if="totalPageCount"
          class="h-full rounded-full bg-primary transition-[width] duration-500 ease-out"
          :style="pageProgressStyle"
        />
        <div v-else class="wiki-progress-indeterminate h-full rounded-full bg-primary" />
      </div>
    </section>

    <ScrollArea class="min-h-0 flex-1">
      <TooltipProvider>
        <nav class="space-y-3 p-3">
          <div v-for="group in groups" :key="group.section ?? '__flat'">
            <p
              v-if="group.section"
              class="mb-1 px-2 text-xs font-medium uppercase tracking-wide text-muted-foreground"
            >
              {{ group.section }}
            </p>
            <button
              v-for="item in group.items"
              :key="item.id"
              type="button"
              class="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm hover:bg-accent/60"
              :class="
                item.id === activeId
                  ? 'bg-accent font-medium text-accent-foreground'
                  : item.status === 'running'
                    ? 'bg-primary/5 text-foreground'
                    : 'text-foreground/80'
              "
              :title="item.error ?? (item.status ? item.title : undefined)"
              @click="emit('select', item.id)"
            >
              <template v-if="item.status">
                <LoaderCircle
                  v-if="item.status === 'running'"
                  class="h-3.5 w-3.5 shrink-0 animate-spin text-primary"
                />
                <Check
                  v-else-if="item.status === 'done'"
                  class="h-3.5 w-3.5 shrink-0 text-green-500"
                />
                <X
                  v-else-if="item.status === 'failed'"
                  class="h-3.5 w-3.5 shrink-0 text-destructive"
                />
                <Circle v-else class="h-3.5 w-3.5 shrink-0 text-muted-foreground/40" />
              </template>
              <Tooltip v-else>
                <TooltipTrigger as-child>
                  <span
                    class="flex h-4 min-w-4 shrink-0 items-center justify-center rounded border px-0.5 text-[9px] font-bold leading-none"
                    :class="importanceClass(item.importance)"
                    :aria-label="importanceLabel(item.importance)"
                  >
                    {{ importanceCode(item.importance) }}
                  </span>
                </TooltipTrigger>
                <TooltipContent side="right">
                  {{ importanceLabel(item.importance) }}
                </TooltipContent>
              </Tooltip>
              <span class="min-w-0 flex-1 truncate" :title="item.title">{{ item.title }}</span>
              <span
                v-if="item.unread"
                class="shrink-0 rounded bg-primary/10 px-1 py-0.5 text-[9px] font-medium leading-none text-primary"
              >
                {{ t("wiki.unread") }}
              </span>
              <span
                v-if="item.status === 'running' && item.wordCount !== undefined"
                class="shrink-0 text-[10px] tabular-nums text-primary/80"
              >
                {{ pageWordCountText(item) }}
              </span>
              <span
                v-else-if="item.status === 'done' && item.durationMs !== undefined"
                class="shrink-0 text-[10px] tabular-nums text-muted-foreground"
              >
                {{ pageStatsText(item) }}
              </span>
            </button>
          </div>
        </nav>
      </TooltipProvider>
    </ScrollArea>

    <div v-if="generating" class="shrink-0 border-t p-2">
      <Button variant="outline" size="sm" class="w-full" @click="emit('cancel')">
        {{ t("wiki.cancel") }}
      </Button>
    </div>
  </aside>
</template>

<style scoped>
@keyframes wiki-progress-slide {
  from {
    transform: translateX(-120%);
  }
  to {
    transform: translateX(350%);
  }
}

.wiki-progress-indeterminate {
  width: 30%;
  animation: wiki-progress-slide 1.4s ease-in-out infinite;
}

@media (prefers-reduced-motion: reduce) {
  .wiki-progress-indeterminate {
    width: 100%;
    animation: none;
    opacity: 0.55;
  }
}
</style>
