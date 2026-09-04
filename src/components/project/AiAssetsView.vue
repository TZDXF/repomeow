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
  Pencil,
  Plug,
  Plus,
  RefreshCw,
  Settings2,
  Sparkles,
  Trash2,
} from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import AiFileDrawer from "@/components/project/AiFileDrawer.vue";
import CcSwitchImportDialog from "@/components/project/CcSwitchImportDialog.vue";
import ConfirmDialog from "@/components/common/ConfirmDialog.vue";
import McpServerFormDialog from "@/components/project/McpServerFormDialog.vue";
import SkillCreateDialog from "@/components/project/SkillCreateDialog.vue";
import { formatTokenCount } from "@/lib/chat";
import { baseName, splitDirName } from "@/lib/path";
import { cmd } from "@/lib/tauri";
import type {
  CcSwitchAssets,
  McpServerEntry,
  Project,
  ProjectAiAssets,
  ProjectMcpFile,
  ProjectSkill,
} from "@/types";

/**
 * 项目详情页「AI 视图」:全宽非卡片布局,聚合展示项目内 AI 资产——
 * 指令/规则文件(CLAUDE.md、AGENTS.md、.cursor/rules 等)、MCP 配置(.mcp.json 等)、
 * skills(.claude/skills、.agents/skills 与 .zcode/skills,按名称去重)与 13 个 agent 工具的安装/配置状态;
 * 支持从 cc-switch(~/.cc-switch)勾选导入 skills 与 MCP 到项目文件,
 * 并可视化管理:MCP 服务器表单新增/编辑/移除、skills 的新建与删除。
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

const ccAvailable = computed(() => !!ccAssets.value?.found);

/** 项目已有 skills 目录名(导入对话框勾选态初始化;skills 可多目录来源,取末段目录名) */
const projectSkillDirs = computed(() => (assets.value?.skills ?? []).map((s) => baseName(s.dir)));
/** 当前抽屉若打开一个已扫描的 SKILL.md，则提供其描述 token 供抽屉头部展示。 */
const drawerSkill = computed(() => {
  if (!drawerPath.value) return null;
  return assets.value?.skills.find((skill) => `${skill.dir}/SKILL.md` === drawerPath.value) ?? null;
});
/** 项目根 .mcp.json 已有的服务器名 */
const projectMcpNames = computed(
  () => assets.value?.mcp.find((m) => m.path === ".mcp.json")?.servers.map((s) => s.name) ?? [],
);

function reload() {
  void load();
}

function onImported() {
  reload();
}

// ── MCP 服务器可视化管理(表单新增/编辑/移除) ────────────────────────────

const mcpFormOpen = ref(false);
const mcpFormConfigPath = ref(".mcp.json");
const mcpFormEditing = ref<McpServerEntry | null>(null);
const pendingMcpDelete = ref<{ file: ProjectMcpFile; entry: McpServerEntry } | null>(null);

const mcpDeleteOpen = computed({
  get: () => pendingMcpDelete.value !== null,
  set: (v) => {
    if (!v) pendingMcpDelete.value = null;
  },
});
const mcpDeleteDescription = computed(() =>
  t("aiAssets.removeMcpDescription", {
    name: pendingMcpDelete.value?.entry.name ?? "",
    path: pendingMcpDelete.value?.file.path ?? "",
  }),
);

function openMcpAdd() {
  mcpFormConfigPath.value = ".mcp.json";
  mcpFormEditing.value = null;
  mcpFormOpen.value = true;
}

function openMcpEdit(file: ProjectMcpFile, entry: McpServerEntry) {
  mcpFormConfigPath.value = file.path;
  mcpFormEditing.value = entry;
  mcpFormOpen.value = true;
}

async function removeMcpServer() {
  const target = pendingMcpDelete.value;
  if (!target) return;
  pendingMcpDelete.value = null;
  try {
    await cmd("remove_project_mcp_server", {
      path: props.project.path,
      configPath: target.file.path,
      name: target.entry.name,
    });
    toast.success(t("aiAssets.removedMcp", { name: target.entry.name }));
    await load();
  } catch (e) {
    toast.error(String(e));
  }
}

/** 服务器类型徽标:取 type,缺省按是否有 url 推断 */
function serverType(config: Record<string, unknown>): string {
  if (config.type === "http" || config.type === "sse" || config.type === "stdio") {
    return config.type;
  }
  return typeof config.url === "string" ? "http" : "stdio";
}

/** 服务器定义摘要:stdio 显示 command + args,http/sse 显示 url */
function mcpSummary(config: Record<string, unknown>): string {
  const command = typeof config.command === "string" ? config.command : "";
  if (command) {
    const args = Array.isArray(config.args)
      ? config.args.filter((a): a is string => typeof a === "string")
      : [];
    return [command, ...args].join(" ");
  }
  return typeof config.url === "string" ? config.url : "";
}

// ── Skills 可视化管理(新建/删除) ────────────────────────────────────

const skillFormOpen = ref(false);
const pendingSkillDelete = ref<ProjectSkill | null>(null);

const skillDeleteOpen = computed({
  get: () => pendingSkillDelete.value !== null,
  set: (v) => {
    if (!v) pendingSkillDelete.value = null;
  },
});
const skillDeleteDescription = computed(() =>
  t("aiAssets.deleteSkillDescription", { dir: pendingSkillDelete.value?.dir ?? "" }),
);

async function removeSkill() {
  const skill = pendingSkillDelete.value;
  if (!skill) return;
  pendingSkillDelete.value = null;
  try {
    await cmd("delete_project_skill", { path: props.project.path, dir: skill.dir });
    // 抽屉正开着被删技能的 SKILL.md 时一并关闭
    if (drawerPath.value?.startsWith(`${skill.dir}/`)) drawerPath.value = null;
    toast.success(t("aiAssets.deletedSkill", { name: skill.name }));
    await load();
  } catch (e) {
    toast.error(String(e));
  }
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

    <div v-if="assets" class="grid items-start gap-x-10 gap-y-8 xl:grid-cols-2">
      <!-- 指令与规则文件 -->
      <section v-if="assets.files.length">
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

      <!-- MCP 配置(可视化管理:服务器行支持表单编辑与移除) -->
      <section>
        <div class="mb-2 flex items-center border-b pb-1.5">
          <p class="text-xs font-medium text-muted-foreground">
            {{ t("aiAssets.mcp") }}
          </p>
          <Button
            size="sm"
            variant="ghost"
            class="ml-auto h-6 gap-1 px-2 text-xs text-muted-foreground hover:text-foreground"
            @click="openMcpAdd"
          >
            <Plus class="h-3 w-3" />
            {{ t("aiAssets.mcpAdd") }}
          </Button>
        </div>
        <p v-if="!assets.mcp.length" class="px-2 text-xs text-muted-foreground">
          {{ t("aiAssets.mcpEmpty") }}
        </p>
        <div v-else class="flex flex-col">
          <template v-for="mcpFile in assets.mcp" :key="mcpFile.path">
            <button
              type="button"
              class="flex items-center gap-2 rounded-md px-2 py-1 text-left hover:bg-accent"
              :title="t('aiAssets.mcpRawHint')"
              @click="drawerPath = mcpFile.path"
            >
              <Plug class="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
              <span class="min-w-0 truncate font-mono text-xs">{{ mcpFile.path }}</span>
              <span class="ml-auto shrink-0 text-[10px] tabular-nums text-muted-foreground/70">
                {{ t("aiAssets.mcpServerCount", { count: mcpFile.servers.length }) }}
              </span>
            </button>
            <div
              v-for="entry in mcpFile.servers"
              :key="`${mcpFile.path}:${entry.name}`"
              class="group flex items-center gap-2 rounded-md py-1 pl-7 pr-1.5 hover:bg-accent/60"
            >
              <span class="w-28 shrink-0 truncate text-xs font-medium" :title="entry.name">
                {{ entry.name }}
              </span>
              <span
                class="shrink-0 rounded bg-muted px-1 py-0.5 text-[10px] uppercase text-muted-foreground"
              >
                {{ serverType(entry.config) }}
              </span>
              <span
                v-if="mcpSummary(entry.config)"
                class="min-w-0 flex-1 truncate font-mono text-xs text-muted-foreground"
                :title="mcpSummary(entry.config)"
              >
                {{ mcpSummary(entry.config) }}
              </span>
              <span v-else class="min-w-0 flex-1" />
              <span class="flex shrink-0 opacity-0 transition-opacity group-hover:opacity-100">
                <Button
                  size="icon"
                  variant="ghost"
                  class="h-6 w-6 text-muted-foreground"
                  :title="t('common.edit')"
                  @click="openMcpEdit(mcpFile, entry)"
                >
                  <Pencil class="h-3.5 w-3.5" />
                </Button>
                <Button
                  size="icon"
                  variant="ghost"
                  class="h-6 w-6 text-muted-foreground hover:text-destructive"
                  :title="t('common.delete')"
                  @click="pendingMcpDelete = { file: mcpFile, entry }"
                >
                  <Trash2 class="h-3.5 w-3.5" />
                </Button>
              </span>
            </div>
          </template>
        </div>
      </section>

      <!-- Skills(可视化管理:新建与删除) -->
      <section>
        <div class="mb-2 flex items-center border-b pb-1.5">
          <p class="text-xs font-medium text-muted-foreground">
            {{ t("aiAssets.skills") }}
          </p>
          <Button
            size="sm"
            variant="ghost"
            class="ml-auto h-6 gap-1 px-2 text-xs text-muted-foreground hover:text-foreground"
            @click="skillFormOpen = true"
          >
            <Plus class="h-3 w-3" />
            {{ t("aiAssets.skillAdd") }}
          </Button>
        </div>
        <p v-if="!assets.skills.length" class="px-2 text-xs text-muted-foreground">
          {{ t("aiAssets.skillsEmpty") }}
        </p>
        <div v-else class="flex flex-col">
          <div
            v-for="skill in assets.skills"
            :key="skill.dir"
            class="group flex items-center gap-2 rounded-md px-2 py-1.5 hover:bg-accent"
            :title="`${skill.dir}${skill.description ? `\n${skill.description}` : ''}`"
          >
            <button
              type="button"
              class="flex min-w-0 flex-1 items-center gap-2 text-left"
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
              <span v-else class="min-w-0 flex-1" />
              <span
                class="shrink-0 text-[10px] tabular-nums text-muted-foreground/70"
                :title="
                  t('aiAssets.skillTokenUsageFull', {
                    description: skill.descriptionTokenCount,
                    total: skill.tokenCount,
                  })
                "
              >
                {{
                  t("aiAssets.skillTokenUsage", {
                    description: formatTokenCount(skill.descriptionTokenCount),
                    total: formatTokenCount(skill.tokenCount),
                  })
                }}
              </span>
            </button>
            <span class="shrink-0 font-mono text-[10px] text-muted-foreground/70">
              {{ splitDirName(skill.dir).parent }}
            </span>
            <Button
              size="icon"
              variant="ghost"
              class="h-6 w-6 shrink-0 text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100 hover:text-destructive"
              :title="t('common.delete')"
              @click="pendingSkillDelete = skill"
            >
              <Trash2 class="h-3.5 w-3.5" />
            </Button>
          </div>
        </div>
      </section>

      <!-- Agent 工具汇总 -->
      <section v-if="assets.agents.length">
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
    :description-token-count="drawerSkill?.descriptionTokenCount ?? null"
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
  <McpServerFormDialog
    v-model:open="mcpFormOpen"
    :project-path="project.path"
    :config-path="mcpFormConfigPath"
    :editing="mcpFormEditing"
    @saved="reload"
  />
  <SkillCreateDialog v-model:open="skillFormOpen" :project-path="project.path" @saved="reload" />
  <ConfirmDialog
    v-model:open="mcpDeleteOpen"
    :title="t('aiAssets.removeMcpTitle')"
    :description="mcpDeleteDescription"
    destructive
    :confirm-text="t('common.delete')"
    @confirm="removeMcpServer"
  />
  <ConfirmDialog
    v-model:open="skillDeleteOpen"
    :title="t('aiAssets.deleteSkillTitle')"
    :description="skillDeleteDescription"
    destructive
    :confirm-text="t('common.delete')"
    @confirm="removeSkill"
  />
</template>
