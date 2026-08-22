<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import {
  CalendarClock,
  Clock,
  Loader2,
  Pencil,
  Play,
  Plus,
  Power,
  PowerOff,
  Search,
  Tags,
  Trash2,
  X,
} from "@lucide/vue";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
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
import { TimeField } from "@/components/ui/time-field";
import TagCheckList from "@/components/tags/TagCheckList.vue";
import { formatLocalDateTime } from "@/lib/format";
import { cmd } from "@/lib/tauri";
import { useProjectsStore } from "@/stores/projects";
import { useTagsStore } from "@/stores/tags";
import type { ReportSchedule } from "@/types";

const { t, locale } = useI18n();
const projectStore = useProjectsStore();
const tagsStore = useTagsStore();

const schedules = ref<ReportSchedule[]>([]);
const loading = ref(false);
/** 手动执行中的任务 id 集合(按钮 loading 态) */
const runningIds = ref<string[]>([]);

/** 周一~周日的本地化短标签(浏览器 Intl,避免 i18n 数组不可靠);2024-01-01 是周一 */
const weekdayNames = computed(() => {
  const fmt = new Intl.DateTimeFormat(locale.value, { weekday: "short" });
  const mon = new Date(2024, 0, 1);
  return Array.from({ length: 7 }, (_, i) => {
    const d = new Date(mon);
    d.setDate(1 + i);
    return fmt.format(d);
  });
});

const activeProjects = computed(() => projectStore.projects.filter((p) => !p.archived_at));

// ── CRUD ───────────────────────────────────────────────────────────────

async function load() {
  loading.value = true;
  try {
    schedules.value = await cmd<ReportSchedule[]>("list_report_schedules");
  } catch (e) {
    toast.error(t("reportSchedule.saveFailed"));
  } finally {
    loading.value = false;
  }
}

async function saveAll(items: ReportSchedule[]) {
  try {
    await cmd("save_report_schedules", { schedules: items });
  } catch (e) {
    toast.error(t("reportSchedule.saveFailed"));
  }
}

async function toggleSchedule(s: ReportSchedule) {
  s.enabled = !s.enabled;
  await saveAll(schedules.value);
}

async function deleteSchedule(id: string) {
  schedules.value = schedules.value.filter((s) => s.id !== id);
  await saveAll(schedules.value);
  toast.success(t("reportSchedule.deleted"));
}

/** 手动执行:忽略星期/去重检查,立即按任务配置生成报告 */
async function runNow(s: ReportSchedule) {
  if (runningIds.value.includes(s.id)) return;
  runningIds.value = [...runningIds.value, s.id];
  try {
    await cmd<number>("run_report_schedule_now", { id: s.id });
    toast.success(t("reportSchedule.runSuccess"));
    // 刷新 lastRunAt 展示
    await load();
  } catch (e) {
    const message = typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
    toast.error(t("reportSchedule.runFailed", { error: message }));
  } finally {
    runningIds.value = runningIds.value.filter((id) => id !== s.id);
  }
}

// ── dialog ─────────────────────────────────────────────────────────────

const dialogOpen = ref(false);
const editing = ref<ReportSchedule | null>(null);

// form
const formName = ref("");
const formTime = ref("18:00");
const formReportType = ref<"daily" | "weekly">("daily");
const formPreviousDay = ref(true);
const formWeekdayMode = ref<"everyday" | "weekdays" | "chineseWorkday">("everyday");
const formWeeklyWorkweek = ref(true);
const formWeeklyStart = ref(1);
const formWeeklyEnd = ref(5);
const formAuthorMode = ref<"me" | "all">("me");
const formProjectIds = ref<number[]>([]);
const formTagIds = ref<number[]>([]);

// project filter (same pattern as DailyReportDialog)
const keyword = ref("");
const filterTagIds = ref<number[]>([]);

const visibleProjects = computed(() => {
  const kw = keyword.value.trim().toLowerCase();
  return activeProjects.value.filter((p) => {
    if (kw && !p.name.toLowerCase().includes(kw) && !p.path.toLowerCase().includes(kw))
      return false;
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

// 持久化的「按标签包含」选择(存入任务,执行时动态反查项目)
const selectedIncludeTags = computed(() =>
  tagsStore.tags.filter((tag) => formTagIds.value.includes(tag.id)),
);

function toggleTagInclude(id: number) {
  formTagIds.value = formTagIds.value.includes(id)
    ? formTagIds.value.filter((x) => x !== id)
    : [...formTagIds.value, id];
}

function toggleProject(id: number) {
  formProjectIds.value = formProjectIds.value.includes(id)
    ? formProjectIds.value.filter((x) => x !== id)
    : [...formProjectIds.value, id];
}

function openCreate() {
  editing.value = null;
  formName.value = "";
  formTime.value = "09:00";
  formReportType.value = "daily";
  formPreviousDay.value = true;
  formWeekdayMode.value = "everyday";
  formWeeklyWorkweek.value = true;
  formWeeklyStart.value = 1;
  formWeeklyEnd.value = 5;
  formAuthorMode.value = "me";
  formProjectIds.value = [];
  formTagIds.value = [];
  keyword.value = "";
  filterTagIds.value = [];
  dialogOpen.value = true;
}

function openEdit(s: ReportSchedule) {
  editing.value = s;
  formName.value = s.name;
  formTime.value = s.timeOfDay;
  formReportType.value = s.reportType;
  formPreviousDay.value = s.previousDay;
  formAuthorMode.value = s.authorMode;
  formProjectIds.value = [...s.projectIds];
  formTagIds.value = [...s.tagIds];
  if (s.chineseWorkdayOnly) formWeekdayMode.value = "chineseWorkday";
  else if (s.weekdaysOnly) formWeekdayMode.value = "weekdays";
  else formWeekdayMode.value = "everyday";
  formWeeklyWorkweek.value = s.weeklyWorkweek;
  formWeeklyStart.value = s.weeklyStartWeekday || 1;
  formWeeklyEnd.value = s.weeklyEndWeekday || 5;
  keyword.value = "";
  filterTagIds.value = [];
  dialogOpen.value = true;
}

async function submit() {
  if (!formProjectIds.value.length && !formTagIds.value.length) {
    toast.error(t("report.noProjects"));
    return;
  }
  const isDaily = formReportType.value === "daily";
  const data: ReportSchedule = {
    id: editing.value?.id ?? crypto.randomUUID(),
    name: formName.value.trim(),
    enabled: editing.value?.enabled ?? true,
    reportType: formReportType.value,
    projectIds: [...formProjectIds.value],
    tagIds: [...formTagIds.value],
    authorMode: formAuthorMode.value,
    timeOfDay: formTime.value,
    weekdaysOnly: isDaily && formWeekdayMode.value === "weekdays",
    chineseWorkdayOnly: isDaily && formWeekdayMode.value === "chineseWorkday",
    previousDay: formPreviousDay.value,
    weeklyWorkweek: formWeeklyWorkweek.value,
    weeklyStartWeekday: formWeeklyStart.value,
    weeklyEndWeekday: formWeeklyEnd.value,
    lastRunAt: editing.value?.lastRunAt ?? null,
  };

  if (editing.value) {
    const idx = schedules.value.findIndex((s) => s.id === editing.value!.id);
    if (idx !== -1) schedules.value[idx] = data;
  } else {
    schedules.value.push(data);
  }
  await saveAll(schedules.value);
  toast.success(t("reportSchedule.saved"));
  dialogOpen.value = false;
}

// ── helpers ────────────────────────────────────────────────────────────

function weekdayLabel(mode: string) {
  if (mode === "chineseWorkday") return t("reportSchedule.chineseWorkdayOnly");
  if (mode === "weekdays") return t("reportSchedule.weekdaysOnly");
  return t("reportSchedule.everyday");
}

function weekdayMode(s: ReportSchedule) {
  if (s.chineseWorkdayOnly) return "chineseWorkday";
  if (s.weekdaysOnly) return "weekdays";
  return "everyday";
}

/** 周报周期描述:工作周模式显示固定文案;自定义模式显示 "周一 ~ 周五" */
function weeklyLabel(s: ReportSchedule) {
  if (s.weeklyWorkweek) return t("reportSchedule.workweekLabel");
  const start = weekdayNames.value[(s.weeklyStartWeekday - 1 + 7) % 7] ?? "";
  const end = weekdayNames.value[(s.weeklyEndWeekday - 1 + 7) % 7] ?? "";
  return `${start} ~ ${end}`;
}

function projectNames(ids: number[]) {
  return ids.map((id) => activeProjects.value.find((p) => p.id === id)?.name ?? "").filter(Boolean);
}

function tagNames(ids: number[]) {
  return ids.map((id) => tagsStore.tags.find((t) => t.id === id)?.name ?? "").filter(Boolean);
}

function lastRun(ts: number | null) {
  if (!ts) return t("reportSchedule.never");
  return formatLocalDateTime(ts);
}

// ── init ───────────────────────────────────────────────────────────────

watch(
  () => projectStore.projects.length,
  (n) => {
    if (n) load();
  },
  { immediate: true },
);
</script>

<template>
  <div class="flex flex-col gap-4">
    <div class="flex items-center justify-between">
      <div>
        <h2 class="text-base font-semibold">{{ t("reportSchedule.title") }}</h2>
        <p class="text-sm text-muted-foreground">{{ t("reportSchedule.description") }}</p>
      </div>
      <Button size="sm" class="gap-1.5" @click="openCreate">
        <Plus class="h-3.5 w-3.5" />
        {{ t("reportSchedule.create") }}
      </Button>
    </div>

    <!-- empty -->
    <div
      v-if="!loading && !schedules.length"
      class="rounded-md border border-dashed p-8 text-center text-sm text-muted-foreground"
    >
      <CalendarClock class="mx-auto mb-2 h-8 w-8 opacity-40" />
      {{ t("reportSchedule.empty") }}
    </div>

    <!-- list -->
    <div v-if="schedules.length" class="flex flex-col gap-2">
      <div v-for="s in schedules" :key="s.id" class="flex items-center gap-3 rounded-md border p-3">
        <div class="flex-1 min-w-0">
          <div class="flex items-center gap-2">
            <span class="text-sm font-medium truncate">{{
              s.name || t("reportSchedule.title")
            }}</span>
            <Badge variant="outline" class="text-[11px]">
              {{
                t(
                  s.reportType === "weekly"
                    ? "reportSchedule.typeWeekly"
                    : "reportSchedule.typeDaily",
                )
              }}
            </Badge>
            <Badge :variant="s.enabled ? 'default' : 'secondary'" class="text-[11px]">
              {{ s.enabled ? t("reportSchedule.enabled") : t("reportSchedule.disabled") }}
            </Badge>
          </div>
          <div
            class="mt-0.5 flex flex-wrap items-center gap-x-3 gap-y-0.5 text-xs text-muted-foreground"
          >
            <span class="flex items-center gap-1">
              <Clock class="h-3 w-3" />
              {{ s.timeOfDay }}
            </span>
            <span v-if="s.reportType === 'weekly'">{{ weeklyLabel(s) }}</span>
            <span v-else
              >{{
                s.previousDay
                  ? t("reportSchedule.dailyRangePrevious")
                  : t("reportSchedule.dailyRangeToday")
              }}
              · {{ weekdayLabel(weekdayMode(s)) }}</span
            >
            <span
              >{{ t("reportSchedule.authorLabel") }}:
              {{
                s.authorMode === "me" ? t("reportSchedule.authorMe") : t("reportSchedule.authorAll")
              }}</span
            >
            <span class="truncate max-w-48" :title="projectNames(s.projectIds).join(', ')">
              {{ projectNames(s.projectIds).slice(0, 2).join(", ")
              }}{{
                projectNames(s.projectIds).length > 2
                  ? ` +${projectNames(s.projectIds).length - 2}`
                  : ""
              }}
            </span>
            <span
              v-if="s.tagIds.length"
              class="flex items-center gap-1"
              :title="tagNames(s.tagIds).join(', ')"
            >
              <Tags class="h-3 w-3" />
              {{ tagNames(s.tagIds).join(", ") }}
            </span>
          </div>
          <div class="mt-1 text-[11px] text-muted-foreground">
            {{ t("reportSchedule.lastRun") }}: {{ lastRun(s.lastRunAt) }}
          </div>
        </div>
        <div class="flex items-center gap-1 shrink-0">
          <Button
            variant="ghost"
            size="icon"
            class="h-7 w-7"
            :disabled="runningIds.includes(s.id)"
            :title="t('reportSchedule.runNow')"
            @click="runNow(s)"
          >
            <Loader2 v-if="runningIds.includes(s.id)" class="h-3.5 w-3.5 animate-spin" />
            <Play v-else class="h-3.5 w-3.5" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            class="h-7 w-7"
            :title="s.enabled ? t('reportSchedule.disabled') : t('reportSchedule.enabled')"
            @click="toggleSchedule(s)"
          >
            <Power v-if="s.enabled" class="h-3.5 w-3.5 text-green-500" />
            <PowerOff v-else class="h-3.5 w-3.5 text-muted-foreground" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            class="h-7 w-7"
            :title="t('reportSchedule.edit')"
            @click="openEdit(s)"
          >
            <Pencil class="h-3.5 w-3.5" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            class="h-7 w-7 text-destructive hover:text-destructive"
            :title="t('reportSchedule.delete')"
            @click="deleteSchedule(s.id)"
          >
            <Trash2 class="h-3.5 w-3.5" />
          </Button>
        </div>
      </div>
    </div>

    <!-- create / edit dialog -->
    <Dialog v-model:open="dialogOpen">
      <DialogContent class="sm:max-w-3xl">
        <DialogHeader>
          <DialogTitle>{{
            editing ? t("reportSchedule.edit") : t("reportSchedule.create")
          }}</DialogTitle>
          <DialogDescription>{{ t("reportSchedule.description") }}</DialogDescription>
        </DialogHeader>

        <div class="grid gap-x-6 gap-y-4 py-2 sm:grid-cols-2">
          <!-- 左列:基本设置 -->
          <div class="flex min-w-0 flex-col gap-4">
            <!-- name -->
            <div class="flex flex-col gap-1.5">
              <label class="text-sm font-medium">{{ t("reportSchedule.nameLabel") }}</label>
              <Input
                v-model="formName"
                :placeholder="t('reportSchedule.namePlaceholder')"
                class="h-8"
              />
            </div>

            <!-- report type -->
            <div class="flex flex-col gap-1.5">
              <label class="text-sm font-medium">{{ t("reportSchedule.reportType") }}</label>
              <div class="flex gap-1.5">
                <Button
                  size="sm"
                  class="h-7 text-xs"
                  :variant="formReportType === 'daily' ? 'default' : 'outline'"
                  @click="formReportType = 'daily'"
                >
                  {{ t("reportSchedule.typeDaily") }}
                </Button>
                <Button
                  size="sm"
                  class="h-7 text-xs"
                  :variant="formReportType === 'weekly' ? 'default' : 'outline'"
                  @click="formReportType = 'weekly'"
                >
                  {{ t("reportSchedule.typeWeekly") }}
                </Button>
              </div>
              <p class="text-[11px] text-muted-foreground">
                {{
                  formReportType === "daily"
                    ? t("reportSchedule.typeDailyHint")
                    : t("reportSchedule.typeWeeklyHint")
                }}
              </p>
            </div>

            <!-- time -->
            <div class="flex flex-col gap-1.5">
              <label class="text-sm font-medium">{{ t("reportSchedule.timeLabel") }}</label>
              <TimeField v-model="formTime" class="w-24" />
            </div>

            <!-- 日报范围(daily):前一天(次日生成)或当天 -->
            <div v-if="formReportType === 'daily'" class="flex flex-col gap-1.5">
              <label class="text-sm font-medium">{{ t("reportSchedule.dailyRangeLabel") }}</label>
              <div class="flex gap-1.5">
                <Button
                  size="sm"
                  class="h-7 text-xs"
                  :variant="formPreviousDay ? 'default' : 'outline'"
                  @click="formPreviousDay = true"
                >
                  {{ t("reportSchedule.dailyRangePrevious") }}
                </Button>
                <Button
                  size="sm"
                  class="h-7 text-xs"
                  :variant="!formPreviousDay ? 'default' : 'outline'"
                  @click="formPreviousDay = false"
                >
                  {{ t("reportSchedule.dailyRangeToday") }}
                </Button>
              </div>
              <p v-if="!formPreviousDay" class="text-[11px] text-muted-foreground">
                {{ t("reportSchedule.dailyTodayHint") }}
              </p>
            </div>

            <!-- weekday filter(daily) -->
            <div v-if="formReportType === 'daily'" class="flex flex-col gap-1.5">
              <label class="text-sm font-medium">{{ t("reportSchedule.weekdayLabel") }}</label>
              <div class="flex flex-wrap gap-1.5">
                <Button
                  size="sm"
                  class="h-7 text-xs"
                  :variant="formWeekdayMode === 'everyday' ? 'default' : 'outline'"
                  @click="formWeekdayMode = 'everyday'"
                >
                  {{ t("reportSchedule.everyday") }}
                </Button>
                <Button
                  size="sm"
                  class="h-7 text-xs"
                  :variant="formWeekdayMode === 'weekdays' ? 'default' : 'outline'"
                  @click="formWeekdayMode = 'weekdays'"
                >
                  {{ t("reportSchedule.weekdaysOnly") }}
                </Button>
                <Button
                  size="sm"
                  class="h-7 text-xs"
                  :variant="formWeekdayMode === 'chineseWorkday' ? 'default' : 'outline'"
                  @click="formWeekdayMode = 'chineseWorkday'"
                >
                  {{ t("reportSchedule.chineseWorkdayOnly") }}
                </Button>
              </div>
              <p
                v-if="formWeekdayMode === 'chineseWorkday'"
                class="text-[11px] text-muted-foreground"
              >
                {{ t("reportSchedule.chineseWorkdayHint") }}
              </p>
            </div>

            <!-- weekly 周期模式:工作周(自动识别连续工作周期) 或 自定义周几~周几 -->
            <div v-else class="flex flex-col gap-1.5">
              <label class="text-sm font-medium">{{ t("reportSchedule.weeklyMode") }}</label>
              <div class="flex gap-1.5">
                <Button
                  size="sm"
                  class="h-7 text-xs"
                  :variant="formWeeklyWorkweek ? 'default' : 'outline'"
                  @click="formWeeklyWorkweek = true"
                >
                  {{ t("reportSchedule.weeklyModeWorkweek") }}
                </Button>
                <Button
                  size="sm"
                  class="h-7 text-xs"
                  :variant="!formWeeklyWorkweek ? 'default' : 'outline'"
                  @click="formWeeklyWorkweek = false"
                >
                  {{ t("reportSchedule.weeklyModeCustom") }}
                </Button>
              </div>
              <p v-if="formWeeklyWorkweek" class="text-[11px] text-muted-foreground">
                {{ t("reportSchedule.workweekHint") }}
              </p>
              <template v-else>
                <div class="flex flex-wrap items-center gap-1.5">
                  <span class="text-xs text-muted-foreground">
                    {{ t("reportSchedule.weeklyStart") }}
                  </span>
                  <Button
                    v-for="(name, i) in weekdayNames"
                    :key="i"
                    size="sm"
                    class="h-7 px-2 text-xs"
                    :variant="formWeeklyStart === i + 1 ? 'default' : 'outline'"
                    @click="formWeeklyStart = i + 1"
                  >
                    {{ name }}
                  </Button>
                </div>
                <div class="flex flex-wrap items-center gap-1.5">
                  <span class="text-xs text-muted-foreground">
                    {{ t("reportSchedule.weeklyEnd") }}
                  </span>
                  <Button
                    v-for="(name, i) in weekdayNames"
                    :key="i"
                    size="sm"
                    class="h-7 px-2 text-xs"
                    :variant="formWeeklyEnd === i + 1 ? 'default' : 'outline'"
                    @click="formWeeklyEnd = i + 1"
                  >
                    {{ name }}
                  </Button>
                </div>
                <p class="text-[11px] text-muted-foreground">
                  {{ t("reportSchedule.weeklyCustomHint") }}
                </p>
              </template>
            </div>

            <!-- author -->
            <div class="flex flex-col gap-1.5">
              <label class="text-sm font-medium">{{ t("reportSchedule.authorLabel") }}</label>
              <div class="flex gap-1.5">
                <Button
                  size="sm"
                  class="h-7 text-xs"
                  :variant="formAuthorMode === 'me' ? 'default' : 'outline'"
                  @click="formAuthorMode = 'me'"
                >
                  {{ t("reportSchedule.authorMe") }}
                </Button>
                <Button
                  size="sm"
                  class="h-7 text-xs"
                  :variant="formAuthorMode === 'all' ? 'default' : 'outline'"
                  @click="formAuthorMode = 'all'"
                >
                  {{ t("reportSchedule.authorAll") }}
                </Button>
              </div>
            </div>
          </div>

          <!-- 右列:选择项目 -->
          <div class="flex h-full min-h-0 flex-col gap-1.5">
            <label class="text-sm font-medium">{{ t("reportSchedule.projectsLabel") }}</label>
            <!-- 按标签动态包含(持久化进任务,执行时反查;区别于下方仅过滤显示的筛选标签) -->
            <div class="flex items-center gap-1.5">
              <DropdownMenu>
                <DropdownMenuTrigger as-child>
                  <Button variant="outline" size="sm" class="h-7 gap-1.5 px-2 text-xs">
                    <Tags class="h-3.5 w-3.5" />
                    {{ t("reportSchedule.tagIncludeLabel") }}
                    <span
                      v-if="formTagIds.length"
                      class="rounded-full bg-primary px-1.5 text-[11px] leading-4 text-primary-foreground"
                    >
                      {{ formTagIds.length }}
                    </span>
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="start" class="w-52">
                  <TagCheckList
                    :tags="tagsStore.tags"
                    :checked-ids="formTagIds"
                    @toggle="toggleTagInclude"
                  />
                  <template v-if="formTagIds.length">
                    <DropdownMenuSeparator />
                    <DropdownMenuItem class="gap-2 text-xs" @click="formTagIds = []">
                      <X class="h-3.5 w-3.5" />
                      {{ t("projects.home.clearFilter") }}
                    </DropdownMenuItem>
                  </template>
                </DropdownMenuContent>
              </DropdownMenu>
              <div v-if="selectedIncludeTags.length" class="flex flex-wrap items-center gap-1.5">
                <button
                  v-for="tag in selectedIncludeTags"
                  :key="tag.id"
                  type="button"
                  class="flex items-center gap-1 rounded-full border px-2 py-0.5 text-[11px] transition-opacity hover:opacity-80"
                  :style="{ backgroundColor: tag.color, borderColor: tag.color, color: '#fff' }"
                  @click="toggleTagInclude(tag.id)"
                >
                  {{ tag.name }}
                  <X class="h-2.5 w-2.5" />
                </button>
              </div>
            </div>
            <p class="text-[11px] text-muted-foreground">
              {{ t("reportSchedule.tagIncludeHint") }}
            </p>
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
                @click="toggleTagFilter(tag.id)"
              >
                {{ tag.name }}
                <X class="h-2.5 w-2.5" />
              </button>
            </div>
            <div
              class="grid min-h-40 flex-1 grid-cols-1 content-start gap-x-2 overflow-y-auto rounded-md border p-2"
            >
              <label
                v-for="p in visibleProjects"
                :key="p.id"
                class="flex cursor-pointer items-center gap-2 rounded px-1.5 py-1 text-sm hover:bg-accent"
              >
                <input
                  type="checkbox"
                  class="h-3.5 w-3.5 shrink-0 accent-primary"
                  :checked="formProjectIds.includes(p.id)"
                  @change="toggleProject(p.id)"
                />
                <span class="truncate" :title="p.path">{{ p.name }}</span>
              </label>
              <p v-if="!visibleProjects.length" class="px-1.5 py-2 text-xs text-muted-foreground">
                {{ t("report.noMatch") }}
              </p>
            </div>
          </div>
        </div>

        <DialogFooter>
          <Button variant="outline" size="sm" @click="dialogOpen = false">
            {{ t("common.cancel") }}
          </Button>
          <Button size="sm" @click="submit">{{ t("common.save") }}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </div>
</template>
