<script setup lang="ts">
import { computed, ref, shallowRef, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { getLocalTimeZone, today as calendarToday } from "@internationalized/date";
import type { CalendarRootProps, RangeCalendarRootProps } from "reka-ui";
import {
  Calendar as CalendarIcon,
  Copy,
  ChevronRight,
  Loader2,
  Search,
  Sparkles,
  Tags,
  X,
} from "@lucide/vue";
import { Markdown, type ControlsConfig } from "vue-stream-markdown";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Switch } from "@/components/ui/switch";
import HolidayCalendar from "@/components/report/HolidayCalendar.vue";
import HolidayRangeCalendar from "@/components/report/HolidayRangeCalendar.vue";
import TagCheckList from "@/components/tags/TagCheckList.vue";
import { generateReport, type ProjectCommits } from "@/lib/ai";
import { planBatchItems, type BatchItem } from "@/lib/batch-report";
import { formatCommitTime, formatDate, parseDateStr } from "@/lib/format";
import { copyToClipboard } from "@/lib/utils";
import { cmd } from "@/lib/tauri";
import { createBeforeDownload, createTableCustomize } from "@/lib/markdown-download";
import { useBatchReportStore } from "@/stores/batch-report";
import { useProjectsStore } from "@/stores/projects";
import { useSettingsStore } from "@/stores/settings";
import { useTagsStore } from "@/stores/tags";
import type { GitCommitInfo, GitUser, ReportPeriodType, WorkWeekRanges } from "@/types";

type Mode = ReportPeriodType;
type DailyRangeKey = "today" | "yesterday" | "custom";
type WeeklyRangeKey = "thisWeek" | "lastWeek" | "custom";
type AuthorMode = "me" | "all";
type ExecMode = "single" | "batch";
type BatchFilter = "workdays" | "hasCommits";

const BATCH_FILTER_OPTIONS: { value: BatchFilter; labelKey: string }[] = [
  { value: "workdays", labelKey: "report.batchFilterWorkdays" },
  { value: "hasCommits", labelKey: "report.batchFilterHasCommits" },
];

const MODE_OPTIONS: { value: Mode; labelKey: string }[] = [
  { value: "daily", labelKey: "report.modeDaily" },
  { value: "weekly", labelKey: "report.modeWeekly" },
];

/** 日报:只能选择 1 天 */
const DAILY_RANGE_OPTIONS: { value: DailyRangeKey; labelKey: string }[] = [
  { value: "today", labelKey: "report.today" },
  { value: "yesterday", labelKey: "report.yesterday" },
  { value: "custom", labelKey: "report.custom" },
];

/** 周报:日期范围,默认本周一~当前日期 */
const WEEKLY_RANGE_OPTIONS: { value: WeeklyRangeKey; labelKey: string }[] = [
  { value: "thisWeek", labelKey: "report.thisWeek" },
  { value: "lastWeek", labelKey: "report.lastWeek" },
  { value: "custom", labelKey: "report.custom" },
];

const AUTHOR_OPTIONS: { value: AuthorMode; labelKey: string }[] = [
  { value: "me", labelKey: "report.authorMe" },
  { value: "all", labelKey: "report.authorAll" },
];

const { t } = useI18n();
const props = defineProps<{ presetProjectId?: number }>();
const open = defineModel<boolean>("open", { required: true });

const store = useProjectsStore();
const settings = useSettingsStore();
const tagsStore = useTagsStore();

const activeProjects = computed(() => store.projects.filter((p) => !p.archived_at));
/** 详情页传入 presetProjectId 时锁定单项目,隐藏项目选择 */
const locked = computed(() => props.presetProjectId != null);

/** 实际参与生成/加载提交记录/批量派发的项目 id 列表。
 *  锁定单项目时强制为 [presetProjectId];否则读取用户当前勾选。
 *  把"locked ? [preset] : selectedIds"统一抽到一个 computed,
 *  避免在 loadCommits/generate/startBatch/resolveSelfNames 等多处重复同一表达式,
 *  并确保新代码引用了正确的"实际生效"集合(而非简单 selectedIds.value) */
const effectiveProjectIds = computed<number[]>(() =>
  props.presetProjectId != null ? [props.presetProjectId] : selectedIds.value,
);

const selectedIds = ref<number[]>([]);
// 项目筛选:关键字(名称/路径)+ 标签(与首页一致,多标签为 AND 语义),仅作用于本弹窗
const keyword = ref("");
const filterTagIds = ref<number[]>([]);

const visibleProjects = computed(() => {
  const kw = keyword.value.trim().toLowerCase();
  return activeProjects.value.filter((p) => {
    if (kw && !p.name.toLowerCase().includes(kw) && !p.path.toLowerCase().includes(kw)) {
      return false;
    }
    return filterTagIds.value.every((id) => p.tags.some((t) => t.id === id));
  });
});

const selectedFilterTags = computed(() =>
  tagsStore.tags.filter((tag) => filterTagIds.value.includes(tag.id)),
);

function toggleTagFilter(id: number) {
  filterTagIds.value = filterTagIds.value.includes(id)
    ? filterTagIds.value.filter((x) => x !== id)
    : [...filterTagIds.value, id];
}
// reka-ui 的 d.ts 存在内联日期类型与 @internationalized/date 两套声明,直接索引组件 props
// 的 modelValue 类型,保证 v-model / max-value 与 RangeCalendar 期望的类型严格一致
type RangeModel = NonNullable<RangeCalendarRootProps["modelValue"]>;
type RangeDateValue = NonNullable<RangeModel["start"]>;
// Calendar 的 modelValue 含多选数组形式,Exclude 掉后才是单日 DateValue
type SingleDateValue = Exclude<NonNullable<CalendarRootProps["modelValue"]>, unknown[]>;

const mode = ref<Mode>("daily");
const dailyKey = ref<DailyRangeKey>("today");
const weeklyKey = ref<WeeklyRangeKey>("thisWeek");
/** 自定义范围(reka RangeCalendar 的 DateRange,起止可为空表示未选完) */
// 用 shallowRef:ref<T> 的 UnwrapRef 会把日期类实例展开成结构类型,破坏名义类型匹配
const customRange = shallowRef<RangeModel>({ start: undefined, end: undefined });
/** 日报自定义单日(reka Calendar 的 DateValue) */
const customDate = shallowRef<SingleDateValue | undefined>(undefined);
/** 日历可选上限:今天(提交记录不可能来自未来);运行时与 reka 内部日期实现相同,仅类型需断言 */
const maxDate = calendarToday(getLocalTimeZone()) as unknown as RangeDateValue;
const maxDateSingle = calendarToday(getLocalTimeZone()) as unknown as SingleDateValue;
const authorMode = ref<AuthorMode>("me");
const generating = ref(false);
const result = ref("");
const savedHistoryId = ref<number | null>(null);

// ── 批量生成(进度在右下角浮窗展示,见 stores/batch-report) ──────────────
const batchStore = useBatchReportStore();
const execMode = ref<ExecMode>("single");
const isBatch = computed(() => execMode.value === "batch");
/** Switch 的 v-model 代理(reka-ui Switch 为 modelValue 布尔协议) */
const batchSwitch = computed({
  get: () => isBatch.value,
  set: (v: boolean) => {
    execMode.value = v ? "batch" : "single";
  },
});
/** 批量模式的总跨度(默认最近 7 天) */
function defaultBatchRange(): RangeModel {
  const end = calendarToday(getLocalTimeZone());
  const start = end.subtract({ days: 6 });
  return {
    start: start as unknown as RangeDateValue,
    end: end as unknown as RangeDateValue,
  };
}
const batchRange = shallowRef<RangeModel>(defaultBatchRange());
const batchSkipExisting = ref(true);
const batchFilter = ref<BatchFilter>("workdays");
const batchPlanning = ref(false);

const batchRangeLabel = computed(() => {
  const { start, end } = batchRange.value;
  if (start && end) return `${formatDate(toLocalDate(start))} - ${formatDate(toLocalDate(end))}`;
  const single = start ?? end;
  return single ? formatDate(toLocalDate(single)) : t("report.pickRange");
});
/** 本次拉取到的提交记录(驱动提交条数与可展开列表;生成前展示,AI 失败也保留) */
const commitData = ref<ProjectCommits[]>([]);
/** 各项目提交列表展开状态,key 为项目名 */
const commitOpen = ref<Record<string, boolean>>({});

const totalCommits = computed(() => commitData.value.reduce((sum, d) => sum + d.commits.length, 0));

/** 所选项目解析出的 git 用户名(项目 id → 显示名),驱动"仅我自己"按钮展示实际名称 */
const selfNames = ref<Record<number, string>>({});
/** 勾选快速变化时防止旧请求覆盖新结果 */
let selfNamesToken = 0;

/** 去重后的 git 用户名,多个以顿号连接(不同仓库可能配置了不同身份) */
const selfLabel = computed(() => {
  const names = [...new Set(Object.values(selfNames.value).filter(Boolean))];
  return names.join("、");
});

async function resolveSelfNames(ids: number[]) {
  const token = ++selfNamesToken;
  const targets = activeProjects.value.filter((p) => ids.includes(p.id));
  const entries = await Promise.all(
    targets.map(async (p) => {
      const user = await cmd<GitUser>("git_current_user", { path: p.path }).catch(() => null);
      return [p.id, user?.name || user?.email || ""] as const;
    }),
  );
  if (token !== selfNamesToken) return;
  selfNames.value = Object.fromEntries(entries);
}

// 勾选变化时重新解析所选项目的 git 用户名
watch(
  () => [...effectiveProjectIds.value].sort((a, b) => a - b).join(","),
  () => void resolveSelfNames(effectiveProjectIds.value),
);

// 表格/代码复制导出控件,与文件预览的 Markdown 渲染保持一致
const controls: ControlsConfig = {
  table: {
    copy: true,
    download: true,
    fullscreen: true,
    customize: createTableCustomize(t),
  },
  code: { copy: true, collapse: true },
};

// 覆盖库默认的 <a download> 实现(WebView2 下静默失败 + 无反馈),
// 走 Tauri save dialog + save_text_file;见 src/lib/markdown-download.ts
const beforeDownload = createBeforeDownload(t);

// 阻止库内联宿主变量,MD 主题交给 CSS 层
const detachedThemeEl = document.createElement("div");
const themeElement = () => detachedThemeEl;

/** DateValue → 本地当天 00:00 的 Date */
function toLocalDate(d: RangeDateValue) {
  return d.toDate(getLocalTimeZone());
}

/** 周报自定义范围触发按钮的展示文案 */
const customRangeLabel = computed(() => {
  const { start, end } = customRange.value;
  if (start && end) return `${formatDate(toLocalDate(start))} - ${formatDate(toLocalDate(end))}`;
  const single = start ?? end;
  return single ? formatDate(toLocalDate(single)) : t("report.pickRange");
});

/** 日报自定义单日触发按钮的展示文案 */
const customDateLabel = computed(() =>
  customDate.value ? formatDate(toLocalDate(customDate.value)) : t("report.pickDate"),
);

/** 后端计算的工作周范围(连续工作周期,含法定节假日/调休识别;打开弹窗时拉取) */
const workWeekRanges = ref<WorkWeekRanges | null>(null);

/** 周报"本周/上周"的具体日期范围。优先用后端工作周算法;
 *  未返回/失败时回退本地算法(周一为一周起点,上周为周一至周日) */
const weekRanges = computed(() => {
  if (workWeekRanges.value) {
    return {
      thisWeek: {
        from: parseDateStr(workWeekRanges.value.thisWeek.from),
        to: parseDateStr(workWeekRanges.value.thisWeek.to),
      },
      lastWeek: {
        from: parseDateStr(workWeekRanges.value.lastWeek.from),
        to: parseDateStr(workWeekRanges.value.lastWeek.to),
      },
    };
  }
  const now = new Date();
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const dow = (today.getDay() + 6) % 7; // 周一=0 .. 周日=6
  const monday = new Date(today);
  monday.setDate(monday.getDate() - dow);
  const lastMonday = new Date(monday);
  lastMonday.setDate(lastMonday.getDate() - 7);
  const lastSunday = new Date(monday);
  lastSunday.setDate(lastSunday.getDate() - 1);
  return {
    thisWeek: { from: monday, to: today },
    lastWeek: { from: lastMonday, to: lastSunday },
  };
});

/** 当前选中的周范围(自定义时无提示) */
const selectedWeekRange = computed(() =>
  weeklyKey.value === "custom" ? null : weekRanges.value[weeklyKey.value],
);

function fmtWeekRange(r: { from: Date; to: Date }) {
  return `${formatDate(r.from)} ~ ${formatDate(r.to)}`;
}

/** 当前选择的日期范围(本地时区,起止均为当天 00:00)。
 *  日报恒为同一天;周报默认本周一~今天 */
const range = computed<{ from: Date; to: Date }>(() => {
  const now = new Date();
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const daysAgo = (n: number) => {
    const d = new Date(today);
    d.setDate(d.getDate() - n);
    return d;
  };
  if (mode.value === "daily") {
    switch (dailyKey.value) {
      case "yesterday":
        return { from: daysAgo(1), to: daysAgo(1) };
      case "custom": {
        const d = customDate.value ? toLocalDate(customDate.value) : today;
        return { from: d, to: d };
      }
      default:
        return { from: today, to: today };
    }
  }
  // 周报:以周一为一周起点(工作周)
  switch (weeklyKey.value) {
    case "lastWeek":
      return weekRanges.value.lastWeek;
    case "custom": {
      const { start, end } = customRange.value;
      return {
        from: start ? toLocalDate(start) : weekRanges.value.thisWeek.from,
        to: end ? toLocalDate(end) : today,
      };
    }
    default:
      return weekRanges.value.thisWeek;
  }
});

// 每次打开重置为初始状态
watch(open, (v) => {
  if (!v) return;
  result.value = "";
  savedHistoryId.value = null;
  mode.value = "daily";
  dailyKey.value = "today";
  weeklyKey.value = "thisWeek";
  customRange.value = { start: undefined, end: undefined };
  customDate.value = undefined;
  authorMode.value = "me";
  commitData.value = [];
  commitOpen.value = {};
  keyword.value = "";
  filterTagIds.value = [];
  execMode.value = "single";
  batchRange.value = defaultBatchRange();
  batchSkipExisting.value = true;
  batchFilter.value = "workdays";
  if (!tagsStore.tags.length) void tagsStore.fetchTags();
  // 拉取后端工作周范围(本周/上周的具体日期);失败保留 null 走本地回退
  void cmd<WorkWeekRanges>("get_work_week_ranges")
    .then((r) => (workWeekRanges.value = r))
    .catch(() => {});
  selectedIds.value = props.presetProjectId != null ? [props.presetProjectId] : [];
  // 锁定单项目时 selectedIds 可能与上次相同而不触发上面的 watch,这里显式解析一次
  selfNames.value = {};
  void resolveSelfNames(effectiveProjectIds.value);
  // 同理:各筛选值均未变时自动加载 watch 不触发,显式拉一次提交记录
  void loadCommits();
});

function toggleProject(id: number) {
  selectedIds.value = selectedIds.value.includes(id)
    ? selectedIds.value.filter((x) => x !== id)
    : [...selectedIds.value, id];
}

const loadingCommits = ref(false);
/** 筛选快速变化时防止旧请求覆盖新结果 */
let commitsToken = 0;

/** 按当前项目/时间范围/作者过滤拉取提交记录(打开弹窗与筛选变化时自动触发)。
 *  批量模式下不预拉提交(跨度大,逐时段在执行时拉取) */
async function loadCommits() {
  if (execMode.value === "batch") return;
  const ids = effectiveProjectIds.value;
  const projects = activeProjects.value.filter((p) => ids.includes(p.id));
  const token = ++commitsToken;
  if (!projects.length) {
    commitData.value = [];
    return;
  }
  loadingCommits.value = true;
  const stale = () => token !== commitsToken;
  try {
    const since = `${formatDate(range.value.from)} 00:00:00`;
    const until = `${formatDate(range.value.to)} 23:59:59`;
    const data = await Promise.all(
      projects.map(async (p) => {
        // "仅自己":取该仓库 git 用户身份作为 --author 过滤;未配置则不过滤
        let author: string | undefined;
        if (authorMode.value === "me") {
          const user = await cmd<GitUser>("git_current_user", { path: p.path }).catch(() => null);
          author = user?.name || user?.email || undefined;
        }
        return {
          projectName: p.name,
          projectDescription: p.description,
          commits: await cmd<GitCommitInfo[]>("git_log", {
            path: p.path,
            since,
            until,
            maxCount: 500,
            author,
          }),
        };
      }),
    );
    if (stale()) return;
    commitData.value = data;
    commitOpen.value = {};
  } catch (e) {
    if (stale()) return;
    commitData.value = [];
    const message = e instanceof Error ? e.message : String(e);
    toast.error(t("report.loadFailed", { error: message }));
  } finally {
    if (!stale()) loadingCommits.value = false;
  }
}

// 项目勾选 / 报告类型 / 时间范围 / 作者过滤 / 单次批量切换 变化时自动刷新提交列表
watch(
  () =>
    [
      [...effectiveProjectIds.value].sort((a, b) => a - b).join(","),
      mode.value,
      formatDate(range.value.from),
      formatDate(range.value.to),
      authorMode.value,
      execMode.value,
    ].join("|"),
  () => void loadCommits(),
);

async function generate() {
  if (generating.value || loadingCommits.value) return;
  const ids = effectiveProjectIds.value;
  if (!ids.length) {
    toast.error(t("report.noProjects"));
    return;
  }
  // 过滤掉时间范围内没有提交的项目:不进 prompt,也不写入历史
  const data = commitData.value.filter((d) => d.commits.length);
  if (!data.length) {
    result.value = "";
    toast.info(t("report.noCommits"));
    return;
  }
  generating.value = true;
  savedHistoryId.value = null;
  try {
    const dateFrom = formatDate(range.value.from);
    const dateTo = formatDate(range.value.to);
    const rangeLabel =
      dateFrom === dateTo ? dateFrom : t("report.rangeLabel", { from: dateFrom, to: dateTo });
    result.value = await generateReport(data, rangeLabel, settings.language, mode.value);

    // 生成成功后自动保存到报告历史(仅含有提交的项目)
    // 项目映射必须完整,否则中止保存并提示刷新项目列表。
    const missingNames: string[] = [];
    const commitDataForSave: {
      projectId: number;
      projectName: string;
      projectDescription: string;
      commits: GitCommitInfo[];
    }[] = [];
    for (const d of data) {
      const project = activeProjects.value.find((p) => p.name === d.projectName);
      if (!project) {
        missingNames.push(d.projectName);
        continue;
      }
      commitDataForSave.push({
        projectId: project.id,
        projectName: d.projectName,
        projectDescription: d.projectDescription,
        commits: d.commits,
      });
    }
    if (missingNames.length) {
      toast.warning(t("report.missingProjectMapping", { names: missingNames.join(", ") }), {
        duration: 8000,
      });
      return;
    }
    if (!commitDataForSave.length) {
      toast.error(t("report.noProjects"));
      return;
    }
    savedHistoryId.value = await cmd<number>("save_report_history", {
      projectIds: commitDataForSave.map((c) => c.projectId),
      dateFrom,
      dateTo,
      rangeLabel,
      authorMode: authorMode.value,
      language: settings.language,
      periodType: mode.value,
      result: result.value,
      commitData: commitDataForSave,
    });
  } catch (e) {
    toast.error(e instanceof Error ? e.message : String(e));
  } finally {
    generating.value = false;
  }
}

async function copyResult() {
  await copyToClipboard(result.value);
}

/** 批量生成:规划时段后交给全局 store 执行,进度在右下角浮窗展示;启动即关闭弹窗 */
async function startBatch() {
  if (batchStore.running || batchPlanning.value) return;
  const ids = effectiveProjectIds.value;
  if (!ids.length) {
    toast.error(t("report.noProjects"));
    return;
  }
  const { start, end } = batchRange.value;
  if (!start || !end) {
    toast.error(t("report.pickRange"));
    return;
  }
  if (!settings.aiBaseUrl.trim() || !settings.aiApiKey.trim() || !settings.aiModel.trim()) {
    toast.error(t("ai.notConfigured"));
    return;
  }
  const dateFrom = formatDate(toLocalDate(start));
  const dateTo = formatDate(toLocalDate(end));

  batchPlanning.value = true;
  let items: BatchItem[];
  try {
    items = await planBatchItems({
      periodType: mode.value,
      dateFrom,
      dateTo,
      workdaysOnly: mode.value === "daily" && batchFilter.value === "workdays",
      skipExisting: batchSkipExisting.value,
      makeLabel: (from, to) => (from === to ? from : t("report.rangeLabel", { from, to })),
    });
  } catch (e) {
    toast.error(e instanceof Error ? e.message : String(e));
    return;
  } finally {
    batchPlanning.value = false;
  }
  if (!items.length) {
    toast.info(t("report.batchEmpty"));
    return;
  }

  const projects = activeProjects.value.filter((p) => ids.includes(p.id));
  void batchStore.start(items, {
    periodType: mode.value,
    projects,
    authorMode: authorMode.value,
    language: settings.language,
    concurrency: settings.aiConcurrency,
  });
  open.value = false;
}
</script>

<template>
  <Dialog v-model:open="open">
    <!-- flex 覆盖基类 grid;内容叠加易超视口,max-h 限制弹窗总高。
         基类 sm:max-w-sm 在 ≥sm 断点的层叠顺序后于非变体类,会盖掉普通 max-w-*,
         故宽度必须用 sm: 变体覆盖;min(*, 100%-2rem) 防止窗口较窄时弹窗贴边。
         未生成报告时只有配置栏,用较窄宽度避免空旷 -->
    <DialogContent
      class="flex max-h-[calc(100dvh-3rem)] flex-col"
      :class="
        result || generating
          ? 'sm:max-w-[min(56rem,calc(100%-2rem))]'
          : 'sm:max-w-[min(34rem,calc(100%-2rem))]'
      "
    >
      <DialogHeader class="shrink-0">
        <!-- pr-8 避开 DialogContent 右上角绝对定位的关闭按钮 -->
        <div class="flex items-center justify-between pr-8">
          <DialogTitle>{{ t("report.title") }}</DialogTitle>
        </div>
        <DialogDescription>{{ t("report.description") }}</DialogDescription>
      </DialogHeader>

      <!-- 左右双栏:左侧筛选配置(固定宽,超出滚动),右侧 Markdown 结果自适应撑满。
           item 的 min-width:auto 会按内容 min-content 撑破弹窗(如长 URL/英文串),
           min-w-0 解除该自动最小宽度;min-h-0 同理,是限高下内部滚动生效的关键 -->
      <div class="flex min-h-0 min-w-0 flex-1 gap-4">
        <div
          class="flex min-h-0 flex-col gap-4 overflow-y-auto pr-1"
          :class="result || generating ? 'w-72 shrink-0' : 'flex-1'"
        >
          <div v-if="!locked" class="flex flex-col gap-1.5">
            <div class="flex items-center justify-between">
              <label class="text-sm font-medium">{{ t("report.selectProjects") }}</label>
              <div class="flex gap-1">
                <Button
                  variant="ghost"
                  size="sm"
                  class="h-6 px-2 text-xs"
                  @click="
                    selectedIds = [
                      ...new Set([...selectedIds, ...visibleProjects.map((p) => p.id)]),
                    ]
                  "
                >
                  {{ t("report.selectAll") }}
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  class="h-6 px-2 text-xs"
                  @click="selectedIds = []"
                >
                  {{ t("report.clear") }}
                </Button>
              </div>
            </div>
            <div class="flex items-center gap-1.5">
              <div class="relative flex-1">
                <Search
                  class="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground"
                />
                <Input
                  v-model="keyword"
                  :placeholder="t('report.projectSearchPlaceholder')"
                  class="h-7 pl-7 text-xs"
                />
              </div>
              <DropdownMenu>
                <DropdownMenuTrigger as-child>
                  <Button variant="outline" size="sm" class="h-7 gap-1.5 px-2 text-xs">
                    <Tags class="h-3.5 w-3.5" />
                    {{ t("projects.home.filterTags") }}
                    <span
                      v-if="filterTagIds.length"
                      class="rounded-full bg-primary px-1.5 text-[11px] leading-4 text-primary-foreground"
                    >
                      {{ filterTagIds.length }}
                    </span>
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end" class="w-52">
                  <TagCheckList
                    :tags="tagsStore.tags"
                    :checked-ids="filterTagIds"
                    @toggle="toggleTagFilter"
                  />
                  <template v-if="filterTagIds.length">
                    <DropdownMenuSeparator />
                    <DropdownMenuItem class="gap-2 text-xs" @click="filterTagIds = []">
                      <X class="h-3.5 w-3.5" />
                      {{ t("projects.home.clearFilter") }}
                    </DropdownMenuItem>
                  </template>
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
            <div v-if="selectedFilterTags.length" class="flex flex-wrap items-center gap-1.5">
              <button
                v-for="tag in selectedFilterTags"
                :key="tag.id"
                type="button"
                class="flex items-center gap-1 rounded-full border px-2 py-0.5 text-[11px] transition-opacity hover:opacity-80"
                :style="{ backgroundColor: tag.color, borderColor: tag.color, color: '#fff' }"
                :title="t('projects.home.removeFilterTag', { name: tag.name })"
                @click="toggleTagFilter(tag.id)"
              >
                {{ tag.name }}
                <X class="h-2.5 w-2.5" />
              </button>
            </div>
            <div class="grid max-h-36 grid-cols-1 gap-x-2 overflow-y-auto rounded-md border p-2">
              <label
                v-for="p in visibleProjects"
                :key="p.id"
                class="flex cursor-pointer items-center gap-2 rounded px-1.5 py-1 text-sm hover:bg-accent"
              >
                <input
                  type="checkbox"
                  class="h-3.5 w-3.5 shrink-0 accent-primary"
                  :checked="selectedIds.includes(p.id)"
                  @change="toggleProject(p.id)"
                />
                <span class="truncate" :title="p.path">{{ p.name }}</span>
              </label>
              <p v-if="!visibleProjects.length" class="px-1.5 py-2 text-xs text-muted-foreground">
                {{ t("report.noMatch") }}
              </p>
            </div>
          </div>

          <div class="flex flex-col gap-1.5">
            <label class="text-sm font-medium">{{ t("report.mode") }}</label>
            <div class="flex items-center gap-1.5">
              <Button
                v-for="opt in MODE_OPTIONS"
                :key="opt.value"
                size="sm"
                :variant="mode === opt.value ? 'default' : 'outline'"
                class="h-7 px-2.5 text-xs"
                :disabled="batchStore.running"
                @click="mode = opt.value"
              >
                {{ t(opt.labelKey) }}
              </Button>
            </div>
          </div>

          <div class="flex flex-col gap-1.5">
            <div class="flex items-center gap-2">
              <label class="text-sm font-medium">{{ t("report.range") }}</label>
              <!-- 单次/批量切换:开关置右为批量(总跨度逐天/逐周拆分生成) -->
              <div class="flex items-center gap-1.5">
                <span class="text-xs" :class="isBatch ? 'text-muted-foreground' : 'font-medium'">
                  {{ t("report.execSingle") }}
                </span>
                <Switch v-model="batchSwitch" :disabled="batchStore.running" />
                <span class="text-xs" :class="isBatch ? 'font-medium' : 'text-muted-foreground'">
                  {{ t("report.execBatch") }}
                </span>
              </div>
            </div>
            <!-- 批量:选择总跨度,逐天/逐周拆分生成 -->
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
                    :locale="settings.language"
                    :max-value="maxDate"
                  />
                </PopoverContent>
              </Popover>
            </div>
            <!-- 日报:只能选择 1 天 -->
            <div v-else-if="mode === 'daily'" class="flex flex-wrap items-center gap-1.5">
              <Button
                v-for="opt in DAILY_RANGE_OPTIONS"
                :key="opt.value"
                size="sm"
                :variant="dailyKey === opt.value ? 'default' : 'outline'"
                class="h-7 px-2.5 text-xs"
                @click="dailyKey = opt.value"
              >
                {{ t(opt.labelKey) }}
              </Button>
              <Popover v-if="dailyKey === 'custom'">
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
                  <HolidayCalendar
                    v-model="customDate"
                    :locale="settings.language"
                    :max-value="maxDateSingle"
                  />
                </PopoverContent>
              </Popover>
            </div>
            <!-- 周报:日期范围,默认本周一~当前日期 -->
            <div v-else class="flex flex-wrap items-center gap-1.5">
              <Button
                v-for="opt in WEEKLY_RANGE_OPTIONS"
                :key="opt.value"
                size="sm"
                :variant="weeklyKey === opt.value ? 'default' : 'outline'"
                class="h-7 px-2.5 text-xs"
                @click="weeklyKey = opt.value"
              >
                {{ t(opt.labelKey) }}
              </Button>
              <Popover v-if="weeklyKey === 'custom'">
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
                    :locale="settings.language"
                    :max-value="maxDate"
                  />
                </PopoverContent>
              </Popover>
            </div>
            <!-- 选中本周/上周后显示具体日期范围(工作周:连续工作周期) -->
            <p
              v-if="!isBatch && mode === 'weekly' && selectedWeekRange"
              class="text-xs text-muted-foreground"
            >
              {{ fmtWeekRange(selectedWeekRange) }}
            </p>
          </div>

          <!-- 批量选项:跳过已有报告 + 日期过滤(仅日报) -->
          <template v-if="isBatch">
            <label class="flex cursor-pointer items-center gap-2 text-sm">
              <input
                v-model="batchSkipExisting"
                type="checkbox"
                class="h-3.5 w-3.5 shrink-0 accent-primary"
                :disabled="batchStore.running"
              />
              {{ t("report.batchSkipExisting") }}
            </label>
            <div v-if="mode === 'daily'" class="flex flex-col gap-1.5">
              <label class="text-sm font-medium">{{ t("report.batchFilter") }}</label>
              <div class="flex flex-wrap items-center gap-1.5">
                <Button
                  v-for="opt in BATCH_FILTER_OPTIONS"
                  :key="opt.value"
                  size="sm"
                  :variant="batchFilter === opt.value ? 'default' : 'outline'"
                  class="h-7 px-2.5 text-xs"
                  :disabled="batchStore.running"
                  @click="batchFilter = opt.value"
                >
                  {{ t(opt.labelKey) }}
                </Button>
              </div>
            </div>
          </template>

          <div class="flex flex-col gap-1.5">
            <label class="text-sm font-medium">{{ t("report.author") }}</label>
            <div class="flex flex-wrap items-center gap-1.5">
              <Button
                v-for="opt in AUTHOR_OPTIONS"
                :key="opt.value"
                size="sm"
                :variant="authorMode === opt.value ? 'default' : 'outline'"
                class="h-7 max-w-64 px-2.5 text-xs"
                @click="authorMode = opt.value"
              >
                <span class="truncate">
                  {{
                    opt.value === "me" && selfLabel
                      ? t("report.authorMeNamed", { name: selfLabel })
                      : t(opt.labelKey)
                  }}
                </span>
              </Button>
            </div>
          </div>

          <div
            v-if="!isBatch && (commitData.length || loadingCommits)"
            class="flex min-w-0 flex-col gap-1.5"
          >
            <div class="flex items-center justify-between">
              <div class="flex items-center gap-1.5">
                <label class="text-sm font-medium">{{ t("report.commits") }}</label>
                <Loader2
                  v-if="loadingCommits"
                  class="h-3.5 w-3.5 animate-spin text-muted-foreground"
                />
              </div>
              <Badge v-if="commitData.length" variant="secondary" class="text-xs">
                {{ t("report.commitCount", { count: totalCommits }) }}
              </Badge>
            </div>
            <div v-if="commitData.length" class="overflow-hidden rounded-md border">
              <Collapsible
                v-for="d in commitData"
                :key="d.projectName"
                v-slot="{ open: expanded }"
                :open="commitOpen[d.projectName]"
                @update:open="commitOpen[d.projectName] = $event"
              >
                <CollapsibleTrigger
                  class="flex w-full cursor-pointer items-center gap-2 px-2.5 py-1.5 text-left text-sm hover:bg-accent"
                >
                  <ChevronRight
                    class="h-3.5 w-3.5 shrink-0 text-muted-foreground transition-transform"
                    :class="{ 'rotate-90': expanded }"
                  />
                  <span class="min-w-0 flex-1 truncate">{{ d.projectName }}</span>
                  <span class="shrink-0 text-xs whitespace-nowrap text-muted-foreground">
                    {{
                      d.commits.length
                        ? t("report.commitCount", { count: d.commits.length })
                        : t("report.excludedNoCommits")
                    }}
                  </span>
                </CollapsibleTrigger>
                <CollapsibleContent class="min-w-0 overflow-hidden">
                  <div
                    v-if="d.commits.length"
                    class="max-h-40 overflow-y-auto overflow-x-hidden border-t"
                  >
                    <div
                      v-for="c in d.commits"
                      :key="c.hash + c.date"
                      class="flex min-w-0 items-center gap-2 px-3 py-1 text-xs"
                    >
                      <code class="shrink-0 rounded bg-muted px-1 py-0.5 font-mono text-[11px]">
                        {{ c.hash }}
                      </code>
                      <span class="min-w-0 flex-1 truncate" :title="c.subject">{{
                        c.subject
                      }}</span>
                      <span
                        class="max-w-28 shrink-0 truncate whitespace-nowrap text-muted-foreground"
                        :title="c.author"
                      >
                        {{ c.author }}
                      </span>
                      <span
                        class="shrink-0 whitespace-nowrap text-muted-foreground"
                        :title="c.date"
                        >{{ formatCommitTime(c.date) }}</span
                      >
                    </div>
                  </div>
                  <p v-else class="border-t px-3 py-2 text-xs text-muted-foreground">
                    {{ t("report.projectNoCommits") }}
                  </p>
                </CollapsibleContent>
              </Collapsible>
            </div>
          </div>

          <!-- pb-0.5 预留按钮 active 态 translate-y-px 的下移空间,
               否则内容刚好撑满时按下按钮会瞬间撑出滚动条、布局位移导致 click 丢失 -->
          <div class="flex justify-end pb-0.5">
            <Button
              v-if="isBatch"
              size="sm"
              class="gap-1.5"
              :disabled="batchStore.running || batchPlanning"
              @click="startBatch"
            >
              <Loader2 v-if="batchPlanning" class="h-3.5 w-3.5 animate-spin" />
              <Sparkles v-else class="h-3.5 w-3.5" />
              {{ batchPlanning ? t("report.batchPlanning") : t("report.batchStart") }}
            </Button>
            <Button
              v-else
              size="sm"
              class="gap-1.5"
              :disabled="generating || loadingCommits"
              @click="generate"
            >
              <Loader2 v-if="generating" class="h-3.5 w-3.5 animate-spin" />
              <Sparkles v-else class="h-3.5 w-3.5" />
              {{ generating ? t("report.generating") : t("report.generate") }}
            </Button>
          </div>
        </div>

        <!-- 生成前不展示结果面板;生成中即显示以呈现进度反馈。
             批量进度不在此展示,由右下角浮窗(BatchProgressFloat)承载 -->
        <div
          v-if="result || generating"
          class="flex min-h-0 min-w-0 flex-1 flex-col rounded-md border"
        >
          <div class="flex shrink-0 items-center justify-between border-b px-3 py-1.5">
            <div class="flex items-center gap-1">
              <span class="text-xs text-muted-foreground">Markdown</span>
              <span
                v-if="savedHistoryId"
                class="rounded bg-green-100 px-1.5 py-px text-[10px] text-green-700 dark:bg-green-900/30 dark:text-green-400"
              >
                {{ t("report.saved") }}
              </span>
            </div>
            <Button
              v-if="result"
              variant="ghost"
              size="sm"
              class="h-6 gap-1 px-2 text-xs"
              @click="copyResult"
            >
              <Copy class="h-3 w-3" />
              {{ t("report.copy") }}
            </Button>
          </div>
          <ScrollArea class="min-h-0 flex-1">
            <p v-if="!result" class="p-4 text-sm text-muted-foreground">
              {{ t("report.generating") }}
            </p>
            <div v-else class="p-4 text-sm">
              <Markdown
                mode="static"
                :content="result"
                :controls="controls"
                :theme-element="themeElement"
                :locale="settings.language"
                :before-download="beforeDownload"
              />
            </div>
          </ScrollArea>
        </div>
      </div>
    </DialogContent>
  </Dialog>
</template>
