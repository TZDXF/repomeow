<script setup lang="ts">
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { Check, ChevronDown, ChevronUp, Pencil, Plus, Trash2 } from "@lucide/vue";
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
import {
  createResourceSkillGroup,
  deleteResourceSkillGroup,
  reorderResourceSkillGroups,
  updateResourceSkillGroup,
  type ResourceSkillGroup,
} from "@/lib/resource-library";

const props = defineProps<{
  open: boolean;
  groups: ResourceSkillGroup[];
}>();

const emit = defineEmits<{
  "update:open": [value: boolean];
  /** 分组增删改/排序后触发(父组件刷新列表) */
  changed: [];
}>();

const { t } = useI18n();

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

const newName = ref("");
const newColor = ref("#3b82f6");
const creating = ref(false);

const editingId = ref<string | null>(null);
const editName = ref("");
const editColor = ref("#3b82f6");
const savingEdit = ref(false);

const reordering = ref(false);

const pendingDelete = ref<ResourceSkillGroup | null>(null);
const deleteConfirmOpen = computed({
  get: () => pendingDelete.value !== null,
  set: (v) => {
    if (!v) {
      pendingDelete.value = null;
    }
  },
});

async function create() {
  const name = newName.value.trim();
  if (!name || creating.value) {
    return;
  }
  creating.value = true;
  try {
    await createResourceSkillGroup(name, newColor.value);
    newName.value = "";
    toast.success(t("settings.resources.skills.groups.created"));
    emit("changed");
  } catch (e) {
    toast.error(String(e));
  } finally {
    creating.value = false;
  }
}

function startEdit(group: ResourceSkillGroup) {
  editingId.value = group.id;
  editName.value = group.name;
  editColor.value = group.color ?? "#3b82f6";
}

async function saveEdit() {
  const id = editingId.value;
  const name = editName.value.trim();
  if (!id || !name || savingEdit.value) {
    return;
  }
  savingEdit.value = true;
  try {
    await updateResourceSkillGroup(id, name, editColor.value);
    editingId.value = null;
    toast.success(t("settings.resources.skills.groups.updated"));
    emit("changed");
  } catch (e) {
    toast.error(String(e));
  } finally {
    savingEdit.value = false;
  }
}

async function confirmRemove() {
  const group = pendingDelete.value;
  if (!group) {
    return;
  }
  try {
    await deleteResourceSkillGroup(group.id);
    toast.success(t("settings.resources.skills.groups.deleted"));
    emit("changed");
  } catch (e) {
    toast.error(String(e));
  } finally {
    pendingDelete.value = null;
  }
}

async function move(index: number, dir: -1 | 1) {
  const target = index + dir;
  if (target < 0 || target >= props.groups.length || reordering.value) {
    return;
  }
  reordering.value = true;
  const ordered = props.groups.map((g) => g.id);
  [ordered[index], ordered[target]] = [ordered[target], ordered[index]];
  try {
    await reorderResourceSkillGroups(ordered);
    emit("changed");
  } catch (e) {
    toast.error(String(e));
  } finally {
    reordering.value = false;
  }
}
</script>

<template>
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <DialogContent class="sm:max-w-md">
      <DialogHeader>
        <DialogTitle>{{ t("settings.resources.skills.groups.title") }}</DialogTitle>
        <DialogDescription>{{
          t("settings.resources.skills.groups.description")
        }}</DialogDescription>
      </DialogHeader>

      <div class="flex flex-col gap-3 py-1">
        <form class="flex flex-col gap-2" @submit.prevent="create">
          <div class="flex gap-2">
            <Input
              v-model="newName"
              class="h-8 flex-1 text-xs"
              :placeholder="t('settings.resources.skills.groups.newPlaceholder')"
              spellcheck="false"
              @keyup.enter="create"
            />
            <Button
              type="submit"
              size="sm"
              class="h-8 gap-1"
              :disabled="!newName.trim() || creating"
            >
              <Plus class="h-3.5 w-3.5" />
              {{ t("settings.resources.skills.groups.create") }}
            </Button>
          </div>
          <div class="flex items-center gap-1.5">
            <button
              v-for="color in PRESET_COLORS"
              :key="color"
              type="button"
              class="h-5 w-5 rounded-full border-2 transition-transform hover:scale-110"
              :class="newColor === color ? 'border-foreground' : 'border-transparent'"
              :style="{ backgroundColor: color }"
              :title="color"
              @click="newColor = color"
            />
          </div>
        </form>

        <div class="flex flex-col gap-1">
          <div
            v-for="(group, index) in groups"
            :key="group.id"
            class="group flex items-center justify-between gap-2 rounded-md px-2 py-1.5 hover:bg-accent"
          >
            <template v-if="editingId === group.id">
              <span class="flex min-w-0 flex-1 items-center gap-2">
                <Input
                  v-model="editName"
                  class="h-7 min-w-0 flex-1 text-xs"
                  spellcheck="false"
                  @keyup.enter="saveEdit"
                />
                <span class="flex items-center gap-1">
                  <button
                    v-for="color in PRESET_COLORS"
                    :key="color"
                    type="button"
                    class="h-4 w-4 rounded-full border-2 transition-transform hover:scale-110"
                    :class="editColor === color ? 'border-foreground' : 'border-transparent'"
                    :style="{ backgroundColor: color }"
                    :title="color"
                    @click="editColor = color"
                  />
                </span>
              </span>
              <Button
                size="icon"
                class="h-7 w-7 shrink-0"
                :disabled="!editName.trim() || savingEdit"
                :title="t('common.save')"
                @click="saveEdit"
              >
                <Check class="h-3.5 w-3.5" />
              </Button>
            </template>
            <template v-else>
              <span class="flex min-w-0 items-center gap-2 text-sm">
                <span
                  class="h-3 w-3 shrink-0 rounded-full"
                  :style="{ backgroundColor: group.color }"
                />
                <span class="truncate">{{ group.name }}</span>
              </span>
              <span
                class="flex shrink-0 items-center opacity-0 transition-opacity group-hover:opacity-100 focus-within:opacity-100"
              >
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7"
                  :disabled="index === 0 || reordering"
                  :title="t('settings.resources.skills.up')"
                  @click="move(index, -1)"
                >
                  <ChevronUp class="h-3.5 w-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7"
                  :disabled="index === groups.length - 1 || reordering"
                  :title="t('settings.resources.skills.down')"
                  @click="move(index, 1)"
                >
                  <ChevronDown class="h-3.5 w-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7"
                  :title="t('settings.resources.skills.groups.rename')"
                  @click="startEdit(group)"
                >
                  <Pencil class="h-3.5 w-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7 text-destructive"
                  :title="t('settings.resources.skills.groups.delete')"
                  @click="pendingDelete = group"
                >
                  <Trash2 class="h-3.5 w-3.5" />
                </Button>
              </span>
            </template>
          </div>
          <p v-if="!groups.length" class="py-6 text-center text-xs text-muted-foreground">
            {{ t("settings.resources.skills.groups.empty") }}
          </p>
        </div>

        <p class="text-xs text-muted-foreground">
          {{ t("settings.resources.skills.groups.deleteHint") }}
        </p>
      </div>

      <DialogFooter>
        <Button variant="outline" @click="emit('update:open', false)">
          {{ t("common.close") }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>

  <ConfirmDialog
    v-model:open="deleteConfirmOpen"
    :title="t('common.delete')"
    :description="
      t('settings.resources.skills.groups.deleteConfirm', { name: pendingDelete?.name })
    "
    :confirm-text="t('common.delete')"
    destructive
    @confirm="confirmRemove"
  />
</template>
