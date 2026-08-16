<script setup lang="ts">
// 托盘弹窗项目行下方行内展开的「常用命令」列表:行均不可点击,点运行图标才执行。
// npm/自定义命令与详情页 ScriptItem 对齐:行首常显绿色 Play 按钮(text-emerald-600)执行,
// 自定义命令带图标时按钮内渲染其自定义图标(与 ScriptItem 一致);
// compose 条目与详情页 DockerCompose 对齐:绿色 Play / 红色 Square 独立按钮执行
// (文件 up -d / down,服务 up -d / stop),下拉菜单 build/buildUp/restart 彩色图标一致;
// 服务行缩进(pl-7)嵌套在所属文件下,按钮常显(托盘为瞬态窗口,不做 hover 显隐)
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { Container, FileCode, Hammer, MoreHorizontal, Play, RotateCw, Square } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { commandIcon } from "@/lib/command-icons";
import { resolveJavaHome } from "@/lib/jdk";
import { runInTerminal } from "@/lib/tauri";
import { useSettingsStore } from "@/stores/settings";
import type { PinnedCommand, Project } from "@/types";

const { t } = useI18n();
const props = defineProps<{ project: Project; pins: PinnedCommand[] }>();
// 托盘窗口在 TrayPopup 挂载时已 init,且每次弹窗显示时经 tray-popup://refresh 从 localStorage
// 补读 JDK 配置;javaBuild 标记执行时按项目解析 JAVA_HOME
const settingsStore = useSettingsStore();

type ComposeAction = "up -d" | "up -d --build" | "build" | "restart" | "down" | "stop";

/** 下拉菜单动作:与详情页一致,up/down/stop 有专属按钮不进菜单;服务级不含 down */
const MENU_ACTIONS: ComposeAction[] = ["build", "up -d --build", "restart"];

/** 菜单图标与颜色同详情页:build 蓝、buildUp 绿、restart 黄 */
const MENU_ICONS: Record<ComposeAction, typeof Play> = {
  "up -d": Play,
  build: Hammer,
  "up -d --build": Hammer,
  restart: RotateCw,
  down: Square,
  stop: Square,
};
const MENU_ICON_CLASSES: Record<ComposeAction, string> = {
  "up -d": "text-emerald-600",
  build: "text-sky-600",
  "up -d --build": "text-emerald-600",
  restart: "text-amber-600",
  down: "text-red-600",
  stop: "text-red-600",
};
const MENU_LABEL_KEYS: Record<ComposeAction, string> = {
  "up -d": "docker.up",
  build: "docker.build",
  "up -d --build": "docker.buildUp",
  restart: "docker.restart",
  down: "docker.down",
  stop: "docker.stop",
};

const isCompose = (p: PinnedCommand) => p.kind === "composeFile" || p.kind === "composeService";

/** composeService 的 target_key 为 "<file>\n<service>",取文件路径 / 服务名 */
const fileOf = (p: PinnedCommand) => p.target_key.split("\n")[0];
const serviceOf = (p: PinnedCommand) => p.target_key.split("\n")[1];

interface PinEntry {
  pin: PinnedCommand;
  /** 嵌套在该文件下的服务标记(与详情页层级一致) */
  services: PinnedCommand[];
}

/**
 * 把扁平的 pins 组织成展示条目:compose 服务归入所属文件条目下;
 * 文件本身未被标记的服务(孤儿)保持独立条目,行内显示文件路径提示
 */
const entries = computed<PinEntry[]>(() => {
  const pinnedFiles = new Set(
    props.pins.filter((p) => p.kind === "composeFile").map((p) => p.target_key),
  );
  const out: PinEntry[] = [];
  for (const p of props.pins) {
    if (p.kind === "composeService" && pinnedFiles.has(fileOf(p))) {
      continue;
    }
    if (p.kind === "composeFile") {
      out.push({
        pin: p,
        services: props.pins.filter(
          (s) => s.kind === "composeService" && fileOf(s) === p.target_key,
        ),
      });
    } else {
      out.push({ pin: p, services: [] });
    }
  }
  return out;
});

function kindIcon(p: PinnedCommand) {
  return p.kind === "composeFile" ? FileCode : Container;
}

/** npm/自定义命令行首执行按钮图标:自定义命令带图标时与 ScriptItem 一致渲染自定义图标 */
function runIcon(p: PinnedCommand) {
  return p.kind === "customCommand" && p.icon ? commandIcon(p.icon) : undefined;
}

/** npm/自定义/javaBuild 命令:点行首 Play 按钮执行存好的命令(与详情页一致) */
async function runPinned(p: PinnedCommand) {
  try {
    // cwd 存的是相对项目根的目录(monorepo 子包),执行时用当前 project.path 拼接,迁移目录后仍可用
    const cwd = p.cwd ? `${props.project.path}/${p.cwd}` : undefined;
    // Spring Boot 标记命令注入项目选择的 JAVA_HOME(与详情页卡片同一解析逻辑)
    const javaHome =
      p.kind === "javaBuild" ? resolveJavaHome(settingsStore, props.project.id) : undefined;
    await runInTerminal(props.project, p.command, cwd, javaHome);
    toast.success(t("pins.started", { name: p.label }));
  } catch (e) {
    toast.error(String(e));
  }
}

/** compose 条目执行指定动作(command 为基础前缀,在此拼接动作与服务名) */
async function runCompose(p: PinnedCommand, action: ComposeAction) {
  const service = p.kind === "composeService" ? serviceOf(p) : undefined;
  const command = `${p.command} ${service ? `${action} ${service}` : action}`;
  try {
    await runInTerminal(props.project, command);
    toast.success(t("pins.started", { name: p.label }));
  } catch (e) {
    toast.error(String(e));
  }
}
</script>

<template>
  <div v-if="pins.length" class="flex flex-col gap-0.5 pb-1 pl-4">
    <template v-for="e in entries" :key="e.pin.id">
      <!-- npm / 自定义命令:与详情页 ScriptItem 对齐,行不可点击,行首绿色 Play 按钮执行 -->
      <div
        v-if="!isCompose(e.pin)"
        class="flex items-center gap-1.5 rounded-md px-2 py-1 transition-colors hover:bg-accent"
      >
        <Button
          variant="ghost"
          size="icon"
          class="h-6 w-6 shrink-0 text-emerald-600"
          :title="e.pin.command"
          @click.stop="runPinned(e.pin)"
        >
          <component :is="runIcon(e.pin)" v-if="runIcon(e.pin)" class="h-3.5 w-3.5" />
          <Play v-else class="h-3.5 w-3.5" />
        </Button>
        <span class="min-w-0 flex-1 truncate text-xs" :title="e.pin.command">
          {{ e.pin.label }}
        </span>
      </div>
      <!-- compose 文件/孤儿服务:与详情页对齐,行不可点击,Play/Square/菜单独立按钮常显 -->
      <div
        v-else
        class="flex items-center gap-1.5 rounded-md px-2 py-1 transition-colors hover:bg-accent"
      >
        <component :is="kindIcon(e.pin)" class="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
        <span
          class="min-w-0 flex-1 truncate font-mono text-xs"
          :title="e.pin.kind === 'composeFile' ? e.pin.target_key : e.pin.command"
        >
          {{ e.pin.label }}
        </span>
        <span
          v-if="e.pin.kind === 'composeService'"
          class="shrink-0 truncate font-mono text-[10px] text-muted-foreground"
          :title="fileOf(e.pin)"
        >
          {{ fileOf(e.pin) }}
        </span>
        <Button
          variant="ghost"
          size="icon"
          class="h-6 w-6 shrink-0 text-emerald-600"
          :title="t('docker.up')"
          @click.stop="runCompose(e.pin, 'up -d')"
        >
          <Play class="h-3.5 w-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          class="h-6 w-6 shrink-0 text-red-600"
          :title="t(e.pin.kind === 'composeFile' ? 'docker.down' : 'docker.stop')"
          @click.stop="runCompose(e.pin, e.pin.kind === 'composeFile' ? 'down' : 'stop')"
        >
          <Square class="h-3.5 w-3.5" />
        </Button>
        <DropdownMenu>
          <DropdownMenuTrigger as-child>
            <Button
              variant="ghost"
              size="icon"
              class="h-6 w-6 shrink-0 text-muted-foreground"
              :title="t('docker.more')"
              @click.stop
            >
              <MoreHorizontal class="h-3.5 w-3.5" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" class="w-36">
            <DropdownMenuItem
              v-for="action in MENU_ACTIONS"
              :key="action"
              class="gap-2 text-xs"
              @click.stop="runCompose(e.pin, action)"
            >
              <component
                :is="MENU_ICONS[action]"
                class="h-3.5 w-3.5"
                :class="MENU_ICON_CLASSES[action]"
              />
              {{ t(MENU_LABEL_KEYS[action]) }}
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
      <!-- 嵌套的服务行:缩进与详情页一致,按钮常显 -->
      <div
        v-for="s in e.services"
        :key="s.id"
        class="flex items-center gap-1.5 rounded-md px-2 py-1 pl-7 transition-colors hover:bg-accent"
      >
        <span class="min-w-0 flex-1 truncate font-mono text-xs" :title="s.label">
          {{ s.label }}
        </span>
        <Button
          variant="ghost"
          size="icon"
          class="h-6 w-6 shrink-0 text-emerald-600"
          :title="t('docker.up')"
          @click.stop="runCompose(s, 'up -d')"
        >
          <Play class="h-3.5 w-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          class="h-6 w-6 shrink-0 text-red-600"
          :title="t('docker.stop')"
          @click.stop="runCompose(s, 'stop')"
        >
          <Square class="h-3.5 w-3.5" />
        </Button>
        <DropdownMenu>
          <DropdownMenuTrigger as-child>
            <Button
              variant="ghost"
              size="icon"
              class="h-6 w-6 shrink-0 text-muted-foreground"
              :title="t('docker.more')"
              @click.stop
            >
              <MoreHorizontal class="h-3.5 w-3.5" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" class="w-36">
            <DropdownMenuItem
              v-for="action in MENU_ACTIONS"
              :key="action"
              class="gap-2 text-xs"
              @click.stop="runCompose(s, action)"
            >
              <component
                :is="MENU_ICONS[action]"
                class="h-3.5 w-3.5"
                :class="MENU_ICON_CLASSES[action]"
              />
              {{ t(MENU_LABEL_KEYS[action]) }}
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
    </template>
  </div>
</template>
