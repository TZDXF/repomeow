<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import { toast } from "vue-sonner";
import { Icon } from "@iconify/vue";
import { Bot, Import, LoaderCircle, Package, Plug, Plus, RefreshCw, Trash2 } from "@lucide/vue";
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
import { useSettingsStore } from "@/stores/settings";
import {
  assignProjectResource,
  claimLocalProjectResource,
  importProjectResource,
  loadProjectResources,
  removeProjectResource,
  resourceTree,
  type ProjectAiTarget,
  type ProjectResourceKind,
  type ProjectResourceSnapshot,
  type ResourceChoice,
  type ResourceDeployment,
} from "@/lib/project-ai-resources";
import { joinPath } from "@/lib/path";
import type { ProjectAiAssets } from "@/types";
import ProjectResourceAddDialog from "./ProjectResourceAddDialog.vue";

const props = defineProps<{
  projectPath: string;
  kind: ProjectResourceKind;
  targets: ProjectAiTarget[];
  assets: ProjectAiAssets | null;
  revision: number;
  /** 技能预览页的返回路径(项目详情页路由) */
  from: string;
}>();
const emit = defineEmits<{ preview: [path: string]; changed: [] }>();
const { t } = useI18n();
const router = useRouter();
const settings = useSettingsStore();
const data = ref<ProjectResourceSnapshot | null>(null);
const loading = ref(false);
const error = ref("");
const addOpen = ref(false);
const removeTargets = ref<ResourceChoice[]>([]);
const removing = ref(false);
const importing = ref<string | null>(null);
const toggling = ref("");
const activeFilter = ref("");
/** 非托管检测用的 assets 快照:仅在 load() 提交 deployments 时同步更新,保证两侧数据成对一致,杜绝刷新期间的幽灵行。 */
const assetsSnapshot = ref<ProjectAiAssets | null>(null);
let sequence = 0;
onBeforeUnmount(() => sequence++);
/** 已有数据时的刷新走静默模式(不闪 loading),配合父组件协调好的时序,避免整列视觉闪动。 */
async function load() {
  const seq = ++sequence;
  const silent = !!data.value;
  if (!silent) {
    loading.value = true;
  }
  error.value = "";
  try {
    const next = await loadProjectResources(props.projectPath, props.kind);
    if (seq === sequence) {
      data.value = next;
      assetsSnapshot.value = props.assets;
    }
  } catch (e) {
    if (seq === sequence) {
      error.value = String(e);
      data.value = null;
    }
  } finally {
    if (!silent && seq === sequence) {
      loading.value = false;
    }
  }
}
watch(
  () => [props.projectPath, props.kind, props.revision],
  () => {
    void load();
  },
  { immediate: true },
);
watch(
  () => [props.projectPath, props.kind],
  () => {
    data.value = null;
    assetsSnapshot.value = null;
    addOpen.value = false;
    removeTargets.value = [];
    activeFilter.value = "";
    toggling.value = "";
  },
);
/** 项目资源列表 = 已添加(shortlist)∪ 已部署;来源被删除时回退部署记录里的名字。 */
const listed = computed(() => {
  const ids = new Set<string>(data.value?.shortlist ?? []);
  for (const d of data.value?.deployments ?? []) {
    ids.add(d.resourceId);
  }
  const resources: ResourceChoice[] = [];
  for (const id of ids) {
    const existing = data.value?.resources.find((r) => r.id === id);
    if (existing) {
      resources.push(existing);
      continue;
    }
    const record = data.value?.deployments.find((d) => d.resourceId === id);
    resources.push({
      id,
      name: record?.name ?? id,
      description: "",
      groupIds: [],
      supportedAgents: [],
    });
  }
  return resources;
});
/** 分组只作为筛选维度(skills):用户分组与市场来源(owner/repo)并列。 */
const tree = computed(() =>
  resourceTree(
    props.kind === "skills" ? (data.value?.groups ?? []) : [],
    listed.value,
    t("projectAi.ungrouped"),
  ).filter((g) => g.resources.length),
);
const filters = computed(() => (props.kind === "skills" ? tree.value : []));
const filtered = computed(() => {
  if (!activeFilter.value) {
    return listed.value;
  }
  return tree.value.find((g) => g.id === activeFilter.value)?.resources ?? [];
});
watch(filters, (next) => {
  if (activeFilter.value && !next.some((g) => g.id === activeFilter.value)) {
    activeFilter.value = "";
  }
});
const unmanagedSkills = computed(
  () =>
    assetsSnapshot.value?.skills.filter(
      (s) => !data.value?.deployments.some((d) => d.path === s.dir),
    ) ?? [],
);
const unmanagedMcp = computed(
  () =>
    assetsSnapshot.value?.mcp.flatMap((file) =>
      file.servers
        .filter(
          (s) => !data.value?.deployments.some((d) => d.path === file.path && d.name === s.name),
        )
        .map((s) => ({ name: s.name, path: file.path })),
    ) ?? [],
);
interface UnmanagedItem {
  key: string;
  name: string;
  /** 预览路径(skills 为 SKILL.md 路径,mcp 为配置文件路径)。 */
  path: string;
  /** 导入来源:skills = 技能目录;mcp = 配置文件路径。 */
  source: string;
}
const unmanaged = computed<UnmanagedItem[]>(() =>
  props.kind === "skills"
    ? unmanagedSkills.value.map((s) => ({
        key: `skill:${s.dir}`,
        name: s.name,
        path: joinPath(s.dir, "SKILL.md"),
        source: s.dir,
      }))
    : unmanagedMcp.value.map((s) => ({
        key: `mcp:${s.path}:${s.name}`,
        name: s.name,
        path: s.path,
        source: s.path,
      })),
);
function records(id: string) {
  return data.value?.deployments.filter((d) => d.resourceId === id) ?? [];
}
function recordOf(resourceId: string, agentId: string): ResourceDeployment | undefined {
  return data.value?.deployments.find((d) => d.resourceId === resourceId && d.agentId === agentId);
}
/** 可见 Agent:设置页未隐藏的目标 + 虽隐藏但已部署该资源的 Agent(允许解除配置)。 */
function visibleAgents(resource: ResourceChoice) {
  return props.targets.filter(
    (a) => !settings.hiddenResourceAgents.includes(a.id) || recordOf(resource.id, a.id),
  );
}
function supported(resource: ResourceChoice, agent: ProjectAiTarget) {
  return resource.supportedAgents.includes(agent.id);
}
function selectable(resource: ResourceChoice, agent: ProjectAiTarget) {
  return supported(resource, agent) || !!recordOf(resource.id, agent.id);
}
function chipClass(resource: ResourceChoice, agent: ProjectAiTarget) {
  const record = recordOf(resource.id, agent.id);
  if (record) {
    return record.status === "configured"
      ? "border-primary/40 bg-primary/10 text-primary"
      : "border-amber-500/40 bg-amber-500/10 text-amber-600 dark:text-amber-400";
  }
  return selectable(resource, agent)
    ? "text-muted-foreground hover:bg-accent hover:text-foreground"
    : "cursor-not-allowed opacity-40";
}
function chipTitle(resource: ResourceChoice, agent: ProjectAiTarget) {
  const record = recordOf(resource.id, agent.id);
  if (record) {
    return `${agent.name} · ${record.path} · ${t(`projectAi.states.${record.status}`)}`;
  }
  if (!supported(resource, agent)) {
    return `${agent.name} · ${t("projectAi.unsupported")}`;
  }
  return `${agent.name} · ${props.kind === "skills" ? agent.skillPath : agent.mcpPath}`;
}
/** 点击 Agent 标签即切换部署:新增勾选或解除该 Agent 的托管配置。 */
async function toggleAgent(resource: ResourceChoice, agent: ProjectAiTarget) {
  if (!data.value || toggling.value || !selectable(resource, agent)) {
    return;
  }
  const current = new Set(records(resource.id).map((d) => d.agentId));
  if (current.has(agent.id)) {
    current.delete(agent.id);
  } else {
    current.add(agent.id);
  }
  toggling.value = `${resource.id}:${agent.id}`;
  try {
    const result = await assignProjectResource({
      path: props.projectPath,
      kind: props.kind,
      resourceId: resource.id,
      agentIds: [...current],
      expectedRevision: data.value.revision,
    });
    if (result.failures.length) {
      toast.error(
        t("projectAi.partial", { count: result.applied, failed: result.failures.length }),
      );
    }
    changed();
  } catch (e) {
    toast.error(String(e));
  } finally {
    toggling.value = "";
  }
}
/** 技能预览页需要绝对目录:部署记录与资产扫描给出的都是项目相对路径,这里拼上项目根 */
function absoluteSkillDir(dir: string): string {
  // 已是绝对路径(盘符/UNC/POSIX 根)时原样返回,防御未来来源变化
  if (/^([A-Za-z]:[\\/]|\\\\|\/)/.test(dir)) {
    return dir;
  }
  return joinPath(props.projectPath, dir);
}
function preview(resource: ResourceChoice) {
  // skills 统一走技能预览页(库技能按 id,本地来源按目录);MCP 保持抽屉只读预览
  if (props.kind === "skills") {
    if (resource.id.startsWith("local:")) {
      const dir = records(resource.id)[0]?.path;
      if (dir) {
        void router.push({
          name: "resource-skill",
          params: { id: "local" },
          query: { dir: absoluteSkillDir(dir), from: props.from },
        });
      }
      return;
    }
    void router.push({
      name: "resource-skill",
      params: { id: resource.id },
      query: { from: props.from },
    });
    return;
  }
  const record = records(resource.id)[0];
  if (record) {
    emit("preview", record.path);
  }
}
/** 非托管项:skills 以本地目录模式打开技能预览页,MCP 保持抽屉只读预览 */
function previewUnmanaged(item: UnmanagedItem) {
  if (props.kind === "skills") {
    void router.push({
      name: "resource-skill",
      params: { id: "local" },
      query: { dir: absoluteSkillDir(item.source), from: props.from },
    });
    return;
  }
  emit("preview", item.path);
}
/** 非托管行的归属 Agent:skills 按目录前缀、mcp 按配置文件路径判定。 */
function unmanagedOwner(item: UnmanagedItem): string {
  const owner =
    props.kind === "skills"
      ? props.targets.find((a) => item.source.startsWith(`${a.skillPath}/`))
      : props.targets.find((a) => a.mcpPath === item.source);
  return owner?.id ?? "";
}
/** 非托管行可见 Agent:未隐藏目标 ∪ 归属 Agent(即使隐藏也展示其现状)。 */
function unmanagedChips(item: UnmanagedItem) {
  return props.targets.filter(
    (a) => !settings.hiddenResourceAgents.includes(a.id) || a.id === unmanagedOwner(item),
  );
}
function unmanagedChipTitle(item: UnmanagedItem, agent: ProjectAiTarget) {
  if (unmanagedOwner(item) === agent.id) {
    return `${agent.name} · ${item.source} · ${t("projectAi.states.configured")}`;
  }
  return `${agent.name} · ${props.kind === "skills" ? agent.skillPath : agent.mcpPath}`;
}
/**
 * 非托管资源直接配置 Agent:先认领为项目本地来源(记录来源、不入库,
 * 来源 Agent 按现状登记),再把目标集合设为「现状 ∪ 点击的 Agent」一次 assign。
 */
async function configureUnmanaged(item: UnmanagedItem, agent: ProjectAiTarget) {
  if (!data.value || toggling.value) {
    return;
  }
  toggling.value = `${item.key}:${agent.id}`;
  try {
    const outcome = await claimLocalProjectResource({
      path: props.projectPath,
      kind: props.kind,
      source: item.source,
      name: props.kind === "mcp" ? item.name : undefined,
      expectedRevision: data.value.revision,
    });
    const snapshot = await loadProjectResources(props.projectPath, props.kind);
    const current = new Set(
      snapshot.deployments.filter((d) => d.resourceId === outcome.resourceId).map((d) => d.agentId),
    );
    current.add(agent.id);
    const result = await assignProjectResource({
      path: props.projectPath,
      kind: props.kind,
      resourceId: outcome.resourceId,
      agentIds: [...current],
      expectedRevision: snapshot.revision,
    });
    if (result.failures.length) {
      toast.error(
        t("projectAi.partial", { count: result.applied, failed: result.failures.length }),
      );
    } else {
      toast.success(t("projectAi.applied", { count: result.applied }));
    }
    changed();
  } catch (e) {
    toast.error(String(e));
  } finally {
    toggling.value = "";
  }
}
async function importUnmanaged(item: UnmanagedItem) {
  if (!data.value || importing.value) {
    return;
  }
  importing.value = item.key;
  try {
    await importProjectResource({
      path: props.projectPath,
      kind: props.kind,
      source: item.source,
      name: props.kind === "mcp" ? item.name : undefined,
      expectedRevision: data.value.revision,
    });
    toast.success(t("projectAi.imported", { name: item.name }));
    changed();
  } catch (e) {
    toast.error(String(e));
  } finally {
    importing.value = null;
  }
}
/** 批量移除:逐项执行,每项执行前重取快照拿到最新 revision(上一次移除会使其失效)。 */
async function confirmRemove() {
  const targets = removeTargets.value;
  if (!targets.length || removing.value) {
    return;
  }
  removing.value = true;
  let removed = 0;
  let failed = 0;
  try {
    for (const target of targets) {
      try {
        const snapshot = await loadProjectResources(props.projectPath, props.kind);
        const result = await removeProjectResource({
          path: props.projectPath,
          kind: props.kind,
          resourceId: target.id,
          expectedRevision: snapshot.revision,
        });
        if (result.failures.length) {
          failed += result.failures.length;
        } else {
          removed++;
        }
      } catch (e) {
        failed++;
        toast.error(String(e));
      }
    }
    if (failed) {
      toast.error(t("projectAi.partial", { count: removed, failed }));
    } else if (targets.length === 1) {
      toast.success(t("projectAi.removed", { name: targets[0].name }));
    } else {
      toast.success(t("projectAi.removedCount", { count: removed }));
    }
    removeTargets.value = [];
    changed();
  } finally {
    removing.value = false;
  }
}
/** 变更后不自行刷新:通知父组件先重扫 assets,再由 revision 驱动本区静默刷新,避免托管/非托管数据错位导致的闪烁。 */
function changed() {
  emit("changed");
}
</script>

<template>
  <section class="min-w-0 space-y-3">
    <div class="flex flex-wrap items-center gap-2 border-b pb-3">
      <h3 class="mr-auto flex items-center gap-2 text-sm font-medium">
        <Package v-if="kind === 'skills'" class="size-4" /><Plug v-else class="size-4" />{{
          kind === "skills" ? "Skills" : "MCP"
        }}<span class="text-xs text-muted-foreground">{{ listed.length }}</span
        ><LoaderCircle v-if="loading" class="size-3 animate-spin" />
      </h3>
      <Button variant="outline" size="sm" class="h-7 px-2 text-xs" @click="addOpen = true"
        ><Plus class="size-3.5" />{{ t("projectAi.add") }}</Button
      >
      <Button
        variant="ghost"
        size="icon"
        class="size-7"
        :title="t('aiAssets.refresh')"
        :disabled="loading"
        @click="load"
        ><RefreshCw class="size-3.5"
      /></Button>
    </div>
    <p class="text-xs text-muted-foreground">{{ t("projectAi.sectionHint") }}</p>
    <p
      v-if="kind === 'mcp'"
      class="rounded-md border border-amber-500/30 bg-amber-500/5 p-2 text-xs text-amber-700 dark:text-amber-400"
    >
      {{ t("projectAi.secretWarning") }}
    </p>
    <div v-if="filters.length" class="flex flex-wrap gap-1.5">
      <button
        class="rounded-full border px-2.5 py-1 text-xs"
        :class="
          !activeFilter
            ? 'border-primary/40 bg-primary/10 text-primary'
            : 'text-muted-foreground hover:bg-accent hover:text-foreground'
        "
        @click="activeFilter = ''"
      >
        {{ t("projectAi.filterAll") }}
        <span class="text-muted-foreground">({{ listed.length }})</span>
      </button>
      <button
        v-for="group in filters"
        :key="group.id"
        class="rounded-full border px-2.5 py-1 text-xs"
        :class="
          activeFilter === group.id
            ? 'border-primary/40 bg-primary/10 text-primary'
            : 'text-muted-foreground hover:bg-accent hover:text-foreground'
        "
        @click="activeFilter = group.id"
      >
        {{ group.name }}
        <span class="text-muted-foreground">({{ group.resources.length }})</span>
      </button>
      <Button
        variant="outline"
        size="sm"
        class="ml-auto h-7 px-2 text-xs text-destructive hover:text-destructive"
        :class="{ invisible: !activeFilter || !filtered.length }"
        :disabled="!activeFilter || !filtered.length"
        :tabindex="activeFilter && filtered.length ? 0 : -1"
        @click="removeTargets = [...filtered]"
        ><Trash2 class="size-3.5" />{{
          t("projectAi.removeSelected", { count: filtered.length })
        }}</Button
      >
    </div>
    <p v-if="error" role="alert" class="text-xs text-destructive">{{ error }}</p>
    <p v-if="data?.sourceError" role="alert" class="text-xs text-amber-600 dark:text-amber-400">
      {{ t(`errors.${data.sourceError}`) }} · {{ t("projectAi.sourceUnavailableHint") }}
    </p>
    <p
      v-if="!loading && !error && !listed.length && !unmanaged.length"
      class="rounded-md border border-dashed p-5 text-center text-xs text-muted-foreground"
    >
      {{ t("projectAi.projectEmpty") }}
    </p>
    <p
      v-else-if="activeFilter && !filtered.length"
      class="rounded-md border border-dashed p-5 text-center text-xs text-muted-foreground"
    >
      {{ t("projectAi.noResults") }}
    </p>
    <template v-if="filtered.length || (!activeFilter && unmanaged.length)">
      <div class="divide-y rounded-md border">
        <div
          v-for="resource in filtered"
          :key="resource.id"
          class="flex flex-wrap items-center gap-2 px-3 py-2.5"
        >
          <button
            v-if="kind === 'skills' || records(resource.id).length"
            class="min-w-0 flex-1 text-left hover:text-primary"
            :title="t('projectAi.preview')"
            @click="preview(resource)"
          >
            <p class="truncate text-xs font-medium">{{ resource.name }}</p>
            <p v-if="resource.description" class="mt-1 truncate text-xs text-muted-foreground">
              {{ resource.description }}
            </p>
          </button>
          <div v-else class="min-w-0 flex-1">
            <p class="truncate text-xs font-medium">{{ resource.name }}</p>
            <p v-if="resource.description" class="mt-1 truncate text-xs text-muted-foreground">
              {{ resource.description }}
            </p>
          </div>
          <div class="flex flex-wrap gap-1">
            <button
              v-for="agent in visibleAgents(resource)"
              :key="agent.id"
              class="flex size-6 items-center justify-center rounded border"
              :class="chipClass(resource, agent)"
              :disabled="!selectable(resource, agent)"
              :title="chipTitle(resource, agent)"
              @click="toggleAgent(resource, agent)"
            >
              <Icon
                v-if="agentBrandIcon(agent.id)"
                :icon="agentBrandIcon(agent.id)!"
                class="size-3.5"
              />
              <Bot v-else class="size-3.5" />
            </button>
            <span
              v-if="!visibleAgents(resource).length"
              class="text-[10px] text-muted-foreground"
              >{{ t("projectAi.noAgents") }}</span
            >
          </div>
          <Button
            variant="ghost"
            size="icon"
            class="size-7 text-destructive hover:text-destructive"
            :title="t('projectAi.remove')"
            @click="removeTargets = [resource]"
            ><Trash2 class="size-3.5"
          /></Button>
        </div>
        <p
          v-if="!activeFilter && unmanaged.length"
          class="bg-muted/40 px-3 py-1.5 text-[10px] text-muted-foreground"
          :title="t('projectAi.unmanagedHint')"
        >
          {{ t("projectAi.unmanaged") }}
        </p>
        <div
          v-for="item in activeFilter ? [] : unmanaged"
          :key="item.key"
          class="flex flex-wrap items-center gap-2 px-3 py-2.5"
        >
          <button
            v-if="kind === 'skills'"
            class="min-w-0 flex-1 text-left hover:text-primary"
            :title="t('projectAi.preview')"
            @click="previewUnmanaged(item)"
          >
            <p class="truncate text-xs font-medium">
              {{ item.name
              }}<span
                class="ml-1.5 rounded bg-muted px-1.5 py-0.5 align-middle text-[10px] font-normal text-muted-foreground"
                :title="t('projectAi.unmanagedHint')"
                >{{ t("projectAi.unmanaged") }}</span
              >
            </p>
            <p class="mt-1 truncate font-mono text-[10px] text-muted-foreground">{{ item.path }}</p>
          </button>
          <div v-else class="min-w-0 flex-1">
            <p class="truncate text-xs font-medium">
              {{ item.name
              }}<span
                class="ml-1.5 rounded bg-muted px-1.5 py-0.5 align-middle text-[10px] font-normal text-muted-foreground"
                :title="t('projectAi.unmanagedHint')"
                >{{ t("projectAi.unmanaged") }}</span
              >
            </p>
            <p class="mt-1 truncate font-mono text-[10px] text-muted-foreground">{{ item.path }}</p>
          </div>
          <div class="flex flex-wrap gap-1">
            <button
              v-for="agent in unmanagedChips(item)"
              :key="agent.id"
              class="flex size-6 items-center justify-center rounded border"
              :class="
                unmanagedOwner(item) === agent.id
                  ? 'border-primary/40 bg-primary/10 text-primary'
                  : 'text-muted-foreground hover:bg-accent hover:text-foreground'
              "
              :disabled="!!toggling || unmanagedOwner(item) === agent.id"
              :title="unmanagedChipTitle(item, agent)"
              @click="configureUnmanaged(item, agent)"
            >
              <LoaderCircle
                v-if="toggling === `${item.key}:${agent.id}`"
                class="size-3.5 animate-spin"
              />
              <Icon
                v-else-if="agentBrandIcon(agent.id)"
                :icon="agentBrandIcon(agent.id)!"
                class="size-3.5"
              />
              <Bot v-else class="size-3.5" />
            </button>
          </div>
          <Button
            variant="outline"
            size="sm"
            class="h-7 px-2 text-xs"
            :disabled="importing === item.key"
            :title="t('projectAi.importHint')"
            @click="importUnmanaged(item)"
            ><LoaderCircle v-if="importing === item.key" class="size-3.5 animate-spin" /><Import
              v-else
              class="size-3.5"
            />{{ t("projectAi.import") }}</Button
          >
        </div>
      </div>
    </template>
    <ProjectResourceAddDialog
      v-model:open="addOpen"
      :project-path="projectPath"
      :kind="kind"
      @changed="changed"
    />
    <Dialog
      :open="!!removeTargets.length"
      @update:open="
        (value) => {
          if (!removing && !value) removeTargets = [];
        }
      "
    >
      <DialogContent class="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{{
            removeTargets.length > 1
              ? t("projectAi.removeSelectedTitle")
              : t("projectAi.removeTitle")
          }}</DialogTitle>
          <DialogDescription>{{
            removeTargets.length > 1
              ? t("projectAi.removeSelectedHint", { count: removeTargets.length })
              : t("projectAi.removeHint", { name: removeTargets[0]?.name })
          }}</DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button variant="ghost" :disabled="removing" @click="removeTargets = []">{{
            t("common.cancel")
          }}</Button>
          <Button variant="destructive" :disabled="removing" @click="confirmRemove"
            ><LoaderCircle v-if="removing" class="size-4 animate-spin" />{{
              t("projectAi.remove")
            }}</Button
          >
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </section>
</template>
