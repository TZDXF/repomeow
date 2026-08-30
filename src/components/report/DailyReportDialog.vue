<script setup lang="ts">
import { computed, ref, shallowRef, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { Loader2, Sparkles } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import ReportCommitList from "@/components/report/ReportCommitList.vue";
import ReportPeriodControls from "@/components/report/ReportPeriodControls.vue";
import ReportProjectSelector from "@/components/report/ReportProjectSelector.vue";
import ReportResultPanel from "@/components/report/ReportResultPanel.vue";
import { createDefaultReportPeriod, toLocalDate } from "@/components/report/report-period";
import { generateAndSaveReport, type ProjectCommits } from "@/lib/ai";
import { planBatchItems, type BatchItem } from "@/lib/batch-report";
import { formatDate, parseDateStr } from "@/lib/format";
import { copyToClipboard } from "@/lib/utils";
import { cmd } from "@/lib/tauri";
import { useAiConfigStore } from "@/stores/ai-config";
import { useBatchReportStore } from "@/stores/batch-report";
import { useProjectsStore } from "@/stores/projects";
import { useSettingsStore } from "@/stores/settings";
import { useTagsStore } from "@/stores/tags";
import type { GitCommitInfo, GitUser, WorkWeekRanges } from "@/types";

type AuthorMode = "me" | "all";

const AUTHOR_OPTIONS: { value: AuthorMode; labelKey: string }[] = [
  { value: "me", labelKey: "report.authorMe" },
  { value: "all", labelKey: "report.authorAll" },
];

const { t } = useI18n();
const props = defineProps<{ presetProjectId?: number }>();
const open = defineModel<boolean>("open", { required: true });

const store = useProjectsStore();
const settings = useSettingsStore();
const aiConfig = useAiConfigStore();
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
// shallowRef 避免把日期类实例展开为结构类型；子组件每次以完整对象更新选择。
const period = shallowRef(createDefaultReportPeriod());
const authorMode = ref<AuthorMode>("me");
const generating = ref(false);
const result = ref("");
const savedHistoryId = ref<number | null>(null);

// ── 批量生成(进度在右下角浮窗展示,见 stores/batch-report) ──────────────
const batchStore = useBatchReportStore();
const isBatch = computed(() => period.value.execMode === "batch");
const batchPlanning = ref(false);
/** 本次拉取到的提交记录(驱动提交条数与可展开列表;生成前展示,AI 失败也保留) */
const commitData = ref<ProjectCommits[]>([]);

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
  if (period.value.mode === "daily") {
    switch (period.value.dailyKey) {
      case "yesterday":
        return { from: daysAgo(1), to: daysAgo(1) };
      case "custom": {
        const d = period.value.customDate ? toLocalDate(period.value.customDate) : today;
        return { from: d, to: d };
      }
      default:
        return { from: today, to: today };
    }
  }
  // 周报:以周一为一周起点(工作周)
  switch (period.value.weeklyKey) {
    case "lastWeek":
      return weekRanges.value.lastWeek;
    case "custom": {
      const { start, end } = period.value.customRange;
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
  period.value = createDefaultReportPeriod();
  authorMode.value = "me";
  commitData.value = [];
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

const loadingCommits = ref(false);
/** 筛选快速变化时防止旧请求覆盖新结果 */
let commitsToken = 0;

/** 按当前项目/时间范围/作者过滤拉取提交记录(打开弹窗与筛选变化时自动触发)。
 *  批量模式下不预拉提交(跨度大,逐时段在执行时拉取) */
async function loadCommits() {
  if (isBatch.value) return;
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
      period.value.mode,
      formatDate(range.value.from),
      formatDate(range.value.to),
      authorMode.value,
      period.value.execMode,
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
  // 预览数据只用于提前反馈；后端会重新读取项目和 Git 提交并保存一致的快照。
  if (!commitData.value.some((project) => project.commits.length)) {
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
    const generated = await generateAndSaveReport({
      projectIds: ids,
      dateFrom,
      dateTo,
      rangeLabel,
      authorMode: authorMode.value,
      language: settings.language,
      periodType: period.value.mode,
    });
    if (!generated) {
      result.value = "";
      toast.info(t("report.noCommits"));
      return;
    }
    result.value = generated.result;
    commitData.value = generated.commitData;
    savedHistoryId.value = generated.historyId;
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
  const { start, end } = period.value.batchRange;
  if (!start || !end) {
    toast.error(t("report.pickRange"));
    return;
  }
  // 默认模型(厂商 baseUrl/apiKey 齐)就绪才允许批量生成
  await aiConfig.ensureLoaded();
  if (!aiConfig.defaultReady) {
    toast.error(t("ai.notConfigured"));
    return;
  }
  const dateFrom = formatDate(toLocalDate(start));
  const dateTo = formatDate(toLocalDate(end));

  batchPlanning.value = true;
  let items: BatchItem[];
  try {
    items = await planBatchItems({
      periodType: period.value.mode,
      dateFrom,
      dateTo,
      workdaysOnly: period.value.mode === "daily" && period.value.batchFilter === "workdays",
      skipExisting: period.value.batchSkipExisting,
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
    periodType: period.value.mode,
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
          <ReportProjectSelector
            v-if="!locked"
            v-model="selectedIds"
            :projects="activeProjects"
            :tags="tagsStore.tags"
          />

          <ReportPeriodControls
            v-model="period"
            :language="settings.language"
            :batch-running="batchStore.running"
            :week-ranges="weekRanges"
          />

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

          <ReportCommitList
            v-if="!isBatch && (commitData.length || loadingCommits)"
            :commit-data="commitData"
            :loading="loadingCommits"
          />

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
        <ReportResultPanel
          v-if="result || generating"
          :result="result"
          :generating="generating"
          :saved-history-id="savedHistoryId"
          :language="settings.language"
          @copy="copyResult"
        />
      </div>
    </DialogContent>
  </Dialog>
</template>
