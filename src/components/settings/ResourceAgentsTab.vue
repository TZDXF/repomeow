<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { Icon } from "@iconify/vue";
import { Bot } from "@lucide/vue";
import { Switch } from "@/components/ui/switch";
import { useSettingsStore } from "@/stores/settings";
import { listProjectAiTargets, type ProjectAiTarget } from "@/lib/project-ai-resources";
import { agentBrandIcon } from "@/lib/agent-icons";

const { t } = useI18n();
const settings = useSettingsStore();
const targets = ref<ProjectAiTarget[]>([]);
const saving = ref(false);
/** 预先解析品牌图标,无对应品牌的回退 lucide Bot。 */
const rows = computed(() =>
  targets.value.map((agent) => ({ ...agent, icon: agentBrandIcon(agent.id) })),
);
onMounted(async () => {
  try {
    targets.value = await listProjectAiTargets();
  } catch (e) {
    toast.error(String(e));
  }
});
async function toggle(id: string, visible: boolean) {
  saving.value = true;
  try {
    const hidden = new Set(settings.hiddenResourceAgents);
    if (visible) {
      hidden.delete(id);
    } else {
      hidden.add(id);
    }
    await settings.setHiddenResourceAgents([...hidden]);
  } catch (e) {
    toast.error(String(e));
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <div class="space-y-4">
    <p class="text-sm text-muted-foreground">{{ t("projectAi.agentSettingsHint") }}</p>
    <div class="divide-y rounded-lg border px-4">
      <div v-for="agent in rows" :key="agent.id" class="flex items-center gap-4 py-4">
        <div class="flex size-9 shrink-0 items-center justify-center rounded-md border bg-muted/40">
          <Icon v-if="agent.icon" :icon="agent.icon" class="size-5" />
          <Bot v-else class="size-5 text-muted-foreground" />
        </div>
        <div class="min-w-0 flex-1">
          <p class="text-sm font-medium">{{ agent.name }}</p>
          <p class="mt-1 break-all font-mono text-xs text-muted-foreground">
            Skills: {{ agent.skillPath }} · MCP: {{ agent.mcpPath }}
          </p>
        </div>
        <Switch
          :model-value="!settings.hiddenResourceAgents.includes(agent.id)"
          :disabled="saving"
          :aria-label="agent.name"
          @update:model-value="toggle(agent.id, $event)"
        />
      </div>
    </div>
  </div>
</template>
