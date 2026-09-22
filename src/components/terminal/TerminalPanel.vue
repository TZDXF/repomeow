<script setup lang="ts">
import { computed, onMounted, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import {
  ChevronDown,
  ChevronUp,
  LoaderCircle,
  RotateCcw,
  Square,
  SquareTerminal,
  Trash2,
  X,
} from "@lucide/vue";
import { Button } from "@/components/ui/button";
import TerminalView from "@/components/terminal/TerminalView.vue";
import { terminalStatusDotClass } from "@/lib/terminal";
import { useSettingsStore } from "@/stores/settings";
import { useTerminalStore } from "@/stores/terminal";

/**
 * 项目详情页底部内嵌终端面板:按项目过滤会话,页签切换,
 * 支持停止/重启/移除/清除已结束;收起为底部细条。
 */
const props = defineProps<{ projectId: number }>();

const { t } = useI18n();
const settings = useSettingsStore();
const store = useTerminalStore();

onMounted(() => {
  void store.init();
});

/** 当前项目的会话(会话按 project_id 归属,worktree 副本共享同一 id) */
const list = computed(() => store.sessions.filter((s) => s.project_id === props.projectId));
const runningCount = computed(() => list.value.filter((s) => s.status === "running").length);
const hasFinished = computed(() => list.value.some((s) => s.status !== "running"));

// 内嵌终端关闭且无任何会话时不渲染,避免底部常驻一条无用细条
const visible = computed(() => settings.embeddedTerminal || list.value.length > 0);

const active = computed(
  () => list.value.find((s) => s.id === store.activeId) ?? list.value[0] ?? null,
);

// 选中会话被移除/切换项目时,顺延到当前项目首个会话
watch(
  () => [props.projectId, list.value.length],
  () => {
    if (active.value && store.activeId !== active.value.id) {
      store.activeId = active.value.id;
    }
  },
  { immediate: true },
);

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
    await store.clearFinished(props.projectId);
  } catch (e) {
    toast.error(String(e));
  }
}
</script>

<template>
  <div v-if="visible" class="shrink-0 border-t">
    <!-- 收起态:底部细条,显示运行中数量 -->
    <button
      v-if="!store.open"
      type="button"
      class="flex h-8 w-full items-center gap-2 px-3 text-xs text-muted-foreground transition-colors hover:bg-accent"
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

    <!-- 展开态:会话页签 + xterm -->
    <div v-else class="flex h-72 flex-col">
      <div class="flex h-9 shrink-0 items-center gap-1 border-b px-2">
        <SquareTerminal class="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
        <div class="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto">
          <div
            v-for="s in list"
            :key="s.id"
            class="group flex h-7 shrink-0 cursor-pointer items-center gap-1.5 rounded-md px-2 text-xs text-muted-foreground transition-colors hover:bg-accent"
            :class="active?.id === s.id && 'bg-accent text-foreground'"
            :title="`${s.command} · ${t(`terminal.status.${s.status}`)}`"
            @click="store.activeId = s.id"
          >
            <span class="h-1.5 w-1.5 shrink-0 rounded-full" :class="terminalStatusDotClass(s)" />
            <span class="max-w-40 truncate">{{ s.label }}</span>
            <span
              v-if="s.status === 'exited' && s.exit_code !== null && s.exit_code !== 0"
              class="shrink-0 text-red-500"
            >
              {{ s.exit_code }}
            </span>
            <button
              type="button"
              class="hidden h-4 w-4 shrink-0 items-center justify-center rounded-sm text-muted-foreground hover:text-foreground group-hover:inline-flex"
              :title="t('terminal.remove')"
              @click.stop="removeSession(s.id)"
            >
              <X class="h-3 w-3" />
            </button>
          </div>
        </div>
        <template v-if="active">
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
