<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { getLocalTimeZone, today as calendarToday } from "@internationalized/date";
import { Calendar as CalendarIcon } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Switch } from "@/components/ui/switch";
import HolidayCalendar from "@/components/report/HolidayCalendar.vue";
import HolidayRangeCalendar from "@/components/report/HolidayRangeCalendar.vue";
import {
  toLocalDate,
  type BatchFilter,
  type DailyRangeKey,
  type RangeDateValue,
  type ReportPeriodSelection,
  type SingleDateValue,
  type WeeklyRangeKey,
} from "@/components/report/report-period";
import { formatDate } from "@/lib/format";
import type { ReportPeriodType } from "@/types";

const BATCH_FILTER_OPTIONS: { value: BatchFilter; labelKey: string }[] = [
  { value: "workdays", labelKey: "report.batchFilterWorkdays" },
  { value: "hasCommits", labelKey: "report.batchFilterHasCommits" },
];

const MODE_OPTIONS: { value: ReportPeriodType; labelKey: string }[] = [
  { value: "daily", labelKey: "report.modeDaily" },
  { value: "weekly", labelKey: "report.modeWeekly" },
];

const DAILY_RANGE_OPTIONS: { value: DailyRangeKey; labelKey: string }[] = [
  { value: "today", labelKey: "report.today" },
  { value: "yesterday", labelKey: "report.yesterday" },
  { value: "custom", labelKey: "report.custom" },
];

const WEEKLY_RANGE_OPTIONS: { value: WeeklyRangeKey; labelKey: string }[] = [
  { value: "thisWeek", labelKey: "report.thisWeek" },
  { value: "lastWeek", labelKey: "report.lastWeek" },
  { value: "custom", labelKey: "report.custom" },
];

const props = defineProps<{
  language: string;
  batchRunning: boolean;
  weekRanges: Record<"thisWeek" | "lastWeek", { from: Date; to: Date }>;
}>();
const selection = defineModel<ReportPeriodSelection>({ required: true });
const { t } = useI18n();

const isBatch = computed(() => selection.value.execMode === "batch");
const maxDate = calendarToday(getLocalTimeZone()) as unknown as RangeDateValue;
const maxDateSingle = calendarToday(getLocalTimeZone()) as unknown as SingleDateValue;

function update(values: Partial<ReportPeriodSelection>) {
  selection.value = { ...selection.value, ...values };
}

const batchSwitch = computed({
  get: () => isBatch.value,
  set: (value: boolean) => update({ execMode: value ? "batch" : "single" }),
});
const batchRange = computed({
  get: () => selection.value.batchRange,
  set: (value: ReportPeriodSelection["batchRange"]) => update({ batchRange: value }),
});
const customDate = computed({
  get: () => selection.value.customDate,
  set: (value: ReportPeriodSelection["customDate"]) => update({ customDate: value }),
});
const customRange = computed({
  get: () => selection.value.customRange,
  set: (value: ReportPeriodSelection["customRange"]) => update({ customRange: value }),
});
const batchSkipExisting = computed({
  get: () => selection.value.batchSkipExisting,
  set: (value: boolean) => update({ batchSkipExisting: value }),
});

const batchRangeLabel = computed(() => {
  const { start, end } = selection.value.batchRange;
  if (start && end) {
    return `${formatDate(toLocalDate(start))} - ${formatDate(toLocalDate(end))}`;
  }
  const single = start ?? end;
  return single ? formatDate(toLocalDate(single)) : t("report.pickRange");
});
const customRangeLabel = computed(() => {
  const { start, end } = selection.value.customRange;
  if (start && end) {
    return `${formatDate(toLocalDate(start))} - ${formatDate(toLocalDate(end))}`;
  }
  const single = start ?? end;
  return single ? formatDate(toLocalDate(single)) : t("report.pickRange");
});
const customDateLabel = computed(() =>
  selection.value.customDate
    ? formatDate(toLocalDate(selection.value.customDate))
    : t("report.pickDate"),
);
const selectedWeekRange = computed(() =>
  selection.value.weeklyKey === "custom" ? null : props.weekRanges[selection.value.weeklyKey],
);

function fmtWeekRange(range: { from: Date; to: Date }) {
  return `${formatDate(range.from)} ~ ${formatDate(range.to)}`;
}
</script>

<template>
  <div class="flex flex-col gap-1.5">
    <label class="text-sm font-medium">{{ t("report.mode") }}</label>
    <div class="flex items-center gap-1.5">
      <Button
        v-for="option in MODE_OPTIONS"
        :key="option.value"
        size="sm"
        :variant="selection.mode === option.value ? 'default' : 'outline'"
        class="h-7 px-2.5 text-xs"
        :disabled="batchRunning"
        @click="update({ mode: option.value })"
      >
        {{ t(option.labelKey) }}
      </Button>
    </div>
  </div>

  <div class="flex flex-col gap-1.5">
    <div class="flex items-center gap-2">
      <label class="text-sm font-medium">{{ t("report.range") }}</label>
      <div class="flex items-center gap-1.5">
        <span class="text-xs" :class="isBatch ? 'text-muted-foreground' : 'font-medium'">
          {{ t("report.execSingle") }}
        </span>
        <Switch v-model="batchSwitch" :disabled="batchRunning" />
        <span class="text-xs" :class="isBatch ? 'font-medium' : 'text-muted-foreground'">
          {{ t("report.execBatch") }}
        </span>
      </div>
    </div>

    <div v-if="isBatch" class="flex flex-wrap items-center gap-1.5">
      <Popover>
        <PopoverTrigger as-child>
          <Button
            variant="outline"
            size="sm"
            class="h-7 gap-1.5 px-2.5 text-xs font-normal"
            :class="{ 'text-muted-foreground': !batchRange.start && !batchRange.end }"
          >
            <CalendarIcon class="h-3.5 w-3.5" />
            {{ batchRangeLabel }}
          </Button>
        </PopoverTrigger>
        <PopoverContent class="w-auto p-0" align="start">
          <HolidayRangeCalendar
            v-model="batchRange"
            :number-of-months="2"
            :locale="language"
            :max-value="maxDate"
          />
        </PopoverContent>
      </Popover>
    </div>

    <div v-else-if="selection.mode === 'daily'" class="flex flex-wrap items-center gap-1.5">
      <Button
        v-for="option in DAILY_RANGE_OPTIONS"
        :key="option.value"
        size="sm"
        :variant="selection.dailyKey === option.value ? 'default' : 'outline'"
        class="h-7 px-2.5 text-xs"
        @click="update({ dailyKey: option.value })"
      >
        {{ t(option.labelKey) }}
      </Button>
      <Popover v-if="selection.dailyKey === 'custom'">
        <PopoverTrigger as-child>
          <Button
            variant="outline"
            size="sm"
            class="h-7 gap-1.5 px-2.5 text-xs font-normal"
            :class="{ 'text-muted-foreground': !customDate }"
          >
            <CalendarIcon class="h-3.5 w-3.5" />
            {{ customDateLabel }}
          </Button>
        </PopoverTrigger>
        <PopoverContent class="w-auto p-0" align="start">
          <HolidayCalendar v-model="customDate" :locale="language" :max-value="maxDateSingle" />
        </PopoverContent>
      </Popover>
    </div>

    <div v-else class="flex flex-wrap items-center gap-1.5">
      <Button
        v-for="option in WEEKLY_RANGE_OPTIONS"
        :key="option.value"
        size="sm"
        :variant="selection.weeklyKey === option.value ? 'default' : 'outline'"
        class="h-7 px-2.5 text-xs"
        @click="update({ weeklyKey: option.value })"
      >
        {{ t(option.labelKey) }}
      </Button>
      <Popover v-if="selection.weeklyKey === 'custom'">
        <PopoverTrigger as-child>
          <Button
            variant="outline"
            size="sm"
            class="h-7 gap-1.5 px-2.5 text-xs font-normal"
            :class="{ 'text-muted-foreground': !customRange.start && !customRange.end }"
          >
            <CalendarIcon class="h-3.5 w-3.5" />
            {{ customRangeLabel }}
          </Button>
        </PopoverTrigger>
        <PopoverContent class="w-auto p-0" align="start">
          <HolidayRangeCalendar
            v-model="customRange"
            :number-of-months="2"
            :locale="language"
            :max-value="maxDate"
          />
        </PopoverContent>
      </Popover>
    </div>

    <p
      v-if="!isBatch && selection.mode === 'weekly' && selectedWeekRange"
      class="text-xs text-muted-foreground"
    >
      {{ fmtWeekRange(selectedWeekRange) }}
    </p>
  </div>

  <template v-if="isBatch">
    <label class="flex cursor-pointer items-center gap-2 text-sm">
      <input
        v-model="batchSkipExisting"
        type="checkbox"
        class="h-3.5 w-3.5 shrink-0 accent-primary"
        :disabled="batchRunning"
      />
      {{ t("report.batchSkipExisting") }}
    </label>
    <div v-if="selection.mode === 'daily'" class="flex flex-col gap-1.5">
      <label class="text-sm font-medium">{{ t("report.batchFilter") }}</label>
      <div class="flex flex-wrap items-center gap-1.5">
        <Button
          v-for="option in BATCH_FILTER_OPTIONS"
          :key="option.value"
          size="sm"
          :variant="selection.batchFilter === option.value ? 'default' : 'outline'"
          class="h-7 px-2.5 text-xs"
          :disabled="batchRunning"
          @click="update({ batchFilter: option.value })"
        >
          {{ t(option.labelKey) }}
        </Button>
      </div>
    </div>
  </template>
</template>
