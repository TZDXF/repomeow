import { describe, expect, it } from "vitest";
import type { Project, Tag } from "@/types";
import { filterReportProjects, selectVisibleProjectIds } from "./project-selector";

const tagA: Tag = { id: 1, name: "A", color: "#f00" };
const tagB: Tag = { id: 2, name: "B", color: "#0f0" };

function makeProject(id: number, tags: Tag[], name = `P${id}`): Project {
  return { id, name, path: `/repos/${name}`, tags } as Project;
}

const p1 = makeProject(1, [tagA]);
const p2 = makeProject(2, [tagB]);
const p3 = makeProject(3, [tagA, tagB]);
const p4 = makeProject(4, []);
const projects = [p1, p2, p3, p4];

describe("filterReportProjects", () => {
  it("any 模式多标签取并集,排除不匹配项目且同时命中多标签的项目不重复", () => {
    expect(
      filterReportProjects(projects, "", { tagIds: [tagA.id, tagB.id], tagFilterMode: "any" }),
    ).toEqual([p1, p2, p3]);
  });

  it("any 模式单标签保留既有匹配行为", () => {
    expect(filterReportProjects(projects, "", { tagIds: [tagA.id], tagFilterMode: "any" })).toEqual(
      [p1, p3],
    );
  });

  it("未选择标签时保留全部项目", () => {
    expect(filterReportProjects(projects, "", { tagIds: [], tagFilterMode: "any" })).toEqual(
      projects,
    );
  });

  it("标签并集与关键词筛选继续同时生效", () => {
    expect(
      filterReportProjects(projects, "p2", { tagIds: [tagA.id, tagB.id], tagFilterMode: "any" }),
    ).toEqual([p2]);
    expect(
      filterReportProjects(projects, "missing", {
        tagIds: [tagA.id, tagB.id],
        tagFilterMode: "any",
      }),
    ).toEqual([]);
  });

  it("默认 all 模式保持其他入口的多标签交集行为", () => {
    expect(filterReportProjects(projects, "", { tagIds: [tagA.id, tagB.id] })).toEqual([p3]);
  });
});

describe("selectVisibleProjectIds", () => {
  it("全选并集结果得到实际生成使用的去重项目 id 集合", () => {
    const visible = filterReportProjects(projects, "", {
      tagIds: [tagA.id, tagB.id],
      tagFilterMode: "any",
    });
    expect(selectVisibleProjectIds([], visible)).toEqual([1, 2, 3]);
  });

  it("保留已被其他条件选中的隐藏项目且不重复添加", () => {
    expect(selectVisibleProjectIds([4, 3], [p1, p2, p3])).toEqual([4, 3, 1, 2]);
  });
});
