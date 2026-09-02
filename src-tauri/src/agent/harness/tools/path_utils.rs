//! 工具路径归一化:对齐 `packages/agent/src/harness/tools/path-utils.ts`。
//!
//! NFD 变体(蓝本 `resolved.normalize("NFD")`)依赖 unicode-normalization crate,
//! 本仓库依赖树未提供,跳过该变体(其余变体保留,见报告偏差)。

use crate::agent::harness::types::{ExecutionEnv, FileError};
use crate::agent::types::AbortSignal;

/// Unicode 空白字符(对齐 TS `UNICODE_SPACES`)。
const UNICODE_SPACES: [char; 9] = [
    '\u{00A0}', '\u{2000}', '\u{2001}', '\u{2002}', '\u{2003}', '\u{2004}', '\u{2005}', '\u{2006}',
    '\u{3000}',
];

/// TS `UNICODE_SPACES` 的完整集合(2007-200A 与 202F、205F 也算)。
const UNICODE_SPACES_EXTRA: [char; 4] = ['\u{2007}', '\u{2008}', '\u{2009}', '\u{200A}'];

const NARROW_NO_BREAK_SPACE: char = '\u{202F}';

/// 归一化工具路径:Unicode 空白转普通空格,剥离前导 `@`。
pub fn normalize_tool_path(path: &str) -> String {
    let mut normalized: String = path
        .chars()
        .map(|c| {
            if UNICODE_SPACES.contains(&c) || UNICODE_SPACES_EXTRA.contains(&c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    if normalized.starts_with('@') {
        normalized = normalized[1..].to_string();
    }
    normalized
}

/// 绝对化工具路径(对齐 TS `resolveToolPath`;env 失败时返回 FileError 而非 panic)。
pub async fn resolve_tool_path(
    env: &dyn ExecutionEnv,
    path: &str,
    signal: Option<AbortSignal>,
) -> Result<String, FileError> {
    env.absolute_path(normalize_tool_path(path), signal).await
}

/// 读工具路径:依次尝试字面路径与 macOS 截图风格的变体
/// (对齐 TS `resolveReadToolPath`;NFD 变体未实现)。
pub async fn resolve_read_tool_path(
    env: &dyn ExecutionEnv,
    path: &str,
    signal: Option<AbortSignal>,
) -> Result<String, FileError> {
    let resolved = resolve_tool_path(env, path, signal.clone()).await?;
    // " (AM|PM)." → 窄不换行空格(macOS 截图 "Screenshot 2024-01-01 at 10.00.00 AM.png")。
    let am_pm_replaced = replace_am_pm(&resolved);
    let smart_quote_replaced = resolved.replace('\'', "\u{2019}");

    let mut variants: Vec<String> = vec![resolved.clone()];
    for candidate in [am_pm_replaced, smart_quote_replaced] {
        if !variants.contains(&candidate) {
            variants.push(candidate);
        }
    }

    for variant in variants {
        if let Ok(true) = env.exists(variant.clone(), signal.clone()).await {
            return Ok(variant);
        }
    }
    Ok(resolved)
}

fn replace_am_pm(path: &str) -> String {
    // 大小写不敏感的 " (AM|PM)\." → "\u202F$1."。
    let mut result = String::with_capacity(path.len());
    let bytes: Vec<char> = path.chars().collect();
    let mut index = 0;
    while index < bytes.len() {
        // 匹配 "(space)(AM|PM)(.)" 的一角:space + A/P + M + '.'
        if bytes[index] == ' '
            && index + 3 < bytes.len()
            && (bytes[index + 1] == 'A' || bytes[index + 1] == 'P')
            && bytes[index + 2] == 'M'
            && bytes[index + 3] == '.'
        {
            result.push(NARROW_NO_BREAK_SPACE);
            result.push(bytes[index + 1]);
            result.push(bytes[index + 2]);
            result.push('.');
            index += 4;
        } else if bytes[index] == ' '
            && index + 3 < bytes.len()
            && (bytes[index + 1] == 'a' || bytes[index + 1] == 'p')
            && bytes[index + 2] == 'm'
            && bytes[index + 3] == '.'
        {
            result.push(NARROW_NO_BREAK_SPACE);
            result.push(bytes[index + 1]);
            result.push(bytes[index + 2]);
            result.push('.');
            index += 4;
        } else {
            result.push(bytes[index]);
            index += 1;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_tool_paths() {
        assert_eq!(
            normalize_tool_path("@relative/file.txt"),
            "relative/file.txt"
        );
        assert_eq!(normalize_tool_path("/a\u{00A0}b\u{2002}c"), "/a b c");
        assert_eq!(normalize_tool_path("/plain"), "/plain");
    }

    #[test]
    fn am_pm_variants_use_narrow_space() {
        assert_eq!(
            replace_am_pm("/shot 10.00.00 AM.png"),
            "/shot 10.00.00\u{202F}AM.png"
        );
        // 只有 "(AM|PM)." 中的空格替换。
        assert_eq!(replace_am_pm("AM.x"), "AM.x");
        assert_eq!(replace_am_pm("/plain.png"), "/plain.png");
    }
}
