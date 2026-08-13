import { ref } from "vue";
import { defineStore } from "pinia";
import { cmd } from "@/lib/tauri";
import type { HiddenKind, ProjectOverview } from "@/types";

/**
 * 详情页首屏聚合数据(隐藏项 + 自定义命令)共享 store。
 * PackageScripts / DockerCompose / CustomCommands 三个卡片挂载时各要其中一部分,
 * 这里合并为一次 get_project_overview IPC(后端单次 DB 锁内完成两条查询)并做进行中去重。
 *
 * stale-while-revalidate:最近一次结果按项目 id 保留(cached),卡片挂载时同步取出
 * 做首屏渲染——否则已隐藏的分组/compose 文件会在刷新完成前短暂可见(闪现再消失)。
 * 卡片内隐藏/恢复成功后经 setHiddenLocal 回写缓存,保证下次进入的 stale 首屏就是最新状态;
 * 自定义命令由 CustomCommands 卡片本地维护增删、挂载时始终等 refresh 最新结果,不读 cached。
 *
 * 缓存是 Map 实现的真 LRU(同 project-assets store)。
 */
export const useProjectOverviewStore = defineStore("project-overview", () => {
  /** 进行中的请求,key 为项目 id:三个卡片同时挂载只发一次 IPC */
  const inflight = new Map<number, Promise<ProjectOverview>>();
  /** 最近一次聚合结果,按项目 id 存放,作为下次进入时的 stale 数据 */
  const byId = ref(new Map<number, ProjectOverview>());
  /** 缓存条目上限,避免多项目堆积(单条仅隐藏项 + 自定义命令,体积很小) */
  const MAX_ENTRIES = 32;

  /** 写入并维持 LRU 上限(同 key 先删后插刷新热度) */
  function setCapped(projectId: number, data: ProjectOverview) {
    const map = byId.value;
    map.delete(projectId);
    map.set(projectId, data);
    const oldest = map.keys().next().value;
    if (map.size > MAX_ENTRIES && oldest !== undefined) map.delete(oldest);
  }

  /** 同步读取缓存的聚合结果(stale 首屏用;无缓存返回 undefined) */
  function cached(projectId: number): ProjectOverview | undefined {
    const hit = byId.value.get(projectId);
    if (hit) setCapped(projectId, hit); // 读取刷新热度
    return hit;
  }

  /** 拉取(或复用进行中的)聚合数据;失败回退缓存(没有则空数据),不向上抛错 */
  function refresh(projectId: number): Promise<ProjectOverview> {
    const pending = inflight.get(projectId);
    if (pending) return pending;
    const p = cmd<ProjectOverview>("get_project_overview", { projectId })
      .then((data) => {
        setCapped(projectId, data);
        return data;
      })
      .catch(
        () =>
          byId.value.get(projectId) ??
          ({ hidden_items: [], custom_commands: [] } as ProjectOverview),
      )
      .finally(() => {
        inflight.delete(projectId);
      });
    inflight.set(projectId, p);
    return p;
  }

  /** 卡片内隐藏/恢复隐藏成功后同步缓存,让下次进入的 stale 首屏与本次操作一致 */
  function setHiddenLocal(projectId: number, kind: HiddenKind, targetKey: string, hidden: boolean) {
    const cur = byId.value.get(projectId);
    if (!cur) return;
    const exists = cur.hidden_items.some((i) => i.kind === kind && i.targetKey === targetKey);
    if (hidden === exists) return;
    const items = hidden
      ? [...cur.hidden_items, { kind, targetKey }]
      : cur.hidden_items.filter((i) => !(i.kind === kind && i.targetKey === targetKey));
    setCapped(projectId, { ...cur, hidden_items: items });
  }

  return { cached, refresh, setHiddenLocal };
});
