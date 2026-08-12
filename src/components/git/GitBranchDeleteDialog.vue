<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { Loader2 } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";

const { t } = useI18n();
// remote=false: 删除本地分支(-d,未合并时由父组件把 needsForce 置 true 切换为 -D 强删确认)
// remote=true: 删除远程分支(git push --delete),无 force 流程
const props = withDefaults(
  defineProps<{
    branch: string;
    needsForce?: boolean;
    remote?: boolean;
    deleting?: boolean;
  }>(),
  { needsForce: false, remote: false, deleting: false },
);
const open = defineModel<boolean>("open", { required: true });
const emit = defineEmits<{ confirm: [] }>();

const title = computed(() =>
  props.remote ? t("git.branch.deleteRemoteTitle") : t("git.branch.deleteTitle"),
);
const hint = computed(() => {
  if (props.remote) {
    return t("git.branch.deleteRemoteConfirm", { name: props.branch });
  }
  return props.needsForce
    ? t("git.branch.deleteForceHint", { name: props.branch })
    : t("git.branch.deleteConfirm", { name: props.branch });
});
const confirmLabel = computed(() => {
  if (props.deleting) {
    return t("git.branch.deleting");
  }
  return props.needsForce && !props.remote ? t("git.branch.forceDelete") : t("common.delete");
});
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent>
      <DialogHeader>
        <DialogTitle>{{ title }}</DialogTitle>
      </DialogHeader>
      <p class="text-sm break-all text-muted-foreground">{{ hint }}</p>
      <DialogFooter>
        <Button variant="outline" :disabled="deleting" @click="open = false">
          {{ t("common.cancel") }}
        </Button>
        <Button variant="destructive" :disabled="deleting" @click="emit('confirm')">
          <Loader2 v-if="deleting" class="h-3.5 w-3.5 animate-spin" />
          {{ confirmLabel }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
