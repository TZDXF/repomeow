//! 路径风格统一辅助:全仓路径只在此处做规范化,禁止各处 ad-hoc `replace('\\', "/")`。
//!
//! 三种形态约定:
//! - **clean(落库/缓存 key)**:平台原生分隔符(Windows `\`,其他 `/`)、无尾随分隔符;
//!   同一目录的不同写法(正反斜杠、尾斜杠)归一为同一字符串,保证 SQLite UNIQUE 与
//!   各类 HashMap key 一致。不做大小写折叠:Windows 比较处另行 `eq_ignore_ascii_case`,
//!   避免破坏 Linux 大小写敏感语义。
//! - **forward(IPC/git)**:恒为 `/` 分隔,与 libgit2 delta path、git pathspec 一致;
//!   跨 IPC 边界传给前端的仓库内路径一律走此形态。
//! - 前端展示路径保持用户输入原样,仅入库/比较/缓存时归一化。

use std::path::{Path, PathBuf};

/// 去掉尾随分隔符,但保留根(盘符根 `C:` → `C:\`、`/` 本身)
fn trim_trailing_seps(s: &str, sep: char) -> String {
    let t = s.trim_end_matches(sep);
    if t.len() == 2 && t.ends_with(':') {
        // Windows 盘符根:`C:` 是盘符相对路径,与 `C:\` 语义不同,必须补回
        format!("{t}{sep}")
    } else if t.is_empty() {
        // 输入全是分隔符(如 `\\` 或 `//`)→ 归一为根
        sep.to_string()
    } else {
        t.to_string()
    }
}

/// 归一化为平台原生分隔符、无尾随分隔符的字符串形式(落库/缓存 key 用)。
/// Windows 下 `/` 统一转 `\`;非 Windows 下 `\` 是合法文件名字符,原样保留。
pub fn clean_str(s: &str) -> String {
    let s = s.trim();
    #[cfg(windows)]
    {
        trim_trailing_seps(&s.replace('/', "\\"), '\\')
    }
    #[cfg(not(windows))]
    {
        trim_trailing_seps(s, '/')
    }
}

/// `clean_str` 的 PathBuf 形式(HashMap key、路径比较用)
pub fn clean(path: &Path) -> PathBuf {
    PathBuf::from(clean_str(&path.to_string_lossy()))
}

/// 归一化为 `/` 分隔字符串(IPC 输出、git pathspec、与 libgit2 输出对齐用)。
/// 输入已是 `/` 分隔时仅去尾随分隔符
pub fn to_forward_slash(path: &Path) -> String {
    to_forward_slash_str(&path.to_string_lossy())
}

/// `to_forward_slash` 的字符串形式
pub fn to_forward_slash_str(s: &str) -> String {
    trim_trailing_seps(&s.trim().replace('\\', "/"), '/')
}

/// Windows: 转 `\` 分隔字符串(explorer `/select,` 等只认反斜杠的下游消费者)
#[cfg(windows)]
pub fn to_native_separator(s: &str) -> String {
    s.replace('/', "\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_slash_normalizes_and_trims() {
        assert_eq!(to_forward_slash_str("C:\\repo\\sub"), "C:/repo/sub");
        assert_eq!(to_forward_slash_str("C:/repo/sub/"), "C:/repo/sub");
        assert_eq!(to_forward_slash_str("repo\\sub\\"), "repo/sub");
        // 已是目标形态时不变
        assert_eq!(to_forward_slash_str("a/b/c"), "a/b/c");
    }

    #[test]
    fn forward_slash_preserves_roots() {
        // Unix 根与盘符根不能丢尾随分隔符后变成相对路径
        assert_eq!(to_forward_slash_str("/"), "/");
        assert_eq!(to_forward_slash_str("C:\\"), "C:/");
        assert_eq!(to_forward_slash_str("C:/"), "C:/");
    }

    #[test]
    #[cfg(windows)]
    fn clean_str_unifies_separators_on_windows() {
        assert_eq!(clean_str("C:/repo/sub"), "C:\\repo\\sub");
        assert_eq!(clean_str("C:\\repo\\sub\\"), "C:\\repo\\sub");
        assert_eq!(clean_str("C:/repo/sub/"), "C:\\repo\\sub");
        // 同一目录两种写法归一为同一字符串(缓存 key / UNIQUE 一致)
        assert_eq!(clean_str("D:/code/proj/"), clean_str("D:\\code\\proj"));
        // 盘符根保留
        assert_eq!(clean_str("C:/"), "C:\\");
        assert_eq!(clean_str("C:\\"), "C:\\");
        // 首尾空白
        assert_eq!(clean_str("  D:\\repo  "), "D:\\repo");
    }

    #[test]
    #[cfg(not(windows))]
    fn clean_str_trims_trailing_sep_on_unix() {
        assert_eq!(clean_str("/repo/sub/"), "/repo/sub");
        assert_eq!(clean_str("/"), "/");
        // 反斜杠是合法文件名字符,不替换
        assert_eq!(clean_str("/repo/a\\b"), "/repo/a\\b");
    }

    #[test]
    fn clean_pathbuf_roundtrip() {
        let p = clean(Path::new(&to_forward_slash_str("x/y/")));
        assert_eq!(p, clean(Path::new("x/y")));
    }

    #[test]
    #[cfg(windows)]
    fn native_separator_for_explorer() {
        assert_eq!(to_native_separator("C:/repo/sub"), r"C:\repo\sub");
        assert_eq!(to_native_separator(r"C:\repo"), r"C:\repo");
    }
}
