<script setup lang="ts">
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { Check, ChevronDown, ChevronUp, ListPlus, Pencil, Plus, Search, Trash2 } from "@lucide/vue";
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
import ScrollArea from "@/components/common/ScrollArea.vue";
import {
  createResourceSkillGroup,
  deleteResourceSkillGroup,
  reorderResourceSkillGroups,
  setResourceSkillGroupSkills,
  updateResourceSkillGroup,
  type ResourceSkill,
  type ResourceSkillGroup,
} from "@/lib/resource-library";

const props = defineProps<{
  open: boolean;
  groups: ResourceSkillGroup[];
  /** 全量技能列表:分组行展示成员数,「添加技能」选择器从中勾选 */
  skills: ResourceSkill[];
}>();

const emit = defineEmits<{
  "update:open": [value: boolean];
  /** 分组增删改/排序/成员调整后触发(父组件刷新列表) */
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
const newDescription = ref("");
const newColor = ref("#3b82f6");
const creating = ref(false);

const editingId = ref<string | null>(null);
const editName = ref("");
const editDescription = ref("");
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

/** 每个分组的技能成员数(groupId → count) */
const memberCount = computed(() => {
  const counts = new Map<string, number>();
  for (const skill of props.skills) {
    for (const groupId of skill.groupIds) {
      counts.set(groupId, (counts.get(groupId) ?? 0) + 1);
    }
  }
  return counts;
});

async function create() {
  const name = newName.value.trim();
  if (!name || creating.value) {
    return;
  }
  creating.value = true;
  try {
    await createResourceSkillGroup(name, newColor.value, newDescription.value.trim());
    newName.value = "";
    newDescription.value = "";
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
  editDescription.value = group.description ?? "";
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
    await updateResourceSkillGroup(id, name, editColor.value, editDescription.value.trim());
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

// ── 「添加技能」选择器:勾选集合整体保存,后端按差量增删成员 ──────────────

const pickerGroup = ref<ResourceSkillGroup | null>(null);
const pickerSelected = ref(new Set<string>());
const pickerQuery = ref("");
const savingPicker = ref(false);

const pickerOpen = computed({
  get: () => pickerGroup.value !== null,
  set: (v) => {
    if (!v) {
      pickerGroup.value = null;
    }
  },
});

const pickerFiltered = computed(() => {
  const q = pickerQuery.value.trim().toLowerCase();
  if (!q) {
    return props.skills;
  }
  return props.skills.filter(
    (skill) => skill.name.toLowerCase().includes(q) || skill.description.toLowerCase().includes(q),
  );
});

function openPicker(group: ResourceSkillGroup) {
  pickerGroup.value = group;
  pickerQuery.value = "";
  pickerSelected.value = new Set(
    props.skills.filter((skill) => skill.groupIds.includes(group.id)).map((skill) => skill.id),
  );
}

function togglePick(skillId: string) {
  const next = new Set(pickerSelected.value);
  if (!next.delete(skillId)) {
    next.add(skillId);
  }
  pickerSelected.value = next;
}

async function savePicker() {
  const group = pickerGroup.value;
  if (!group || savingPicker.value) {
    return;
  }
  savingPicker.value = true;
  try {
    await setResourceSkillGroupSkills(group.id, [...pickerSelected.value]);
    pickerGroup.value = null;
    toast.success(t("settings.resources.skills.groups.skillsUpdated", { name: group.name }));
    emit("changed");
  } catch (e) {
    toast.error(String(e));
  } finally {
    savingPicker.value = false;
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
          <Input
            v-model="newDescription"
            class="h-8 text-xs"
            :placeholder="t('settings.resources.skills.groups.descPlaceholder')"
            spellcheck="false"
          />
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
              <span class="flex min-w-0 flex-1 flex-col gap-1.5">
                <span class="flex items-center gap-2">
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
                <Input
                  v-model="editDescription"
                  class="h-7 text-xs"
                  :placeholder="t('settings.resources.skills.groups.descPlaceholder')"
                  spellcheck="false"
                />
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
              <span class="flex min-w-0 flex-col gap-0.5">
                <span class="flex min-w-0 items-center gap-2 text-sm">
                  <span
                    class="h-3 w-3 shrink-0 rounded-full"
                    :style="{ backgroundColor: group.color }"
                  />
                  <span class="truncate">{{ group.name }}</span>
                  <span class="shrink-0 text-[11px] text-muted-foreground">
                    {{
                      t("settings.resources.skills.groups.memberCount", {
                        count: memberCount.get(group.id) ?? 0,
                      })
                    }}
                  </span>
                </span>
                <span
                  v-if="group.description"
                  class="truncate pl-5 text-[11px] text-muted-foreground"
                  :title="group.description"
                >
                  {{ group.description }}
                </span>
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
                  :title="t('settings.resources.skills.groups.pickSkills')"
                  @click="openPicker(group)"
                >
                  <ListPlus class="h-3.5 w-3.5" />
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

  <Dialog :open="pickerOpen" @update:open="pickerOpen = $event">
    <DialogContent class="sm:max-w-md">
      <DialogHeader>
        <DialogTitle>{{
          t("settings.resources.skills.groups.pickSkillsTitle", { name: pickerGroup?.name })
        }}</DialogTitle>
        <DialogDescription>{{
          t("settings.resources.skills.groups.pickSkillsHint")
        }}</DialogDescription>
      </DialogHeader>

      <div class="relative">
        <Search
          class="absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground"
        />
        <Input
          v-model="pickerQuery"
          class="h-8 pl-8 text-xs"
          :placeholder="t('settings.resources.skills.searchPlaceholder')"
          spellcheck="false"
        />
      </div>

      <ScrollArea class="max-h-64">
        <button
          v-for="skill in pickerFiltered"
          :key="skill.id"
          type="button"
          class="flex w-full items-start gap-2 rounded-md px-2 py-1.5 text-left transition-colors hover:bg-accent"
          @click="togglePick(skill.id)"
        >
          <span
            class="mt-0.5 flex h-4 w-4 shrink-0 items-center justify-center rounded border transition-colors"
            :class="
              pickerSelected.has(skill.id)
                ? 'border-primary bg-primary text-primary-foreground'
                : 'border-input'
            "
          >
            <Check v-if="pickerSelected.has(skill.id)" class="h-3 w-3" />
          </span>
          <span class="min-w-0 flex-1">
            <span class="block truncate text-xs">{{ skill.name }}</span>
            <span v-if="skill.description" class="block truncate text-[11px] text-muted-foreground">
              {{ skill.description }}
            </span>
          </span>
        </button>
        <p
          v-if="!pickerFiltered.length"
          class="px-2 py-6 text-center text-xs text-muted-foreground"
        >
          {{
            skills.length
              ? t("settings.resources.skills.noMatch")
              : t("settings.resources.skills.empty")
          }}
        </p>
      </ScrollArea>

      <DialogFooter>
        <Button variant="outline" :disabled="savingPicker" @click="pickerOpen = false">
          {{ t("common.cancel") }}
        </Button>
        <Button :disabled="savingPicker" @click="savePicker">
          {{ t("settings.resources.skills.groups.pickSkillsSave") }}
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
