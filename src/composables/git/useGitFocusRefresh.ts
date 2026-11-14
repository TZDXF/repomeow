import { onBeforeUnmount, onMounted } from "vue";
import { getCurrentWindow } from "@tauri-apps/api/window";

/**
 * 窗口重新聚焦时触发一次 Git 检查的最小间隔(ms)。
 * 窗口来回切换(点编辑器、点浏览器、点回应用)会连续产生失焦/聚焦事件,
 * 这里节流,避免与后台定时检查、手动刷新叠加成重复的 git status 查询。
 */
export const FOCUS_REFRESH_INTERVAL = 10_000;

/**
 * 聚焦刷新闸门:首次聚焦直接放行,之后按最小间隔节流。
 * 抽成纯函数便于单测,不依赖 Tauri 窗口 API 与组件生命周期。
 */
export function createFocusRefreshGate(intervalMs: number = FOCUS_REFRESH_INTERVAL) {
  let lastAt: number | null = null;
  return {
    /** now 为当前时间戳(ms);距上次放行不足 intervalMs 时返回 false 并保持上次记录 */
    tryAcquire(now: number) {
      // 首次聚焦必须放行(用户切回窗口正是需要最新状态的时刻)
      if (lastAt !== null && now - lastAt < intervalMs) {
        return false;
      }
      lastAt = now;
      return true;
    },
  };
}

export interface GitFocusRefreshOptions {
  /** 页面是否处于激活态:缓存的项目页只在当前展示时响应窗口聚焦 */
  active?: () => boolean;
  /** 触发间隔(ms),默认 FOCUS_REFRESH_INTERVAL */
  intervalMs?: number;
  /** 注入时钟,便于测试 */
  now?: () => number;
}

/** 与窗口聚焦事件解耦的控制器,start/stop 显式调用(便于测试与生命周期管理) */
export function createGitFocusRefresh(
  refresh: () => void | Promise<unknown>,
  options: GitFocusRefreshOptions = {},
) {
  const gate = createFocusRefreshGate(options.intervalMs),
    now = options.now ?? (() => Date.now()),
    active = options.active ?? (() => true);
  let unlisten: (() => void) | undefined,
    stopped = false;

  async function run() {
    if (stopped || !active() || !gate.tryAcquire(now())) {
      return;
    }
    try {
      await refresh();
    } catch {
      // 失败静默:后台定时检查与 git://project-changed 事件推送会兜底,
      // 聚焦刷新不应向用户弹出错误提示(常见场景如目录被外部删除)
    }
  }

  async function start() {
    if (stopped) {
      return;
    }
    const stop = await getCurrentWindow().onFocusChanged(({ payload }) => {
      // 失焦不检查:此时用户已离开应用,查询结果大概率立刻过期
      if (payload) {
        void run();
      }
    });
    if (stopped) {
      // start 期间已卸载:立即退订,避免泄漏监听器
      stop();
      return;
    }
    unlisten = stop;
  }

  function stop() {
    stopped = true;
    unlisten?.();
    unlisten = undefined;
  }

  return { start, stop, run };
}

/**
 * 组件内使用:挂载后订阅窗口聚焦,卸载时退订。
 *
 * 详情页头部 Git 状态依赖后端定时检查(间隔由设置项决定,默认 10 分钟),
 * 期间在编辑器里改动文件不会及时反映。窗口重新聚焦时强制重查当前工作区,
 * 即可立刻看到变更(已暂存/已修改/未跟踪)。
 */
export function useGitFocusRefresh(
  refresh: () => void | Promise<unknown>,
  options: GitFocusRefreshOptions = {},
) {
  const controller = createGitFocusRefresh(refresh, options);
  onMounted(() => void controller.start());
  onBeforeUnmount(() => controller.stop());
}
