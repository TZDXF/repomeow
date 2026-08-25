<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";
import { toast } from "vue-sonner";
import { useNow } from "@vueuse/core";
import {
  ArrowLeft,
  BookOpenText,
  FileCode,
  FolderOpen,
  GitPullRequestArrow,
  LoaderCircle,
  RefreshCw,
  SlidersHorizontal,
  Trash2,
} from "@lucide/vue";
import { Markdown, type ControlsConfig } from "vue-stream-markdown";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import SourceFileDialog from "@/components/wiki/SourceFileDialog.vue";
import WikiGenerateDialog from "@/components/wiki/WikiGenerateDialog.vue";
import WikiPageNavigation from "@/components/wiki/WikiPageNavigation.vue";
import type { WikiNavItem } from "@/components/wiki/wiki-navigation";
import { useWikiPreviewScroll } from "@/composables/wiki/useWikiPreviewScroll";
import { createBeforeDownload, createTableCustomize } from "@/lib/markdown-download";
import { openWikiDir } from "@/lib/wiki";
import type { WikiGenerationActivityType, WikiGenPhase } from "@/lib/wiki-generator";
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

const totalPageCount = computed(() => generation.value?.pages.length ?? 0);
const processedPageCount = computed(
  () =>
    generation.value?.pages.filter((item) => ["done", "failed", "cancelled"].includes(item.status))
      .length ?? 0,
);
const contextSummaryText = computed(() => {
  if (!generation.value || !["collecting", "outlining"].includes(generation.value.phase)) {
    return "";
  }
  const context = generation.value?.context;
  if (!context) {
    return "";
  }
  return t("wiki.progress.contextSummary", {
    files: context.fileCount,
    readme: t(context.hasReadme ? "wiki.progress.found" : "wiki.progress.notFound"),
    manifests: context.manifestCount,
    truncated: context.treeTruncated ? t("wiki.progress.treeTruncated") : "",
  });
});

const visibleActivities = computed(() => generation.value?.activities.slice(-160) ?? []);

function activityTypeLabel(type: WikiGenerationActivityType): string {
  switch (type) {
    case "read":
      return t("wiki.progress.activityRead");
    case "tool":
      return t("wiki.progress.activityTool");
    default:
      return t("wiki.progress.activityScan");
  }
}

function activityText(_type: WikiGenerationActivityType, text: string): string {
  return text;
}

/** 权限决策行的着色(agent 后端外显的自动决策,拒绝需醒目) */
function activityTextClass(text: string): string {
  if (text.startsWith("已拒绝")) return "text-amber-600 dark:text-amber-400";
  if (text.startsWith("已允许")) return "text-muted-foreground/70";
  return "text-muted-foreground";
}

/** agent 大纲阶段的工具调用次数(探索强度指标,仅大纲阶段展示) */
const outlineToolCalls = computed(() =>
  generation.value?.phase === "outlining" ? (generation.value?.toolCalls ?? 0) : 0,
);

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
const retryStatus = computed(() => {
  const retries = generation.value?.retries;
  if (!retries) return undefined;
  return previewItem.value ? retries[previewItem.value.page.id] : retries.outline;
});
const retryText = computed(() => {
  const retry = retryStatus.value;
  if (!retry) return "";
  return t(`wiki.progress.retrying.${retry.reason}`, {
    seconds: retry.delaySeconds,
    attempt: retry.attempt,
    max: retry.maxAttempts,
  });
});

// ── 左侧导航列表(生成中与最终查看复用同一列表样式) ───────────────────────

const navItems = computed<WikiNavItem[]>(() => {
  if (generatingHere.value) {
    return (generation.value?.pages ?? []).map((i) => ({
      id: i.page.id,
      title: i.page.title,
      section: i.page.section ?? null,
      importance: i.page.importance,
      status: i.status,
      error: i.error,
      durationMs: i.durationMs,
    }));
  }
  return pages.value.map((p) => ({
    id: p.id,
    title: p.title,
    section: p.section ?? null,
    importance: p.importance,
  }));
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
  void scrollPreviewToTop();
}

// ── 流式预览自动跟随滚动(用户上翻阅读时暂停,回到底部自动恢复) ─────────────

const activePreviewId = computed(() => previewItem.value?.page.id);
const activityCount = computed(() => generation.value?.activities.length ?? 0);
const previewHost = ref<HTMLElement | null>(null);
const activityLogHost = ref<HTMLElement | null>(null);
const { scrollPreviewToTop } = useWikiPreviewScroll({
  generating: generatingHere,
  activePreviewId,
  previewContent,
  activityCount,
  previewHost,
  activityLogHost,
});

// ── 生成 / 操作 ───────────────────────────────────────────────────────────

/** 生成配置对话框:生成/重新生成前选择,或右上角入口修改当前项目的独立配置 */
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

/** 实际执行整本生成(对话框确认后,或增量更新退化时读取当前项目配置直接跑) */
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
      <WikiPageNavigation
        :items="navItems"
        :active-id="activeId"
        :generating="generatingHere"
        :phase="generation?.phase === 'idle' ? undefined : generation?.phase"
        :total-page-count="totalPageCount"
        :processed-page-count="processedPageCount"
        @select="selectPage"
        @cancel="wiki.cancel(project.path)"
      />

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
            <div
              v-if="retryStatus"
              class="mb-3 flex items-center gap-2 rounded-md border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs text-amber-700 dark:text-amber-300"
              role="status"
              aria-live="polite"
            >
              <RefreshCw class="h-3.5 w-3.5 shrink-0 animate-spin" />
              <span>{{ retryText }}</span>
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
              <p
                v-if="contextSummaryText"
                class="mt-4 max-w-md text-xs leading-5 text-muted-foreground"
              >
                {{ contextSummaryText }}
              </p>
              <p
                v-if="outlineToolCalls > 0"
                class="mt-2 text-xs tabular-nums text-muted-foreground"
                role="status"
              >
                {{ t("wiki.progress.toolCalls", { count: outlineToolCalls }) }}
              </p>
              <div
                v-if="visibleActivities.length"
                ref="activityLogHost"
                class="mt-4 h-52 w-full max-w-2xl overflow-y-auto rounded-lg border bg-muted/20 p-2 text-left font-mono text-[11px] leading-5"
                aria-live="polite"
                :aria-label="t('wiki.progress.activityTitle')"
              >
                <div
                  v-for="(activity, index) in visibleActivities"
                  :key="`${index}-${activity.type}-${activity.text}`"
                  class="flex min-w-0 gap-2"
                >
                  <span class="shrink-0 text-primary/80">{{
                    activityTypeLabel(activity.type)
                  }}</span>
                  <span
                    class="min-w-0 truncate"
                    :class="activityTextClass(activity.text)"
                    :title="activityText(activity.type, activity.text)"
                    >{{ activityText(activity.type, activity.text) }}</span
                  >
                </div>
              </div>
              <p v-else class="mt-4 max-w-sm text-xs leading-5 text-muted-foreground">
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
      :project-path="project.path"
      :mode="genDialogMode"
      @close="genDialogOpen = false"
      @confirm="onGenConfirm"
    />
  </div>
</template>
