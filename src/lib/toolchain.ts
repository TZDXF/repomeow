/**
 * 「开发环境」工具链面板的分组展示逻辑(纯函数,便于单测)。
 *
 * 后端 `detect_toolchains` 已按平台剔除 macOS 上未安装的 .NET / PowerShell 7,
 * 前端只按 kind 归组并丢掉空组:不适用的工具被后端剔除后,对应分组标题与空行
 * 不会留在设置页。分组顺序固定,未知 kind 不会被展示(避免后端新增生态时前端
 * 出现没图标的裸分组)。
 */
import type { ToolchainKind, ToolchainStatus } from "@/types";

/** 分组展示顺序(与后端 ToolchainKind 列举顺序对齐;java 由 JavaSection 单独渲染) */
export const TOOLCHAIN_KIND_ORDER: readonly ToolchainKind[] = [
  "rust",
  "python",
  "node",
  "dotnet",
  "go",
  "git",
  "powershell",
];

export interface ToolchainGroup {
  kind: ToolchainKind;
  tools: ToolchainStatus[];
}

/** 按 kind 归组并过滤空组;工具行顺序保持后端 TOOLS 注册表的返回顺序 */
export function groupToolchains(
  items: readonly ToolchainStatus[],
  order: readonly ToolchainKind[] = TOOLCHAIN_KIND_ORDER,
): ToolchainGroup[] {
  return order
    .map((kind) => ({ kind, tools: items.filter((tool) => tool.kind === kind) }))
    .filter((group) => group.tools.length > 0);
}
