import { code } from "@stream-markdown/code";
import type { PreviewerConfig } from "vue-stream-markdown";
import { math } from "@stream-markdown/math";
import { mermaid } from "@stream-markdown/mermaid";
import "katex/dist/katex.min.css";

/**
 * vue-stream-markdown 2.x 的代码高亮 / mermaid / 数学公式渲染全部改为显式
 * 注册的扩展,不再随包内置。这里创建全局共享的单一实例组(扩展内部持有
 * Shiki 高亮器等单例资源,重复创建会重复 preload),由 SafeMarkdown 默认注入。
 */
export const markdownExtensions = {
  code: code(),
  mermaid: mermaid(),
  math: math(),
};

/**
 * 合并调用方自定义 previewers 并强制 html 预览空沙箱:库默认 sandbox 是
 * allow-scripts(脚本会执行),预览内容来自不可信 Markdown,html.sandbox 由
 * 应用收口、不接受放宽。`false`(显式关闭预览)原样保留;对象形态下
 * mermaid 等内置预览不受影响(对象形态依旧启用内置预览)。
 */
export function mergeSafePreviewers(custom?: PreviewerConfig): PreviewerConfig {
  if (custom === false) return false;
  const obj = typeof custom === "object" ? custom : undefined;
  return {
    ...obj,
    html: { ...obj?.html, sandbox: "" },
  };
}
