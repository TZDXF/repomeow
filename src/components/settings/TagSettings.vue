<script setup lang="ts">
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { Pencil, Plus, Trash2 } from "@lucide/vue";
import ConfirmDialog from "@/components/common/ConfirmDialog.vue";
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
import { ScrollArea } from "@/components/ui/scroll-area";
import { Separator } from "@/components/ui/separator";
import { useTagsStore } from "@/stores/tags";
import type { Tag } from "@/types";

const { t } = useI18n();
const store = useTagsStore();

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

// 新建
const newName = ref("");
const newColor = ref("#3b82f6");
const submitting = ref(false);

async function create() {
  if (!newName.value.trim() || submitting.value) return;
  submitting.value = true;
  try {
    await store.createTag(newName.value.trim(), newColor.value);
    newName.value = "";
    toast.success(t("settings.tags.created"));
  } catch (e) {
    toast.error(String(e));
  } finally {
    submitting.value = false;
  }
}

// 编辑
const editingTag = ref<Tag | null>(null);
const editName = ref("");
const editColor = ref("#3b82f6");
const saving = ref(false);

function openEdit(tag: Tag) {
  editingTag.value = tag;
  editName.value = tag.name;
  editColor.value = tag.color;
}

async function saveEdit() {
  const tag = editingTag.value;
  if (!tag || !editName.value.trim() || saving.value) return;
  saving.value = true;
  try {
    await store.updateTag(tag.id, editName.value.trim(), editColor.value);
    editingTag.value = null;
    toast.success(t("settings.tags.updated"));
  } catch (e) {
    toast.error(String(e));
  } finally {
    saving.value = false;
  }
}

// 删除
const pendingRemove = ref<Tag | null>(null);
const removeConfirmOpen = computed({
  get: () => pendingRemove.value !== null,
  set: (v) => {
    if (!v) pendingRemove.value = null;
  },
});

function remove(tag: Tag) {
  pendingRemove.value = tag;
}

async function confirmRemove() {
  const tag = pendingRemove.value;
  if (!tag) return;
  try {
    await store.deleteTag(tag.id);
    toast.success(t("settings.tags.deleted"));
  } catch (e) {
    toast.error(String(e));
  }
}
</script>

<template>
  <section>
    <h2 class="text-base font-semibold">{{ t("settings.tags.title") }}</h2>
    <p class="mt-1 text-sm text-muted-foreground">{{ t("settings.tags.description") }}</p>

    <form class="mt-4 flex flex-col gap-2" @submit.prevent="create">
      <div class="flex gap-2">
        <Input v-model="newName" :placeholder="t('settings.tags.newPlaceholder')" class="flex-1" />
        <Button type="submit" size="sm" :disabled="!newName.trim() || submitting">
          <Plus class="h-4 w-4" />
          {{ t("settings.tags.create") }}
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

    <Separator class="my-4" />

    <ScrollArea class="max-h-96">
      <div class="flex flex-col gap-1">
        <div
          v-for="tag in store.tags"
          :key="tag.id"
          class="group flex items-center justify-between rounded-md px-2 py-1.5 hover:bg-accent"
        >
          <span class="flex items-center gap-2 text-sm">
            <span class="h-3 w-3 rounded-full" :style="{ backgroundColor: tag.color }" />
            {{ tag.name }}
          </span>
          <span class="flex items-center opacity-0 transition-opacity group-hover:opacity-100">
            <Button
              variant="ghost"
              size="icon"
              class="h-7 w-7"
              :title="t('settings.tags.edit')"
              @click="openEdit(tag)"
            >
              <Pencil class="h-3.5 w-3.5" />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              class="h-7 w-7"
              :title="t('settings.tags.delete')"
              @click="remove(tag)"
            >
              <Trash2 class="h-3.5 w-3.5" />
            </Button>
          </span>
        </div>
        <p v-if="!store.tags.length" class="py-6 text-center text-xs text-muted-foreground">
          {{ t("settings.tags.empty") }}
        </p>
      </div>
    </ScrollArea>

    <Dialog :open="!!editingTag" @update:open="!$event && (editingTag = null)">
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{{ t("settings.tags.editDialogTitle") }}</DialogTitle>
          <DialogDescription>{{ t("settings.tags.editDialogDescription") }}</DialogDescription>
        </DialogHeader>
        <form class="flex flex-col gap-2" @submit.prevent="saveEdit">
          <Input v-model="editName" :placeholder="t('settings.tags.editPlaceholder')" />
          <div class="flex items-center gap-1.5">
            <button
              v-for="color in PRESET_COLORS"
              :key="color"
              type="button"
              class="h-6 w-6 rounded-full border-2 transition-transform hover:scale-110"
              :class="editColor === color ? 'border-foreground' : 'border-transparent'"
              :style="{ backgroundColor: color }"
              :title="color"
              @click="editColor = color"
            />
          </div>
          <DialogFooter>
            <Button type="submit" size="sm" :disabled="!editName.trim() || saving">
              {{ t("settings.tags.save") }}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>

    <ConfirmDialog
      v-model:open="removeConfirmOpen"
      :title="t('common.delete')"
      :description="t('settings.tags.deleteConfirm', { name: pendingRemove?.name })"
      :confirm-text="t('common.delete')"
      destructive
      @confirm="confirmRemove"
    />
  </section>
</template>
