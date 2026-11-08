import { computed, ref } from "vue";
import { defineStore } from "pinia";
import { toast } from "vue-sonner";
import { i18n } from "@/i18n";
import {
  runBatchItems,
  type BatchItem,
  type BatchItemStatus,
  type BatchRunOptions,
} from "@/lib/batch-report";

/**
 * 批量生成报告的全局任务状态。
 * 进度展示从生成弹窗抽离为右下角浮窗(BatchProgressFloat),
 * 弹窗关闭/页面切换后任务与进度仍然存活。
 */
export const useBatchReportStore = defineStore("batchReport", () => {
  /** 当前批次时段列表(无提交的时段在确认后从此列表移除) */
  const items = ref<BatchItem[]>([]),
    running = ref(false),
    /** 浮窗是否可见(有批次即显示,结束后跳转历史或手动关闭时隐藏) */
    active = ref(false),
    /** 浮窗展开(进度条+明细) / 折叠(环形进度) */
    expanded = ref(false),
    /** 因无提交而被排除的时段数(从列表移除,仅计入统计) */
    noCommitDays = ref(0);
  /** 取消控制器:中止进行中的 AI 请求并停止派发新任务 */
  let aborter: AbortController | null = null;

  const stats = computed(() => {
      const list = items.value,
        done = list.filter((i) => i.status === "done").length,
        failed = list.filter((i) => i.status === "failed").length,
        // 跳过 = 已有报告 + 已取消 + 无提交被排除的天数
        skipped =
          list.filter((i) => i.status.startsWith("skipped") || i.status === "cancelled").length +
          noCommitDays.value;
      return {
        // 总数按最初规划计算(移除的无提交时段仍占分母,进度平滑递增)
        total: list.length + noCommitDays.value,
        finished: done + failed + skipped,
        done,
        skipped,
        failed,
      };
    }),
    /** 0-1 进度,驱动环形/条形进度 */
    progress = computed(() => (stats.value.total ? stats.value.finished / stats.value.total : 0)),
    /** 当前批次是否已全部结束(成功/跳过/失败/取消) */
    settled = computed(() => active.value && !running.value);

  /**
   * 状态变更写回。
   * - 无提交的时段直接从列表移除(进度排除无提交日期),仅累计到 noCommitDays
   * - 其余状态以整体重赋值写回(items.value = 新数组):
   *   ref 的 .value 赋值必然触发所有依赖,不依赖数组下标 SET/迭代键的触发细节
   */
  function setItemStatus(item: BatchItem, status: BatchItemStatus, error?: string) {
    if (status === "skipped-no-commits") {
      noCommitDays.value += 1;
      items.value = items.value.filter(
        (i) => !(i.dateFrom === item.dateFrom && i.dateTo === item.dateTo),
      );
      return;
    }
    const idx = items.value.findIndex(
      (i) => i.dateFrom === item.dateFrom && i.dateTo === item.dateTo,
    );
    if (idx === -1) {
      return;
    }
    const next = items.value.slice();
    next[idx] = { ...next[idx], status, error };
    items.value = next;
  }

  /** 启动批量生成;同时只允许一个批次在跑(调用方需先禁用入口) */
  async function start(batch: BatchItem[], options: BatchRunOptions) {
    if (running.value) {
      return;
    }
    items.value = batch;
    noCommitDays.value = 0;
    active.value = true;
    expanded.value = true;
    running.value = true;
    aborter = new AbortController();
    try {
      await runBatchItems(items.value, options, aborter.signal, setItemStatus);
    } finally {
      running.value = false;
      aborter = null;
    }
    const s = stats.value,
      message = i18n.global.t("report.batchDoneToast", {
        done: s.done,
        skipped: s.skipped,
        failed: s.failed,
      });
    if (s.failed) {
      toast.warning(message);
    } else {
      toast.success(message);
    }
  }

  /** 取消:中止进行中的任务(AI 请求立即中断,取消的时段不保存) */
  function cancel() {
    aborter?.abort();
  }

  function toggleExpanded() {
    expanded.value = !expanded.value;
  }

  /** 关闭浮窗(仅批次结束后允许) */
  function dismiss() {
    if (running.value) {
      return;
    }
    active.value = false;
    expanded.value = false;
    items.value = [];
    noCommitDays.value = 0;
  }

  return {
    items,
    running,
    active,
    expanded,
    stats,
    progress,
    settled,
    start,
    cancel,
    toggleExpanded,
    dismiss,
  };
});
