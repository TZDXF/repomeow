import { getLocalTimeZone, today as calendarToday } from "@internationalized/date";
import type { CalendarRootProps, RangeCalendarRootProps } from "reka-ui";
import type { ReportPeriodType } from "@/types";

export type DailyRangeKey = "today" | "yesterday" | "custom";
export type WeeklyRangeKey = "thisWeek" | "lastWeek" | "custom";
export type ExecMode = "single" | "batch";
export type BatchFilter = "workdays" | "hasCommits";

export type RangeModel = NonNullable<RangeCalendarRootProps["modelValue"]>;
export type RangeDateValue = NonNullable<RangeModel["start"]>;
export type SingleDateValue = Exclude<NonNullable<CalendarRootProps["modelValue"]>, unknown[]>;

export interface ReportPeriodSelection {
  mode: ReportPeriodType;
  dailyKey: DailyRangeKey;
  weeklyKey: WeeklyRangeKey;
  execMode: ExecMode;
  customRange: RangeModel;
  customDate: SingleDateValue | undefined;
  batchRange: RangeModel;
  batchSkipExisting: boolean;
  batchFilter: BatchFilter;
}

function defaultBatchRange(): RangeModel {
  const end = calendarToday(getLocalTimeZone());
  const start = end.subtract({ days: 6 });
  return {
    start: start as unknown as RangeDateValue,
    end: end as unknown as RangeDateValue,
  };
}

export function createDefaultReportPeriod(): ReportPeriodSelection {
  return {
    mode: "daily",
    dailyKey: "today",
    weeklyKey: "thisWeek",
    execMode: "single",
    customRange: { start: undefined, end: undefined },
    customDate: undefined,
    batchRange: defaultBatchRange(),
    batchSkipExisting: true,
    batchFilter: "workdays",
  };
}

export function toLocalDate(date: RangeDateValue | SingleDateValue) {
  return date.toDate(getLocalTimeZone());
}
