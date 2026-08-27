import { computed, ref, type ComputedRef } from "vue";
import type { ComposerTranslation } from "vue-i18n";
import { getLocalTimeZone, type DateValue } from "@internationalized/date";
import { cmd } from "@/lib/tauri";
import type { HolidayData, HolidayName } from "@/types";

// 模块级缓存:节假日/调休为全集数据(get_holiday_data 一次返回 2004–2026),
// 一次拉取供多个日历组件共享;数据到达前为空集合,日历先按常规周末着色
const holidayDates = ref<string[]>([]);
const workdayDates = ref<string[]>([]);
const holidayNameMap = ref<Record<string, HolidayName>>({});
const workdayNameMap = ref<Record<string, HolidayName>>({});
let loadPromise: Promise<void> | null = null;

/** 法定节假日/调休补班数据(响应式;首次调用时从后端拉取,失败回退空集合) */
export function useHolidayData(): {
  holidaySet: ComputedRef<Set<string>>;
  workdaySet: ComputedRef<Set<string>>;
  holidayNames: ComputedRef<Record<string, HolidayName>>;
  workdayNames: ComputedRef<Record<string, HolidayName>>;
} {
  if (!loadPromise) {
    loadPromise = cmd<HolidayData>("get_holiday_data")
      .then((data) => {
        holidayDates.value = data.holidays;
        workdayDates.value = data.workdays;
        holidayNameMap.value = data.holidayNames;
        workdayNameMap.value = data.workdayNames;
      })
      .catch(() => {
        // 拉取失败:保持空集合,日历退化为常规周末着色
      });
  }
  return {
    holidaySet: computed(() => new Set(holidayDates.value)),
    workdaySet: computed(() => new Set(workdayDates.value)),
    holidayNames: computed(() => holidayNameMap.value),
    workdayNames: computed(() => workdayNameMap.value),
  };
}

/** 日历日期着色/提示所需的节假日数据(日期集合 + 节日名称表) */
export interface HolidayDayContext {
  holidaySet: Set<string>;
  workdaySet: Set<string>;
  holidayNames?: Record<string, HolidayName>;
  workdayNames?: Record<string, HolidayName>;
}

/** 是否周末(本地时区) */
export function isWeekendDate(dv: DateValue): boolean {
  const d = dv.toDate(getLocalTimeZone());
  return d.getDay() === 0 || d.getDay() === 6;
}

/** 日历日期 class:法定节假日红 > 调休补班绿 > 普通周末淡红(与报告历史日历一致) */
export function getHolidayDayClass(ds: string, weekend: boolean, ctx: HolidayDayContext): string {
  const classes: string[] = [];
  if (ctx.holidaySet.has(ds)) {
    classes.push("report-calendar-holiday");
  } else if (weekend && !ctx.workdaySet.has(ds)) {
    classes.push("report-calendar-weekend");
  }
  if (ctx.workdaySet.has(ds)) {
    classes.push("report-calendar-makeup");
  }
  return classes.join(" ");
}

/** 按界面语言取节日名:中文环境取中文名,其余语言取英文名 */
export function localizedHolidayName(
  name: HolidayName | undefined,
  lang: string,
): string | undefined {
  if (!name) return undefined;
  return lang.startsWith("zh") ? name.zh : name.en;
}

/** 日历日期悬停提示:节假日/调休补班优先展示真实节日名(如「春节」),无名称数据时回退通用类别 */
export function getHolidayDayTitle(
  ds: string,
  weekend: boolean,
  ctx: HolidayDayContext,
  lang: string,
  t: ComposerTranslation,
): string | undefined {
  if (ctx.holidaySet.has(ds)) {
    return localizedHolidayName(ctx.holidayNames?.[ds], lang) ?? t("reportHistory.holiday");
  }
  if (ctx.workdaySet.has(ds)) {
    const base = t("reportHistory.makeupWorkday");
    const name = localizedHolidayName(ctx.workdayNames?.[ds], lang);
    return name ? `${base} · ${name}` : base;
  }
  if (weekend) return t("reportHistory.weekend");
  return undefined;
}
