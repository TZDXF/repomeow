import { computed, ref } from "vue";
import { defineStore } from "pinia";
import { toast } from "vue-sonner";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { i18n } from "@/i18n";
import { cmd, onListen } from "@/lib/tauri";
import { javaMajorVersion } from "@/lib/jdk";
import { useSettingsStore } from "@/stores/settings";
import type { JdkCandidate, JdkVendor } from "@/types";

/** 安装源选项(厂商名为专有名词,不走 i18n) */
export const JDK_VENDORS: { value: JdkVendor; label: string }[] = [
  { value: "adoptium", label: "Adoptium (Temurin)" },
  { value: "zulu", label: "Azul Zulu" },
];

export function jdkVendorLabel(vendor: JdkVendor): string {
  return JDK_VENDORS.find((v) => v.value === vendor)?.label ?? vendor;
}

/** 后端 jdk://install-progress 事件载荷(下载字节量/解压阶段) */
export interface JdkInstallProgress {
  stage: string;
  received: number;
  total: number;
}

/**
 * JDK 在线安装的全局状态。install_jdk 一次要下载数十至两百 MB,状态放 store
 * 而非 JavaSection 组件:关闭对话框或离开设置页安装继续(后端命令不随组件
 * 卸载取消),完成/失败经全局 toast 通知,设置页「在线安装」按钮实时显示进度,
 * 重新打开对话框可回看。同一时刻只允许一个安装任务(重复 start 直接忽略)。
 */
export const useJdkInstallStore = defineStore("jdk-install", () => {
  const installing = ref(false);
  const vendor = ref<JdkVendor>("adoptium");
  const major = ref(0);
  const progress = ref<JdkInstallProgress | null>(null);
  let unlisten: UnlistenFn | undefined;

  /** 下载进度百分比;解压阶段/未知总量返回 null(转不确定态样式) */
  const downloadPct = computed(() => {
    const p = progress.value;
    if (!p || p.stage !== "download" || !p.total) return null;
    return Math.min(100, Math.round((p.received / p.total) * 100));
  });

  /** 启动一次在线安装(进行中时忽略重复调用);结果经 toast 通知,不向上抛错 */
  async function start(nextVendor: JdkVendor, nextMajor: number) {
    if (installing.value) return;
    installing.value = true;
    vendor.value = nextVendor;
    major.value = nextMajor;
    progress.value = { stage: "download", received: 0, total: 0 };
    unlisten?.();
    unlisten = await onListen<JdkInstallProgress>("jdk://install-progress", (p) => {
      progress.value = p;
    });
    const t = i18n.global.t;
    const settings = useSettingsStore();
    try {
      const installed = await cmd<JdkCandidate>("install_jdk", {
        vendor: nextVendor,
        major: nextMajor,
      });
      // Windows 路径大小写不敏感去重
      if (settings.jdkList.some((j) => j.path.toLowerCase() === installed.path.toLowerCase())) {
        toast.info(t("settings.devEnv.alreadyInList"));
      } else {
        await settings.saveJdk({
          id: crypto.randomUUID(),
          name: `Java ${javaMajorVersion(installed.version)}`,
          path: installed.path,
        });
        toast.success(
          t("settings.devEnv.installed", {
            vendor: jdkVendorLabel(nextVendor),
            version: installed.version,
          }),
        );
      }
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      installing.value = false;
      progress.value = null;
      unlisten?.();
      unlisten = undefined;
    }
  }

  return { installing, vendor, major, progress, downloadPct, start };
});
