/** 按本地时钟对齐每个整点;回调延迟(如休眠恢复)后重新对齐,不累计漂移。 */
export function scheduleHourly(callback: () => void): () => void {
  let timer: ReturnType<typeof setTimeout>;
  let stopped = false;

  function schedule() {
    const now = new Date();
    const next = new Date(now);
    next.setHours(next.getHours() + 1, 0, 0, 0);
    timer = setTimeout(() => {
      try {
        callback();
      } finally {
        if (!stopped) schedule();
      }
    }, next.getTime() - now.getTime());
  }

  schedule();
  return () => {
    stopped = true;
    clearTimeout(timer);
  };
}
