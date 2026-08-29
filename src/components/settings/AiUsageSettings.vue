<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { Loader2, RefreshCw, Trash2 } from "@lucide/vue";
import ConfirmDialog from "@/components/common/ConfirmDialog.vue";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { AI_TASK_TYPES } from "@/lib/ai-usage";
import { formatCompactNumber, formatLocalDateTime } from "@/lib/format";
import { cmd } from "@/lib/tauri";
import { buildUsageHeatmap, type UsageHeatCell } from "@/lib/usage-heatmap";
import type { AiUsageEntry, AiUsageSummary, AiUsageTaskStat } from "@/types";

const { t } = useI18n();

const PAGE_SIZE = 50;

// ── 汇总与明细加载 ─────────────────────────────────────────────────────────

const summary = ref<AiUsageSummary | null>(null);
const entries = ref<AiUsageEntry[]>([]);
const hasMore = ref(false);
const loading = ref(false);
const taskFilter = ref<string>("all");
// 单调递增请求令牌:筛选切换时丢弃过期响应(照抄 ReportHistory 的防竞态范式)
let loadToken = 0;

async function loadSummary() {
  summary.value = await cmd<AiUsageSummary>("get_ai_usage_summary");
}

async function fetchPage(offset: number) {
  return cmd<AiUsageEntry[]>("list_ai_usage_log", {
    offset,
    limit: PAGE_SIZE,
    ...(taskFilter.value === "all" ? {} : { taskType: taskFilter.value }),
  });
}

/** 首屏/筛选切换:汇总 + 明细第一页 */
async function reload() {
  const token = ++loadToken;
  loading.value = true;
  try {
    const [, page] = await Promise.all([loadSummary(), fetchPage(0)]);
    if (token !== loadToken) return;
    entries.value = page;
    hasMore.value = page.length >= PAGE_SIZE;
  } catch (e) {
    if (token === loadToken) toast.error(String(e));
  } finally {
    if (token === loadToken) loading.value = false;
  }
}

async function loadMore() {
  if (loading.value || !hasMore.value) return;
  const token = ++loadToken;
  loading.value = true;
  try {
    const page = await fetchPage(entries.value.length);
    if (token !== loadToken) return;
    entries.value.push(...page);
    hasMore.value = page.length >= PAGE_SIZE;
  } catch (e) {
    if (token === loadToken) toast.error(String(e));
  } finally {
    if (token === loadToken) loading.value = false;
  }
}

function onFilterChange(value: unknown) {
  taskFilter.value = String(value);
  void reload();
}

onMounted(() => void reload());

// ── 清空日志 ───────────────────────────────────────────────────────────────

const clearOpen = ref(false);

async function confirmClear() {
  try {
    const deleted = await cmd<number>("clear_ai_usage_log");
    toast.success(t("settings.usage.cleared", { count: deleted }));
    taskFilter.value = "all";
    await reload();
  } catch (e) {
    toast.error(String(e));
  }
}

// ── 展示辅助 ───────────────────────────────────────────────────────────────

const cells = computed(() => [
  exactCell(t("settings.usage.calls"), summary.value?.totalCalls ?? null),
  tokenCell(t("settings.usage.inputTokens"), summary.value?.totalInputTokens ?? null),
  tokenCell(t("settings.usage.outputTokens"), summary.value?.totalOutputTokens ?? null),
  tokenCell(t("settings.usage.cachedTokens"), summary.value?.totalCachedTokens ?? null),
  tokenCell(t("settings.usage.totalTokens"), summary.value?.totalTokens ?? null),
]);

/** 最近半年热力图(周列 × 周一~周日行;空窗口也照常渲染,保持布局稳定) */
const heatmap = computed(() => buildUsageHeatmap(summary.value?.byDay ?? []));

const monthLabelByCol = computed(() => {
  const map = new Map<number, number>();
  for (const { col, month } of heatmap.value.monthLabels) map.set(col, month);
  return map;
});

/** 强度档对应配色:0 档为空底,1-4 档按 primary 透明度递增(亮暗主题/皮肤自适应) */
const LEVEL_CLASSES = [
  "bg-muted",
  "bg-primary/25",
  "bg-primary/45",
  "bg-primary/70",
  "bg-primary",
] as const;

function levelClass(level: number): string {
  return LEVEL_CLASSES[level] ?? LEVEL_CLASSES[0];
}

/** 星期标签列只在一/三/五行显示文字,其余行占位对齐 */
function weekdayLabel(row: number): string {
  const keys = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"] as const;
  return row % 2 === 0 ? t(`settings.usage.weekdayShort.${keys[row]}`) : "";
}

function cellTitle(cell: UsageHeatCell): string {
  if (cell.calls === 0 && cell.totalTokens === 0) {
    return t("settings.usage.dayEmpty", { date: cell.day });
  }
  return t("settings.usage.dayTooltip", {
    date: cell.day,
    count: fmtExact(cell.calls),
    tokens: fmtExact(cell.totalTokens),
  });
}

function taskStatTitle(stat: AiUsageTaskStat): string {
  return t("settings.usage.taskTooltip", {
    calls: fmtExact(stat.calls),
    input: fmtExact(stat.inputTokens),
    output: fmtExact(stat.outputTokens),
    cached: fmtExact(stat.cachedTokens),
  });
}

/** 分布条占比基准:优先按合计 tokens 相对最大值,全为 0(均未上报)时退化为按次数 */
const taskBarBase = computed(() => {
  const stats = summary.value?.byTask ?? [];
  const maxTokens = Math.max(0, ...stats.map((s) => s.totalTokens));
  if (maxTokens > 0) return { key: "totalTokens" as const, max: maxTokens };
  return { key: "calls" as const, max: Math.max(1, ...stats.map((s) => s.calls)) };
});

function taskBarWidth(stat: AiUsageTaskStat): string {
  const value = taskBarBase.value.key === "totalTokens" ? stat.totalTokens : stat.calls;
  if (value <= 0) return "0%";
  // 最小 4% 保证小占比条目仍可见
  return `${Math.max(4, Math.round((value / taskBarBase.value.max) * 100))}%`;
}

/** 类型固定配色(chart 色板,亮暗主题/皮肤自适应) */
const TASK_BAR_CLASSES: Record<string, string> = {
  wiki: "bg-chart-2",
  commit: "bg-chart-3",
  report: "bg-chart-4",
};

function taskBarClass(taskType: string): string {
  return TASK_BAR_CLASSES[taskType] ?? "bg-primary/70";
}

function fmtExact(n: number | null | undefined): string {
  return n == null ? "—" : n.toLocaleString();
}

function fmtTokens(n: number | null | undefined): string {
  return n == null ? "—" : formatCompactNumber(n);
}

function exactCell(label: string, value: number | null) {
  const text = fmtExact(value);
  return { label, value: text, title: text };
}

function tokenCell(label: string, value: number | null) {
  return { label, value: fmtTokens(value), title: fmtExact(value) };
}

function taskLabel(taskType: string): string {
  const key = `settings.usage.tasks.${taskType}`;
  return t(key);
}

/** 输入 → 输出;缓存命中大于 0 时以括号附在输入后(缓存是输入的子集) */
function ioText(entry: AiUsageEntry): string {
  const cached =
    entry.cachedTokens != null && entry.cachedTokens > 0
      ? ` (${fmtTokens(entry.cachedTokens)})`
      : "";
  return `${fmtTokens(entry.inputTokens)}${cached} → ${fmtTokens(entry.outputTokens)}`;
}

function ioTitle(entry: AiUsageEntry): string {
  return t("settings.usage.ioTooltip", {
    input: fmtExact(entry.inputTokens),
    cached: fmtExact(entry.cachedTokens),
    output: fmtExact(entry.outputTokens),
  });
}
</script>

<template>
  <section>
    <div class="flex items-center justify-between">
      <h2 class="text-base font-semibold">{{ t("settings.usage.title") }}</h2>
      <Button
        variant="ghost"
        size="icon"
        class="h-7 w-7"
        :title="t('common.refresh')"
        @click="reload"
      >
        <Loader2 v-if="loading" class="h-3.5 w-3.5 animate-spin" />
        <RefreshCw v-else class="h-3.5 w-3.5" />
      </Button>
    </div>
    <p class="mt-1 text-sm text-muted-foreground">{{ t("settings.usage.description") }}</p>

    <template v-if="summary">
      <!-- 汇总五格 -->
      <div class="mt-4 grid grid-cols-5 gap-2">
        <div v-for="cell in cells" :key="cell.label" class="rounded-lg border px-2 py-2">
          <p class="truncate text-xs text-muted-foreground">{{ cell.label }}</p>
          <p class="mt-0.5 truncate text-base font-semibold tabular-nums" :title="cell.title">
            {{ cell.value }}
          </p>
        </div>
      </div>

      <!-- 最近半年热力图(GitHub 贡献图风格:周列 × 周一~周日行) -->
      <div class="mt-1 border-t pt-4">
        <label class="text-sm font-medium">{{ t("settings.usage.trend") }}</label>
        <div class="mt-2 flex gap-1.5">
          <!-- 星期标签列:与格子行对齐(月份标签行高约 10px + 间距 4px) -->
          <div class="mt-[14px] flex w-4 shrink-0 flex-col gap-[3px]">
            <span
              v-for="row in 7"
              :key="row"
              class="flex h-3 items-center text-[10px] leading-none text-muted-foreground"
            >
              {{ weekdayLabel(row - 1) }}
            </span>
          </div>
          <div class="min-w-0">
            <!-- 月份标签:标在包含当月 1 号的周列上方,文字溢出空槽位即可 -->
            <div class="flex gap-[3px]">
              <span
                v-for="col in heatmap.weeks.length"
                :key="col"
                class="w-3 shrink-0 overflow-visible text-[10px] leading-none whitespace-nowrap text-muted-foreground"
              >
                {{
                  monthLabelByCol.has(col - 1)
                    ? t(`settings.usage.monthShort.m${monthLabelByCol.get(col - 1)}`)
                    : ""
                }}
              </span>
            </div>
            <div class="mt-1 flex gap-[3px]">
              <div v-for="(week, col) in heatmap.weeks" :key="col" class="flex flex-col gap-[3px]">
                <div
                  v-for="cell in week"
                  :key="cell.day"
                  class="h-3 w-3 rounded-[2px]"
                  :class="
                    cell.future
                      ? 'invisible'
                      : `${levelClass(cell.level)} hover:ring-1 hover:ring-foreground/40`
                  "
                  :title="cell.future ? undefined : cellTitle(cell)"
                />
              </div>
            </div>
            <!-- 图例 -->
            <div
              class="mt-1.5 flex items-center justify-end gap-1 text-[10px] text-muted-foreground"
            >
              <span>{{ t("settings.usage.legendLess") }}</span>
              <span
                v-for="lvl in 5"
                :key="lvl"
                class="h-2.5 w-2.5 rounded-[2px]"
                :class="levelClass(lvl - 1)"
              />
              <span>{{ t("settings.usage.legendMore") }}</span>
            </div>
          </div>
        </div>
      </div>

      <!-- 任务类型分布(条形图:类型 · 占比条 · 次数 · 合计 tokens) -->
      <div class="mt-1 border-t pt-4">
        <label class="text-sm font-medium">{{ t("settings.usage.distribution") }}</label>
        <div v-if="summary.byTask.length" class="mt-2 flex flex-col gap-0.5">
          <div
            v-for="stat in summary.byTask"
            :key="stat.taskType"
            class="flex items-center gap-3 rounded-md px-2 py-1.5 text-xs hover:bg-accent"
            :title="taskStatTitle(stat)"
          >
            <Badge variant="secondary" class="w-20 shrink-0 justify-center">
              {{ taskLabel(stat.taskType) }}
            </Badge>
            <div class="flex h-4 min-w-0 flex-1 items-center">
              <div
                class="h-1.5 rounded-full transition-[width]"
                :class="taskBarClass(stat.taskType)"
                :style="{ width: taskBarWidth(stat) }"
              />
            </div>
            <span class="w-16 shrink-0 text-right tabular-nums text-muted-foreground">
              {{ t("settings.usage.callsCount", { count: fmtExact(stat.calls) }) }}
            </span>
            <span
              class="w-16 shrink-0 text-right font-medium tabular-nums"
              :title="fmtExact(stat.totalTokens)"
            >
              {{ stat.totalTokens > 0 ? fmtTokens(stat.totalTokens) : "—" }}
            </span>
          </div>
        </div>
        <p v-else class="py-4 text-center text-xs text-muted-foreground">
          {{ t("settings.usage.empty") }}
        </p>
      </div>
    </template>

    <!-- 明细日志 -->
    <div class="mt-1 border-t pt-4">
      <div class="flex items-center justify-between gap-2">
        <label class="text-sm font-medium">{{ t("settings.usage.details") }}</label>
        <Button
          variant="outline"
          size="sm"
          class="gap-1.5 text-destructive hover:text-destructive"
          :disabled="!entries.length"
          @click="clearOpen = true"
        >
          <Trash2 class="h-3.5 w-3.5" />
          {{ t("settings.usage.clear") }}
        </Button>
      </div>

      <Select :model-value="taskFilter" @update:model-value="onFilterChange">
        <SelectTrigger class="mt-2 h-8 w-44 text-xs">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectGroup>
            <SelectItem value="all">{{ t("settings.usage.filterAll") }}</SelectItem>
            <SelectItem v-for="type in AI_TASK_TYPES" :key="type" :value="type">
              {{ taskLabel(type) }}
            </SelectItem>
          </SelectGroup>
        </SelectContent>
      </Select>

      <ScrollArea class="mt-2 max-h-80">
        <div class="flex flex-col gap-0.5 pr-2">
          <div
            v-for="entry in entries"
            :key="entry.id"
            class="flex items-center justify-between gap-2 rounded-md px-2 py-1.5 text-xs hover:bg-accent"
          >
            <span class="w-28 shrink-0 tabular-nums text-muted-foreground">
              {{ formatLocalDateTime(entry.createdAt) }}
            </span>
            <Badge variant="secondary" class="w-20 shrink-0 justify-center">
              {{ taskLabel(entry.taskType) }}
            </Badge>
            <span
              class="min-w-0 flex-1 truncate text-muted-foreground"
              :title="entry.model || undefined"
            >
              {{ entry.model || "—" }}
            </span>
            <span class="shrink-0 tabular-nums text-muted-foreground" :title="ioTitle(entry)">
              {{ ioText(entry) }}
            </span>
            <span
              class="w-14 shrink-0 text-right font-medium tabular-nums"
              :title="fmtExact(entry.totalTokens)"
            >
              {{ fmtTokens(entry.totalTokens) }}
            </span>
          </div>
          <p v-if="!entries.length && !loading" class="py-6 text-center text-muted-foreground">
            {{
              taskFilter === "all" ? t("settings.usage.empty") : t("settings.usage.emptyFiltered")
            }}
          </p>
        </div>
      </ScrollArea>
      <p v-if="entries.length >= PAGE_SIZE" class="mt-1 text-center">
        <Button v-if="hasMore" variant="ghost" size="sm" :disabled="loading" @click="loadMore">
          {{ loading ? t("common.loading") : t("settings.usage.loadMore") }}
        </Button>
        <span v-else class="text-xs text-muted-foreground">
          {{ t("settings.usage.noMore") }}
        </span>
      </p>
    </div>

    <ConfirmDialog
      v-model:open="clearOpen"
      :title="t('settings.usage.clear')"
      :description="t('settings.usage.clearConfirm')"
      :confirm-text="t('settings.usage.clear')"
      destructive
      @confirm="confirmClear"
    />
  </section>
</template>
