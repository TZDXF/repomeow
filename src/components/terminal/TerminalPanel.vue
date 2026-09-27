<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import {
  Check,
  ChevronDown,
  ChevronUp,
  LoaderCircle,
  Plus,
  RotateCcw,
  Square,
  SquareTerminal,
  Trash2,
  X,
} from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import TerminalView from "@/components/terminal/TerminalView.vue";
import {
  getTerminalCapabilities,
  terminalStatusDotClass,
  type TerminalCapabilities,
} from "@/lib/terminal";
import { useSettingsStore, type TerminalKind } from "@/stores/settings";
import { useTerminalStore } from "@/stores/terminal";
import type { Project, TerminalSessionInfo } from "@/types";

/**
 * 项目详情页底部内嵌终端面板:按项目过滤会话,页签可拖拽排序,
 * 顶部边缘拖拽调整高度;交互式 Shell 无停止/重启按钮;收起为底部细条。
 */
const props = defineProps<{ project: Project }>();

const { t } = useI18n();
const settings = useSettingsStore();
const store = useTerminalStore();
const creating = ref(false);
/** Windows 下各 shell 的可用性(决定新建菜单项是否禁用);非 Windows 不显示类型选择 */
const capabilities = ref<TerminalCapabilities | null>(null);

onMounted(() => {
  void store.init();
  void getTerminalCapabilities().then((c) => (capabilities.value = c));
});

/** 新建终端可选的 shell 类型(仅 Windows 展示菜单) */
const shellChoices: { kind: TerminalKind; label: string }[] = [
  { kind: "cmd", label: "cmd" },
  { kind: "powershell", label: "PowerShell" },
  { kind: "gitbash", label: "Git Bash" },
];
const showShellPicker = computed(() => capabilities.value?.isWindows === true);

/** 当前项目的会话(会话按 project_id 归属,worktree 副本共享同一 id) */
const list = computed(() => store.sessions.filter((s) => s.project_id === props.project.id));
/** 应用手工拖拽排序后的展示顺序:排序表内的优先,新会话按默认序附后 */
const orderedList = computed(() => {
  const order = store.tabOrders.get(props.project.id);
  if (!order?.length) return list.value;
  const byId = new Map(list.value.map((s) => [s.id, s]));
  const used = new Set<number>();
  const out: TerminalSessionInfo[] = [];
  for (const id of order) {
    const s = byId.get(id);
    if (s && !used.has(id)) {
      out.push(s);
      used.add(id);
    }
  }
  for (const s of list.value) {
    if (!used.has(s.id)) out.push(s);
  }
  return out;
});
const runningCount = computed(() => list.value.filter((s) => s.status === "running").length);
const hasFinished = computed(() => list.value.some((s) => s.status !== "running"));

// 内嵌终端关闭且无任何会话时不渲染,避免底部常驻一条无用细条
const visible = computed(() => settings.embeddedTerminal || list.value.length > 0);

const active = computed(
  () => list.value.find((s) => s.id === store.activeId) ?? list.value[0] ?? null,
);

// 选中会话被移除/切换项目时,顺延到当前项目首个会话
watch(
  () => [props.project.id, list.value.length],
  () => {
    if (active.value && store.activeId !== active.value.id) {
      store.activeId = active.value.id;
    }
  },
  { immediate: true },
);

// ── 面板高度:顶部边缘拖拽 ─────────────────────────────────────────

const MIN_PANEL_HEIGHT = 160;

function startResize(e: PointerEvent) {
  const handle = e.currentTarget as HTMLElement;
  const startY = e.clientY;
  const startHeight = store.panelHeight;
  const maxHeight = () => Math.round(window.innerHeight * 0.85);
  handle.setPointerCapture(e.pointerId);
  const onMove = (ev: PointerEvent) => {
    const next = startHeight + (startY - ev.clientY);
    store.panelHeight = Math.min(Math.max(next, MIN_PANEL_HEIGHT), maxHeight());
  };
  const onUp = () => {
    handle.removeEventListener("pointermove", onMove);
    handle.removeEventListener("pointerup", onUp);
    handle.removeEventListener("pointercancel", onUp);
  };
  handle.addEventListener("pointermove", onMove);
  handle.addEventListener("pointerup", onUp);
  handle.addEventListener("pointercancel", onUp);
}

// ── 页签拖拽排序 ─────────────────────────────────────────────────

const draggingId = ref<number | null>(null);
const dropTargetId = ref<number | null>(null);

function onTabDragStart(id: number, e: DragEvent) {
  draggingId.value = id;
  if (e.dataTransfer) {
    e.dataTransfer.effectAllowed = "move";
    e.dataTransfer.setData("text/plain", String(id));
  }
}

function onTabDragOver(id: number, e: DragEvent) {
  if (draggingId.value === null || draggingId.value === id) return;
  e.preventDefault();
  dropTargetId.value = id;
}

function onTabDrop(id: number, e: DragEvent) {
  e.preventDefault();
  const from = draggingId.value;
  if (from === null || from === id) return;
  const ids = orderedList.value.map((s) => s.id);
  const fromIdx = ids.indexOf(from);
  const toIdx = ids.indexOf(id);
  if (fromIdx < 0 || toIdx < 0) return;
  ids.splice(toIdx, 0, ...ids.splice(fromIdx, 1));
  store.setTabOrder(props.project.id, ids);
}

function onTabDragEnd() {
  draggingId.value = null;
  dropTargetId.value = null;
}

// ── 会话操作 ─────────────────────────────────────────────────────

async function createTerminal(shell?: TerminalKind) {
  if (creating.value) return;
  creating.value = true;
  try {
    await store.create(props.project, shell);
  } catch (e) {
    toast.error(String(e));
  } finally {
    creating.value = false;
  }
}

async function stopActive() {
  if (!active.value) return;
  try {
    await store.stop(active.value.id);
  } catch (e) {
    toast.error(String(e));
  }
}

async function restartActive() {
  if (!active.value) return;
  try {
    await store.restart(active.value.id);
  } catch (e) {
    toast.error(String(e));
  }
}

async function removeSession(id: number) {
  try {
    await store.remove(id);
  } catch (e) {
    toast.error(String(e));
  }
}

async function clearFinished() {
  try {
    await store.clearFinished(props.project.id);
  } catch (e) {
    toast.error(String(e));
  }
}
</script>

<template>
  <div v-if="visible" class="shrink-0">
    <!-- 收起态:底部细条,显示运行中数量 -->
    <button
      v-if="!store.open"
      type="button"
      class="flex h-8 w-full items-center gap-2 border-t px-3 text-xs text-muted-foreground transition-colors hover:bg-accent"
      :title="t('terminal.expand')"
      @click="store.open = true"
    >
      <SquareTerminal class="h-3.5 w-3.5" />
      <span>{{ t("terminal.title") }}</span>
      <span
        v-if="runningCount"
        class="flex items-center gap-1 text-emerald-600 dark:text-emerald-400"
      >
        <LoaderCircle class="h-3 w-3 animate-spin" />
        {{ t("terminal.runningCount", { count: runningCount }) }}
      </span>
      <ChevronUp class="ml-auto h-3.5 w-3.5" />
    </button>

    <!-- 展开态:顶部拖拽条 + 会话页签 + xterm -->
    <div v-else class="flex flex-col" :style="{ height: `${store.panelHeight}px` }">
      <!-- 顶部拖拽条:自身即顶部分隔线,悬停高亮,避免与边框叠成两条线 -->
      <div
        class="h-1.5 shrink-0 cursor-row-resize border-t transition-colors hover:border-primary"
        :title="t('terminal.resize')"
        @pointerdown="startResize"
      />
      <div class="flex h-9 shrink-0 items-center gap-1 border-b px-2">
        <SquareTerminal class="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
        <div class="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto">
          <div
            v-for="s in orderedList"
            :key="s.id"
            draggable="true"
            class="group flex h-7 shrink-0 cursor-pointer items-center gap-1.5 rounded-md px-2 text-xs text-muted-foreground transition-colors hover:bg-accent"
            :class="[
              active?.id === s.id && 'bg-accent text-foreground',
              draggingId === s.id && 'opacity-50',
              dropTargetId === s.id &&
                draggingId !== s.id &&
                'shadow-[inset_2px_0_0_0_var(--primary)]',
            ]"
            :title="`${s.command} · ${t(`terminal.status.${s.status}`)}`"
            @click="store.activeId = s.id"
            @dragstart="onTabDragStart(s.id, $event)"
            @dragover="onTabDragOver(s.id, $event)"
            @drop="onTabDrop(s.id, $event)"
            @dragend="onTabDragEnd"
          >
            <span class="h-1.5 w-1.5 shrink-0 rounded-full" :class="terminalStatusDotClass(s)" />
            <span class="max-w-40 truncate">{{ s.label }}</span>
            <span
              v-if="s.status === 'exited' && s.exit_code !== null && s.exit_code !== 0"
              class="shrink-0 text-red-500"
            >
              {{ s.exit_code }}
            </span>
            <!-- 关闭按钮常驻占位,悬停仅切换透明度,避免页签宽度跳变 -->
            <button
              type="button"
              class="pointer-events-none flex h-4 w-4 shrink-0 items-center justify-center rounded-sm text-muted-foreground opacity-0 transition-opacity hover:text-foreground focus-visible:opacity-100 group-hover:pointer-events-auto group-hover:opacity-100"
              :title="t('terminal.remove')"
              @click.stop="removeSession(s.id)"
            >
              <X class="h-3 w-3" />
            </button>
          </div>
          <!-- 新建按钮:固定在最右一个页签之后;Windows 下可下拉选择 shell 类型 -->
          <DropdownMenu v-if="settings.embeddedTerminal && showShellPicker">
            <DropdownMenuTrigger as-child>
              <Button
                variant="ghost"
                size="icon"
                class="h-7 w-7 shrink-0"
                :disabled="creating"
                :title="t('terminal.create')"
                :aria-label="t('terminal.create')"
              >
                <LoaderCircle v-if="creating" class="h-3.5 w-3.5 animate-spin" />
                <Plus v-else class="h-3.5 w-3.5" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start" class="w-44">
              <DropdownMenuItem
                v-for="choice in shellChoices"
                :key="choice.kind"
                class="flex items-center justify-between gap-2"
                :disabled="capabilities?.shells[choice.kind] === false"
                @click="createTerminal(choice.kind)"
              >
                <span>{{ choice.label }}</span>
                <Check
                  v-if="settings.terminal === choice.kind"
                  class="h-3.5 w-3.5 text-muted-foreground"
                />
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
          <Button
            v-else-if="settings.embeddedTerminal"
            variant="ghost"
            size="icon"
            class="h-7 w-7 shrink-0"
            :disabled="creating"
            :title="t('terminal.create')"
            :aria-label="t('terminal.create')"
            @click="createTerminal()"
          >
            <LoaderCircle v-if="creating" class="h-3.5 w-3.5 animate-spin" />
            <Plus v-else class="h-3.5 w-3.5" />
          </Button>
        </div>
        <!-- 交互式 Shell 没有停止/启动状态,仅命令会话展示停止与重启 -->
        <template v-if="active && !active.interactive">
          <Button
            v-if="active.status === 'running'"
            variant="ghost"
            size="icon"
            class="h-7 w-7 shrink-0"
            :title="t('terminal.stop')"
            @click="stopActive"
          >
            <Square class="h-3.5 w-3.5" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            class="h-7 w-7 shrink-0"
            :title="t('terminal.restart')"
            @click="restartActive"
          >
            <RotateCcw class="h-3.5 w-3.5" />
          </Button>
        </template>
        <Button
          v-if="hasFinished"
          variant="ghost"
          size="icon"
          class="h-7 w-7 shrink-0"
          :title="t('terminal.clearFinished')"
          @click="clearFinished"
        >
          <Trash2 class="h-3.5 w-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          class="h-7 w-7 shrink-0"
          :title="t('terminal.collapse')"
          @click="store.open = false"
        >
          <ChevronDown class="h-3.5 w-3.5" />
        </Button>
      </div>
      <div class="min-h-0 flex-1">
        <TerminalView v-if="active" :key="active.id" :session-id="active.id" />
        <div
          v-else
          class="flex h-full flex-col items-center justify-center gap-1 text-sm text-muted-foreground"
        >
          <p>{{ t("terminal.empty") }}</p>
          <p class="text-xs">{{ t("terminal.emptyHint") }}</p>
        </div>
      </div>
    </div>
  </div>
</template>
