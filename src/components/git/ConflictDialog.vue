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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { agentList, type AgentInfo } from "@/lib/agent";
import { getEditorAvailability, isEditorUnavailable } from "@/lib/open-with";
import type { EditorAvailability } from "@/lib/open-with";
import { cmd } from "@/lib/tauri";
import type { EditorKind, Project } from "@/types";

const { t } = useI18n();
// path 缺省为项目路径;worktree 内产生的冲突传 worktree 路径,确保「打开」落在正确目录
const props = defineProps<{ project: Project; conflicts: string[]; path?: string }>();
const open = defineModel<boolean>("open", { required: true });

const LAST_AGENT_KEY = "repomeow.conflict-agent";
const availability = ref<EditorAvailability | null>(null);
const installedAgents = ref<AgentInfo[]>([]);
const agentsLoading = ref(true);
const selectedAgentId = ref("");
const startingAgent = ref(false);

function readLastAgent(): string {
  try {
    return globalThis.localStorage?.getItem(LAST_AGENT_KEY) ?? "";
  } catch {
    return "";
  }
}

onMounted(async () => {
  const availabilityPromise = getEditorAvailability().then((value) => {
    availability.value = value;
  });
  try {
    const agents = (await agentList()).filter((agent) => agent.installed);
    installedAgents.value = agents;
    const remembered = readLastAgent();
    selectedAgentId.value =
      agents.find((agent) => agent.id === remembered)?.id ??
      agents.find((agent) => agent.id === "codex")?.id ??
      agents[0]?.id ??
      "";
  } catch {
    installedAgents.value = [];
  } finally {
    agentsLoading.value = false;
  }
  await availabilityPromise;
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

/** 创建独立后台任务，由显式选择的本地 ACP agent 修改并暂存冲突文件。 */
async function resolveWithAgent() {
  if (!selectedAgentId.value || startingAgent.value) {
    return;
  }
  startingAgent.value = true;
  try {
    await cmd<string>("resolve_git_conflicts_with_agent", {
      agentId: selectedAgentId.value,
      projectId: props.project.id,
      projectName: props.project.name,
      path: props.path ?? props.project.path,
    });
    try {
      globalThis.localStorage?.setItem(LAST_AGENT_KEY, selectedAgentId.value);
    } catch {
      // 偏好持久化失败不影响任务。
    }
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
        <Select v-model="selectedAgentId" :disabled="agentsLoading || !installedAgents.length">
          <SelectTrigger>
            <SelectValue :placeholder="t('git.conflict.agentPlaceholder')" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem v-for="agent in installedAgents" :key="agent.id" :value="agent.id">
              {{ agent.name }}
            </SelectItem>
          </SelectContent>
        </Select>
        <p class="text-xs text-muted-foreground">
          {{
            agentsLoading
              ? t("git.conflict.agentLoading")
              : installedAgents.length
                ? t("git.conflict.agentHint")
                : t("git.conflict.agentUnavailable")
          }}
        </p>
      </div>
      <DialogFooter class="flex-wrap gap-2">
        <Button :disabled="!selectedAgentId || startingAgent" @click="resolveWithAgent">
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
