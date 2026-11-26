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
    },
    cap = readRepoJson("src-tauri/capabilities/default.json") as {
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

// 平台覆盖文件(tauri.macos.conf.json)通过 `tauri build --config` 叠加到主配置上,
// 而 `app.windows` 是数组:JSON Merge Patch(RFC 7396)对数组是整元素替换而不是逐键合并。
// 于是主配置窗口的每个键都必须在覆盖文件里重述一遍——漏一个键(尺寸、visible、
// dragDropEnabled)该键就在 macOS 上静默退回 Tauri 默认值,而这类回退在 CI 上不可见。
// 本组断言就是这个隐性契约的守护:除装饰相关键外,两份配置的窗口必须逐键一致。
type WindowConfig = Record<string, unknown>;

/** 允许 macOS 覆盖文件与主配置不同的窗口键(仅与原生窗口装饰相关)。 */
const WINDOW_DECORATION_KEYS: readonly string[] = [
  "decorations",
  "titleBarStyle",
  "hiddenTitle",
  "trafficLightPosition",
];

const readWindows = (relativePath: string): WindowConfig[] =>
  (readRepoJson(relativePath) as { app: { windows: WindowConfig[] } }).app.windows;

const stripDecorationKeys = (window: WindowConfig): WindowConfig =>
  Object.fromEntries(
    Object.entries(window).filter(([key]) => !WINDOW_DECORATION_KEYS.includes(key)),
  );

describe("平台覆盖配置与主配置的窗口一致性", () => {
  const mainWindows = readWindows("src-tauri/tauri.conf.json"),
    macOSWindows = readWindows("src-tauri/tauri.macos.conf.json");

  it("macOS 覆盖文件必须重述主配置窗口的全部非装饰键", () => {
    expect(macOSWindows).toHaveLength(mainWindows.length);
    mainWindows.forEach((mainWindow, index) => {
      // 少键、多键、改值都会在这里失败:toEqual 比对键集合与值,而不只是“取到几个键”
      expect(stripDecorationKeys(macOSWindows[index])).toEqual(stripDecorationKeys(mainWindow));
    });
  });

  it("macOS 覆盖文件必须保留原生装饰并覆盖装饰相关键", () => {
    const macOSWindow = macOSWindows[0];
    expect(macOSWindow.decorations).toBe(true);
    expect(macOSWindow.titleBarStyle).toBe("Overlay");
    expect(macOSWindow.hiddenTitle).toBe(true);
    // 前端给交通灯预留的左侧间距由该值推导(见 lib/titlebar.test.ts)
    expect(macOSWindow.trafficLightPosition).toBeTypeOf("object");
  });
});
