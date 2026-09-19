import { computed, ref } from "vue";

/** 按仓库/分支隔离重试状态，每次执行固定目标，避免异步期间切换目标后误推。 */
export function useCommitAndPush<T extends { key: string } = { key: string }>(
  commit: (target: T) => Promise<void>,
  push: (target: T) => Promise<void>,
  getTarget: () => T,
) {
  const pendingTargets = ref(new Set<string>());
  const pendingPush = computed(() => pendingTargets.value.has(getTarget().key));

  async function run() {
    const target = getTarget();
    if (!pendingTargets.value.has(target.key)) {
      await commit(target);
      pendingTargets.value.add(target.key);
    }
    await push(target);
    pendingTargets.value.delete(target.key);
  }

  return { pendingPush, run };
}
