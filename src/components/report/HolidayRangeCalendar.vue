<script setup lang="ts">
import type { DateValue, RangeCalendarRootEmits, RangeCalendarRootProps } from "reka-ui";
import { computed, type HTMLAttributes } from "vue";
import { reactiveOmit } from "@vueuse/core";
import { RangeCalendarRoot, useForwardPropsEmits } from "reka-ui";
import { useI18n } from "vue-i18n";
import { cn } from "@/lib/utils";
import {
  getHolidayDayClass,
  getHolidayDayTitle,
  isWeekendDate,
  useHolidayData,
  type HolidayDayContext,
} from "@/lib/holidays";
import "@/styles/holiday-calendar.css";
import {
  RangeCalendarCell,
  RangeCalendarCellTrigger,
  RangeCalendarGrid,
  RangeCalendarGridBody,
  RangeCalendarGridHead,
  RangeCalendarGridRow,
  RangeCalendarHeadCell,
  RangeCalendarHeader,
  RangeCalendarHeading,
  RangeCalendarNextButton,
  RangeCalendarPrevButton,
} from "@/components/ui/range-calendar";

/** 范围选择日历(ui RangeCalendar 的节假日版):高亮法定节假日/调休补班/周末,与报告历史日历一致 */
const props = defineProps<RangeCalendarRootProps & { class?: HTMLAttributes["class"] }>();
const emits = defineEmits<RangeCalendarRootEmits>();

const delegatedProps = reactiveOmit(props, "class");

const forwarded = useForwardPropsEmits(delegatedProps, emits);

const { t, locale } = useI18n();
const { holidaySet, workdaySet, holidayNames, workdayNames } = useHolidayData();

/** 节假日判定数据(日期集合 + 真实节日名称表) */
const dayCtx = computed<HolidayDayContext>(() => ({
  holidaySet: holidaySet.value,
  workdaySet: workdaySet.value,
  holidayNames: holidayNames.value,
  workdayNames: workdayNames.value,
}));

function dayClass(dv: DateValue): string {
  return getHolidayDayClass(dv.toString(), isWeekendDate(dv), dayCtx.value);
}

function dayTitle(dv: DateValue): string | undefined {
  return getHolidayDayTitle(dv.toString(), isWeekendDate(dv), dayCtx.value, locale.value, t);
}
</script>

<template>
  <RangeCalendarRoot
    v-slot="{ grid, weekDays }"
    v-bind="forwarded"
    :week-starts-on="1"
    data-slot="range-calendar"
    :class="cn('p-3', props.class)"
  >
    <RangeCalendarHeader>
      <RangeCalendarHeading />
      <div class="flex items-center gap-1">
        <RangeCalendarPrevButton />
        <RangeCalendarNextButton />
      </div>
    </RangeCalendarHeader>

    <div class="mt-4 flex flex-col gap-y-4 sm:flex-row sm:gap-x-4 sm:gap-y-0">
      <RangeCalendarGrid v-for="month in grid" :key="month.value.toString()">
        <RangeCalendarGridHead>
          <RangeCalendarGridRow>
            <RangeCalendarHeadCell v-for="day in weekDays" :key="day">
              {{ day }}
            </RangeCalendarHeadCell>
          </RangeCalendarGridRow>
        </RangeCalendarGridHead>
        <RangeCalendarGridBody>
          <RangeCalendarGridRow
            v-for="(weekDates, index) in month.rows"
            :key="`weekDate-${index}`"
            class="mt-2 w-full"
          >
            <RangeCalendarCell
              v-for="weekDate in weekDates"
              :key="weekDate.toString()"
              :date="weekDate"
            >
              <RangeCalendarCellTrigger
                :day="weekDate"
                :month="month.value"
                :class="dayClass(weekDate)"
                :title="dayTitle(weekDate)"
              />
            </RangeCalendarCell>
          </RangeCalendarGridRow>
        </RangeCalendarGridBody>
      </RangeCalendarGrid>
    </div>
  </RangeCalendarRoot>
</template>
