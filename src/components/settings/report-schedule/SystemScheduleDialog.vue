<script setup lang="ts">
import { ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { clampIntervalMinutes } from "@/composables/report-schedule/schedule-utils";
import type { SystemSchedule } from "@/types";

/** 内置任务间隔编辑:不可更名、不可删除,仅调整执行间隔(分钟) */
const props = defineProps<{ schedule: SystemSchedule | null }>();
const open = defineModel<boolean>("open", { required: true });
const emit = defineEmits<{ save: [intervalMinutes: number] }>();

const { t } = useI18n();
const formInterval = ref(10);

watch(open, (isOpen) => {
  if (isOpen && props.schedule) {
    formInterval.value = props.schedule.intervalMinutes;
  }
});

function submit() {
  emit("save", clampIntervalMinutes(formInterval.value));
}
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="sm:max-w-sm">
      <DialogHeader>
        <DialogTitle>{{ t("reportSchedule.editSystemTask") }}</DialogTitle>
        <DialogDescription>{{ t("reportSchedule.systemTaskHint") }}</DialogDescription>
      </DialogHeader>
      <div class="flex flex-col gap-1.5 py-2">
        <label class="text-sm font-medium">{{ t("reportSchedule.intervalLabel") }}</label>
        <div class="flex items-center gap-2">
          <Input
            v-model.number="formInterval"
            type="number"
            min="1"
            max="1440"
            step="1"
            class="h-8 w-28"
          />
          <span class="text-sm text-muted-foreground">{{ t("reportSchedule.minutes") }}</span>
        </div>
        <p class="text-[11px] text-muted-foreground">
          {{ t("reportSchedule.intervalHint") }}
        </p>
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
