import { HighlightStyle } from "@codemirror/language";
import { tags as t } from "@lezer/highlight";
import { EditorView } from "@codemirror/view";

// ── 任务描述 ─────────────────────────────────────────────────────────────────
// CodeMirror 只读查看器的外观:结构色(背景/gutter/选区)走 shadcn CSS 变量,
// token 色走 --cm-* 变量(定义在 src/style.css 的 :root/.dark,色值取自原
// Shiki github-light/github-dark)——两者都随根节点 .dark 切换自动生效,
// 无需 JS 侧重建主题。字号/行高对齐原视图(13px/1.55)。

export const cmViewerTheme = EditorView.theme({
  "&": {
    height: "100%",
    backgroundColor: "transparent",
    color: "var(--color-foreground)",
  },
  "&.cm-focused": { outline: "none" },
  ".cm-scroller": {
    fontFamily:
      'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace',
    fontSize: "13px",
    lineHeight: "1.55",
  },
  ".cm-content": {
    padding: "16px 8px 48px 8px",
  },
  ".cm-gutters": {
    backgroundColor: "transparent",
    color: "var(--color-muted-foreground)",
    border: "none",
    borderRight: "1px solid var(--color-border)",
  },
  ".cm-lineNumbers .cm-gutterElement": {
    minWidth: "2.75rem",
    padding: "0 12px 0 12px",
    opacity: "0.8",
  },
  ".cm-foldGutter .cm-gutterElement": {
    color: "var(--color-muted-foreground)",
    cursor: "pointer",
  },
  ".cm-foldPlaceholder": {
    backgroundColor: "var(--color-muted)",
    border: "none",
    color: "var(--color-muted-foreground)",
    borderRadius: "3px",
    margin: "0 4px",
    padding: "0 6px",
  },
  ".cm-selectionBackground, &.cm-focused .cm-selectionBackground": {
    backgroundColor: "var(--color-accent)",
  },
  // 文件内查找高亮:普通匹配黄底,当前项橙底描边(VS Code 观感)
  ".cm-find-match": {
    backgroundColor: "rgba(234, 179, 8, 0.35)",
    borderRadius: "2px",
  },
  ".cm-find-match.cm-find-match-current": {
    backgroundColor: "rgba(249, 115, 22, 0.5)",
    outline: "1px solid rgba(249, 115, 22, 0.9)",
  },
});

export const cmViewerHighlight = HighlightStyle.define([
  { tag: [t.keyword, t.controlKeyword, t.moduleKeyword, t.operatorKeyword], color: "var(--cm-keyword)" },
  { tag: [t.string, t.special(t.string), t.regexp, t.escape], color: "var(--cm-string)" },
  {
    tag: [t.lineComment, t.blockComment, t.docComment, t.meta],
    color: "var(--cm-comment)",
    fontStyle: "italic",
  },
  { tag: [t.number, t.bool, t.null, t.atom], color: "var(--cm-number)" },
  {
    tag: [t.function(t.variableName), t.function(t.propertyName), t.macroName, t.labelName],
    color: "var(--cm-function)",
  },
  {
    tag: [t.typeName, t.className, t.namespace, t.definition(t.typeName)],
    color: "var(--cm-type)",
  },
  { tag: t.tagName, color: "var(--cm-tag)" },
  { tag: t.attributeName, color: "var(--cm-attribute)" },
  { tag: t.link, color: "var(--cm-string)", textDecoration: "underline" },
  { tag: t.heading, color: "var(--cm-function)", fontWeight: "bold" },
  { tag: t.emphasis, fontStyle: "italic" },
  { tag: t.strong, fontWeight: "bold" },
  { tag: t.strikethrough, textDecoration: "line-through" },
  // 兜底放最后:变量/属性用近前景色,运算符/括号等留给默认前景
  { tag: [t.variableName, t.propertyName], color: "var(--cm-variable)" },
]);
