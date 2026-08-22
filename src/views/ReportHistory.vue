<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import { toast } from "vue-sonner";
import {
  ArrowLeft,
  CalendarIcon,
  ChevronRight,
  FileText,
  FolderGit2,
  Loader2,
  Search,
  Tags,
  Trash2,
} from "@lucide/vue";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { Markdown, type ControlsConfig } from "vue-stream-markdown";
import { Badge } from "@/components/ui/badge";
import ConfirmDialog from "@/components/common/ConfirmDialog.vue";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { ScrollArea } from "@/components/ui/scroll-area";
import DailyReportDialog from "@/components/report/DailyReportDialog.vue";
import ReportCalendar from "@/components/report/ReportCalendar.vue";
import TagCheckList from "@/components/tags/TagCheckList.vue";
import { cmd, onListen } from "@/lib/tauri";
import { formatCommitTime, formatDate, formatLocalDateTime, parseDateStr } from "@/lib/format";
import { createBeforeDownload, createTableCustomize } from "@/lib/markdown-download";
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

const TYPE_OPTIONS: { value: TypeFilter; labelKey: string }[] = [
  { value: "all", labelKey: "reportHistory.typeAll" },
  { value: "daily", labelKey: "reportHistory.typeDaily" },
  { value: "weekly", labelKey: "reportHistory.typeWeekly" },
];

/** 日历选中视角:日(单日) | 周(周一至周日) | 月(整月),决定右侧列表的日期范围 */
const VIEW_OPTIONS: { value: ReportViewMode; labelKey: string }[] = [
  { value: "day", labelKey: "reportHistory.viewDay" },
  { value: "week", labelKey: "reportHistory.viewWeek" },
  { value: "month", labelKey: "reportHistory.viewMonth" },
];

const { t } = useI18n();
const router = useRouter();
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
const projectKeyword = ref("");

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

const filteredProjects = computed(() => {
  const kw = projectKeyword.value.trim().toLowerCase();
  if (!kw) return activeProjects.value;
  return activeProjects.value.filter(
    (p) => p.name.toLowerCase().includes(kw) || p.path.toLowerCase().includes(kw),
  );
});

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

/** 已选项目对象列表 */
const selectedProjects = computed(() =>
  activeProjects.value.filter((p) => filterProjectIds.value.includes(p.id)),
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
const commitOpen = ref<Record<string, boolean>>({});

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

/** 周末/节假日徽章仅日视角有意义(周/月范围混合多种日期类型) */
const dateBadge = computed(() => {
  if (viewMode.value !== "day" || !selectedDate.value) return null;
  const ds = selectedDate.value;
  if (calendarData.value?.holidays.includes(ds))
    return { label: t("reportHistory.holiday"), variant: "secondary" as const };
  if (calendarData.value?.workdays.includes(ds))
    return { label: t("reportHistory.makeupWorkday"), variant: "secondary" as const };
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
  commitOpen.value = {};
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
      <Button
        variant="ghost"
        size="icon"
        class="h-8 w-8"
        :title="t('reportHistory.back')"
        @click="router.push('/')"
      >
        <ArrowLeft class="h-4 w-4" />
      </Button>
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
                {{ t(opt.labelKey) }}
              </Button>
            </div>
          </div>
          <div>
            <label class="mb-1 block text-[11px] text-muted-foreground">
              {{ t("reportHistory.searchProject") }}
            </label>
            <DropdownMenu>
              <DropdownMenuTrigger as-child>
                <Button
                  variant="outline"
                  size="sm"
                  class="h-7 w-full justify-start gap-1.5 px-2 text-xs font-normal"
                >
                  <FolderGit2 class="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                  <span class="truncate">{{ t("reportHistory.searchProject") }}</span>
                  <span
                    v-if="filterProjectIds.length"
                    class="ml-auto rounded-full bg-primary px-1.5 text-[11px] leading-4 text-primary-foreground"
                    >{{ filterProjectIds.length }}</span
                  >
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="start" class="w-52">
                <div class="px-1 pb-1">
                  <div class="relative">
                    <Search
                      class="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground"
                    />
                    <input
                      v-model="projectKeyword"
                      :placeholder="t('projects.home.searchPlaceholder')"
                      class="h-7 w-full rounded-md border border-input bg-transparent pl-7 pr-2 text-xs outline-none placeholder:text-muted-foreground focus-visible:ring-1 focus-visible:ring-ring"
                      @keydown="
                        (e: KeyboardEvent) => {
                          if (e.key !== 'Escape') e.stopPropagation();
                        }
                      "
                    />
                  </div>
                </div>
                <div class="max-h-56 overflow-y-auto">
                  <div
                    v-for="p in filteredProjects"
                    :key="p.id"
                    class="flex cursor-pointer items-center gap-2 rounded-sm px-2 py-1.5 text-xs hover:bg-accent"
                    @click="toggleProjectFilter(p.id)"
                    @keydown.enter="toggleProjectFilter(p.id)"
                  >
                    <input
                      type="checkbox"
                      class="h-3.5 w-3.5 shrink-0 accent-primary"
                      :checked="filterProjectIds.includes(p.id)"
                      @click.stop
                      @change="toggleProjectFilter(p.id)"
                    />
                    <span class="truncate">{{ p.name }}</span>
                  </div>
                  <p
                    v-if="!activeProjects.length"
                    class="px-2 py-1.5 text-xs text-muted-foreground"
                  >
                    {{ t("projects.home.emptyAll") }}
                  </p>
                  <p
                    v-else-if="!filteredProjects.length"
                    class="px-2 py-1.5 text-xs text-muted-foreground"
                  >
                    {{ t("projects.home.emptyFiltered") }}
                  </p>
                </div>
              </DropdownMenuContent>
            </DropdownMenu>
            <div v-if="selectedProjects.length" class="mt-1.5 flex flex-wrap gap-1">
              <span
                v-for="p in selectedProjects"
                :key="p.id"
                class="inline-flex items-center gap-1 rounded-full border px-1.5 py-px text-[11px] cursor-pointer hover:bg-accent"
                @click="toggleProjectFilter(p.id)"
              >
                {{ p.name }}
                <span class="ml-0.5 text-muted-foreground">&times;</span>
              </span>
            </div>
          </div>
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
              <div class="flex items-center justify-between">
                <h3 class="text-xs font-medium text-muted-foreground">
                  {{ t("reportHistory.reportCount", { count: reports.length }) }}
                </h3>
              </div>

              <!-- report cards -->
              <Collapsible
                v-for="r in reports"
                :key="r.id"
                :open="expandedReportId === r.id"
                @update:open="expandedReportId = $event ? r.id : null"
                class="rounded-lg border"
              >
                <CollapsibleTrigger
                  class="group flex w-full cursor-pointer items-center gap-2 px-3 py-2.5 text-left hover:bg-accent/50 rounded-t-lg"
                  :class="expandedReportId !== r.id && 'rounded-b-lg'"
                >
                  <ChevronRight
                    class="h-3.5 w-3.5 shrink-0 text-muted-foreground transition-transform"
                    :class="{ 'rotate-90': expandedReportId === r.id }"
                  />
                  <span class="min-w-0 flex-1 truncate text-xs">
                    <span class="font-medium"
                      >{{ r.projectNames.slice(0, 3).join(", ")
                      }}{{
                        r.projectNames.length > 3 ? ` +${r.projectNames.length - 3}` : ""
                      }}</span
                    >
                    <span
                      class="ml-2 text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100"
                    >
                      {{ formatLocalDateTime(r.createdAt) }}
                    </span>
                  </span>
                  <Badge
                    variant="outline"
                    class="text-[11px] shrink-0"
                    :class="
                      r.periodType === 'weekly'
                        ? 'border-violet-500/40 bg-violet-500/10 text-violet-600 dark:text-violet-400'
                        : ''
                    "
                  >
                    {{
                      t(
                        r.periodType === "weekly"
                          ? "reportHistory.typeWeekly"
                          : "reportHistory.typeDaily",
                      )
                    }}
                  </Badge>
                  <Badge variant="secondary" class="text-[11px] shrink-0">
                    {{ t("reportHistory.totalCommits", { count: r.totalCommits }) }}
                  </Badge>
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-5 w-5 shrink-0 text-muted-foreground hover:text-destructive"
                    :title="t('common.delete')"
                    @click.stop="deleteReport(r.id)"
                  >
                    <Trash2 class="h-3 w-3" />
                  </Button>
                </CollapsibleTrigger>
                <CollapsibleContent>
                  <div class="border-t">
                    <!-- Markdown -->
                    <div class="px-4 py-3 text-sm">
                      <Markdown
                        mode="static"
                        :content="r.result"
                        :controls="controls"
                        :theme-element="themeElement"
                        :locale="settings.language"
                        :before-download="beforeDownload"
                      />
                    </div>
                    <!-- Commits within this report -->
                    <div v-if="r.commits.length" class="border-t">
                      <Collapsible
                        v-for="c in r.commits"
                        :key="c.projectName"
                        v-slot="{ open: expanded }"
                        :open="commitOpen[`${r.id}-${c.projectName}`]"
                        @update:open="commitOpen[`${r.id}-${c.projectName}`] = $event"
                      >
                        <CollapsibleTrigger
                          class="flex w-full cursor-pointer items-center gap-1.5 px-4 py-1.5 text-left text-xs hover:bg-accent/50"
                        >
                          <ChevronRight
                            class="h-3 w-3 shrink-0 text-muted-foreground transition-transform"
                            :class="{ 'rotate-90': expanded }"
                          />
                          <span class="min-w-0 flex-1 truncate font-medium">{{
                            c.projectName
                          }}</span>
                          <span class="shrink-0 text-muted-foreground">{{ c.commits.length }}</span>
                        </CollapsibleTrigger>
                        <CollapsibleContent>
                          <div class="ml-5 max-h-40 overflow-y-auto border-l">
                            <div
                              v-for="commit in c.commits"
                              :key="commit.hash + commit.date"
                              class="flex min-w-0 items-center gap-1.5 border-b px-2 py-0.5 text-[11px]"
                            >
                              <code
                                class="shrink-0 rounded bg-muted px-1 py-px font-mono text-[10px]"
                              >
                                {{ commit.hash }}
                              </code>
                              <span class="min-w-0 flex-1 truncate" :title="commit.subject">
                                {{ commit.subject }}
                              </span>
                              <span class="shrink-0 text-muted-foreground">
                                {{ formatCommitTime(commit.date) }}
                              </span>
                            </div>
                          </div>
                        </CollapsibleContent>
                      </Collapsible>
                    </div>
                  </div>
                </CollapsibleContent>
              </Collapsible>
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
