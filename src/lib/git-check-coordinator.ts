import { cleanPath, toForwardSlash } from "@/lib/path";

/**
 * 单条路径 Git 检查的有效期(ms):与后端 `status_monitor.rs::STATUS_TTL` 对齐。
 * 后端缓存只保护 `force = false` 的调用,而前端有进入详情页、切 worktree、窗口聚焦、
 * 列表批量刷新等多个入口,所以额外共享一份路径级记录:同一路径 15s 内最多扫描一次
 * (`force` 同样受这条保护),确需真正重扫的入口先 `forget` 再发起。
 */
export const GIT_STATUS_TTL_MS = 15_000;

/** 路径归一化为记录 key:分隔符统一 + 小写(Windows 盘符/目录大小写不敏感) */
export function gitCheckKey(path: string): string {
  return toForwardSlash(cleanPath(path)).toLowerCase();
}

export interface GitCheckCoordinatorOptions {
  /** 记录有效期(ms),默认 GIT_STATUS_TTL_MS */
  ttlMs?: number;
  /** 注入时钟,便于测试 */
  now?: () => number;
}

/** 单次检查的结束回调入参:成功才写入记录,无论成败都解除进行中标记 */
export interface GitCheckFinishResult<T> {
  ok: boolean;
  value?: T;
}

interface InFlightEntry {
  check: Promise<unknown>;
  /** 本次登记的身份:登记被取代(并发覆盖/forget/reset)后旧回调不再改动记录 */
  token: object;
}

/**
 * 路径级 Git 检查协调器:进行中请求复用 + 最近一次成功检查的复用。
 *
 * 大仓库单次 `git status` 可能耗时数秒,窗口来回切换会让多个入口在极短时间内重复
 * 发起同一个仓库的扫描。各入口共享同一份记录后:
 * - 有效期内的结果直接复用,不再发 IPC(`force` 也不能绕开这条保护);
 * - 同一路径已有请求在跑时复用它,不叠加并发扫描;
 * - 记录只在检查成功后写入,失败(目录失效、git 报错)不占用有效期,允许重试。
 */
export function createGitCheckCoordinator(options: GitCheckCoordinatorOptions = {}) {
  const ttlMs = options.ttlMs ?? GIT_STATUS_TTL_MS,
    now = options.now ?? (() => Date.now()),
    inFlight = new Map<string, InFlightEntry>(),
    recent = new Map<string, { at: number; value: unknown }>();

  function freshAt(path: string, at: number) {
    const record = recent.get(gitCheckKey(path));
    if (!record) {
      return undefined;
    }
    // at < record.at 说明系统时钟回拨:按"不新鲜"处理,宁可多查一次也不长期冻结
    return at >= record.at && at - record.at < ttlMs ? record : undefined;
  }

  return {
    /** 该路径尚未结束的检查请求(无则 undefined),供后续入口复用而非重新发起 */
    pending<T>(path: string): Promise<T> | undefined {
      return inFlight.get(gitCheckKey(path))?.check as Promise<T> | undefined;
    },
    /** 该路径最近一次成功检查是否仍在有效期内(不区分本次调用是否要求 force) */
    fresh(path: string, at: number = now()): boolean {
      return freshAt(path, at) !== undefined;
    },
    /** 有效期内的最近一次检查结果;过期或从未检查过时为 undefined */
    cached<T>(path: string, at: number = now()): T | undefined {
      return freshAt(path, at)?.value as T | undefined;
    },
    /**
     * 登记一次进行中的检查,返回结束回调。
     * `ok === true` 才写入完成时间与结果;登记已被取代时回调不改动任何记录。
     */
    begin<T>(path: string, check: Promise<T>) {
      const key = gitCheckKey(path),
        token = {};
      inFlight.set(key, { check, token });
      return (result: GitCheckFinishResult<T> = { ok: false }) => {
        const entry = inFlight.get(key);
        if (!entry || entry.token !== token) {
          return;
        }
        inFlight.delete(key);
        if (result.ok) {
          recent.set(key, { at: now(), value: result.value });
        }
      };
    },
    /** 该路径刚被检查过(批量结果/后端事件推送):登记时间与结果,供紧随其后的入口复用 */
    record<T>(path: string, value: T, at: number = now()) {
      recent.set(gitCheckKey(path), { at, value });
    },
    /**
     * 彻底丢弃该路径的记录与进行中请求(路径变更、worktree 合回/变基后必须真正重扫)。
     * 旧请求的结束回调因 token 不匹配而不再写入记录,不会用过期结果污染缓存。
     */
    forget(path: string) {
      const key = gitCheckKey(path);
      recent.delete(key);
      inFlight.delete(key);
    },
    /** 清空全部记录(仅测试使用) */
    reset() {
      inFlight.clear();
      recent.clear();
    },
  };
}

export type GitCheckCoordinator = ReturnType<typeof createGitCheckCoordinator>;

/** 全应用共享的检查记录:store 的检查入口与窗口聚焦刷新都经它协调 */
export const gitCheckCoordinator = createGitCheckCoordinator();
