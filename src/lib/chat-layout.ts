/** 右侧问答栏至少为主工作区保留 360px;窄容器中主工作区至少占 40%。 */
export function chatSidebarBounds(containerWidth: number) {
  const max = Math.max(0, containerWidth - Math.min(360, containerWidth * 0.4));
  return { min: Math.min(420, max), max };
}

export function clampChatSidebarWidth(width: number, containerWidth: number) {
  const { min, max } = chatSidebarBounds(containerWidth);
  return Math.min(Math.max(width, min), max);
}
