/**
 * 简单并发池:worker 模式,共享游标取任务;signal 中止时停止派发新任务。
 * 批量报告与 wiki 逐页生成共用
 */
export async function runPool(
  limit: number,
  tasks: (() => Promise<void>)[],
  signal: AbortSignal,
): Promise<void> {
  let index = 0;
  async function worker() {
    while (index < tasks.length && !signal.aborted) {
      const task = tasks[index++];
      // 并发池语义:单个 worker 内必须串行 await,并行度由 worker 数量控制
      // eslint-disable-next-line no-await-in-loop
      await task();
    }
  }
  const n = Math.max(1, Math.min(limit, tasks.length));
  await Promise.all(Array.from({ length: n }, worker));
}
