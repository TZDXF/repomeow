<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { Icon } from "@iconify/vue";
import { Bot, Import, LoaderCircle, Trash2, Wrench } from "@lucide/vue";
import { agentBrandIcon } from "@/lib/agent-icons";
import { Button } from "@/components/ui/button";
import {
  type ProjectAiTarget,
  type ProjectResourceKind,
  type ResourceChoice,
  type ResourceDeployment,
} from "@/lib/project-ai-resources";
import {
  findDeployment,
  localImportTargetOf,
  resourceSelectable,
  resourceSupported,
  visibleResourceAgents,
} from "@/composables/project-resources/use-project-resource-data";
import { repairableRecordsOf } from "@/composables/project-resources/use-resource-repair";

const props = defineProps<{
  resource: ResourceChoice;
  kind: ProjectResourceKind;
  targets: ProjectAiTarget[];
  /** 该资源的部署记录(父层已按 resourceId 过滤) */
  records: ResourceDeployment[];
  hiddenAgents: string[];
  /** 导入防重入锁:当前导入中的资源 id(行内导入按钮据此转圈) */
  importing: string | null;
}>();
const emit = defineEmits<{
  preview: [];
  toggleAgent: [agent: ProjectAiTarget];
  repair: [];
  importLocal: [];
  remove: [];
}>();
const { t } = useI18n();

const agents = computed(() =>
  visibleResourceAgents(props.targets, props.kind, props.records, props.hiddenAgents),
);
const repairable = computed(() => repairableRecordsOf(props.records));
const importTarget = computed(() => localImportTargetOf(props.resource, props.records, props.kind));

function chipClass(agent: ProjectAiTarget) {
  const record = findDeployment(props.records, agent.id);
  if (record) {
    return record.status === "configured"
      ? "border-primary/40 bg-primary/10 text-primary"
      : "border-amber-500/40 bg-amber-500/10 text-amber-600 dark:text-amber-400";
  }
  return resourceSelectable(props.resource, agent, props.records)
    ? "text-muted-foreground hover:bg-accent hover:text-foreground"
    : "cursor-not-allowed opacity-40";
}
function chipTitle(agent: ProjectAiTarget) {
  const record = findDeployment(props.records, agent.id);
  if (record) {
    return `${agent.name} · ${record.path} · ${t(`projectAi.states.${record.status}`)}`;
  }
  if (!resourceSupported(props.resource, agent)) {
    return `${agent.name} · ${t("projectAi.unsupported")}`;
  }
  return `${agent.name} · ${props.kind === "skills" ? agent.skillPath : agent.mcpPath}`;
}
</script>

<template>
  <div class="flex flex-wrap items-center gap-2 px-3 py-2.5">
    <button
      v-if="kind === 'skills' || records.length"
      class="min-w-0 flex-1 text-left hover:text-primary"
      :title="t('projectAi.preview')"
      @click="emit('preview')"
    >
      <p class="truncate text-xs font-medium">{{ resource.name }}</p>
      <p v-if="resource.description" class="mt-1 truncate text-xs text-muted-foreground">
        {{ resource.description }}
      </p>
      <p
        v-if="resource.sourceDirs && resource.sourceDirs.length > 1"
        class="mt-1 truncate font-mono text-[10px] text-muted-foreground"
        :title="resource.sourceDirs.join('\n')"
      >
        {{ resource.sourceDirs.join(" · ") }}
      </p>
    </button>
    <div v-else class="min-w-0 flex-1">
      <p class="truncate text-xs font-medium">{{ resource.name }}</p>
      <p v-if="resource.description" class="mt-1 truncate text-xs text-muted-foreground">
        {{ resource.description }}
      </p>
      <p
        v-if="resource.sourceDirs && resource.sourceDirs.length > 1"
        class="mt-1 truncate font-mono text-[10px] text-muted-foreground"
        :title="resource.sourceDirs.join('\n')"
      >
        {{ resource.sourceDirs.join(" · ") }}
      </p>
    </div>
    <div class="flex flex-wrap gap-1">
      <button
        v-for="agent in agents"
        :key="agent.id"
        class="flex size-6 items-center justify-center rounded border"
        :class="chipClass(agent)"
        :disabled="!resourceSelectable(resource, agent, records)"
        :title="chipTitle(agent)"
        @click="emit('toggleAgent', agent)"
      >
        <Icon v-if="agentBrandIcon(agent.id)" :icon="agentBrandIcon(agent.id)!" class="size-3.5" />
        <Bot v-else class="size-3.5" />
      </button>
      <span v-if="!agents.length" class="text-[10px] text-muted-foreground">{{
        t("projectAi.noAgents")
      }}</span>
    </div>
    <Button
      v-if="repairable.length"
      variant="outline"
      size="sm"
      class="h-7 px-2 text-xs text-amber-600 hover:text-amber-600 dark:text-amber-400 dark:hover:text-amber-400"
      :title="t('projectAi.repairHint')"
      @click="emit('repair')"
      ><Wrench class="size-3.5" />{{ t("projectAi.repair") }}</Button
    >
    <Button
      v-if="importTarget"
      variant="outline"
      size="sm"
      class="h-7 px-2 text-xs"
      :disabled="importing === resource.id"
      :title="t('projectAi.importHint')"
      @click="emit('importLocal')"
      ><LoaderCircle v-if="importing === resource.id" class="size-3.5 animate-spin" /><Import
        v-else
        class="size-3.5"
      />{{ t("projectAi.import") }}</Button
    >
    <Button
      variant="ghost"
      size="icon"
      class="size-7 text-destructive hover:text-destructive"
      :title="t('projectAi.remove')"
      @click="emit('remove')"
      ><Trash2 class="size-3.5"
    /></Button>
  </div>
</template>
