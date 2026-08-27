import { onBeforeUnmount, readonly, ref } from "vue";

/**
 * canvas 类图表(ECharts)的主题感知:跟踪根节点 .dark 类与 data-theme 皮肤。
 * 亮暗或皮肤切换后 themeStamp 自增(isDark 不变的皮肤切换也靠它触发重建图表配色)。
 */
export function useChartTheme() {
  const root = document.documentElement;
  const isDark = ref(root.classList.contains("dark"));
  const themeStamp = ref(0);
  const observer = new MutationObserver(() => {
    isDark.value = root.classList.contains("dark");
    themeStamp.value++;
  });
  observer.observe(root, { attributes: true, attributeFilter: ["class", "data-theme"] });
  onBeforeUnmount(() => observer.disconnect());
  return { isDark: readonly(isDark), themeStamp: readonly(themeStamp) };
}
