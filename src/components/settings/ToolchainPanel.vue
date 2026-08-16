<script setup lang="ts">
import { computed, markRaw, onMounted, ref } from "vue";
import type { Component } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import type { AcceptableValue } from "reka-ui";
import {
  Box,
  Check,
  FileTerminal,
  GitBranch,
  Hammer,
  Hexagon,
  Plus,
  RotateCw,
  X,
} from "@lucide/vue";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { cmd } from "@/lib/tauri";
import type { ToolchainKind, ToolchainOp, ToolchainStatus } from "@/types";

const { t } = useI18n();

const items = ref<ToolchainStatus[]>([]);
const scanning = ref(false);
/** 首扫是否已结束(区分「检测中」与「检测失败」两种空列表) */
const scanned = ref(false);

/** 分组展示顺序与图标;工具行顺序由后端 TOOLS 注册表决定 */
const KIND_ORDER: ToolchainKind[] = ["rust", "python", "node", "dotnet", "git"];
const GROUP_ICONS: Record<ToolchainKind, Component> = {
  rust: markRaw(Hammer),
  python: markRaw(FileTerminal),
  node: markRaw(Hexagon),
  dotnet: markRaw(Box),
  git: markRaw(GitBranch),
};

const groups = computed(() =>
  KIND_ORDER.map((kind) => ({
    kind,
    icon: GROUP_ICONS[kind],
    tools: items.value.filter((tool) => tool.kind === kind),
  })).filter((group) => group.tools.length > 0),
);

/** 版本管理器各自管理的东西不同,版本区标签随之区分 */
function versionLabel(tool: ToolchainStatus): string {
  if (tool.id === "rustup") return t("settings.devEnv.tools.toolchainsLabel");
  if (tool.id === "dotnet") return t("settings.devEnv.tools.sdksLabel");
  return t("settings.devEnv.tools.nodeVersionsLabel");
}

function displayName(tool: ToolchainStatus): string {
  return tool.id === "vp" ? "vp (Vite+)" : tool.id;
}

async function scan() {
  if (scanning.value) return;
  scanning.value = true;
  try {
    items.value = await cmd<ToolchainStatus[]>("detect_toolchains");
    scanned.value = true;
  } catch (e) {
    toast.error(e instanceof Error ? e.message : String(e));
  } finally {
    scanning.value = false;
  }
}

/** 执行工具链操作:后端解析出命令串并在系统终端新窗口执行 */
async function run(tool: string, op: ToolchainOp, version?: string) {
  try {
    await cmd("toolchain_op", { tool, op, ...(version ? { version } : {}) });
    toast.success(t("settings.devEnv.tools.opLaunched"));
  } catch (e) {
    toast.error(e instanceof Error ? e.message : String(e));
  }
}

function install(tool: ToolchainStatus) {
  // dotnet 的 winget id 按大版本区分,先选目标大版本
  if (tool.id === "dotnet") {
    dotnetMajor.value = "10";
    dotnetOpen.value = true;
    return;
  }
  void run(tool.id, "install");
}

function uninstall(tool: ToolchainStatus) {
  let message = t("settings.devEnv.tools.uninstallConfirm", { name: displayName(tool) });
  if (tool.id === "git" || tool.id === "gh") {
    message += `\n${t("settings.devEnv.tools.uninstallGitWarn")}`;
  } else if (tool.id === "vp") {
    message += `\n${t("settings.devEnv.tools.uninstallVpWarn")}`;
  }
  if (!window.confirm(message)) return;
  void run(tool.id, "uninstall");
}

// ---- 版本管理(nvm/fnm/vp/rustup) ─────────────────────────────────────────

/** 各版本管理器行内的「安装指定版本」输入框 */
const versionInputs = ref<Record<string, string>>({});

function useVersion(tool: ToolchainStatus, name: string) {
  if (!tool.caps.can_switch) return;
  void run(tool.id, "use", name);
}

function removeVersion(tool: ToolchainStatus, name: string) {
  if (
    !tool.caps.can_switch ||
    !window.confirm(t("settings.devEnv.tools.uninstallVersionConfirm", { name }))
  ) {
    return;
  }
  void run(tool.id, "uninstall_version", name);
}

function addVersion(tool: ToolchainStatus) {
  const version = (versionInputs.value[tool.id] ?? "").trim();
  if (!version) return;
  versionInputs.value[tool.id] = "";
  void run(tool.id, "install_version", version);
}

// ---- dotnet 安装大版本选择 ────────────────────────────────────────────────

const dotnetOpen = ref(false);
const dotnetMajor = ref("10");
const DOTNET_MAJORS = ["8", "9", "10"];

function onDotnetMajorChange(value: AcceptableValue) {
  if (typeof value === "string") dotnetMajor.value = value;
}

function confirmDotnetInstall() {
  dotnetOpen.value = false;
  void run("dotnet", "install", dotnetMajor.value);
}

onMounted(() => {
  void scan();
});
</script>

<template>
  <section>
    <div class="flex items-start justify-between gap-4">
      <div>
        <h2 class="text-base font-semibold">{{ t("settings.devEnv.tools.title") }}</h2>
        <p class="mt-1 text-xs text-muted-foreground">
          {{ t("settings.devEnv.tools.hint") }}
        </p>
      </div>
      <Button size="sm" variant="outline" :disabled="scanning" @click="scan">
        <RotateCw class="h-4 w-4" :class="scanning && 'animate-spin'" />
        {{ scanning ? t("settings.devEnv.tools.rescanning") : t("settings.devEnv.tools.rescan") }}
      </Button>
    </div>

    <p v-if="scanning && !items.length" class="mt-4 text-sm text-muted-foreground">
      {{ t("settings.devEnv.tools.rescanning") }}
    </p>
    <p v-else-if="!items.length" class="mt-4 text-sm text-muted-foreground">
      {{ t("settings.devEnv.tools.scanFailed") }}
    </p>

    <div v-for="group in groups" :key="group.kind">
      <div class="mb-2 mt-5 flex items-center gap-2 text-sm font-medium text-muted-foreground">
        <component :is="group.icon" class="h-4 w-4" />
        {{ t(`settings.devEnv.tools.groups.${group.kind}`) }}
      </div>
      <div class="flex flex-col gap-2">
        <div
          v-for="tool in group.tools"
          :key="tool.id"
          class="rounded-lg border"
          :class="!tool.found && 'border-dashed'"
        >
          <div class="flex items-center gap-3 px-3 py-2.5">
            <span
              class="w-24 shrink-0 font-mono text-sm font-medium"
              :class="!tool.found && 'text-muted-foreground'"
            >
              {{ displayName(tool) }}
            </span>
            <Badge v-if="tool.version" variant="secondary" class="font-mono">
              {{ tool.version }}
            </Badge>
            <Badge v-else-if="tool.found" variant="outline">
              {{ t("settings.devEnv.tools.installedNoVersion") }}
            </Badge>
            <Badge v-else variant="outline" class="text-muted-foreground">
              {{ t("settings.devEnv.tools.notFound") }}
            </Badge>
            <span
              v-if="tool.found && (tool.id === 'rustc' || tool.id === 'cargo')"
              class="hidden shrink-0 text-xs text-muted-foreground sm:inline"
            >
              {{ t("settings.devEnv.tools.managedByRustup") }}
            </span>
            <span
              v-if="tool.path"
              class="min-w-0 flex-1 truncate font-mono text-xs text-muted-foreground"
              :title="tool.path"
            >
              {{ tool.path }}
            </span>
            <span v-else class="min-w-0 flex-1"></span>
            <div class="flex shrink-0 gap-1.5">
              <Button
                v-if="tool.caps.can_install"
                size="sm"
                variant="outline"
                @click="install(tool)"
              >
                {{ t("settings.devEnv.tools.install") }}
              </Button>
              <Button
                v-if="tool.caps.can_update"
                size="sm"
                variant="outline"
                @click="run(tool.id, 'update')"
              >
                {{ t("settings.devEnv.tools.update") }}
              </Button>
              <Button
                v-if="tool.caps.can_uninstall"
                size="sm"
                variant="outline"
                @click="uninstall(tool)"
              >
                {{ t("settings.devEnv.tools.uninstall") }}
              </Button>
            </div>
          </div>
          <!-- 版本区:nvm/fnm/vp/rustup 可切换装卸,dotnet 只读展示已装 SDK -->
          <div
            v-if="tool.versions.length"
            class="flex flex-wrap items-center gap-1.5 border-t px-3 py-2 text-xs"
          >
            <span class="mr-1 text-muted-foreground">{{ versionLabel(tool) }}</span>
            <span
              v-for="v in tool.versions"
              :key="v.name"
              class="inline-flex items-center gap-1 rounded-md border px-2 py-0.5 font-mono"
              :class="v.current ? 'border-primary bg-primary/10' : 'border-border'"
            >
              <Check v-if="v.current" class="h-3 w-3 text-primary" />
              <button
                v-if="tool.caps.can_switch && !v.current"
                type="button"
                :title="t('settings.devEnv.tools.setDefault')"
                class="hover:text-primary hover:underline"
                @click="useVersion(tool, v.name)"
              >
                {{ v.name }}
              </button>
              <span v-else :title="v.current ? t('settings.devEnv.tools.current') : ''">
                {{ v.name }}
              </span>
              <button
                v-if="tool.caps.can_switch"
                type="button"
                class="rounded p-0.5 text-muted-foreground hover:bg-accent hover:text-destructive"
                :title="t('settings.devEnv.tools.uninstall')"
                @click="removeVersion(tool, v.name)"
              >
                <X class="h-3 w-3" />
              </button>
            </span>
            <template v-if="tool.caps.can_switch">
              <Input
                v-model="versionInputs[tool.id]"
                :placeholder="t('settings.devEnv.tools.installVersionPlaceholder')"
                class="h-6 w-28 px-2 text-xs"
                @keyup.enter="addVersion(tool)"
              />
              <Button
                size="icon-sm"
                variant="ghost"
                :title="t('settings.devEnv.tools.installVersionBtn')"
                @click="addVersion(tool)"
              >
                <Plus class="h-3.5 w-3.5" />
              </Button>
            </template>
          </div>
        </div>
      </div>
    </div>

    <Dialog v-model:open="dotnetOpen">
      <DialogContent class="sm:max-w-[min(24rem,calc(100%-2rem))]">
        <DialogHeader>
          <DialogTitle>{{ t("settings.devEnv.tools.dotnetInstallTitle") }}</DialogTitle>
        </DialogHeader>
        <div class="flex flex-col gap-1.5">
          <label class="text-sm font-medium">{{
            t("settings.devEnv.tools.dotnetMajorLabel")
          }}</label>
          <Select :model-value="dotnetMajor" @update:model-value="onDotnetMajorChange">
            <SelectTrigger>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectGroup>
                <SelectItem v-for="major in DOTNET_MAJORS" :key="major" :value="major">
                  .NET {{ major }}
                </SelectItem>
              </SelectGroup>
            </SelectContent>
          </Select>
        </div>
        <DialogFooter>
          <Button type="button" @click="confirmDotnetInstall">
            {{ t("settings.devEnv.tools.install") }}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </section>
</template>
