/** Markdown 内容中 URL/路径的处理工具(README 图片解析、链接拦截共用) */

import { cleanPath, isInsidePath, toForwardSlash } from "@/lib/path";

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
  const rootLen = isUnc ? 2 : path.startsWith("/") ? 0 : 1;
  for (const part of path.split(/[\\/]+/)) {
    if (!part || part === ".") continue;
    if (part === "..") {
      if (out.length > rootLen) out.pop();
      continue;
    }
    out.push(part);
  }
  return (isUnc ? "//" : path.startsWith("/") ? "/" : "") + out.join("/");
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

/** 允许作为外链直接打开的协议白名单 */
const SAFE_LINK_SCHEMES = new Set(["http:", "https:", "mailto:"]);

/**
 * 链接 href 白名单校验(自定义 MdLink 渲染器会绕过库内置 harden,需自行把关):
 * - 允许 http/https/mailto、页内锚点(#)、相对路径与本地绝对路径
 * - 其余协议(javascript:、data:、file:、vbscript: 等)返回 null,调用方应降级为纯文本
 * 判定前剔除 ASCII 空白/控制符,防 "java\tscript:" 之类绕过(浏览器解析 URL 时会忽略这些字符)
 */
export function safeLinkHref(url: string | null | undefined): string | null {
  if (!url) return null;
  const trimmed = url.trim();
  if (!trimmed) return null;
  if (trimmed.startsWith("#")) return trimmed;
  // eslint-disable-next-line no-control-regex -- 故意匹配控制符:浏览器解析 URL 会忽略它们,需在判定前剔除
  const compact = trimmed.replace(/[\u0000-\u0020]/g, "");
  if (!hasScheme(compact)) return trimmed;
  const scheme = compact.slice(0, compact.indexOf(":") + 1).toLowerCase();
  return SAFE_LINK_SCHEMES.has(scheme) ? trimmed : null;
}

/** 仅允许 http(s) 图片和可信根目录内的本地图片;没有根目录时禁用本地图片。 */
export function safeMarkdownImage(url: string, base: string, root: string): string | null {
  const value = url.trim();
  // eslint-disable-next-line no-control-regex -- 拒绝路径内控制字符
  if (!value || /[\u0000-\u001f\u007f]/.test(value)) return null;
  if (/^https?:\/\//i.test(value)) {
    try {
      const parsed = new URL(value);
      return parsed.protocol === "http:" || parsed.protocol === "https:" ? value : null;
    } catch {
      return null;
    }
  }
  let decoded: string;
  try {
    decoded = decodeURIComponent(value).split(/[?#]/, 1)[0];
  } catch {
    return null;
  }
  // 不允许网络路径、asset/file 等协议或没有可信根目录的本地路径。
  if (
    !root ||
    !base ||
    // eslint-disable-next-line no-control-regex -- 同时检查 URL 解码后的控制字符
    /[\u0000-\u001f\u007f]/.test(decoded) ||
    /^[\\/]{2}/.test(decoded) ||
    hasScheme(decoded)
  )
    return null;
  const normalize = (path: string): string => {
    const forward = toForwardSlash(path);
    const prefix = forward.startsWith("/") ? "/" : "";
    const parts: string[] = [];
    for (const part of forward.split("/")) {
      if (!part || part === ".") continue;
      if (part === "..") parts.pop();
      else parts.push(part);
    }
    return prefix + parts.join("/");
  };
  const cleanRoot = normalize(root);
  const cleanBase = normalize(base);
  if (!cleanRoot || !isInsidePath(cleanRoot, cleanBase)) return null;
  const path = WINDOWS_ABS.test(decoded)
    ? normalize(decoded)
    : normalize(`${cleanBase}/${decoded.replace(/^[\\/]+/, "")}`);
  return isInsidePath(cleanRoot, path) ? path : null;
}
