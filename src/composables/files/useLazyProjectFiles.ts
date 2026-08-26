import { computed, nextTick, ref, watch, type ComputedRef, type Ref } from "vue";
import { isReadmeName } from "@/lib/file-kind";
import { buildVisibleRows, entryName, prefetchTargets, sortDirEntries } from "@/lib/lazy-file-tree";
import { cmd } from "@/lib/tauri";
import type { ProjectFileEntry } from "@/types";

const PREFETCH_CONCURRENCY = 4;

interface UseLazyProjectFilesOptions {
  rootPath: ComputedRef<string>;
  selected: Ref<string | null>;
}

/**
 * 项目文件树的逐层加载状态机。
 *
 * childrenMap 只缓存当前工作区已经读取的目录层级；可见目录的下一层会以固定并发数
 * 在后台预取。根路径变化时，旧请求通过序号失效，不会回填到新工作区。
 */
export function useLazyProjectFiles({ rootPath, selected }: UseLazyProjectFilesOptions) {
  const childrenMap = ref(new Map<string, ProjectFileEntry[]>());
  const expandedFolders = ref(new Set<string>());
  const listLoading = ref(false);
  const listError = ref(false);
  let listSeq = 0;
  const inflight = new Map<string, Promise<void>>();

  const prefetchQueue: string[] = [];
  const prefetchQueued = new Set<string>();
  let prefetchActive = 0;

  function prefetchNext(children: ProjectFileEntry[]) {
    for (const dir of prefetchTargets(children)) {
      if (childrenMap.value.has(dir) || inflight.has(dir) || prefetchQueued.has(dir)) {
        continue;
      }
      prefetchQueued.add(dir);
      prefetchQueue.push(dir);
    }
    pumpPrefetch();
  }

  function pumpPrefetch() {
    while (prefetchActive < PREFETCH_CONCURRENCY && prefetchQueue.length) {
      const dir = prefetchQueue.shift()!;
      prefetchQueued.delete(dir);
      prefetchActive++;
      void ensureChildren(dir).finally(() => {
        prefetchActive--;
        pumpPrefetch();
      });
    }
  }

  async function ensureChildren(dir: string): Promise<void> {
    if (childrenMap.value.has(dir)) {
      return;
    }
    const existing = inflight.get(dir);
    if (existing) {
      return existing;
    }
    const path = rootPath.value;
    if (!path) {
      return;
    }
    const seq = listSeq;
    const request = (async () => {
      try {
        const entries = await cmd<ProjectFileEntry[]>("list_project_files", {
          path,
          dir: dir || null,
        });
        if (seq !== listSeq) {
          return;
        }
        const next = new Map(childrenMap.value);
        next.set(dir, sortDirEntries(entries));
        childrenMap.value = next;
        if (dir === "" || expandedFolders.value.has(dir)) {
          prefetchNext(entries);
        }
      } catch {
        if (seq !== listSeq) {
          return;
        }
        if (dir === "") {
          listError.value = true;
        } else {
          const next = new Map(childrenMap.value);
          next.set(dir, []);
          childrenMap.value = next;
        }
      }
    })();
    inflight.set(dir, request);
    void request.finally(() => {
      if (inflight.get(dir) === request) {
        inflight.delete(dir);
      }
    });
    return request;
  }

  function toggleFolder(fullPath: string) {
    const children = childrenMap.value.get(fullPath);
    if (children && children.length === 0) {
      return;
    }
    const next = new Set(expandedFolders.value);
    if (next.has(fullPath)) {
      next.delete(fullPath);
    } else {
      next.add(fullPath);
      if (children) {
        prefetchNext(children);
      } else {
        void ensureChildren(fullPath);
      }
    }
    expandedFolders.value = next;
  }

  /** 加载并展开文件的全部祖先目录，使外部搜索选中的文件在树中可见。 */
  async function revealPath(path: string) {
    const segments = path.split("/");
    const prefixes: string[] = [];
    let prefix = "";
    for (let i = 0; i < segments.length - 1; i++) {
      prefix = prefix ? `${prefix}/${segments[i]}` : segments[i];
      prefixes.push(prefix);
    }
    await Promise.all(prefixes.map((dir) => ensureChildren(dir)));
    const next = new Set(expandedFolders.value);
    for (const dir of prefixes) {
      next.add(dir);
    }
    expandedFolders.value = next;
    await nextTick();
    document.querySelector(".file-row-selected")?.scrollIntoView({ block: "center" });
  }

  const visibleRows = computed(() => buildVisibleRows(childrenMap.value, expandedFolders.value));
  const rootEmpty = computed(
    () => childrenMap.value.has("") && childrenMap.value.get("")!.length === 0,
  );

  async function loadFiles() {
    if (!rootPath.value) {
      return;
    }
    listSeq++;
    inflight.clear();
    prefetchQueue.length = 0;
    prefetchQueued.clear();
    childrenMap.value = new Map();
    expandedFolders.value = new Set();
    listLoading.value = true;
    listError.value = false;
    try {
      await ensureChildren("");
      const readme = childrenMap.value
        .get("")
        ?.find((entry) => !entry.isDir && isReadmeName(entryName(entry.path)));
      if (readme && selected.value === null) {
        selected.value = readme.path;
      }
    } finally {
      listLoading.value = false;
    }
  }

  watch(rootPath, () => void loadFiles(), { immediate: true });

  return {
    expandedFolders,
    listError,
    listLoading,
    rootEmpty,
    revealPath,
    toggleFolder,
    visibleRows,
  };
}
