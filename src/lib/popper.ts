import { getCurrentWindow } from "@tauri-apps/api/window";

/**
 * popper 类浮层(Select/DropdownMenu/ContextMenu/Popover/Tooltip)的碰撞边距。
 * 应用使用自绘标题栏(TitleBar.vue,h-9 = 36px,且 z-60 盖在浮层 z-50 之上),
 * floating-ui 只认视口边界,浮层向上展开时会钻进标题栏下方被遮挡;
 * 顶部边距设为标题栏高度后,翻转判定与 --reka-*-available-height 都会让出这块区域。
 * 托盘弹窗窗口没有标题栏,不需要让位。
 */
export const POPPER_COLLISION_PADDING = {
  top: getCurrentWindow().label === "tray-popup" ? 0 : 36,
};
