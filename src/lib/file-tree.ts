import type { GitCommitFile } from "@/types";

/**
 * 提交文件树节点:按 "/" 切分文件路径后逐层聚合;
 * file 仅在节点本身是文件(叶子)时有值,中间目录节点 file 为 null
 */
export interface FileTreeNode {
  name: string;
  fullPath: string;
  file: GitCommitFile | null;
  children: FileTreeNode[];
}

/** 把提交的文件列表按目录层级聚合成树(目录在前、文件在后,同层按名称排序) */
export function buildFileTree(files: GitCommitFile[]): FileTreeNode[] {
  const roots: FileTreeNode[] = [];
  const byPath = new Map<string, FileTreeNode>();
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
  const sortLevel = (nodes: FileTreeNode[]) => {
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
