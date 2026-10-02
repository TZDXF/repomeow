<script setup lang="ts">
import { Clock, GitBranch, Pencil, Power, PowerOff } from "@lucide/vue";
import { useI18n } from "vue-i18n";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { formatLocalDateTime } from "@/lib/format";
import type { SystemSchedule } from "@/types";

/** 应用内置任务列表:固定存在,只允许启停和修改执行间隔(不可删除/更名) */
defineProps<{ schedules: SystemSchedule[] }>();
const emit = defineEmits<{
  toggle: [schedule: SystemSchedule];
  edit: [schedule: SystemSchedule];
}>();

const { t } = useI18n();

function lastRun(ts: number | null) {
  if (!ts) return t("reportSchedule.never");
  return formatLocalDateTime(ts);
}
</script>

<template>
  <div class="flex flex-col gap-2">
    <div
      v-for="schedule in schedules"
      :key="schedule.id"
      class="flex items-center gap-3 rounded-md border p-3"
    >
      <GitBranch class="h-4 w-4 shrink-0 text-primary" />
      <div class="min-w-0 flex-1">
        <div class="flex items-center gap-2">
          <span class="truncate text-sm font-medium">
            {{ t("reportSchedule.systemGitUpdate") }}
          </span>
          <Badge variant="outline" class="text-[11px]">
            {{ t("reportSchedule.systemTask") }}
          </Badge>
          <Badge :variant="schedule.enabled ? 'default' : 'secondary'" class="text-[11px]">
            {{ schedule.enabled ? t("reportSchedule.enabled") : t("reportSchedule.disabled") }}
          </Badge>
        </div>
        <div class="mt-0.5 flex items-center gap-1 text-xs text-muted-foreground">
          <Clock class="h-3 w-3" />
          {{ t("reportSchedule.everyMinutes", { count: schedule.intervalMinutes }) }}
        </div>
        <div class="mt-1 text-[11px] text-muted-foreground">
          {{ t("reportSchedule.lastRun") }}: {{ lastRun(schedule.lastRunAt) }}
        </div>
      </div>
      <div class="flex shrink-0 items-center gap-1">
        <Button
          variant="ghost"
          size="icon"
          class="h-7 w-7"
          :title="schedule.enabled ? t('reportSchedule.enabled') : t('reportSchedule.disabled')"
          @click="emit('toggle', schedule)"
        >
          <Power v-if="schedule.enabled" class="h-3.5 w-3.5 text-green-500" />
          <PowerOff v-else class="h-3.5 w-3.5 text-muted-foreground" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          class="h-7 w-7"
          :title="t('reportSchedule.edit')"
          @click="emit('edit', schedule)"
        >
          <Pencil class="h-3.5 w-3.5" />
        </Button>
      </div>
    </div>
  </div>
</template>
