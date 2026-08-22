import { type ClassValue, clsx } from "clsx";
import { toast } from "vue-sonner";
import { twMerge } from "tailwind-merge";
import { i18n } from "@/i18n";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

/** 复制文本到剪贴板并统一 toast 反馈 */
export async function copyToClipboard(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
    toast.success(i18n.global.t("common.copied"));
  } catch (e) {
    toast.error(String(e));
  }
}

/** 带取消的防抖(@vueuse/core 的 useDebounceFn 无 cancel,供「手动立即触发/关闭即取消」场景复用) */
export function debounce<A extends unknown[]>(fn: (...args: A) => void, ms: number) {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const run = (...args: A) => {
    clearTimeout(timer);
    timer = setTimeout(() => fn(...args), ms);
  };
  run.cancel = () => clearTimeout(timer);
  return run;
}
