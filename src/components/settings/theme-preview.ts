/** 将鼠标在选项中的横坐标归一化为亮色预览占比。 */
export function themePreviewSplit(clientX: number, left: number, width: number): number {
  if (width <= 0) {
    return 50;
  }
  return Math.min(100, Math.max(0, ((clientX - left) / width) * 100));
}
