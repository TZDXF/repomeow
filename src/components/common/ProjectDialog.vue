<script setup lang="ts">
import { inject } from "vue";
import { DialogRoot, type DialogRootEmits, type DialogRootProps } from "reka-ui";
import { useProjectDialogState } from "@/composables/useProjectDialogState";
import { PROJECT_PAGE_ACTIVE } from "@/composables/useProjectPageState";

const props = withDefaults(defineProps<DialogRootProps>(), {
  open: undefined,
  modal: undefined,
  unmountOnHide: undefined,
});
const emit = defineEmits<DialogRootEmits>();
const active = inject(PROJECT_PAGE_ACTIVE, undefined);
const { visible, unmountOnHide, updateOpen } = useProjectDialogState(
  props,
  (value) => emit("update:open", value),
  active,
);
</script>

<template>
  <DialogRoot
    v-slot="slotProps"
    data-slot="dialog"
    :open="visible"
    :modal="modal"
    :unmount-on-hide="unmountOnHide"
    @update:open="updateOpen"
  >
    <slot v-bind="slotProps" />
  </DialogRoot>
</template>
