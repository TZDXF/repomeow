<script setup lang="ts">
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import { useSplitDiffLayout } from "@/composables/git/useSplitDiffLayout";
import type { DiffLine, DiffSideRow } from "@/lib/diff";
import { emphasisTextHtml, wordClsOf } from "@/lib/diff-highlight";
import type { GitCommitFileDiff } from "@/types";

const props = defineProps<{
  rows: DiffSideRow[];
  landedDiff: GitCommitFileDiff | null;
  lineHtml: Map<DiffLine, string>;
  wordRanges: Map<DiffLine, [number, number]>;
  /** 短暂高亮的新文件行号(语义变更定位;仅右侧窗格,1.6s 后由父组件清除) */
  revealLine?: number | null;
}>();

const emit = defineEmits<{
  expandFold: [key: string];
}>();

const splitRatio = defineModel<number>("splitRatio", { required: true });
const currentRowPos = defineModel<number>("currentRowPos", { required: true });
const { t } = useI18n();

const sideRows = computed(() => props.rows);
const splitActive = computed(() => true);
const landedDiff = computed(() => props.landedDiff);

const splitWrapEl = ref<HTMLElement | null>(null);
const leftPaneEl = ref<HTMLElement | null>(null);
const rightPaneEl = ref<HTMLElement | null>(null);
const leftGutterEl = ref<HTMLElement | null>(null);
const rightGutterEl = ref<HTMLElement | null>(null);
const dividerEl = ref<HTMLElement | null>(null);

const {
  leftRows,
  rightRows,
  leftMarkers,
  rightMarkers,
  dividerShapes,
  hbarPad,
  scrollTopAt,
  syncPaneScroll,
  startSplitResize,
} = useSplitDiffLayout({
  sideRows,
  layoutRows: sideRows,
  splitActive,
  landedDiff,
  splitRatio,
  currentRowPos,
  elements: {
    splitWrapEl,
    leftPaneEl,
    rightPaneEl,
    leftGutterEl,
    rightGutterEl,
    dividerEl,
  },
});

function sideText(line: DiffLine | null) {
  return line ? line.text.slice(1) : "";
}

function hlOf(line: DiffLine | null | undefined) {
  if (!line) {
    return "";
  }
  const html = props.lineHtml.get(line);
  if (html) {
    return html;
  }
  const range = props.wordRanges.get(line);
  return range ? emphasisTextHtml(line.text.slice(1), range, wordClsOf(line)) : "";
}

function scrollToRow(rowPosition: number) {
  const left = scrollTopAt("left", rowPosition);
  const right = scrollTopAt("right", rowPosition);
  if (leftPaneEl.value) {
    leftPaneEl.value.scrollTop = left;
  }
  if (leftGutterEl.value) {
    leftGutterEl.value.scrollTop = left;
  }
  if (rightPaneEl.value) {
    rightPaneEl.value.scrollTop = right;
  }
  if (rightGutterEl.value) {
    rightGutterEl.value.scrollTop = right;
  }
}

defineExpose({ scrollToRow });
</script>

<template>
  <!-- 左内容 | 左行号 | 连接条 | 右行号 | 右内容 -->
  <div ref="splitWrapEl" class="flex min-h-0 flex-1">
    <!-- direction:rtl 只用于把旧版本窗格的纵向滚动条移到左边，内层恢复 ltr -->
    <div
      ref="leftPaneEl"
      class="min-w-0 overflow-auto [direction:rtl]"
      :style="{ flex: `${splitRatio} 1 0%` }"
      @scroll="syncPaneScroll('left')"
    >
      <div class="diff-code relative min-w-max py-1 text-xs leading-5 [direction:ltr]">
        <template v-for="(row, i) in leftRows" :key="i">
          <div
            v-if="row.kind === 'hunk'"
            class="bg-muted/60 px-3 whitespace-pre text-muted-foreground select-none"
          >
            {{ row.text }}
          </div>
          <div
            v-else-if="row.kind === 'meta'"
            class="px-3 whitespace-pre text-muted-foreground select-none"
          >
            {{ row.text }}
          </div>
          <button
            v-else-if="row.kind === 'fold'"
            class="block w-full bg-muted/40 select-none hover:bg-accent"
            :title="t('git.graph.detail.diffExpand', { count: row.count })"
            @click="emit('expandFold', row.foldKey)"
          >
            <div class="diff-fold-wave h-5" />
          </button>
          <div v-else class="h-5 pl-2" :class="row.line?.kind === 'del' ? 'bg-red-500/10' : ''">
            <span v-if="hlOf(row.line)" class="diff-hl whitespace-pre" v-html="hlOf(row.line)" />
            <span v-else class="whitespace-pre">{{ sideText(row.line) }}</span>
          </div>
        </template>
        <div
          v-for="(marker, i) in leftMarkers"
          :key="i"
          class="pointer-events-none absolute inset-x-0 h-0.5 -translate-y-1/2"
          :class="marker.cls"
          :style="{ top: `calc(0.25rem + ${marker.top * 1.25}rem)` }"
        />
      </div>
    </div>

    <div
      ref="leftGutterEl"
      class="shrink-0 overflow-auto border-l border-border/60 font-mono text-xs leading-5 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
      @scroll="syncPaneScroll('leftGutter')"
    >
      <div
        class="relative py-1"
        :style="{ paddingBottom: `calc(0.25rem + ${hbarPad.leftGutter}px)` }"
      >
        <template v-for="(row, i) in leftRows" :key="i">
          <div v-if="row.kind === 'hunk'" class="h-5 bg-muted/60 select-none" />
          <div v-else-if="row.kind === 'meta'" class="h-5 select-none" />
          <button
            v-else-if="row.kind === 'fold'"
            class="block h-5 w-full bg-muted/40 select-none hover:bg-accent"
            :title="t('git.graph.detail.diffExpand', { count: row.count })"
            @click="emit('expandFold', row.foldKey)"
          >
            <div class="diff-fold-wave h-full" />
          </button>
          <div
            v-else
            class="h-5 w-10 text-center text-muted-foreground/50 select-none"
            :class="row.line?.kind === 'del' ? 'bg-red-500/10' : ''"
          >
            {{ row.line?.oldLine ?? "" }}
          </div>
        </template>
        <div
          v-for="(marker, i) in leftMarkers"
          :key="i"
          class="pointer-events-none absolute inset-x-0 h-0.5 -translate-y-1/2"
          :class="marker.cls"
          :style="{ top: `calc(0.25rem + ${marker.top * 1.25}rem)` }"
        />
      </div>
    </div>

    <div
      ref="dividerEl"
      class="relative w-5 shrink-0 cursor-col-resize select-none transition-colors hover:bg-primary/40"
      @pointerdown="startSplitResize"
    >
      <svg class="pointer-events-none absolute inset-0 h-full w-full">
        <template v-for="(shape, i) in dividerShapes" :key="i">
          <path v-if="shape.kind === 'poly'" :d="shape.d" :class="shape.cls" />
          <line v-else x1="0" :y1="shape.y1" x2="100%" :y2="shape.y2" :class="shape.cls" />
        </template>
      </svg>
    </div>

    <div
      ref="rightGutterEl"
      class="shrink-0 overflow-auto border-r border-border/60 font-mono text-xs leading-5 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
      @scroll="syncPaneScroll('rightGutter')"
    >
      <div
        class="relative py-1"
        :style="{ paddingBottom: `calc(0.25rem + ${hbarPad.rightGutter}px)` }"
      >
        <template v-for="(row, i) in rightRows" :key="i">
          <div v-if="row.kind === 'hunk'" class="h-5 bg-muted/60 select-none" />
          <div v-else-if="row.kind === 'meta'" class="h-5 select-none" />
          <button
            v-else-if="row.kind === 'fold'"
            class="block h-5 w-full bg-muted/40 select-none hover:bg-accent"
            :title="t('git.graph.detail.diffExpand', { count: row.count })"
            @click="emit('expandFold', row.foldKey)"
          >
            <div class="diff-fold-wave h-full" />
          </button>
          <div
            v-else
            class="h-5 w-10 text-center text-muted-foreground/50 select-none"
            :class="row.line?.kind === 'add' ? 'bg-green-500/10' : ''"
          >
            {{ row.line?.newLine ?? "" }}
          </div>
        </template>
        <div
          v-for="(marker, i) in rightMarkers"
          :key="i"
          class="pointer-events-none absolute inset-x-0 h-0.5 -translate-y-1/2"
          :class="marker.cls"
          :style="{ top: `calc(0.25rem + ${marker.top * 1.25}rem)` }"
        />
      </div>
    </div>

    <div
      ref="rightPaneEl"
      class="min-w-0 overflow-auto"
      :style="{ flex: `${1 - splitRatio} 1 0%` }"
      @scroll="syncPaneScroll('right')"
    >
      <div class="diff-code relative min-w-max py-1 text-xs leading-5">
        <template v-for="(row, i) in rightRows" :key="i">
          <div
            v-if="row.kind === 'hunk'"
            class="bg-muted/60 px-3 whitespace-pre text-muted-foreground select-none"
          >
            {{ row.text }}
          </div>
          <div
            v-else-if="row.kind === 'meta'"
            class="px-3 whitespace-pre text-muted-foreground select-none"
          >
            {{ row.text }}
          </div>
          <button
            v-else-if="row.kind === 'fold'"
            class="block w-full bg-muted/40 select-none hover:bg-accent"
            :title="t('git.graph.detail.diffExpand', { count: row.count })"
            @click="emit('expandFold', row.foldKey)"
          >
            <div class="diff-fold-wave h-5" />
          </button>
          <div
            v-else
            class="h-5 pl-2 transition-colors"
            :class="[
              row.line?.kind === 'add' ? 'bg-green-500/10' : '',
              revealLine != null && row.line?.newLine === revealLine ? 'bg-primary/20' : '',
            ]"
          >
            <span v-if="hlOf(row.line)" class="diff-hl whitespace-pre" v-html="hlOf(row.line)" />
            <span v-else class="whitespace-pre">{{ sideText(row.line) }}</span>
          </div>
        </template>
        <div
          v-for="(marker, i) in rightMarkers"
          :key="i"
          class="pointer-events-none absolute inset-x-0 h-0.5 -translate-y-1/2"
          :class="marker.cls"
          :style="{ top: `calc(0.25rem + ${marker.top * 1.25}rem)` }"
        />
      </div>
    </div>
  </div>
</template>

<style scoped>
.diff-code {
  font-family:
    ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New",
    "Microsoft YaHei", monospace;
}

.diff-fold-wave {
  background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='6'%3E%3Cpath d='M0 3 Q3 0.5 6 3 T12 3' fill='none' stroke='%239ca3af' stroke-width='1.2'/%3E%3C/svg%3E");
  background-repeat: repeat-x;
  background-position: center;
}

.divider-del,
.divider-add,
.divider-mod {
  stroke-linejoin: round;
}

.divider-del {
  fill: rgb(239 68 68 / 0.16);
  stroke: rgb(239 68 68 / 0.45);
  stroke-width: 1;
}

.divider-add {
  fill: rgb(34 197 94 / 0.16);
  stroke: rgb(34 197 94 / 0.45);
  stroke-width: 1;
}

.divider-mod {
  fill: rgb(59 130 246 / 0.16);
  stroke: rgb(59 130 246 / 0.45);
  stroke-width: 1;
}

.divider-fold {
  stroke: #9ca3af;
  stroke-width: 1.2;
}

.insert-add {
  background-color: rgb(34 197 94 / 0.55);
}

.insert-del {
  background-color: rgb(239 68 68 / 0.55);
}
</style>
