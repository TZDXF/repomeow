/** 分支树节点:按 "/" 切分分支名后逐层聚合;branch 仅在节点本身也是分支时有值 */
export interface BranchTreeNode {
  name: string;
  fullPath: string;
  branch: string | null;
  children: BranchTreeNode[];
}

export function buildBranchTree(names: string[]): BranchTreeNode[] {
  const roots: BranchTreeNode[] = [];
  const byPath = new Map<string, BranchTreeNode>();
  for (const full of names) {
    const segs = full.split("/");
    let prefix = "";
    let siblings = roots;
    for (let i = 0; i < segs.length; i++) {
      prefix = prefix ? `${prefix}/${segs[i]}` : segs[i];
      let node = byPath.get(prefix);
      if (!node) {
        node = { name: segs[i], fullPath: prefix, branch: null, children: [] };
        byPath.set(prefix, node);
        siblings.push(node);
      }
      if (i === segs.length - 1) {
        node.branch = full;
      }
      siblings = node.children;
    }
  }
  return roots;
}
