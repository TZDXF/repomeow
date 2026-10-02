<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { Icon } from "@iconify/vue";
import { Bot, Import, LoaderCircle, Trash2 } from "@lucide/vue";
import { agentBrandIcon } from "@/lib/agent-icons";
import { Button } from "@/components/ui/button";
import { type ProjectAiTarget, type ProjectResourceKind } from "@/lib/project-ai-resources";
import {
  unmanagedOwnerSource,
  visibleUnmanagedAgents,
  type UnmanagedItem,
} from "@/composables/project-resources/use-project-resource-data";

const props = defineProps<{
  item: UnmanagedItem;
  kind: ProjectResourceKind;
  targets: ProjectAiTarget[];
  hiddenAgents: string[];
  /** 分配防重入锁:`${item.key}:${agent.id}` 命中时该 chip 转圈 */
  toggling: string;
  /** 导入防重入锁:当前导入中的条目 key */
  importing: string | null;
}>();
const emit = defineEmits<{
  preview: [];
  configure: [agent: ProjectAiTarget];
  import: [];
  remove: [];
}>();
const { t } = useI18n();

const agents = computed(() =>
  visibleUnmanagedAgents(props.targets, props.kind, props.hiddenAgents),
);

function ownerSource(agentId: string) {
  return unmanagedOwnerSource(props.item, agentId, props.targets, props.kind);
}
function chipTitle(agent: ProjectAiTarget) {
  const owned = ownerSource(agent.id);
  if (owned) {
    return `${agent.name} · ${owned} · ${t("projectAi.states.configured")}`;
  }
  return `${agent.name} · ${props.kind === "skills" ? agent.skillPath : agent.mcpPath}`;
}
</script>

<template>
  <div class="flex flex-wrap items-center gap-2 px-3 py-2.5">
    <button
      v-if="kind === 'skills'"
      class="min-w-0 flex-1 text-left hover:text-primary"
      :title="t('projectAi.preview')"
      @click="emit('preview')"
    >
      <p class="truncate text-xs font-medium">{{ item.name }}</p>
      <p v-if="item.description" class="mt-1 truncate text-xs text-muted-foreground">
        {{ item.description }}
      </p>
      <p v-else class="mt-1 truncate font-mono text-[10px] text-muted-foreground">
        {{ item.path }}
      </p>
    </button>
    <div v-else class="min-w-0 flex-1">
      <p class="truncate text-xs font-medium">{{ item.name }}</p>
      <p class="mt-1 truncate font-mono text-[10px] text-muted-foreground">{{ item.path }}</p>
    </div>
    <div class="flex flex-wrap gap-1">
      <button
        v-for="agent in agents"
        :key="agent.id"
        class="flex size-6 items-center justify-center rounded border"
        :class="
          ownerSource(agent.id)
            ? 'border-primary/40 bg-primary/10 text-primary'
            : 'text-muted-foreground hover:bg-accent hover:text-foreground'
        "
        :disabled="!!toggling || !!ownerSource(agent.id)"
        :title="chipTitle(agent)"
        @click="emit('configure', agent)"
      >
        <LoaderCircle v-if="toggling === `${item.key}:${agent.id}`" class="size-3.5 animate-spin" />
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
      @click="emit('import')"
      ><LoaderCircle v-if="importing === item.key" class="size-3.5 animate-spin" /><Import
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
