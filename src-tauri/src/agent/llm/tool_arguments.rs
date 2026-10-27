//! 流式工具参数：保留原始 JSON，生成容错预览，结束时一次性定稿。
//!
//! 对齐 pi-ai 的 `ToolCallArgumentsStream` / `parseToolCallArguments`。
//! 小参数保持逐 delta 预览；大参数采用几何增长快照，避免每个 delta
//! 都对累计全文做修复和解析而产生平方级开销。最终定稿始终读取完整原文。

use serde_json::{Map, Value};

pub(crate) fn escape_control_character(ch: char) -> String {
    match ch {
        '\u{8}' => "\\b".to_string(),
        '\u{c}' => "\\f".to_string(),
        '\n' => "\\n".to_string(),
        '\r' => "\\r".to_string(),
        '\t' => "\\t".to_string(),
        _ => format!("\\u{:04x}", ch as u32),
    }
}

/// TS repairJson:转义字符串内的裸控制字符、把非法转义的双重化。
pub(crate) fn repair_json(json_text: &str) -> String {
    let mut repaired = String::with_capacity(json_text.len());
    let chars: Vec<char> = json_text.chars().collect();
    let mut in_string = false;
    let mut index = 0usize;
    while index < chars.len() {
        let ch = chars[index];
        if !in_string {
            repaired.push(ch);
            if ch == '"' {
                in_string = true;
            }
            index += 1;
            continue;
        }
        if ch == '"' {
            repaired.push(ch);
            in_string = false;
            index += 1;
            continue;
        }
        if ch == '\\' {
            match chars.get(index + 1) {
                None => {
                    repaired.push_str("\\\\");
                    index += 1;
                }
                Some('u') => {
                    let hex: String = chars
                        .get(index + 2..index + 6)
                        .unwrap_or(&[])
                        .iter()
                        .collect();
                    if hex.chars().count() == 4 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                        repaired.push_str("\\u");
                        repaired.push_str(&hex);
                        index += 6;
                    } else {
                        repaired.push_str("\\\\");
                        index += 1;
                    }
                }
                Some(next) if matches!(next, '"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't') => {
                    repaired.push('\\');
                    repaired.push(*next);
                    index += 2;
                }
                Some(_) => {
                    repaired.push_str("\\\\");
                    index += 1;
                }
            }
            continue;
        }
        if (ch as u32) < 0x20 {
            repaired.push_str(&escape_control_character(ch));
        } else {
            repaired.push(ch);
        }
        index += 1;
    }
    repaired
}

/// 扫描部分 JSON,生成「补全闭合」候选并返回首个可解析者。
/// 处理:未闭合字符串(值保留部分文本/键丢弃)、未闭合容器、悬挂 `,`/`:`,
/// 尾部残缺字面量(true/数字残片)。
fn close_partial_json(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut stack: Vec<JsonFrame> = Vec::new();
    let mut in_string = false;
    let mut string_start = 0usize;
    let mut escaped = false;

    let mut index = 0usize;
    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            if escaped {
                escaped = false;
            } else {
                match byte {
                    b'\\' => escaped = true,
                    b'"' => in_string = false,
                    _ => {}
                }
            }
            index += 1;
            continue;
        }
        match byte {
            b'"' => {
                in_string = true;
                string_start = index;
            }
            b'{' => stack.push(JsonFrame::Object),
            b'[' => stack.push(JsonFrame::Array),
            b'}' | b']' => {
                stack.pop();
            }
            _ => {}
        }
        index += 1;
    }

    let mut candidates: Vec<String> = Vec::new();
    if in_string {
        // 候选 1:保留字符串部分内容后闭合
        let mut kept = input.to_string();
        if escaped {
            kept.pop();
        }
        kept.push('"');
        candidates.push(kept);
        // 候选 2:丢弃整段未完成字符串(键或值)
        candidates.push(input[..string_start].to_string());
    } else {
        // 候选 1:尾部字面量可能已完整
        candidates.push(input.to_string());
        // 候选 2:截掉尾部残缺字面量(true/数字残片等)
        let cut = input
            .rfind(['{', '}', '[', ']', '"', ':', ','])
            .map_or(0, |position| position + 1);
        candidates.push(input[..cut].to_string());
    }

    for candidate in candidates {
        if let Some(closed) = close_candidate(candidate, &stack) {
            if serde_json::from_str::<Value>(&closed).is_ok() {
                return Some(closed);
            }
        }
    }
    None
}

#[derive(Clone, Copy)]
enum JsonFrame {
    Object,
    Array,
}

/// 清理悬挂分隔符并按栈补全闭合括号;返回 None 表示无法修复。
fn close_candidate(mut text: String, stack: &[JsonFrame]) -> Option<String> {
    loop {
        text = text.trim_end().to_string();
        if text.ends_with(',') {
            text.pop();
            continue;
        }
        if text.ends_with(':') {
            // 丢弃悬挂的键:回退到该键的起始引号,可能再次暴露逗号
            let bytes = text.as_bytes();
            let colon_index = bytes.len() - 1;
            let mut probe = colon_index;
            while probe > 0 && bytes[probe - 1].is_ascii_whitespace() {
                probe -= 1;
            }
            if probe == 0 || bytes[probe - 1] != b'"' {
                return None;
            }
            let closing_quote = probe - 1;
            let mut opening_quote = None;
            let mut scan = closing_quote;
            while scan > 0 {
                scan -= 1;
                if bytes[scan] == b'"' {
                    let mut backslashes = 0usize;
                    let mut back = scan;
                    while back > 0 && bytes[back - 1] == b'\\' {
                        backslashes += 1;
                        back -= 1;
                    }
                    if backslashes.is_multiple_of(2) {
                        opening_quote = Some(scan);
                        break;
                    }
                }
            }
            let opening_quote = opening_quote?;
            text.truncate(opening_quote);
            continue;
        }
        break;
    }
    for frame in stack.iter().rev() {
        text.push(match frame {
            JsonFrame::Object => '}',
            JsonFrame::Array => ']',
        });
    }
    Some(text)
}

pub(crate) fn parse_streaming_json_object(partial: &str) -> Map<String, Value> {
    let trimmed = partial.trim();
    if trimmed.is_empty() {
        return Map::new();
    }
    let repaired = repair_json(trimmed);
    let closed_original = close_partial_json(trimmed);
    let closed_repaired = close_partial_json(&repaired);
    let direct = [trimmed, repaired.as_str()];
    for candidate in direct
        .into_iter()
        .chain(closed_original.iter().map(String::as_str))
        .chain(closed_repaired.iter().map(String::as_str))
    {
        if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(candidate) {
            return map;
        }
    }
    Map::new()
}

/// 单个工具调用的参数流。原文只追加，不写入 partial message 的临时解析状态。
#[derive(Clone, Debug, Default)]
pub(crate) struct ToolCallArguments {
    raw: String,
    preview: Map<String, Value>,
    next_preview_len: usize,
    preview_count: usize,
}

impl ToolCallArguments {
    const PREVIEW_ALWAYS_UNTIL: usize = 8 * 1024;

    pub(crate) fn push(&mut self, delta: &str) {
        self.raw.push_str(delta);
        if self.raw.len() < self.next_preview_len {
            return;
        }
        self.preview = parse_streaming_json_object(&self.raw);
        self.preview_count += 1;
        self.next_preview_len = if self.raw.len() <= Self::PREVIEW_ALWAYS_UNTIL {
            self.raw.len() + 1
        } else {
            // 每次预览之后至少再接收 25%，累计解析工作量保持 O(n)。
            self.raw.len() + (self.raw.len() / 4).max(1024)
        };
    }

    pub(crate) fn preview(&self) -> Map<String, Value> {
        self.preview.clone()
    }

    pub(crate) fn raw(&self) -> &str {
        &self.raw
    }

    pub(crate) fn replace_raw(&mut self, raw: &str) {
        self.raw.clear();
        self.raw.push_str(raw);
        self.next_preview_len = 0;
    }

    pub(crate) fn finish(self) -> (Map<String, Value>, Option<String>) {
        parse_tool_call_arguments(&self.raw)
    }
}

/// 最终参数只接受完整 JSON 对象；非法 JSON 不会退化成部分可执行参数。
pub(crate) fn parse_tool_call_arguments(raw: &str) -> (Map<String, Value>, Option<String>) {
    if raw.trim().is_empty() {
        return (Map::new(), None);
    }
    let direct = serde_json::from_str::<Value>(raw);
    let repaired = repair_json(raw);
    let parsed = direct.or_else(|_| serde_json::from_str::<Value>(&repaired));
    match parsed {
        Ok(Value::Object(arguments)) => (arguments, None),
        Ok(_) => tool_arguments_error("Tool arguments must be a JSON object", raw),
        Err(error) => tool_arguments_error(&error.to_string(), raw),
    }
}

fn tool_arguments_error(message: &str, raw: &str) -> (Map<String, Value>, Option<String>) {
    (
        Map::new(),
        Some(format!("{message}\n\nReceived arguments:\n{raw}")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn final_arguments_require_a_complete_object() {
        let (arguments, error) = parse_tool_call_arguments(r#"{"path":"a.rs","content":"abc"#);
        assert!(arguments.is_empty());
        assert!(error.is_some_and(|error| error.contains("Received arguments")));

        let (arguments, error) = parse_tool_call_arguments("[1,2]");
        assert!(arguments.is_empty());
        assert!(error.is_some_and(|error| error.contains("JSON object")));
    }

    #[test]
    fn final_arguments_tolerate_invalid_unicode_escapes() {
        let (arguments, error) =
            parse_tool_call_arguments(r#"{"content":"\\usepackage{amsmath}"}"#);
        assert!(error.is_none());
        assert_eq!(arguments["content"], json!(r"\usepackage{amsmath}"));
    }

    #[test]
    fn large_arguments_do_not_reparse_every_delta_and_finish_exactly() {
        let content = "喵".repeat(40_000);
        let raw = serde_json::to_string(&json!({"content": content})).unwrap();
        let mut stream = ToolCallArguments::default();
        let chars: Vec<char> = raw.chars().collect();
        for delta in chars.chunks(64) {
            stream.push(&delta.iter().collect::<String>());
        }
        assert!(
            stream.preview_count < 100,
            "preview count: {}",
            stream.preview_count
        );
        let (arguments, error) = stream.finish();
        assert!(error.is_none());
        assert_eq!(
            arguments["content"].as_str().unwrap().chars().count(),
            40_000
        );
    }
}
