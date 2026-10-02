import { computed, ref, type ComputedRef, type Ref } from "vue";
import { toast } from "vue-sonner";
import { cmd } from "@/lib/tauri";
import { getCachedTranslation, putCachedTranslation } from "@/lib/translation-cache";

/**
 * Markdown 翻译共享状态机(技能预览页 / AI 文件抽屉 / 远程审计详情共用):
 * 「缓存查询 → AI 翻译 → 原文/译文切换 → 重新翻译」全流程,译文按
 * 「界面语言 + 正文内容 hash」缓存入 IndexedDB 保留 30 天。
 *
 * 防串位约定:
 * - 请求开始时捕获身份(getIdentity)、正文与语言快照;缓存 await 与 AI await
 *   之后都校验「序号 + 当前身份」,取消/换文件/保存/卸载(reset)后旧结果一律丢弃;
 * - 缓存查询期间 translating 即为 true,防止重复点击并发触发;
 * - 过期请求的失败不 toast;过期请求的 finally 不复位新请求的状态。
 *
 * 模型来源:后端 ai_translate_markdown 经 load_config_for(app, "translation")
 * 取翻译专用配置,未配置时回退设置页默认模型。
 */
export interface MarkdownTranslationOptions {
  /** 当前翻译对象身份(文件路径/审计条目);null = 无可翻译对象 */
  getIdentity: () => string | null;
  /** 当前正文;null/空 = 不可翻译 */
  getText: () => string | null;
  /** 目标语言(请求开始时取快照) */
  getLanguage: () => string;
  /** runId 前缀(ai_cancel_run 句柄命名,区分调用来源) */
  runIdPrefix?: string;
}

export interface MarkdownTranslation {
  translating: Ref<boolean>;
  translatedText: Ref<string | null>;
  /** 译文所属身份;与当前身份不符时不展示译文 */
  translatedFor: Ref<string | null>;
  showTranslated: Ref<boolean>;
  /** 当前身份已有译文(「重新翻译」按钮显隐依据) */
  hasTranslation: ComputedRef<boolean>;
  /** 渲染正文:译文激活时取译文,否则取原文 */
  displayContent: (original: string) => string;
  /** 主按钮:翻译中再点 = 取消;有译文 = 原文/译文切换;否则发起翻译 */
  toggle: () => Promise<void>;
  /** 重新翻译:跳过缓存强制再调 AI,成功后覆盖缓存 */
  retranslate: () => Promise<void>;
  /** 取消在途请求并清空译文状态(换文件/保存/关闭/卸载时调用) */
  reset: () => void;
}

export function useMarkdownTranslation(options: MarkdownTranslationOptions): MarkdownTranslation {
  const translating = ref(false);
  const translatedText = ref<string | null>(null);
  const translatedFor = ref<string | null>(null);
  const showTranslated = ref(false);
  /** 当前在途翻译的 runId(ai_cancel_run 的取消句柄);空 = 无在途请求 */
  let translateRunId = "";
  /** 翻译轮次序号:取消/换文件后自增,使仍在途的缓存查询与 AI 结果作废 */
  let translateSeq = 0;

  const hasTranslation = computed(
    () => translatedText.value !== null && translatedFor.value === options.getIdentity(),
  );

  function displayContent(original: string): string {
    return showTranslated.value && hasTranslation.value && translatedText.value !== null
      ? translatedText.value
      : original;
  }

  /** 过期判定:序号被新一轮重置,或请求期间身份已切换 */
  function isStale(seq: number, identity: string): boolean {
    return seq !== translateSeq || options.getIdentity() !== identity;
  }

  function applyResult(identity: string, result: string) {
    translatedText.value = result;
    translatedFor.value = identity;
    showTranslated.value = true;
  }

  function reset() {
    translateSeq += 1;
    if (translateRunId) {
      void cmd<void>("ai_cancel_run", { runId: translateRunId }).catch(() => {});
      translateRunId = "";
    }
    translating.value = false;
    translatedText.value = null;
    translatedFor.value = null;
    showTranslated.value = false;
  }

  async function run(skipCache: boolean) {
    // 请求开始即捕获身份/正文/语言快照,await 之后只认这组快照
    const identity = options.getIdentity();
    const text = options.getText();
    if (!identity || !text) return;
    const language = options.getLanguage();
    const seq = ++translateSeq;
    // 缓存查询期间同样置忙:防止 await 期间重复点击并发触发第二次翻译
    translating.value = true;
    try {
      if (!skipCache) {
        // 先查 IndexedDB 缓存(键 = 界面语言 + 内容 hash):命中直接展示、不再调 AI
        const cached = await getCachedTranslation(text, language);
        if (isStale(seq, identity)) return;
        if (cached !== null) {
          applyResult(identity, cached);
          return;
        }
      }
      const runId = `${options.runIdPrefix ?? "translate"}-${Date.now()}-${Math.random()
        .toString(36)
        .slice(2)}`;
      translateRunId = runId;
      const result = await cmd<string | null>("ai_translate_markdown", {
        request: { text, language, runId },
      });
      // 取消后返回 null;换文件/切走后的在途结果作废
      if (result === null || isStale(seq, identity)) return;
      applyResult(identity, result);
      void putCachedTranslation(text, language, result);
    } catch (e) {
      // 旧请求(已被取消/替换)的失败不打扰用户
      if (isStale(seq, identity)) return;
      toast.error(String(e));
    } finally {
      // 只复位本轮状态:新请求已开始时,旧 finally 不得清掉新请求的 runId/busy
      if (seq === translateSeq) {
        translateRunId = "";
        translating.value = false;
      }
    }
  }

  async function toggle() {
    if (translating.value) {
      reset();
      return;
    }
    if (showTranslated.value) {
      showTranslated.value = false;
      return;
    }
    if (hasTranslation.value) {
      showTranslated.value = true;
      return;
    }
    await run(false);
  }

  async function retranslate() {
    if (translating.value || !hasTranslation.value) return;
    await run(true);
  }

  return {
    translating,
    translatedText,
    translatedFor,
    showTranslated,
    hasTranslation,
    displayContent,
    toggle,
    retranslate,
    reset,
  };
}
