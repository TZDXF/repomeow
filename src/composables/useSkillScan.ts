import { computed, ref, shallowRef, type ComputedRef, type Ref, type ShallowRef } from "vue";
import { toast } from "vue-sonner";
import { cmd } from "@/lib/tauri";
import { getCachedScanReport, putCachedScanReport } from "@/lib/scan-cache";
import type { ResourceSkillScanReport } from "@/lib/resource-library";

/**
 * 技能安全扫描状态机(资源库技能预览页):运行/取消/已用时长计时/结果缓存。
 *
 * - runId 是后端 ai_cancel_run 的取消句柄,seq 是前端轮次序号;
 * - 取消(用户点按钮)只发 ai_cancel_run:后端仍返回含静态发现的报告
 *   (llmStatus = canceled),照常展示;
 * - dispose(离开页面)才会作废在途结果:序号自增后,晚到的报告不回填、
 *   失败不 toast、finally 不复位状态,并停表、发取消;
 * - 结果按「技能 id + 内容指纹」缓存 IndexedDB 30 天,指纹不符自动失效。
 */
export interface SkillScanRunOptions {
  language: string;
  runId: string;
  providerId?: string;
  modelId?: string;
}

export interface SkillScanModelRef {
  providerId: string;
  modelId: string;
}

export interface UseSkillScanOptions {
  /** 实际扫描调用(由调用方按库技能/本地目录分流) */
  scan: (options: SkillScanRunOptions) => Promise<ResourceSkillScanReport>;
  /** 界面语言快照(扫描提示词语言) */
  getLanguage: () => string;
  /** 扫描报告缓存键 */
  getCacheId: () => string;
  /** 技能内容指纹(缓存命中/回填依据);undefined = 未就绪不缓存 */
  getFingerprint: () => string | undefined;
  /** 错误展示格式化(errors.<code> i18n 通道回落原始字符串) */
  formatError: (e: unknown) => string;
}

export interface SkillScan {
  scanning: Ref<boolean>;
  scanReport: ShallowRef<ResourceSkillScanReport | null>;
  /** 扫描进行中的已用时长(秒) */
  scanElapsed: Ref<number>;
  scanElapsedLabel: ComputedRef<string>;
  /** 发起扫描;model 为 null 时跟随默认模型 */
  runScan: (model: SkillScanModelRef | null) => Promise<void>;
  /** 用户取消:仅发 ai_cancel_run,静态部分报告仍会展示 */
  cancelScan: () => void;
  /** 打开页面即恢复上次报告(内容指纹一致才命中) */
  hydrateCache: () => void;
  /** 离开页面:作废在途结果、停表、发取消 */
  dispose: () => void;
}

export function useSkillScan(options: UseSkillScanOptions): SkillScan {
  const scanning = ref(false);
  const scanReport = shallowRef<ResourceSkillScanReport | null>(null);
  const scanElapsed = ref(0);
  let scanRunId = "";
  let scanSeq = 0;
  let elapsedTimer: ReturnType<typeof setInterval> | null = null;

  const scanElapsedLabel = computed(() => {
    const minutes = Math.floor(scanElapsed.value / 60);
    const seconds = scanElapsed.value % 60;
    return `${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`;
  });

  function startTimer() {
    stopTimer();
    scanElapsed.value = 0;
    elapsedTimer = setInterval(() => {
      scanElapsed.value += 1;
    }, 1000);
  }

  function stopTimer() {
    if (elapsedTimer) {
      clearInterval(elapsedTimer);
      elapsedTimer = null;
    }
  }

  function cancelRunHandle() {
    if (scanRunId) {
      void cmd<void>("ai_cancel_run", { runId: scanRunId }).catch(() => {});
    }
  }

  function hydrateCache() {
    const fingerprint = options.getFingerprint();
    if (!fingerprint || scanReport.value || scanning.value) return;
    const cacheId = options.getCacheId();
    const seq = scanSeq;
    void getCachedScanReport(cacheId, fingerprint).then((cached) => {
      // 离页/新一轮扫描开始后,晚到的缓存回填作废
      if (seq !== scanSeq) return;
      if (cached && !scanReport.value && !scanning.value) {
        scanReport.value = cached;
      }
    });
  }

  async function runScan(model: SkillScanModelRef | null) {
    if (scanning.value) return;
    const seq = ++scanSeq;
    scanning.value = true;
    scanReport.value = null;
    startTimer();
    const runId = `scan-${Date.now()}-${Math.random().toString(36).slice(2)}`;
    scanRunId = runId;
    try {
      const report = await options.scan({
        language: options.getLanguage(),
        runId,
        providerId: model?.providerId,
        modelId: model?.modelId,
      });
      // dispose 后晚到的报告不回填、不写缓存
      if (seq !== scanSeq) return;
      scanReport.value = report;
      // 成功后写入缓存;技能内容变化会改变指纹,旧缓存自动失效
      const fingerprint = options.getFingerprint();
      if (fingerprint) void putCachedScanReport(options.getCacheId(), fingerprint, report);
    } catch (e) {
      // dispose 后的在途失败不 toast
      if (seq !== scanSeq) return;
      toast.error(options.formatError(e));
    } finally {
      if (seq === scanSeq) {
        scanRunId = "";
        scanning.value = false;
        stopTimer();
      }
    }
  }

  function cancelScan() {
    cancelRunHandle();
  }

  function dispose() {
    scanSeq += 1;
    cancelRunHandle();
    scanRunId = "";
    scanning.value = false;
    stopTimer();
  }

  return {
    scanning,
    scanReport,
    scanElapsed,
    scanElapsedLabel,
    runScan,
    cancelScan,
    hydrateCache,
    dispose,
  };
}
