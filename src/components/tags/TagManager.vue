<script setup lang="ts">
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { Plus, Trash2 } from "@lucide/vue";
import ConfirmDialog from "@/components/common/ConfirmDialog.vue";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Separator } from "@/components/ui/separator";
import { useTagsStore } from "@/stores/tags";

const { t } = useI18n();
const emit = defineEmits<{ refreshProjects: [] }>();

const store = useTagsStore();

const visible = ref(false);
const newName = ref("");
const newColor = ref("#3b82f6");
const submitting = ref(false);

const PRESET_COLORS = [
  "#3b82f6",
  "#22c55e",
  "#eab308",
  "#ef4444",
  "#a855f7",
  "#ec4899",
  "#14b8a6",
  "#f97316",
];

async function create() {
  if (!newName.value.trim() || submitting.value) return;
  submitting.value = true;
  try {
    await store.createTag(newName.value.trim(), newColor.value);
    newName.value = "";
    toast.success(t("tags.manager.created"));
  } catch (e) {
    toast.error(String(e));
  } finally {
    submitting.value = false;
  }
}

/** 待确认删除的标签,ConfirmDialog 确认后执行 */
const pendingRemove = ref<{ id: number; name: string } | null>(null);
const removeConfirmOpen = computed({
  get: () => pendingRemove.value !== null,
  set: (v) => {
    if (!v) pendingRemove.value = null;
  },
});

function remove(id: number, name: string) {
  pendingRemove.value = { id, name };
}

async function confirmRemove() {
  const tag = pendingRemove.value;
  if (!tag) return;
  try {
    await store.deleteTag(tag.id);
    emit("refreshProjects");
    toast.success(t("tags.manager.deleted"));
  } catch (e) {
    toast.error(String(e));
  }
}
</script>

<template>
  <Dialog v-model:open="visible">
    <DialogTrigger as-child>
      <slot />
    </DialogTrigger>
    <DialogContent>
      <DialogHeader>
        <DialogTitle>{{ t("tags.manager.title") }}</DialogTitle>
        <DialogDescription>{{ t("tags.manager.description") }}</DialogDescription>
      </DialogHeader>
      <form class="flex flex-col gap-2" @submit.prevent="create">
        <div class="flex gap-2">
          <Input v-model="newName" :placeholder="t('tags.manager.newPlaceholder')" class="flex-1" />
          <Button type="submit" size="sm" :disabled="!newName.trim() || submitting">
            <Plus class="h-4 w-4" />
            {{ t("tags.manager.create") }}
          </Button>
        </div>
        <div class="flex items-center gap-1.5">
          <button
            v-for="color in PRESET_COLORS"
            :key="color"
            type="button"
            class="h-6 w-6 rounded-full border-2 transition-transform hover:scale-110"
            :class="newColor === color ? 'border-foreground' : 'border-transparent'"
            :style="{ backgroundColor: color }"
            :title="color"
            @click="newColor = color"
          />
        </div>
      </form>
      <Separator />
      <ScrollArea class="max-h-64">
        <div class="flex flex-col gap-1">
          <div
            v-for="tag in store.tags"
            :key="tag.id"
            class="flex items-center justify-between rounded-md px-2 py-1.5 hover:bg-accent"
          >
            <span class="flex items-center gap-2 text-sm">
              <span class="h-3 w-3 rounded-full" :style="{ backgroundColor: tag.color }" />
              {{ tag.name }}
            </span>
            <Button
              variant="ghost"
              size="icon"
              class="h-7 w-7"
              :title="t('tags.manager.delete')"
              @click="remove(tag.id, tag.name)"
            >
              <Trash2 class="h-3.5 w-3.5" />
            </Button>
          </div>
          <p v-if="!store.tags.length" class="py-6 text-center text-xs text-muted-foreground">
            {{ t("tags.manager.empty") }}
          </p>
        </div>
      </ScrollArea>
    </DialogContent>
  </Dialog>
  <ConfirmDialog
    v-model:open="removeConfirmOpen"
    :title="t('common.delete')"
    :description="t('tags.manager.deleteConfirm', { name: pendingRemove?.name })"
    :confirm-text="t('common.delete')"
    destructive
    @confirm="confirmRemove"
  />
</template>
