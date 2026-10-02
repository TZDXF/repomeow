<script setup lang="ts">
import { useI18n } from "vue-i18n";
import { Icon } from "@iconify/vue";
import { Bot, LoaderCircle } from "@lucide/vue";
import { agentBrandIcon } from "@/lib/agent-icons";
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
  type RepairAction,
  type ResourceChoice,
  type ResourceDeployment,
} from "@/lib/project-ai-resources";
import { repairBusyKey } from "@/composables/project-resources/use-resource-repair";

const props = defineProps<{
  /** 当前修复的资源;null 时面板关闭 */
  resource: ResourceChoice | null;
  /** 该资源的可修复部署记录(父层已按状态过滤) */
  records: ResourceDeployment[];
  /** 修复防重入锁:repairBusyKey 格式,非空时全部按钮禁用 */
  repairing: string;
  /** Agent 显示名回退(id → name) */
  agentName: (agentId: string) => string;
}>();
const emit = defineEmits<{
  close: [];
  applyUpdate: [record: ResourceDeployment];
  repair: [record: ResourceDeployment, action: RepairAction];
}>();
const { t } = useI18n();

function onOpenChange(value: boolean) {
  if (!props.repairing && !value) {
    emit("close");
  }
}
</script>

<template>
  <Dialog :open="!!resource" @update:open="onOpenChange">
    <DialogContent class="sm:max-w-lg">
      <DialogHeader>
        <DialogTitle>{{ t("projectAi.repairTitle", { name: resource?.name }) }}</DialogTitle>
        <DialogDescription>{{ t("projectAi.repairHint") }}</DialogDescription>
      </DialogHeader>
      <div class="divide-y rounded-md border">
        <div
          v-for="record in records"
          :key="record.agentId"
          class="flex flex-wrap items-center gap-2 px-3 py-2.5"
        >
          <span
            class="flex size-6 shrink-0 items-center justify-center rounded border text-muted-foreground"
          >
            <Icon
              v-if="agentBrandIcon(record.agentId)"
              :icon="agentBrandIcon(record.agentId)!"
              class="size-3.5"
            />
            <Bot v-else class="size-3.5" />
          </span>
          <div class="min-w-0 flex-1">
            <p class="truncate text-xs font-medium">
              {{ agentName(record.agentId) }} ·
              <span class="text-amber-600 dark:text-amber-400">{{
                t(`projectAi.states.${record.status}`)
              }}</span>
            </p>
            <p class="mt-1 truncate font-mono text-[10px] text-muted-foreground">
              {{ record.path }}
            </p>
          </div>
          <div class="flex gap-1">
            <Button
              v-if="record.status === 'update' || record.status === 'missing'"
              size="sm"
              class="h-7 px-2 text-xs"
              :disabled="!!repairing"
              :title="t('projectAi.applyUpdateHint')"
              @click="emit('applyUpdate', record)"
              ><LoaderCircle
                v-if="repairing === repairBusyKey(record, 'update')"
                class="size-3.5 animate-spin"
              />{{ t("projectAi.applyUpdate") }}</Button
            >
            <Button
              v-if="record.status === 'modified'"
              size="sm"
              class="h-7 px-2 text-xs"
              :disabled="!!repairing"
              :title="t('projectAi.forceApplyHint')"
              @click="emit('repair', record, 'reapply')"
              ><LoaderCircle
                v-if="repairing === repairBusyKey(record, 'reapply')"
                class="size-3.5 animate-spin"
              />{{ t("projectAi.forceApply") }}</Button
            >
            <Button
              v-if="record.status === 'modified' || record.status === 'conflict'"
              variant="outline"
              size="sm"
              class="h-7 px-2 text-xs"
              :disabled="!!repairing"
              :title="t('projectAi.detachHint')"
              @click="emit('repair', record, 'detach')"
              ><LoaderCircle
                v-if="repairing === repairBusyKey(record, 'detach')"
                class="size-3.5 animate-spin"
              />{{ t("projectAi.detach") }}</Button
            >
          </div>
        </div>
      </div>
      <DialogFooter>
        <Button variant="ghost" :disabled="!!repairing" @click="emit('close')">{{
          t("common.cancel")
        }}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
