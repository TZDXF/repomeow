<script setup lang="ts">
import { onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { Bot, Code, Loader2, Terminal, TriangleAlert } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ScrollArea } from "@/components/ui/scroll-area";
import AgentModelThinkingSelect from "@/components/ai-elements/AgentModelThinkingSelect.vue";
import { useAiConfigStore } from "@/stores/ai-config";
import { getEditorAvailability, isEditorUnavailable } from "@/lib/open-with";
import type { EditorAvailability } from "@/lib/open-with";
import { cmd } from "@/lib/tauri";
import type { EditorKind, Project } from "@/types";

const { t } = useI18n();
// path 缺省为项目路径;worktree 内产生的冲突传 worktree 路径,确保「打开」落在正确目录
const props = defineProps<{ project: Project; conflicts: string[]; path?: string }>();
const open = defineModel<boolean>("open", { required: true });

const availability = ref<EditorAvailability | null>(null);
const aiConfig = useAiConfigStore();
const model = ref("");
const thinking = ref("");
const agentsLoading = ref(true);
const startingAgent = ref(false);

onMounted(async () => {
  void getEditorAvailability()
    .then((value) => {
      availability.value = value;
    })
    .catch(() => {});
  try {
    await aiConfig.ensureLoaded();
  } catch (error) {
    toast.error(String(error));
  } finally {
    agentsLoading.value = false;
  }
});

/** 冲突不在应用内手工解决:引导用户到更合适的工具中处理。 */
async function openIn(kind: EditorKind) {
  try {
    await cmd("open_with", { path: props.path ?? props.project.path, kind });
    open.value = false;
  } catch (e) {
    toast.error(String(e));
  }
}

/** 内置 Agent 受限修复；模型与思考强度传入后台任务。 */
async function resolveWithAgent() {
  if (agentsLoading.value || startingAgent.value) {
    return;
  }
  startingAgent.value = true;
  try {
    await cmd<string>("resolve_git_conflicts_with_agent", {
      model: model.value || null,
      thinking: thinking.value || null,
      projectId: props.project.id,
      projectName: props.project.name,
      path: props.path ?? props.project.path,
    });
    toast.success(t("git.conflict.agentStarted"));
    open.value = false;
  } catch (error) {
    toast.error(String(error));
  } finally {
    startingAgent.value = false;
  }
}
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="sm:max-w-xl">
      <DialogHeader>
        <DialogTitle class="flex items-center gap-2">
          <TriangleAlert class="h-4 w-4 text-amber-500" />
          {{ t("git.conflict.title") }}
        </DialogTitle>
        <DialogDescription>
          {{ t("git.conflict.description", { count: conflicts.length }) }}
        </DialogDescription>
      </DialogHeader>
      <div class="flex flex-col gap-1.5">
        <p class="text-sm font-medium">{{ t("git.conflict.files") }}</p>
        <ScrollArea class="h-40 rounded-md border">
          <ul class="p-2 font-mono text-xs text-muted-foreground">
            <li v-for="f in conflicts" :key="f" class="truncate py-0.5" :title="f">
              {{ f }}
            </li>
          </ul>
        </ScrollArea>
      </div>
      <div class="flex flex-col gap-1.5">
        <p class="text-sm font-medium">{{ t("git.conflict.agentLabel") }}</p>
        <AgentModelThinkingSelect
          v-model:model="model"
          v-model:thinking="thinking"
          :disabled="agentsLoading || startingAgent"
          trigger-class="w-full"
        />
        <p class="text-xs text-muted-foreground">{{ t("git.conflict.agentHint") }}</p>
      </div>
      <DialogFooter class="flex-wrap gap-2">
        <Button :disabled="agentsLoading || startingAgent" @click="resolveWithAgent">
          <Loader2 v-if="startingAgent" class="h-4 w-4 animate-spin" />
          <Bot v-else class="h-4 w-4" />
          {{ startingAgent ? t("git.conflict.agentStarting") : t("git.conflict.resolveWithAgent") }}
        </Button>
        <Button
          v-if="!isEditorUnavailable('vscode', availability)"
          variant="outline"
          @click="openIn('vscode')"
        >
          <Code class="h-4 w-4" />
          {{ t("git.conflict.openVscode") }}
        </Button>
        <Button variant="outline" @click="openIn('terminal')">
          <Terminal class="h-4 w-4" />
          {{ t("git.conflict.openTerminal") }}
        </Button>
        <Button variant="ghost" @click="open = false">
          {{ t("git.conflict.close") }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
