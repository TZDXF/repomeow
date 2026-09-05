<script setup lang="ts">
import { computed, onActivated, ref } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  ChevronDown,
  ChevronUp,
  Download,
  FileArchive,
  FolderOpen,
  Layers,
  Link2,
  Pencil,
  Search,
  Trash2,
} from "@lucide/vue";
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
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import {
  deleteResourceSkill,
  filterSkills,
  importResourceSkillArchive,
  importResourceSkillFolder,
  importResourceSkillUrl,
  listResourceSkills,
  mergeReorderedVisible,
  openResourceSkillDir,
  reorderResourceSkills,
  type ResourceSkill,
  type ResourceSkillGroup,
  type ResourceSkillImportOutcome,
} from "@/lib/resource-library";
import ResourceSkillEditDialog from "./ResourceSkillEditDialog.vue";
import ResourceSkillGroupsDialog from "./ResourceSkillGroupsDialog.vue";

const { t } = useI18n();

const loading = ref(true);
const groups = ref<ResourceSkillGroup[]>([]);
const skills = ref<ResourceSkill[]>([]);
const query = ref("");
const activeGroupId = ref<string | null>(null);

const editDialogOpen = ref(false);
const editingSkill = ref<ResourceSkill | null>(null);
const groupsDialogOpen = ref(false);
const pendingDelete = ref<ResourceSkill | null>(null);

const importing = ref(false);
const urlDialogOpen = ref(false);
const urlInput = ref("");
const urlSubmitting = ref(false);

const filtered = computed(() => filterSkills(skills.value, query.value, activeGroupId.value));
const groupMap = computed(() => new Map(groups.value.map((g) => [g.id, g])));
const deleteConfirmOpen = computed({
  get: () => pendingDelete.value !== null,
  set: (v) => {
    if (!v) {
      pendingDelete.value = null;
    }
  },
});

async function load() {
  loading.value = true;
  try {
    const list = await listResourceSkills();
    groups.value = list.groups;
    skills.value = list.skills;
    if (activeGroupId.value && !groups.value.some((g) => g.id === activeGroupId.value)) {
      activeGroupId.value = null;
    }
  } catch (e) {
    toast.error(String(e));
  } finally {
    loading.value = false;
  }
}

// 本页固定在 KeepAlive 内:onActivated 首次挂载与每次切回都会触发,
// 从市场标签页安装技能后切回即可看到新技能(KeepAlive 不会重跑 onMounted)
onActivated(load);

function openEdit(skill: ResourceSkill) {
  editingSkill.value = skill;
  editDialogOpen.value = true;
}

/** 导入结果外显:成功条数 + 逐条跳过原因;返回是否导入了至少一个技能 */
function announce(outcome: ResourceSkillImportOutcome): boolean {
  if (outcome.imported.length) {
    toast.success(
      t("settings.resources.skills.import.imported", { count: outcome.imported.length }),
    );
  }
  for (const item of outcome.skipped) {
    toast.warning(
      t(
        item.reason === "conflict"
          ? "settings.resources.skills.import.skipConflict"
          : "settings.resources.skills.import.skipInvalid",
        { name: item.name },
      ),
    );
  }
  return outcome.imported.length > 0;
}

/** 执行一次导入并刷新列表;错误已在 toast 外显,以布尔告知调用方成败 */
async function runImport(task: () => Promise<ResourceSkillImportOutcome>): Promise<boolean> {
  if (importing.value) {
    return false;
  }
  importing.value = true;
  try {
    const ok = announce(await task());
    await load();
    return ok;
  } catch (e) {
    toast.error(String(e));
    return false;
  } finally {
    importing.value = false;
  }
}

async function importArchive() {
  let selected: string | null = null;
  try {
    const result = await openDialog({
      multiple: false,
      filters: [{ name: "ZIP", extensions: ["zip"] }],
    });
    selected = typeof result === "string" ? result : null;
  } catch (e) {
    toast.error(String(e));
    return;
  }
  if (selected) {
    await runImport(() => importResourceSkillArchive(selected as string));
  }
}

async function importFolder() {
  let selected: string | null = null;
  try {
    const result = await openDialog({ directory: true, multiple: false });
    selected = typeof result === "string" ? result : null;
  } catch (e) {
    toast.error(String(e));
    return;
  }
  if (selected) {
    await runImport(() => importResourceSkillFolder(selected as string));
  }
}

async function confirmImportUrl() {
  const url = urlInput.value.trim();
  if (!url || urlSubmitting.value) {
    return;
  }
  if (!/^https?:\/\//i.test(url)) {
    toast.error(t("settings.resources.skills.import.urlInvalid"));
    return;
  }
  urlSubmitting.value = true;
  const ok = await runImport(() => importResourceSkillUrl(url));
  urlSubmitting.value = false;
  if (ok) {
    urlDialogOpen.value = false;
    urlInput.value = "";
  }
}

/** 在可见列表内移动一项;持久化时合并回全量顺序(隐藏项相对位置不变) */
async function moveSkill(skill: ResourceSkill, dir: -1 | 1) {
  const visibleIds = filtered.value.map((s) => s.id);
  const index = visibleIds.indexOf(skill.id);
  const target = index + dir;
  if (index === -1 || target < 0 || target >= visibleIds.length) {
    return;
  }
  const nextVisible = [...visibleIds];
  [nextVisible[index], nextVisible[target]] = [nextVisible[target], nextVisible[index]];
  const merged = mergeReorderedVisible(
    skills.value.map((s) => s.id),
    nextVisible,
  );
  // 按合并后的 id 顺序重建列表;理论上每个 id 都能命中,仍用过滤兜底避免 undefined 混入
  const byId = new Map(skills.value.map((s) => [s.id, s]));
  skills.value = merged
    .map((id) => byId.get(id))
    .filter((s): s is ResourceSkill => s !== undefined);
  try {
    await reorderResourceSkills(merged);
  } catch (e) {
    toast.error(t("settings.resources.skills.reorderFailed", { error: String(e) }));
    await load();
  }
}

async function openDir(skill: ResourceSkill) {
  try {
    await openResourceSkillDir(skill.id);
  } catch (e) {
    toast.error(String(e));
  }
}

async function confirmDelete() {
  const skill = pendingDelete.value;
  if (!skill) {
    return;
  }
  try {
    await deleteResourceSkill(skill.id);
    skills.value = skills.value.filter((s) => s.id !== skill.id);
    toast.success(t("settings.resources.skills.deleted"));
  } catch (e) {
    toast.error(String(e));
  } finally {
    pendingDelete.value = null;
  }
}
</script>

<template>
  <section>
    <div class="flex items-center gap-2">
      <div class="relative max-w-xs flex-1">
        <Search
          class="absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground"
        />
        <Input
          v-model="query"
          class="h-8 pl-8 text-xs"
          :placeholder="t('settings.resources.skills.searchPlaceholder')"
          spellcheck="false"
        />
      </div>
      <Button
        variant="outline"
        size="sm"
        class="h-8 shrink-0 gap-1.5"
        @click="groupsDialogOpen = true"
      >
        <Layers class="h-3.5 w-3.5" />
        {{ t("settings.resources.skills.manageGroups") }}
      </Button>
      <DropdownMenu>
        <DropdownMenuTrigger as-child>
          <Button size="sm" class="h-8 shrink-0 gap-1.5" :disabled="importing">
            <Download class="h-3.5 w-3.5" />
            {{ t("settings.resources.skills.import.trigger") }}
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuItem class="gap-2" @click="importArchive">
            <FileArchive class="h-3.5 w-3.5" />
            {{ t("settings.resources.skills.import.archive") }}
          </DropdownMenuItem>
          <DropdownMenuItem class="gap-2" @click="importFolder">
            <FolderOpen class="h-3.5 w-3.5" />
            {{ t("settings.resources.skills.import.folder") }}
          </DropdownMenuItem>
          <DropdownMenuItem class="gap-2" @click="urlDialogOpen = true">
            <Link2 class="h-3.5 w-3.5" />
            {{ t("settings.resources.skills.import.url") }}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>

    <div v-if="groups.length" class="mt-3 flex flex-wrap items-center gap-1.5">
      <button
        type="button"
        class="rounded-full border px-2.5 py-1 text-xs transition-colors"
        :class="
          activeGroupId === null
            ? 'border-foreground bg-foreground text-background'
            : 'text-muted-foreground hover:bg-accent hover:text-foreground'
        "
        @click="activeGroupId = null"
      >
        {{ t("settings.resources.skills.allGroups") }}
      </button>
      <button
        v-for="group in groups"
        :key="group.id"
        type="button"
        class="flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs transition-colors"
        :class="
          activeGroupId === group.id
            ? 'border-foreground bg-foreground text-background'
            : 'text-muted-foreground hover:bg-accent hover:text-foreground'
        "
        @click="activeGroupId = group.id"
      >
        <span
          class="h-2 w-2 rounded-full"
          :style="{ backgroundColor: group.color ?? 'var(--muted-foreground)' }"
        />
        {{ group.name }}
      </button>
    </div>

    <p v-if="loading" class="mt-6 text-center text-xs text-muted-foreground">
      {{ t("common.loading") }}
    </p>
    <div v-else-if="filtered.length" class="mt-4 grid grid-cols-1 gap-3 lg:grid-cols-2">
      <div v-for="(skill, index) in filtered" :key="skill.id" class="group rounded-lg border p-3">
        <div class="flex items-start justify-between gap-2">
          <p class="min-w-0 truncate text-sm font-medium" :title="skill.name">{{ skill.name }}</p>
          <span
            class="flex shrink-0 items-center opacity-0 transition-opacity group-hover:opacity-100 focus-within:opacity-100"
          >
            <Button
              variant="ghost"
              size="icon"
              class="h-7 w-7"
              :disabled="index === 0"
              :title="t('settings.resources.skills.up')"
              @click="moveSkill(skill, -1)"
            >
              <ChevronUp class="h-3.5 w-3.5" />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              class="h-7 w-7"
              :disabled="index === filtered.length - 1"
              :title="t('settings.resources.skills.down')"
              @click="moveSkill(skill, 1)"
            >
              <ChevronDown class="h-3.5 w-3.5" />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              class="h-7 w-7"
              :title="t('settings.resources.skills.edit')"
              @click="openEdit(skill)"
            >
              <Pencil class="h-3.5 w-3.5" />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              class="h-7 w-7 text-destructive"
              :title="t('settings.resources.skills.delete')"
              @click="pendingDelete = skill"
            >
              <Trash2 class="h-3.5 w-3.5" />
            </Button>
          </span>
        </div>
        <p v-if="skill.description" class="mt-1 line-clamp-2 text-xs text-muted-foreground">
          {{ skill.description }}
        </p>
        <div class="mt-2 flex items-center justify-between gap-2">
          <div class="flex min-w-0 flex-wrap gap-1.5">
            <span
              v-for="groupId in skill.groupIds"
              :key="groupId"
              class="flex items-center gap-1 rounded-full bg-muted px-2 py-0.5 text-[11px] text-muted-foreground"
            >
              <span
                class="h-1.5 w-1.5 rounded-full"
                :style="{
                  backgroundColor: groupMap.get(groupId)?.color ?? 'var(--muted-foreground)',
                }"
              />
              {{ groupMap.get(groupId)?.name ?? groupId }}
            </span>
          </div>
          <Button
            variant="ghost"
            size="sm"
            class="h-7 shrink-0 gap-1 px-2 text-xs text-muted-foreground"
            @click="openDir(skill)"
          >
            <FolderOpen class="h-3.5 w-3.5" />
            {{ t("settings.resources.skills.openDir") }}
          </Button>
        </div>
      </div>
    </div>
    <p
      v-else
      class="mt-6 rounded-md border border-dashed px-3 py-8 text-center text-xs text-muted-foreground"
    >
      {{
        query || activeGroupId
          ? t("settings.resources.skills.noMatch")
          : t("settings.resources.skills.empty")
      }}
    </p>

    <ResourceSkillEditDialog
      v-if="editingSkill"
      v-model:open="editDialogOpen"
      :groups="groups"
      :skill="editingSkill"
      @saved="load"
    />
    <ResourceSkillGroupsDialog v-model:open="groupsDialogOpen" :groups="groups" @changed="load" />
    <ConfirmDialog
      v-model:open="deleteConfirmOpen"
      :title="t('common.delete')"
      :description="t('settings.resources.skills.deleteConfirm', { name: pendingDelete?.name })"
      :confirm-text="t('common.delete')"
      destructive
      @confirm="confirmDelete"
    />

    <Dialog :open="urlDialogOpen" @update:open="urlDialogOpen = $event">
      <DialogContent class="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{{ t("settings.resources.skills.import.urlTitle") }}</DialogTitle>
          <DialogDescription>
            {{ t("settings.resources.skills.import.urlDescription") }}
          </DialogDescription>
        </DialogHeader>
        <Input
          v-model="urlInput"
          class="h-8 text-xs"
          :placeholder="t('settings.resources.skills.import.urlPlaceholder')"
          spellcheck="false"
          :disabled="urlSubmitting"
          @keydown.enter="confirmImportUrl"
        />
        <DialogFooter>
          <Button variant="outline" :disabled="urlSubmitting" @click="urlDialogOpen = false">
            {{ t("common.cancel") }}
          </Button>
          <Button :disabled="!urlInput.trim() || urlSubmitting" @click="confirmImportUrl">
            {{ t("settings.resources.skills.import.urlConfirm") }}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </section>
</template>
