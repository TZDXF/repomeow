<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";
import { toast } from "vue-sonner";
import { useNow } from "@vueuse/core";
import { BookOpenText, LoaderCircle, RefreshCw } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import ScrollArea from "@/components/common/ScrollArea.vue";
import WikiGenerateDialog from "@/components/wiki/WikiGenerateDialog.vue";
import WikiHeader from "@/components/wiki/WikiHeader.vue";
import WikiPageNavigation from "@/components/wiki/WikiPageNavigation.vue";
import WikiPageToc from "@/components/wiki/WikiPageToc.vue";
import WikiStaticContent from "@/components/wiki/WikiStaticContent.vue";
import WikiWritingAnimation from "@/components/wiki/WikiWritingAnimation.vue";
import type { WikiNavItem } from "@/components/wiki/wiki-navigation";
import { openWikiDir } from "@/lib/wiki";
import type { WikiGenPhase } from "@/lib/wiki-generator";
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

// 首次渲染前启动加载，避免把尚未读取的数据误判为空；也响应项目切换。
// 生成状态托管在全局 store:离开页面不中止,回来直接续看进度
watch(
  () => project.value?.path,
  async (path) => {
    if (path) await wiki.load(path);
  },
  { immediate: true },
);

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
  [pages, generatingHere],
  ([list, generating]) => {
    if (!generating && !list.some((p) => p.id === selectedId.value)) {
      selectedId.value = list[0]?.id ?? null;
    }
  },
  { immediate: true },
);
const current = computed(() => pages.value.find((p) => p.id === selectedId.value) ?? null);

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

/** agent 工具调用累计次数(大纲探索与页面补读),驱动「文档书写中」动画的粒子与徽标 */
const writingToolCalls = computed(() => generation.value?.toolCalls ?? 0);

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

/** 生成期间只阅读已完成页面，不展示中间正文或自动跟随生成页。 */
const generationItem = computed(() =>
  generation.value?.pages.find((item) => item.page.id === selectedId.value),
);
const completedPages = computed<WikiPageData[]>(() =>
  (generation.value?.pages ?? [])
    .filter((item) => item.status === "done")
    .map((item) => ({
      ...item.page,
      content: generation.value?.streamContents[item.page.id] ?? "",
    })),
);
const readingPage = computed(() =>
  generatingHere.value
    ? (completedPages.value.find((page) => page.id === selectedId.value) ?? null)
    : current.value,
);
const retryStatus = computed(() => {
  const retries = generation.value?.retries;
  if (!retries) return undefined;
  return generationItem.value ? retries[generationItem.value.page.id] : retries.outline;
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
    unread: project.value ? wiki.isPageUnread(project.value.path, p.id) : false,
  }));
});

const activeId = computed(() => selectedId.value);

function selectPage(id: string) {
  selectedId.value = id;
  if (!generatingHere.value && project.value) wiki.markPageRead(project.value.path, id);
}

/** 页面进入正文视口即视为已读；也覆盖首次打开和更新后仍停留在当前页的情况。 */
watch(current, (page) => {
  if (page && project.value && !generatingHere.value) {
    wiki.markPageRead(project.value.path, page.id);
  }
});

/** 从正文底部跳转相关页面,与普通切换共用同一条选中路径 */
function selectRelatedPage(id: string) {
  selectPage(id);
}

// 切换阅读页面时回到顶部，不再跟随生成内容滚动。
const previewHost = ref<HTMLElement | null>(null);
watch(
  () => readingPage.value?.id,
  async () => {
    await nextTick();
    const viewport = previewHost.value?.closest('[data-slot="scroll-area-viewport"]');
    if (viewport) viewport.scrollTop = 0;
  },
);

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
 * 无 headSha(非 git 项目)、历史改写导致 diff 失败、或旧 Wiki 由已移除的三方
 * agent 后端生成(generator 不一致)时退化为整本重生成
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
</script>

<template>
  <div v-if="project" class="flex h-full flex-col">
    <WikiHeader
      :project-name="project.name"
      :generating="generatingHere"
      :elapsed-text="elapsedText"
      :stale="wiki.data?.stale ?? false"
      :has-data="!!wiki.data"
      :updating="wiki.updating"
      @back="router.push(`/projects/${project.id}`)"
      @update="updateWiki"
      @edit-config="requestConfigEdit"
      @regenerate="requestGenerate"
      @open-dir="openDir"
      @remove="removeWiki"
    />

    <!-- 主体:左侧页面列表 + 右侧内容;生成中复用同一布局(列表带单页状态,右侧阅读已完成页面) -->
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

      <!-- 相对定位容器承载正文滚动区;目录浮窗挂在视口层,滚动时保持可见 -->
      <div class="relative min-w-0 flex-1">
        <ScrollArea class="h-full">
          <div ref="previewHost" class="mx-auto max-w-3xl px-6 py-5 text-sm">
            <!-- 未完成页面只展示生成状态 -->
            <template v-if="generatingHere && !readingPage">
              <div v-if="generationItem" class="mb-3 flex items-center gap-2 border-b pb-3">
                <LoaderCircle class="h-3.5 w-3.5 shrink-0 animate-spin text-muted-foreground" />
                <span class="min-w-0 flex-1 truncate text-xs text-muted-foreground">
                  {{ t("wiki.writing") }} · {{ generationItem.page.title }}
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
              <div class="flex min-h-[22rem] flex-col items-center justify-center text-center">
                <WikiWritingAnimation :tool-calls="writingToolCalls" />
                <p class="mt-6 text-sm font-medium">
                  {{ phaseText }}
                </p>
                <p
                  v-if="contextSummaryText"
                  class="mt-3 max-w-md text-xs leading-5 text-muted-foreground"
                >
                  {{ contextSummaryText }}
                </p>
                <p class="mt-4 max-w-sm text-xs leading-5 text-muted-foreground">
                  {{ t("wiki.progress.leaveHint") }}
                </p>
              </div>
            </template>

            <WikiStaticContent
              v-else-if="readingPage"
              :page="readingPage"
              :pages="generatingHere ? completedPages : pages"
              :project-root="project.path"
              :language="settings.language"
              :regenerating="wiki.regeneratingPage === readingPage.id"
              :generation-active="generatingHere"
              @regenerate="regeneratePage"
              @select-related="selectRelatedPage"
            />
          </div>
        </ScrollArea>

        <WikiPageToc
          v-if="readingPage"
          :root="previewHost"
          :content="readingPage.content"
          :page-id="readingPage.id"
        />
      </div>
    </div>

    <!-- 已有内容刷新时不隐藏；首次读取结束前不展示空态。 -->
    <div
      v-else-if="wiki.loading"
      class="flex flex-1 items-center justify-center gap-2 text-sm text-muted-foreground"
      role="status"
    >
      <LoaderCircle class="h-4 w-4 animate-spin" />
      <span>{{ t("common.loading") }}</span>
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

    <WikiGenerateDialog
      :open="genDialogOpen"
      :project-path="project.path"
      :mode="genDialogMode"
      @close="genDialogOpen = false"
      @confirm="onGenConfirm"
    />
  </div>
</template>
