<script setup lang="ts">
import { computed, reactive, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { LoaderCircle, Plug, Sparkles } from "@lucide/vue";
import { Badge } from "@/components/ui/badge";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Switch } from "@/components/ui/switch";
import { cmd } from "@/lib/tauri";
import type { CcSwitchAssets, CcSwitchMcpServer, CcSwitchSkill, Project } from "@/types";

/**
 * 「从 CC Switch 导入」对话框:列出 ~/.cc-switch 管理的 skills 与 MCP 服务器,
 * 勾选即导出到项目(skill → .claude/skills/<dir>,MCP → .mcp.json 合并),
 * 取消勾选即从项目移除;勾选状态由卡片按项目文件探测结果初始化,操作即时生效。
 */
const props = defineProps<{
  project: Project;
  /** cc-switch 资产;null 表示本机没有 ~/.cc-switch */
  assets: CcSwitchAssets | null;
  /** 项目 .claude/skills 下已有的技能目录名(不含前缀) */
  projectSkillDirs: string[];
  /** 项目 .mcp.json 的 mcpServers 键(仅根目录 .mcp.json,导出目标文件) */
  projectMcpNames: string[];
}>();
const emit = defineEmits<{ (e: "changed"): void }>();
const open = defineModel<boolean>("open", { required: true });

const { t } = useI18n();

type Tab = "skills" | "mcp";
const tab = ref<Tab>("skills");

/** 本地勾选态(prop 初始化 + 操作成功后立即更新,不等卡片刷新) */
const checkedSkills = ref(new Set<string>());
const checkedMcp = ref(new Set<string>());
/** 进行中的切换(id 集合),行级 loading */
const busy = reactive(new Set<string>());

watch(
  open,
  (v) => {
    if (!v) return;
    checkedSkills.value = new Set(props.projectSkillDirs);
    checkedMcp.value = new Set(props.projectMcpNames);
  },
  { immediate: true },
);

const skills = computed(() => props.assets?.skills ?? []);
const mcpServers = computed(() => props.assets?.mcpServers ?? []);

/** MCP 服务器定义摘要:stdio 显示 command + args,http/sse 显示 url */
function mcpSummary(server: CcSwitchMcpServer): string {
  const config = server.serverConfig as Record<string, unknown>;
  const command = typeof config.command === "string" ? config.command : "";
  if (command) {
    const args = Array.isArray(config.args) ? config.args.filter((a) => typeof a === "string") : [];
    return [command, ...args].join(" ");
  }
  return typeof config.url === "string" ? config.url : "";
}

async function toggleSkill(skill: CcSwitchSkill, on: boolean) {
  if (busy.has(skill.id)) return;
  busy.add(skill.id);
  try {
    await cmd("set_project_cc_skill", {
      path: props.project.path,
      directory: skill.directory,
      enable: on,
    });
    if (on) checkedSkills.value.add(skill.directory);
    else checkedSkills.value.delete(skill.directory);
    checkedSkills.value = new Set(checkedSkills.value);
    emit("changed");
  } catch (e) {
    toast.error(String(e));
  } finally {
    busy.delete(skill.id);
  }
}

async function toggleMcp(server: CcSwitchMcpServer, on: boolean) {
  if (busy.has(server.id)) return;
  busy.add(server.id);
  try {
    await cmd("set_project_cc_mcp", {
      path: props.project.path,
      serverId: server.id,
      serverName: server.name,
      enable: on,
    });
    if (on) checkedMcp.value.add(server.name);
    else checkedMcp.value.delete(server.name);
    checkedMcp.value = new Set(checkedMcp.value);
    emit("changed");
  } catch (e) {
    toast.error(String(e));
  } finally {
    busy.delete(server.id);
  }
}
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="sm:max-w-[min(40rem,calc(100%-2rem))]">
      <DialogHeader>
        <DialogTitle>{{ t("aiAssets.import.title") }}</DialogTitle>
        <DialogDescription>{{ t("aiAssets.import.description") }}</DialogDescription>
      </DialogHeader>

      <div class="flex gap-1 rounded-lg bg-muted p-1">
        <button
          type="button"
          class="flex flex-1 items-center justify-center gap-1.5 rounded-md px-3 py-1.5 text-sm transition-colors"
          :class="
            tab === 'skills'
              ? 'bg-background shadow-sm'
              : 'text-muted-foreground hover:text-foreground'
          "
          @click="tab = 'skills'"
        >
          <Sparkles class="h-3.5 w-3.5" />
          {{ t("aiAssets.import.tabSkills") }} ({{ skills.length }})
        </button>
        <button
          type="button"
          class="flex flex-1 items-center justify-center gap-1.5 rounded-md px-3 py-1.5 text-sm transition-colors"
          :class="
            tab === 'mcp'
              ? 'bg-background shadow-sm'
              : 'text-muted-foreground hover:text-foreground'
          "
          @click="tab = 'mcp'"
        >
          <Plug class="h-3.5 w-3.5" />
          {{ t("aiAssets.import.tabMcp") }} ({{ mcpServers.length }})
        </button>
      </div>

      <ScrollArea class="max-h-[50vh]">
        <div v-if="tab === 'skills'" class="flex flex-col">
          <p v-if="!skills.length" class="py-8 text-center text-sm text-muted-foreground">
            {{ t("aiAssets.import.emptySkills") }}
          </p>
          <div
            v-for="skill in skills"
            :key="skill.id"
            class="flex items-center gap-3 border-b px-1 py-2.5 last:border-b-0"
          >
            <div class="min-w-0 flex-1">
              <p class="truncate text-sm font-medium" :title="skill.name">{{ skill.name }}</p>
              <p
                v-if="skill.description"
                class="truncate text-xs text-muted-foreground"
                :title="skill.description"
              >
                {{ skill.description }}
              </p>
            </div>
            <LoaderCircle
              v-if="busy.has(skill.id)"
              class="h-4 w-4 animate-spin text-muted-foreground"
            />
            <Switch
              :model-value="checkedSkills.has(skill.directory)"
              :disabled="busy.has(skill.id)"
              @update:model-value="toggleSkill(skill, $event)"
            />
          </div>
        </div>

        <div v-else class="flex flex-col">
          <p v-if="!mcpServers.length" class="py-8 text-center text-sm text-muted-foreground">
            {{ t("aiAssets.import.emptyMcp") }}
          </p>
          <div
            v-for="server in mcpServers"
            :key="server.id"
            class="flex items-center gap-3 border-b px-1 py-2.5 last:border-b-0"
          >
            <div class="min-w-0 flex-1">
              <div class="flex items-center gap-1.5">
                <p class="truncate text-sm font-medium" :title="server.name">{{ server.name }}</p>
                <Badge
                  v-for="tag in server.tags.slice(0, 3)"
                  :key="tag"
                  variant="secondary"
                  class="shrink-0 text-[10px]"
                >
                  {{ tag }}
                </Badge>
              </div>
              <p
                v-if="mcpSummary(server)"
                class="truncate font-mono text-xs text-muted-foreground"
                :title="mcpSummary(server)"
              >
                {{ mcpSummary(server) }}
              </p>
              <p
                v-else-if="server.description"
                class="truncate text-xs text-muted-foreground"
                :title="server.description"
              >
                {{ server.description }}
              </p>
            </div>
            <LoaderCircle
              v-if="busy.has(server.id)"
              class="h-4 w-4 animate-spin text-muted-foreground"
            />
            <Switch
              :model-value="checkedMcp.has(server.name)"
              :disabled="busy.has(server.id)"
              @update:model-value="toggleMcp(server, $event)"
            />
          </div>
        </div>
      </ScrollArea>
    </DialogContent>
  </Dialog>
</template>
