<script setup lang="ts">
import type { Component } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import {
  Ban,
  Check,
  CheckCircle2,
  ChevronDown,
  Clock3,
  History,
  Loader2,
  MinusCircle,
  X,
  XCircle,
} from "@lucide/vue";
import { Button } from "@/components/ui/button";
import ScrollArea from "@/components/common/ScrollArea.vue";
import type { BatchItem } from "@/lib/batch-report";
import { useBatchReportStore } from "@/stores/batch-report";

const { t } = useI18n();
const router = useRouter();
const store = useBatchReportStore();

/** 明细列表的状态图标与颜色 */
const STATUS_META: Record<BatchItem["status"], { icon: Component; class: string }> = {
  pending: { icon: Clock3, class: "text-muted-foreground" },
  running: { icon: Loader2, class: "animate-spin text-primary" },
  done: { icon: CheckCircle2, class: "text-green-600 dark:text-green-400" },
  "skipped-existing": { icon: MinusCircle, class: "text-muted-foreground" },
  "skipped-no-commits": { icon: MinusCircle, class: "text-muted-foreground" },
  failed: { icon: XCircle, class: "text-red-600 dark:text-red-400" },
  cancelled: { icon: Ban, class: "text-muted-foreground" },
};

const STATUS_LABEL: Record<BatchItem["status"], string> = {
  pending: "report.batchStatusPending",
  running: "report.batchStatusRunning",
  done: "report.batchStatusDone",
  "skipped-existing": "report.batchStatusSkippedExisting",
  "skipped-no-commits": "report.batchStatusSkippedNoCommits",
  failed: "report.batchStatusFailed",
  cancelled: "report.batchStatusCancelled",
};

/** 环形进度几何(viewBox 36,半径 15.5) */
const RING_R = 15.5;
const RING_C = 2 * Math.PI * RING_R;

function goHistory() {
  router.push("/report-history");
  store.dismiss();
}

/** 折叠态点击:进行中点击展开;已完成点击进入历史页 */
function onRingClick() {
  if (store.settled) {
    goHistory();
  } else {
    store.toggleExpanded();
  }
}
</script>

<template>
  <Transition name="batch-float">
    <div v-if="store.active" class="fixed right-4 bottom-4 z-50 flex flex-col items-end">
      <Transition name="batch-pop" mode="out-in">
        <!-- 折叠:环形进度;完成后点击进历史页 -->
        <button
          v-if="!store.expanded"
          key="ring"
          type="button"
          class="relative flex h-12 w-12 items-center justify-center rounded-full border bg-card shadow-lg transition-shadow hover:shadow-xl"
          :title="store.settled ? t('report.history') : t('report.batchExpand')"
          @click="onRingClick"
        >
          <svg viewBox="0 0 36 36" class="absolute inset-0 h-full w-full -rotate-90">
            <circle cx="18" cy="18" :r="RING_R" fill="none" class="stroke-muted" stroke-width="3" />
            <circle
              cx="18"
              cy="18"
              :r="RING_R"
              fill="none"
              stroke-width="3"
              stroke-linecap="round"
              class="transition-[stroke-dashoffset] duration-300"
              :class="
                store.settled
                  ? store.stats.failed
                    ? 'stroke-yellow-500'
                    : 'stroke-green-500'
                  : 'stroke-primary'
              "
              :stroke-dasharray="RING_C"
              :stroke-dashoffset="RING_C * (1 - store.progress)"
            />
          </svg>
          <Check v-if="store.settled && !store.stats.failed" class="h-4 w-4 text-green-500" />
          <X v-else-if="store.settled" class="h-4 w-4 text-yellow-500" />
          <span v-else class="text-[10px] font-medium tabular-nums">
            {{ Math.round(store.progress * 100) }}%
          </span>
        </button>

        <!-- 展开:进度条 + 逐时段明细 -->
        <div
          v-else
          key="panel"
          class="flex w-80 flex-col overflow-hidden rounded-lg border bg-card shadow-xl"
        >
          <div class="flex shrink-0 items-center justify-between border-b px-3 py-2">
            <div class="flex items-center gap-2">
              <Loader2 v-if="store.running" class="h-3.5 w-3.5 animate-spin text-primary" />
              <CheckCircle2
                v-else
                class="h-3.5 w-3.5"
                :class="store.stats.failed ? 'text-yellow-500' : 'text-green-500'"
              />
              <span class="text-sm font-medium">{{ t("report.batchFloatTitle") }}</span>
              <span class="text-xs text-muted-foreground tabular-nums">
                {{ store.stats.finished }}/{{ store.stats.total }}
              </span>
            </div>
            <div class="flex items-center gap-0.5">
              <Button
                v-if="store.running"
                variant="ghost"
                size="sm"
                class="h-6 gap-1 px-2 text-xs"
                @click="store.cancel()"
              >
                <Ban class="h-3 w-3" />
                {{ t("report.batchCancel") }}
              </Button>
              <Button
                variant="ghost"
                size="icon"
                class="h-6 w-6"
                :title="t('report.batchCollapse')"
                @click="store.toggleExpanded()"
              >
                <ChevronDown class="h-3.5 w-3.5" />
              </Button>
            </div>
          </div>

          <!-- 进度条 -->
          <div class="h-1 w-full shrink-0 bg-muted">
            <div
              class="h-full transition-[width] duration-300"
              :class="
                store.settled
                  ? store.stats.failed
                    ? 'bg-yellow-500'
                    : 'bg-green-500'
                  : 'bg-primary'
              "
              :style="{ width: `${store.progress * 100}%` }"
            />
          </div>

          <div class="shrink-0 px-3 pt-2 text-xs text-muted-foreground">
            {{
              t("report.batchSummary", {
                done: store.stats.done,
                skipped: store.stats.skipped,
                failed: store.stats.failed,
              })
            }}
          </div>
          <ScrollArea class="mt-1 max-h-48 min-h-0">
            <div class="flex flex-col p-2 pt-0">
              <div
                v-for="item in store.items"
                :key="`${item.dateFrom}|${item.dateTo}`"
                class="flex items-center gap-2 rounded px-2 py-1 text-sm"
                :title="item.error"
              >
                <component
                  :is="STATUS_META[item.status].icon"
                  class="h-3.5 w-3.5 shrink-0"
                  :class="STATUS_META[item.status].class"
                />
                <span class="min-w-0 flex-1 truncate">{{ item.label }}</span>
                <span
                  class="max-w-32 shrink-0 truncate text-xs"
                  :class="
                    item.status === 'failed'
                      ? 'text-red-600 dark:text-red-400'
                      : 'text-muted-foreground'
                  "
                >
                  {{
                    item.status === "failed" && item.error
                      ? item.error
                      : t(STATUS_LABEL[item.status])
                  }}
                </span>
              </div>
            </div>
          </ScrollArea>

          <!-- 完成后:进入历史页 -->
          <div v-if="store.settled" class="shrink-0 border-t p-2">
            <Button size="sm" class="w-full gap-1.5" @click="goHistory">
              <History class="h-3.5 w-3.5" />
              {{ t("report.history") }}
            </Button>
          </div>
        </div>
      </Transition>
    </div>
  </Transition>
</template>

<style scoped>
/* 浮窗整体出现/消失 */
.batch-float-enter-active,
.batch-float-leave-active {
  transition:
    opacity 0.2s ease,
    transform 0.2s ease;
  transform-origin: bottom right;
}
.batch-float-enter-from,
.batch-float-leave-to {
  opacity: 0;
  transform: translateY(8px) scale(0.9);
}

/* 环形 ⇄ 面板展开/折叠:out-in 先后过渡,从右下角缩放 */
.batch-pop-enter-active,
.batch-pop-leave-active {
  transition:
    opacity 0.18s ease,
    transform 0.18s ease;
  transform-origin: bottom right;
}
.batch-pop-enter-from,
.batch-pop-leave-to {
  opacity: 0;
  transform: scale(0.6);
}
</style>
