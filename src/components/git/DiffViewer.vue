<script setup lang="ts">
import { computed, ref, shallowRef, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useLocalStorage } from "@vueuse/core";
import {
  ChevronDown,
  ChevronUp,
  Columns2,
  Eraser,
  ExternalLink,
  FoldVertical,
  Highlighter,
  Loader2,
  Rows2,
} from "@lucide/vue";
import SplitDiffView from "@/components/git/SplitDiffView.vue";
import { Badge } from "@/components/ui/badge";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  intralineRanges,
  parseDiff,
  toSideBySideRows,
  type DiffFold,
  type DiffLine,
} from "@/lib/diff";
import { emphasisTextHtml, highlightDiffLines, wordClsOf } from "@/lib/diff-highlight";
import { blockStarts, foldContextLines } from "@/lib/diff-viewer";
import type { GitCommitFileDiff } from "@/types";

/**
 * 单文件 diff 查看器:提交详情面板与提交对话框变更预览共用,保证两处观感/交互一致。
 * 只接收「取数结果」,取数本身由父组件负责(提交 diff 与工作区 diff 命令不同):
 * 切换文件时父组件先换 filePath、保留旧 diff 直至新结果落地,本组件内部解析 + shiki 着色
 * 完成后行模型 / 高亮 / 元数据(landedDiff)同帧替换,避免纯文本先闪现再上色。
 */
const props = defineProps<{
  /** 当前文件的 diff 结果;null = 尚未加载 */
  diff: GitCommitFileDiff | null;
  /** diff 对应的文件路径(标题与着色语言推断);切文件时先于 diff 变化 */
  filePath: string | null;
  loading: boolean;
  error: string;
  /** 并排是否适用:新增/删除文件一侧必然全空,强制逐行视图并隐藏切换按钮 */
  splitApplicable: boolean;
  /** 当前文件是否可在 IDE 打开(已删除文件工作区已不存在,不可打开) */
  canOpenIde: boolean;
}>();

/** 忽略空白差异模式:none 不忽略 / eol 行尾 / change 空白数量变化 / all 全部空白。
 *  由父组件持有(持久化),模式变化需父组件按新模式重取 diff(行集会变) */
const ignoreWs = defineModel<"none" | "eol" | "change" | "all">("ignoreWs", { required: true });

const emit = defineEmits<{
  /** 在 IDE 打开当前文件(路径拼接与编辑器选择由父组件负责) */
  openIde: [];
}>();

const { t } = useI18n();

/** 行内差异高亮(持久化):成对增删行中实际不同的片段加强底色 */
const wordDiff = useLocalStorage("repomeow:commit-diff-word", true);
/** 并排查看(持久化):旧版本在左、新版本在右 */
const splitDiff = useLocalStorage("repomeow:commit-diff-split", false);
/** 并排左右内容窗格的宽度比(持久化,拖拽中间连接条调整;0.5 = 各占一半) */
const splitRatio = useLocalStorage("repomeow:commit-diff-split-ratio", 0.5);

/** diff 行模型与着色结果:由下方 watch 在着色完成后同帧写入(键就是行对象引用,fold/sideRows 复用同一批) */
const diffLines = shallowRef<DiffLine[]>([]);
const lineHtml = shallowRef(new Map<DiffLine, string>());
/** 行内差异区间(wordDiff 开启时按当前 diff 行集计算;shiki 未着色的行回退渲染也用它) */
const wordRanges = shallowRef(new Map<DiffLine, [number, number]>());
/** 与 diffLines 同帧落地的 diff 元数据:truncated 徽标 / hunk 头过滤 / 工具栏显隐以它为准,
 *  不用 props.diff——新 diff 到达与着色完成之间有一个异步间隙,元数据必须跟随行模型 */
const landedDiff = shallowRef<GitCommitFileDiff | null>(null);
/** 着色并发序号:连切文件/连按开关时,旧的着色结果不得覆盖新的 */
let highlightSeq = 0;

/** 已手动展开的折叠区(换文件时重置)。声明必须先于下方 immediate watch(同步执行,后置声明触发 TDZ) */
const expandedFolds = ref(new Set<string>());
const currentScrollTop = ref(0);
/** 并排模式当前滚动位置(sideRow 空间的小数行,与 scrollTopAt/locateRowPos 同坐标系) */
const currentRowPos = ref(0);

// 新 diff 到达:先解析再着色,完成后行模型 / 高亮 / 元数据同帧落地;折叠与滚动位置属旧 diff,一并重置
watch(
  () => props.diff,
  async (result) => {
    const seq = ++highlightSeq;
    if (!result || !props.filePath) {
      diffLines.value = [];
      lineHtml.value = new Map();
      wordRanges.value = new Map();
      landedDiff.value = null;
      expandedFolds.value = new Set();
      currentScrollTop.value = 0;
      currentRowPos.value = 0;
      return;
    }
    const filePath = props.filePath;
    const lines = parseDiff(result.diff);
    const ranges = wordDiff.value ? intralineRanges(lines) : new Map<DiffLine, [number, number]>();
    const htmlMap =
      (await highlightDiffLines(lines, filePath, ranges)) ?? new Map<DiffLine, string>();
    if (seq !== highlightSeq) return;
    wordRanges.value = ranges;
    lineHtml.value = htmlMap;
    diffLines.value = lines;
    landedDiff.value = result;
    expandedFolds.value = new Set();
    currentScrollTop.value = 0;
    currentRowPos.value = 0;
  },
  { immediate: true },
);

/** 仅重算行内区间并重新着色(行内高亮开关用,不重新取 diff) */
async function rehighlight(filePath: string, ranges: Map<DiffLine, [number, number]>) {
  const seq = ++highlightSeq;
  const htmlMap =
    (await highlightDiffLines(diffLines.value, filePath, ranges)) ?? new Map<DiffLine, string>();
  if (seq !== highlightSeq || props.filePath !== filePath) return;
  wordRanges.value = ranges;
  lineHtml.value = htmlMap;
}

watch(wordDiff, () => {
  const path = props.filePath;
  if (!path || !diffLines.value.length) return;
  const ranges = wordDiff.value
    ? intralineRanges(diffLines.value)
    : new Map<DiffLine, [number, number]>();
  void rehighlight(path, ranges);
});

// --- diff 解析:lib/diff.ts 的 parseDiff。
// 后端 context_lines 已拉满,diff 含完整文件内容;过长的未更改区间折叠为可点击展开的占位行(IDEA 风格)。
// hunk 头:非截断 diff 整文件已铺开,逐行/并排视图都不渲染(见 displayLines);截断 diff(>10 万行,
// libgit2 在 @@ 上报畸形头如 2-)仍展示——此时它是定位"被截断的变更段"的唯一线索。

function expandFold(key: string) {
  if (!key) return;
  const next = new Set(expandedFolds.value);
  next.add(key);
  expandedFolds.value = next;
}

/** 模板取 fold 字段的访问器(避免依赖模板内联合类型收窄) */
function foldKeyOf(line: DiffLine | DiffFold) {
  return line.kind === "fold" ? line.key : "";
}
function foldCountOf(line: DiffLine | DiffFold) {
  return line.kind === "fold" ? line.count : 0;
}

const displayLines = computed(() => {
  const lines = diffLines.value;
  // 隐藏 hunk 头(截断 diff 除外:此时它是定位被截断变更段的唯一线索)
  return foldContextLines(
    landedDiff.value?.truncated ? lines : lines.filter((line) => line.kind !== "hunk"),
    expandedFolds.value,
  );
});

/** 模板取行内 HTML:优先 shiki 着色结果;未着色但有行内差异区间的行退化为转义纯文本 + 差异底色 */
function hlOf(line: DiffLine | null | undefined) {
  if (!line) return "";
  const html = lineHtml.value.get(line);
  if (html) return html;
  const range = wordRanges.value.get(line);
  return range ? emphasisTextHtml(line.text.slice(1), range, wordClsOf(line)) : "";
}

const splitActive = computed(() => splitDiff.value && props.splitApplicable);
// hunk 头已在 displayLines 统一过滤(截断 diff 保留,作变更段定位线索),
// sideRows / paneRowOffsets / changeBlocks 等下游全部随之保持一致
const sideRows = computed(() => toSideBySideRows(displayLines.value));

// --- 差异导航:上/下一个差异;逐行模式按 px(行索引 × 行高 + py-1 上内边距),
// 并排模式按 sideRow 空间行位置(经 scrollTopAt 换算到两侧窗格) ---
const unifiedEl = ref<HTMLElement | null>(null);
const splitViewEl = ref<InstanceType<typeof SplitDiffView> | null>(null);

function rowHeightPx() {
  return parseFloat(getComputedStyle(document.documentElement).fontSize) * 1.25 || 20;
}

/** 差异块首行在滚动内容中的行索引(连续增删算一个差异):并排按 sideRows,逐行按 displayLines */
const changeRowIdx = computed(() => {
  if (splitActive.value) {
    return blockStarts(
      sideRows.value.map(
        (row) => row.kind === "line" && (row.left?.kind === "del" || row.right?.kind === "add"),
      ),
    );
  }
  return blockStarts(displayLines.value.map((line) => line.kind === "add" || line.kind === "del"));
});

/** 差异块顶部位置:逐行为 scrollTop(px),并排为 sideRow 空间行位置 */
function changeOffsets() {
  if (splitActive.value) return changeRowIdx.value;
  const h = rowHeightPx();
  return changeRowIdx.value.map((i) => h / 5 + i * h);
}

const hasPrevChange = computed(() =>
  splitActive.value
    ? changeOffsets().some((o) => o < currentRowPos.value - 0.1)
    : changeOffsets().some((o) => o < currentScrollTop.value - 2),
);
const hasNextChange = computed(() =>
  splitActive.value
    ? changeOffsets().some((o) => o > currentRowPos.value + 0.1)
    : changeOffsets().some((o) => o > currentScrollTop.value + 2),
);

function scrollToChange(dir: 1 | -1) {
  const offsets = changeOffsets();
  if (splitActive.value) {
    const cur = currentRowPos.value;
    const target =
      dir === 1
        ? offsets.find((o) => o > cur + 0.1)
        : [...offsets].reverse().find((o) => o < cur - 0.1);
    if (target == null) return;
    currentRowPos.value = target;
    splitViewEl.value?.scrollToRow(target);
    return;
  }
  const cur = currentScrollTop.value;
  const target =
    dir === 1 ? offsets.find((o) => o > cur + 2) : [...offsets].reverse().find((o) => o < cur - 2);
  if (target == null) return;
  currentScrollTop.value = target;
  if (unifiedEl.value) unifiedEl.value.scrollTop = target;
}

function onUnifiedScroll() {
  currentScrollTop.value = unifiedEl.value?.scrollTop ?? 0;
}

/** 收起全部已展开的未更改片段 */
const hasExpandedFolds = computed(() => expandedFolds.value.size > 0);
function collapseAllFolds() {
  expandedFolds.value = new Set();
}
</script>

<template>
  <!-- commit-diff 类供 shiki 双主题变量按 .dark 切换 -->
  <div class="commit-diff flex min-h-0 min-w-0 flex-1 flex-col">
    <div class="flex shrink-0 items-center gap-2 border-b px-3 py-1.5">
      <span class="min-w-0 flex-1 truncate font-mono text-xs" :title="filePath ?? undefined">
        {{ filePath ?? "" }}
      </span>
      <Badge v-if="landedDiff?.truncated" variant="outline" class="h-5 shrink-0 px-1.5 text-[10px]">
        {{ t("git.graph.detail.diffTruncated") }}
      </Badge>
      <template v-if="landedDiff">
        <button
          class="shrink-0 rounded-sm p-1 text-muted-foreground transition-colors not-disabled:hover:bg-accent not-disabled:hover:text-foreground disabled:opacity-40"
          :disabled="!hasPrevChange"
          :title="t('git.graph.detail.diffPrevChange')"
          @click="scrollToChange(-1)"
        >
          <ChevronUp class="h-3.5 w-3.5" />
        </button>
        <button
          class="shrink-0 rounded-sm p-1 text-muted-foreground transition-colors not-disabled:hover:bg-accent not-disabled:hover:text-foreground disabled:opacity-40"
          :disabled="!hasNextChange"
          :title="t('git.graph.detail.diffNextChange')"
          @click="scrollToChange(1)"
        >
          <ChevronDown class="h-3.5 w-3.5" />
        </button>
        <button
          class="shrink-0 rounded-sm p-1 text-muted-foreground transition-colors not-disabled:hover:bg-accent not-disabled:hover:text-foreground disabled:opacity-40"
          :disabled="!hasExpandedFolds"
          :title="t('git.graph.detail.diffCollapseFolds')"
          @click="collapseAllFolds"
        >
          <FoldVertical class="h-3.5 w-3.5" />
        </button>
      </template>
      <DropdownMenu v-if="landedDiff">
        <DropdownMenuTrigger as-child>
          <button
            class="shrink-0 rounded-sm p-1 transition-colors hover:bg-accent hover:text-foreground"
            :class="ignoreWs !== 'none' ? 'bg-accent text-foreground' : 'text-muted-foreground'"
            :title="t('git.graph.detail.diffIgnoreWs')"
          >
            <Eraser class="h-3.5 w-3.5" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" class="w-auto whitespace-nowrap">
          <DropdownMenuRadioGroup v-model="ignoreWs">
            <DropdownMenuRadioItem value="none">
              {{ t("git.graph.detail.diffIgnoreWsNone") }}
            </DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="eol">
              {{ t("git.graph.detail.diffIgnoreWsEol") }}
            </DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="change">
              {{ t("git.graph.detail.diffIgnoreWsChange") }}
            </DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="all">
              {{ t("git.graph.detail.diffIgnoreWsAll") }}
            </DropdownMenuRadioItem>
          </DropdownMenuRadioGroup>
        </DropdownMenuContent>
      </DropdownMenu>
      <button
        v-if="landedDiff"
        class="shrink-0 rounded-sm p-1 transition-colors hover:bg-accent hover:text-foreground"
        :class="wordDiff ? 'bg-accent text-foreground' : 'text-muted-foreground'"
        :title="t('git.graph.detail.diffWordHl')"
        @click="wordDiff = !wordDiff"
      >
        <Highlighter class="h-3.5 w-3.5" />
      </button>
      <button
        v-if="landedDiff && splitApplicable"
        class="shrink-0 rounded-sm p-1 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
        :title="t(splitActive ? 'git.graph.detail.diffUnified' : 'git.graph.detail.diffSplit')"
        @click="splitDiff = !splitDiff"
      >
        <Rows2 v-if="splitDiff" class="h-3.5 w-3.5" />
        <Columns2 v-else class="h-3.5 w-3.5" />
      </button>
      <button
        v-if="canOpenIde"
        class="shrink-0 rounded-sm p-1 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
        :title="t('git.graph.detail.openInIde')"
        @click="emit('openIde')"
      >
        <ExternalLink class="h-3.5 w-3.5" />
      </button>
    </div>

    <SplitDiffView
      v-if="splitActive && !loading && !error && filePath"
      ref="splitViewEl"
      v-model:split-ratio="splitRatio"
      v-model:current-row-pos="currentRowPos"
      :rows="sideRows"
      :landed-diff="landedDiff"
      :line-html="lineHtml"
      :word-ranges="wordRanges"
      @expand-fold="expandFold"
    />

    <div ref="unifiedEl" v-else class="min-h-0 flex-1 overflow-auto" @scroll="onUnifiedScroll">
      <div v-if="loading" class="flex h-full items-center justify-center">
        <Loader2 class="h-4 w-4 animate-spin text-muted-foreground" />
      </div>
      <p v-else-if="error" class="px-3 py-2 text-xs text-destructive">
        {{ t("git.graph.detail.diffLoadFailed") }}:{{ error }}
      </p>
      <p
        v-else-if="!filePath"
        class="flex h-full items-center justify-center text-xs text-muted-foreground"
      >
        {{ t("git.graph.detail.selectFile") }}
      </p>

      <!-- 逐行(unified)视图 -->
      <div v-else class="diff-code min-w-max py-1 text-xs leading-5">
        <template v-for="(line, i) in displayLines" :key="i">
          <div
            v-if="line.kind === 'hunk'"
            class="bg-muted/60 px-3 text-muted-foreground select-none"
          >
            {{ line.text }}
          </div>
          <div v-else-if="line.kind === 'meta'" class="px-3 text-muted-foreground select-none">
            {{ line.text }}
          </div>
          <button
            v-else-if="line.kind === 'fold'"
            class="block w-full bg-muted/40 select-none hover:bg-accent"
            :title="t('git.graph.detail.diffExpand', { count: foldCountOf(line) })"
            @click="expandFold(foldKeyOf(line))"
          >
            <div class="diff-fold-wave mx-3 h-5" />
          </button>
          <div
            v-else
            class="flex w-full"
            :class="
              line.kind === 'add' ? 'bg-green-500/10' : line.kind === 'del' ? 'bg-red-500/10' : ''
            "
          >
            <span class="w-10 shrink-0 pr-2 text-right text-muted-foreground/50 select-none">
              {{ line.oldLine ?? "" }}
            </span>
            <span class="w-10 shrink-0 pr-2 text-right text-muted-foreground/50 select-none">
              {{ line.newLine ?? "" }}
            </span>
            <span class="whitespace-pre">
              <span v-if="hlOf(line)" class="diff-hl" v-html="hlOf(line)" />
              <template v-else>{{ line.text.slice(1) }}</template>
            </span>
          </div>
        </template>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* diff 代码字体栈:Tailwind font-mono 以泛型 monospace 收尾,WebView2(zh-CN)按「等宽语境」
   把栈内没有的 CJK 字符回退到系统中文等宽默认 NSimSun(宋体系),与 Consolas 的 ASCII 观感割裂;
   在泛型前显式插入微软雅黑,汉字统一走雅黑。ASCII 仍按原栈取 Consolas(macOS 则前置条目取 SF Mono/Menlo) */
.diff-code {
  font-family:
    ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New",
    "Microsoft YaHei", monospace;
}

/* shiki 双主题产物只在 token span 上留 --shiki-light/--shiki-dark 变量,这里按 .dark 切换实际用哪组;
   token 经 v-html 注入没有 scoped 属性,选择器整段包 :global()(同 CommandEditor 的写法) */
:global(.commit-diff .diff-hl span) {
  color: var(--shiki-light);
}

:global(html.dark .commit-diff .diff-hl span) {
  color: var(--shiki-dark);
}

/* 折叠占位行的波浪线:SVG data-uri 平铺,background 文档隔离无法用 currentColor,取中性灰适配亮暗主题 */
.diff-fold-wave {
  background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='6'%3E%3Cpath d='M0 3 Q3 0.5 6 3 T12 3' fill='none' stroke='%239ca3af' stroke-width='1.2'/%3E%3C/svg%3E");
  background-repeat: repeat-x;
  background-position: center;
}

/* 行内差异底色:成对增删行中实际不同的片段;套在 shiki token span 外层,只加背景不改文字色。
   token 经 v-html 注入没有 scoped 属性,选择器整段包 :global()(同 .diff-hl 的写法) */
:global(.commit-diff .diff-word-del) {
  background-color: rgb(239 68 68 / 0.28);
}

:global(.commit-diff .diff-word-add) {
  background-color: rgb(34 197 94 / 0.28);
}
</style>
