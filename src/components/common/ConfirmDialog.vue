<script setup lang="ts">
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

const { t } = useI18n();

withDefaults(
  defineProps<{
    title: string;
    description: string;
    confirmText?: string;
    /** 危险操作:确认按钮使用 destructive 样式 */
    destructive?: boolean;
  }>(),
  { confirmText: undefined, destructive: false },
);

const open = defineModel<boolean>("open", { required: true });
const emit = defineEmits<{ confirm: [] }>();

function onConfirm() {
  emit("confirm");
  open.value = false;
}
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent>
      <DialogHeader>
        <DialogTitle>{{ title }}</DialogTitle>
        <DialogDescription class="whitespace-pre-line">{{ description }}</DialogDescription>
      </DialogHeader>
      <slot></slot>
      <DialogFooter>
        <Button variant="outline" @click="open = false">{{ t("common.cancel") }}</Button>
        <Button :variant="destructive ? 'destructive' : 'default'" @click="onConfirm">
          {{ confirmText ?? t("common.confirm") }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
