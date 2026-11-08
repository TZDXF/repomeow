import type { Project } from "@/types";

/**
 * 收藏分组比较器:收藏项目置顶,组内按收藏时间倒序(最近收藏最前)。
 * 返回 0 表示两者同组(都收藏或都未收藏),调用方再叠加各自的排序键。
 */
export function compareFavorited(a: Project, b: Project): number {
  const fa = a.favorited_at ?? 0,
    fb = b.favorited_at ?? 0;
  return fb - fa;
}
