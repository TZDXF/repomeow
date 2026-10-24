import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

// 安全基线防回归测试:渗透评估(2026-10)确认的三项 webview 配置弱点修复后,
// 用单测钉住配置,防止后续改动悄悄回退(csp:null / 作用域通配无 deny / opener 通配)。
const readRepoJson = (relativePath: string): unknown =>
  JSON.parse(readFileSync(fileURLToPath(new URL(`../${relativePath}`, import.meta.url)), "utf-8"));

describe("tauri 配置安全基线", () => {
  const conf = readRepoJson("src-tauri/tauri.conf.json") as {
    app: {
      security: {
        csp: string | null;
        assetProtocol: { scope: { allow: string[]; deny: string[] } };
      };
    };
  };
  const cap = readRepoJson("src-tauri/capabilities/default.json") as {
    permissions: (string | { identifier: string })[];
  };

  it("必须显式配置 CSP,不得回退为 null", () => {
    const csp = conf.app.security.csp;
    expect(csp).toBeTruthy();
    expect(csp).toContain("default-src 'self'");
    expect(csp).toContain("script-src 'self'");
    expect(csp).toContain("object-src 'none'");
  });

  it("asset 协议作用域必须保留敏感路径 deny 清单", () => {
    const scope = conf.app.security.assetProtocol.scope;
    expect(scope.allow).toContain("**");
    for (const pattern of [
      "**/.repomeow/**",
      "**/.ssh/**",
      "**/.env",
      "**/.npmrc",
      "**/*.pem",
      "**/*.key",
    ]) {
      expect(scope.deny).toContain(pattern);
    }
  });

  it("不得向前端授予 opener 任意路径打开能力", () => {
    const wildcardOpenPath = cap.permissions.some(
      (permission) =>
        typeof permission === "object" && permission.identifier === "opener:allow-open-path",
    );
    expect(wildcardOpenPath).toBe(false);
  });
});
