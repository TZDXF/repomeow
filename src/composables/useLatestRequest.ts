/** 标记最新请求；失效后，旧请求不得再更新状态。 */
export function useLatestRequest() {
  let generation = 0;
  return {
    begin() {
      const current = ++generation;
      return () => current === generation;
    },
    invalidate() {
      generation += 1;
    },
  };
}
