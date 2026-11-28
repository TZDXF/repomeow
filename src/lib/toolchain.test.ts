import { describe, expect, it } from "vitest";
import { groupToolchains, TOOLCHAIN_KIND_ORDER } from "./toolchain";
import type { ToolchainKind, ToolchainStatus } from "@/types";

function tool(id: string, kind: ToolchainKind, found = true): ToolchainStatus {
  return {
    id,
    kind,
    found,
    version: found ? "1.0.0" : null,
    path: null,
    source: null,
    versions: [],
    account: null,
    caps: {
      can_install: false,
      can_update: false,
      can_uninstall: false,
      can_switch: false,
      can_list_remote: false,
    },
  };
}

// TZD-157:后端在 macOS 上剔除未安装的 .NET / PowerShell 7 后,前端不得残留空分组标题。
describe("groupToolchains", () => {
  it("按固定顺序分组并保留组内顺序", () => {
    const groups = groupToolchains([
      tool("gh", "git"),
      tool("node", "node"),
      tool("git", "git"),
      tool("cargo", "rust"),
      tool("go", "go"),
    ]);
    expect(groups.map((group) => group.kind)).toEqual(["rust", "node", "go", "git"]);
    // 组内顺序沿用后端 TOOLS 注册表返回顺序(这里 gh 在 git 之前)
    expect(groups[3].tools.map((item) => item.id)).toEqual(["gh", "git"]);
  });

  it("macOS 上缺少 .NET / PowerShell 项时不出现对应分组", () => {
    const macos = [tool("rustup", "rust"), tool("node", "node", false), tool("git", "git")];
    expect(groupToolchains(macos).map((group) => group.kind)).toEqual(["rust", "node", "git"]);
    // 已安装的 .NET 照常分出 dotnet 组
    const withDotnet = [...macos, tool("dotnet", "dotnet")];
    expect(groupToolchains(withDotnet).map((group) => group.kind)).toContain("dotnet");
  });

  it("空列表、只有一条工具、未知 kind 均不产生空组或裸分组", () => {
    expect(groupToolchains([])).toEqual([]);
    expect(groupToolchains([tool("pwsh", "powershell")]).map((group) => group.kind)).toEqual([
      "powershell",
    ]);
    // 后端若新增未登记的 kind,前端宁可暂不展示,也不产生没图标的裸分组
    const unknown = tool("unknown", "kotlin" as ToolchainKind);
    expect(groupToolchains([unknown])).toEqual([]);
  });

  it("分组顺序常量与后端 ToolchainKind 取值一一对应", () => {
    expect(new Set(TOOLCHAIN_KIND_ORDER).size).toBe(TOOLCHAIN_KIND_ORDER.length);
    const groups = groupToolchains(TOOLCHAIN_KIND_ORDER.map((kind) => tool(kind, kind)));
    expect(groups.map((group) => group.kind)).toEqual([...TOOLCHAIN_KIND_ORDER]);
  });
});
