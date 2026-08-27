<script setup lang="ts">
import type { CalendarRootEmits, CalendarRootProps, DateValue } from "reka-ui";
import { computed, type HTMLAttributes, type Ref } from "vue";
import { getLocalTimeZone, today } from "@internationalized/date";
import { reactiveOmit, useVModel } from "@vueuse/core";
import { CalendarRoot, useForwardPropsEmits } from "reka-ui";
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

/** 单日选择日历(ui Calendar 的节假日版):高亮法定节假日/调休补班/周末,与报告历史日历一致 */
const props = withDefaults(defineProps<CalendarRootProps & { class?: HTMLAttributes["class"] }>(), {
  modelValue: undefined,
});
const emits = defineEmits<CalendarRootEmits>();

const delegatedProps = reactiveOmit(props, "class", "placeholder");

const placeholder = useVModel(props, "placeholder", emits, {
  passive: true,
  defaultValue: props.defaultPlaceholder ?? today(getLocalTimeZone()),
}) as Ref<DateValue>;

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
  <CalendarRoot
    v-slot="{ grid, weekDays }"
    v-bind="forwarded"
    v-model:placeholder="placeholder"
    :week-starts-on="1"
    data-slot="calendar"
    :class="cn('p-3', props.class)"
  >
    <CalendarHeader class="pt-0">
      <nav class="absolute inset-x-0 top-0 flex items-center justify-between">
        <CalendarPrevButton />
        <CalendarNextButton />
      </nav>
      <CalendarHeading />
    </CalendarHeader>

    <div class="mt-4 flex flex-col gap-y-4 sm:flex-row sm:gap-x-4 sm:gap-y-0">
      <CalendarGrid v-for="month in grid" :key="month.value.toString()">
        <CalendarGridHead>
          <CalendarGridRow>
            <CalendarHeadCell v-for="day in weekDays" :key="day">
              {{ day }}
            </CalendarHeadCell>
          </CalendarGridRow>
        </CalendarGridHead>
        <CalendarGridBody>
          <CalendarGridRow
            v-for="(weekDates, index) in month.rows"
            :key="`weekDate-${index}`"
            class="mt-2 w-full"
          >
            <CalendarCell v-for="weekDate in weekDates" :key="weekDate.toString()" :date="weekDate">
              <CalendarCellTrigger
                :day="weekDate"
                :month="month.value"
                :class="dayClass(weekDate)"
                :title="dayTitle(weekDate)"
              />
            </CalendarCell>
          </CalendarGridRow>
        </CalendarGridBody>
      </CalendarGrid>
    </div>
  </CalendarRoot>
</template>
