<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import { Compartment, EditorState, StateEffect, StateField } from "@codemirror/state";
import {
  EditorView,
  Decoration,
  drawSelection,
  lineNumbers,
  type DecorationSet,
} from "@codemirror/view";
import { codeFolding, foldGutter, syntaxHighlighting } from "@codemirror/language";
import { cmViewerHighlight, cmViewerTheme } from "@/lib/cm-theme";
import { resolveCmLanguage } from "@/lib/cm-languages";
import { buildFindRegExp, collectMatches, type FindQuery, type TextRange } from "@/lib/text-search";

// ── 任务描述 ─────────────────────────────────────────────────────────────────
// CodeMirror 6 只读代码查看器(文件预览专用):
// - 行号 + 代码折叠 + 语法高亮;换行经 wrap prop 由 Compartment 热切换;
// - 语言按 path 惰性解析,期间先以纯文本呈现、解析完成后 reconfigure;
// - 只读双保险(readOnly + editable:false),但保留鼠标选区——后续「选取代码
//   做批注」经 getView() 拿 EditorView,读 state.selection 或注 Decoration;
// - 文件内查找:runFind/gotoMatch/clearFind 经 expose 驱动,匹配经 StateField
//   挂 Decoration.mark 高亮(当前项强调),文档替换时随 tr.map 保持定位。
// 滚动交给 CM 自带 scroller(行号槽固定、双向滚动),宿主须提供确定高度。

const props = defineProps<{ text: string; path: string; wrap: boolean }>();

const host = ref<HTMLElement | null>(null);
let view: EditorView | null = null;
let langSeq = 0;

const languageConf = new Compartment();
const wrapConf = new Compartment();

async function applyLanguage() {
  const mySeq = ++langSeq;
  const lang = await resolveCmLanguage(props.path);
  // 卸载或期间已切换到别的文件:丢弃过期结果
  if (!view || mySeq !== langSeq) return;
  view.dispatch({ effects: languageConf.reconfigure(lang ?? []) });
}

// ── 文件内查找 ────────────────────────────────────────────────────────────────
const setFindRanges = StateEffect.define<{ ranges: TextRange[]; current: number }>();

const findMarks = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(value, tr) {
    let next = value.map(tr.changes);
    for (const e of tr.effects) {
      if (!e.is(setFindRanges)) continue;
      const { ranges, current } = e.value;
      next = Decoration.set(
        ranges.map((r, i) =>
          (i === current
            ? Decoration.mark({ class: "cm-find-match cm-find-match-current" })
            : Decoration.mark({ class: "cm-find-match" })
          ).range(r.from, r.to),
        ),
        true,
      );
    }
    return next;
  },
  provide: (field) => EditorView.decorations.from(field),
});

let findRanges: TextRange[] = [];
let findCursor = -1;

/** 在当前文档上执行查找并刷新高亮;返回带行号的全部匹配(空/非法查询为空数组) */
function runFind(query: FindQuery): TextRange[] {
  findRanges = [];
  findCursor = -1;
  if (!view) return [];
  const re = buildFindRegExp(query);
  if (re) {
    findRanges = collectMatches(view.state.doc.toString(), re);
    // 从光标处就近选定起始匹配,没有更靠后的则回到首个
    findCursor = findRanges.findIndex((r) => r.from >= view!.state.selection.main.head);
    if (findCursor === -1 && findRanges.length) findCursor = 0;
  }
  view.dispatch({ effects: setFindRanges.of({ ranges: findRanges, current: findCursor }) });
  return findRanges;
}

/** 当前匹配索引(runFind 后读取,-1 表示无) */
function getFindCursor(): number {
  return findCursor;
}

/** 清除查找高亮与状态 */
function clearFind() {
  findRanges = [];
  findCursor = -1;
  view?.dispatch({ effects: setFindRanges.of({ ranges: [], current: -1 }) });
}

/** 跳到第 index 个匹配(循环取模、选中并居中滚动),返回实际索引;无匹配返回 -1 */
function gotoMatch(index: number): number {
  if (!view || !findRanges.length) return -1;
  const i = ((index % findRanges.length) + findRanges.length) % findRanges.length;
  const r = findRanges[i];
  findCursor = i;
  view.dispatch({
    selection: { anchor: r.from, head: r.to },
    effects: [
      EditorView.scrollIntoView(r.from, { y: "center" }),
      setFindRanges.of({ ranges: findRanges, current: i }),
    ],
  });
  return i;
}

/** 定位到 1-based 行号(全文搜索跳转用) */
function revealLine(line: number) {
  if (!view) return;
  const no = Math.min(Math.max(1, line), view.state.doc.lines);
  const l = view.state.doc.line(no);
  view.dispatch({
    selection: { anchor: l.from },
    effects: EditorView.scrollIntoView(l.from, { y: "center" }),
  });
}

onMounted(() => {
  view = new EditorView({
    parent: host.value ?? undefined,
    state: EditorState.create({
      doc: props.text,
      extensions: [
        cmViewerTheme,
        syntaxHighlighting(cmViewerHighlight),
        lineNumbers(),
        // foldGutter 只渲染折叠槽标记,实际折叠状态/效果由 codeFolding 注册;
        // 默认 openText "⌄" 的墨迹悬在基线下方,视觉上比行中心低约 5px,换
        // 实心小三角(墨迹中心与字体盒中心对齐,形似 VS Code 折叠箭头)
        foldGutter({ openText: "▾", closedText: "▸" }),
        codeFolding(),
        drawSelection(),
        EditorState.readOnly.of(true),
        EditorView.editable.of(false),
        languageConf.of([]),
        wrapConf.of(props.wrap ? EditorView.lineWrapping : []),
        findMarks,
      ],
    }),
  });
  void applyLanguage();
});

watch(
  () => props.text,
  (text) => {
    if (!view) return;
    view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: text } });
  },
);

watch(
  () => props.path,
  () => {
    void applyLanguage();
  },
);

watch(
  () => props.wrap,
  (wrap) => {
    view?.dispatch({ effects: wrapConf.reconfigure(wrap ? EditorView.lineWrapping : []) });
  },
);

onBeforeUnmount(() => {
  view?.destroy();
  view = null;
});

/** 供批注/查找等扩展层访问编辑器实例(挂载后非空) */
function getView() {
  return view;
}

defineExpose({ getView, runFind, clearFind, gotoMatch, getFindCursor, revealLine });
</script>

<template>
  <div ref="host" class="min-h-0 min-w-0 flex-1 overflow-hidden" />
</template>
