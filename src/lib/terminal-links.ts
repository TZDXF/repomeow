import { openUrl } from "@tauri-apps/plugin-opener";
import { toast } from "vue-sonner";

/** 普通文本链接与 OSC 8 超链接共用,禁止终端输出触发非网页协议。 */
export function activateTerminalLink(event: MouseEvent, uri: string): void {
  if (!event.ctrlKey || event.button !== 0) return;
  let url: URL;
  try {
    url = new URL(uri);
  } catch {
    return;
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") return;
  event.preventDefault();
  void openUrl(uri).catch((error: unknown) => toast.error(String(error)));
}
