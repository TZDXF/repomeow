import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// 回归守卫(node 环境无法挂载整个图谱页):
// 1) 分支列表必须在日志请求之前独立落库。绑在同一个 Promise.all 里时,日志的坏修订
//    (如已被 prune 的远程分支)会让新分支列表一起失败,侧栏继续展示已删除分支与旧图谱。
// 2) 日志范围必须经过 prepareGraphLoad/resolveFilterTarget 校正,失效筛选回退而非硬用坏修订。
const source = readFileSync(new URL("./GitGraph.vue", import.meta.url), "utf8");

describe("提交图谱加载顺序与筛选校正", () => {
  it("分支列表先于图谱日志请求并独立落库", () => {
    const prepareAt = source.indexOf("prepareGraphLoad("),
      logAt = source.indexOf('cmd("git_graph_log"');
    expect(prepareAt).toBeGreaterThan(-1);
    expect(logAt).toBeGreaterThan(prepareAt);
    expect(source).not.toMatch(/Promise\.all\(\[\s*cmd<GitBranches>/);
  });

  it("日志前校验筛选目标,失效分支回退当前检出分支", () => {
    expect(source).toMatch(/if \(correction\.dropped\)/);
    expect(source).toMatch(/selectedBranch\.value = correction\.branch;/);
  });
});
