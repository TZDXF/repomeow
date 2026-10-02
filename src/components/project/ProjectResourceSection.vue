<script setup lang="ts">
import { ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import { LoaderCircle, Package, Plug, Plus, Trash2 } from "@lucide/vue";
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
  type ProjectAiTarget,
  type ProjectResourceKind,
  type ResourceChoice,
} from "@/lib/project-ai-resources";
import { joinPath } from "@/lib/path";
import type { ProjectAiAssets } from "@/types";
import {
  useProjectResourceData,
  type UnmanagedItem,
} from "@/composables/project-resources/use-project-resource-data";
import { useResourceActions } from "@/composables/project-resources/use-resource-actions";
import { useResourceRepair } from "@/composables/project-resources/use-resource-repair";
import ManagedResourceRow from "./resource-section/ManagedResourceRow.vue";
import UnmanagedResourceRow from "./resource-section/UnmanagedResourceRow.vue";
import RepairDialog from "./resource-section/RepairDialog.vue";
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

// ── 数据层 / 动作层 / 修复层(实现见 composables/project-resources/*) ──
const {
  data,
  loading,
  error,
  activeFilter,
  listed,
  unmanaged,
  filters,
  filtered,
  filteredUnmanaged,
  groupCount,
  records,
} = useProjectResourceData({
  projectPath: () => props.projectPath,
  kind: () => props.kind,
  revision: () => props.revision,
  targets: () => props.targets,
  assets: () => props.assets,
});
const {
  removeTargets,
  removing,
  unmanagedRemoveTarget,
  removingUnmanaged,
  importing,
  toggling,
  toggleAgent,
  configureUnmanaged,
  importLocal,
  importUnmanaged,
  confirmRemove,
  confirmRemoveUnmanaged,
} = useResourceActions({
  projectPath: () => props.projectPath,
  kind: () => props.kind,
  targets: () => props.targets,
  data,
  records,
  changed,
});
const { repairResource, repairing, repairRecords, repairAgentName, applyUpdate, repairRecord } =
  useResourceRepair({
    projectPath: () => props.projectPath,
    kind: () => props.kind,
    targets: () => props.targets,
    data,
    records,
    changed,
  });

const addOpen = ref(false);
watch(
  () => [props.projectPath, props.kind],
  () => {
    addOpen.value = false;
  },
);

/** 变更后不自行刷新:通知父组件先重扫 assets,再由 revision 驱动本区静默刷新,避免托管/非托管数据错位导致的闪烁。 */
function changed() {
  emit("changed");
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
      // 同名多副本时以主来源(id 内嵌的认领目录)为准,而非首个部署记录。
      const dir = resource.id.slice("local:skills:".length) || records(resource.id)[0]?.path;
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
</script>

<template>
  <section class="min-w-0 space-y-3">
    <div class="flex flex-wrap items-center gap-2 border-b pb-3">
      <h3 class="mr-auto flex items-center gap-2 text-sm font-medium">
        <Package v-if="kind === 'skills'" class="size-4" /><Plug v-else class="size-4" />{{
          kind === "skills" ? "Skills" : "MCP"
        }}<span class="text-xs text-muted-foreground">{{ listed.length + unmanaged.length }}</span
        ><LoaderCircle v-if="loading" class="size-3 animate-spin" />
      </h3>
      <Button variant="outline" size="sm" class="h-7 px-2 text-xs" @click="addOpen = true"
        ><Plus class="size-3.5" />{{ t("projectAi.add") }}</Button
      >
    </div>
    <p class="text-xs text-muted-foreground">{{ t("projectAi.sectionHint") }}</p>
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
        <span class="text-muted-foreground">({{ listed.length + unmanaged.length }})</span>
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
        <span class="text-muted-foreground">({{ groupCount(group) }})</span>
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
      v-else-if="activeFilter && !filtered.length && !filteredUnmanaged.length"
      class="rounded-md border border-dashed p-5 text-center text-xs text-muted-foreground"
    >
      {{ t("projectAi.noResults") }}
    </p>
    <template v-if="filtered.length || filteredUnmanaged.length">
      <div class="divide-y rounded-md border">
        <ManagedResourceRow
          v-for="resource in filtered"
          :key="resource.id"
          :resource="resource"
          :kind="kind"
          :targets="targets"
          :records="records(resource.id)"
          :hidden-agents="settings.hiddenResourceAgents"
          :importing="importing"
          @preview="preview(resource)"
          @toggle-agent="(agent) => toggleAgent(resource, agent)"
          @repair="repairResource = resource"
          @import-local="importLocal(resource)"
          @remove="removeTargets = [resource]"
        />
        <UnmanagedResourceRow
          v-for="item in filteredUnmanaged"
          :key="item.key"
          :item="item"
          :kind="kind"
          :targets="targets"
          :hidden-agents="settings.hiddenResourceAgents"
          :toggling="toggling"
          :importing="importing"
          @preview="previewUnmanaged(item)"
          @configure="(agent) => configureUnmanaged(item, agent)"
          @import="importUnmanaged(item)"
          @remove="unmanagedRemoveTarget = item"
        />
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
    <Dialog
      :open="!!unmanagedRemoveTarget"
      @update:open="
        (value) => {
          if (!removingUnmanaged && !value) unmanagedRemoveTarget = null;
        }
      "
    >
      <DialogContent class="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{{ t("projectAi.removeTitle") }}</DialogTitle>
          <DialogDescription>{{
            t("projectAi.unmanagedRemoveHint", { name: unmanagedRemoveTarget?.name })
          }}</DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button
            variant="ghost"
            :disabled="removingUnmanaged"
            @click="unmanagedRemoveTarget = null"
            >{{ t("common.cancel") }}</Button
          >
          <Button
            variant="destructive"
            :disabled="removingUnmanaged"
            @click="confirmRemoveUnmanaged"
            ><LoaderCircle v-if="removingUnmanaged" class="size-4 animate-spin" />{{
              t("projectAi.remove")
            }}</Button
          >
        </DialogFooter>
      </DialogContent>
    </Dialog>
    <RepairDialog
      :resource="repairResource"
      :records="repairRecords"
      :repairing="repairing"
      :agent-name="repairAgentName"
      @close="repairResource = null"
      @apply-update="applyUpdate"
      @repair="repairRecord"
    />
  </section>
</template>
