import {
  DEFAULT_FILE,
  DEFAULT_FOLDER,
  DEFAULT_FOLDER_OPENED,
  getIconForFile,
  getIconForFolder,
  getIconForOpenFolder,
} from "vscode-icons-js";
import { addCollection } from "@iconify/vue";
import vscodeIconsJson from "@iconify-json/vscode-icons/icons.json";
import type { IconifyJSON } from "@iconify/vue";

/**
 * vscode-icons 文件/目录图标解析。
 * 图标集走 @iconify-json/vscode-icons(Iconify 自动同步 vscode-icons 上游,
 * pnpm 升级即可跟随更新),文件名映射走 vscode-icons-js;
 * 映射不到或图标在集合中缺失时回退默认文件/目录图标。
 * 图标为内联 SVG,经路由懒加载随文件预览页按需加载。
 */
const collection = vscodeIconsJson as IconifyJSON;
addCollection(collection);

const available = new Set(Object.keys(collection.icons));
const PREFIX = "vscode-icons:";

/** vscode-icons-js 的 SVG 文件名转 iconify 图标名(file_type_ts.svg -> file-type-ts) */
function toIconifyName(svgFileName: string): string {
  return svgFileName.replace(/\.svg$/, "").replace(/_/g, "-");
}

function resolve(svgFileName: string | undefined, fallback: string): string {
  const name = svgFileName && toIconifyName(svgFileName);
  return PREFIX + (name && available.has(name) ? name : toIconifyName(fallback));
}

/** 文件图标名(按文件名含扩展名匹配,如 foo.ts -> vscode-icons:file-type-typescript) */
export function fileIcon(fileName: string): string {
  return resolve(getIconForFile(fileName), DEFAULT_FILE);
}

/** 目录图标名;open 为 true 时取展开态图标 */
export function folderIcon(folderName: string, open: boolean): string {
  return open
    ? resolve(getIconForOpenFolder(folderName), DEFAULT_FOLDER_OPENED)
    : resolve(getIconForFolder(folderName), DEFAULT_FOLDER);
}
