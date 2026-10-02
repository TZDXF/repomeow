<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";
import { openPath, openUrl } from "@tauri-apps/plugin-opener";
import { toast } from "vue-sonner";
import {
  ArrowLeft,
  Check,
  Code,
  Eye,
  ExternalLink,
  FolderOpen,
  Languages,
  ListPlus,
  RefreshCw,
  ShieldAlert,
} from "@lucide/vue";
import { Markdown, type ControlsConfig, type NodeRenderers } from "vue-stream-markdown";
import ResourcePublicAudits from "@/components/settings/ResourcePublicAudits.vue";
import FileTreeList from "@/components/common/FileTreeList.vue";
import CodeViewer from "@/components/files/CodeViewer.vue";
import ImageViewer from "@/components/files/ImageViewer.vue";
import VideoViewer from "@/components/files/VideoViewer.vue";
import MdLink from "@/components/markdown/MdLink.vue";
import type { SupportedLocale } from "@/i18n";
import { buildFileTree, flattenVisibleTree, type FileTreeRow } from "@/lib/file-tree";
import { createBeforeDownload } from "@/lib/markdown-download";
import { hasScheme, resolvePath, safeLinkHref } from "@/lib/markdown";
import { extOf, IMAGE_EXTS, VIDEO_EXTS } from "@/lib/file-kind";
import { joinPath } from "@/lib/path";
import { useImagePreview } from "@/composables/files/useImagePreview";
import SkillScanPanel from "@/components/skill-preview/SkillScanPanel.vue";
import { scanLevelClass, scanLevelLabel } from "@/components/skill-preview/scan-labels";
import { useMarkdownTranslation } from "@/composables/useMarkdownTranslation";
import { useSkillScan, type SkillScanModelRef } from "@/composables/useSkillScan";
import { useSettingsStore } from "@/stores/settings";
import {
  marketplaceAuditId,
  listResourceSkills,
  openResourceSkillDir,
  readResourceSkillFile,
  readResourceSkillTokens,
  resourceSkillDirPath,
  readSkillDirFile,
  readSkillDirOverview,
  scanResourceSkill,
  scanSkillDir,
  updateResourceSkillGroups,
  type ResourceSkill,
  type ResourceSkillGroup,
  type ResourceSkillTokenFile,
  type ResourceSkillTokenReport,
} from "@/lib/resource-library";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import ScrollArea from "@/components/common/ScrollArea.vue";

const route = useRoute();
const router = useRouter();
const { t, te, locale } = useI18n();
const settingsStore = useSettingsStore();

const skillId = computed(() => String(route.params.id ?? ""));
/** 本地目录模式(项目内非托管技能):query.dir 为技能目录绝对路径,不经资源库 */
const localDir = computed(() => {
  const value = route.query.dir;
  return typeof value === "string" ? value : "";
});
const isLocal = computed(() => !!localDir.value);
/** 扫描报告缓存键:库技能用 id,本地目录用 dir: 前缀路径 */
const scanCacheId = computed(() => (isLocal.value ? `dir:${localDir.value}` : skillId.value));
/** 本地目录模式的名称/描述(来自目录报告的 frontmatter 解析) */
const localMeta = ref<{ name: string; description: string } | null>(null);
const skill = ref<ResourceSkill | null>(null);
const tokens = ref<ResourceSkillTokenReport | null>(null);
const loadingTokens = ref(false);

/** 右侧选中内容:文件或安全扫描 */
type Selection = { kind: "file"; path: string } | { kind: "scan" };
const selected = ref<Selection>({ kind: "file", path: "SKILL.md" });

// ── 图片预览(与 ProjectFiles 共用 useImagePreview):asset 协议直显 ──
const selectedFilePath = computed(() =>
  selected.value.kind === "file" ? selected.value.path : null,
);
/** 资源库技能的磁盘目录(图片拼绝对路径用;本地模式直接用 localDir) */
const librarySkillDir = ref<string | null>(null);
const { isImage, isSvg, svgMode, svgSource, imageSrc, onSelectImage, isVideo, videoSrc } =
  useImagePreview(selectedFilePath, (path) => {
    const root = isLocal.value ? localDir.value : librarySkillDir.value;
    return root ? joinPath(root, path) : null;
  });

// ── Markdown 渲染(与 AI 抽屉同一套配置)────────────────────────────────
const language = computed(() => locale.value as SupportedLocale);
const controls: ControlsConfig = {
  table: { copy: true, download: true },
  code: { copy: true, collapse: true },
};
const beforeDownload = createBeforeDownload(t);
// 传游离元素,避免 Markdown 库将 island/pixel 的十六进制主题变量写成无效的 hsl(#…)
const detachedThemeEl = document.createElement("div");
const themeElement = () => detachedThemeEl;
// 自定义链接渲染器:绕过库内置 harden(裸相对路径如 references/x.md 会被误判拦截),
// 输出真实 href,点击行为统一由 onMarkdownClick 拦截(同 ProjectFiles)
const nodeRenderers: NodeRenderers = { link: MdLink };

onMounted(() => {
  void loadSkill();
  void loadTokens();
});

async function loadSkill() {
  if (isLocal.value) {
    return;
  }
  try {
    const list = await listResourceSkills();
    allGroups.value = list.groups;
    skill.value = list.skills.find((s) => s.id === skillId.value) ?? null;
    librarySkillDir.value = await resourceSkillDirPath(skillId.value);
  } catch (e) {
    toast.error(String(e));
  }
}

async function loadTokens() {
  loadingTokens.value = true;
  try {
    if (isLocal.value) {
      const report = await readSkillDirOverview(localDir.value);
      tokens.value = {
        id: scanCacheId.value,
        descriptionTokens: report.descriptionTokens,
        totalTokens: report.totalTokens,
        files: report.files,
        hash: report.hash,
      };
      localMeta.value = { name: report.name, description: report.description };
    } else {
      tokens.value = await readResourceSkillTokens(skillId.value);
    }
    hydrateScanCache();
    // 默认选中 SKILL.md;缺失时选第一个文件,目录为空则落在安全扫描
    const files = tokens.value?.files ?? [];
    if (selected.value.kind === "file" && selected.value.path === "SKILL.md") {
      if (!files.some((f) => f.path === "SKILL.md")) {
        selected.value = files.length ? { kind: "file", path: files[0].path } : { kind: "scan" };
      }
    }
    // 默认选中的文件不经过 selectFile,内容需在此触发加载
    if (selected.value.kind === "file") {
      void ensureFileContent(selected.value.path);
    }
  } catch (e) {
    toast.error(String(e));
  } finally {
    loadingTokens.value = false;
  }
}

function goBack() {
  // 项目 AI 资产页等入口经 from 回参指定来源,优先返回来源页;
  // 否则回设置页,带 category 回参落在「资源管理」分类
  const from = route.query.from;
  if (typeof from === "string" && from.startsWith("/")) {
    void router.push(from);
    return;
  }
  void router.push({ path: "/settings", query: { category: "resources" } });
}

function openSourcePage() {
  const url = skill.value?.marketplace?.url;
  if (url) {
    openUrl(url).catch((e) => toast.error(String(e)));
  }
}

async function openDir() {
  try {
    if (isLocal.value) {
      await openPath(localDir.value);
    } else {
      await openResourceSkillDir(skillId.value);
    }
  } catch (e) {
    toast.error(String(e));
  }
}

// ── 分组归属:头部按钮打开对话框,勾选分组后整体保存 ─────────────────────
const groupsDialogOpen = ref(false);
const allGroups = ref<ResourceSkillGroup[]>([]);
const selectedGroupIds = ref(new Set<string>());
const savingGroups = ref(false);

function openGroupsDialog() {
  if (!skill.value) return;
  selectedGroupIds.value = new Set(skill.value.groupIds);
  groupsDialogOpen.value = true;
  void loadGroups();
}

async function loadGroups() {
  try {
    allGroups.value = (await listResourceSkills()).groups;
  } catch (e) {
    toast.error(String(e));
  }
}

function toggleGroup(groupId: string) {
  const next = new Set(selectedGroupIds.value);
  if (!next.delete(groupId)) {
    next.add(groupId);
  }
  selectedGroupIds.value = next;
}

async function saveGroups() {
  if (!skill.value || savingGroups.value) return;
  savingGroups.value = true;
  try {
    skill.value = await updateResourceSkillGroups(skill.value.id, [...selectedGroupIds.value]);
    groupsDialogOpen.value = false;
    toast.success(t("settings.resources.skills.previewPage.groups.updated"));
  } catch (e) {
    toast.error(String(e));
  } finally {
    savingGroups.value = false;
  }
}

/** 头部徽标展示的所属分组;分组记录缺失时回退原始 id */
const skillGroupEntries = computed(() => {
  if (!skill.value) return [];
  const map = new Map(allGroups.value.map((group) => [group.id, group]));
  return skill.value.groupIds.map((id) => ({
    id,
    name: map.get(id)?.name ?? id,
    color: map.get(id)?.color,
  }));
});

// ── 左侧列表:文件树与 token ───────────────────────────────────────────
function isMarkdown(path: string): boolean {
  return path.toLowerCase().endsWith(".md");
}

function formatTokens(value: number): string {
  return value.toLocaleString();
}

/** 技能目录文件树:buildFileTree 聚合层级(目录在前),默认全部展开;
 *  SKILL.md 行尾以「描述/内容」展示技能级 token 占用 */
const treeNodes = computed(() => buildFileTree(tokens.value?.files ?? []));
const collapsedDirs = ref(new Set<string>());
const treeRows = computed(() => flattenVisibleTree(treeNodes.value, collapsedDirs.value));

function onTreeToggle(row: FileTreeRow<ResourceSkillTokenFile>) {
  const next = new Set(collapsedDirs.value);
  if (next.has(row.fullPath)) {
    next.delete(row.fullPath);
  } else {
    next.add(row.fullPath);
  }
  collapsedDirs.value = next;
}

function onTreeSelect(row: FileTreeRow<ResourceSkillTokenFile>) {
  if (row.data) {
    selectFile(row.fullPath);
  }
}

// ── 右侧:文件内容 ─────────────────────────────────────────────────────
const fileCache = new Map<string, string | null>();
/** 正在加载的文件路径(null = 无在途加载);按路径判断,避免连续切换时加载态互相覆盖 */
const loadingPath = ref<string | null>(null);
const fileContent = ref<string | null>(null);

/** 按当前选中文件同步内容(缓存命中立即展示,未命中保持 null 等待加载) */
function syncContent() {
  fileContent.value =
    selected.value.kind === "file" ? (fileCache.get(selected.value.path) ?? null) : null;
}

async function ensureFileContent(path: string) {
  // 图片/视频走 asset 协议直显,不读文本内容(svg 源码模式除外)
  const selExt = extOf(path);
  if ((IMAGE_EXTS.has(selExt) && !svgSource.value) || VIDEO_EXTS.has(selExt)) {
    fileContent.value = null;
    return;
  }
  if (fileCache.has(path)) {
    syncContent();
    return;
  }
  loadingPath.value = path;
  syncContent();
  try {
    const result = isLocal.value
      ? await readSkillDirFile(localDir.value, path)
      : await readResourceSkillFile(skillId.value, path);
    fileCache.set(path, result.content);
  } catch (e) {
    toast.error(t("settings.resources.skills.previewPage.readFailed", { error: String(e) }));
  } finally {
    if (loadingPath.value === path) {
      loadingPath.value = null;
    }
  }
  syncContent();
}

function selectFile(path: string) {
  if (selected.value.kind === "file" && selected.value.path === path) return;
  resetTranslation();
  selected.value = { kind: "file", path };
  onSelectImage(path);
  void ensureFileContent(path);
}

// svg 预览/源码切换:源码模式需补读文本内容,切回预览时由 ensureFileContent 的守卫清空
watch(svgSource, () => {
  if (selected.value.kind === "file") void ensureFileContent(selected.value.path);
});

function selectScan() {
  if (selected.value.kind === "scan") return;
  resetTranslation();
  selected.value = { kind: "scan" };
}

/** 相对链接解析基准 = 当前 md 文件所在目录(技能内相对路径,"." 表示技能根) */
const mdBaseDir = computed(() =>
  selected.value.kind === "file"
    ? selected.value.path.split("/").slice(0, -1).join("/") || "."
    : ".",
);
/** 技能目录内全部文件路径(链接跳转前的存在性校验) */
const filePathSet = computed(() => new Set((tokens.value?.files ?? []).map((f) => f.path)));

/** md 渲染容器(页内锚点滚动时限定查找范围) */
const mdContainerRef = ref<HTMLElement | null>(null);

/** GitHub 风格标题 slug:小写、去标点(保留中日韩等文字/数字/_/-)、空白转连字符 */
function slugifyHeading(text: string): string {
  return text
    .trim()
    .toLowerCase()
    .replace(/[^\p{L}\p{N}_\- ]/gu, "")
    .replace(/\s+/g, "-");
}

/** 页内锚点:库渲染的标题不带 id,按 GitHub slug 规则(重名追加 -1/-2)匹配并滚动到位 */
function scrollToAnchor(hash: string) {
  const container = mdContainerRef.value;
  if (!container) return;
  let target = hash;
  try {
    target = decodeURIComponent(hash);
  } catch {
    // 含非法 % 序列时按原样匹配
  }
  target = target.toLowerCase();
  const seen = new Map<string, number>();
  for (const el of container.querySelectorAll<HTMLElement>("h1, h2, h3, h4, h5, h6")) {
    const base = slugifyHeading(el.textContent ?? "");
    const count = seen.get(base) ?? 0;
    seen.set(base, count + 1);
    if ((count === 0 ? base : `${base}-${count}`) === target) {
      el.scrollIntoView({ behavior: "smooth", block: "start" });
      return;
    }
  }
}

/** md 内链接点击:页内锚点滚动到对应标题;外链交系统浏览器;相对路径解析为技能内文件并在右侧切换查看 */
async function onMarkdownClick(e: MouseEvent) {
  const a = (e.target as HTMLElement).closest("a");
  if (!a) return;
  // MdLink 已按协议白名单过滤,这里再过一遍 safeLinkHref 兜底(防其他来源的 <a>)
  const href = safeLinkHref(a.getAttribute("href"));
  e.preventDefault();
  if (!href) return;
  if (href.startsWith("#")) {
    scrollToAnchor(href.slice(1));
    return;
  }
  // safeLinkHref 放行后即只剩白名单协议(http/https/mailto),可安全交系统浏览器
  if (hasScheme(href)) {
    await openUrl(href).catch(() => {});
    return;
  }
  const target = resolvePath(mdBaseDir.value, href);
  if (filePathSet.value.has(target)) {
    selectFile(target);
  } else {
    toast.error(t("settings.resources.skills.previewPage.linkTargetMissing", { path: target }));
  }
}

// ── 右侧:翻译(md 文件;后端 ai_translate_markdown 优先用翻译专用配置、未配置回退
//  设置页默认模型;译文按内容 hash 缓存 30 天;状态机与 AI 抽屉/远程审计共用)──
const translation = useMarkdownTranslation({
  getIdentity: () => (selected.value.kind === "file" ? selected.value.path : null),
  getText: () => fileContent.value,
  getLanguage: () => settingsStore.language,
});
const {
  translating,
  showTranslated,
  hasTranslation,
  toggle: toggleTranslate,
  retranslate,
  reset: resetTranslation,
} = translation;

const isTranslatable = computed(
  () => selected.value.kind === "file" && isMarkdown(selected.value.path),
);

const displayContent = computed(() => translation.displayContent(fileContent.value ?? ""));

// ── 右侧:安全扫描(静态规则 + 内置 Agent 语义层;生命周期由 useSkillScan 持有,
//  结果展示在 components/skill-preview/SkillScanPanel)────────────────────
/** 命令错误优先走 errors.<code> i18n 通道,回落原始字符串 */
function translateError(e: unknown): string {
  const code = (e as { code?: string } | null)?.code;
  if (code && te(`errors.${code}`)) {
    return t(`errors.${code}`);
  }
  return String(e);
}

const {
  scanning,
  scanReport,
  scanElapsedLabel,
  runScan,
  cancelScan,
  hydrateCache: hydrateScanCache,
  dispose: disposeScan,
} = useSkillScan({
  scan: (options) =>
    isLocal.value
      ? scanSkillDir(localDir.value, options)
      : scanResourceSkill(skillId.value, options),
  getLanguage: () => settingsStore.language,
  getCacheId: () => scanCacheId.value,
  getFingerprint: () => tokens.value?.hash,
  formatError: translateError,
});

/** 面板发起扫描:model 为 null 时跟随默认模型(后端回退) */
function onRunScan(model: SkillScanModelRef | null) {
  void runScan(model);
}

/** 侧栏扫描入口徽标(配色/等级标签与结果面板共用 scan-labels) */
function levelBadgeClass(level: string): string {
  return scanLevelClass(level);
}
function levelBadgeLabel(level: string): string {
  return scanLevelLabel({ t, te }, level);
}

// 离开页面:取消在途翻译与扫描并作废晚到结果(旧实现只清了计时器,任务仍在跑)
onBeforeUnmount(() => {
  resetTranslation();
  disposeScan();
});
</script>

<template>
  <div class="flex h-full flex-col">
    <header class="flex shrink-0 items-center gap-2 border-b px-4 py-2.5">
      <Button
        variant="ghost"
        size="icon"
        class="h-8 w-8"
        :title="t('settings.back')"
        @click="goBack"
      >
        <ArrowLeft class="h-4 w-4" />
      </Button>
      <div class="min-w-0">
        <h1
          class="truncate text-sm font-semibold"
          :title="skill?.name ?? localMeta?.name ?? skillId"
        >
          {{ skill?.name ?? localMeta?.name ?? skillId }}
        </h1>
        <p
          v-if="skill?.description ?? localMeta?.description"
          class="truncate text-xs text-muted-foreground"
        >
          {{ skill?.description ?? localMeta?.description }}
        </p>
        <!-- 分组操作与所属标签集中展示 -->
        <div
          v-if="!isLocal || skillGroupEntries.length"
          class="mt-1 flex flex-wrap items-center gap-1"
        >
          <Button
            v-if="!isLocal"
            variant="ghost"
            size="sm"
            class="h-6 shrink-0 gap-1 rounded-full px-2 text-[11px] text-muted-foreground"
            :title="t('settings.resources.skills.previewPage.groups.button')"
            @click="openGroupsDialog"
          >
            <ListPlus class="h-3 w-3" />
            {{ t("settings.resources.skills.previewPage.groups.button") }}
          </Button>

          <span
            v-for="entry in skillGroupEntries"
            :key="entry.id"
            class="flex items-center gap-1 rounded-full bg-muted px-2 py-0.5 text-[11px] text-muted-foreground"
          >
            <span
              class="h-1.5 w-1.5 shrink-0 rounded-full"
              :style="{ backgroundColor: entry.color ?? 'var(--muted-foreground)' }"
            />
            {{ entry.name }}
          </span>
        </div>
      </div>
      <div class="ml-auto flex shrink-0 items-center gap-1.5">
        <Button
          v-if="skill?.marketplace"
          variant="secondary"
          size="sm"
          class="h-8 gap-1.5"
          :disabled="!skill.marketplace.url"
          :title="t('settings.resources.skills.openSource', { source: skill.marketplace.source })"
          @click="openSourcePage"
        >
          <ExternalLink class="h-3.5 w-3.5" />
          {{ skill.marketplace.source }}
        </Button>
        <Button variant="ghost" size="sm" class="h-8 gap-1.5" @click="openDir">
          <FolderOpen class="h-3.5 w-3.5" />
          {{ t("settings.resources.skills.previewPage.openDir") }}
        </Button>
      </div>
    </header>

    <div class="flex min-h-0 flex-1">
      <!-- 左侧:文件树 + 安全扫描入口 -->
      <aside class="flex w-72 shrink-0 flex-col border-r">
        <ScrollArea class="min-h-0 flex-1">
          <p v-if="loadingTokens" class="px-3 py-4 text-center text-xs text-muted-foreground">
            {{ t("common.loading") }}
          </p>
          <p
            v-else-if="!tokens?.files.length"
            class="px-3 py-4 text-center text-xs text-muted-foreground"
          >
            {{ t("settings.resources.skills.previewPage.filesEmpty") }}
          </p>
          <FileTreeList
            v-else
            size="sm"
            :rows="treeRows"
            :selected="selected.kind === 'file' ? selected.path : null"
            @select="onTreeSelect"
            @toggle="onTreeToggle"
          >
            <template #trailing="{ row }">
              <span
                v-if="row.data && row.data.tokens === null"
                class="shrink-0 text-[11px] opacity-70"
              >
                {{ t("settings.resources.skills.previewPage.binary") }}
              </span>
              <!-- SKILL.md 行尾以「描述/内容」展示技能级 token 占用 -->
              <span
                v-else-if="row.data && row.data.path === 'SKILL.md'"
                class="shrink-0 font-mono text-[11px] text-muted-foreground"
                :title="`${t('settings.resources.skills.previewPage.summary.description')} / ${t('settings.resources.skills.previewPage.summary.content')}`"
              >
                {{ formatTokens(tokens?.descriptionTokens ?? 0) }}/{{
                  formatTokens(row.data.tokens ?? 0)
                }}
              </span>
              <span
                v-else-if="row.data"
                class="shrink-0 font-mono text-[11px] text-muted-foreground"
              >
                {{ formatTokens(row.data.tokens ?? 0) }}
              </span>
            </template>
          </FileTreeList>
        </ScrollArea>

        <div class="shrink-0 border-t p-1.5">
          <button
            type="button"
            class="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-xs transition-colors"
            :class="
              selected.kind === 'scan'
                ? 'bg-accent text-foreground'
                : 'text-muted-foreground hover:bg-accent/60 hover:text-foreground'
            "
            @click="selectScan"
          >
            <ShieldAlert class="h-3.5 w-3.5 shrink-0" />
            <span class="min-w-0 flex-1 truncate">
              {{ t("settings.resources.skills.previewPage.scan.title") }}
            </span>
            <RefreshCw v-if="scanning" class="h-3 w-3 shrink-0 animate-spin" />
            <span
              v-else-if="scanReport"
              class="shrink-0 rounded-full border px-1.5 py-0.5 text-[11px] font-medium"
              :class="levelBadgeClass(scanReport.level)"
            >
              {{ levelBadgeLabel(scanReport.level) }} {{ scanReport.score }}
            </span>
          </button>
        </div>
      </aside>

      <!-- 右侧:选中内容 -->
      <div class="flex min-w-0 flex-1 flex-col">
        <div
          v-if="selected.kind === 'file'"
          class="flex shrink-0 items-center gap-2 border-b px-4 py-2"
        >
          <span
            class="min-w-0 truncate font-mono text-xs text-muted-foreground"
            :title="selected.path"
          >
            {{ selected.path }}
          </span>
          <div
            v-if="selected.kind === 'file' && isSvg"
            class="ml-auto flex shrink-0 items-center gap-1"
          >
            <Button
              variant="ghost"
              size="icon"
              class="h-7 w-7"
              :class="svgMode === 'preview' ? 'bg-accent' : ''"
              :title="t('files.rendered')"
              @click="svgMode = 'preview'"
            >
              <Eye class="h-3.5 w-3.5" />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              class="h-7 w-7"
              :class="svgMode === 'source' ? 'bg-accent' : ''"
              :title="t('files.source')"
              @click="svgMode = 'source'"
            >
              <Code class="h-3.5 w-3.5" />
            </Button>
          </div>
          <div v-if="isTranslatable" class="ml-auto flex shrink-0 items-center gap-1">
            <Button
              v-if="hasTranslation"
              variant="ghost"
              size="icon"
              class="h-7 w-7"
              :disabled="translating"
              :title="t('settings.resources.skills.previewPage.translate.retranslate')"
              @click="retranslate"
            >
              <RefreshCw class="h-3.5 w-3.5" />
            </Button>
            <Button
              variant="ghost"
              size="sm"
              class="h-7 gap-1 px-2 text-xs"
              :disabled="!fileContent"
              @click="toggleTranslate"
            >
              <Languages class="h-3.5 w-3.5" :class="{ 'animate-pulse': translating }" />
              {{
                t(
                  translating
                    ? "settings.resources.skills.previewPage.translate.translating"
                    : showTranslated
                      ? "settings.resources.skills.previewPage.translate.showOriginal"
                      : hasTranslation
                        ? "settings.resources.skills.previewPage.translate.showTranslation"
                        : "settings.resources.skills.previewPage.translate.trigger",
                )
              }}
            </Button>
          </div>
        </div>

        <!-- 安全审计:组件自管布局——左侧内容(含标题)原生滚动,右侧来源卡片固定不随滚动 -->
        <ResourcePublicAudits
          v-if="selected.kind === 'scan'"
          class="min-h-0 flex-1"
          :marketplace-id="skill ? marketplaceAuditId(skill) : null"
          :local-scan="scanReport"
          :scanning="scanning"
        >
          <template #local>
            <SkillScanPanel
              :scanning="scanning"
              :report="scanReport"
              :elapsed-label="scanElapsedLabel"
              @run="onRunScan"
              @cancel="cancelScan"
            />
          </template>
        </ResourcePublicAudits>

        <ScrollArea v-else class="min-h-0 flex-1">
          <!-- 文件内容(注意:这里不能再包一层无指令的 <template>,否则会被渲染成真实 DOM 节点导致内容不可见) -->
          <p
            v-if="loadingPath === selected.path"
            class="py-8 text-center text-xs text-muted-foreground"
          >
            {{ t("common.loading") }}
          </p>
          <div v-else-if="isVideo" class="h-full min-h-0">
            <VideoViewer v-if="videoSrc" :src="videoSrc" :name="selected.path" />
            <p v-else class="px-3 py-8 text-center text-xs text-muted-foreground">
              {{ t("settings.resources.skills.previewPage.fileBinary") }}
            </p>
          </div>
          <div v-else-if="isImage && !svgSource" class="h-full min-h-0">
            <ImageViewer v-if="imageSrc" :src="imageSrc" :svg="isSvg" :alt="selected.path" />
            <p v-else class="px-3 py-8 text-center text-xs text-muted-foreground">
              {{ t("settings.resources.skills.previewPage.fileBinary") }}
            </p>
          </div>
          <p
            v-else-if="fileContent === null"
            class="px-3 py-8 text-center text-xs text-muted-foreground"
          >
            {{ t("settings.resources.skills.previewPage.fileBinary") }}
          </p>
          <p
            v-else-if="!displayContent"
            class="px-3 py-8 text-center text-xs text-muted-foreground"
          >
            {{ t("settings.resources.skills.previewPage.emptyFile") }}
          </p>
          <div
            v-else-if="isMarkdown(selected.path)"
            ref="mdContainerRef"
            class="mx-auto max-w-3xl p-4"
            @click="onMarkdownClick"
          >
            <Markdown
              mode="static"
              :content="displayContent"
              :controls="controls"
              :theme-element="themeElement"
              :locale="language"
              :before-download="beforeDownload"
              :node-renderers="nodeRenderers"
            />
          </div>
          <div v-else class="h-full">
            <CodeViewer :text="fileContent ?? ''" :path="selected.path" :wrap="true" />
          </div>
        </ScrollArea>
      </div>
    </div>

    <!-- 分组归属对话框:勾选分组整体保存,与 Skills 页「管理分组」共用数据 -->
    <Dialog :open="groupsDialogOpen" @update:open="groupsDialogOpen = $event">
      <DialogContent class="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{{ t("settings.resources.skills.groups.title") }}</DialogTitle>
          <DialogDescription>
            {{ t("settings.resources.skills.previewPage.groups.hint") }}
          </DialogDescription>
        </DialogHeader>

        <ScrollArea class="max-h-64">
          <button
            v-for="group in allGroups"
            :key="group.id"
            type="button"
            class="flex w-full items-start gap-2 rounded-md px-2 py-1.5 text-left transition-colors hover:bg-accent"
            @click="toggleGroup(group.id)"
          >
            <span
              class="mt-0.5 flex h-4 w-4 shrink-0 items-center justify-center rounded border transition-colors"
              :class="
                selectedGroupIds.has(group.id)
                  ? 'border-primary bg-primary text-primary-foreground'
                  : 'border-input'
              "
            >
              <Check v-if="selectedGroupIds.has(group.id)" class="h-3 w-3" />
            </span>
            <span class="min-w-0 flex-1">
              <span class="flex min-w-0 items-center gap-2 text-xs">
                <span
                  class="h-2.5 w-2.5 shrink-0 rounded-full"
                  :style="{ backgroundColor: group.color }"
                />
                <span class="truncate">{{ group.name }}</span>
              </span>
              <span
                v-if="group.description"
                class="block truncate text-[11px] text-muted-foreground"
                :title="group.description"
              >
                {{ group.description }}
              </span>
            </span>
          </button>
          <p v-if="!allGroups.length" class="px-2 py-6 text-center text-xs text-muted-foreground">
            {{ t("settings.resources.skills.previewPage.groups.empty") }}
          </p>
        </ScrollArea>

        <DialogFooter>
          <Button variant="outline" :disabled="savingGroups" @click="groupsDialogOpen = false">
            {{ t("common.cancel") }}
          </Button>
          <Button :disabled="savingGroups || !allGroups.length" @click="saveGroups">
            {{ t("common.save") }}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </div>
</template>
