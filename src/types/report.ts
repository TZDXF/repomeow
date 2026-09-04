import type { GitCommitInfo } from "./git";

/** 报告类型:日报(单日) | 周报(日期范围) */
export type ReportPeriodType = "daily" | "weekly";

/** 报告历史日历的选中视角:按日 | 按周(周一至周日) | 按月 */
export type ReportViewMode = "day" | "week" | "month";

/** 工作周日期范围(get_work_week_ranges,起止均为 "YYYY-MM-DD") */
export interface WorkWeekRange {
  from: string;
  to: string;
}

/** 本周/上周工作周范围(连续工作周期,含法定节假日/调休识别) */
export interface WorkWeekRanges {
  thisWeek: WorkWeekRange;
  lastWeek: WorkWeekRange;
}

/** 报告历史列表项 */
export interface ReportHistoryItem {
  id: number;
  projectIds: number[];
  dateFrom: string;
  dateTo: string;
  rangeLabel: string;
  authorMode: string;
  language: string;
  periodType: ReportPeriodType;
  createdAt: number;
  projectNames: string[];
  totalCommits: number;
}

/** 报告历史详情(含 Markdown 正文与各项目提交记录) */
export interface ReportHistoryDetail {
  id: number;
  projectIds: number[];
  dateFrom: string;
  dateTo: string;
  rangeLabel: string;
  authorMode: string;
  language: string;
  periodType: ReportPeriodType;
  createdAt: number;
  projectNames: string[];
  totalCommits: number;
  result: string;
  commits: ReportCommitItem[];
}

/** 报告历史中单个项目的提交记录 */
export interface ReportCommitItem {
  projectId: number | null;
  projectName: string;
  projectDescription: string;
  commits: GitCommitInfo[];
}

/** 保存报告时传入的提交数据 */
export interface SaveReportCommit {
  projectId: number | null;
  projectName: string;
  projectDescription?: string;
  commits: GitCommitInfo[];
}

/** 定时任务配置 */
export interface ReportSchedule {
  id: string;
  name: string;
  enabled: boolean;
  /** 报告类型:日报(前一天或当天) | 周报(工作周,最后一个工作日触发) */
  reportType: ReportPeriodType;
  projectIds: number[];
  /** 按标签动态包含:执行时反查带有任一选中标签的未归档项目,与 projectIds 取并集 */
  tagIds: number[];
  authorMode: "me" | "all";
  timeOfDay: string;
  /** 日报:仅周一~周五 */
  weekdaysOnly: boolean;
  /** 日报:仅中国工作日 */
  chineseWorkdayOnly: boolean;
  /** 日报:true = 前一天(次日生成,默认);false = 当天 */
  previousDay: boolean;
  /** 周报:true = 工作周模式(自动识别连续工作周期,末日触发);false = 自定义周几~周几 */
  weeklyWorkweek: boolean;
  /** 周报自定义:范围起始周几(1=周一 .. 7=周日) */
  weeklyStartWeekday: number;
  /** 周报自定义:范围结束/触发周几(1=周一 .. 7=周日) */
  weeklyEndWeekday: number;
  lastRunAt: number | null;
}

/** 应用内置定时任务；可启停和修改间隔，不允许删除。 */
export interface SystemSchedule {
  id: "git_update" | string;
  enabled: boolean;
  intervalMinutes: number;
  lastRunAt: number | null;
}

/** 定时任务触发后发送给前端的通知 */
export interface ReportGeneratedPayload {
  scheduleName: string;
  historyId: number;
  dateFrom: string;
  dateTo: string;
}

/** 节日名称(中/英双语,来自 chinese-days 数据) */
export interface HolidayName {
  en: string;
  zh: string;
}

/** 日历某天各类型报告数量(供日历按类型分色展示标记) */
export interface CalendarDayReports {
  daily: number;
  weekly: number;
}

/** 日历标注数据：某月每天报告数(按类型拆分) + 节假日/调休及其中英节日名 */
export interface CalendarMeta {
  dates: Record<string, CalendarDayReports>;
  holidays: string[];
  workdays: string[];
  /** 法定节假日 → 节日名 */
  holidayNames: Record<string, HolidayName>;
  /** 调休补班日 → 所补节日名 */
  workdayNames: Record<string, HolidayName>;
}

/** 节假日/调休标注数据(get_holiday_data 返回的全集,供日期选择日历高亮) */
export interface HolidayData {
  holidays: string[];
  workdays: string[];
  /** 法定节假日 → 节日名 */
  holidayNames: Record<string, HolidayName>;
  /** 调休补班日 → 所补节日名 */
  workdayNames: Record<string, HolidayName>;
}

/** 批量生成的单个时段(plan_batch_report_ranges;daily 为单日,weekly 为一个工作周) */
export interface BatchRange {
  dateFrom: string;
  dateTo: string;
  isWorkday: boolean;
}

/** 已有报告的日期范围(list_report_dates,供批量生成"跳过已有"匹配) */
export interface ReportDateRange {
  dateFrom: string;
  dateTo: string;
}
