//! 截断工具:对齐 `packages/agent/src/harness/utils/truncate.ts`。
//!
//! 两条独立限制,先到先得:行数(默认 2000)与字节数(默认 50KB)。
//! 除 bash 尾截断的"末行部分保留"边界外,永不返回残缺行。
//! Rust 字符串为 UTF-8,`char` 计数与 JS 的 UTF-16 code-unit 计数在增补平面字符
//! (emoji 等)上不同 —— 对齐蓝本按**字节**截断的核心行为,字符计数差异仅影响
//! `truncateLine`/`estimateTokens` 的保守估计(见报告偏差说明)。

use serde::{Deserialize, Serialize};

pub const DEFAULT_MAX_LINES: usize = 2000;
pub const DEFAULT_MAX_BYTES: usize = 50 * 1024; // 50KB
/// 单条 grep 匹配行的最大字符数。
pub const GREP_MAX_LINE_LENGTH: usize = 500;

/// 截断结果(对齐 TS `TruncationResult`;serde 输出 camelCase 供 details 使用)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TruncationResult {
    pub content: String,
    pub truncated: bool,
    /// "lines" | "bytes" | null
    pub truncated_by: Option<TruncatedBy>,
    pub total_lines: usize,
    pub total_bytes: usize,
    pub output_lines: usize,
    pub output_bytes: usize,
    pub last_line_partial: bool,
    pub first_line_exceeds_limit: bool,
    pub max_lines: usize,
    pub max_bytes: usize,
}

/// 触发的限制种类。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TruncatedBy {
    Lines,
    Bytes,
}

/// 截断选项。
#[derive(Clone, Copy, Debug, Default)]
pub struct TruncationOptions {
    pub max_lines: Option<usize>,
    pub max_bytes: Option<usize>,
}

/// UTF-8 字节长度(Rust 原生;对齐蓝本 runtimeBuffer.byteLength)。
pub fn utf8_byte_length(content: &str) -> usize {
    content.len()
}

/// 按行拆分用于计数;结尾换行不产生额外空行(对齐 TS `splitLinesForCounting`)。
fn split_lines_for_counting(content: &str) -> Vec<&str> {
    if content.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<&str> = content.split('\n').collect();
    if content.ends_with('\n') {
        lines.pop();
    }
    lines
}

/// 把字符串按字节从尾部截断,并对齐 UTF-8 字符边界(对齐 TS `truncateStringToBytesFromEnd`)。
fn truncate_string_to_bytes_from_end(text: &str, max_bytes: usize) -> String {
    if max_bytes == 0 {
        return String::new();
    }
    if text.len() <= max_bytes {
        return text.to_string();
    }
    // 从字节末尾回退到字符边界,保证 ≤ max_bytes 且不拆散多字节字符。
    let mut start = text.len() - max_bytes;
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    if start >= text.len() {
        return String::new();
    }
    text[start..].to_string()
}

/// 人类可读的字节大小(对齐 TS `formatSize`)。
pub fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// 从头部截断(保留前 N 行/字节;适合文件读取)。
pub fn truncate_head(content: &str, options: TruncationOptions) -> TruncationResult {
    let max_lines = options.max_lines.unwrap_or(DEFAULT_MAX_LINES);
    let max_bytes = options.max_bytes.unwrap_or(DEFAULT_MAX_BYTES);

    let total_bytes = utf8_byte_length(content);
    let lines = split_lines_for_counting(content);
    let total_lines = lines.len();

    if total_lines <= max_lines && total_bytes <= max_bytes {
        return TruncationResult {
            content: content.to_string(),
            truncated: false,
            truncated_by: None,
            total_lines,
            total_bytes,
            output_lines: total_lines,
            output_bytes: total_bytes,
            last_line_partial: false,
            first_line_exceeds_limit: false,
            max_lines,
            max_bytes,
        };
    }

    let first_line_bytes = utf8_byte_length(lines[0]);
    if first_line_bytes > max_bytes {
        return TruncationResult {
            content: String::new(),
            truncated: true,
            truncated_by: Some(TruncatedBy::Bytes),
            total_lines,
            total_bytes,
            output_lines: 0,
            output_bytes: 0,
            last_line_partial: false,
            first_line_exceeds_limit: true,
            max_lines,
            max_bytes,
        };
    }

    let mut output_lines: Vec<&str> = Vec::new();
    let mut output_bytes_count = 0usize;
    let mut truncated_by = TruncatedBy::Lines;

    for (index, line) in lines.iter().enumerate().take(max_lines) {
        // +1 为换行字节(首行除外)。
        let line_bytes = utf8_byte_length(line) + usize::from(index > 0);
        if output_bytes_count + line_bytes > max_bytes {
            truncated_by = TruncatedBy::Bytes;
            break;
        }
        output_lines.push(line);
        output_bytes_count += line_bytes;
    }

    if output_lines.len() >= max_lines && output_bytes_count <= max_bytes {
        truncated_by = TruncatedBy::Lines;
    }

    let output_content = output_lines.join("\n");
    let final_output_bytes = utf8_byte_length(&output_content);

    TruncationResult {
        content: output_content,
        truncated: true,
        truncated_by: Some(truncated_by),
        total_lines,
        total_bytes,
        output_lines: output_lines.len(),
        output_bytes: final_output_bytes,
        last_line_partial: false,
        first_line_exceeds_limit: false,
        max_lines,
        max_bytes,
    }
}

/// 从尾部截断(保留最后 N 行/字节;适合 bash 输出)。末行超限时允许部分保留。
pub fn truncate_tail(content: &str, options: TruncationOptions) -> TruncationResult {
    let max_lines = options.max_lines.unwrap_or(DEFAULT_MAX_LINES);
    let max_bytes = options.max_bytes.unwrap_or(DEFAULT_MAX_BYTES);

    let total_bytes = utf8_byte_length(content);
    let lines = split_lines_for_counting(content);
    let total_lines = lines.len();

    if total_lines <= max_lines && total_bytes <= max_bytes {
        return TruncationResult {
            content: content.to_string(),
            truncated: false,
            truncated_by: None,
            total_lines,
            total_bytes,
            output_lines: total_lines,
            output_bytes: total_bytes,
            last_line_partial: false,
            first_line_exceeds_limit: false,
            max_lines,
            max_bytes,
        };
    }

    let mut output_lines: Vec<String> = Vec::new();
    let mut output_bytes_count = 0usize;
    let mut truncated_by = TruncatedBy::Lines;
    let mut last_line_partial = false;

    for line in lines.iter().rev() {
        if output_lines.len() >= max_lines {
            break;
        }
        // +1 为换行字节(已收集行非空时)。
        let line_bytes = utf8_byte_length(line) + usize::from(!output_lines.is_empty());
        if output_bytes_count + line_bytes > max_bytes {
            truncated_by = TruncatedBy::Bytes;
            if output_lines.is_empty() {
                let truncated_line = truncate_string_to_bytes_from_end(line, max_bytes);
                output_bytes_count = utf8_byte_length(&truncated_line);
                output_lines.insert(0, truncated_line);
                last_line_partial = true;
            }
            break;
        }
        output_lines.insert(0, (*line).to_string());
        output_bytes_count += line_bytes;
    }

    if output_lines.len() >= max_lines && output_bytes_count <= max_bytes {
        truncated_by = TruncatedBy::Lines;
    }

    let output_content = output_lines.join("\n");
    let final_output_bytes = utf8_byte_length(&output_content);

    TruncationResult {
        content: output_content,
        truncated: true,
        truncated_by: Some(truncated_by),
        total_lines,
        total_bytes,
        output_lines: output_lines.len(),
        output_bytes: final_output_bytes,
        last_line_partial,
        first_line_exceeds_limit: false,
        max_lines,
        max_bytes,
    }
}

/// 单行截断到最大字符数并加 `[truncated]` 后缀(grep 匹配行用)。
pub fn truncate_line(line: &str, max_chars: usize) -> (String, bool) {
    if line.chars().count() <= max_chars {
        return (line.to_string(), false);
    }
    let text: String = line.chars().take(max_chars).collect();
    (format!("{text}... [truncated]"), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tail_by_bytes(input: &str, max_bytes: usize) -> String {
        let bytes = input.as_bytes();
        if bytes.len() <= max_bytes {
            return input.to_string();
        }
        let mut start = bytes.len() - max_bytes;
        while start < bytes.len() && (bytes[start] & 0xC0) == 0x80 {
            start += 1;
        }
        String::from_utf8_lossy(&bytes[start..]).to_string()
    }

    #[test]
    fn counts_utf8_bytes() {
        let content = "aé🙂\nb";
        let result = truncate_head(
            content,
            TruncationOptions {
                max_lines: Some(10),
                max_bytes: Some(100),
            },
        );
        assert!(!result.truncated);
        assert_eq!(result.total_bytes, 9);
        assert_eq!(result.output_bytes, 9);
        assert_eq!(result.total_lines, 2);
    }

    #[test]
    fn trailing_newline_is_not_an_extra_line() {
        let content = "line\nline\nline\n";
        let head = truncate_head(
            content,
            TruncationOptions {
                max_lines: Some(3),
                max_bytes: Some(100),
            },
        );
        let tail = truncate_tail(
            content,
            TruncationOptions {
                max_lines: Some(3),
                max_bytes: Some(100),
            },
        );
        assert!(!head.truncated);
        assert_eq!(head.total_lines, 3);
        assert_eq!(head.output_lines, 3);
        assert!(!tail.truncated);
        assert_eq!(tail.total_lines, 3);
    }

    #[test]
    fn head_truncates_by_lines() {
        let content = (0..10).map(|i| format!("line{i}")).collect::<Vec<_>>().join("\n");
        let result = truncate_head(
            &content,
            TruncationOptions {
                max_lines: Some(3),
                max_bytes: Some(1000),
            },
        );
        assert!(result.truncated);
        assert_eq!(result.truncated_by, Some(TruncatedBy::Lines));
        assert_eq!(result.output_lines, 3);
        assert_eq!(result.content, "line0\nline1\nline2");
    }

    #[test]
    fn head_truncates_by_bytes() {
        let content = "aaaa\nbbbb\ncccc";
        let result = truncate_head(
            content,
            TruncationOptions {
                max_lines: Some(10),
                max_bytes: Some(9),
            },
        );
        assert!(result.truncated);
        assert_eq!(result.truncated_by, Some(TruncatedBy::Bytes));
        // "aaaa"(4)+ "\n"(1)+"bbbb"(4) = 9 → 恰好放下。
        assert_eq!(result.content, "aaaa\nbbbb");
        assert_eq!(result.output_bytes, 9);
    }

    #[test]
    fn head_first_line_exceeds_limit() {
        let result = truncate_head(
            "aaaaaaaaaa\nb",
            TruncationOptions {
                max_lines: Some(10),
                max_bytes: Some(5),
            },
        );
        assert!(result.truncated);
        assert!(result.first_line_exceeds_limit);
        assert_eq!(result.content, "");
        assert_eq!(result.output_lines, 0);
    }

    #[test]
    fn tail_keeps_last_lines() {
        let content = "l1\nl2\nl3\nl4";
        let result = truncate_tail(
            content,
            TruncationOptions {
                max_lines: Some(2),
                max_bytes: Some(100),
            },
        );
        assert!(result.truncated);
        assert_eq!(result.truncated_by, Some(TruncatedBy::Lines));
        assert_eq!(result.content, "l3\nl4");
    }

    #[test]
    fn tail_matches_buffer_semantics_for_single_line_multibyte() {
        // 单行输入:尾截退化为字节截断,与按字节参考实现逐字节一致。
        let input = "aé🙂bcdefg🙂h";
        for max_bytes in [0usize, 1, 2, 3, 4, 5, 8, 11, 12, 13, 14, 15, 18, 19, 20, 21] {
            let result = truncate_tail(
                input,
                TruncationOptions {
                    max_lines: Some(10),
                    max_bytes: Some(max_bytes),
                },
            );
            let expected = tail_by_bytes(input, max_bytes);
            assert_eq!(result.content, expected, "mismatch at maxBytes={max_bytes}");
            assert!(result.output_bytes <= max_bytes, "exceeded at maxBytes={max_bytes}");
        }
    }

    #[test]
    fn tail_multibyte_lines_keep_line_semantics() {
        // 多行输入:行不可拆(除非末行单独超限)。
        let result = truncate_tail(
            "aé🙂\nbc\ndefg🙂h",
            TruncationOptions {
                max_lines: Some(10),
                max_bytes: Some(11),
            },
        );
        // 尾行 "defg🙂h"(9 字节)保留;上一行 "bc" 放不下。
        assert_eq!(result.content, "defg🙂h");
        assert_eq!(result.truncated_by, Some(TruncatedBy::Bytes));
    }

    #[test]
    fn tail_partial_last_line() {
        // 末行 10 字节超过 5 字节上限且无前序行 → 部分保留。
        let result = truncate_tail(
            "0123456789",
            TruncationOptions {
                max_lines: Some(10),
                max_bytes: Some(5),
            },
        );
        assert!(result.truncated);
        assert!(result.last_line_partial);
        assert_eq!(result.content, "56789");
    }

    #[test]
    fn truncate_line_limits_chars() {
        let (text, truncated) = truncate_line("abcdef", 4);
        assert!(truncated);
        assert_eq!(text, "abcd... [truncated]");
        let (text, truncated) = truncate_line("abc", 4);
        assert!(!truncated);
        assert_eq!(text, "abc");
    }

    #[test]
    fn format_size_matches_ts() {
        assert_eq!(format_size(512), "512B");
        assert_eq!(format_size(1024), "1.0KB");
        assert_eq!(format_size(50 * 1024), "50.0KB");
        assert_eq!(format_size(2 * 1024 * 1024), "2.0MB");
    }
}
