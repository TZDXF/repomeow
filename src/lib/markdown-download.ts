import { save } from "@tauri-apps/plugin-dialog";
import { toast } from "vue-sonner";
import type { ComposerTranslation } from "vue-i18n";
import type { Control, DownloadEvent } from "vue-stream-markdown";
import { cmd } from "@/lib/tauri";
import {
  extractTableData,
  tableDataToCSV,
  tableDataToMarkdown,
  tableDataToTSV,
} from "@/lib/table-format";

// vue-stream-markdown 没再导出 ContentDownloadEvent,本地按需声明
type ContentDownloadType = "code" | "mermaid" | "table";

/**
 * 给 `<Markdown :controls="controls">` 用的 table customize 钩子:
 * 替换库内置的「下载」按钮 onClick,让用户真正在 save dialog 里选什么格式
 * (CSV / TSV / Markdown)就生成什么内容。
 *
 * 背景:库内置下载的流程是「用户在 toolbar 下拉里选格式 → 库按该格式生成字符串 →
 * 直接弹原生下载 (`<a download>`)」。原本 Tauri WebView2 下下载静默失败;之前我们用
 * beforeDownload 拦截改走 Tauri save dialog,但此时 `event.content` 已经是库按
 * 用户下拉选项预生成的字符串,而 dialog 的过滤器默认又是 CSV —— 用户在 dialog 里
 * 选 Markdown,文件名会变成 .md,但**写入的内容仍是 CSV 字符串**(选 csv) 或反过来,
 * 文件后缀与内容不一致。
 *
 * 修复:完全替换 download 的 onClick —— 点击时从当前 table DOM 抽 TableData
 * (extractTableData,与库内置 getContent 走的是同一个数据源),
 * 弹 save dialog 让用户在过滤器里选 CSV / TSV / Markdown,根据选中的扩展名生成对应内容
 * (本地实现 tableDataToCSV/TSV/Markdown,行为与库一致),再用 save_text_file 写入。
 *
 * copy / fullscreen 按钮走库内置逻辑,不动。
 */
export function createTableCustomize(t: ComposerTranslation) {
  return (builtinControls: Control[]): Control[] => {
    return builtinControls.map((c) => {
      if (c.key !== "download") {
        return c;
      }
      return {
        ...c,
        // 去掉 options,避免 Dropdown;改成单击按钮直接弹 save dialog
        options: undefined,
        onClick: async (event: MouseEvent) => {
          try {
            const tableEl = findTableElement(event.currentTarget as HTMLElement | null);
            if (!tableEl) {
              toast.error(t("markdown.tableNotFound"));
              return;
            }
            const data = extractTableData(tableEl),
              path = await save({
                title: t("markdown.saveDialogTitle"),
                defaultPath: "table.csv",
                filters: [
                  { name: "CSV", extensions: ["csv"] },
                  { name: "TSV", extensions: ["tsv"] },
                  { name: "Markdown", extensions: ["md"] },
                ],
              });
            if (!path) {
              return;
            }
            const ext = path.split(".").pop()?.toLowerCase() ?? "";
            let content: string;
            if (ext === "tsv") {
              content = tableDataToTSV(data);
            } else if (ext === "md" || ext === "markdown") {
              content = tableDataToMarkdown(data);
            } else {
              content = tableDataToCSV(data);
            }
            await cmd<void>("save_text_file", { path, content });
            toast.success(t("markdown.savedAs", { path }));
          } catch (e) {
            toast.error(t("markdown.saveFailed", { error: String(e) }));
          }
        },
      };
    });
  };
}

/**
 * Markdown 中"下载"按钮的拦截钩子(vue-stream-markdown 的 beforeDownload)。
 * 仅用于 code / mermaid —— 表格由 createTableCustomize 完全接管,这里不再处理。
 *
 * 背景:库默认走 `URL.createObjectURL + <a download>`,在 Tauri WebView2 下经常静默失败
 * 且无任何反馈。这里改为:弹 Tauri save dialog 拿目标路径 → 调 `save_text_file` 写入。
 */
export function createBeforeDownload(t: ComposerTranslation) {
  return async (event: DownloadEvent) => {
    // 图片(type: "image")走库默认 a[download] 处理即可
    if (event.type === "image") {
      return true;
    }
    const contentType = event.type satisfies ContentDownloadType,
      filename = defaultFilename(contentType);
    try {
      const path = await save({
        title: t("markdown.saveDialogTitle"),
        defaultPath: filename,
        filters: filtersFor(contentType),
      });
      if (!path) {
        return false;
      }
      await cmd<void>("save_text_file", { path, content: event.content });
      toast.success(t("markdown.savedAs", { path }));
      return false;
    } catch (e) {
      toast.error(t("markdown.saveFailed", { error: String(e) }));
      return false;
    }
  };
}

function findTableElement(start: HTMLElement | null): HTMLTableElement | null {
  if (!start) {
    return null;
  }
  // toolbar 与 table 是兄弟节点(都在 [data-stream-markdown="table-wrapper"] 内),
  // closest('table') 走祖先链会找不到 table。
  // 可靠路径:从触发元素自己开始向上找最近的 table-wrapper → 在其子树中找 <table>。
  // 这样全屏 Modal 内的 toolbar 也能找到 modal 自己的表格(而不是外面的)。
  let el: HTMLElement | null = start,
    wrapper: HTMLElement | null = null;
  while (el) {
    if (el.dataset.streamMarkdown === "table-wrapper") {
      wrapper = el;
      break;
    }
    el = el.parentElement;
  }
  return wrapper?.querySelector("table") ?? null;
}

function defaultFilename(type: ContentDownloadType): string {
  switch (type) {
    case "code":
      return "snippet.txt";
    case "mermaid":
      return "diagram.mmd";
    case "table":
      return "table.csv";
  }
}

function filtersFor(type: ContentDownloadType) {
  switch (type) {
    case "code":
      return [
        { name: "Text", extensions: ["txt", "md"] },
        { name: "All", extensions: ["*"] },
      ];
    case "mermaid":
      return [
        { name: "Mermaid", extensions: ["mmd", "md"] },
        { name: "All", extensions: ["*"] },
      ];
    case "table":
      return [
        { name: "CSV", extensions: ["csv"] },
        { name: "TSV", extensions: ["tsv"] },
        { name: "Markdown", extensions: ["md"] },
        { name: "All", extensions: ["*"] },
      ];
  }
}
