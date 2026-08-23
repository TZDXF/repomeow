<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";
import { toast } from "vue-sonner";
import { useNow } from "@vueuse/core";
import {
  ArrowLeft,
  BookOpenText,
  Check,
  Circle,
  FileCode,
  FolderOpen,
  GitPullRequestArrow,
  LoaderCircle,
  RefreshCw,
  SlidersHorizontal,
  Trash2,
  X,
} from "@lucide/vue";
import { Markdown, type ControlsConfig } from "vue-stream-markdown";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import SourceFileDialog from "@/components/wiki/SourceFileDialog.vue";
import WikiGenerateDialog from "@/components/wiki/WikiGenerateDialog.vue";
import { createBeforeDownload, createTableCustomize } from "@/lib/markdown-download";
import { openWikiDir } from "@/lib/wiki";
import type { WikiGenPhase, WikiPageStatus } from "@/lib/wiki-generator";
import { parseWikiSources } from "@/lib/wiki-parse";
import { useProjectsStore } from "@/stores/projects";
import { useSettingsStore } from "@/stores/settings";
import { useWikiStore } from "@/stores/wiki";
import type { Project, WikiPageData } from "@/types";

const { t } = useI18n();
const route = useRoute();
const router = useRouter();
const store = useProjectsStore();
const settings = useSettingsStore();
const wiki = useWikiStore();

const project = computed<Project | undefined>(() => {
  const id = Number(route.params.id);
  return Number.isFinite(id) ? store.projects.find((p) => p.id === id) : undefined;
});

// 生成状态托管在全局 store:离开页面不中止,回来直接续看进度
onMounted(async () => {
  if (project.value) await wiki.load(project.value.path);
});

/** 当前项目的生成状态;不同项目的任务在全局 store 中按路径隔离 */
const generation = computed(() => {
  const p = project.value;
  return p ? wiki.generationFor(p.path) : undefined;
});
const generatingHere = computed(() => {
  const p = project.value;
  return p ? wiki.isGenerating(p.path) : false;
});

// ── 页面选择 ──────────────────────────────────────────────────────────────

const pages = computed(() => wiki.data?.pages ?? []);
const selectedId = ref<string | null>(null);
watch(
  pages,
  (list) => {
    if (!list.some((p) => p.id === selectedId.value)) {
      selectedId.value = list[0]?.id ?? null;
    }
  },
  { immediate: true },
);
const current = computed(() => pages.value.find((p) => p.id === selectedId.value) ?? null);

/** 当前页关联的有效 Wiki 页面;保持大纲 relatedPages 声明顺序并忽略无效/重复/self ID */
const currentRelatedPages = computed<WikiPageData[]>(() => {
  const page = current.value;
  if (!page) {
    return [];
  }
  const seen = new Set<string>();
  const related: WikiPageData[] = [];
  for (const id of page.relatedPages) {
    if (id === page.id || seen.has(id)) {
      continue;
    }
    const target = pages.value.find((item) => item.id === id);
    if (target) {
      seen.add(id);
      related.push(target);
    }
  }
  return related;
});

/** importance 色点(参照 deepwiki-open:high 紫 / medium 琥珀 / low 珊瑚) */
function importanceClass(importance: string): string {
  switch (importance) {
    case "high":
      return "bg-violet-500";
    case "low":
      return "bg-rose-400";
    default:
      return "bg-amber-400";
  }
}

function importanceLabel(importance: string): string {
  switch (importance) {
    case "high":
      return t("wiki.importance.high");
    case "low":
      return t("wiki.importance.low");
    default:
      return t("wiki.importance.medium");
  }
}

// ── 生成进度 ───────────────────────────────────────────────────────────────

const phaseText = computed(() => {
  const map: Record<WikiGenPhase, string> = {
    collecting: t("wiki.phase.collecting"),
    outlining: t("wiki.phase.outlining"),
    generating: t("wiki.phase.generating"),
    done: "",
    failed: "",
    cancelled: "",
  };
  const phase = generation.value?.phase ?? "idle";
  return phase === "idle" ? "" : map[phase];
});

const phaseSteps = computed(() => [
  t("wiki.progress.collecting"),
  t("wiki.progress.outlining"),
  t("wiki.progress.generating"),
]);

const activePhaseIndex = computed(() => {
  switch (generation.value?.phase) {
    case "outlining":
      return 1;
    case "generating":
      return 2;
    default:
      return 0;
  }
});

const totalPageCount = computed(() => generation.value?.pages.length ?? 0);
const processedPageCount = computed(
  () =>
    generation.value?.pages.filter((item) => ["done", "failed", "cancelled"].includes(item.status))
      .length ?? 0,
);
const failedPageCount = computed(
  () => generation.value?.pages.filter((item) => item.status === "failed").length ?? 0,
);
const pageProgressPercent = computed(() =>
  totalPageCount.value > 0
    ? Math.round((processedPageCount.value / totalPageCount.value) * 100)
    : 0,
);
const pageProgressStyle = computed(() => ({ width: `${pageProgressPercent.value}%` }));

/** 每秒刷新展示用时;生成开始时间由全局 store 持有,路由返回后不会重新计时 */
const now = useNow({ interval: 1000 });
const elapsedText = computed(() => {
  const startedAt = generation.value?.startedAt;
  if (!startedAt) return "00:00";
  const totalSeconds = Math.max(0, Math.floor((now.value.getTime() - startedAt) / 1000));
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  const pair = `${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`;
  return hours > 0 ? `${String(hours).padStart(2, "0")}:${pair}` : pair;
});

/** 手动选中的预览页;未选或选中页不可预览时自动跟随第一个正在生成的页 */
const previewId = ref<string | null>(null);
const previewItem = computed(() => {
  const list = generation.value?.pages ?? [];
  const manual = list.find((i) => i.page.id === previewId.value);
  if (manual && (generation.value?.streamContents[manual.page.id] || manual.status === "running")) {
    return manual;
  }
  return list.find((i) => i.status === "running") ?? null;
});
const previewContent = computed(() =>
  previewItem.value ? (generation.value?.streamContents[previewItem.value.page.id] ?? "") : "",
);

// ── 左侧导航列表(生成中与最终查看复用同一列表样式) ───────────────────────

/** status 仅生成中的条目携带:存在时以状态图标替代 importance 色点 */
interface WikiNavItem {
  id: string;
  title: string;
  section: string | null;
  importance: string;
  status?: WikiPageStatus;
  error?: string;
}

const navItems = computed<WikiNavItem[]>(() => {
  if (generatingHere.value) {
    return (generation.value?.pages ?? []).map((i) => ({
      id: i.page.id,
      title: i.page.title,
      section: i.page.section ?? null,
      importance: i.page.importance,
      status: i.status,
      error: i.error,
    }));
  }
  return pages.value.map((p) => ({
    id: p.id,
    title: p.title,
    section: p.section ?? null,
    importance: p.importance,
  }));
});

/** 按 section 保序分组;无 section 的页面归入 null 组(扁平展示) */
const navGroups = computed(() => {
  const groups: { section: string | null; items: WikiNavItem[] }[] = [];
  for (const p of navItems.value) {
    const section = p.section ?? null;
    const last = groups[groups.length - 1];
    if (last && last.section === section) last.items.push(p);
    else groups.push({ section, items: [p] });
  }
  return groups;
});

const activeId = computed(() =>
  generatingHere.value ? (previewItem.value?.page.id ?? null) : selectedId.value,
);

function selectPage(id: string) {
  if (generatingHere.value) previewId.value = id;
  else selectedId.value = id;
}

/** 从正文底部跳转相关页面后回到正文顶部,与 deepwiki-open 的页面导航行为一致 */
function selectRelatedPage(id: string) {
  selectPage(id);
  void nextTick(() => {
    const viewport = scrollViewport();
    if (viewport) {
      setViewportScroll(viewport, "top");
    }
  });
}

// ── 流式预览自动跟随滚动(用户上翻阅读时暂停,回到底部自动恢复) ─────────────

const previewHost = ref<HTMLElement | null>(null);
let pinnedToBottom = true;
let suppressScrollEvents = false;

/** reka-ui ScrollArea 的实际滚动元素是内部 viewport(本仓库包装层标注的 data-slot) */
function scrollViewport(): HTMLElement | null {
  return previewHost.value?.closest('[data-slot="scroll-area-viewport"]') ?? null;
}

function onPreviewScroll(e: Event) {
  if (suppressScrollEvents) return;
  const el = e.target as HTMLElement;
  pinnedToBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
}

/** 程序化滚动需屏蔽自身触发的 scroll 事件,避免覆盖 pinned 状态 */
function setViewportScroll(vp: HTMLElement, position: "top" | "bottom") {
  suppressScrollEvents = true;
  vp.scrollTop = position === "top" ? 0 : vp.scrollHeight;
  requestAnimationFrame(() => {
    suppressScrollEvents = false;
  });
}

watch(previewHost, (el) => {
  if (el) scrollViewport()?.addEventListener("scroll", onPreviewScroll, { passive: true });
});

watch(previewContent, async () => {
  if (!generatingHere.value || !pinnedToBottom) return;
  await nextTick();
  const vp = scrollViewport();
  if (vp) setViewportScroll(vp, "bottom");
});

// 切换预览页 / 进入与结束生成时回到顶部并重新开始跟随;
// 生成属于其他项目时 previewId 分量为 null,不会扰动本页静态视图的滚动
watch(
  () => [generatingHere.value, generatingHere.value ? previewItem.value?.page.id : null] as const,
  async () => {
    pinnedToBottom = true;
    await nextTick();
    const vp = scrollViewport();
    if (vp) setViewportScroll(vp, "top");
  },
);

// ── 生成 / 操作 ───────────────────────────────────────────────────────────

/** 生成配置对话框:生成/重新生成前选择,或右上角入口直接查看/修改已记录配置 */
const genDialogOpen = ref(false);
const genDialogMode = ref<"generate" | "edit">("generate");

function requestGenerate() {
  genDialogMode.value = "generate";
  genDialogOpen.value = true;
}

function requestConfigEdit() {
  genDialogMode.value = "edit";
  genDialogOpen.value = true;
}

/** 对话框确认(配置已写回设置):generate 模式随之启动整本生成 */
function onGenConfirm() {
  genDialogOpen.value = false;
  if (genDialogMode.value === "generate") {
    generate();
  }
}

/** 实际执行整本生成(对话框确认后,或增量更新退化时用已记录配置直接跑) */
function generate() {
  const p = project.value;
  if (!p) return;
  wiki
    .generate({ id: p.id, path: p.path, name: p.name }, settings.language)
    .then(() => {
      const result = wiki.generationFor(p.path);
      if (result?.phase === "failed") {
        toast.error(t("wiki.failed", { error: result.error || "-" }));
      }
    })
    .catch(() => {});
}

async function removeWiki() {
  const p = project.value;
  if (!p) return;
  try {
    await wiki.remove(p.path);
    toast.success(t("wiki.deleted"));
  } catch (e) {
    toast.error(t("wiki.deleteFailed", { error: String(e) }));
  }
}

async function regeneratePage(page: WikiPageData) {
  const p = project.value;
  if (!p) return;
  try {
    await wiki.regeneratePage({ id: p.id, path: p.path, name: p.name }, page, settings.language);
    toast.success(t("wiki.pageRegenerated"));
  } catch (e) {
    toast.error(t("wiki.failed", { error: String(e) }));
  }
}

function openDir() {
  const p = project.value;
  if (!p) return;
  openWikiDir(p.path).catch((e) =>
    toast.error(t("settings.prompts.openDirFailed", { error: String(e) })),
  );
}

/**
 * 增量更新:只重生成受 headSha..HEAD 变更影响的页面。
 * 无 headSha(非 git 项目)、历史改写导致 diff 失败、或生成后端切换(generator 不一致)
 * 时退化为整本重生成
 */
async function updateWiki() {
  const p = project.value;
  if (!p) return;
  try {
    const count = await wiki.update({ id: p.id, path: p.path, name: p.name }, settings.language);
    toast.success(count > 0 ? t("wiki.updatedPages", { count }) : t("wiki.updateNoop"));
  } catch {
    generate();
  }
}

/** 来源文件查看对话框目标:仓库内相对路径 + 可选行区间(null 关闭) */
interface SourceTarget {
  path: string;
  start: number | null;
  end: number | null;
}
const sourceTarget = ref<SourceTarget | null>(null);

/**
 * 当前页正文解析:剥离末尾 <!-- sources --> 注释块(页面 LLM 标注的来源行区间,
 * 渲染不可见),区间合并进来源 chips(relevantFiles 同时充当 basename 补全白名单);
 * 无块时 ranges 为空,chips 退化为文件级
 */
const currentSources = computed(() =>
  parseWikiSources(current.value?.content ?? "", current.value?.relevantFiles),
);

function openSource(path: string) {
  const r = currentSources.value.ranges.get(path);
  sourceTarget.value = { path, start: r?.start ?? null, end: r?.end ?? null };
}

/** chip 上的行区间徽标文本(`:12-40` / `:7`);无标注返回空串 */
function sourceRangeLabel(path: string): string {
  const r = currentSources.value.ranges.get(path);
  if (!r) return "";
  return r.end > r.start ? `:${r.start}-${r.end}` : `:${r.start}`;
}

/** 流式预览同样剥离 sources 注释块(生成中途的未闭合尾巴也一并隐藏) */
const previewDisplay = computed(() => parseWikiSources(previewContent.value).body);

// ── Markdown 渲染配置(与 ReportHistory 一致) ─────────────────────────────

const controls: ControlsConfig = {
  table: {
    copy: true,
    download: true,
    fullscreen: true,
    customize: createTableCustomize(t),
  },
  code: { copy: true, collapse: true },
};
const detachedThemeEl = document.createElement("div");
const themeElement = () => detachedThemeEl;
const beforeDownload = createBeforeDownload(t);
</script>

<template>
  <div v-if="project" class="flex h-full flex-col">
    <header class="flex shrink-0 items-center gap-2 border-b px-4 py-3">
      <Button
        variant="ghost"
        size="icon"
        class="h-8 w-8 shrink-0"
        :title="t('wiki.back')"
        @click="router.push(`/projects/${project.id}`)"
      >
        <ArrowLeft class="h-4 w-4" />
      </Button>
      <BookOpenText class="h-4 w-4 shrink-0 text-muted-foreground" />
      <span class="min-w-0 flex-1 truncate text-sm font-medium">
        {{ project.name }} · {{ t("wiki.title") }}
      </span>
      <div
        v-if="generatingHere"
        class="flex shrink-0 items-center gap-2 rounded-full border border-primary/20 bg-primary/5 px-2.5 py-1 text-xs text-primary"
      >
        <LoaderCircle class="h-3.5 w-3.5 animate-spin" />
        <span>{{ t("wiki.progress.inProgress") }}</span>
        <span class="text-muted-foreground">·</span>
        <span class="tabular-nums text-muted-foreground">{{ elapsedText }}</span>
      </div>
      <Badge v-if="wiki.data?.stale" variant="secondary" :title="t('wiki.staleHint')">
        {{ t("wiki.stale") }}
      </Badge>
      <template v-if="wiki.data && !generatingHere">
        <Button
          v-if="wiki.data.stale"
          variant="outline"
          size="sm"
          :disabled="wiki.updating"
          :title="t('wiki.updateHint')"
          @click="updateWiki"
        >
          <LoaderCircle v-if="wiki.updating" class="h-4 w-4 animate-spin" />
          <GitPullRequestArrow v-else class="h-4 w-4" />
          {{ t("wiki.update") }}
        </Button>
        <Button
          variant="outline"
          size="sm"
          :title="t('wiki.genConfigTitle')"
          @click="requestConfigEdit"
        >
          <SlidersHorizontal class="h-4 w-4" />
        </Button>
        <Button variant="outline" size="sm" @click="requestGenerate">
          <RefreshCw class="h-4 w-4" />
          {{ t("wiki.regenerate") }}
        </Button>
        <Button variant="outline" size="sm" @click="openDir">
          <FolderOpen class="h-4 w-4" />
          {{ t("wiki.openDir") }}
        </Button>
        <Button variant="outline" size="sm" @click="removeWiki">
          <Trash2 class="h-4 w-4" />
          {{ t("wiki.delete") }}
        </Button>
      </template>
    </header>

    <!-- 主体:左侧页面列表 + 右侧内容;生成中复用同一布局(列表带单页状态,右侧流式预览) -->
    <div v-if="generatingHere || wiki.data" class="flex min-h-0 flex-1">
      <aside class="flex w-64 shrink-0 flex-col border-r">
        <section v-if="generatingHere" class="shrink-0 border-b bg-muted/20 p-3" aria-live="polite">
          <div class="flex items-center gap-2">
            <LoaderCircle class="h-4 w-4 shrink-0 animate-spin text-primary" />
            <p class="min-w-0 flex-1 truncate text-xs font-medium">{{ phaseText }}</p>
            <span
              v-if="totalPageCount"
              class="shrink-0 text-xs font-semibold tabular-nums text-primary"
            >
              {{ pageProgressPercent }}%
            </span>
          </div>

          <div class="mt-3 grid grid-cols-3 gap-1" aria-hidden="true">
            <div v-for="(step, index) in phaseSteps" :key="step" class="text-center">
              <div class="mb-1 flex items-center">
                <span
                  class="h-px flex-1"
                  :class="index <= activePhaseIndex ? 'bg-primary/50' : 'bg-border'"
                />
                <span
                  class="relative h-2.5 w-2.5 shrink-0 rounded-full border"
                  :class="
                    index < activePhaseIndex
                      ? 'border-primary bg-primary'
                      : index === activePhaseIndex
                        ? 'border-primary bg-background shadow-[0_0_0_3px] shadow-primary/15'
                        : 'border-border bg-background'
                  "
                >
                  <span
                    v-if="index === activePhaseIndex"
                    class="absolute inset-0 animate-ping rounded-full bg-primary/50"
                  />
                </span>
                <span
                  class="h-px flex-1"
                  :class="index < activePhaseIndex ? 'bg-primary/50' : 'bg-border'"
                />
              </div>
              <span
                class="text-[10px]"
                :class="index <= activePhaseIndex ? 'text-foreground' : 'text-muted-foreground/60'"
              >
                {{ step }}
              </span>
            </div>
          </div>

          <div
            class="mt-3 h-1.5 overflow-hidden rounded-full bg-muted"
            role="progressbar"
            :aria-label="phaseText"
            :aria-valuemin="totalPageCount ? 0 : undefined"
            :aria-valuemax="totalPageCount ? 100 : undefined"
            :aria-valuenow="totalPageCount ? pageProgressPercent : undefined"
          >
            <div
              v-if="totalPageCount"
              class="h-full rounded-full bg-primary transition-[width] duration-500 ease-out"
              :style="pageProgressStyle"
            />
            <div v-else class="wiki-progress-indeterminate h-full rounded-full bg-primary" />
          </div>

          <div class="mt-2 text-[11px] text-muted-foreground">
            <span v-if="totalPageCount" class="tabular-nums">
              {{
                t("wiki.progress.pages", {
                  processed: processedPageCount,
                  total: totalPageCount,
                })
              }}
            </span>
            <span v-else>{{ t("wiki.progress.preparing") }}</span>
          </div>
          <p v-if="failedPageCount" class="mt-1 text-[11px] text-destructive">
            {{ t("wiki.progress.failedPages", { count: failedPageCount }) }}
          </p>
        </section>
        <ScrollArea class="min-h-0 flex-1">
          <TooltipProvider>
            <nav class="space-y-3 p-3">
              <div v-for="g in navGroups" :key="g.section ?? '__flat'">
                <p
                  v-if="g.section"
                  class="mb-1 px-2 text-xs font-medium uppercase tracking-wide text-muted-foreground"
                >
                  {{ g.section }}
                </p>
                <button
                  v-for="p in g.items"
                  :key="p.id"
                  type="button"
                  class="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm hover:bg-accent/60"
                  :class="
                    p.id === activeId
                      ? 'bg-accent font-medium text-accent-foreground'
                      : p.status === 'running'
                        ? 'bg-primary/5 text-foreground'
                        : 'text-foreground/80'
                  "
                  :title="p.error ?? (p.status ? p.title : undefined)"
                  @click="selectPage(p.id)"
                >
                  <template v-if="p.status">
                    <LoaderCircle
                      v-if="p.status === 'running'"
                      class="h-3.5 w-3.5 shrink-0 animate-spin text-primary"
                    />
                    <Check
                      v-else-if="p.status === 'done'"
                      class="h-3.5 w-3.5 shrink-0 text-green-500"
                    />
                    <X
                      v-else-if="p.status === 'failed'"
                      class="h-3.5 w-3.5 shrink-0 text-destructive"
                    />
                    <Circle v-else class="h-3.5 w-3.5 shrink-0 text-muted-foreground/40" />
                  </template>
                  <Tooltip v-else>
                    <TooltipTrigger as-child>
                      <span
                        class="flex h-3.5 w-3.5 shrink-0 items-center justify-center"
                        :aria-label="importanceLabel(p.importance)"
                      >
                        <span
                          class="h-1.5 w-1.5 rounded-full"
                          :class="importanceClass(p.importance)"
                        />
                      </span>
                    </TooltipTrigger>
                    <TooltipContent side="right">
                      {{ importanceLabel(p.importance) }}
                    </TooltipContent>
                  </Tooltip>
                  <span class="min-w-0 flex-1 truncate" :title="p.title">{{ p.title }}</span>
                </button>
              </div>
            </nav>
          </TooltipProvider>
        </ScrollArea>
        <div v-if="generatingHere" class="shrink-0 border-t p-2">
          <Button variant="outline" size="sm" class="w-full" @click="wiki.cancel(project.path)">
            {{ t("wiki.cancel") }}
          </Button>
        </div>
      </aside>

      <ScrollArea class="min-w-0 flex-1">
        <div ref="previewHost" class="mx-auto max-w-3xl px-6 py-5 text-sm">
          <!-- 生成中:流式预览 -->
          <template v-if="generatingHere">
            <div v-if="previewItem" class="mb-3 flex items-center gap-2 border-b pb-3">
              <LoaderCircle class="h-3.5 w-3.5 shrink-0 animate-spin text-muted-foreground" />
              <span class="min-w-0 flex-1 truncate text-xs text-muted-foreground">
                {{ t("wiki.writing") }} · {{ previewItem.page.title }}
              </span>
              <span
                v-if="totalPageCount"
                class="shrink-0 text-xs tabular-nums text-muted-foreground"
              >
                {{ processedPageCount }} / {{ totalPageCount }}
              </span>
            </div>
            <Markdown
              v-if="previewDisplay"
              mode="streaming"
              :content="previewDisplay"
              :controls="controls"
              :theme-element="themeElement"
              :locale="settings.language"
              :before-download="beforeDownload"
            />
            <div v-else class="flex min-h-[22rem] flex-col items-center justify-center text-center">
              <div class="relative mb-5 flex h-16 w-16 items-center justify-center">
                <span class="absolute inset-0 animate-ping rounded-full bg-primary/10" />
                <span class="absolute inset-2 animate-pulse rounded-full bg-primary/10" />
                <BookOpenText class="relative h-7 w-7 text-primary" />
              </div>
              <p class="text-sm font-medium">
                {{ previewItem ? t("wiki.waitingFirstChunk") : phaseText }}
              </p>
              <div class="mt-3 flex gap-1.5" aria-hidden="true">
                <span class="h-1.5 w-1.5 animate-bounce rounded-full bg-primary/70" />
                <span
                  class="h-1.5 w-1.5 animate-bounce rounded-full bg-primary/70 [animation-delay:150ms]"
                />
                <span
                  class="h-1.5 w-1.5 animate-bounce rounded-full bg-primary/70 [animation-delay:300ms]"
                />
              </div>
              <p class="mt-4 max-w-sm text-xs leading-5 text-muted-foreground">
                {{ t("wiki.progress.leaveHint") }}
              </p>
            </div>
          </template>

          <!-- 静态正文 -->
          <template v-else-if="current">
            <div class="mb-3 flex items-center gap-2">
              <span class="min-w-0 flex-1 truncate text-xs text-muted-foreground">
                {{ current.file }}
              </span>
              <Button
                variant="ghost"
                size="sm"
                :disabled="wiki.regeneratingPage === current.id"
                :title="t('wiki.regeneratePage')"
                @click="regeneratePage(current)"
              >
                <LoaderCircle
                  v-if="wiki.regeneratingPage === current.id"
                  class="h-3.5 w-3.5 animate-spin"
                />
                <RefreshCw v-else class="h-3.5 w-3.5" />
              </Button>
            </div>
            <div v-if="current.content">
              <Markdown
                mode="static"
                :content="currentSources.body"
                :controls="controls"
                :theme-element="themeElement"
                :locale="settings.language"
                :before-download="beforeDownload"
              />
              <!-- 来源文件(chips 来自大纲标注;行区间来自页面末尾 sources 注释块) -->
              <div v-if="current.relevantFiles.length" class="mt-6 border-t pt-3">
                <p class="mb-2 text-xs font-medium text-muted-foreground">
                  {{ t("wiki.sources") }}
                </p>
                <div class="flex flex-wrap gap-1.5">
                  <button
                    v-for="f in current.relevantFiles"
                    :key="f"
                    type="button"
                    class="flex max-w-full items-center gap-1 rounded-md border px-2 py-0.5 font-mono text-xs text-muted-foreground hover:bg-accent hover:text-foreground"
                    :title="f"
                    @click="openSource(f)"
                  >
                    <FileCode class="h-3 w-3 shrink-0" />
                    <span class="min-w-0 truncate [direction:rtl] [text-align:left]">{{ f }}</span>
                    <span v-if="sourceRangeLabel(f)" class="shrink-0 text-primary">{{
                      sourceRangeLabel(f)
                    }}</span>
                  </button>
                </div>
              </div>
              <!-- 相关页面(按 relatedPages 中的页面 ID 查找并跳转) -->
              <div v-if="currentRelatedPages.length" class="mt-6 border-t pt-3">
                <p class="mb-2 text-xs font-medium text-muted-foreground">
                  {{ t("wiki.relatedPages") }}
                </p>
                <div class="flex flex-wrap gap-1.5">
                  <button
                    v-for="page in currentRelatedPages"
                    :key="page.id"
                    type="button"
                    class="flex max-w-full items-center gap-1.5 rounded-md border px-2.5 py-1 text-xs text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                    :title="page.title"
                    @click="selectRelatedPage(page.id)"
                  >
                    <BookOpenText class="h-3 w-3 shrink-0" />
                    <span class="truncate">{{ page.title }}</span>
                  </button>
                </div>
              </div>
            </div>
            <div
              v-else
              class="flex flex-col items-center gap-2 rounded-md border border-dashed py-12 text-sm text-muted-foreground"
            >
              <p>{{ t("wiki.emptyContent") }}</p>
              <Button variant="outline" size="sm" @click="regeneratePage(current)">
                <RefreshCw class="h-4 w-4" />
                {{ t("wiki.regeneratePage") }}
              </Button>
            </div>
          </template>
        </div>
      </ScrollArea>
    </div>

    <!-- 空态 -->
    <div v-else class="flex flex-1 flex-col items-center justify-center gap-3 p-8">
      <BookOpenText class="h-10 w-10 text-muted-foreground/50" />
      <p class="text-sm font-medium">{{ t("wiki.emptyTitle") }}</p>
      <p class="max-w-md text-center text-sm text-muted-foreground">
        {{ t("wiki.emptyDescription") }}
      </p>
      <p v-if="generation?.error" class="max-w-md text-center text-sm text-destructive">
        {{ generation.error }}
      </p>
      <Button class="mt-2" @click="requestGenerate">
        <BookOpenText class="h-4 w-4" />
        {{ t("wiki.generate") }}
      </Button>
    </div>

    <SourceFileDialog
      :root="project.path"
      :rel-path="sourceTarget?.path ?? null"
      :start-line="sourceTarget?.start ?? null"
      :end-line="sourceTarget?.end ?? null"
      @close="sourceTarget = null"
    />
    <WikiGenerateDialog
      :open="genDialogOpen"
      :mode="genDialogMode"
      @close="genDialogOpen = false"
      @confirm="onGenConfirm"
    />
  </div>
</template>

<style scoped>
@keyframes wiki-progress-slide {
  from {
    transform: translateX(-120%);
  }
  to {
    transform: translateX(350%);
  }
}

.wiki-progress-indeterminate {
  width: 30%;
  animation: wiki-progress-slide 1.4s ease-in-out infinite;
}

@media (prefers-reduced-motion: reduce) {
  .wiki-progress-indeterminate {
    width: 100%;
    animation: none;
    opacity: 0.55;
  }
}
</style>
