<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { Tags, X } from "@lucide/vue";
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
import ReportProjectSelector from "@/components/report/ReportProjectSelector.vue";
import TagCheckList from "@/components/tags/TagCheckList.vue";
import {
  buildScheduleFromForm,
  createEmptyScheduleForm,
  isScheduleFormValid,
  scheduleToForm,
  weekdayShortNames,
  type ScheduleFormState,
} from "@/composables/report-schedule/schedule-utils";
import { useProjectsStore } from "@/stores/projects";
import { useTagsStore } from "@/stores/tags";
import type { ReportSchedule } from "@/types";

/** 定时报告 新建/编辑 对话框:只负责表单与校验,持久化由父组件经 submit 事件完成 */
const props = defineProps<{ editing: ReportSchedule | null }>();
const open = defineModel<boolean>("open", { required: true });
const emit = defineEmits<{ submit: [data: ReportSchedule] }>();

const { t, locale } = useI18n();
const projectStore = useProjectsStore();
const tagsStore = useTagsStore();

const form = ref<ScheduleFormState>(createEmptyScheduleForm());

/** 周一~周日的本地化短标签(浏览器 Intl,避免 i18n 数组不可靠);2024-01-01 是周一 */
const weekdayNames = computed(() => weekdayShortNames(locale.value));

const activeProjects = computed(() => projectStore.projects.filter((p) => !p.archived_at));

// 每次打开时重建表单;关键词/筛选标签等临时筛选状态随对话框卸载自动重置
watch(open, (isOpen) => {
  if (!isOpen) return;
  form.value = props.editing ? scheduleToForm(props.editing) : createEmptyScheduleForm();
});

// ── 按标签动态包含(持久化进任务,执行时反查项目;与 ReportProjectSelector 内的临时筛选标签无关) ──

const selectedIncludeTags = computed(() =>
  tagsStore.tags.filter((tag) => form.value.tagIds.includes(tag.id)),
);

function toggleTagInclude(id: number) {
  const ids = form.value.tagIds;
  form.value.tagIds = ids.includes(id) ? ids.filter((x) => x !== id) : [...ids, id];
}

function submit() {
  if (!isScheduleFormValid(form.value)) {
    toast.error(t("report.noProjects"));
    return;
  }
  emit("submit", buildScheduleFromForm(form.value, props.editing));
}
</script>

<template>
  <Dialog v-model:open="open">
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
              v-model="form.name"
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
                :variant="form.reportType === 'daily' ? 'default' : 'outline'"
                @click="form.reportType = 'daily'"
              >
                {{ t("reportSchedule.typeDaily") }}
              </Button>
              <Button
                size="sm"
                class="h-7 text-xs"
                :variant="form.reportType === 'weekly' ? 'default' : 'outline'"
                @click="form.reportType = 'weekly'"
              >
                {{ t("reportSchedule.typeWeekly") }}
              </Button>
            </div>
            <p class="text-[11px] text-muted-foreground">
              {{
                form.reportType === "daily"
                  ? t("reportSchedule.typeDailyHint")
                  : t("reportSchedule.typeWeeklyHint")
              }}
            </p>
          </div>

          <!-- time -->
          <div class="flex flex-col gap-1.5">
            <label class="text-sm font-medium">{{ t("reportSchedule.timeLabel") }}</label>
            <TimeField v-model="form.time" class="w-24" />
          </div>

          <!-- 日报范围(daily):前一天(次日生成)或当天 -->
          <div v-if="form.reportType === 'daily'" class="flex flex-col gap-1.5">
            <label class="text-sm font-medium">{{ t("reportSchedule.dailyRangeLabel") }}</label>
            <div class="flex gap-1.5">
              <Button
                size="sm"
                class="h-7 text-xs"
                :variant="form.previousDay ? 'default' : 'outline'"
                @click="form.previousDay = true"
              >
                {{ t("reportSchedule.dailyRangePrevious") }}
              </Button>
              <Button
                size="sm"
                class="h-7 text-xs"
                :variant="!form.previousDay ? 'default' : 'outline'"
                @click="form.previousDay = false"
              >
                {{ t("reportSchedule.dailyRangeToday") }}
              </Button>
            </div>
            <p v-if="!form.previousDay" class="text-[11px] text-muted-foreground">
              {{ t("reportSchedule.dailyTodayHint") }}
            </p>
          </div>

          <!-- weekday filter(daily) -->
          <div v-if="form.reportType === 'daily'" class="flex flex-col gap-1.5">
            <label class="text-sm font-medium">{{ t("reportSchedule.weekdayLabel") }}</label>
            <div class="flex flex-wrap gap-1.5">
              <Button
                size="sm"
                class="h-7 text-xs"
                :variant="form.weekdayMode === 'everyday' ? 'default' : 'outline'"
                @click="form.weekdayMode = 'everyday'"
              >
                {{ t("reportSchedule.everyday") }}
              </Button>
              <Button
                size="sm"
                class="h-7 text-xs"
                :variant="form.weekdayMode === 'weekdays' ? 'default' : 'outline'"
                @click="form.weekdayMode = 'weekdays'"
              >
                {{ t("reportSchedule.weekdaysOnly") }}
              </Button>
              <Button
                size="sm"
                class="h-7 text-xs"
                :variant="form.weekdayMode === 'chineseWorkday' ? 'default' : 'outline'"
                @click="form.weekdayMode = 'chineseWorkday'"
              >
                {{ t("reportSchedule.chineseWorkdayOnly") }}
              </Button>
            </div>
            <p
              v-if="form.weekdayMode === 'chineseWorkday'"
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
                :variant="form.weeklyWorkweek ? 'default' : 'outline'"
                @click="form.weeklyWorkweek = true"
              >
                {{ t("reportSchedule.weeklyModeWorkweek") }}
              </Button>
              <Button
                size="sm"
                class="h-7 text-xs"
                :variant="!form.weeklyWorkweek ? 'default' : 'outline'"
                @click="form.weeklyWorkweek = false"
              >
                {{ t("reportSchedule.weeklyModeCustom") }}
              </Button>
            </div>
            <p v-if="form.weeklyWorkweek" class="text-[11px] text-muted-foreground">
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
                  :variant="form.weeklyStart === i + 1 ? 'default' : 'outline'"
                  @click="form.weeklyStart = i + 1"
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
                  :variant="form.weeklyEnd === i + 1 ? 'default' : 'outline'"
                  @click="form.weeklyEnd = i + 1"
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
                :variant="form.authorMode === 'me' ? 'default' : 'outline'"
                @click="form.authorMode = 'me'"
              >
                {{ t("reportSchedule.authorMe") }}
              </Button>
              <Button
                size="sm"
                class="h-7 text-xs"
                :variant="form.authorMode === 'all' ? 'default' : 'outline'"
                @click="form.authorMode = 'all'"
              >
                {{ t("reportSchedule.authorAll") }}
              </Button>
            </div>
          </div>
        </div>

        <!-- 右列:选择项目(筛选关键词/筛选标签由 ReportProjectSelector 内部管理) -->
        <ReportProjectSelector
          v-model="form.projectIds"
          :projects="activeProjects"
          :tags="tagsStore.tags"
          fill-height
          class="h-full min-h-0"
        >
          <template #controls>
            <!-- 按标签动态包含(持久化进任务,执行时反查;区别于下方仅过滤显示的筛选标签) -->
            <div class="flex items-center gap-1.5">
              <DropdownMenu>
                <DropdownMenuTrigger as-child>
                  <Button variant="outline" size="sm" class="h-7 gap-1.5 px-2 text-xs">
                    <Tags class="h-3.5 w-3.5" />
                    {{ t("reportSchedule.tagIncludeLabel") }}
                    <span
                      v-if="form.tagIds.length"
                      class="rounded-full bg-primary px-1.5 text-[11px] leading-4 text-primary-foreground"
                    >
                      {{ form.tagIds.length }}
                    </span>
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="start" class="w-52">
                  <TagCheckList
                    :tags="tagsStore.tags"
                    :checked-ids="form.tagIds"
                    @toggle="toggleTagInclude"
                  />
                  <template v-if="form.tagIds.length">
                    <DropdownMenuSeparator />
                    <DropdownMenuItem class="gap-2 text-xs" @click="form.tagIds = []">
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
          </template>
        </ReportProjectSelector>
      </div>

      <DialogFooter>
        <Button variant="outline" size="sm" @click="open = false">
          {{ t("common.cancel") }}
        </Button>
        <Button size="sm" @click="submit">{{ t("common.save") }}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
