<script setup lang="ts">
import { ref } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { CalendarClock, Plus } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { useReportSchedules } from "@/composables/report-schedule/useReportSchedules";
import ScheduleEditDialog from "./report-schedule/ScheduleEditDialog.vue";
import ScheduleList from "./report-schedule/ScheduleList.vue";
import SystemScheduleDialog from "./report-schedule/SystemScheduleDialog.vue";
import SystemScheduleList from "./report-schedule/SystemScheduleList.vue";
import type { ReportSchedule, SystemSchedule } from "@/types";

/**
 * 设置页「定时报告」板块:编排层。
 * 数据与持久化在 useReportSchedules;列表/对话框拆到 report-schedule/ 子组件;
 * 表单构造与展示文案等纯逻辑在 composables/report-schedule/schedule-utils.ts。
 */
const { t } = useI18n();
const {
  schedules,
  systemSchedules,
  loading,
  runningIds,
  saveSystemSchedule,
  toggleSystemSchedule,
  toggleSchedule,
  upsertSchedule,
  deleteSchedule,
  runNow,
} = useReportSchedules();

// ── 对话框开关与编辑目标 ─────────────────────────────────────────────────

const dialogOpen = ref(false);
const editing = ref<ReportSchedule | null>(null);
const systemDialogOpen = ref(false);
const systemEditing = ref<SystemSchedule | null>(null);

function openCreate() {
  editing.value = null;
  dialogOpen.value = true;
}

function openEdit(s: ReportSchedule) {
  editing.value = s;
  dialogOpen.value = true;
}

function openSystemEdit(schedule: SystemSchedule) {
  systemEditing.value = schedule;
  systemDialogOpen.value = true;
}

async function submitSchedule(data: ReportSchedule) {
  await upsertSchedule(data);
  dialogOpen.value = false;
}

async function submitSystemSchedule(intervalMinutes: number) {
  const schedule = systemEditing.value;
  if (!schedule) {
    return;
  }
  try {
    await saveSystemSchedule(schedule, schedule.enabled, intervalMinutes);
    toast.success(t("reportSchedule.saved"));
    systemDialogOpen.value = false;
  } catch {
    toast.error(t("reportSchedule.saveFailed"));
  }
}
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

    <!-- 应用内置任务：固定存在，只允许启停和修改执行间隔 -->
    <SystemScheduleList
      v-if="systemSchedules.length"
      :schedules="systemSchedules"
      @toggle="toggleSystemSchedule"
      @edit="openSystemEdit"
    />

    <!-- empty -->
    <div
      v-if="!loading && !systemSchedules.length && !schedules.length"
      class="rounded-md border border-dashed p-8 text-center text-sm text-muted-foreground"
    >
      <CalendarClock class="mx-auto mb-2 h-8 w-8 opacity-40" />
      {{ t("reportSchedule.empty") }}
    </div>

    <!-- list -->
    <ScheduleList
      v-if="schedules.length"
      :schedules="schedules"
      :running-ids="runningIds"
      @toggle="toggleSchedule"
      @edit="openEdit"
      @remove="deleteSchedule"
      @run-now="runNow"
    />

    <!-- 内置任务间隔编辑：不可更名、不可删除 -->
    <SystemScheduleDialog
      v-model:open="systemDialogOpen"
      :schedule="systemEditing"
      @save="submitSystemSchedule"
    />

    <!-- create / edit dialog -->
    <ScheduleEditDialog v-model:open="dialogOpen" :editing="editing" @submit="submitSchedule" />
  </div>
</template>
