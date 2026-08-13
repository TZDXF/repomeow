import { defineStore } from "pinia";
import { cmd } from "@/lib/tauri";
import type { ProjectOverview } from "@/types";

/**
 * 详情页首屏聚合数据(隐藏项 + 自定义命令)共享 store。
 * PackageScripts / DockerCompose / CustomCommands 三个卡片挂载时各要其中一部分,
 * 这里合并为一次 get_project_overview IPC(后端单次 DB 锁内完成两条查询)并做进行中去重。
 * 不做结果缓存:隐藏项/自定义命令由各卡片本地维护增删,挂载时始终拉最新,仅对同时发起的请求去重。
 */
export const useProjectOverviewStore = defineStore("project-overview", () => {
  /** 进行中的请求,key 为项目 id:三个卡片同时挂载只发一次 IPC */
  const inflight = new Map<number, Promise<ProjectOverview>>();

  /** 拉取(或复用进行中的)聚合数据;失败返回空数据(卡片按无数据显示) */
  function refresh(projectId: number): Promise<ProjectOverview> {
    const pending = inflight.get(projectId);
    if (pending) return pending;
    const p = cmd<ProjectOverview>("get_project_overview", { projectId })
      .catch(() => ({ hidden_items: [], custom_commands: [] }) as ProjectOverview)
      .finally(() => {
        inflight.delete(projectId);
      });
    inflight.set(projectId, p);
    return p;
  }

  return { refresh };
});
