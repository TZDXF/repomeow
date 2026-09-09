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
  ShieldCheck,
} from "@lucide/vue";
import { Markdown, type ControlsConfig, type NodeRenderers } from "vue-stream-markdown";
import FileTreeList from "@/components/common/FileTreeList.vue";
import {
  ModelSelector,
  modelDisplayName,
  parseModelOptionValue,
  type ModelSelectorGroup,
} from "@/components/ai-elements/model-selector";
import CodeViewer from "@/components/files/CodeViewer.vue";
import ImageViewer from "@/components/files/ImageViewer.vue";
import MdLink from "@/components/markdown/MdLink.vue";
import type { SupportedLocale } from "@/i18n";
import { buildFileTree, flattenVisibleTree, type FileTreeRow } from "@/lib/file-tree";
import { formatRelativeTime } from "@/lib/format";
import { createBeforeDownload } from "@/lib/markdown-download";
import { hasScheme, resolvePath } from "@/lib/markdown";
import { extOf, IMAGE_EXTS } from "@/lib/file-kind";
import { joinPath } from "@/lib/path";
import { useImagePreview } from "@/composables/files/useImagePreview";
import { getCachedScanReport, putCachedScanReport } from "@/lib/scan-cache";
import { cmd } from "@/lib/tauri";
import { getCachedTranslation, putCachedTranslation } from "@/lib/translation-cache";
import { useAiConfigStore } from "@/stores/ai-config";
import { useSettingsStore } from "@/stores/settings";
import {
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
  type ResourceSkillScanFinding,
  type ResourceSkillScanReport,
  type ResourceSkillTokenFile,
  type ResourceSkillTokenReport,
} from "@/lib/resource-library";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ScrollArea } from "@/components/ui/scroll-area";

const route = useRoute();
const router = useRouter();
const { t, te, locale } = useI18n();
const settingsStore = useSettingsStore();
const aiConfig = useAiConfigStore();

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
const { isImage, isSvg, svgMode, svgSource, imageSrc, onSelectImage } = useImagePreview(
  selectedFilePath,
  (path) => {
    const root = isLocal.value ? localDir.value : librarySkillDir.value;
    return root ? joinPath(root, path) : null;
  },
);

// ── Markdown 渲染(与 AI 抽屉同一套配置)────────────────────────────────
const language = computed(() => locale.value as SupportedLocale);
const controls: ControlsConfig = {
  table: { copy: true, download: true },
  code: { copy: true, collapse: true },
};
const beforeDownload = createBeforeDownload(t);
// 传游离元素,避免 Markdown 库将 island/glass 的十六进制主题变量写成无效的 hsl(#…)
const detachedThemeEl = document.createElement("div");
const themeElement = () => detachedThemeEl;
// 自定义链接渲染器:绕过库内置 harden(裸相对路径如 references/x.md 会被误判拦截),
// 输出真实 href,点击行为统一由 onMarkdownClick 拦截(同 ProjectFiles)
const nodeRenderers: NodeRenderers = { link: MdLink };

onMounted(() => {
  void loadSkill();
  void loadTokens();
  // 安全扫描的模型选择器数据源(已加载时复用内存副本)
  aiConfig.ensureLoaded().catch(() => {});
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
  // 图片走 asset 协议直显,不读文本内容(svg 源码模式除外)
  if (IMAGE_EXTS.has(extOf(path)) && !svgSource.value) {
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
  const href = a.getAttribute("href");
  e.preventDefault();
  if (!href) return;
  if (href.startsWith("#")) {
    scrollToAnchor(href.slice(1));
    return;
  }
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

// ── 右侧:翻译(md 文件;走设置页默认模型,经后端 ai_translate_markdown;译文按内容 hash 缓存 30 天)──
const translating = ref(false);
/** 已缓存译文的文件路径;与当前选中文件不符时不显示译文 */
const translatedFor = ref<string | null>(null);
const translatedText = ref<string | null>(null);
const showTranslated = ref(false);
/** 当前在途翻译的 runId(ai_cancel_run 的取消句柄);空 = 无在途请求 */
let translateRunId = "";
/** 翻译轮次序号:取消/换文件后自增,使仍在途的缓存查询结果作废 */
let translateSeq = 0;

const isTranslatable = computed(
  () => selected.value.kind === "file" && isMarkdown(selected.value.path),
);

const displayContent = computed(() => {
  if (
    showTranslated.value &&
    translatedText.value !== null &&
    translatedFor.value === (selected.value.kind === "file" ? selected.value.path : null)
  ) {
    return translatedText.value;
  }
  return fileContent.value ?? "";
});

function resetTranslation() {
  translateSeq += 1;
  if (translateRunId) {
    void cmd<void>("ai_cancel_run", { runId: translateRunId }).catch(() => {});
    translateRunId = "";
  }
  translating.value = false;
  translatedText.value = null;
  translatedFor.value = null;
  showTranslated.value = false;
}

async function toggleTranslate() {
  // 翻译中再点 = 取消;已有译文 = 原文/译文切换
  if (translating.value) {
    resetTranslation();
    return;
  }
  if (showTranslated.value) {
    showTranslated.value = false;
    return;
  }
  const path = selected.value.kind === "file" ? selected.value.path : null;
  const text = fileContent.value;
  if (!path || !text) return;
  if (translatedFor.value === path) {
    showTranslated.value = true;
    return;
  }
  const seq = ++translateSeq;
  // 先查 IndexedDB 缓存(键 = 界面语言 + 正文内容 hash,保留 30 天):
  // 命中直接展示、不再调 AI,未命中才走后端翻译并回填缓存
  const cached = await getCachedTranslation(text, settingsStore.language);
  if (seq !== translateSeq || selected.value.kind !== "file" || selected.value.path !== path) {
    return;
  }
  if (cached !== null) {
    translatedText.value = cached;
    translatedFor.value = path;
    showTranslated.value = true;
    return;
  }
  translating.value = true;
  const runId = `translate-${Date.now()}-${Math.random().toString(36).slice(2)}`;
  translateRunId = runId;
  try {
    const result = await cmd<string | null>("ai_translate_markdown", {
      request: { text, language: settingsStore.language, runId },
    });
    // 取消后返回 null,同样忽略;换文件/切到扫描后的在途结果作废
    if (result === null || selected.value.kind !== "file" || selected.value.path !== path) return;
    translatedText.value = result;
    translatedFor.value = path;
    showTranslated.value = true;
    void putCachedTranslation(text, settingsStore.language, result);
  } catch (e) {
    toast.error(String(e));
  } finally {
    if (translateRunId === runId) {
      translateRunId = "";
      translating.value = false;
    }
  }
}

// ── 右侧:安全扫描(静态规则 + 内置 Agent 语义层)──────────────────────
const scanning = ref(false);
const scanReport = ref<ResourceSkillScanReport | null>(null);
let scanRunId = "";

/** 扫描进行中的已用时长(秒),驱动进度面板的计时显示 */
const scanElapsed = ref(0);
let scanElapsedTimer: ReturnType<typeof setInterval> | null = null;

watch(scanning, (on) => {
  if (scanElapsedTimer) {
    clearInterval(scanElapsedTimer);
    scanElapsedTimer = null;
  }
  if (on) {
    scanElapsed.value = 0;
    scanElapsedTimer = setInterval(() => {
      scanElapsed.value += 1;
    }, 1000);
  }
});

onBeforeUnmount(() => {
  if (scanElapsedTimer) clearInterval(scanElapsedTimer);
});

const scanElapsedLabel = computed(() => {
  const minutes = Math.floor(scanElapsed.value / 60);
  const seconds = scanElapsed.value % 60;
  return `${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`;
});

// ── 扫描模型选择:复合值 "providerId/modelId",缺省跟随设置页默认模型 ──
// 显式引用失效(厂商/模型被删)时后端会回退默认模型,前端在配置加载后
// 把选项归位,保证触发器显示与实际生效模型一致。
const SCAN_MODEL_STORAGE_KEY = "repomeow.rl-scan-model";
/** 通用选项哨兵值:不含 "/",parseModelOptionValue 解析为 null = 跟随默认模型 */
const SCAN_MODEL_DEFAULT = "default";
const scanModelValue = ref(localStorage.getItem(SCAN_MODEL_STORAGE_KEY) ?? SCAN_MODEL_DEFAULT);

const scanDefaultModelLabel = computed(() => {
  const model = aiConfig.defaultModel?.model;
  return model
    ? t("settings.resources.skills.previewPage.scan.modelDefaultNamed", {
        model: modelDisplayName(model),
      })
    : t("settings.resources.skills.previewPage.scan.modelDefault");
});

const scanModelGroups = computed<ModelSelectorGroup[]>(() => {
  const config = aiConfig.config;
  if (!config) return [];
  return Object.entries(config.providers).map(([providerId, provider]) => ({
    providerId,
    providerName: provider.name || providerId,
    models: provider.models,
  }));
});

watch(
  () => aiConfig.loaded,
  (loaded) => {
    if (!loaded || scanModelValue.value === SCAN_MODEL_DEFAULT) return;
    const valid = scanModelGroups.value.some((group) =>
      group.models.some((model) => `${group.providerId}/${model.id}` === scanModelValue.value),
    );
    if (!valid) scanModelValue.value = SCAN_MODEL_DEFAULT;
  },
  { immediate: true },
);

watch(scanModelValue, (value) => {
  if (value === SCAN_MODEL_DEFAULT) {
    localStorage.removeItem(SCAN_MODEL_STORAGE_KEY);
  } else {
    localStorage.setItem(SCAN_MODEL_STORAGE_KEY, value);
  }
});

/** 命令错误优先走 errors.<code> i18n 通道,回落原始字符串 */
function translateError(e: unknown): string {
  const code = (e as { code?: string } | null)?.code;
  if (code && te(`errors.${code}`)) {
    return t(`errors.${code}`);
  }
  return String(e);
}

/** 打开页面即恢复上次扫描报告(IndexedDB 缓存,内容指纹一致才命中) */
function hydrateScanCache() {
  const fingerprint = tokens.value?.hash;
  if (!fingerprint || scanReport.value || scanning.value) return;
  void getCachedScanReport(scanCacheId.value, fingerprint).then((cached) => {
    if (cached && !scanReport.value && !scanning.value) {
      scanReport.value = cached;
    }
  });
}

async function runScan() {
  if (scanning.value) return;
  scanning.value = true;
  scanReport.value = null;
  const runId = `scan-${Date.now()}-${Math.random().toString(36).slice(2)}`;
  scanRunId = runId;
  const modelRef =
    scanModelValue.value === SCAN_MODEL_DEFAULT
      ? null
      : parseModelOptionValue(scanModelValue.value);
  try {
    const options = {
      language: settingsStore.language,
      runId,
      providerId: modelRef?.providerId,
      modelId: modelRef?.modelId,
    };
    const report = isLocal.value
      ? await scanSkillDir(localDir.value, options)
      : await scanResourceSkill(skillId.value, options);
    if (scanRunId === runId) {
      scanReport.value = report;
      // 成功后写入缓存;技能内容变化会改变指纹,旧缓存自动失效
      const fingerprint = tokens.value?.hash;
      if (fingerprint) void putCachedScanReport(scanCacheId.value, fingerprint, report);
    }
  } catch (e) {
    toast.error(translateError(e));
  } finally {
    if (scanRunId === runId) {
      scanRunId = "";
      scanning.value = false;
    }
  }
}

function cancelScan() {
  if (scanRunId) {
    void cmd<void>("ai_cancel_run", { runId: scanRunId }).catch(() => {});
  }
}

const SEVERITY_CLASSES: Record<string, string> = {
  critical: "border-red-600/30 bg-red-600/10 text-red-600 dark:text-red-400",
  high: "border-orange-500/30 bg-orange-500/10 text-orange-600 dark:text-orange-400",
  medium: "border-amber-500/30 bg-amber-500/10 text-amber-600 dark:text-amber-400",
  low: "border-border bg-muted text-muted-foreground",
};

const LEVEL_CLASSES: Record<string, string> = {
  low: "border-emerald-500/30 bg-emerald-500/10 text-emerald-600 dark:text-emerald-400",
  medium: "border-amber-500/30 bg-amber-500/10 text-amber-600 dark:text-amber-400",
  high: "border-orange-500/30 bg-orange-500/10 text-orange-600 dark:text-orange-400",
  critical: "border-red-600/30 bg-red-600/10 text-red-600 dark:text-red-400",
};

function severityClass(severity: string): string {
  return SEVERITY_CLASSES[severity] ?? SEVERITY_CLASSES.medium;
}

function levelClass(level: string): string {
  return LEVEL_CLASSES[level] ?? LEVEL_CLASSES.medium;
}

function levelLabel(level: string): string {
  const key = `settings.resources.skills.previewPage.scan.level.${level}`;
  return te(key) ? t(key) : level;
}

function severityLabel(severity: string): string {
  const key = `settings.resources.skills.previewPage.scan.severity.${severity}`;
  return te(key) ? t(key) : severity;
}

function categoryLabel(finding: ResourceSkillScanFinding): string {
  const key = `settings.resources.skills.previewPage.scan.category.${finding.category}`;
  return te(key) ? t(key) : finding.category;
}

/** 静态发现标题走规则 i18n;AI 发现直接用模型输出标题 */
function findingTitle(finding: ResourceSkillScanFinding): string {
  if (finding.ruleId) {
    const key = `settings.resources.skills.previewPage.scan.rules.${finding.ruleId}`;
    if (te(key)) return t(key);
  }
  return finding.title || finding.category;
}

const llmNotice = computed(() => {
  const report = scanReport.value;
  if (!report) return "";
  if (report.llmStatus === "skipped") {
    return t("settings.resources.skills.previewPage.scan.llmSkipped");
  }
  if (report.llmStatus === "canceled") {
    return t("settings.resources.skills.previewPage.scan.llmCanceled");
  }
  if (report.llmStatus === "failed") {
    return t("settings.resources.skills.previewPage.scan.llmFailed", {
      error: report.llmErrorMessage || report.llmErrorCode || "",
    });
  }
  return "";
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
        <!-- 当前技能所属分组徽标,样式与 Skills 列表卡片一致 -->
        <div v-if="skillGroupEntries.length" class="flex flex-wrap items-center gap-1">
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
        <Button
          v-if="!isLocal"
          variant="ghost"
          size="sm"
          class="h-8 gap-1.5"
          :title="t('settings.resources.skills.previewPage.groups.button')"
          @click="openGroupsDialog"
        >
          <ListPlus class="h-3.5 w-3.5" />
          {{ t("settings.resources.skills.previewPage.groups.button") }}
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
              :class="levelClass(scanReport.level)"
            >
              {{ levelLabel(scanReport.level) }} {{ scanReport.score }}
            </span>
          </button>
        </div>
      </aside>

      <!-- 右侧:选中内容 -->
      <div class="flex min-w-0 flex-1 flex-col">
        <div class="flex shrink-0 items-center gap-2 border-b px-4 py-2">
          <span
            v-if="selected.kind === 'file'"
            class="min-w-0 truncate font-mono text-xs text-muted-foreground"
            :title="selected.path"
          >
            {{ selected.path }}
          </span>
          <span v-else class="min-w-0 truncate text-xs font-medium">
            {{ t("settings.resources.skills.previewPage.scan.title") }}
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
              v-if="translatedFor !== null && !translating"
              variant="ghost"
              size="sm"
              class="h-7 gap-1 px-2 text-xs"
              @click="showTranslated = !showTranslated"
            >
              {{
                t(
                  showTranslated
                    ? "settings.resources.skills.previewPage.translate.showOriginal"
                    : "settings.resources.skills.previewPage.translate.showTranslation",
                )
              }}
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
                    : "settings.resources.skills.previewPage.translate.trigger",
                )
              }}
            </Button>
          </div>
          <div
            v-else-if="selected.kind === 'scan'"
            class="ml-auto flex shrink-0 items-center gap-1.5"
          >
            <span :title="t('settings.resources.skills.previewPage.scan.modelLabel')">
              <ModelSelector
                v-model="scanModelValue"
                :groups="scanModelGroups"
                :disabled="scanning"
                :generic-option="{
                  value: SCAN_MODEL_DEFAULT,
                  label: scanDefaultModelLabel,
                }"
                trigger-class="text-muted-foreground min-w-0 max-w-96"
              />
            </span>
            <Button
              variant="outline"
              size="sm"
              class="h-7 shrink-0 gap-1 px-2 text-xs"
              :disabled="scanning"
              @click="runScan"
            >
              <ShieldCheck v-if="!scanReport" class="h-3.5 w-3.5" />
              <RefreshCw v-else class="h-3.5 w-3.5" />
              {{
                t(
                  scanReport
                    ? "settings.resources.skills.previewPage.scan.rerun"
                    : "settings.resources.skills.previewPage.scan.run",
                )
              }}
            </Button>
          </div>
        </div>

        <ScrollArea class="min-h-0 flex-1">
          <!-- 安全扫描 -->
          <div v-if="selected.kind === 'scan'" class="mx-auto max-w-3xl space-y-3 p-4">
            <div v-if="scanning" class="flex flex-col items-center gap-4 px-6 py-16 text-center">
              <div class="relative flex h-14 w-14 items-center justify-center">
                <span class="scan-pulse absolute inset-0 rounded-full bg-primary/15" />
                <span class="absolute inset-2 rounded-full border border-primary/25" />
                <ShieldCheck class="h-6 w-6 text-primary" />
              </div>
              <div class="space-y-1">
                <p class="text-sm font-medium">
                  {{ t("settings.resources.skills.previewPage.scan.running") }}
                </p>
                <p class="text-xs text-muted-foreground">
                  {{ t("settings.resources.skills.previewPage.scan.runningHint") }}
                </p>
              </div>
              <div class="h-0.5 w-52 overflow-hidden rounded-full bg-muted">
                <div class="scan-indeterminate h-full w-1/3 rounded-full bg-primary/70" />
              </div>
              <p class="font-mono text-[11px] tabular-nums text-muted-foreground/70">
                {{ scanElapsedLabel }}
              </p>
              <Button variant="ghost" size="sm" class="h-7 px-3 text-xs" @click="cancelScan">
                {{ t("settings.resources.skills.previewPage.scan.cancel") }}
              </Button>
            </div>

            <template v-else-if="scanReport">
              <div class="flex flex-wrap items-center gap-2">
                <span
                  class="flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs font-medium"
                  :class="levelClass(scanReport.level)"
                >
                  {{ levelLabel(scanReport.level) }}
                  <span class="font-mono">{{ scanReport.score }}</span>
                </span>
                <span class="text-xs text-muted-foreground">
                  {{
                    t("settings.resources.skills.previewPage.scan.filesScanned", {
                      count: scanReport.filesScanned,
                    })
                  }}
                </span>
                <span class="text-xs text-muted-foreground">
                  {{
                    t("settings.resources.skills.previewPage.scan.scannedAt", {
                      time: formatRelativeTime(scanReport.scannedAt),
                    })
                  }}
                </span>
                <span v-if="scanReport.suppressedCount > 0" class="text-xs text-muted-foreground">
                  {{
                    t("settings.resources.skills.previewPage.scan.suppressed", {
                      count: scanReport.suppressedCount,
                    })
                  }}
                </span>
              </div>

              <p v-if="llmNotice" class="text-xs text-amber-600 dark:text-amber-400">
                {{ llmNotice }}
              </p>
              <div
                v-else-if="scanReport.llmSummary"
                class="rounded-md border bg-muted/40 p-2.5 text-xs"
              >
                <span class="font-medium">
                  {{ t("settings.resources.skills.previewPage.scan.summary") }}
                </span>
                <span class="text-muted-foreground">{{ scanReport.llmSummary }}</span>
              </div>

              <p
                v-if="!scanReport.findings.length"
                class="rounded-md border border-dashed px-3 py-8 text-center text-xs text-muted-foreground"
              >
                {{ t("settings.resources.skills.previewPage.scan.noFindings") }}
              </p>
              <div v-else class="space-y-2">
                <div
                  v-for="(finding, index) in scanReport.findings"
                  :key="`${finding.ruleId ?? 'llm'}-${index}`"
                  class="rounded-md border p-3"
                >
                  <div class="flex flex-wrap items-center gap-1.5">
                    <span
                      class="rounded-full border px-2 py-0.5 text-[11px] font-medium"
                      :class="severityClass(finding.severity)"
                    >
                      {{ severityLabel(finding.severity) }}
                    </span>
                    <span
                      class="rounded-full bg-muted px-2 py-0.5 text-[11px] text-muted-foreground"
                    >
                      {{ categoryLabel(finding) }}
                    </span>
                    <Badge
                      variant="outline"
                      :title="
                        t(
                          finding.source === 'llm'
                            ? 'settings.resources.skills.previewPage.scan.llmAnalysis'
                            : 'settings.resources.skills.previewPage.scan.staticRules',
                        )
                      "
                    >
                      {{
                        t(
                          finding.source === "llm"
                            ? "settings.resources.skills.previewPage.scan.source.llm"
                            : "settings.resources.skills.previewPage.scan.source.static",
                        )
                      }}
                    </Badge>
                    <span
                      v-if="finding.location"
                      class="ml-auto min-w-0 truncate font-mono text-[11px] text-muted-foreground"
                      :title="finding.location"
                    >
                      {{ finding.location }}
                    </span>
                  </div>
                  <p class="mt-2 text-sm font-medium">{{ findingTitle(finding) }}</p>
                  <p v-if="finding.detail" class="mt-1 text-xs text-muted-foreground">
                    {{ finding.detail }}
                  </p>
                  <code
                    v-if="finding.evidence"
                    class="mt-2 block max-h-24 overflow-auto rounded bg-muted px-2 py-1.5 font-mono text-xs"
                  >
                    {{ finding.evidence }}
                  </code>
                </div>
              </div>
            </template>

            <p
              v-else
              class="rounded-md border border-dashed px-3 py-8 text-center text-xs text-muted-foreground"
            >
              {{ t("settings.resources.skills.previewPage.scan.notScanned") }}
            </p>
          </div>

          <!-- 文件内容 -->
          <template v-else>
            <p
              v-if="loadingPath === selected.path"
              class="py-8 text-center text-xs text-muted-foreground"
            >
              {{ t("common.loading") }}
            </p>
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
          </template>
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

        <div class="max-h-64 overflow-y-auto">
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
        </div>

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

<style scoped>
/* 扫描进行中:细进度条滑动 + 盾牌外圈脉冲 */
@keyframes scan-slide {
  0% {
    transform: translateX(-100%);
  }
  100% {
    transform: translateX(400%);
  }
}

.scan-indeterminate {
  animation: scan-slide 1.4s ease-in-out infinite;
}

@keyframes scan-pulse {
  0% {
    transform: scale(0.85);
    opacity: 0.9;
  }
  70%,
  100% {
    transform: scale(1.4);
    opacity: 0;
  }
}

.scan-pulse {
  animation: scan-pulse 1.8s ease-out infinite;
}
</style>
