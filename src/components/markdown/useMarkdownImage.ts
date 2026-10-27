import { ref, watch, type Ref } from "vue";
import { convertFileSrc } from "@tauri-apps/api/core";
import { cmd } from "@/lib/tauri";
import { safeMarkdownImage } from "@/lib/markdown";

/** 在每次输入变化时清除旧图片,并忽略过期请求或组件卸载后的结果。 */
export function useMarkdownImage(
  getUrl: () => string,
  getBase: () => string,
  getRoot: () => string,
): Ref<string | undefined> {
  const src = ref<string>();
  watch(
    () => [getUrl(), getBase(), getRoot()] as const,
    ([url, base, root], _previous, onCleanup) => {
      let active = true;
      onCleanup(() => {
        active = false;
      });
      src.value = undefined;
      const safe = safeMarkdownImage(url, base, root);
      if (!safe) return;
      if (/^https?:\/\//i.test(safe)) {
        src.value = safe;
        return;
      }
      void cmd<string>("resolve_markdown_image", { root, imagePath: safe })
        .then((resolved) => {
          if (active) src.value = convertFileSrc(resolved);
        })
        .catch(() => {});
    },
    { immediate: true, flush: "sync" },
  );
  return src;
}
