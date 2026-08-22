import type { GitCommitFile } from "@/types";

/**
 * 文件树节点:按 "/" 切分文件路径后逐层聚合;
 * file 仅在节点本身是文件(叶子)时有值,中间目录节点 file 为 null
 */
export interface FileTreeNode<T extends { path: string } = GitCommitFile> {
  name: string;
  fullPath: string;
  file: T | null;
  children: FileTreeNode<T>[];
}

/**
 * 统一文件树行:懒加载树(lazy-file-tree)与静态树此处共用的渲染行模型,
 * FileTreeList.vue 只认这个形状;data 携带业务数据(git 文件条目等)供 slot 使用
 */
export interface FileTreeRow<T = unknown> {
  /** v-for key;懒加载树的加载占位行为 `${dir}::__loading` */
  key: string;
  /** 显示名 */
  name: string;
  /** 完整相对路径(title 兜底 / 选中比较) */
  fullPath: string;
  isDir: boolean;
  depth: number;
  /** 目录行当前展开态(箭头方向 / 文件夹开闭图标) */
  expanded: boolean;
  /** 是否渲染展开箭头(已知空目录为 false) */
  expandable: boolean;
  /** 灰显(被 git 排除、不可提交等,语义由调用方定义) */
  dimmed: boolean;
  /** 「加载中」占位行 */
  loading: boolean;
  /** 行 title 覆盖(缺省用 fullPath) */
  title?: string;
  /** 业务数据(git 文件条目 / 目录条目),目录行可能为 null */
  data: T | null;
}

/** 把扁平文件列表按目录层级聚合成树(目录在前、文件在后,同层按名称排序) */
export function buildFileTree<T extends { path: string }>(files: T[]): FileTreeNode<T>[] {
  const roots: FileTreeNode<T>[] = [];
  const byPath = new Map<string, FileTreeNode<T>>();
  for (const file of files) {
    const segs = file.path.split("/");
    let prefix = "";
    let siblings = roots;
    for (let i = 0; i < segs.length; i++) {
      prefix = prefix ? `${prefix}/${segs[i]}` : segs[i];
      let node = byPath.get(prefix);
      if (!node) {
        node = { name: segs[i], fullPath: prefix, file: null, children: [] };
        byPath.set(prefix, node);
        siblings.push(node);
      }
      if (i === segs.length - 1) {
        node.file = file;
      }
      siblings = node.children;
    }
  }
  const sortLevel = (nodes: FileTreeNode<T>[]) => {
    nodes.sort((a, b) => {
      const dirDiff = Number(b.file === null) - Number(a.file === null);
      return dirDiff || a.name.localeCompare(b.name);
    });
    for (const n of nodes) {
      sortLevel(n.children);
    }
  };
  sortLevel(roots);
  return roots;
}

/** 静态树行化选项:dim 标记灰显文件、title 覆盖行 title(如重命名 old → new) */
export interface TreeRowOptions<T> {
  dim?: (file: T) => boolean;
  title?: (file: T) => string;
}

/**
 * 静态树拍平为可见行(跳过折叠目录的子级),与懒加载树的 buildVisibleRows
 * 输出同构,供 FileTreeList 统一渲染
 */
export function flattenVisibleTree<T extends { path: string }>(
  nodes: FileTreeNode<T>[],
  collapsed: ReadonlySet<string>,
  opts?: TreeRowOptions<T>,
): FileTreeRow<T>[] {
  const out: FileTreeRow<T>[] = [];
  const walk = (list: FileTreeNode<T>[], depth: number) => {
    for (const node of list) {
      // expanded 仅对目录行有意义(文件行恒 false)
      const expanded = node.file === null && !collapsed.has(node.fullPath);
      out.push({
        key: node.fullPath,
        name: node.name,
        fullPath: node.fullPath,
        isDir: node.file === null,
        depth,
        expanded,
        expandable: node.children.length > 0,
        dimmed: node.file ? (opts?.dim?.(node.file) ?? false) : false,
        loading: false,
        title: node.file ? opts?.title?.(node.file) : undefined,
        data: node.file,
      });
      if (node.children.length && expanded) {
        walk(node.children, depth + 1);
      }
    }
  };
  walk(nodes, 0);
  return out;
}

/** 平铺模式行化选项:name 覆盖显示名(如重命名箭头)、dim 标记灰显、title 覆盖行 title */
export interface FlatRowOptions<T> {
  name?: (file: T) => string;
  dim?: (file: T) => boolean;
  title?: (file: T) => string;
}

/** 平铺模式:文件列表直接转 depth 0 行(无箭头、不缩进),与树形行同构 */
export function flatFileRows<T extends { path: string }>(
  files: T[],
  opts?: FlatRowOptions<T>,
): FileTreeRow<T>[] {
  return files.map((file) => ({
    key: file.path,
    name: opts?.name?.(file) ?? file.path.slice(file.path.lastIndexOf("/") + 1),
    fullPath: file.path,
    isDir: false,
    depth: 0,
    expanded: false,
    expandable: false,
    dimmed: opts?.dim?.(file) ?? false,
    loading: false,
    title: opts?.title?.(file),
    data: file,
  }));
}
