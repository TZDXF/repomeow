import { openUrl } from "@tauri-apps/plugin-opener";
import { safeLinkHref } from "@/lib/markdown";

/** 在所有 Markdown 渲染点阻止浏览器导航;相对链接仍交给外层按场景处理。 */
export function interceptMarkdownLink(event: MouseEvent): void {
  const target = event.target as Element | null;
  const anchor = target?.closest?.("a");
  if (!anchor) return;
  event.preventDefault();
  if (
    event.type !== "click" ||
    event.button !== 0 ||
    event.ctrlKey ||
    event.metaKey ||
    event.shiftKey ||
    event.altKey
  ) {
    event.stopPropagation();
    return;
  }
  const href = safeLinkHref(anchor.getAttribute("href"));
  if (!href) {
    event.stopPropagation();
    return;
  }
  // 拒绝带控制符的协议变体,避免白名单判断与系统 URL 解析不一致。
  // eslint-disable-next-line no-control-regex -- 拒绝控制字符混淆协议
  if (/^(https?:|mailto:)/i.test(href) && !/[\u0000-\u0020\u007f]/.test(href)) {
    event.stopPropagation();
    void openUrl(href).catch(() => {});
  }
}
