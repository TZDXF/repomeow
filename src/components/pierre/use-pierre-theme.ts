import { computed, onBeforeUnmount, ref } from "vue";
import { useSettingsStore } from "@/stores/settings";

/**
 * pierre 组件(@pierre/diffs / @pierre/trees)的亮暗主题桥接。
 * 与 stores/settings.ts 应用 .dark 的判定同一口径:theme = system 时跟随系统;
 * github-light / github-dark 双主题与 CommandEditor、原 diff 着色的 shiki 主题保持一致。
 */
export const PIERRE_THEMES = { light: "github-light", dark: "github-dark" } as const;

export function usePierreDark() {
  const settings = useSettingsStore();
  const media = window.matchMedia("(prefers-color-scheme: dark)");
  const systemMatches = ref(media.matches);
  const onChange = (e: MediaQueryListEvent) => {
    systemMatches.value = e.matches;
  };
  media.addEventListener("change", onChange);
  onBeforeUnmount(() => media.removeEventListener("change", onChange));
  return computed(
    () => settings.theme === "dark" || (settings.theme === "system" && systemMatches.value),
  );
}
