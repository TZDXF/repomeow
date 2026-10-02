<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import { ChevronRight, LoaderCircle, SquareTerminal, Trash2 } from "@lucide/vue";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import ScrollArea from "@/components/common/ScrollArea.vue";
import { formatRelativeTime } from "@/lib/format";
import { terminalStatusDotClass } from "@/lib/terminal";
import { useTerminalStore } from "@/stores/terminal";
import type { TerminalSessionInfo } from "@/types";

/**
 * 标题栏右侧的内嵌终端会话入口:图标显示运行中数量,
 * 点击弹出命令会话(npm/docker/custom 等,运行中与历史),按项目分组;
 * 主动创建的交互式 Shell 不在此展示,只在项目终端面板可见;
 * 点击会话跳转到对应项目详情页并展开终端面板定位到该会话。
 */
const { t } = useI18n();
const router = useRouter();
const store = useTerminalStore();
const open = ref(false);

onMounted(() => {
  void store.init();
});

/** 交互式 Shell 由用户在面板手动创建,全局概览只关心命令会话 */
const sessions = computed(() => store.sessions.filter((s) => !s.interactive));
const runningCount = computed(() => sessions.value.filter((s) => s.status === "running").length);
const hasFinished = computed(() => sessions.value.some((s) => s.status !== "running"));
const visible = computed(() => sessions.value.length > 0);

interface ProjectGroup {
  projectId: number;
  projectName: string;
  running: number;
  sessions: TerminalSessionInfo[];
}

/** 按项目分组:组内保持全局排序(运行中优先),组间运行中优先、其余按最近启动倒序 */
const groups = computed<ProjectGroup[]>(() => {
  const map = new Map<number, ProjectGroup>();
  for (const s of sessions.value) {
    let g = map.get(s.project_id);
    if (!g) {
      g = { projectId: s.project_id, projectName: s.project_name, running: 0, sessions: [] };
      map.set(s.project_id, g);
    }
    g.sessions.push(s);
    if (s.status === "running") g.running++;
  }
  return [...map.values()].sort(
    (a, b) => b.running - a.running || b.sessions[0].started_at - a.sessions[0].started_at,
  );
});

function statusText(s: TerminalSessionInfo): string {
  const base = t(`terminal.status.${s.status}`);
  return s.status === "exited" && s.exit_code !== null
    ? `${base} · ${t("terminal.exitCode", { code: s.exit_code })}`
    : base;
}

/** 运行中看启动时间,已结束看结束时间(无结束时间的异常态退回启动时间) */
function timeText(s: TerminalSessionInfo): string {
  const ts = s.status === "running" ? s.started_at : (s.finished_at ?? s.started_at);
  return formatRelativeTime(ts);
}

async function openSession(s: TerminalSessionInfo) {
  // 先定位再跳转:同页时面板直接展开选中,跨页时详情页挂载后按 activeId 归位
  store.activeId = s.id;
  store.open = true;
  open.value = false;
  await router.push(`/projects/${s.project_id}`);
}

async function clearFinished() {
  await store.clearFinished(undefined, { excludeInteractive: true });
}
</script>

<template>
  <Popover v-if="visible" v-model:open="open">
    <PopoverTrigger as-child>
      <button
        type="button"
        class="relative flex w-11 items-center justify-center text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
        :title="t('terminal.menuTitle')"
        @mousedown.stop
        @dblclick.stop
      >
        <LoaderCircle v-if="runningCount" class="h-4 w-4 animate-spin text-primary" />
        <SquareTerminal v-else class="h-4 w-4" />
        <span
          v-if="runningCount"
          class="absolute right-1.5 top-1.5 flex h-3.5 min-w-3.5 items-center justify-center rounded-full bg-primary px-1 text-[10px] font-semibold leading-none text-primary-foreground tabular-nums"
        >
          {{ runningCount }}
        </span>
      </button>
    </PopoverTrigger>

    <PopoverContent align="end" class="z-[70] w-96 gap-0 p-0" @mousedown.stop>
      <div class="flex items-center justify-between border-b px-3 py-2.5">
        <div>
          <p class="font-medium">{{ t("terminal.menuTitle") }}</p>
          <p class="text-xs text-muted-foreground">{{ t("terminal.menuHint") }}</p>
        </div>
        <span v-if="runningCount" class="text-xs text-muted-foreground">
          {{ t("terminal.runningCount", { count: runningCount }) }}
        </span>
      </div>

      <ScrollArea class="max-h-96 p-2">
        <section v-for="g in groups" :key="g.projectId" class="[&:not(:first-child)]:mt-3">
          <h3
            class="flex items-center gap-1.5 px-1 pb-1.5 text-xs font-medium text-muted-foreground"
          >
            <span class="min-w-0 flex-1 truncate">{{ g.projectName }}</span>
            <span v-if="g.running" class="shrink-0 text-primary">
              {{ t("terminal.runningCount", { count: g.running }) }}
            </span>
          </h3>
          <button
            v-for="s in g.sessions"
            :key="s.id"
            type="button"
            class="group flex w-full items-center gap-2 rounded-md px-2 py-2 text-left hover:bg-accent"
            @click="openSession(s)"
          >
            <span class="h-1.5 w-1.5 shrink-0 rounded-full" :class="terminalStatusDotClass(s)" />
            <div class="min-w-0 flex-1">
              <div class="flex items-center gap-2">
                <span class="min-w-0 flex-1 truncate text-sm">{{ s.label }}</span>
                <span class="shrink-0 text-xs text-muted-foreground">{{ statusText(s) }}</span>
              </div>
              <p class="mt-0.5 truncate text-xs text-muted-foreground" :title="s.command">
                {{ s.command }} · {{ timeText(s) }}
              </p>
            </div>
            <ChevronRight class="h-4 w-4 shrink-0 text-muted-foreground" />
          </button>
        </section>
      </ScrollArea>

      <div v-if="hasFinished" class="flex justify-end border-t px-3 py-2">
        <button
          type="button"
          class="flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground"
          @click="clearFinished"
        >
          <Trash2 class="h-3 w-3" />
          {{ t("terminal.clearFinished") }}
        </button>
      </div>
    </PopoverContent>
  </Popover>
</template>
