import { ref } from "vue";
import { useLatestRequest } from "@/composables/useLatestRequest";
import type { GitCommitFileDiff } from "@/types";

/** 统一管理 diff 请求竞态；保留旧内容可避免切换文件时闪烁。 */
export function useGitDiffLoader(options: { keepPrevious: boolean }) {
  const diff = ref<GitCommitFileDiff | null>(null);
  const loading = ref(false);
  const error = ref("");
  const request = useLatestRequest();

  async function load(fetchDiff: () => Promise<GitCommitFileDiff>) {
    const isCurrent = request.begin();
    if (!options.keepPrevious) diff.value = null;
    loading.value = !options.keepPrevious || !diff.value;
    error.value = "";
    try {
      const result = await fetchDiff();
      if (isCurrent()) diff.value = result;
    } catch (e) {
      if (isCurrent()) error.value = String(e);
    } finally {
      if (isCurrent()) loading.value = false;
    }
  }

  function reset() {
    request.invalidate();
    diff.value = null;
    loading.value = false;
    error.value = "";
  }
  return { diff, loading, error, load, reset };
}
