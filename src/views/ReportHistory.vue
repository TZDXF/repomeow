<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { save } from "@tauri-apps/plugin-dialog";
import { CalendarIcon, Download, FileText, Loader2, Tags } from "@lucide/vue";
import type { UnlistenFn } from "@tauri-apps/api/event";
import type { ControlsConfig } from "vue-stream-markdown";
import { Badge } from "@/components/ui/badge";
import ConfirmDialog from "@/components/common/ConfirmDialog.vue";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import ScrollArea from "@/components/common/ScrollArea.vue";
import DailyReportDialog from "@/components/report/DailyReportDialog.vue";
import ReportCalendar from "@/components/report/ReportCalendar.vue";
import ProjectFilterSelect from "@/components/report/history/ProjectFilterSelect.vue";
import ReportCard from "@/components/report/history/ReportCard.vue";
import TagCheckList from "@/components/tags/TagCheckList.vue";
import { cmd, onListen } from "@/lib/tauri";
import { formatDate, formatLocalDateTime, parseDateStr } from "@/lib/format";
import { localizedHolidayName } from "@/lib/holidays";
import { createBeforeDownload, createTableCustomize } from "@/lib/markdown-download";
import {
  buildReportExportMarkdown,
  createReportExportFilename,
  type ReportExportLabels,
} from "@/lib/report-export";
import { useSettingsStore } from "@/stores/settings";
import { useProjectsStore } from "@/stores/projects";
import { useTagsStore } from "@/stores/tags";
import type {
  CalendarMeta,
  ReportGeneratedPayload,
  ReportHistoryDetail,
  ReportPeriodType,
  ReportViewMode,
} from "@/types";

type TypeFilter = "all" | ReportPeriodType;

/**
 * dotClass 让日报/周报筛选按钮兼任颜色图例,与日历标记、右侧列表徽章一致:
 * 日报=中性色、周报=紫色。日报点用 bg-current 跟随按钮文字色:
 * 普通主题选中态按钮底为实心主色,玻璃拟态主题下为透明底+主色文字,
 * 写死前景色会在其中一种形态里消失,跟随文字色则两种主题都保持可见。
 */
const TYPE_OPTIONS: { value: TypeFilter; labelKey: string; dotClass?: string }[] = [
  { value: "all", labelKey: "reportHistory.typeAll" },
  { value: "daily", labelKey: "reportHistory.typeDaily", dotClass: "bg-current opacity-60" },
  { value: "weekly", labelKey: "reportHistory.typeWeekly", dotClass: "bg-violet-500" },
];

/** 日历选中视角:日(单日) | 周(周一至周日) | 月(整月),决定右侧列表的日期范围 */
const VIEW_OPTIONS: { value: ReportViewMode; labelKey: string }[] = [
  { value: "day", labelKey: "reportHistory.viewDay" },
  { value: "week", labelKey: "reportHistory.viewWeek" },
  { value: "month", labelKey: "reportHistory.viewMonth" },
];

const { t } = useI18n();
const settings = useSettingsStore();
const projectStore = useProjectsStore();
const tagsStore = useTagsStore();

// ── calendar ────────────────────────────────────────────────────────────

const calendarYear = ref(new Date().getFullYear());
const calendarMonth = ref(new Date().getMonth() + 1);
const calendarData = ref<CalendarMeta | null>(null);
const calendarLoading = ref(false);

/**
 * 延迟显示的轻量刷新指示:切月/筛选时保留旧数据静默刷新,
 * 请求超过 200ms 才在日历右上角亮小 spinner,避免全遮罩反复显隐造成闪烁
 */
const calendarRefreshing = ref(false);
let refreshingTimer: ReturnType<typeof setTimeout> | undefined;

watch(calendarLoading, (v) => {
  if (v) {
    refreshingTimer = setTimeout(() => {
      calendarRefreshing.value = true;
    }, 200);
  } else {
    clearTimeout(refreshingTimer);
    calendarRefreshing.value = false;
  }
});

// ── selection ───────────────────────────────────────────────────────────

const selectedDate = ref<string | null>(formatDate(new Date()));
const viewMode = ref<ReportViewMode>("day");
const filterProjectIds = ref<number[]>([]);
const filterTagIds = ref<number[]>([]);
const filterType = ref<TypeFilter>("all");

/** 当前选中范围(闭区间 "YYYY-MM-DD"):日视角为单日,周视角为周一至周日,月视角为整月 */
const selectedRange = computed<{ from: string; to: string } | null>(() => {
  const ds = selectedDate.value;
  if (!ds) return null;
  if (viewMode.value === "day") return { from: ds, to: ds };
  const d = parseDateStr(ds);
  if (isNaN(d.getTime())) return null;
  if (viewMode.value === "week") {
    // 与日历 week-starts-on=1 一致:所在周周一至周日
    const dow = (d.getDay() + 6) % 7;
    const start = new Date(d);
    start.setDate(d.getDate() - dow);
    const end = new Date(start);
    end.setDate(start.getDate() + 6);
    return { from: formatDate(start), to: formatDate(end) };
  }
  const from = new Date(d.getFullYear(), d.getMonth(), 1);
  const to = new Date(d.getFullYear(), d.getMonth() + 1, 0);
  return { from: formatDate(from), to: formatDate(to) };
});

/** 传给后端的类型过滤参数("all" 时不过滤) */
const reportTypeParam = computed(() => (filterType.value === "all" ? null : filterType.value));

function toggleProjectFilter(id: number) {
  filterProjectIds.value = filterProjectIds.value.includes(id)
    ? filterProjectIds.value.filter((x) => x !== id)
    : [...filterProjectIds.value, id];
}

function toggleTagFilter(id: number) {
  filterTagIds.value = filterTagIds.value.includes(id)
    ? filterTagIds.value.filter((x) => x !== id)
    : [...filterTagIds.value, id];
}

/** 已选标签对象列表 */
const selectedTags = computed(() =>
  tagsStore.tags.filter((t) => filterTagIds.value.includes(t.id)),
);

const reports = ref<ReportHistoryDetail[]>([]);
const reportsLoading = ref(false);

// ── generate report dialog ──────────────────────────────────────────────

/** 右上角「生成报告」弹窗 */
const reportOpen = ref(false);

/**
 * 报告保存成功(手动/批量/定时)后后端会 emit report://generated,
 * 收到即刷新日历标注与当日列表,新生成的报告立即可见
 */
let unlistenReportGenerated: UnlistenFn | undefined;

onMounted(async () => {
  unlistenReportGenerated = await onListen<ReportGeneratedPayload>("report://generated", () => {
    loadCalendarMeta(calendarYear.value, calendarMonth.value);
    const r = selectedRange.value;
    if (r) {
      loadReports(r.from, r.to);
    }
  });
});

onUnmounted(() => {
  unlistenReportGenerated?.();
  clearTimeout(refreshingTimer);
});

// ── expand state ────────────────────────────────────────────────────────

const expandedReportId = ref<number | null>(null);

/** 当前展开的报告;周报时在日历上高亮整个时间范围 */
const highlightRange = computed(() => {
  const r = reports.value.find((x) => x.id === expandedReportId.value);
  return r && r.periodType === "weekly" ? { start: r.dateFrom, end: r.dateTo } : null;
});

// ── derived ─────────────────────────────────────────────────────────────

const activeProjects = computed(() => projectStore.projects.filter((p) => !p.archived_at));

/** 选中范围的格式化描述(日:单日;周:起止区间;月:年月),用浏览器 Intl API 避免 i18n 数组不可靠 */
const rangeLabel = computed(() => {
  const range = selectedRange.value;
  if (!range) return "";
  const from = parseDateStr(range.from);
  const to = parseDateStr(range.to);
  if (isNaN(from.getTime()) || isNaN(to.getTime())) return "";
  const lang = settings.language;
  if (viewMode.value === "day") {
    const wd = from.toLocaleDateString(lang, { weekday: "short" });
    // zh-CN: "2026年7月15日 周三", en-US: "July 15, 2026 Wed"
    if (lang === "zh-CN") {
      return `${from.getFullYear()}年${from.getMonth() + 1}月${from.getDate()}日 ${wd}`;
    }
    return `${from.toLocaleDateString(lang, { month: "long", day: "numeric" })}, ${from.getFullYear()} ${wd}`;
  }
  if (viewMode.value === "month") {
    // zh-CN: "2026年8月", en-US: "August 2026"
    if (lang === "zh-CN") {
      return `${from.getFullYear()}年${from.getMonth() + 1}月`;
    }
    return from.toLocaleDateString(lang, { month: "long", year: "numeric" });
  }
  // week: zh-CN "2026年8月17日 – 8月23日"(跨年/跨月补全),en-US "Aug 17 – Aug 23, 2026"
  if (lang === "zh-CN") {
    const sameYear = from.getFullYear() === to.getFullYear();
    const fromPart = `${from.getFullYear()}年${from.getMonth() + 1}月${from.getDate()}日`;
    const toPart = `${sameYear ? "" : `${to.getFullYear()}年`}${to.getMonth() + 1}月${to.getDate()}日`;
    return `${fromPart} – ${toPart}`;
  }
  const opt: Intl.DateTimeFormatOptions = { month: "short", day: "numeric" };
  return `${from.toLocaleDateString(lang, opt)} – ${to.toLocaleDateString(lang, opt)}, ${to.getFullYear()}`;
});

/** 周末/节假日徽章仅日视角有意义(周/月范围混合多种日期类型);节假日优先展示真实节日名 */
const dateBadge = computed(() => {
  if (viewMode.value !== "day" || !selectedDate.value) return null;
  const ds = selectedDate.value;
  const data = calendarData.value;
  const lang = settings.language;
  if (data?.holidays.includes(ds)) {
    const name = localizedHolidayName(data.holidayNames[ds], lang);
    return { label: name ?? t("reportHistory.holiday"), variant: "secondary" as const };
  }
  if (data?.workdays.includes(ds)) {
    const base = t("reportHistory.makeupWorkday");
    const name = localizedHolidayName(data.workdayNames[ds], lang);
    return { label: name ? `${base} · ${name}` : base, variant: "secondary" as const };
  }
  const d = parseDateStr(ds);
  if (d.getDay() === 0 || d.getDay() === 6)
    return { label: t("reportHistory.weekend"), variant: "outline" as const };
  return null;
});

// ── Markdown config ─────────────────────────────────────────────────────

const controls: ControlsConfig = {
  table: {
    copy: true,
    download: true,
    fullscreen: true,
    customize: createTableCustomize(t),
  },
  code: { copy: true, collapse: true },
};
const detachedThemeEl = document.createElement("div");
const themeElement = () => detachedThemeEl;

// 与 DailyReportDialog 一致:覆盖库默认下载,走 Tauri save dialog。
const beforeDownload = createBeforeDownload(t);

// ── data loading ────────────────────────────────────────────────────────

/** 单调递增请求令牌:用于丢弃已过期/被覆盖的请求结果,防止旧响应覆盖新状态 */
let calendarRequestToken = 0;
let reportsRequestToken = 0;

async function loadCalendarMeta(year: number, month: number) {
  const token = ++calendarRequestToken;
  calendarLoading.value = true;
  try {
    const result = await cmd<CalendarMeta>("get_calendar_meta", {
      year,
      month,
      projectIds: filterProjectIds.value,
      tagIds: filterTagIds.value,
      reportType: reportTypeParam.value,
    });
    // 期间已有更新的请求发起,丢弃本次响应
    if (token !== calendarRequestToken) return;
    calendarData.value = result;
  } catch (e) {
    if (token !== calendarRequestToken) return;
    toast.error(t("reportHistory.loadCalendarFailed"));
  } finally {
    if (token === calendarRequestToken) {
      calendarLoading.value = false;
    }
  }
}

async function loadReports(from: string, to: string) {
  const token = ++reportsRequestToken;
  reportsLoading.value = true;
  expandedReportId.value = null;
  try {
    const result = await cmd<ReportHistoryDetail[]>("get_reports_by_range", {
      dateFrom: from,
      dateTo: to,
      projectIds: filterProjectIds.value,
      tagIds: filterTagIds.value,
      reportType: reportTypeParam.value,
    });
    if (token !== reportsRequestToken) return;
    reports.value = result;
  } catch (e) {
    if (token !== reportsRequestToken) return;
    toast.error(t("reportHistory.loadFailed"));
    reports.value = [];
  } finally {
    if (token === reportsRequestToken) {
      reportsLoading.value = false;
      // 手风琴模式：默认展开第一条
      if (reports.value.length > 0) {
        expandedReportId.value = reports.value[0].id;
      }
    }
  }
}

/** 待确认删除的报告,ConfirmDialog 确认后执行 */
const pendingDelete = ref<number | null>(null);
const deleteConfirmOpen = computed({
  get: () => pendingDelete.value !== null,
  set: (v) => {
    if (!v) pendingDelete.value = null;
  },
});

function deleteReport(id: number) {
  pendingDelete.value = id;
}

async function confirmDeleteReport() {
  const id = pendingDelete.value;
  if (id == null) return;
  try {
    await cmd("delete_report_history", { id });
    reports.value = reports.value.filter((r) => r.id !== id);
    toast.success(t("reportHistory.deleted"));
    // 刷新日历标注
    loadCalendarMeta(calendarYear.value, calendarMonth.value);
  } catch (e) {
    toast.error(e instanceof Error ? e.message : String(e));
  }
}

const exporting = ref(false);

const exportLabels = computed<ReportExportLabels>(() => ({
  collection: t("reportHistory.exportCollection"),
  daily: t("reportHistory.typeDaily"),
  weekly: t("reportHistory.typeWeekly"),
  dateRange: t("reportHistory.exportDateRange"),
  projects: t("reportHistory.exportProjects"),
  generatedAt: t("reportHistory.generatedAt"),
  commits: t("reportHistory.exportCommitCount"),
  commitDetails: t("reportHistory.commits"),
}));

/** 导出单条报告或当前筛选范围内的全部报告为一个 Markdown 文件。 */
async function exportReports(items: ReportHistoryDetail[]) {
  if (!items.length || exporting.value) {
    return;
  }
  exporting.value = true;
  try {
    const labels = exportLabels.value;
    const path = await save({
      title: t("reportHistory.exportDialogTitle"),
      defaultPath: createReportExportFilename(items, labels),
      filters: [{ name: "Markdown", extensions: ["md"] }],
    });
    if (!path) {
      return;
    }
    const content = buildReportExportMarkdown(items, {
      labels,
      formatCreatedAt: (timestamp) => formatLocalDateTime(timestamp),
    });
    await cmd<void>("save_text_file", { path, content });
    toast.success(t("reportHistory.exported", { path }));
  } catch (e) {
    toast.error(t("reportHistory.exportFailed", { error: String(e) }));
  } finally {
    exporting.value = false;
  }
}

// ── watchers ────────────────────────────────────────────────────────────

function onMonthChange(year: number, month: number) {
  calendarYear.value = year;
  calendarMonth.value = month;
}

watch(
  () =>
    [
      calendarYear.value,
      calendarMonth.value,
      filterProjectIds.value,
      filterTagIds.value,
      filterType.value,
    ] as const,
  ([y, m]) => loadCalendarMeta(y, m),
  { immediate: true },
);

/** 选中范围或任一筛选条件变化时刷新报告列表(日/周/月视角切换即范围变化) */
watch(
  () =>
    [
      selectedRange.value?.from ?? null,
      selectedRange.value?.to ?? null,
      filterProjectIds.value,
      filterTagIds.value,
      filterType.value,
    ] as const,
  ([from, to]) => {
    if (from && to) {
      loadReports(from, to);
    } else {
      reports.value = [];
    }
  },
  { immediate: true },
);
</script>

<template>
  <div class="flex h-full flex-col">
    <!-- header -->
    <header class="flex shrink-0 items-center gap-2 border-b px-4 py-2.5">
      <h1 class="text-sm font-semibold">{{ t("reportHistory.title") }}</h1>
      <Button
        variant="outline"
        size="sm"
        class="ml-auto h-8 gap-1.5"
        :title="t('report.title')"
        @click="reportOpen = true"
      >
        <FileText class="h-3.5 w-3.5" />
        {{ t("report.title") }}
      </Button>
    </header>

    <!-- body -->
    <div class="flex min-h-0 flex-1">
      <!-- ── left panel ────────────────────────────────────────────────── -->
      <div class="flex w-72 shrink-0 flex-col border-r">
        <!-- filters -->
        <div class="shrink-0 space-y-2 border-b px-3 py-2.5">
          <div>
            <label class="mb-1 block text-[11px] text-muted-foreground">
              {{ t("reportHistory.typeLabel") }}
            </label>
            <div class="flex items-center gap-1">
              <Button
                v-for="opt in TYPE_OPTIONS"
                :key="opt.value"
                size="sm"
                :variant="filterType === opt.value ? 'default' : 'outline'"
                class="h-7 flex-1 px-2 text-xs"
                @click="filterType = opt.value"
              >
                <span v-if="opt.dotClass" class="h-1.5 w-1.5 rounded-full" :class="opt.dotClass" />
                {{ t(opt.labelKey) }}
              </Button>
            </div>
          </div>
          <ProjectFilterSelect
            :projects="activeProjects"
            :checked-ids="filterProjectIds"
            @toggle="toggleProjectFilter"
          />
          <div>
            <label class="mb-1 block text-[11px] text-muted-foreground">
              {{ t("reportHistory.filterTag") }}
            </label>
            <DropdownMenu>
              <DropdownMenuTrigger as-child>
                <Button
                  variant="outline"
                  size="sm"
                  class="h-7 w-full justify-start gap-1.5 px-2 text-xs font-normal"
                >
                  <Tags class="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                  <span class="truncate">{{ t("reportHistory.filterTag") }}</span>
                  <span
                    v-if="filterTagIds.length"
                    class="ml-auto rounded-full bg-primary px-1.5 text-[11px] leading-4 text-primary-foreground"
                    >{{ filterTagIds.length }}</span
                  >
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="start" class="w-48">
                <TagCheckList
                  :tags="tagsStore.tags"
                  :checked-ids="filterTagIds"
                  @toggle="toggleTagFilter"
                />
              </DropdownMenuContent>
            </DropdownMenu>
            <div v-if="selectedTags.length" class="mt-1.5 flex flex-wrap gap-1">
              <span
                v-for="tag in selectedTags"
                :key="tag.id"
                class="inline-flex items-center gap-1 rounded-full border px-1.5 py-px text-[11px] cursor-pointer hover:bg-accent"
                :title="t('tags.picker.remove')"
                @click="toggleTagFilter(tag.id)"
              >
                <span class="h-2 w-2 rounded-full" :style="{ backgroundColor: tag.color }" />
                {{ tag.name }}
                <span class="ml-0.5 text-muted-foreground">&times;</span>
              </span>
            </div>
          </div>
        </div>

        <!-- calendar -->
        <div class="relative min-h-0 flex-1">
          <!-- 仅首次加载(无数据)用全遮罩;后续刷新保留旧数据,避免切月闪烁 -->
          <div
            v-if="calendarLoading && !calendarData"
            class="absolute inset-0 z-10 flex items-center justify-center bg-background/60"
          >
            <Loader2 class="h-5 w-5 animate-spin text-muted-foreground" />
          </div>
          <Loader2
            v-else-if="calendarRefreshing"
            class="absolute right-10 top-2.5 z-10 h-3.5 w-3.5 animate-spin text-muted-foreground"
          />
          <div class="flex h-full flex-col">
            <!-- 选中视角:日 / 周 / 月 -->
            <div class="flex shrink-0 items-center gap-1 px-3 pt-2">
              <Button
                v-for="opt in VIEW_OPTIONS"
                :key="opt.value"
                size="sm"
                :variant="viewMode === opt.value ? 'default' : 'outline'"
                class="h-6 flex-1 px-2 text-[11px]"
                @click="viewMode = opt.value"
              >
                {{ t(opt.labelKey) }}
              </Button>
            </div>
            <ScrollArea class="min-h-0 flex-1">
              <ReportCalendar
                v-model="selectedDate"
                :calendar-data="calendarData"
                :highlight-range="highlightRange"
                :selection-mode="viewMode"
                @month-change="onMonthChange"
              />
            </ScrollArea>
          </div>
        </div>
      </div>

      <!-- ── right panel ───────────────────────────────────────────────── -->
      <div class="flex min-h-0 min-w-0 flex-1 flex-col">
        <!-- empty: no date selected -->
        <template v-if="!selectedDate">
          <div class="flex flex-1 flex-col items-center justify-center gap-3 text-muted-foreground">
            <CalendarIcon class="h-10 w-10 opacity-30" />
            <p class="text-sm">{{ t("reportHistory.selectDateHint") }}</p>
          </div>
        </template>

        <!-- loading -->
        <template v-else-if="reportsLoading">
          <div class="flex flex-1 items-center justify-center">
            <Loader2 class="h-5 w-5 animate-spin text-muted-foreground" />
          </div>
        </template>

        <!-- content -->
        <template v-else>
          <!-- date header -->
          <div class="flex shrink-0 items-center gap-2 border-b px-4 py-2">
            <span class="text-sm font-medium">{{ rangeLabel }}</span>
            <Badge v-if="dateBadge" :variant="dateBadge.variant" class="text-[11px]">
              {{ dateBadge.label }}
            </Badge>
            <Badge
              v-if="viewMode === 'day' && selectedDate === formatDate(new Date())"
              variant="secondary"
              class="text-[11px]"
            >
              {{ t("reportHistory.today") }}
            </Badge>
          </div>

          <!-- no reports -->
          <div
            v-if="!reports.length"
            class="flex flex-1 items-center justify-center text-sm text-muted-foreground"
          >
            {{
              viewMode === "day"
                ? t("reportHistory.noReportsOnDate")
                : t("reportHistory.noReportsInRange")
            }}
          </div>

          <!-- reports + commits -->
          <ScrollArea v-else class="min-h-0 flex-1">
            <div class="flex flex-col gap-3 p-4">
              <!-- toolbar -->
              <div class="flex items-center justify-between gap-2">
                <h3 class="text-xs font-medium text-muted-foreground">
                  {{ t("reportHistory.reportCount", { count: reports.length }) }}
                </h3>
                <Button
                  variant="outline"
                  size="sm"
                  class="h-7 gap-1.5 px-2.5 text-xs"
                  :disabled="exporting"
                  @click="exportReports(reports)"
                >
                  <Loader2 v-if="exporting" class="h-3.5 w-3.5 animate-spin" />
                  <Download v-else class="h-3.5 w-3.5" />
                  {{ t("reportHistory.exportAll") }}
                </Button>
              </div>

              <!-- report cards -->
              <ReportCard
                v-for="r in reports"
                :key="r.id"
                :report="r"
                :open="expandedReportId === r.id"
                :exporting="exporting"
                :controls="controls"
                :theme-element="themeElement"
                :locale="settings.language"
                :before-download="beforeDownload"
                @update:open="expandedReportId = $event ? r.id : null"
                @export="exportReports([$event])"
                @delete="deleteReport"
              />
            </div>
          </ScrollArea>
        </template>
      </div>
    </div>

    <DailyReportDialog v-model:open="reportOpen" />
    <ConfirmDialog
      v-model:open="deleteConfirmOpen"
      :title="t('common.delete')"
      :description="t('reportHistory.deleteConfirm')"
      :confirm-text="t('common.delete')"
      destructive
      @confirm="confirmDeleteReport"
    />
  </div>
</template>
