import type { Project, ReportSchedule, Tag } from "@/types";

export type WeekdayMode = "everyday" | "weekdays" | "chineseWorkday";
export type ScheduleReportType = "daily" | "weekly";
export type ScheduleAuthorMode = "me" | "all";

/** 定时报告编辑表单的完整状态(与持久化的 ReportSchedule 字段一一对应) */
export interface ScheduleFormState {
  name: string;
  time: string;
  reportType: ScheduleReportType;
  /** 日报:true = 前一天(次日生成);false = 当天 */
  previousDay: boolean;
  weekdayMode: WeekdayMode;
  /** 周报:true = 工作周模式;false = 自定义周几~周几 */
  weeklyWorkweek: boolean;
  /** 周报自定义:范围起始周几(1=周一 .. 7=周日) */
  weeklyStart: number;
  /** 周报自定义:范围结束/触发周几(1=周一 .. 7=周日) */
  weeklyEnd: number;
  authorMode: ScheduleAuthorMode;
  projectIds: number[];
  /** 按标签动态包含(持久化进任务,执行时反查项目);区别于仅过滤显示的筛选标签 */
  tagIds: number[];
}

/** 新建任务的表单默认值(与原 openCreate 的重置值一致) */
export function createEmptyScheduleForm(): ScheduleFormState {
  return {
    name: "",
    time: "09:00",
    reportType: "daily",
    previousDay: true,
    weekdayMode: "everyday",
    weeklyWorkweek: true,
    weeklyStart: 1,
    weeklyEnd: 5,
    authorMode: "me",
    projectIds: [],
    tagIds: [],
  };
}

/** 由持久化任务推导星期过滤模式(优先级:中国工作日 > 周一至周五 > 每天) */
export function weekdayModeOf(
  s: Pick<ReportSchedule, "chineseWorkdayOnly" | "weekdaysOnly">,
): WeekdayMode {
  if (s.chineseWorkdayOnly) return "chineseWorkday";
  if (s.weekdaysOnly) return "weekdays";
  return "everyday";
}

/** 编辑既有任务时的表单初值(数组拷贝,避免直接改到列表数据) */
export function scheduleToForm(s: ReportSchedule): ScheduleFormState {
  return {
    name: s.name,
    time: s.timeOfDay,
    reportType: s.reportType,
    previousDay: s.previousDay,
    weekdayMode: weekdayModeOf(s),
    weeklyWorkweek: s.weeklyWorkweek,
    weeklyStart: s.weeklyStartWeekday || 1,
    weeklyEnd: s.weeklyEndWeekday || 5,
    authorMode: s.authorMode,
    projectIds: [...s.projectIds],
    tagIds: [...s.tagIds],
  };
}

/** 表单是否可提交:至少勾选一个项目或一个「按标签包含」标签 */
export function isScheduleFormValid(
  form: Pick<ScheduleFormState, "projectIds" | "tagIds">,
): boolean {
  return form.projectIds.length > 0 || form.tagIds.length > 0;
}

/**
 * 由表单构造持久化的定时任务。
 * editing 为 null 时新建:生成 id、默认启用、lastRunAt 为 null;
 * 编辑时保留 id / enabled / lastRunAt;周报时星期过滤标志强制归零。
 */
export function buildScheduleFromForm(
  form: ScheduleFormState,
  editing: ReportSchedule | null,
): ReportSchedule {
  const isDaily = form.reportType === "daily";
  return {
    id: editing?.id ?? crypto.randomUUID(),
    name: form.name.trim(),
    enabled: editing?.enabled ?? true,
    reportType: form.reportType,
    projectIds: [...form.projectIds],
    tagIds: [...form.tagIds],
    authorMode: form.authorMode,
    timeOfDay: form.time,
    weekdaysOnly: isDaily && form.weekdayMode === "weekdays",
    chineseWorkdayOnly: isDaily && form.weekdayMode === "chineseWorkday",
    previousDay: form.previousDay,
    weeklyWorkweek: form.weeklyWorkweek,
    weeklyStartWeekday: form.weeklyStart,
    weeklyEndWeekday: form.weeklyEnd,
    lastRunAt: editing?.lastRunAt ?? null,
  };
}

/** 周一~周日的本地化短标签(Intl,避免 i18n 数组不可靠);2024-01-01 是周一 */
export function weekdayShortNames(locale: string): string[] {
  const fmt = new Intl.DateTimeFormat(locale, { weekday: "short" });
  const mon = new Date(2024, 0, 1);
  return Array.from({ length: 7 }, (_, i) => {
    const d = new Date(mon);
    d.setDate(1 + i);
    return fmt.format(d);
  });
}

/** 周报自定义周期描述:"周一 ~ 周五";工作周模式由调用方显示固定文案 */
export function weeklyRangeLabel(
  s: Pick<ReportSchedule, "weeklyStartWeekday" | "weeklyEndWeekday">,
  names: string[],
): string {
  const start = names[(s.weeklyStartWeekday - 1 + 7) % 7] ?? "";
  const end = names[(s.weeklyEndWeekday - 1 + 7) % 7] ?? "";
  return `${start} ~ ${end}`;
}

/** 系统内置任务执行间隔钳制到 1..1440 分钟(非法输入回退到 fallback) */
export function clampIntervalMinutes(value: number, fallback = 10): number {
  return Math.min(24 * 60, Math.max(1, Math.round(value || fallback)));
}

/** 任务列表展示:按 id 反查项目名称,忽略已删除/未登记的 id */
export function projectNames(ids: number[], projects: Pick<Project, "id" | "name">[]): string[] {
  return ids.map((id) => projects.find((p) => p.id === id)?.name ?? "").filter(Boolean);
}

/** 任务列表展示:按 id 反查标签名称 */
export function tagNames(ids: number[], tags: Pick<Tag, "id" | "name">[]): string[] {
  return ids.map((id) => tags.find((t) => t.id === id)?.name ?? "").filter(Boolean);
}
