<script setup lang="ts">
import { computed, ref, watch, type Ref } from "vue";
import { useI18n } from "vue-i18n";
import { ChevronLeft, ChevronRight } from "@lucide/vue";
import { getLocalTimeZone, parseDate, today, type DateValue } from "@internationalized/date";
import { CalendarRoot } from "reka-ui";
import {
  CalendarCell,
  CalendarCellTrigger,
  CalendarGrid,
  CalendarGridBody,
  CalendarGridHead,
  CalendarGridRow,
  CalendarHeadCell,
  CalendarHeader,
  CalendarHeading,
  CalendarNextButton,
  CalendarPrevButton,
} from "@/components/ui/calendar";
import type { CalendarMeta, ReportViewMode } from "@/types";
import {
  getHolidayDayClass,
  getHolidayDayTitle,
  isWeekendDate,
  type HolidayDayContext,
} from "@/lib/holidays";
import "@/styles/holiday-calendar.css";
import { useSettingsStore } from "@/stores/settings";

const props = defineProps<{
  modelValue: string | null;
  calendarData: CalendarMeta | null;
  /** 周报时间范围高亮("YYYY-MM-DD" 起止,闭区间);null 表示不高亮 */
  highlightRange?: { start: string; end: string } | null;
  /** 选中视角:日(点选单日) | 周(点选某日即选中其周一至周日整周) | 月(月格点选整月) */
  selectionMode: ReportViewMode;
}>();

const emit = defineEmits<{
  "update:modelValue": [date: string];
  "month-change": [year: number, month: number];
}>();

const { t } = useI18n();
const settings = useSettingsStore();

/** 节假日判定数据(日期集合 + 真实节日名称表),随 calendarData 更新 */
const dayCtx = computed<HolidayDayContext>(() => ({
  holidaySet: new Set(props.calendarData?.holidays ?? []),
  workdaySet: new Set(props.calendarData?.workdays ?? []),
  holidayNames: props.calendarData?.holidayNames ?? {},
  workdayNames: props.calendarData?.workdayNames ?? {},
}));
const reportDates = computed(() => props.calendarData?.dates ?? {});

/** 使用浏览器 Intl API 生成周一~周日的短标签，避免 t() 数组不可靠 */
const weekDayLabels = computed(() => {
  const lang = settings.language;
  const fmt = new Intl.DateTimeFormat(lang, { weekday: lang === "zh-CN" ? "narrow" : "short" });
  // 2024-01-01 is Monday
  const mon = new Date(2024, 0, 1);
  return Array.from({ length: 7 }, (_, i) => {
    const d = new Date(mon);
    d.setDate(1 + i);
    return fmt.format(d);
  });
});

// ── CalendarRoot model: string ↔ DateValue ────────────────────────────

const innerValue = ref<DateValue>() as Ref<DateValue | undefined>;

// parent → calendar
watch(
  () => props.modelValue,
  (v) => {
    if (!v) {
      innerValue.value = undefined;
      return;
    }
    try {
      innerValue.value = parseDate(v);
    } catch {
      // ignore invalid date
    }
  },
  { immediate: true },
);

// calendar → parent
watch(innerValue, (v) => {
  if (v) {
    emit("update:modelValue", v.toString());
  }
});

function onCalendarUpdate(v: DateValue | undefined) {
  innerValue.value = v;
}

// ── month navigation ────────────────────────────────────────────────────

const placeholder = ref(today(getLocalTimeZone())) as Ref<DateValue>;
const lastPlaceholder = ref("");

watch(placeholder, (val) => {
  const ds = val.toString();
  if (ds === lastPlaceholder.value) return;
  lastPlaceholder.value = ds;
  emit("month-change", val.year, val.month);
});

// ── 年/月选择视图(点击标题逐级下钻:日 → 月 → 年)──────────────────────────

type CalendarView = "days" | "months" | "years";
const view = ref<CalendarView>("days");

/**
 * 选中视角驱动内部视图:月视角常驻月格视图(月格即选中态,不再下钻回日视图),
 * 并把 placeholder 对齐到已选月份所在年,保证选中月落在展示年份内;
 * 日/周视角回到日网格(周视角在日网格上整周铺选中带)。
 */
watch(
  () => props.selectionMode,
  (mode) => {
    if (mode === "month") {
      if (props.modelValue) {
        try {
          placeholder.value = parseDate(props.modelValue);
        } catch {
          // ignore invalid date
        }
      }
      view.value = "months";
    } else {
      view.value = "days";
    }
  },
);

/** 月份短标签(1月..12月 / Jan..Dec),用 Intl 生成,无需新增 i18n 词条 */
const monthLabels = computed(() => {
  const fmt = new Intl.DateTimeFormat(settings.language, { month: "short" });
  return Array.from({ length: 12 }, (_, i) => fmt.format(new Date(2024, i, 1)));
});

/** 月选择视图标题:仅年份(zh 显示「2026年」,en 显示「2026」) */
const yearLabel = computed(() =>
  new Intl.DateTimeFormat(settings.language, { year: "numeric" }).format(
    new Date(placeholder.value.year, 0, 1),
  ),
);

/** 年选择视图:当前年所在十年为核心,前后各扩 1 年共 12 格(首尾两格淡化) */
const decadeStart = computed(() => Math.floor(placeholder.value.year / 10) * 10);
const yearCells = computed(() => Array.from({ length: 12 }, (_, i) => decadeStart.value - 1 + i));

/** 月/年视图下的左右翻页:月视图 ±1 年,年视图 ±10 年 */
function shiftView(dir: 1 | -1) {
  placeholder.value =
    view.value === "months"
      ? placeholder.value.add({ years: dir })
      : placeholder.value.add({ years: dir * 10 });
}

function pickMonth(month: number) {
  placeholder.value = placeholder.value.set({ month });
  if (props.selectionMode === "month") {
    // 月视角:月格即选中态,modelValue 落在该月 1 日,由父组件推导出整月范围
    innerValue.value = placeholder.value.set({ day: 1 });
    return;
  }
  view.value = "days";
}

function pickYear(year: number) {
  placeholder.value = placeholder.value.set({ year });
  view.value = "months";
}

/** 高亮规则:已选日期所在月/年铺底色,今天所在月/年标主色 */
function monthCellClass(month: number): string {
  const sel = innerValue.value;
  const now = today(getLocalTimeZone());
  const y = placeholder.value.year;
  const classes: string[] = [];
  const isSelected = !!sel && sel.year === y && sel.month === month;
  if (isSelected) {
    // 月视角下月格即选中态,用主色实心;日/周视角的月视图仅是下钻导航,保持淡底
    classes.push(
      props.selectionMode === "month"
        ? "bg-primary text-primary-foreground font-medium"
        : "bg-primary/10 font-medium",
    );
  }
  if (now.year === y && now.month === month && !(isSelected && props.selectionMode === "month")) {
    classes.push("text-primary");
  }
  return classes.join(" ");
}

function yearCellClass(year: number): string {
  const sel = innerValue.value;
  const now = today(getLocalTimeZone());
  const classes: string[] = [];
  if (year < decadeStart.value || year > decadeStart.value + 9) {
    classes.push("text-muted-foreground/50");
  }
  if (sel && sel.year === year) {
    classes.push("bg-primary/10 font-medium");
  }
  if (now.year === year) {
    classes.push("text-primary");
  }
  return classes.join(" ");
}

// ── helpers ─────────────────────────────────────────────────────────────

function getDayClass(dv: DateValue): string {
  return getHolidayDayClass(dv.toString(), isWeekendDate(dv), dayCtx.value);
}

function getDayTitle(dv: DateValue): string | undefined {
  return getHolidayDayTitle(dv.toString(), isWeekendDate(dv), dayCtx.value, settings.language, t);
}

/** 当天日报数量(中性色圆点) */
function getDailyCount(dv: DateValue): number {
  return reportDates.value[dv.toString()]?.daily ?? 0;
}

/** 当天周报数量(紫色圆点) */
function getWeeklyCount(dv: DateValue): number {
  return reportDates.value[dv.toString()]?.weekly ?? 0;
}

/** 周报范围高亮:日期是否落在 highlightRange 闭区间内(字符串即 ISO 日期,可直接字典序比较) */
function isInHighlightRange(dv: DateValue): boolean {
  const r = props.highlightRange;
  if (!r) return false;
  const ds = dv.toString();
  return ds >= r.start && ds <= r.end;
}

/** 范围高亮 class:中段去圆角连成带,起止日保留外侧圆角(完整类名供 Tailwind 扫描) */
function getHighlightClass(dv: DateValue): string {
  if (!isInHighlightRange(dv)) return "";
  const r = props.highlightRange!;
  const ds = dv.toString();
  if (ds === r.start && ds === r.end) return "bg-primary/10";
  if (ds === r.start) return "bg-primary/10 rounded-r-none";
  if (ds === r.end) return "bg-primary/10 rounded-l-none";
  return "bg-primary/10 rounded-none";
}

/** 周视角的选中周范围:已选日期所在周一至周日(与 week-starts-on=1 一致) */
const selectedWeekRange = computed(() => {
  if (props.selectionMode !== "week") return null;
  const sel = innerValue.value;
  if (!sel) return null;
  const dow = (sel.toDate(getLocalTimeZone()).getDay() + 6) % 7;
  const start = sel.subtract({ days: dow });
  return { start: start.toString(), end: start.add({ days: 6 }).toString() };
});

/**
 * 选中周铺带 class:比报告范围高亮(bg-primary/10)更实一档以示选中,
 * 同日两带重叠时以选中带为准(模板里 || 短路,避免 bg 类冲突结果不可预期)
 */
function getSelectionClass(dv: DateValue): string {
  const r = selectedWeekRange.value;
  if (!r) return "";
  const ds = dv.toString();
  if (ds < r.start || ds > r.end) return "";
  if (ds === r.start) return "bg-primary/15 rounded-r-none";
  if (ds === r.end) return "bg-primary/15 rounded-l-none";
  return "bg-primary/15 rounded-none";
}

/** Type helper: narrow grid cell date to DateValue for CalendarCellTrigger */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
function asDateValue(dv: any): DateValue {
  return dv as DateValue;
}
</script>

<template>
  <CalendarRoot
    v-slot="{ grid }"
    :model-value="innerValue"
    @update:model-value="onCalendarUpdate"
    v-model:placeholder="placeholder"
    :week-starts-on="1"
    :locale="settings.language"
    class="p-1"
  >
    <CalendarHeader class="mb-1">
      <!-- nav 全宽覆盖在标题上方,pointer-events-none 让标题按钮可点,翻页按钮自身恢复 -->
      <nav
        class="pointer-events-none absolute inset-x-0 top-0 flex items-center justify-between px-1"
      >
        <!-- 日视图用 reka 自带翻页(处理禁用态);月/年视图直接改 placeholder -->
        <CalendarPrevButton
          v-if="view === 'days'"
          class="pointer-events-auto size-7 bg-transparent p-0 opacity-50 hover:opacity-100 border rounded-md inline-flex items-center justify-center"
        >
          <ChevronLeft class="size-4" />
        </CalendarPrevButton>
        <button
          v-else
          type="button"
          class="pointer-events-auto size-7 bg-transparent p-0 opacity-50 hover:opacity-100 border rounded-md inline-flex items-center justify-center"
          @click="shiftView(-1)"
        >
          <ChevronLeft class="size-4" />
        </button>
        <CalendarNextButton
          v-if="view === 'days'"
          class="pointer-events-auto size-7 bg-transparent p-0 opacity-50 hover:opacity-100 border rounded-md inline-flex items-center justify-center"
        >
          <ChevronRight class="size-4" />
        </CalendarNextButton>
        <button
          v-else
          type="button"
          class="pointer-events-auto size-7 bg-transparent p-0 opacity-50 hover:opacity-100 border rounded-md inline-flex items-center justify-center"
          @click="shiftView(1)"
        >
          <ChevronRight class="size-4" />
        </button>
      </nav>
      <!-- 标题逐级下钻:日(年月) → 月(年) → 年(十年区间,到顶不可再点) -->
      <CalendarHeading v-if="view === 'days'" v-slot="{ headingValue }" as-child>
        <button
          type="button"
          class="text-sm font-medium rounded-md px-2 py-0.5 hover:bg-accent"
          @click="view = 'months'"
        >
          {{ headingValue }}
        </button>
      </CalendarHeading>
      <button
        v-else-if="view === 'months'"
        type="button"
        class="text-sm font-medium rounded-md px-2 py-0.5 hover:bg-accent"
        @click="view = 'years'"
      >
        {{ yearLabel }}
      </button>
      <div v-else class="text-sm font-medium px-2 py-0.5">
        {{ decadeStart }}–{{ decadeStart + 9 }}
      </div>
    </CalendarHeader>

    <!-- 月选择视图:12 个月,选中后回到日视图 -->
    <div v-if="view === 'months'" class="grid grid-cols-3 gap-1 p-1">
      <button
        v-for="(label, i) in monthLabels"
        :key="i"
        type="button"
        class="flex h-9 items-center justify-center rounded-md text-sm hover:bg-accent"
        :class="monthCellClass(i + 1)"
        @click="pickMonth(i + 1)"
      >
        {{ label }}
      </button>
    </div>

    <!-- 年选择视图:十年区间 12 格,选中后进入月选择 -->
    <div v-else-if="view === 'years'" class="grid grid-cols-3 gap-1 p-1">
      <button
        v-for="y in yearCells"
        :key="y"
        type="button"
        class="flex h-9 items-center justify-center rounded-md text-sm hover:bg-accent"
        :class="yearCellClass(y)"
        @click="pickYear(y)"
      >
        {{ y }}
      </button>
    </div>

    <!-- 日视图保持挂载(v-show),避免来回切换时 reka 网格状态重建 -->
    <CalendarGrid v-for="month in grid" v-show="view === 'days'" :key="month.value.toString()">
      <CalendarGridHead>
        <CalendarGridRow>
          <CalendarHeadCell
            v-for="day in weekDayLabels"
            :key="day"
            class="text-muted-foreground flex-1 font-normal text-[0.8rem] text-center"
          >
            {{ day }}
          </CalendarHeadCell>
        </CalendarGridRow>
      </CalendarGridHead>
      <CalendarGridBody>
        <CalendarGridRow v-for="(row, _idx) in month.rows" :key="_idx" class="flex">
          <CalendarCell
            v-for="cellDate in row"
            :key="cellDate.toString()"
            :date="asDateValue(cellDate)"
          >
            <CalendarCellTrigger
              :day="asDateValue(cellDate)"
              :month="month.value"
              :class="[
                getDayClass(cellDate),
                getSelectionClass(cellDate) || getHighlightClass(cellDate),
              ]"
              :title="getDayTitle(cellDate)"
              class="group/cell flex size-8 flex-col items-center justify-center rounded-md p-0 font-normal text-sm"
            >
              <span class="leading-none">{{ cellDate.day }}</span>
              <!-- 报告标记行固定高度,无报告留空对齐:日报=中性点、周报=紫点(与右侧列表徽章同色);
                   选中态底色为主色,中性点切换为主色前景保证对比 -->
              <span class="mt-0.5 flex h-1.5 items-center justify-center gap-1">
                <span
                  v-if="getDailyCount(cellDate) > 0"
                  class="h-1.5 w-1.5 rounded-full bg-foreground/60 group-data-[selected]/cell:bg-primary-foreground/80"
                />
                <span
                  v-if="getWeeklyCount(cellDate) > 0"
                  class="h-1.5 w-1.5 rounded-full bg-violet-500"
                />
              </span>
            </CalendarCellTrigger>
          </CalendarCell>
        </CalendarGridRow>
      </CalendarGridBody>
    </CalendarGrid>
  </CalendarRoot>
</template>
