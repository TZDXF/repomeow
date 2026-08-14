import type { ProjectFileEntry } from "@/types";

/**
 * 懒加载文件树的纯逻辑:后端按层返回目录子项(ProjectFiles 缓存于 childrenMap,
 * key 为目录相对路径、根为 ""),此处负责层内排序、可见行展开与预取目标挑选,
 * 与 Vue 状态解耦便于单测
 */

/** 单层预取的目录数上限:防止超宽目录(如未排除的依赖目录)一次排入过多后台请求 */
export const PREFETCH_MAX_PER_LEVEL = 100;

/** 条目显示名(相对路径最后一段) */
export function entryName(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

/** 层内排序:目录在前、文件在后,同层按名称排序(与 buildFileTree 同比较器) */
export function sortDirEntries(entries: ProjectFileEntry[]): ProjectFileEntry[] {
  return [...entries].sort((a, b) => {
    const dirDiff = Number(b.isDir) - Number(a.isDir);
    return dirDiff || entryName(a.path).localeCompare(entryName(b.path));
  });
}

/**
 * 预取目标:子项中未被 git 排除的目录(排除目录如 node_modules 展开时按需拉取,
 * 不提前扫),按当前顺序至多取 PREFETCH_MAX_PER_LEVEL 个
 */
export function prefetchTargets(children: readonly ProjectFileEntry[]): string[] {
  const out: string[] = [];
  for (const e of children) {
    if (!e.isDir || e.ignored) {
      continue;
    }
    out.push(e.path);
    if (out.length >= PREFETCH_MAX_PER_LEVEL) {
      break;
    }
  }
  return out;
}

export interface LazyFileRow {
  /** v-for key;加载占位行为 `${dir}::__loading` */
  key: string;
  name: string;
  fullPath: string;
  isDir: boolean;
  depth: number;
  /** 被 git 排除(.gitignore/.ignore),降低灰度显示 */
  dimmed: boolean;
  /** 目录已知为空(子层已加载且无子项):不渲染展开箭头、点击不动作 */
  emptyDir: boolean;
  /** 「加载中」占位行:目录已展开但子层未就位 */
  loading: boolean;
}

/**
 * 由 childrenMap + 展开集合生成可见行:DFS 根层,仅下钻「已展开且子层已加载」的目录;
 * 已展开但子层未就位的目录追加一行加载占位
 */
export function buildVisibleRows(
  childrenMap: ReadonlyMap<string, readonly ProjectFileEntry[]>,
  expanded: ReadonlySet<string>,
): LazyFileRow[] {
  const out: LazyFileRow[] = [];
  const walk = (dir: string, depth: number) => {
    const children = childrenMap.get(dir);
    if (!children) {
      return;
    }
    for (const entry of children) {
      const knownEmpty = entry.isDir && childrenMap.get(entry.path)?.length === 0;
      out.push({
        key: entry.path,
        name: entryName(entry.path),
        fullPath: entry.path,
        isDir: entry.isDir,
        depth,
        dimmed: entry.ignored,
        emptyDir: knownEmpty,
        loading: false,
      });
      if (!entry.isDir || !expanded.has(entry.path)) {
        continue;
      }
      if (childrenMap.has(entry.path)) {
        walk(entry.path, depth + 1);
      } else {
        out.push({
          key: `${entry.path}::__loading`,
          name: "",
          fullPath: entry.path,
          isDir: false,
          depth: depth + 1,
          dimmed: false,
          emptyDir: false,
          loading: true,
        });
      }
    }
  };
  walk("", 0);
  return out;
}
