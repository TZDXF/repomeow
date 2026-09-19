import { ref } from "vue";

/** 提交成功后保留推送阶段，失败重试不得再次提交（包括部分文件提交）。 */
export function useCommitAndPush(commit: () => Promise<void>, push: () => Promise<void>) {
  const pendingPush = ref(false);

  async function run() {
    if (!pendingPush.value) {
      await commit();
      pendingPush.value = true;
    }
    await push();
    pendingPush.value = false;
  }

  return { pendingPush, run };
}
