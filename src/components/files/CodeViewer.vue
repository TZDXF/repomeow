<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import { Compartment, EditorState } from "@codemirror/state";
import { EditorView, drawSelection, lineNumbers } from "@codemirror/view";
import { codeFolding, foldGutter, syntaxHighlighting } from "@codemirror/language";
import { cmViewerHighlight, cmViewerTheme } from "@/lib/cm-theme";
import { resolveCmLanguage } from "@/lib/cm-languages";

// ── 任务描述 ─────────────────────────────────────────────────────────────────
// CodeMirror 6 只读代码查看器(文件预览专用):
// - 行号 + 代码折叠 + 语法高亮;换行经 wrap prop 由 Compartment 热切换;
// - 语言按 path 惰性解析,期间先以纯文本呈现、解析完成后 reconfigure;
// - 只读双保险(readOnly + editable:false),但保留鼠标选区——后续「选取代码
//   做批注/查找」经 getView() 拿 EditorView,读 state.selection 或注 Decoration。
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

defineExpose({ getView });
</script>

<template>
  <div ref="host" class="min-h-0 min-w-0 flex-1 overflow-hidden" />
</template>
