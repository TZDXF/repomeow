import { onBeforeUnmount, onMounted, readonly, ref, watch } from "vue";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  gitCheckCoordinator,
  gitCheckKey,
  type GitCheckCoordinator,
} from "@/lib/git-check-coordinator";

/** 托盘迷你弹窗 label(与 Rust `tray.rs::TRAY_POPUP_LABEL` 一致) */
export const TRAY_POPUP_LABEL = "tray-popup";

/**
 * 聚焦触发检查的最小间隔(ms),与后端 `status_monitor.rs::STATUS_TTL`(15s)对齐:
 * 前端节流若比后端缓存有效期更宽松,就会绕过缓存,让同一路径在一次窗口切换里重复扫描。
 */
export const FOCUS_REFRESH_INTERVAL = 15_000;

/** 聚焦刷新闸门:首次聚焦直接放行,之后按最小间隔节流(纯函数,便于单测) */
export function createFocusRefreshGate(intervalMs: number = FOCUS_REFRESH_INTERVAL) {
  let lastAt: number | null = null;
  return {
    /** now 为当前时间戳(ms);距上次放行不足 intervalMs 时返回 false 并保留上次记录 */
    tryAcquire(now: number) {
      // 首次聚焦放行(用户切回窗口正是最需要最新状态的时刻)
      if (lastAt !== null && now - lastAt < intervalMs) {
        return false;
      }
      lastAt = now;
      return true;
    },
  };
}

/**
 * 窗口"重新聚焦"的版本号:主窗口只有 `App.vue` 订阅一次 Tauri 窗口事件并递增它,
 * 需要这个时机的页面 watch 版本号即可——同一窗口切换不会被多个监听器重复响应,
 * 也就不会成倍放大 Git 检查。
 */
const focusVersion = ref(0);

/** 只读聚焦版本号(页面侧 watch 它) */
export const windowFocusVersion = readonly(focusVersion);

/** 记录一次窗口重新聚焦(`App.vue` 的窗口事件与测试共用同一入口) */
export function notifyWindowFocus() {
  focusVersion.value += 1;
}

export interface WindowFocusSignalOptions {
  /** 当前窗口 label(测试注入),默认取 Tauri 当前窗口 */
  label?: () => string;
  /** 订阅窗口焦点(测试注入),默认 `onFocusChanged` */
  subscribe?: (handler: (focused: boolean) => void) => Promise<() => void>;
}

async function defaultSubscribe(handler: (focused: boolean) => void) {
  return getCurrentWindow().onFocusChanged(({ payload }) => handler(payload));
}

/**
 * 主窗口聚焦信号控制器:订阅窗口焦点,托盘迷你弹窗跳过,聚焦时递增版本号。
 * `start/stop` 显式暴露以便单测与生命周期管理。
 */
export function createWindowFocusSignal(options: WindowFocusSignalOptions = {}) {
  const label = options.label ?? (() => getCurrentWindow().label),
    subscribe = options.subscribe ?? defaultSubscribe;
  let unlisten: (() => void) | undefined,
    stopped = false;

  async function start() {
    if (stopped || label() === TRAY_POPUP_LABEL) {
      // 托盘迷你弹窗失焦即收起、也没有项目详情页:不参与主窗口的 Git 检查
      return;
    }
    const stop = await subscribe((focused) => {
      // 失焦不检查:此时用户已离开应用,查到的最新状态很快又会过期
      if (focused) {
        notifyWindowFocus();
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

  return { start, stop };
}

/** 组件内使用(`App.vue` 挂载唯一实例):托盘弹窗窗口内部自动跳过 */
export function useWindowFocusSignal(options: WindowFocusSignalOptions = {}) {
  const signal = createWindowFocusSignal(options);
  onMounted(() => void signal.start());
  onBeforeUnmount(signal.stop);
  return signal;
}

export interface GitFocusRefreshOptions {
  /** 本次聚焦要检查的目标路径;返回 null 表示当前没有可检查的项目 */
  target?: () => string | null;
  /** 页面是否处于激活态(缓存到后台的项目页不响应聚焦) */
  active?: () => boolean;
  /** 聚焦触发的最小间隔(ms),默认 FOCUS_REFRESH_INTERVAL */
  intervalMs?: number;
  /** 注入时钟,便于测试 */
  now?: () => number;
  /** 共享检查记录,默认全应用单例 */
  coordinator?: GitCheckCoordinator;
  /** 当前窗口 label(测试注入),默认取 Tauri 当前窗口 */
  label?: () => string;
  /** 直接订阅窗口焦点(测试注入),默认 `onFocusChanged` */
  subscribe?: (handler: (focused: boolean) => void) => Promise<() => void>;
}

/**
 * 聚焦刷新的纯逻辑部分(与窗口事件、组件生命周期解耦),便于单测。
 *
 * 避免大仓库被反复 `git status` 的三重保护:
 * 1. 页面未激活或没有目标路径时不检查;
 * 2. 共享协调器——同一路径 15s 内已被任何入口成功检查时直接复用(覆盖"刚进详情页
 *    的普通检查"),因此聚焦不会再叠一次强制扫描;
 * 3. 进行中请求——同一路径已有检查在跑时复用它,慢查询不会被下一次聚焦叠加;
 * 4. 聚焦闸门——同一控制器在 intervalMs 内不重复触发。
 */
export function createGitFocusRefresh(
  refresh: () => void | Promise<unknown>,
  options: GitFocusRefreshOptions = {},
) {
  const coordinator = options.coordinator ?? gitCheckCoordinator,
    gate = createFocusRefreshGate(options.intervalMs),
    now = options.now ?? (() => Date.now()),
    active = options.active ?? (() => true),
    label = options.label ?? (() => getCurrentWindow().label),
    subscribe = options.subscribe ?? defaultSubscribe;
  let running: Promise<unknown> | undefined,
    unlisten: (() => void) | undefined,
    stopped = false;

  /** 托盘迷你弹窗失焦即收起、也没有项目详情页:不参与主窗口的 Git 检查 */
  function inTrayPopup() {
    try {
      return label() === TRAY_POPUP_LABEL;
    } catch {
      // 非 Tauri 环境(单测)拿不到窗口 label:按主窗口处理
      return false;
    }
  }

  async function run() {
    if (stopped || inTrayPopup() || !active()) {
      return;
    }
    const path = options.target ? options.target() : null;
    if (options.target && !path) {
      // 首页/设置/报告页没有具体项目:聚焦不需要任何检查
      return;
    }
    if (running) {
      // 同页的检查仍在进行:复用它,慢查询不会被下一次聚焦叠加
      await running.catch(() => undefined);
      return;
    }
    if (path !== null) {
      if (coordinator.fresh(path, now())) {
        // 刚被任何入口检查过(进入详情页/写操作/事件推送/上一轮聚焦):复用结果,不重扫
        return;
      }
      const pending = coordinator.pending(path);
      if (pending) {
        await pending.catch(() => undefined);
        return;
      }
    }
    if (!gate.tryAcquire(now())) {
      return;
    }
    let request: Promise<unknown>;
    try {
      request = Promise.resolve(refresh());
    } catch {
      // 同步抛出同样静默(目录被外部删除等),后台事件推送会兜底
      return;
    }
    const finish = path === null ? undefined : coordinator.begin(path, request);
    running = request;
    try {
      const value = await request;
      finish?.({ ok: true, value });
    } catch {
      finish?.({ ok: false });
      // 失败静默:后台定时检查与 git://project-changed 事件推送会兜底;
      // 聚焦刷新不弹错(目录被外部删除、git 临时报错都很常见)
    } finally {
      if (running === request) {
        running = undefined;
      }
    }
  }

  /** 直接订阅窗口焦点(独立使用时;主窗口事件统一由 `useWindowFocusSignal` 承担) */
  async function start() {
    if (stopped || inTrayPopup()) {
      return;
    }
    const stop = await subscribe((focused) => {
      // 失焦不检查:此时用户已离开应用,查到的最新状态很快又会过期
      if (focused) {
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
    running = undefined;
  }

  return { run, start, stop };
}

/**
 * 页面侧使用:窗口重新聚焦时检查当前工作区的 Git 状态。
 *
 * 页面自己提供 `refresh`(结果写回哪个项目对象、worktree 副本等页面私有状态)与
 * 可选 `target`(用于按路径做重复检查去重),窗口事件与托盘跳过由 `App.vue` 的
 * `useWindowFocusSignal` 统一承担,这里只消费聚焦版本号。
 *
 * 同一路径的重复检查由共享协调器挡下:页面进入时的普通检查与紧随其后的聚焦刷新
 * 只会真正扫描一次。
 */
export function useGitFocusRefresh(
  refresh: () => void | Promise<unknown>,
  options: GitFocusRefreshOptions = {},
) {
  const controller = createGitFocusRefresh(refresh, options),
    stopWatch = watch(
      () => windowFocusVersion.value,
      () => void controller.run(),
    );
  onBeforeUnmount(stopWatch);
  return controller;
}

/** 供 store 与页面复用的路径 key 归一化 */
export { gitCheckKey };
