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
import { formatLocalDateTime } from "@/lib/format";
import { cmd } from "@/lib/tauri";
import type { AiUsageEntry, AiUsageSummary } from "@/types";

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
  { label: t("settings.usage.calls"), value: fmtNum(summary.value?.totalCalls ?? null) },
  {
    label: t("settings.usage.inputTokens"),
    value: fmtNum(summary.value?.totalInputTokens ?? null),
  },
  {
    label: t("settings.usage.outputTokens"),
    value: fmtNum(summary.value?.totalOutputTokens ?? null),
  },
  { label: t("settings.usage.totalTokens"), value: fmtNum(summary.value?.totalTokens ?? null) },
  {
    label: t("settings.usage.cachedTokens"),
    value: fmtNum(summary.value?.totalCachedTokens ?? null),
  },
]);

/** 按日趋势正序排列,高度按当日 tokens 归一化 */
const dayBars = computed(() => {
  const days = [...(summary.value?.byDay ?? [])].reverse();
  const max = Math.max(...days.map((d) => d.totalTokens), 1);
  return days.map((d) => ({
    ...d,
    heightPct: Math.max((d.totalTokens / max) * 100, d.totalTokens > 0 ? 6 : 2),
  }));
});

const maxTaskTokens = computed(() =>
  Math.max(...(summary.value?.byTask ?? []).map((s) => s.totalTokens), 1),
);

function fmtNum(n: number | null | undefined): string {
  return n == null ? "—" : n.toLocaleString();
}

function taskLabel(taskType: string): string {
  const key = `settings.usage.tasks.${taskType}`;
  return t(key);
}

function dayTitle(day: string, calls: number, totalTokens: number): string {
  return t("settings.usage.dayTooltip", { date: day, count: calls, tokens: totalTokens });
}

/** 输入 → 输出;缓存命中大于 0 时以括号附在输入后(缓存是输入的子集) */
function ioText(entry: AiUsageEntry): string {
  const cached =
    entry.cachedTokens != null && entry.cachedTokens > 0 ? ` (${fmtNum(entry.cachedTokens)})` : "";
  return `${fmtNum(entry.inputTokens)}${cached} → ${fmtNum(entry.outputTokens)}`;
}

function ioTitle(entry: AiUsageEntry): string {
  return t("settings.usage.ioTooltip", {
    input: fmtNum(entry.inputTokens),
    cached: fmtNum(entry.cachedTokens),
    output: fmtNum(entry.outputTokens),
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
          <p class="mt-0.5 truncate text-base font-semibold tabular-nums" :title="cell.value">
            {{ cell.value }}
          </p>
        </div>
      </div>

      <!-- 任务类型分布 -->
      <div class="mt-1 border-t pt-4">
        <label class="text-sm font-medium">{{ t("settings.usage.distribution") }}</label>
        <div v-if="summary.byTask.length" class="mt-2 flex flex-col gap-1.5">
          <div
            v-for="stat in summary.byTask"
            :key="stat.taskType"
            class="flex items-center gap-2 text-sm"
            :title="`${stat.calls} · ${stat.inputTokens.toLocaleString()} → ${stat.outputTokens.toLocaleString()}`"
          >
            <Badge variant="secondary" class="w-20 shrink-0 justify-center">
              {{ taskLabel(stat.taskType) }}
            </Badge>
            <div class="h-1.5 flex-1 overflow-hidden rounded-full bg-accent">
              <div
                class="h-full rounded-full bg-primary/70"
                :style="{
                  width: `${Math.max((stat.totalTokens / maxTaskTokens) * 100, stat.totalTokens > 0 ? 4 : 0)}%`,
                }"
              />
            </div>
            <span class="shrink-0 text-xs tabular-nums text-muted-foreground">
              {{ stat.calls }} · {{ stat.totalTokens.toLocaleString() }}
            </span>
          </div>
        </div>
        <p v-else class="py-4 text-center text-xs text-muted-foreground">
          {{ t("settings.usage.empty") }}
        </p>
      </div>

      <!-- 近 30 天趋势 -->
      <div v-if="dayBars.length" class="mt-1 border-t pt-4">
        <label class="text-sm font-medium">{{ t("settings.usage.trend") }}</label>
        <div class="mt-2 flex h-24 items-end gap-[3px]">
          <div
            v-for="d in dayBars"
            :key="d.day"
            class="min-w-0 flex-1 rounded-t-sm bg-primary/70 transition-[height]"
            :style="{ height: `${d.heightPct}%` }"
            :title="dayTitle(d.day, d.calls, d.totalTokens)"
          />
        </div>
        <div class="mt-1 flex justify-between text-xs tabular-nums text-muted-foreground">
          <span>{{ dayBars[0]?.day }}</span>
          <span>{{ dayBars[dayBars.length - 1]?.day }}</span>
        </div>
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
            <span class="w-14 shrink-0 text-right font-medium tabular-nums">
              {{ fmtNum(entry.totalTokens) }}
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
