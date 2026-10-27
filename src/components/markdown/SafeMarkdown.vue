<script setup lang="ts">
import { computed } from "vue";
import { Markdown } from "vue-stream-markdown";
import {
  type ControlsConfig,
  type DownloadEvent,
  type ExtensionOverrides,
  type LocaleConfig,
  type MarkdownComponents,
  type PreviewerConfig,
} from "vue-stream-markdown";
import { markdownExtensions, mergeSafePreviewers } from "@/lib/markdown-extensions";
import MdImage from "./MdImage.vue";
import MdLink from "./MdLink.vue";
import { interceptMarkdownLink } from "./markdown-links";

/**
 * vue-stream-markdown 的统一安全封装:全部渲染点必须经此组件,禁止直接使用
 * 库的 <Markdown>,防止新增渲染点遗漏安全配置(历史漏洞:XSS 评估发现 8 处
 * <Markdown> 均未传 previewers,html 围栏代码块被写进 allow-scripts 的
 * iframe 自动执行)。
 *
 * - previewers:html 围栏代码块的预览 iframe 一律空沙箱(脚本不执行、表单/
 *   弹窗/同源全禁),保留预览外观;调用方仍可传 previewers 覆盖其他选项,
 *   但 html.sandbox 合并后仍以空沙箱兜底。
 * - components:强制接管 img 和 a,调用方可追加其他标签;所有链接点击在此拦截。
 * - extensions:全局共享的 code/mermaid/math 扩展实例(2.x 起需显式注册)。
 */
interface Props {
  content?: string;
  mode?: "static" | "streaming";
  controls?: ControlsConfig;
  themeElement?: () => HTMLElement | undefined;
  locale?: string | LocaleConfig;
  beforeDownload?: (event: DownloadEvent) => boolean | Promise<boolean>;
  /** 调用方自定义的预览器配置,与空沙箱默认值浅合并 */
  previewers?: PreviewerConfig;
  /** 调用方追加的自定义标签渲染器(在默认 img/a 接管之后合并) */
  components?: MarkdownComponents;
  /** 默认为全局共享实例;传 false 可禁用某个内置扩展 */
  extensions?: ExtensionOverrides;
}

const props = defineProps<Props>();

const mergedPreviewers = computed<PreviewerConfig>(() => mergeSafePreviewers(props.previewers));

const mergedComponents = computed<MarkdownComponents>(() => ({
  ...props.components,
  img: MdImage,
  a: MdLink,
}));

const mergedExtensions = computed<ExtensionOverrides>(() => props.extensions ?? markdownExtensions);
</script>

<template>
  <div
    style="display: contents"
    @click.capture="interceptMarkdownLink"
    @auxclick.capture="interceptMarkdownLink"
    @contextmenu.capture="interceptMarkdownLink"
  >
    <Markdown
      :content="content"
      :mode="mode"
      :controls="controls"
      :theme-element="themeElement"
      :locale="locale"
      :before-download="beforeDownload"
      :previewers="mergedPreviewers"
      :components="mergedComponents"
      :extensions="mergedExtensions"
    />
  </div>
</template>
