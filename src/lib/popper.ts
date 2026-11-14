import { getCurrentWindow } from "@tauri-apps/api/window";

/**
 * popper 类浮层(Select/DropdownMenu/ContextMenu/Popover/Tooltip)的碰撞边距。
 * 应用使用自绘标题栏(TitleBar.vue,高度取 style.css 的 --titlebar-height = 2.25rem = 36px,
 * 且 z-60 盖在浮层 z-50 之上),floating-ui 只认视口边界,浮层向上展开时会钻进标题栏
 * 下方被遮挡;顶部边距设为标题栏高度后,翻转判定与 --reka-*-available-height 都会让出这块区域。
 * 托盘弹窗窗口没有标题栏,不需要让位。
 * 注意:CSS 读不到这个常量,标题栏改高时请同步这里(两处都源自 --titlebar-height)。
 */
export const POPPER_COLLISION_PADDING = {
  top: getCurrentWindow().label === "tray-popup" ? 0 : 36,
};
