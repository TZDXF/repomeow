import { ref } from "vue";
import { defineStore } from "pinia";
import { cmd } from "@/lib/tauri";
import type { Project, ProjectAssets } from "@/types";

/**
 * 项目资产(package scripts + compose 文件)扫描结果共享 store。
 * PackageScripts 与 DockerCompose 卡片挂载时都要这份数据,这里合并为一次 IPC
 * (后端 scan_project_assets 单次目录遍历 + walk 缓存),并进行中去重。
 *
 * stale-while-revalidate:扫描结果按「项目 id + 路径」保留,再次进入时先拿旧数据
 * 渲染、后台刷新完成后自动替换——大项目全量目录遍历要数百毫秒,不等结果才不卡首屏。
 * key 含 path:worktree 切换(id 同 path 不同)各自保留各自的旧数据,互不覆盖。
 */
export const useProjectAssetsStore = defineStore("project-assets", () => {
  /** 按「项目 id + 路径」存放最近一次扫描结果,作为下次进入时的 stale 数据 */
  const byProject = ref<Record<string, ProjectAssets>>({});
  /** 进行中的请求,key 同 byProject:两个卡片同时挂载只发一次 IPC */
  const inflight = new Map<string, Promise<void>>();
  /** 缓存条目上限:卡片同时只展示一个项目,积攒的多项目旧数据定期修剪 */
  const MAX_ENTRIES = 16;

  const keyOf = (project: Project) => `${project.id}\n${project.path}`;

  function assetsOf(project: Project): ProjectAssets | undefined {
    return byProject.value[keyOf(project)];
  }

  /** 拉取(或复用进行中的)扫描结果;失败保留旧数据(没有则写空),不向上抛错 */
  function refresh(project: Project): Promise<void> {
    const key = keyOf(project);
    const pending = inflight.get(key);
    if (pending) return pending;
    const p = (async () => {
      try {
        const assets = await cmd<ProjectAssets>("scan_project_assets", {
          path: project.path,
        });
        const map = { ...byProject.value };
        const keys = Object.keys(map);
        if (keys.length >= MAX_ENTRIES && !(key in map)) delete map[keys[0]];
        map[key] = assets;
        byProject.value = map;
      } catch {
        // 刷新失败不冲掉可用的旧数据;仅在没有任何数据时写空结果(卡片按无数据显示)
        if (!byProject.value[key]) {
          byProject.value = {
            ...byProject.value,
            [key]: { package_scripts: [], compose_files: [] },
          };
        }
      }
    })().finally(() => {
      inflight.delete(key);
    });
    inflight.set(key, p);
    return p;
  }

  return { assetsOf, refresh };
});
