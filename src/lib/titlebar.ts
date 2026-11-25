/**
 * 标题栏布局决策(纯函数,便于单测)。
 *
 * RepoMeow 的窗口装饰按平台分两套:
 * - Windows/Linux:`src-tauri/tauri.conf.json` 的 `decorations: false`,标题栏完全自绘,
 *   右侧自带最小化/最大化/关闭三个按钮。
 * - macOS:`src-tauri/tauri.macos.conf.json` 覆写为 `decorations: true` +
 *   `titleBarStyle: Overlay` + `hiddenTitle: true`,窗口保留系统圆角与阴影,左上角由系统
 *   绘制红黄绿交通灯;前端标题栏只保留业务控件(tab 条、报告/设置/更新入口)。
 *
 * 因此 macOS 上前端需要:(1) 让出交通灯占用的左侧空间;(2) 不再渲染自绘窗口按钮;
 * (3) 不自己处理双击缩放(见下)。
 *
 * 注意 `app.windows` 是数组,平台覆盖文件按 JSON Merge Patch(RFC 7396)整体替换数组元素,
 * 所以 `tauri.macos.conf.json` 里重述了尺寸等公共键;改动主配置的窗口尺寸时要同步这一份。
 */

/** Windows 风格自绘标题栏的左内边距(12px,Tailwind 的 pl-3)。 */
export const TITLEBAR_EDGE_INSET_PX = 12;

/**
 * macOS 上给原生交通灯预留的左侧间距(px)。
 * 实测(tauri.macos.conf.json 的 `trafficLightPosition: {x:16, y:20}`)三个按钮的窗口坐标:
 * close 16..30、mini 39..53、zoom 62..76,每个 14x14,纵向中心 18px 正好是 36px 标题栏的中线。
 * 留到 80px 让 tab 条从绿灯右侧 4px 起,不会被压住。
 * 全屏时不做收放:系统全屏下标题栏自动隐藏、鼠标移到顶边才浮出交通灯(位置不变),
 * 一旦收成 12px,浮出的交通灯就会盖住 tab 条;80px 的空隙在业务上没有成本。
 */
export const MACOS_TRAFFIC_LIGHT_INSET_PX = 80;

export interface TitleBarLayout {
  /** 是否渲染自绘的最小化/最大化/关闭按钮 */
  showWindowControls: boolean;
  /** 双击自绘拖拽区是否由前端调用 toggleMaximize */
  handleDragRegionDblClick: boolean;
  /** 标题栏左内边距(px) */
  leadInsetPx: number;
}

/**
 * @param macOS 运行平台是否为 macOS(见 `lib/platform.ts`)
 */
export function resolveTitleBarLayout({ macOS }: { macOS: boolean }): TitleBarLayout {
  if (!macOS) {
    return {
      showWindowControls: true,
      handleDragRegionDblClick: true,
      leadInsetPx: TITLEBAR_EDGE_INSET_PX,
    };
  }
  return {
    // 交通灯已由系统绘制;自绘按钮会与它们重复,且「红点在左、关闭在右」不符合 macOS 习惯
    showWindowControls: false,
    // Tauri 内置的 drag.js 在 macOS 上改为 mouseup 触发 internal_toggle_maximize
    // (鼠标移出拖拽区可取消,与系统行为一致);前端再调一次 toggleMaximize 会互相抵消
    handleDragRegionDblClick: false,
    leadInsetPx: MACOS_TRAFFIC_LIGHT_INSET_PX,
  };
}
