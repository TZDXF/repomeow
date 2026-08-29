import { ref, shallowRef, type Ref, type ShallowRef } from "vue";
import { cmd } from "@/lib/tauri";

/**
 * 语义分析请求的通用状态机:loading / error / result + requestId 取消与防串位。
 *
 * - 每次 run 生成新的 requestId 并先取消旧请求(避免快速切换实体占满 sem 并发槽);
 * - 序号防串位:旧响应到达时不覆盖新请求的状态;
 * - semantic_canceled 错误静默吞掉(取消是用户/切换的正常路径,不弹错)。
 */
export interface SemanticRequest<TArgs extends unknown[], TResult> {
  loading: Ref<boolean>;
  error: Ref<string>;
  /** 后端错误码(如 semantic_entity_not_found);无错误时为 null */
  errorCode: Ref<string | null>;
  result: ShallowRef<TResult | null>;
  run: (...args: TArgs) => Promise<TResult | null>;
  cancel: () => void;
  reset: () => void;
}

export function isSemanticCanceled(error: unknown): boolean {
  return (
    !!error &&
    typeof error === "object" &&
    (error as { code?: unknown }).code === "semantic_canceled"
  );
}

export function useSemanticRequest<TArgs extends unknown[], TResult>(
  invoke: (requestId: string, ...args: TArgs) => Promise<TResult>,
): SemanticRequest<TArgs, TResult> {
  const loading = ref(false);
  const error = ref("");
  const errorCode = ref<string | null>(null);
  const result = shallowRef<TResult | null>(null);
  let seq = 0;
  let currentRequestId: string | null = null;

  function cancel() {
    const id = currentRequestId;
    currentRequestId = null;
    if (id) {
      void cmd<boolean>("semantic_cancel", { requestId: id }).catch(() => {});
    }
  }

  async function run(...args: TArgs): Promise<TResult | null> {
    const mySeq = ++seq;
    cancel();
    const requestId = crypto.randomUUID();
    currentRequestId = requestId;
    loading.value = true;
    error.value = "";
    errorCode.value = null;
    try {
      const value = await invoke(requestId, ...args);
      if (mySeq !== seq) return null;
      result.value = value;
      return value;
    } catch (e) {
      if (mySeq !== seq || isSemanticCanceled(e)) return null;
      error.value = e instanceof Error ? e.message : String(e);
      errorCode.value =
        e && typeof e === "object" && typeof (e as { code?: unknown }).code === "string"
          ? (e as { code: string }).code
          : null;
      return null;
    } finally {
      if (mySeq === seq) {
        loading.value = false;
        if (currentRequestId === requestId) currentRequestId = null;
      }
    }
  }

  function reset() {
    seq += 1;
    cancel();
    loading.value = false;
    error.value = "";
    errorCode.value = null;
    result.value = null;
  }

  return { loading, error, errorCode, result, run, cancel, reset };
}
