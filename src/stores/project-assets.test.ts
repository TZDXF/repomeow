import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { effectScope, nextTick, ref } from "vue";
import { cmd } from "@/lib/tauri";
import { watchProjectPageReactivation } from "@/composables/useProjectPageState";
import { useProjectAssetsStore } from "@/stores/project-assets";
import type { Project, ProjectAssets } from "@/types";

vi.mock("@/lib/tauri", () => ({ cmd: vi.fn() }));
const project = { id: 1, path: "C:/repos/project" } as Project;
function assets(name: string): ProjectAssets {
  return {
    package_scripts: [{ dir: ".", package_name: "project", scripts: [{ name, command: name }] }],
    compose_files: [],
    java_builds: [],
  };
}

describe("缓存项目卡片的数据校验", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    setActivePinia(createPinia());
  });

  it("外部新增脚本后切回页面会重扫,同页多个卡片共享一次请求", async () => {
    vi.mocked(cmd).mockResolvedValueOnce(assets("old")).mockResolvedValueOnce(assets("added"));
    const store = useProjectAssetsStore();
    const active = ref(true);
    const draft = ref("未提交的命令草稿");
    const scope = effectScope();
    scope.run(() => {
      // PackageScripts / DockerCompose / SpringBootCard 共用重新激活钩子。
      for (let i = 0; i < 3; i++) {
        watchProjectPageReactivation(active, () => store.refresh(project));
      }
    });
    await store.refresh(project);
    expect(store.assetsOf(project)?.package_scripts[0].scripts[0].name).toBe("old");
    expect(cmd).toHaveBeenCalledTimes(1);
    active.value = false;
    await nextTick();
    expect(cmd).toHaveBeenCalledTimes(1);
    active.value = true;
    await nextTick();
    await vi.waitFor(() => {
      expect(store.assetsOf(project)?.package_scripts[0].scripts[0].name).toBe("added");
    });
    expect(cmd).toHaveBeenCalledTimes(2);
    expect(draft.value).toBe("未提交的命令草稿");
    scope.stop();
  });

  it("重新校验失败时保留已加载的脚本", async () => {
    vi.mocked(cmd)
      .mockResolvedValueOnce(assets("existing"))
      .mockRejectedValueOnce(new Error("scan failed"));
    const store = useProjectAssetsStore();
    await store.refresh(project);
    await store.refresh(project);
    expect(store.assetsOf(project)?.package_scripts[0].scripts[0].name).toBe("existing");
  });
});
