import { computed, shallowRef, ref } from "vue";
import { defineStore } from "pinia";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { getVersion } from "@tauri-apps/api/app";
import { toast } from "vue-sonner";
import { i18n } from "@/i18n";

export type UpdateStatus =
  | "idle"
  | "checking"
  | "available"
  | "downloading"
  | "installed"
  | "error";

/**
 * 把插件抛出的原始错误(常含超长 URL / 底层 reqwest 报错,
 * 如 "error sending request for url (https://github.com/...)")转成
 * 适合 toast / 对话框展示的简短文案,避免 UI 溢出。
 */
function toFriendlyError(e: unknown): string {
  const raw = e instanceof Error ? e.message : String(e);
  const lower = raw.toLowerCase();
  const networkHints = [
    "error sending request",
    "network",
    "dns",
    "timed out",
    "timeout",
    "connect",
    "tcp",
    "eof",
  ];
  if (networkHints.some((hint) => lower.includes(hint))) {
    return i18n.global.t("update.errorNetwork");
  }
  // 去掉 URL(最易导致溢出),并截断过长的原始信息
  const cleaned = raw
    .replace(/https?:\/\/\S+/g, "")
    .replace(/\(\s*\)/g, "")
    .trim();
  const maxLen = 120;
  return cleaned.length > maxLen ? `${cleaned.slice(0, maxLen)}...` : cleaned;
}

export const useUpdateStore = defineStore("update", () => {
  const status = ref<UpdateStatus>("idle");
  /** 检查到的可用更新(由插件返回,含版本号/更新说明/下载安装方法;类实例含私有字段,必须 shallowRef 避免被响应式代理) */
  const update = shallowRef<Update | null>(null);
  /** 当前应用版本号 */
  const currentVersion = ref("");
  /** 下载进度(字节) */
  const downloaded = ref(0);
  const total = ref(0);
  const error = ref("");
  /** 更新详情对话框开关(toast action / 标题栏按钮 / 设置页均可打开) */
  const dialogOpen = ref(false);

  const hasUpdate = computed(() => status.value === "available" && update.value !== null);

  const progress = computed(() => {
    if (!total.value) return 0;
    return Math.min(100, Math.round((downloaded.value / total.value) * 100));
  });

  async function init() {
    if (currentVersion.value) return;
    try {
      currentVersion.value = await getVersion();
    } catch {
      currentVersion.value = "";
    }
  }

  /**
   * 检查更新
   * @param manual true 时通过 toast 反馈「已是最新/检查失败」;false 为静默检查(仅发现更新时提示)
   */
  async function checkForUpdate(manual: boolean) {
    if (status.value === "checking" || status.value === "downloading") return;
    const t = i18n.global.t;
    status.value = "checking";
    error.value = "";
    try {
      const result = await check();
      if (result) {
        update.value = result;
        status.value = "available";
        toast.info(t("update.available", { version: result.version }), {
          action: {
            label: t("update.viewDetail"),
            onClick: () => {
              dialogOpen.value = true;
            },
          },
        });
      } else {
        update.value = null;
        status.value = "idle";
        if (manual) toast.success(t("update.upToDate"));
      }
    } catch (e) {
      status.value = "error";
      error.value = toFriendlyError(e);
      if (manual) toast.error(t("update.checkFailed", { error: error.value }));
    }
  }

  async function downloadAndInstall() {
    const target = update.value;
    if (!target || status.value === "downloading") return;
    const t = i18n.global.t;
    status.value = "downloading";
    downloaded.value = 0;
    total.value = 0;
    try {
      await target.downloadAndInstall((event) => {
        if (event.event === "Started") {
          total.value = event.data.contentLength ?? 0;
        } else if (event.event === "Progress") {
          downloaded.value += event.data.chunkLength;
        }
      });
      status.value = "installed";
      // 后台下载(对话框已关闭)时 toast 通知完成,带「立即重启」入口;
      // 对话框打开时由对话框自身展示重启按钮,不重复提示
      if (!dialogOpen.value) {
        toast.success(t("update.installedHint"), {
          action: {
            label: t("update.restartNow"),
            onClick: () => relaunchApp(),
          },
        });
      }
    } catch (e) {
      status.value = "error";
      error.value = toFriendlyError(e);
      if (!dialogOpen.value) {
        toast.error(t("update.installFailed", { error: error.value }));
      }
    }
  }

  async function relaunchApp() {
    await relaunch();
  }

  return {
    status,
    update,
    currentVersion,
    downloaded,
    total,
    error,
    dialogOpen,
    hasUpdate,
    progress,
    init,
    checkForUpdate,
    downloadAndInstall,
    relaunchApp,
  };
});
