import { defineStore } from "pinia";
import { cmd } from "@/lib/tauri";
import type { HiddenItem } from "@/types";

/**
 * 项目隐藏项(packageFile / packageScript / composeFile)列表共享 store。
 * PackageScripts 与 DockerCompose 卡片挂载时各要一份完整列表,这里合并为一次 IPC 并做进行中去重。
 * 不做结果缓存:隐藏项由各卡片本地维护增删,挂载时始终拉最新,仅对同时发起的请求去重。
 */
export const useHiddenItemsStore = defineStore("hidden-items", () => {
  /** 进行中的请求,key 为项目 id:两个卡片同时挂载只发一次 IPC */
  const inflight = new Map<number, Promise<HiddenItem[]>>();

  /** 拉取(或复用进行中的)隐藏项列表;失败返回空数组(卡片按无隐藏处理) */
  function refresh(projectId: number): Promise<HiddenItem[]> {
    const pending = inflight.get(projectId);
    if (pending) return pending;
    const p = cmd<HiddenItem[]>("list_hidden_items", { projectId })
      .catch(() => [] as HiddenItem[])
      .finally(() => {
        inflight.delete(projectId);
      });
    inflight.set(projectId, p);
    return p;
  }

  return { refresh };
});
