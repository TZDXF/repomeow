<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import {
  Bot,
  FileCode,
  FileText,
  LoaderCircle,
  Package,
  Plug,
  RefreshCw,
  Settings2,
  Sparkles,
} from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import AiFileDrawer from "@/components/project/AiFileDrawer.vue";
import CcSwitchImportDialog from "@/components/project/CcSwitchImportDialog.vue";
import { baseName, splitDirName } from "@/lib/path";
import { cmd } from "@/lib/tauri";
import type { CcSwitchAssets, Project, ProjectAiAssets } from "@/types";

/**
 * 项目详情页「AI 视图」:全宽非卡片布局,聚合展示项目内 AI 资产——
 * 指令/规则文件(CLAUDE.md、AGENTS.md、.cursor/rules 等)、MCP 配置(.mcp.json 等)、
 * skills(.claude/skills 与 .agents/skills,按名称去重)与 13 个 agent 工具的安装/配置状态;
 * 支持从 cc-switch(~/.cc-switch)勾选导入 skills 与 MCP 到项目文件。
 * 点击文件条目打开右侧抽屉预览/编辑(AiFileDrawer)。
 */
const props = defineProps<{ project: Project }>();
const { t } = useI18n();

const assets = ref<ProjectAiAssets | null>(null);
const ccAssets = ref<CcSwitchAssets | null>(null);
const loading = ref(false);
const drawerPath = ref<string | null>(null);
const importOpen = ref(false);

async function load() {
  loading.value = true;
  try {
    assets.value = await cmd<ProjectAiAssets>("scan_project_ai_assets", {
      path: props.project.path,
    });
  } catch (e) {
    toast.error(String(e));
  } finally {
    loading.value = false;
  }
}

async function loadCc() {
  try {
    ccAssets.value = await cmd<CcSwitchAssets>("ai_cc_switch_assets");
  } catch {
    ccAssets.value = null;
  }
}

watch(() => props.project.path, load, { immediate: true });
void loadCc();

/** agent id → 展示名(来自后端 registry) */
const agentNames = computed(() => {
  const map = new Map<string, string>();
  for (const a of assets.value?.agents ?? []) map.set(a.id, a.name);
  return map;
});
function agentLabel(id: string): string {
  return agentNames.value.get(id) ?? id;
}

const fileIcon = (kind: string) =>
  kind === "setting" ? Settings2 : kind === "rule" ? FileText : FileCode;

const isEmpty = computed(
  () =>
    !!assets.value &&
    !assets.value.files.length &&
    !assets.value.mcp.length &&
    !assets.value.skills.length,
);
const ccAvailable = computed(() => !!ccAssets.value?.found);

/** 项目已有 skills 目录名(导入对话框勾选态初始化;skills 可多目录来源,取末段目录名) */
const projectSkillDirs = computed(() => (assets.value?.skills ?? []).map((s) => baseName(s.dir)));
/** 项目根 .mcp.json 已有的服务器名 */
const projectMcpNames = computed(
  () => assets.value?.mcp.find((m) => m.path === ".mcp.json")?.servers ?? [],
);

function onImported() {
  void load();
}
</script>

<template>
  <div class="flex flex-col gap-6 px-6 pb-6 pt-2">
    <div class="flex items-center gap-2">
      <h2 class="flex items-center gap-2 text-sm font-semibold">
        <Bot class="h-4 w-4" />
        {{ t("aiAssets.title") }}
        <LoaderCircle v-if="loading" class="h-3.5 w-3.5 animate-spin text-muted-foreground" />
      </h2>
      <div class="ml-auto flex items-center gap-1.5">
        <Button v-if="ccAvailable" size="sm" variant="outline" @click="importOpen = true">
          <Sparkles class="h-4 w-4" />
          {{ t("aiAssets.importCc") }}
        </Button>
        <Button size="sm" variant="ghost" :title="t('aiAssets.refresh')" @click="load">
          <RefreshCw class="h-4 w-4" />
        </Button>
      </div>
    </div>

    <p v-if="isEmpty" class="text-sm text-muted-foreground">
      {{ t("aiAssets.empty") }}
    </p>
    <div v-else class="grid items-start gap-x-10 gap-y-8 xl:grid-cols-2">
      <!-- 指令与规则文件 -->
      <section v-if="assets?.files.length">
        <p class="mb-2 border-b pb-1.5 text-xs font-medium text-muted-foreground">
          {{ t("aiAssets.files") }}
        </p>
        <div class="flex flex-col">
          <button
            v-for="file in assets.files"
            :key="file.path"
            type="button"
            class="flex items-center gap-2 rounded-md px-2 py-1.5 text-left hover:bg-accent"
            @click="drawerPath = file.path"
          >
            <component
              :is="fileIcon(file.kind)"
              class="h-3.5 w-3.5 shrink-0 text-muted-foreground"
            />
            <span class="min-w-0 truncate font-mono text-xs">{{ file.path }}</span>
            <span class="ml-auto flex shrink-0 gap-1">
              <span
                v-for="agent in file.agents.slice(0, 3)"
                :key="agent"
                class="rounded bg-muted px-1 py-0.5 text-[10px] text-muted-foreground"
              >
                {{ agentLabel(agent) }}
              </span>
              <span v-if="file.agents.length > 3" class="text-[10px] text-muted-foreground">
                +{{ file.agents.length - 3 }}
              </span>
            </span>
          </button>
        </div>
      </section>

      <!-- MCP 配置 -->
      <section v-if="assets?.mcp.length">
        <p class="mb-2 border-b pb-1.5 text-xs font-medium text-muted-foreground">
          {{ t("aiAssets.mcp") }}
        </p>
        <div class="flex flex-col">
          <button
            v-for="mcp in assets.mcp"
            :key="mcp.path"
            type="button"
            class="flex items-center gap-2 rounded-md px-2 py-1.5 text-left hover:bg-accent"
            @click="drawerPath = mcp.path"
          >
            <Plug class="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
            <span class="min-w-0 truncate font-mono text-xs">{{ mcp.path }}</span>
            <span class="ml-auto flex shrink-0 gap-1">
              <span
                v-for="server in mcp.servers.slice(0, 4)"
                :key="server"
                class="rounded bg-muted px-1 py-0.5 text-[10px] text-muted-foreground"
              >
                {{ server }}
              </span>
              <span v-if="mcp.servers.length > 4" class="text-[10px] text-muted-foreground">
                +{{ mcp.servers.length - 4 }}
              </span>
            </span>
          </button>
        </div>
      </section>

      <!-- Skills -->
      <section v-if="assets?.skills.length">
        <p class="mb-2 border-b pb-1.5 text-xs font-medium text-muted-foreground">
          {{ t("aiAssets.skills") }}
        </p>
        <div class="flex flex-col">
          <button
            v-for="skill in assets.skills"
            :key="skill.dir"
            type="button"
            class="flex items-center gap-2 rounded-md px-2 py-1.5 text-left hover:bg-accent"
            :title="`${skill.dir}${skill.description ? `\n${skill.description}` : ''}`"
            @click="drawerPath = `${skill.dir}/SKILL.md`"
          >
            <Package class="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
            <span class="w-36 shrink-0 truncate text-xs font-medium">{{ skill.name }}</span>
            <span
              v-if="skill.description"
              class="min-w-0 flex-1 truncate text-xs text-muted-foreground"
            >
              {{ skill.description }}
            </span>
            <span class="ml-auto shrink-0 font-mono text-[10px] text-muted-foreground/70">
              {{ splitDirName(skill.dir).parent }}
            </span>
          </button>
        </div>
      </section>

      <!-- Agent 工具汇总 -->
      <section v-if="assets?.agents.length">
        <p class="mb-2 border-b pb-1.5 text-xs font-medium text-muted-foreground">
          {{ t("aiAssets.agents") }}
        </p>
        <TooltipProvider>
          <div class="flex flex-wrap gap-1.5">
            <Tooltip v-for="agent in assets.agents" :key="agent.id">
              <TooltipTrigger as-child>
                <span
                  class="flex items-center gap-1 rounded-md border px-2 py-1 text-xs"
                  :class="
                    agent.installed && agent.configs.length
                      ? 'border-green-500/40 text-green-600 dark:text-green-400'
                      : agent.installed
                        ? 'text-foreground'
                        : 'text-muted-foreground/60'
                  "
                >
                  <span
                    class="h-1.5 w-1.5 rounded-full"
                    :class="
                      agent.installed && agent.configs.length
                        ? 'bg-green-500'
                        : agent.installed
                          ? 'bg-blue-500'
                          : 'bg-muted-foreground/40'
                    "
                  />
                  {{ agent.name }}
                </span>
              </TooltipTrigger>
              <TooltipContent>
                <p>
                  {{ agent.installed ? t("aiAssets.installed") : t("aiAssets.notInstalled") }}
                  <template v-if="agent.configs.length">
                    · {{ t("aiAssets.configured", { count: agent.configs.length }) }}
                  </template>
                </p>
                <p
                  v-for="config in agent.configs"
                  :key="config"
                  class="font-mono text-[10px] opacity-80"
                >
                  {{ config }}
                </p>
              </TooltipContent>
            </Tooltip>
          </div>
        </TooltipProvider>
      </section>
    </div>
  </div>

  <AiFileDrawer
    :root="project.path"
    :rel-path="drawerPath"
    @close="drawerPath = null"
    @navigate="drawerPath = $event"
  />
  <CcSwitchImportDialog
    v-model:open="importOpen"
    :project="project"
    :assets="ccAssets"
    :project-skill-dirs="projectSkillDirs"
    :project-mcp-names="projectMcpNames"
    @changed="onImported"
  />
</template>
