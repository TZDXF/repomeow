import { computed, ref, watch, type ComputedRef, type Ref } from "vue";
import type CodeViewer from "@/components/files/CodeViewer.vue";
import type FindBar from "@/components/files/FindBar.vue";
import { buildFindRegExp, type FindQuery } from "@/lib/text-search";

interface UseFileFindOptions {
  codeViewer: Ref<InstanceType<typeof CodeViewer> | null>;
  codeVisible: ComputedRef<boolean>;
  findBarRef: Ref<InstanceType<typeof FindBar> | null>;
  previewText: Ref<string | null>;
}

/** 管理 CodeViewer 的页内查找状态、匹配导航以及文档切换后的查询刷新。 */
export function useFileFind({
  codeViewer,
  codeVisible,
  findBarRef,
  previewText,
}: UseFileFindOptions) {
  const findOpen = ref(false);
  const findText = ref("");
  const findCase = ref(false);
  const findWord = ref(false);
  const findRegex = ref(false);
  const findTotal = ref(0);
  const findIndex = ref(-1);

  const findQuery = computed<FindQuery>(() => ({
    text: findText.value,
    caseSensitive: findCase.value,
    wholeWord: findWord.value,
    useRegex: findRegex.value,
  }));

  const findInvalid = computed(() => {
    if (!findRegex.value || !findText.value.trim()) {
      return false;
    }
    return (
      buildFindRegExp({
        text: findText.value,
        caseSensitive: true,
        wholeWord: true,
        useRegex: true,
      }) === null
    );
  });

  function refreshFind(scrollToCurrent: boolean) {
    const viewer = codeViewer.value;
    if (!viewer) {
      return;
    }
    const ranges = viewer.runFind(findQuery.value);
    findTotal.value = ranges.length;
    findIndex.value = viewer.getFindCursor();
    if (scrollToCurrent && ranges.length) {
      findIndex.value = viewer.gotoMatch(viewer.getFindCursor());
    }
  }

  watch([findText, findCase, findWord, findRegex], () => {
    if (findOpen.value) {
      refreshFind(true);
    }
  });

  // post-flush 保证 CodeViewer 已经先切换到新文档，Markdown 切源码时实例也已挂载。
  watch(
    [codeVisible, previewText],
    () => {
      if (findOpen.value && codeVisible.value) {
        refreshFind(false);
      }
    },
    { flush: "post" },
  );

  function findStep(delta: number) {
    if (!findTotal.value) {
      return;
    }
    findIndex.value = codeViewer.value?.gotoMatch(findIndex.value + delta) ?? -1;
  }

  function onFindToggle(key: "caseSensitive" | "wholeWord" | "useRegex") {
    if (key === "caseSensitive") {
      findCase.value = !findCase.value;
    } else if (key === "wholeWord") {
      findWord.value = !findWord.value;
    } else {
      findRegex.value = !findRegex.value;
    }
  }

  function openFind() {
    if (!codeVisible.value) {
      return;
    }
    findOpen.value = true;
    refreshFind(true);
    findBarRef.value?.focusInput();
  }

  function closeFind() {
    findOpen.value = false;
    findTotal.value = 0;
    findIndex.value = -1;
    codeViewer.value?.clearFind();
  }

  return {
    closeFind,
    findCase,
    findIndex,
    findInvalid,
    findOpen,
    findQuery,
    findRegex,
    findStep,
    findText,
    findTotal,
    findWord,
    onFindToggle,
    openFind,
  };
}
