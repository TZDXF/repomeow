<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";
import { toast } from "vue-sonner";
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
  Trash2,
  X,
} from "@lucide/vue";
import { Markdown, type ControlsConfig } from "vue-stream-markdown";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import SourceFileDialog from "@/components/wiki/SourceFileDialog.vue";
import { createBeforeDownload, createTableCustomize } from "@/lib/markdown-download";
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

// 生成状态托管在全局 store:离开页面不中止,回来直接续看进度
onMounted(async () => {
  if (project.value) await wiki.load(project.value.path);
});

/** 当前项目是否正在生成(生成可能属于另一个项目,那时本页正常查看自己的 wiki) */
const generatingHere = computed(
  () => wiki.generating && wiki.genFor != null && wiki.genFor === project.value?.path,
);

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

/** 按 section 保序分组;无 section 的页面归入 null 组(扁平展示) */
const grouped = computed(() => {
  const groups: { section: string | null; items: WikiPageData[] }[] = [];
  for (const p of pages.value) {
    const section = p.section ?? null;
    const g = groups.find((g) => g.section === section);
    if (g) g.items.push(p);
    else groups.push({ section, items: [p] });
  }
  return groups;
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

// ── 生成进度(左右布局:左侧阶段/进度/页面列表,右侧流式预览) ─────────────────

const phaseText = computed(() => {
  const map: Record<WikiGenPhase, string> = {
    collecting: t("wiki.phase.collecting"),
    outlining: t("wiki.phase.outlining"),
    generating: t("wiki.phase.generating"),
    done: "",
    failed: "",
    cancelled: "",
  };
  return wiki.phase === "idle" ? "" : map[wiki.phase as WikiGenPhase];
});

/** 手动选中的预览页;未选或选中页不可预览时自动跟随第一个正在生成的页 */
const previewId = ref<string | null>(null);
const previewItem = computed(() => {
  const list = wiki.pages;
  const manual = list.find((i) => i.page.id === previewId.value);
  if (manual && (wiki.streamContents[manual.page.id] || manual.status === "running")) {
    return manual;
  }
  return list.find((i) => i.status === "running") ?? null;
});
const previewContent = computed(() =>
  previewItem.value ? (wiki.streamContents[previewItem.value.page.id] ?? "") : "",
);

// ── 生成 / 操作 ───────────────────────────────────────────────────────────

function generate() {
  const p = project.value;
  if (!p) return;
  wiki
    .generate({ path: p.path, name: p.name }, settings.language)
    .then(() => {
      if (wiki.phase === "failed") {
        toast.error(t("wiki.failed", { error: wiki.genError || "-" }));
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
    await wiki.regeneratePage(p.path, page, settings.language);
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
 * 无 headSha(非 git 项目)或历史改写导致 diff 失败时退化为整本重生成
 */
async function updateWiki() {
  const p = project.value;
  if (!p) return;
  try {
    const count = await wiki.update({ path: p.path, name: p.name }, settings.language);
    toast.success(count > 0 ? t("wiki.updatedPages", { count }) : t("wiki.updateNoop"));
  } catch {
    generate();
  }
}

/** 来源文件查看对话框:选中的仓库内相对路径(null 关闭) */
const sourceFile = ref<string | null>(null);

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
        <Button variant="outline" size="sm" :disabled="wiki.generating" @click="generate">
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

    <!-- 生成中(当前项目):左侧阶段/进度/页面列表,右侧流式预览 -->
    <div v-if="generatingHere" class="flex min-h-0 flex-1">
      <aside class="flex w-72 shrink-0 flex-col border-r">
        <div class="shrink-0 space-y-3 border-b p-4">
          <div class="flex items-center gap-2 text-sm">
            <LoaderCircle class="h-4 w-4 animate-spin text-muted-foreground" />
            {{ phaseText }}
          </div>
          <template v-if="wiki.phase === 'generating' && wiki.pages.length">
            <div class="h-1.5 overflow-hidden rounded-full bg-muted">
              <div
                class="h-full rounded-full bg-primary transition-all"
                :style="{ width: `${(wiki.doneCount / wiki.pages.length) * 100}%` }"
              />
            </div>
            <p class="text-xs text-muted-foreground">
              {{ wiki.doneCount }} / {{ wiki.pages.length }}
            </p>
          </template>
        </div>
        <ScrollArea class="min-h-0 flex-1">
          <ul class="space-y-0.5 p-2 text-sm">
            <li v-for="item in wiki.pages" :key="item.page.id">
              <button
                type="button"
                class="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left hover:bg-accent/60"
                :class="item.page.id === previewItem?.page.id ? 'bg-accent' : ''"
                @click="previewId = item.page.id"
              >
                <LoaderCircle
                  v-if="item.status === 'running'"
                  class="h-3.5 w-3.5 shrink-0 animate-spin text-primary"
                />
                <Check
                  v-else-if="item.status === 'done'"
                  class="h-3.5 w-3.5 shrink-0 text-green-500"
                />
                <X
                  v-else-if="item.status === 'failed'"
                  class="h-3.5 w-3.5 shrink-0 text-destructive"
                />
                <Circle v-else class="h-3.5 w-3.5 shrink-0 text-muted-foreground/40" />
                <span class="min-w-0 flex-1 truncate" :title="item.error">
                  {{ item.page.title }}
                </span>
              </button>
            </li>
          </ul>
        </ScrollArea>
        <div class="shrink-0 border-t p-3">
          <Button variant="outline" size="sm" class="w-full" @click="wiki.cancel()">
            {{ t("wiki.cancel") }}
          </Button>
        </div>
      </aside>

      <div class="flex min-w-0 flex-1 flex-col">
        <div class="shrink-0 border-b px-4 py-2 text-xs text-muted-foreground">
          <template v-if="previewItem">
            {{ t("wiki.writing") }} · {{ previewItem.page.title }}
          </template>
          <template v-else>{{ phaseText }}</template>
        </div>
        <ScrollArea class="min-h-0 flex-1">
          <div class="mx-auto max-w-3xl px-6 py-5 text-sm">
            <Markdown
              v-if="previewContent"
              mode="streaming"
              :content="previewContent"
              :controls="controls"
              :theme-element="themeElement"
              :locale="settings.language"
              :before-download="beforeDownload"
            />
            <p v-else class="py-12 text-center text-muted-foreground">
              {{ t("wiki.waitingFirstChunk") }}
            </p>
          </div>
        </ScrollArea>
      </div>
    </div>

    <!-- 空态 -->
    <div v-else-if="!wiki.data" class="flex flex-1 flex-col items-center justify-center gap-3 p-8">
      <BookOpenText class="h-10 w-10 text-muted-foreground/50" />
      <p class="text-sm font-medium">{{ t("wiki.emptyTitle") }}</p>
      <p class="max-w-md text-center text-sm text-muted-foreground">
        {{ t("wiki.emptyDescription") }}
      </p>
      <p v-if="wiki.genError" class="max-w-md text-center text-sm text-destructive">
        {{ wiki.genError }}
      </p>
      <Button
        class="mt-2"
        :disabled="wiki.generating"
        :title="wiki.generating ? t('wiki.busyOther') : undefined"
        @click="generate"
      >
        <BookOpenText class="h-4 w-4" />
        {{ t("wiki.generate") }}
      </Button>
    </div>

    <!-- wiki 内容:左侧页面树 + 右侧正文 -->
    <div v-else class="flex min-h-0 flex-1">
      <aside class="w-64 shrink-0 border-r">
        <ScrollArea class="h-full">
          <nav class="space-y-3 p-3">
            <div v-for="g in grouped" :key="g.section ?? '__flat'">
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
                  p.id === selectedId
                    ? 'bg-accent font-medium text-accent-foreground'
                    : 'text-foreground/80'
                "
                @click="selectedId = p.id"
              >
                <span
                  class="h-1.5 w-1.5 shrink-0 rounded-full"
                  :class="importanceClass(p.importance)"
                />
                <span class="min-w-0 flex-1 truncate" :title="p.title">{{ p.title }}</span>
              </button>
            </div>
          </nav>
        </ScrollArea>
      </aside>

      <ScrollArea class="min-w-0 flex-1">
        <div v-if="current" class="mx-auto max-w-3xl px-6 py-5">
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
          <div v-if="current.content" class="text-sm">
            <Markdown
              mode="static"
              :content="current.content"
              :controls="controls"
              :theme-element="themeElement"
              :locale="settings.language"
              :before-download="beforeDownload"
            />
            <!-- 来源文件(大纲阶段标注):点击查看文件内容 -->
            <div v-if="current.relevantFiles.length" class="mt-6 border-t pt-3">
              <p class="mb-2 text-xs font-medium text-muted-foreground">{{ t("wiki.sources") }}</p>
              <div class="flex flex-wrap gap-1.5">
                <button
                  v-for="f in current.relevantFiles"
                  :key="f"
                  type="button"
                  class="flex max-w-full items-center gap-1 rounded-md border px-2 py-0.5 font-mono text-xs text-muted-foreground hover:bg-accent hover:text-foreground"
                  :title="f"
                  @click="sourceFile = f"
                >
                  <FileCode class="h-3 w-3 shrink-0" />
                  <span class="min-w-0 truncate [direction:rtl] [text-align:left]">{{ f }}</span>
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
        </div>
      </ScrollArea>
    </div>

    <SourceFileDialog :root="project.path" :rel-path="sourceFile" @close="sourceFile = null" />
  </div>
</template>
