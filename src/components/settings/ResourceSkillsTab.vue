<script setup lang="ts">
import { computed, onActivated, ref } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import { toast } from "vue-sonner";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  ArrowUpCircle,
  Download,
  ExternalLink,
  FileArchive,
  FolderOpen,
  Layers,
  Link2,
  RefreshCw,
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
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  checkResourceMarketplaceUpdates,
  collectSkillSources,
  deleteResourceSkill,
  filterSkills,
  importResourceSkillArchive,
  importResourceSkillFolder,
  importResourceSkillUrl,
  listResourceSkills,
  updateResourceMarketplaceSkill,
  type ResourceMarketplaceUpdateStatus,
  type ResourceSkill,
  type ResourceSkillGroup,
  type ResourceSkillImportOutcome,
} from "@/lib/resource-library";
import ResourceSkillGroupsDialog from "./ResourceSkillGroupsDialog.vue";

const { t, te } = useI18n();
const router = useRouter();

const loading = ref(true);
const groups = ref<ResourceSkillGroup[]>([]);
const skills = ref<ResourceSkill[]>([]);
const query = ref("");
const activeGroupId = ref<string | null>(null);
// 「来源」特殊分组:按市场技能的 marketplace.source 自动派生,与分组互斥选中
const activeSourceId = ref<string | null>(null);

const groupsDialogOpen = ref(false);
const pendingDelete = ref<ResourceSkill | null>(null);

const importing = ref(false);
const urlDialogOpen = ref(false);
const urlInput = ref("");
const urlSubmitting = ref(false);

// 市场技能更新状态:skillId → true=有更新 / false=已最新 / null=无法判断
const checkingUpdates = ref(false);
const updatingIds = ref<string[]>([]);
const updateStatus = ref(new Map<string, boolean | null>());

const marketplaceSkillCount = computed(
  () => skills.value.filter((skill) => skill.marketplace).length,
);
const updatableCount = computed(
  () => skills.value.filter((skill) => updateStatus.value.get(skill.id) === true).length,
);
const updatingAll = ref(false);

function isUpdating(skillId: string): boolean {
  return updatingIds.value.includes(skillId);
}

/** 检查结果错误码 → 文案;沿用 errors.<code> i18n 通道 */
function statusMessage(status: ResourceMarketplaceUpdateStatus): string {
  if (status.errorCode && te(`errors.${status.errorCode}`)) {
    return t(`errors.${status.errorCode}`);
  }
  return t("settings.resources.skills.updateCheckFailed");
}

async function checkUpdates() {
  if (checkingUpdates.value || marketplaceSkillCount.value === 0) {
    return;
  }
  checkingUpdates.value = true;
  try {
    const statuses = await checkResourceMarketplaceUpdates();
    const next = new Map<string, boolean | null>();
    const failed: string[] = [];
    let available = 0;
    for (const status of statuses) {
      next.set(status.skillId, status.updateAvailable);
      if (status.updateAvailable) {
        available += 1;
      }
      if (status.updateAvailable === null) {
        failed.push(statusMessage(status));
      }
    }
    updateStatus.value = next;
    if (available > 0) {
      toast.success(t("settings.resources.skills.updateFound", { count: available }));
    } else if (failed.length) {
      toast.warning(failed[0]);
    }
  } catch (e) {
    toast.error(String(e));
  } finally {
    checkingUpdates.value = false;
  }
}

/** 更新单个市场技能;失败经 toast 外显并以布尔告知调用方 */
async function updateOne(skill: ResourceSkill): Promise<boolean> {
  updatingIds.value = [...updatingIds.value, skill.id];
  try {
    await updateResourceMarketplaceSkill(skill.id);
    updateStatus.value = new Map(updateStatus.value).set(skill.id, false);
    return true;
  } catch (e) {
    toast.error(
      t("settings.resources.skills.updateFailed", { name: skill.name, error: String(e) }),
    );
    return false;
  } finally {
    updatingIds.value = updatingIds.value.filter((id) => id !== skill.id);
  }
}

async function applyUpdate(skill: ResourceSkill) {
  if (isUpdating(skill.id)) {
    return;
  }
  if (await updateOne(skill)) {
    toast.success(t("settings.resources.skills.updateDone", { name: skill.name }));
    await load();
  }
}

/** 全部更新:逐个拉取有可用更新的市场技能,结束后统一刷新列表 */
async function applyAllUpdates() {
  if (updatingAll.value || updatableCount.value === 0) {
    return;
  }
  updatingAll.value = true;
  try {
    const targets = skills.value.filter((skill) => updateStatus.value.get(skill.id) === true);
    let done = 0;
    for (const skill of targets) {
      if (await updateOne(skill)) {
        done += 1;
      }
    }
    if (done > 0) {
      toast.success(t("settings.resources.skills.updateAllDone", { count: done }));
    }
    await load();
  } finally {
    updatingAll.value = false;
  }
}

function openSourcePage(skill: ResourceSkill) {
  if (skill.marketplace?.url) {
    openUrl(skill.marketplace.url).catch((e) => toast.error(String(e)));
  }
}

const filtered = computed(() =>
  filterSkills(skills.value, query.value, activeGroupId.value, activeSourceId.value),
);
const groupMap = computed(() => new Map(groups.value.map((g) => [g.id, g])));
const sources = computed(() => collectSkillSources(skills.value));
const filterPickerOpen = ref(false);
const filterPickerQuery = ref("");
const filterOptions = computed(() => [
  ...groups.value.map((group) => ({
    key: `group:${group.id}`,
    id: group.id,
    name: group.name,
    kind: "group" as const,
    color: group.color,
    title: group.description || group.name,
  })),
  ...sources.value.map((source) => ({
    key: `source:${source}`,
    id: source,
    name: source,
    kind: "source" as const,
    color: null,
    title: source,
  })),
]);
const visibleFilters = computed(() => filterOptions.value.slice(0, 4));
const searchedFilters = computed(() => {
  const normalized = filterPickerQuery.value.trim().toLowerCase();
  return filterOptions.value.filter((option) => option.name.toLowerCase().includes(normalized));
});

function selectFilter(option: (typeof filterOptions.value)[number]) {
  if (option.kind === "group") selectGroup(option.id);
  else selectSource(option.id);
  filterPickerOpen.value = false;
  filterPickerQuery.value = "";
}

/** 分组与来源两个筛选维度互斥:选中任一分组/来源即清空另一维度 */
function selectGroup(id: string | null) {
  activeGroupId.value = id;
  activeSourceId.value = null;
}

function selectSource(id: string) {
  activeSourceId.value = id;
  activeGroupId.value = null;
}
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
    // 当前选中的来源已无对应市场技能(被删除/更新掉来源)时归位到「全部」
    if (
      activeSourceId.value &&
      !skills.value.some((s) => s.marketplace?.source === activeSourceId.value)
    ) {
      activeSourceId.value = null;
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

/** 卡片点击 → 独立技能预览页(token 占用 / 翻译 / 安全扫描) */
function openPreview(skill: ResourceSkill) {
  void router.push({ name: "resource-skill", params: { id: skill.id } });
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
        v-if="marketplaceSkillCount > 0"
        variant="outline"
        size="sm"
        class="h-8 shrink-0 gap-1.5"
        :disabled="checkingUpdates"
        :title="t('settings.resources.skills.checkUpdatesHint', { count: marketplaceSkillCount })"
        @click="checkUpdates"
      >
        <RefreshCw class="h-3.5 w-3.5" :class="{ 'animate-spin': checkingUpdates }" />
        {{
          t(
            checkingUpdates
              ? "settings.resources.skills.checkingUpdates"
              : "settings.resources.skills.checkUpdates",
          )
        }}
      </Button>
      <Button
        v-if="updatableCount > 0"
        variant="outline"
        size="sm"
        class="h-8 shrink-0 gap-1.5 border-amber-500/50 text-amber-600 dark:text-amber-400"
        :disabled="updatingAll"
        :title="t('settings.resources.skills.updateAllHint')"
        @click="applyAllUpdates"
      >
        <ArrowUpCircle class="h-3.5 w-3.5" :class="{ 'animate-spin': updatingAll }" />
        {{
          t(
            updatingAll
              ? "settings.resources.skills.updating"
              : "settings.resources.skills.updateAll",
            { count: updatableCount },
          )
        }}
      </Button>
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
        <DropdownMenuContent align="end" class="w-auto">
          <DropdownMenuItem class="gap-2 whitespace-nowrap" @click="importArchive">
            <FileArchive class="h-3.5 w-3.5" />
            {{ t("settings.resources.skills.import.archive") }}
          </DropdownMenuItem>
          <DropdownMenuItem class="gap-2 whitespace-nowrap" @click="importFolder">
            <FolderOpen class="h-3.5 w-3.5" />
            {{ t("settings.resources.skills.import.folder") }}
          </DropdownMenuItem>
          <DropdownMenuItem class="gap-2 whitespace-nowrap" @click="urlDialogOpen = true">
            <Link2 class="h-3.5 w-3.5" />
            {{ t("settings.resources.skills.import.url") }}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>

    <div v-if="filterOptions.length" class="mt-3 flex min-w-0 items-center gap-1.5">
      <div class="flex min-w-0 items-center gap-1.5 overflow-hidden">
        <button
          type="button"
          class="shrink-0 rounded-full border px-2.5 py-1 text-xs transition-colors"
          :class="
            activeGroupId === null && activeSourceId === null
              ? 'border-foreground bg-foreground text-background'
              : 'text-muted-foreground hover:bg-accent hover:text-foreground'
          "
          @click="selectGroup(null)"
        >
          {{ t("settings.resources.skills.allGroups") }}
        </button>
        <button
          v-for="option in visibleFilters"
          :key="option.key"
          type="button"
          class="flex max-w-40 shrink-0 items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs transition-colors"
          :class="
            (option.kind === 'group' ? activeGroupId : activeSourceId) === option.id
              ? 'border-foreground bg-foreground text-background'
              : 'text-muted-foreground hover:bg-accent hover:text-foreground'
          "
          :title="option.title"
          @click="selectFilter(option)"
        >
          <ExternalLink v-if="option.kind === 'source'" class="h-3 w-3 shrink-0" />
          <span
            v-else
            class="h-2 w-2 shrink-0 rounded-full"
            :style="{ backgroundColor: option.color ?? 'var(--muted-foreground)' }"
          />
          <span class="truncate">{{ option.name }}</span>
        </button>
      </div>
      <Popover v-model:open="filterPickerOpen">
        <PopoverTrigger as-child>
          <Button variant="outline" size="sm" class="h-7 shrink-0 gap-1 rounded-full px-2 text-xs">
            <Search class="h-3 w-3" />
            {{ t("settings.resources.skills.searchFilters") }}
          </Button>
        </PopoverTrigger>
        <PopoverContent class="w-72 p-2" align="start" @open-auto-focus.prevent>
          <div class="relative">
            <Search
              class="absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground"
            />
            <Input
              v-model="filterPickerQuery"
              class="h-8 pl-8 text-xs"
              :placeholder="t('settings.resources.skills.searchFilters')"
              spellcheck="false"
            />
          </div>
          <ScrollArea class="mt-1 max-h-56">
            <button
              v-for="option in searchedFilters"
              :key="option.key"
              type="button"
              class="hover:bg-accent flex w-full items-center gap-1.5 rounded-sm px-2 py-1.5 text-left text-xs"
              :class="
                (option.kind === 'group' ? activeGroupId : activeSourceId) === option.id
                  ? 'bg-accent text-foreground'
                  : ''
              "
              :title="option.title"
              @click="selectFilter(option)"
            >
              <ExternalLink v-if="option.kind === 'source'" class="h-3 w-3 shrink-0" />
              <span
                v-else
                class="h-2 w-2 shrink-0 rounded-full"
                :style="{ backgroundColor: option.color ?? 'var(--muted-foreground)' }"
              />
              <span class="truncate">{{ option.name }}</span>
            </button>
            <p v-if="!searchedFilters.length" class="px-2 py-2 text-xs text-muted-foreground">
              {{ t("settings.resources.skills.noMatchingFilters") }}
            </p>
          </ScrollArea>
        </PopoverContent>
      </Popover>
    </div>

    <p v-if="loading" class="mt-6 text-center text-xs text-muted-foreground">
      {{ t("common.loading") }}
    </p>
    <div v-else-if="filtered.length" class="mt-4 grid grid-cols-1 gap-3 lg:grid-cols-2">
      <div
        v-for="skill in filtered"
        :key="skill.id"
        class="group cursor-pointer rounded-lg border p-3 transition-colors hover:border-foreground/40"
        :title="t('settings.resources.skills.preview')"
        @click="openPreview(skill)"
      >
        <div class="flex items-start justify-between gap-2">
          <p class="min-w-0 truncate text-sm font-medium" :title="skill.name">{{ skill.name }}</p>
          <span
            class="flex shrink-0 items-center opacity-0 transition-opacity group-hover:opacity-100 focus-within:opacity-100"
          >
            <Button
              variant="ghost"
              size="icon"
              class="h-7 w-7 text-destructive"
              :title="t('settings.resources.skills.delete')"
              @click.stop="pendingDelete = skill"
            >
              <Trash2 class="h-3.5 w-3.5" />
            </Button>
          </span>
        </div>
        <p v-if="skill.description" class="mt-1 line-clamp-2 text-xs text-muted-foreground">
          {{ skill.description }}
        </p>
        <div class="mt-2 flex items-center justify-between gap-2">
          <div class="flex min-w-0 flex-wrap items-center gap-1.5">
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
            <button
              v-if="skill.marketplace"
              type="button"
              class="flex items-center gap-1 rounded-full bg-muted px-2 py-0.5 text-[11px] text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
              :title="
                t('settings.resources.skills.openSource', { source: skill.marketplace.source })
              "
              @click.stop="openSourcePage(skill)"
            >
              <ExternalLink class="h-3 w-3" />
              {{ skill.marketplace.source }}
            </button>
          </div>
          <div v-if="updateStatus.get(skill.id) === true" class="flex shrink-0 items-center gap-1">
            <Button
              variant="outline"
              size="sm"
              class="h-7 gap-1 border-amber-500/50 px-2 text-xs text-amber-600 dark:text-amber-400"
              :disabled="isUpdating(skill.id)"
              :title="t('settings.resources.skills.updateAvailableHint')"
              @click.stop="applyUpdate(skill)"
            >
              <ArrowUpCircle class="h-3.5 w-3.5" />
              {{
                t(
                  isUpdating(skill.id)
                    ? "settings.resources.skills.updating"
                    : "settings.resources.skills.update",
                )
              }}
            </Button>
          </div>
        </div>
      </div>
    </div>
    <p
      v-else
      class="mt-6 rounded-md border border-dashed px-3 py-8 text-center text-xs text-muted-foreground"
    >
      {{
        query || activeGroupId || activeSourceId
          ? t("settings.resources.skills.noMatch")
          : t("settings.resources.skills.empty")
      }}
    </p>

    <ResourceSkillGroupsDialog
      v-model:open="groupsDialogOpen"
      :groups="groups"
      :skills="skills"
      @changed="load"
    />
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
