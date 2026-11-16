// 「常用命令」标记:一次拉取全部(单表小数据量),详情页各组件按 project_id 过滤,
// 托盘弹窗按项目分组展示;主窗口与托盘弹窗是独立 Pinia 实例,靠 projects://pins-changed 事件同步
import { ref } from "vue";
import { defineStore } from "pinia";
import { cmd } from "@/lib/tauri";
import type { PinnedCommand, PinKind } from "@/types";

export interface PinInput {
  kind: PinKind;
  targetKey: string;
  label: string;
  command: string;
  cwd?: string;
}

export const usePinsStore = defineStore("pins", () => {
  const pins = ref<PinnedCommand[]>([]),
    loaded = ref(false);
  let loadSequence = 0;

  async function fetchPins() {
    const sequence = ++loadSequence,
      fresh = await cmd<PinnedCommand[]>("list_pinned_commands", {});
    if (sequence !== loadSequence) {
      return;
    }
    pins.value = fresh;
    loaded.value = true;
  }

  /** 首次使用前确保已加载(详情页组件挂载时调用) */
  async function ensureLoaded() {
    if (!loaded.value) {
      await fetchPins();
    }
  }

  function pinsOf(projectId: number): PinnedCommand[] {
    return pins.value.filter((p) => p.project_id === projectId);
  }

  function isPinned(projectId: number, kind: PinKind, targetKey: string): boolean {
    return pins.value.some(
      (p) => p.project_id === projectId && p.kind === kind && p.target_key === targetKey,
    );
  }

  /** 设置/取消标记:成功后就地增删数组元素,无需整表重拉 */
  async function setPinned(projectId: number, input: PinInput, pinned: boolean) {
    await cmd("set_pinned_command", {
      projectId,
      kind: input.kind,
      targetKey: input.targetKey,
      pinned,
      label: input.label,
      command: input.command,
      cwd: input.cwd ?? null,
    });
    if (pinned) {
      // 后端 INSERT ... ON CONFLICT DO UPDATE 会刷新快照;本地直接重拉该项最稳妥,但单表很小,整表刷新即可
      await fetchPins();
    } else {
      pins.value = pins.value.filter(
        (p) =>
          !(
            p.project_id === projectId &&
            p.kind === input.kind &&
            p.target_key === input.targetKey
          ),
      );
    }
  }

  return { pins, loaded, fetchPins, ensureLoaded, pinsOf, isPinned, setPinned };
});
