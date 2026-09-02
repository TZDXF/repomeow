/** Markdown 内容中 URL/路径的处理工具(README 图片解析、链接拦截共用) */

import { cleanPath } from "@/lib/path";

/** Windows 盘符绝对路径(C:\ 或 C:/) */
const WINDOWS_ABS = /^[a-zA-Z]:[\\/]/;

/**
 * 带协议头的 URL(http:, data:, asset: 等)。
 * 注意排除 Windows 盘符路径(C:\...):其首段形如协议,但实为本地路径。
 */
export function hasScheme(url: string): boolean {
  return /^[a-z][a-z0-9+.-]*:/i.test(url) && !WINDOWS_ABS.test(url);
}

/** 统一分隔符并归一化 ./ ../ 段(Tauri asset 协议拒绝含 .. 的请求路径) */
function normalizeSegments(path: string): string {
  const isUnc = path.startsWith("\\\\") || path.startsWith("//");
  const out: string[] = [];
  // 根部长度:盘符("D:")为 1,UNC 头(server/share)为 2;.. 不得弹出根之外
  const rootLen = isUnc ? 2 : 1;
  for (const part of path.split(/[\\/]+/)) {
    if (!part || part === ".") continue;
    if (part === "..") {
      if (out.length > rootLen) out.pop();
      continue;
    }
    out.push(part);
  }
  return (isUnc ? "//" : "") + out.join("/");
}

/**
 * 把 Markdown 里的相对路径解析成项目内的绝对路径。
 * - 前导 / 或 \ 按 GitHub 语义视为相对项目根目录(root-relative),而非系统根目录
 * - 返回归一化后的路径(统一为 / 分隔,不含 ./ ../ 段)
 */
export function resolvePath(base: string, rel: string): string {
  let clean: string;
  try {
    clean = decodeURIComponent(rel);
  } catch {
    // 含非法 % 序列(如 100%.png)时按原样处理
    clean = rel;
  }
  clean = clean.split("#")[0].split("?")[0];
  // 已是绝对路径(Windows 盘符 / UNC)原样返回
  if (WINDOWS_ABS.test(clean) || clean.startsWith("\\\\")) return clean;
  // root-relative(/xxx)与普通相对路径统一拼到项目根上;base 为根(C:\、/)时不再补分隔符
  const root = cleanPath(base);
  const joined = `${root}${/[\\/]$/.test(root) ? "" : "/"}${clean.replace(/^[\\/]+/, "")}`;
  return normalizeSegments(joined);
}
