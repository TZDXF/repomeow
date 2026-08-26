import type { WikiPageStatus } from "@/lib/wiki-generator";

/** Wiki 左侧导航条目；生成中条目携带状态并以状态图标替代重要性标记。 */
export interface WikiNavItem {
  id: string;
  title: string;
  section: string | null;
  importance: string;
  status?: WikiPageStatus;
  error?: string;
  durationMs?: number;
  /** 生成中该页已产出的字符数(running 页实时更新) */
  wordCount?: number;
  unread?: boolean;
}
