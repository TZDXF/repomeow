<script setup lang="ts">
import { computed, ref, watch } from "vue";
import type { Component } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import {
  BookOpenText,
  CheckCircle2,
  ChevronRight,
  Clock3,
  FileText,
  GitMerge,
  History,
  LoaderCircle,
  Trash2,
} from "@lucide/vue";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { formatRelativeTime } from "@/lib/format";
import {
  useBackgroundTasksStore,
  type BackgroundTaskItem,
  type BackgroundTaskKind,
  type FrontendBackgroundTask,
} from "@/stores/background-tasks";
import { useBatchReportStore } from "@/stores/batch-report";
import { useWikiStore } from "@/stores/wiki";

const { t } = useI18n();
const router = useRouter();
const store = useBackgroundTasksStore();
const batchReportStore = useBatchReportStore();
const wikiStore = useWikiStore();
const open = ref(false);

interface TaskGroup {
  key: BackgroundTaskKind;
  label: string;
  count: number;
  progress: string;
}

const KIND_ICONS: Record<BackgroundTaskKind, Component> = {
  report: FileText,
  wiki: BookOpenText,
  conflict: GitMerge,
};

function kindLabel(kind: BackgroundTaskKind): string {
  return {
    report: t("titleBar.reportTask"),
    wiki: t("titleBar.wikiTask"),
    conflict: t("titleBar.conflictTask"),
  }[kind];
}

function progressLabel(task: Pick<BackgroundTaskItem, "completed" | "total">): string {
  return task.total > 0 ? `${task.completed}/${task.total}` : t("titleBar.running");
}

const frontendTasks = computed<FrontendBackgroundTask[]>(() => {
  const tasks: FrontendBackgroundTask[] = [];
  if (batchReportStore.running) {
    tasks.push({
      id: "batch-report",
      kind: "report",
      label: t("report.batchFloatTitle"),
      completed: batchReportStore.stats.finished,
      total: batchReportStore.stats.total,
    });
  }
  for (const task of wikiStore.backgroundTasks) {
    const action = {
      generate: t("titleBar.wikiGenerateTask"),
      update: t("titleBar.wikiUpdateTask"),
      page: t("titleBar.wikiPageTask"),
    }[task.action];
    tasks.push({
      id: task.id,
      kind: "wiki",
      label: `${task.projectName} · ${action}`,
      completed: task.completed,
      total: task.total,
      target:
        task.projectId === undefined ? undefined : { kind: "wiki", projectId: task.projectId },
    });
  }
  return tasks;
});

watch(
  frontendTasks,
  (tasks) => {
    store.syncFrontendTasks("title-bar", tasks);
  },
  { deep: true, immediate: true },
);

watch(open, (isOpen) => {
  if (isOpen) {
    store.refreshHistory();
  }
});

const taskGroups = computed<TaskGroup[]>(() => {
  const kinds: BackgroundTaskKind[] = ["report", "wiki", "conflict"];
  return kinds.flatMap((kind) => {
    const tasks = store.tasks.filter((task) => task.kind === kind);
    if (!tasks.length) {
      return [];
    }
    const determinate = tasks.filter((task) => task.total > 0);
    const completed = determinate.reduce((sum, task) => sum + task.completed, 0);
    const total = determinate.reduce((sum, task) => sum + task.total, 0);
    let progress = t("titleBar.running");
    if (determinate.length === tasks.length) {
      progress = `${completed}/${total}`;
    } else if (total > 0) {
      progress = `${completed}/${total}+`;
    }
    return [{ key: kind, label: kindLabel(kind), count: tasks.length, progress }];
  });
});

const taskCount = computed(() => store.tasks.length);
const visible = computed(() => taskCount.value > 0 || store.history.length > 0);

function progressPercent(task: BackgroundTaskItem): number {
  if (task.total <= 0) {
    return 0;
  }
  return Math.max(0, Math.min(100, (task.completed / task.total) * 100));
}

function finishedTime(task: BackgroundTaskItem): string {
  return formatRelativeTime(task.finishedAt === null ? null : task.finishedAt / 1000);
}

async function openTask(task: BackgroundTaskItem) {
  if (!task.target) {
    return;
  }
  const suffix = task.target.kind === "wiki" ? "/wiki" : "";
  await router.push(`/projects/${task.target.projectId}${suffix}`);
  open.value = false;
}
</script>

<template>
  <Popover v-if="visible" v-model:open="open">
    <PopoverTrigger as-child>
      <button
        type="button"
        class="flex min-w-0 items-center gap-1.5 rounded px-1.5 py-0.5 font-normal transition-colors hover:bg-accent hover:text-foreground"
        :title="t('titleBar.taskCenter')"
        @mousedown.stop
        @dblclick.stop
      >
        <template v-if="taskCount">
          <span class="flex shrink-0 items-center gap-1 text-foreground/80">
            <LoaderCircle class="h-3.5 w-3.5 animate-spin text-primary" />
            {{ t("titleBar.backgroundTasks", { count: taskCount }) }}
          </span>
          <span
            v-for="group in taskGroups"
            :key="group.key"
            class="max-w-36 truncate rounded bg-muted px-1.5 py-0.5 tabular-nums text-muted-foreground"
          >
            {{ group.label }}<template v-if="group.count > 1"> ×{{ group.count }}</template>
            {{ group.progress }}
          </span>
        </template>
        <template v-else>
          <History class="h-3.5 w-3.5 text-muted-foreground" />
          <span>{{ t("titleBar.recentTasks", { count: store.history.length }) }}</span>
        </template>
      </button>
    </PopoverTrigger>

    <PopoverContent align="start" class="z-[70] w-96 gap-0 p-0" @mousedown.stop>
      <div class="flex items-center justify-between border-b px-3 py-2.5">
        <div>
          <p class="font-medium">{{ t("titleBar.taskCenter") }}</p>
          <p class="text-xs text-muted-foreground">{{ t("titleBar.historyHint") }}</p>
        </div>
        <span v-if="taskCount" class="text-xs text-muted-foreground">
          {{ t("titleBar.backgroundTasks", { count: taskCount }) }}
        </span>
      </div>

      <div class="max-h-96 overflow-y-auto p-2">
        <section v-if="store.tasks.length">
          <h3 class="px-1 pb-1.5 text-xs font-medium text-muted-foreground">
            {{ t("titleBar.activeTasks") }}
          </h3>
          <button
            v-for="task in store.tasks"
            :key="task.id"
            type="button"
            class="group flex w-full items-center gap-2 rounded-md px-2 py-2 text-left hover:bg-accent disabled:cursor-default disabled:hover:bg-transparent"
            :disabled="!task.target"
            @click="openTask(task)"
          >
            <component :is="KIND_ICONS[task.kind]" class="h-4 w-4 shrink-0 text-primary" />
            <div class="min-w-0 flex-1">
              <div class="flex items-center gap-2">
                <span class="shrink-0 text-xs text-muted-foreground">{{
                  kindLabel(task.kind)
                }}</span>
                <span class="min-w-0 flex-1 truncate text-sm">{{ task.label }}</span>
                <span class="shrink-0 text-xs tabular-nums text-muted-foreground">
                  {{ progressLabel(task) }}
                </span>
              </div>
              <div v-if="task.total > 0" class="mt-1.5 h-1 overflow-hidden rounded-full bg-muted">
                <div
                  class="h-full rounded-full bg-primary transition-[width] duration-300"
                  :style="{ width: `${progressPercent(task)}%` }"
                />
              </div>
              <div v-else class="mt-1.5 h-1 overflow-hidden rounded-full bg-muted">
                <div class="task-progress-indeterminate h-full rounded-full bg-primary" />
              </div>
            </div>
            <ChevronRight v-if="task.target" class="h-4 w-4 shrink-0 text-muted-foreground" />
          </button>
        </section>

        <section v-if="store.history.length" :class="store.tasks.length ? 'mt-3' : ''">
          <div class="flex items-center justify-between px-1 pb-1.5">
            <h3 class="text-xs font-medium text-muted-foreground">
              {{ t("titleBar.recentCompleted") }}
            </h3>
            <button
              type="button"
              class="flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground"
              @click="store.clearHistory()"
            >
              <Trash2 class="h-3 w-3" />
              {{ t("titleBar.clearHistory") }}
            </button>
          </div>
          <button
            v-for="task in store.history"
            :key="`${task.id}:${task.finishedAt}`"
            type="button"
            class="group flex w-full items-center gap-2 rounded-md px-2 py-2 text-left hover:bg-accent disabled:cursor-default disabled:hover:bg-transparent"
            :disabled="!task.target"
            @click="openTask(task)"
          >
            <CheckCircle2 class="h-4 w-4 shrink-0 text-muted-foreground" />
            <div class="min-w-0 flex-1">
              <div class="flex items-center gap-2">
                <span class="shrink-0 text-xs text-muted-foreground">{{
                  kindLabel(task.kind)
                }}</span>
                <span class="min-w-0 flex-1 truncate text-sm">{{ task.label }}</span>
              </div>
              <div class="mt-0.5 flex items-center gap-1 text-xs text-muted-foreground">
                <Clock3 class="h-3 w-3" />
                {{ finishedTime(task) }}
                <span>·</span>
                <span>{{ t("titleBar.finished") }}</span>
              </div>
            </div>
            <ChevronRight v-if="task.target" class="h-4 w-4 shrink-0 text-muted-foreground" />
          </button>
        </section>
      </div>
    </PopoverContent>
  </Popover>
</template>

<style scoped>
@keyframes task-progress-slide {
  0% {
    width: 22%;
    transform: translateX(-110%);
  }
  50% {
    width: 45%;
  }
  100% {
    width: 22%;
    transform: translateX(460%);
  }
}

.task-progress-indeterminate {
  animation: task-progress-slide 1.4s ease-in-out infinite;
}

@media (prefers-reduced-motion: reduce) {
  .task-progress-indeterminate {
    width: 35%;
    animation: none;
  }
}
</style>
