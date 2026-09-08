import { describe, expect, it, vi } from "vitest";
import {
  assignChanges,
  resourceTree,
  selectionState,
  toggleResources,
  type ResourceChoice,
  type ResourceDeployment,
} from "./project-ai-resources";

vi.mock("@/lib/tauri", () => ({ cmd: vi.fn() }));

const resources: ResourceChoice[] = [
  {
    id: "shared",
    name: "Shared",
    description: "",
    groupIds: ["a", "b"],
    supportedAgents: ["claude"],
  },
  { id: "solo", name: "Solo", description: "", groupIds: ["a"], supportedAgents: ["claude"] },
  { id: "loose", name: "Loose", description: "", groupIds: ["deleted"], supportedAgents: [] },
];

describe("project resource selection", () => {
  it("shows multi-group skills in each group and orphans under ungrouped", () => {
    const tree = resourceTree(
      [
        { id: "a", name: "A", description: "" },
        { id: "b", name: "B", description: "" },
      ],
      resources,
      "Ungrouped",
    );
    expect(tree.map((g) => g.resources.map((r) => r.id))).toEqual([
      ["shared", "solo"],
      ["shared"],
      ["loose"],
    ]);
  });
  it("treats marketplace source as an additional group alongside user groups", () => {
    const market: ResourceChoice[] = [
      {
        id: "m1",
        name: "M1",
        description: "",
        groupIds: ["a"],
        supportedAgents: [],
        source: "owner/repo",
      },
      {
        id: "m2",
        name: "M2",
        description: "",
        groupIds: [],
        supportedAgents: [],
        source: "owner/repo",
      },
      {
        id: "m3",
        name: "M3",
        description: "",
        groupIds: [],
        supportedAgents: [],
        source: "other/lib",
      },
      { id: "m4", name: "M4", description: "", groupIds: [], supportedAgents: [] },
    ];
    const tree = resourceTree([{ id: "a", name: "A", description: "" }], market, "Ungrouped");
    expect(tree.map((g) => [g.name, g.resources.map((r) => r.id)])).toEqual([
      ["A", ["m1"]],
      ["other/lib", ["m3"]],
      ["owner/repo", ["m1", "m2"]],
      ["Ungrouped", ["m4"]],
    ]);
  });
  it("uses one set across groups and toggles only the passed IDs", () => {
    const initial = new Set(["loose"]);
    const selected = toggleResources(initial, ["shared", "solo", "shared"], true);
    expect(initial).toEqual(new Set(["loose"]));
    expect(selected.size).toBe(3);
    expect(selectionState(["shared"], selected)).toBe("all");
    const next = toggleResources(selected, ["shared"], false);
    expect(selectionState(["shared", "solo"], next)).toBe("some");
    expect(next.has("loose")).toBe(true);
  });
  it("does not mark empty groups selected and ignores duplicate IDs", () => {
    expect(selectionState([], new Set())).toBe("none");
    expect(selectionState(["shared", "shared"], new Set(["shared"]))).toBe("all");
  });
  it("counts agent assignment changes for one resource, including updates", () => {
    const entries = [
      { agentId: "claude", resourceId: "shared", status: "update" },
      { agentId: "cursor", resourceId: "shared", status: "configured" },
      { agentId: "cursor", resourceId: "solo", status: "configured" },
    ] as ResourceDeployment[];
    expect(assignChanges(entries, "shared", new Set(["claude", "gemini"]))).toEqual({
      add: 1,
      remove: 1,
      update: 1,
    });
  });
});

it("sends the snapshot revision with add/assign/remove commands", async () => {
  const { cmd } = await import("@/lib/tauri");
  const {
    addProjectResources,
    assignProjectResource,
    importProjectResource,
    removeProjectResource,
  } = await import("./project-ai-resources");
  await addProjectResources({
    path: "D:/demo",
    kind: "skills",
    resourceIds: ["shared"],
    expectedRevision: "revision-1",
  });
  expect(cmd).toHaveBeenCalledWith("project_ai_add", {
    path: "D:/demo",
    kind: "skills",
    resourceIds: ["shared"],
    expectedRevision: "revision-1",
  });
  await assignProjectResource({
    path: "D:/demo",
    kind: "skills",
    resourceId: "shared",
    agentIds: ["claude"],
    expectedRevision: "revision-1",
  });
  expect(cmd).toHaveBeenCalledWith("project_ai_assign", {
    path: "D:/demo",
    kind: "skills",
    resourceId: "shared",
    agentIds: ["claude"],
    expectedRevision: "revision-1",
  });
  await removeProjectResource({
    path: "D:/demo",
    kind: "skills",
    resourceId: "shared",
    expectedRevision: "revision-1",
  });
  expect(cmd).toHaveBeenCalledWith("project_ai_remove", {
    path: "D:/demo",
    kind: "skills",
    resourceId: "shared",
    expectedRevision: "revision-1",
  });
  await importProjectResource({
    path: "D:/demo",
    kind: "mcp",
    source: ".mcp.json",
    name: "context",
    expectedRevision: "revision-1",
  });
  expect(cmd).toHaveBeenCalledWith("project_ai_import", {
    path: "D:/demo",
    kind: "mcp",
    source: ".mcp.json",
    name: "context",
    expectedRevision: "revision-1",
  });
});
