import { generateReport } from "@/lib/ai";
import { runPool } from "@/lib/async-pool";
import { cmd } from "@/lib/tauri";
import type { SupportedLocale } from "@/i18n";
import type {
  BatchRange,
  GitCommitInfo,
  GitUser,
  Project,
  ReportDateRange,
  ReportPeriodType,
  SaveReportCommit,
} from "@/types";

export type BatchItemStatus =
  | "pending"
  | "running"
  | "done"
  | "skipped-existing"
  | "skipped-no-commits"
  | "failed"
  | "cancelled";

/** 批量生成的单个时段及其执行状态(供进度 UI 直接渲染,需以 reactive 数组持有) */
export interface BatchItem {
  dateFrom: string;
  dateTo: string;
  label: string;
  status: BatchItemStatus;
  error?: string;
}

export interface BatchPlanOptions {
  periodType: ReportPeriodType;
  /** 总跨度 "YYYY-MM-DD" */
  dateFrom: string;
  dateTo: string;
  /** 仅工作日(仅日报生效;为 true 时非工作日时段不列入结果) */
  workdaysOnly: boolean;
  /** 跳过已有同类型报告的时段(日报按日期、周报按周范围匹配) */
  skipExisting: boolean;
  /** 时段展示/保存用的范围标签 */
  makeLabel: (dateFrom: string, dateTo: string) => string;
}

/**
 * 规划批量生成的时段列表:
 * 后端切分(日报逐天/周报按工作周)后,按过滤选项标注或剔除时段。
 * 日报选「仅工作日」时非工作日直接不列入(数量多,展示意义低);
 * 「跳过已有」命中的时段保留在列表中,状态标为 skipped-existing。
 */
export async function planBatchItems(options: BatchPlanOptions): Promise<BatchItem[]> {
  const ranges = await cmd<BatchRange[]>("plan_batch_report_ranges", {
    periodType: options.periodType,
    dateFrom: options.dateFrom,
    dateTo: options.dateTo,
  });

  // 已有报告匹配集合:日报按 date_to,周报按 (date_from, date_to) 对
  let existingDaily = new Set<string>();
  let existingWeekly = new Set<string>();
  if (options.skipExisting) {
    const existing = await cmd<ReportDateRange[]>("list_report_dates", {
      periodType: options.periodType,
      dateFrom: options.dateFrom,
      dateTo: options.dateTo,
    });
    if (options.periodType === "weekly") {
      existingWeekly = new Set(existing.map((r) => `${r.dateFrom}|${r.dateTo}`));
    } else {
      existingDaily = new Set(existing.map((r) => r.dateTo));
    }
  }

  const items: BatchItem[] = [];
  for (const r of ranges) {
    if (options.periodType === "daily" && options.workdaysOnly && !r.isWorkday) {
      continue;
    }
    const skipped =
      options.periodType === "weekly"
        ? existingWeekly.has(`${r.dateFrom}|${r.dateTo}`)
        : existingDaily.has(r.dateTo);
    items.push({
      dateFrom: r.dateFrom,
      dateTo: r.dateTo,
      label: options.makeLabel(r.dateFrom, r.dateTo),
      status: skipped ? "skipped-existing" : "pending",
    });
  }
  return items;
}

export interface BatchRunOptions {
  periodType: ReportPeriodType;
  /** 选中的项目(已按归档过滤) */
  projects: Project[];
  authorMode: "me" | "all";
  language: SupportedLocale;
  /** 并发上限(同时生成的报告份数) */
  concurrency: number;
}

interface ProjectCommitsWithId {
  projectId: number;
  projectName: string;
  projectDescription: string;
  commits: GitCommitInfo[];
}

/**
 * 时段状态变更回调:执行层不直接修改 item,状态变更统一上报,
 * 由调用方(store)以不可变方式写回响应式数组,保证 UI 更新链路可靠
 */
export type BatchStatusCallback = (
  item: BatchItem,
  status: BatchItemStatus,
  error?: string,
) => void;

/**
 * 执行批量生成:对每个 pending 时段拉取提交 → 过滤无提交项目 → AI 生成 → 存入历史。
 * 单个时段失败不影响其他时段;signal 中止时停止派发新任务,
 * 进行中的任务在阶段边界响应取消(AI 请求经 abortSignal 立即中止),取消的时段不保存。
 * 作者身份按项目缓存(不随日期变化),避免每个时段重复调用 git_current_user。
 * items 只读;所有状态变更通过 onStatus 上报。
 */
export async function runBatchItems(
  items: BatchItem[],
  options: BatchRunOptions,
  signal: AbortSignal,
  onStatus: BatchStatusCallback,
): Promise<void> {
  // 预解析各项目的 git 用户身份(仅「我自己」模式需要;失败的项目不过滤作者)
  const authors = new Map<number, string | undefined>();
  if (options.authorMode === "me") {
    await Promise.all(
      options.projects.map(async (p) => {
        const user = await cmd<GitUser>("git_current_user", { path: p.path }).catch(() => null);
        authors.set(p.id, user?.name || user?.email || undefined);
      }),
    );
  }

  /** 已被并发池派发执行的任务(item 不再本地变更状态,用此集合判断"未派发") */
  const dispatched = new Set<BatchItem>();

  async function runOne(item: BatchItem) {
    dispatched.add(item);
    onStatus(item, "running");
    try {
      const since = `${item.dateFrom} 00:00:00`;
      const until = `${item.dateTo} 23:59:59`;
      const data: ProjectCommitsWithId[] = await Promise.all(
        options.projects.map(async (p) => ({
          projectId: p.id,
          projectName: p.name,
          projectDescription: p.description,
          commits: await cmd<GitCommitInfo[]>("git_log", {
            path: p.path,
            since,
            until,
            maxCount: 500,
            author: authors.get(p.id),
          }),
        })),
      );
      if (signal.aborted) {
        onStatus(item, "cancelled");
        return;
      }
      // 排除没有提交的项目;该时段全部无提交则跳过
      const withCommits = data.filter((d) => d.commits.length);
      if (!withCommits.length) {
        onStatus(item, "skipped-no-commits");
        return;
      }

      // AI 请求挂中止信号,取消时立即中断
      const result = await generateReport(
        withCommits,
        item.label,
        options.language,
        options.periodType,
        signal,
      );
      // 保存前最后检查:取消的时段不写入历史
      if (signal.aborted) {
        onStatus(item, "cancelled");
        return;
      }

      const commitData: SaveReportCommit[] = withCommits.map((d) => ({
        projectId: d.projectId,
        projectName: d.projectName,
        projectDescription: d.projectDescription,
        commits: d.commits,
      }));
      await cmd<number>("save_report_history", {
        projectIds: commitData.map((c) => c.projectId).filter((id): id is number => id != null),
        dateFrom: item.dateFrom,
        dateTo: item.dateTo,
        rangeLabel: item.label,
        authorMode: options.authorMode,
        language: options.language,
        periodType: options.periodType,
        result,
        commitData,
      });
      onStatus(item, "done");
    } catch (e) {
      // 中止导致的异常归为取消而非失败
      if (signal.aborted) {
        onStatus(item, "cancelled");
        return;
      }
      onStatus(item, "failed", e instanceof Error ? e.message : String(e));
    }
  }

  const tasks = items.filter((i) => i.status === "pending").map((item) => () => runOne(item));
  await runPool(options.concurrency, tasks, signal);

  // 取消后未派发的任务标记为 cancelled(状态不经本地修改,以是否派发为准)
  if (signal.aborted) {
    for (const item of items) {
      if (item.status === "pending" && !dispatched.has(item)) {
        onStatus(item, "cancelled");
      }
    }
  }
}
