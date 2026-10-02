import { describe, expect, it } from "vitest";
import type {
  ProjectAiTarget,
  ProjectResourceSnapshot,
  ResourceChoice,
  ResourceDeployment,
} from "@/lib/project-ai-resources";
import type { ProjectAiAssets, ProjectSkill } from "@/types";
import {
  buildListedResources,
  buildUnmanagedItems,
  localImportTargetOf,
  resourceSelectable,
  unmanagedOwnerSource,
  unmanagedOwners,
  visibleResourceAgents,
  type UnmanagedItem,
} from "./use-project-resource-data";
import { repairableRecordsOf, repairBusyKey } from "./use-resource-repair";

function choice(partial: Partial<ResourceChoice> & Pick<ResourceChoice, "id">): ResourceChoice {
  return { name: partial.id, description: "", groupIds: [], supportedAgents: [], ...partial };
}

function deployment(
  partial: Partial<ResourceDeployment> & Pick<ResourceDeployment, "resourceId" | "agentId">,
): ResourceDeployment {
  return {
    kind: "skills",
    name: partial.resourceId,
    path: "",
    fingerprint: "",
    status: "configured",
    ...partial,
  };
}

function snapshot(partial: Partial<ProjectResourceSnapshot>): ProjectResourceSnapshot {
  return {
    revision: "1",
    groups: [],
    resources: [],
    deployments: [],
    shortlist: [],
    sourceError: null,
    ...partial,
  };
}

function target(partial: Partial<ProjectAiTarget> & Pick<ProjectAiTarget, "id">): ProjectAiTarget {
  return { name: partial.id, skillPath: `.${partial.id}/skills`, mcpPath: "", ...partial };
}

function skill(dir: string, name: string, description = ""): ProjectSkill {
  return { dir, name, description, descriptionTokenCount: 0, tokenCount: 0 };
}

describe("buildListedResources", () => {
  it("列表 = shortlist ∪ 已部署;来源缺失时回退部署记录里的名字", () => {
    const data = snapshot({
      shortlist: ["a", "b"],
      resources: [choice({ id: "a", name: "资源A" })],
      deployments: [deployment({ resourceId: "c", agentId: "claude", name: "部署C" })],
    });
    const listed = buildListedResources(data);
    expect(listed.map((r) => r.id)).toEqual(["a", "b", "c"]);
    expect(listed[0].name).toBe("资源A");
    // b、c 在 resources 里不存在:b 回退 id,c 回退部署记录名
    expect(listed[1]).toMatchObject({ id: "b", name: "b" });
    expect(listed[2]).toMatchObject({ id: "c", name: "部署C" });
  });

  it("空快照与 null 都返回空列表", () => {
    expect(buildListedResources(null)).toEqual([]);
    expect(buildListedResources(snapshot({}))).toEqual([]);
  });
});

describe("buildUnmanagedItems", () => {
  it("mcp:按文件展开服务器,跳过已部署(path+name)条目", () => {
    const assets: ProjectAiAssets = {
      files: [],
      skills: [],
      mcp: [
        {
          path: ".mcp.json",
          dialect: "claude",
          agents: ["claude"],
          servers: [
            { name: "s1", config: {} },
            { name: "s2", config: {} },
          ],
        },
      ],
    };
    const deployments = [
      deployment({
        resourceId: "x",
        agentId: "claude",
        kind: "mcp",
        path: ".mcp.json",
        name: "s1",
      }),
    ];
    const items = buildUnmanagedItems("mcp", assets, deployments, []);
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({
      key: "mcp:.mcp.json:s2",
      name: "s2",
      source: ".mcp.json",
      sources: [".mcp.json"],
    });
  });

  it("skills:跳过已部署目录,同名跨 Agent 目录合并为一行(主来源取排序首项)", () => {
    const assets: ProjectAiAssets = {
      files: [],
      mcp: [],
      skills: [
        skill(".claude/skills/foo", "foo", "来自claude"),
        skill(".agents/skills/foo", "foo"),
        skill(".agents/skills/bar", "bar"),
      ],
    };
    const deployments = [
      deployment({ resourceId: "x", agentId: "agents", path: ".agents/skills/bar" }),
    ];
    const targets = [target({ id: "claude" }), target({ id: "agents" })];
    const items = buildUnmanagedItems("skills", assets, deployments, targets);
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({
      key: "skill:foo",
      name: "foo",
      description: "来自claude",
      path: ".claude/skills/foo/SKILL.md",
      source: ".claude/skills/foo",
      sources: [".claude/skills/foo", ".agents/skills/foo"],
    });
  });
});

describe("非托管归属", () => {
  const targets = [
    target({ id: "claude", skillPath: ".claude/skills", mcpPath: ".mcp.json" }),
    target({ id: "codex", skillPath: ".codex/skills", mcpPath: ".codex/config.toml" }),
  ];
  const skillItem: UnmanagedItem = {
    key: "skill:foo",
    name: "foo",
    description: "",
    path: ".claude/skills/foo/SKILL.md",
    source: ".claude/skills/foo",
    sources: [".claude/skills/foo", ".codex/skills/foo"],
  };
  const mcpItem: UnmanagedItem = {
    key: "mcp:.mcp.json:s1",
    name: "s1",
    description: "",
    path: ".mcp.json",
    source: ".mcp.json",
    sources: [".mcp.json"],
  };

  it("skills 同名合并行可有多个归属(每个目录前缀各算一个)", () => {
    expect(unmanagedOwners(skillItem, targets, "skills")).toEqual(["claude", "codex"]);
    expect(unmanagedOwnerSource(skillItem, "codex", targets, "skills")).toBe(".codex/skills/foo");
    expect(unmanagedOwnerSource(skillItem, "unknown", targets, "skills")).toBe("");
  });

  it("mcp 按配置文件路径判定唯一归属", () => {
    expect(unmanagedOwners(mcpItem, targets, "mcp")).toEqual(["claude"]);
    expect(unmanagedOwnerSource(mcpItem, "claude", targets, "mcp")).toBe(".mcp.json");
    expect(unmanagedOwnerSource(mcpItem, "codex", targets, "mcp")).toBe("");
  });
});

describe("resourceSelectable / visibleResourceAgents", () => {
  const targets = [
    target({ id: "claude", mcpPath: ".mcp.json" }),
    target({ id: "codex", mcpPath: "" }),
  ];

  it("受支持或已部署(允许解除配置)的 Agent 可切换", () => {
    const resource = choice({ id: "a", supportedAgents: ["claude"] });
    expect(resourceSelectable(resource, targets[0], [])).toBe(true);
    expect(resourceSelectable(resource, targets[1], [])).toBe(false);
    // 不支持但已部署:允许解除
    expect(
      resourceSelectable(resource, targets[1], [deployment({ resourceId: "a", agentId: "codex" })]),
    ).toBe(true);
  });

  it("隐藏 Agent 不展示,但已部署该资源的隐藏 Agent 仍可见;mcp 过滤未接入目标", () => {
    const records = [deployment({ resourceId: "a", agentId: "codex" })];
    expect(visibleResourceAgents(targets, "skills", [], ["claude"]).map((a) => a.id)).toEqual([
      "codex",
    ]);
    expect(visibleResourceAgents(targets, "skills", records, ["codex"]).map((a) => a.id)).toEqual([
      "claude",
      "codex",
    ]);
    // mcp:codex 无 mcpPath 且无部署记录 → 不可见
    expect(visibleResourceAgents(targets, "mcp", [], []).map((a) => a.id)).toEqual(["claude"]);
    expect(visibleResourceAgents(targets, "mcp", records, []).map((a) => a.id)).toEqual([
      "claude",
      "codex",
    ]);
  });
});

describe("localImportTargetOf", () => {
  it("非 local 资源返回 null", () => {
    expect(localImportTargetOf(choice({ id: "lib:foo" }), [], "skills")).toBeNull();
  });

  it("skills:从 local id 解析主来源目录", () => {
    expect(
      localImportTargetOf(choice({ id: "local:skills:.claude/skills/foo" }), [], "skills"),
    ).toEqual({ source: ".claude/skills/foo" });
    expect(localImportTargetOf(choice({ id: "local:skills:" }), [], "skills")).toBeNull();
  });

  it("mcp:优先部署记录,无记录时从 id 的 path#name 解析", () => {
    const resource = choice({ id: "local:mcp:.mcp.json#s1" });
    expect(
      localImportTargetOf(
        resource,
        [
          deployment({
            resourceId: resource.id,
            agentId: "claude",
            kind: "mcp",
            path: ".mcp.json",
            name: "s1",
          }),
        ],
        "mcp",
      ),
    ).toEqual({ source: ".mcp.json", name: "s1" });
    expect(localImportTargetOf(resource, [], "mcp")).toEqual({ source: ".mcp.json", name: "s1" });
    expect(localImportTargetOf(choice({ id: "local:mcp:.mcp.json" }), [], "mcp")).toBeNull();
  });
});

describe("repairableRecordsOf / repairBusyKey", () => {
  it("仅保留有修复出路的异常状态", () => {
    const records = (
      [
        "configured",
        "update",
        "modified",
        "missing",
        "sourceMissing",
        "sourceUnavailable",
        "conflict",
      ] as const
    ).map((status) => deployment({ resourceId: "a", agentId: "claude", status }));
    expect(repairableRecordsOf(records).map((r) => r.status)).toEqual([
      "update",
      "modified",
      "missing",
      "conflict",
    ]);
  });

  it("忙碌键格式 resourceId:agentId:action", () => {
    const record = deployment({ resourceId: "a", agentId: "claude" });
    expect(repairBusyKey(record, "update")).toBe("a:claude:update");
    expect(repairBusyKey(record, "reapply")).toBe("a:claude:reapply");
    expect(repairBusyKey(record, "detach")).toBe("a:claude:detach");
  });
});
