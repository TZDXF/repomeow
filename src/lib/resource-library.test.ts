import { describe, expect, it, vi } from "vitest";
import {
  filterMarketplaceSkills,
  filterSkills,
  formatArgLines,
  formatEnvLines,
  formatHeaderLines,
  isResourceMcpTransport,
  markMarketplaceInstalled,
  mergeMarketplaceSources,
  parseArgLines,
  parseEnvLines,
  parseHeaderLines,
  parseResourceMcpJson,
  ResourceMcpJsonError,
  collectSkillSources,
  type ResourceMarketplaceSkill,
  type ResourceSkill,
} from "./resource-library";

// resource-library.ts 顶层 import 了 @/lib/tauri(invoke/listen),Node 测试环境需替换
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(),
}));

function skill(partial: Partial<ResourceSkill> & { id: string; name: string }): ResourceSkill {
  return {
    description: "",
    directory: partial.name.toLowerCase().replace(/ /g, "-"),
    groupIds: [],
    sortOrder: 0,
    ...partial,
  };
}

describe("filterSkills", () => {
  const skills = [
    skill({ id: "a", name: "Git Commit", description: "生成提交信息", groupIds: ["git"] }),
    skill({ id: "b", name: "Wiki Write", description: "写项目 wiki", groupIds: ["wiki", "doc"] }),
    skill({ id: "c", name: "Docker Deploy", description: "部署 docker compose" }),
  ];

  it("空关键词且不筛分组时返回全部", () => {
    expect(filterSkills(skills, "", null)).toHaveLength(3);
    expect(filterSkills(skills, "   ", null)).toHaveLength(3);
  });

  it("按分组过滤", () => {
    expect(filterSkills(skills, "", "git").map((s) => s.id)).toEqual(["a"]);
    expect(filterSkills(skills, "", "doc").map((s) => s.id)).toEqual(["b"]);
    expect(filterSkills(skills, "", "missing")).toEqual([]);
  });

  it("关键词匹配名称或描述,大小写不敏感", () => {
    expect(filterSkills(skills, "commit", null).map((s) => s.id)).toEqual(["a"]);
    expect(filterSkills(skills, "WIKI", null).map((s) => s.id)).toEqual(["b"]);
    expect(filterSkills(skills, "部署", null).map((s) => s.id)).toEqual(["c"]);
    expect(filterSkills(skills, "不存在", null)).toEqual([]);
  });

  it("分组与关键词同时生效", () => {
    expect(filterSkills(skills, "write", "wiki").map((s) => s.id)).toEqual(["b"]);
    expect(filterSkills(skills, "write", "git")).toEqual([]);
  });

  it("按来源过滤市场技能,手动技能不命中任何来源", () => {
    const withSource = [
      skill({
        id: "a",
        name: "Git Commit",
        groupIds: ["git"],
        marketplace: { id: "m1", source: "owner/repo-a", url: "https://example.com/a" },
      }),
      skill({
        id: "b",
        name: "Wiki Write",
        marketplace: { id: "m2", source: "owner/repo-b", url: "https://example.com/b" },
      }),
      skill({ id: "c", name: "Docker Deploy" }),
    ];
    expect(filterSkills(withSource, "", null, "owner/repo-a").map((s) => s.id)).toEqual(["a"]);
    expect(filterSkills(withSource, "", null, null)).toHaveLength(3);
    // 来源与分组同时生效(AND 语义)
    expect(filterSkills(withSource, "", "git", "owner/repo-a").map((s) => s.id)).toEqual(["a"]);
    expect(filterSkills(withSource, "", "git", "owner/repo-b")).toEqual([]);
  });
});

describe("collectSkillSources", () => {
  it("去重、按名称排序,手动技能不产生来源", () => {
    const skills = [
      skill({
        id: "a",
        name: "B",
        marketplace: { id: "m1", source: "owner/repo-b", url: "https://example.com/b" },
      }),
      skill({
        id: "b",
        name: "A",
        marketplace: { id: "m2", source: "owner/repo-a", url: "https://example.com/a" },
      }),
      skill({
        id: "c",
        name: "C",
        marketplace: { id: "m3", source: "owner/repo-b", url: "https://example.com/b" },
      }),
      skill({ id: "d", name: "D" }),
    ];
    expect(collectSkillSources(skills)).toEqual(["owner/repo-a", "owner/repo-b"]);
    expect(collectSkillSources([])).toEqual([]);
  });
});

describe("parseEnvLines / formatEnvLines", () => {
  it("解析 KEY=VALUE 并忽略空行与注释", () => {
    expect(parseEnvLines("A=1\n\n# 注释\nB=two words\nbad-line\n")).toEqual({
      A: "1",
      B: "two words",
    });
  });

  it("重复键后写覆盖先写", () => {
    expect(parseEnvLines("A=1\nA=2")).toEqual({ A: "2" });
  });

  it("无 = 的行整体忽略,= 在行首也忽略", () => {
    expect(parseEnvLines("A=1\n=oops\nB=2")).toEqual({ A: "1", B: "2" });
  });

  it("roundtrip 保持键值", () => {
    const env = { FOO: "bar", "A B": "c=d" };
    expect(parseEnvLines(formatEnvLines(env))).toEqual(env);
  });
});

describe("parseHeaderLines / formatHeaderLines", () => {
  it("解析 Header: value 并忽略空行与注释", () => {
    expect(parseHeaderLines("Authorization: Bearer x\n\n# 注释\nX-Trace: on\nbad\n")).toEqual({
      Authorization: "Bearer x",
      "X-Trace": "on",
    });
  });

  it("重复头后写覆盖先写", () => {
    expect(parseHeaderLines("X-A: 1\nX-A: 2")).toEqual({ "X-A": "2" });
  });

  it("roundtrip 保持头", () => {
    const headers = { Authorization: "Bearer x", Accept: "application/json" };
    expect(parseHeaderLines(formatHeaderLines(headers))).toEqual(headers);
  });
});

describe("parseArgLines / formatArgLines", () => {
  it("每行一个参数,忽略空行与 # 注释", () => {
    expect(parseArgLines("--flag\n\n# 注释\nvalue\n  spaced  ")).toEqual([
      "--flag",
      "value",
      "spaced",
    ]);
  });

  it("roundtrip", () => {
    expect(parseArgLines(formatArgLines(["--flag", "a b"]))).toEqual(["--flag", "a b"]);
  });
});

describe("isResourceMcpTransport", () => {
  it("识别三种传输方式", () => {
    expect(isResourceMcpTransport("stdio")).toBe(true);
    expect(isResourceMcpTransport("http")).toBe(true);
    expect(isResourceMcpTransport("sse")).toBe(true);
  });

  it("未知值与其他类型返回 false", () => {
    expect(isResourceMcpTransport("ws")).toBe(false);
    expect(isResourceMcpTransport(null)).toBe(false);
    expect(isResourceMcpTransport(1)).toBe(false);
  });
});

describe("parseResourceMcpJson", () => {
  it("解析 mcpServers 包装结构(stdio + 远程)", () => {
    const text = JSON.stringify({
      mcpServers: {
        filesystem: {
          command: "npx",
          args: ["-y", "@modelcontextprotocol/server-filesystem"],
          env: { ROOT: "/tmp" },
        },
        remote: {
          type: "http",
          url: "https://mcp.example.com/mcp",
          headers: { Authorization: "Bearer xxx" },
        },
      },
    });
    const parsed = parseResourceMcpJson(text);
    expect(parsed.skipped).toEqual([]);
    expect(parsed.entries).toEqual([
      {
        name: "filesystem",
        transport: "stdio",
        command: "npx",
        args: ["-y", "@modelcontextprotocol/server-filesystem"],
        env: { ROOT: "/tmp" },
      },
      {
        name: "remote",
        transport: "http",
        url: "https://mcp.example.com/mcp",
        headers: { Authorization: "Bearer xxx" },
      },
    ]);
  });

  it("解析 servers 包装结构与裸键值表", () => {
    const wrapped = parseResourceMcpJson(
      JSON.stringify({ servers: { fs: { command: "uvx", args: ["mcp"] } } }),
    );
    expect(wrapped.entries).toEqual([
      { name: "fs", transport: "stdio", command: "uvx", args: ["mcp"] },
    ]);
    const bare = parseResourceMcpJson(
      JSON.stringify({ gh: { url: "https://x/sse", type: "sse" } }),
    );
    expect(bare.entries).toEqual([{ name: "gh", transport: "sse", url: "https://x/sse" }]);
  });

  it("裸键值表兼容名称与定义对象分行的多行排版", () => {
    const text = `{"blender":
{
  "command": "cmd",
  "args": [
    "/c",
    "uvx",
    "blender-mcp"
  ]
}}`;
    const parsed = parseResourceMcpJson(text);
    expect(parsed.skipped).toEqual([]);
    expect(parsed.entries).toEqual([
      { name: "blender", transport: "stdio", command: "cmd", args: ["/c", "uvx", "blender-mcp"] },
    ]);
  });

  it("单个服务器定义对象:名称取自身 name 字段,缺失时留空待补填", () => {
    const named = parseResourceMcpJson(
      JSON.stringify({ name: "我的服务", command: "node", args: ["server.js"] }),
    );
    expect(named.entries).toEqual([
      { name: "我的服务", transport: "stdio", command: "node", args: ["server.js"] },
    ]);
    const unnamed = parseResourceMcpJson(JSON.stringify({ url: "https://mcp.example.com" }));
    expect(unnamed.entries).toEqual([
      { name: "", transport: "http", url: "https://mcp.example.com" },
    ]);
  });

  it("type/transport 别名归一,未声明类型按形状推断", () => {
    const parsed = parseResourceMcpJson(
      JSON.stringify({
        a: { type: "streamable-http", url: "https://a" },
        b: { transport: "STDIO", command: "npx" },
        c: { command: "node", url: "https://b" },
      }),
    );
    expect(parsed.entries.map((entry) => entry.transport)).toEqual(["http", "stdio", "stdio"]);
  });

  it("缺 command/url 与未知 type 的条目跳过,其余照常解析", () => {
    const parsed = parseResourceMcpJson(
      JSON.stringify({
        ok: { command: "npx" },
        empty: { args: ["-y"] },
        ws: { type: "websocket", url: "wss://x" },
      }),
    );
    expect(parsed.entries).toEqual([{ name: "ok", transport: "stdio", command: "npx" }]);
    expect(parsed.skipped).toEqual([
      { name: "empty", reason: "invalid" },
      { name: "ws", reason: "unsupported" },
    ]);
  });

  it("标量 env/headers/args 值字符串化,非标量丢弃", () => {
    const parsed = parseResourceMcpJson(
      JSON.stringify({
        s: { command: "npx", args: ["-y", 3, null, ["x"]], env: { N: 1, B: true, BAD: { a: 1 } } },
      }),
    );
    expect(parsed.entries[0]).toEqual({
      name: "s",
      transport: "stdio",
      command: "npx",
      args: ["-y", "3"],
      env: { N: "1", B: "true" },
    });
  });

  it("非法 JSON 与无法识别的结构抛 ResourceMcpJsonError", () => {
    try {
      parseResourceMcpJson("{ not json");
      expect.unreachable();
    } catch (e) {
      expect((e as ResourceMcpJsonError).code).toBe("invalidJson");
    }
    try {
      parseResourceMcpJson("[1, 2]");
      expect.unreachable();
    } catch (e) {
      expect((e as ResourceMcpJsonError).code).toBe("unrecognized");
    }
    try {
      parseResourceMcpJson('{"foo": "bar"}');
      expect.unreachable();
    } catch (e) {
      expect((e as ResourceMcpJsonError).code).toBe("unrecognized");
    }
  });
});

function marketplaceSkill(
  partial: Partial<ResourceMarketplaceSkill> & { id: string; name: string },
): ResourceMarketplaceSkill {
  return {
    description: "",
    source: "builtin",
    installs: 0,
    url: "https://skills.sh/test",
    ...partial,
  };
}

describe("filterMarketplaceSkills", () => {
  const entries = [
    marketplaceSkill({ id: "m1", name: "Git Flow", description: "分支工作流", source: "builtin" }),
    marketplaceSkill({
      id: "m2",
      name: "Docker Tips",
      description: "容器部署技巧",
      source: "community",
      installedSkillId: "local-2",
    }),
    marketplaceSkill({ id: "m3", name: "Wiki Helper", description: "写 wiki", source: "builtin" }),
  ];

  it("空关键词且不筛来源时返回全部", () => {
    expect(filterMarketplaceSkills(entries, "", null)).toHaveLength(3);
    expect(filterMarketplaceSkills(entries, "   ", null)).toHaveLength(3);
  });

  it("按来源过滤,缺失来源的条目只归入 null", () => {
    expect(filterMarketplaceSkills(entries, "", "builtin").map((s) => s.id)).toEqual(["m1", "m3"]);
    expect(filterMarketplaceSkills(entries, "", "community").map((s) => s.id)).toEqual(["m2"]);
    expect(filterMarketplaceSkills(entries, "", "missing")).toEqual([]);
  });

  it("关键词匹配名称或描述,大小写不敏感,已安装条目同样参与", () => {
    expect(filterMarketplaceSkills(entries, "docker", null).map((s) => s.id)).toEqual(["m2"]);
    expect(filterMarketplaceSkills(entries, "WIKI", null).map((s) => s.id)).toEqual(["m3"]);
    expect(filterMarketplaceSkills(entries, "分支", null).map((s) => s.id)).toEqual(["m1"]);
    expect(filterMarketplaceSkills(entries, "不存在", null)).toEqual([]);
  });

  it("来源与关键词同时生效", () => {
    expect(filterMarketplaceSkills(entries, "tips", "community").map((s) => s.id)).toEqual(["m2"]);
    expect(filterMarketplaceSkills(entries, "tips", "builtin")).toEqual([]);
  });
});

describe("markMarketplaceInstalled", () => {
  const entries = [
    marketplaceSkill({ id: "m1", name: "Git Flow", source: "builtin" }),
    marketplaceSkill({ id: "m2", name: "Docker Tips", source: "community" }),
  ];

  it("把对应条目标记为已安装并写入本地技能 id", () => {
    const next = markMarketplaceInstalled(entries, "m1", "local-1");
    expect(next.find((s) => s.id === "m1")?.installedSkillId).toBe("local-1");
    expect(next.find((s) => s.id === "m2")?.installedSkillId).toBeUndefined();
  });

  it("纯函数:不改原数组,返回新数组", () => {
    markMarketplaceInstalled(entries, "m2", "local-2");
    expect(entries.find((s) => s.id === "m2")?.installedSkillId).toBeUndefined();
    const next = markMarketplaceInstalled(entries, "m2", "local-2");
    expect(next).not.toBe(entries);
  });

  it("覆盖既有标记,id 不存在时内容不变", () => {
    const marked = markMarketplaceInstalled(entries, "m2", "local-old");
    const next = markMarketplaceInstalled(marked, "m2", "local-new");
    expect(next.find((s) => s.id === "m2")?.installedSkillId).toBe("local-new");
    expect(markMarketplaceInstalled(entries, "nope", "x")).toEqual(entries);
  });
});

describe("mergeMarketplaceSources", () => {
  it("按 id 去重,保留 base 顺序,新来源追加尾部", () => {
    const base = [
      { id: "builtin", name: "内置目录" },
      { id: "community", name: "社区源", url: "https://example.com" },
    ];
    const incoming = [
      { id: "community", name: "社区源" },
      { id: "mirror", name: "镜像源" },
    ];
    expect(mergeMarketplaceSources(base, incoming)).toEqual([
      { id: "builtin", name: "内置目录" },
      { id: "community", name: "社区源", url: "https://example.com" },
      { id: "mirror", name: "镜像源" },
    ]);
  });

  it("已知识源用新数据更新,新数据缺的字段保留 base 旧值", () => {
    const base = [{ id: "a", name: "旧名", url: "https://old" }];
    expect(mergeMarketplaceSources(base, [{ id: "a", name: "新名" }])).toEqual([
      { id: "a", name: "新名", url: "https://old" },
    ]);
  });

  it("空入参安全", () => {
    expect(mergeMarketplaceSources([], [])).toEqual([]);
    expect(mergeMarketplaceSources([{ id: "a", name: "A" }], [])).toEqual([{ id: "a", name: "A" }]);
    expect(mergeMarketplaceSources([], [{ id: "b", name: "B" }])).toEqual([{ id: "b", name: "B" }]);
  });
});
