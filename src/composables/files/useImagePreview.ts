import { computed, ref, type Ref } from "vue";
import { convertFileSrc } from "@tauri-apps/api/core";
import { extOf, IMAGE_EXTS } from "@/lib/file-kind";

/** 图片预览公共逻辑(ProjectFiles 与 ResourceSkillPreview 共用):
 *  图片不读文本内容,走 asset 协议(convertFileSrc)直显;
 *  svg 兼具图像与文本两种形态,支持「预览/源码」切换,源码模式下仍按文本读取。
 *  toAbsolutePath 把选中文件(仓库/技能内相对路径)解析为磁盘绝对路径,返回 null 表示不可预览。 */
export function useImagePreview(
  selected: Ref<string | null>,
  toAbsolutePath: (path: string) => string | null,
) {
  const selectedExt = computed(() => (selected.value ? extOf(selected.value) : ""));
  const isImage = computed(() => IMAGE_EXTS.has(selectedExt.value));
  const isSvg = computed(() => selectedExt.value === "svg");

  const svgMode = ref<"preview" | "source">("preview");
  const svgSource = computed(() => isSvg.value && svgMode.value === "source");

  /** 图片展示地址(asset 协议 URL);非图片或绝对路径不可解析时为空串 */
  const imageSrc = computed(() => {
    const path = selected.value;
    if (!path || !isImage.value) return "";
    const abs = toAbsolutePath(path);
    return abs ? convertFileSrc(abs) : "";
  });

  /** 选中文件时调用:沿用既有习惯,svg 选中后默认落源码模式 */
  function onSelectImage(path: string) {
    if (extOf(path) === "svg") svgMode.value = "source";
  }

  return { selectedExt, isImage, isSvg, svgMode, svgSource, imageSrc, onSelectImage };
}
