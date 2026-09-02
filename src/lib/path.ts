/**
 * 路径风格统一辅助:与后端 `src-tauri/src/path_util.rs` 对齐,
 * 散落的 `split("/")` / `replace(/\\/g, "/")` 一律改用这里。
 *
 * 形态约定:
 * - 后端 IPC 返回的仓库内路径(git 文件清单、worktree 路径)恒为 `/` 分隔;
 * - 用户输入/系统对话框返回的项目路径保持原样展示,仅在比较/切分时归一化;
 * - Windows 与 POSIX 分隔符都按 `[\\/]` 处理,不做平台探测。
 */

/** 去首尾空白与尾随分隔符(保留盘符根 `C:\` 与 `/` 本身) */
export function cleanPath(p: string): string {
  const trimmed = p.trim();
  const stripped = trimmed.replace(/[\\/]+$/, "");
  if (stripped === "") {
    return trimmed.startsWith("/") ? "/" : trimmed;
  }
  // 盘符根:C: 是盘符相对路径,与 C:\ 语义不同,补回分隔符
  if (/^[A-Za-z]:$/.test(stripped)) {
    return `${stripped}\\`;
  }
  return stripped;
}

/** 归一化为 `/` 分隔(与后端/git 输出对齐,用于比较与切分) */
export function toForwardSlash(p: string): string {
  return p.trim().replace(/\\/g, "/").replace(/\/+$/, "");
}

/** 文件名(最后一段,两种分隔符都支持) */
export function baseName(p: string): string {
  const segments = p.split(/[\\/]/).filter(Boolean);
  return segments[segments.length - 1] ?? p;
}

/** 切出父目录与目录名,父目录保留原路径的分隔符风格(Windows 盘符下用 \) */
export function splitDirName(p: string): { parent: string; name: string } {
  const segments = p.split(/[\\/]/).filter(Boolean);
  const name = segments.pop() ?? "";
  const sep = p.includes("\\") ? "\\" : "/";
  return { parent: segments.join(sep) || sep, name };
}

/** 拼接父目录与子段,分隔符跟随父路径的写法;父路径为根(`C:\`、`/`)时不再补分隔符 */
export function joinPath(parent: string, name: string): string {
  const base = cleanPath(parent);
  if (base.endsWith("\\") || base.endsWith("/")) {
    return base + name;
  }
  const sep = parent.includes("\\") ? "\\" : "/";
  return `${base}${sep}${name}`;
}

/** 展示用路径:p 位于 root 内时显示相对路径,否则原样返回(分隔符归一后比较) */
export function displayRelativeTo(root: string, p: string): string {
  const r = toForwardSlash(root);
  const c = toForwardSlash(p);
  if (r && c.startsWith(`${r}/`)) {
    return c.slice(r.length + 1);
  }
  return p;
}
