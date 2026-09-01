//! grep 工具:对齐 `packages/coding-agent/src/core/tools/grep.ts`。
//!
//! 蓝本经 `rg --json --line-number --color=never --hidden` 子进程搜索;本实现用
//! `ignore::WalkBuilder + regex` 纯 Rust 等价(默认 require-git 与 rg 的仓库判定
//! 一致,`--hidden` 对应 `hidden(false)`,`--glob` 对应 overrides 白/黑名单),
//! 不依赖运行时下载 ripgrep。输出契约保持:相对搜索根的 `/` 分隔路径、
//! `path:line: text` / `path-line- text`(context)、默认 100 条匹配、单行
//! 500 字符、总量 50KB 截断与 details 通知。

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::harness::tools::path_utils::resolve_tool_path;
use crate::agent::harness::types::{ExecutionEnv, FileKind, SimpleError};
use crate::agent::harness::utils::truncate::{
    format_size, truncate_head, truncate_line, TruncationOptions, TruncationResult,
    DEFAULT_MAX_BYTES, GREP_MAX_LINE_LENGTH,
};
use crate::agent::types::{AbortSignal, AgentTool, AgentToolResult, ToolExecutionError};

const DEFAULT_LIMIT: usize = 100;
/// 超过该大小的文件跳过(蓝本靠 rg 流式读取;全量读入需设上限防内存放大)。
const MAX_SEARCH_FILE_BYTES: u64 = 32 * 1024 * 1024;
/// 二进制探测窗口(与 rg 的 NUL 探测近似)。
const BINARY_SNIFF_BYTES: usize = 8192;

/// grep 工具参数(对齐 TS `GrepToolInput`)。
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrepToolInput {
    pub pattern: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub glob: Option<String>,
    #[serde(default)]
    pub ignore_case: Option<bool>,
    #[serde(default)]
    pub literal: Option<bool>,
    #[serde(default)]
    pub context: Option<f64>,
    #[serde(default)]
    pub limit: Option<f64>,
}

/// grep 工具详情(对齐 TS `GrepToolDetails`)。
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrepToolDetails {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncation: Option<TruncationResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_limit_reached: Option<usize>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub lines_truncated: bool,
}

struct RawMatch {
    file_path: String,
    line_number: usize,
    line_text: Option<String>,
}

/// 原始行(已归一化 \r)是否命中正则。
fn line_matches(matcher: &regex::Regex, line: &str) -> bool {
    matcher.is_match(line)
}

/// 归一化行序(剥离 \r,与蓝本 getFileLines 的 CRLF 归一一致)。
fn normalized_lines(content: &str) -> Vec<String> {
    content
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .map(str::to_string)
        .collect()
}

fn is_binary_content(bytes: &[u8]) -> bool {
    let window = &bytes[..bytes.len().min(BINARY_SNIFF_BYTES)];
    window.contains(&0)
}

/// 目录搜索(阻塞;调用方放 spawn_blocking)。
fn search_directory(
    root: &std::path::Path,
    matcher: &regex::Regex,
    glob: Option<&str>,
    limit: usize,
    signal: Option<&AbortSignal>,
) -> Result<Vec<RawMatch>, ToolExecutionError> {
    let mut builder = ignore::WalkBuilder::new(root);
    // rg --hidden:搜索隐藏文件但仍遵守 .gitignore。
    builder.hidden(false);
    if let Some(glob) = glob {
        let mut overrides = ignore::overrides::OverrideBuilder::new(root);
        overrides.add(glob).map_err(|error| {
            ToolExecutionError::from(SimpleError::new(format!("Invalid glob: {error}")))
        })?;
        let overrides = overrides.build().map_err(|error| {
            ToolExecutionError::from(SimpleError::new(format!("Invalid glob: {error}")))
        })?;
        builder.overrides(overrides);
    }

    let mut matches: Vec<RawMatch> = Vec::new();
    for entry in builder.build() {
        if let Some(signal) = signal {
            if signal.is_cancelled() {
                return Err(ToolExecutionError::from(SimpleError::new(
                    "Operation aborted",
                )));
            }
        }
        let Ok(entry) = entry else { continue };
        // rg 默认不跟随符号链接,只搜常规文件。
        let file_type = match entry.file_type() {
            Some(file_type) => file_type,
            None => continue,
        };
        if file_type.is_symlink() || !file_type.is_file() {
            continue;
        }
        let path = entry.path();
        let Ok(metadata) = std::fs::metadata(path) else {
            continue;
        };
        if metadata.len() > MAX_SEARCH_FILE_BYTES {
            continue;
        }
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        if is_binary_content(&bytes) {
            continue;
        }
        let content = String::from_utf8_lossy(&bytes);
        for (index, line) in normalized_lines(&content).into_iter().enumerate() {
            if line_matches(matcher, &line) {
                matches.push(RawMatch {
                    file_path: path.to_string_lossy().to_string(),
                    line_number: index + 1,
                    line_text: Some(line.to_string()),
                });
                if matches.len() >= limit {
                    return Ok(matches);
                }
            }
        }
    }
    Ok(matches)
}

fn format_display_path(file_path: &str, search_path: &str, is_directory: bool) -> String {
    if is_directory {
        if let Ok(relative) = std::path::Path::new(file_path).strip_prefix(search_path) {
            let text = relative.to_string_lossy().replace('\\', "/");
            if !text.is_empty() && !text.starts_with("../") {
                return text;
            }
        }
    }
    std::path::Path::new(file_path)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| file_path.to_string())
}

/// 创建 grep 工具(构造时捕获 env;返回 core AgentTool)。
pub fn create_grep_tool(env: Arc<dyn ExecutionEnv>) -> AgentTool {
    AgentTool {
        name: "grep".to_string(),
        label: "grep".to_string(),
        description: format!(
            "Search file contents for a pattern. Returns matching lines with file paths and line numbers. Respects .gitignore. Output is truncated to {DEFAULT_LIMIT} matches or {}KB (whichever is hit first). Long lines are truncated to {GREP_MAX_LINE_LENGTH} chars.",
            DEFAULT_MAX_BYTES / 1024
        ),
        parameters: json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Search pattern (regex or literal string)"
                },
                "path": {
                    "type": "string",
                    "description": "Directory or file to search (default: current directory)"
                },
                "glob": {
                    "type": "string",
                    "description": "Filter files by glob pattern, e.g. '*.ts' or '**/*.spec.ts'"
                },
                "ignoreCase": {
                    "type": "boolean",
                    "description": "Case-insensitive search (default: false)"
                },
                "literal": {
                    "type": "boolean",
                    "description": "Treat pattern as literal string instead of regex (default: false)"
                },
                "context": {
                    "type": "number",
                    "description": "Number of lines to show before and after each match (default: 0)"
                },
                "limit": {
                    "type": "number",
                    "description": "Maximum number of matches to return (default: 100)"
                }
            },
            "required": ["pattern"]
        }),
        execution_mode: None,
        prepare_arguments: None,
        execute: Arc::new(move |_tool_call_id, params, signal, _on_update| {
            let env = env.clone();
            Box::pin(async move {
                let input: GrepToolInput = serde_json::from_value(params)
                    .map_err(|error| ToolExecutionError::from(SimpleError::new(error.to_string())))?;
                if let Some(signal) = &signal {
                    if signal.is_cancelled() {
                        return Err(ToolExecutionError::from(SimpleError::new(
                            "Operation aborted",
                        )));
                    }
                }

                let search_input = input.path.as_deref().unwrap_or("").trim();
                let search_path = resolve_tool_path(
                    env.as_ref(),
                    if search_input.is_empty() {
                        "."
                    } else {
                        search_input
                    },
                    signal.clone(),
                )
                .await
                .map_err(|error| {
                    ToolExecutionError::from(SimpleError::new(error.to_string()))
                })?;
                let info =
                    env.file_info(search_path.clone()).await.map_err(|_| {
                        ToolExecutionError::from(SimpleError::new(format!(
                            "Path not found: {search_path}"
                        )))
                    })?;
                let is_directory = info.kind == FileKind::Directory;

                let context_value = input.context.unwrap_or(0.0).max(0.0) as usize;
                let effective_limit = (input.limit.unwrap_or(DEFAULT_LIMIT as f64).max(1.0)) as usize;

                let pattern_source = if input.literal.unwrap_or(false) {
                    regex::escape(&input.pattern)
                } else {
                    input.pattern.clone()
                };
                let mut regex_builder = regex::RegexBuilder::new(&pattern_source);
                if input.ignore_case.unwrap_or(false) {
                    regex_builder.case_insensitive(true);
                }
                let matcher = regex_builder.build().map_err(|error| {
                    ToolExecutionError::from(SimpleError::new(format!(
                        "Invalid pattern: {error}"
                    )))
                })?;

                let match_limit_reached;
                let matches: Vec<RawMatch> = if is_directory {
                    let root = std::path::PathBuf::from(&search_path);
                    let glob = input.glob.clone();
                    let matcher_for_blocking = matcher.clone();
                    let signal_for_blocking = signal.clone();
                    let limit_for_blocking = effective_limit;
                    // 阻塞遍历放专用线程池,不占用异步执行器。
                    let result = tokio::task::spawn_blocking(move || {
                        search_directory(
                            &root,
                            &matcher_for_blocking,
                            glob.as_deref(),
                            limit_for_blocking,
                            signal_for_blocking.as_ref(),
                        )
                    })
                    .await
                    .map_err(|error| {
                        ToolExecutionError::from(SimpleError::new(error.to_string()))
                    })??;
                    match_limit_reached = result.len() >= effective_limit;
                    result
                } else {
                    let bytes = env
                        .read_binary_file(search_path.clone(), signal.clone())
                        .await
                        .map_err(|_| {
                            ToolExecutionError::from(SimpleError::new(format!(
                                "Path not found: {search_path}"
                            )))
                        })?;
                    if is_binary_content(&bytes) {
                        match_limit_reached = false;
                        Vec::new()
                    } else {
                        let content = String::from_utf8_lossy(&bytes);
                        let mut found: Vec<RawMatch> = Vec::new();
                        for (index, line) in normalized_lines(&content).into_iter().enumerate() {
                            if line_matches(&matcher, &line) {
                                found.push(RawMatch {
                                    file_path: search_path.clone(),
                                    line_number: index + 1,
                                    line_text: Some(line),
                                });
                                if found.len() >= effective_limit {
                                    break;
                                }
                            }
                        }
                        match_limit_reached = found.len() >= effective_limit;
                        found
                    }
                };

                if matches.is_empty() {
                    return Ok(AgentToolResult {
                        content: vec![crate::agent::types::TextOrImageContent::text(
                            "No matches found",
                        )],
                        ..Default::default()
                    });
                }

                let display_cache: Arc<std::sync::Mutex<HashMap<String, String>>> =
                    Arc::new(std::sync::Mutex::new(HashMap::new()));
                let format_path = |file_path: &str| -> String {
                    let mut cache = display_cache
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    cache
                        .entry(file_path.to_string())
                        .or_insert_with(|| {
                            format_display_path(file_path, &search_path, is_directory)
                        })
                        .clone()
                };

                // context > 0 时按文件缓存行序重读(对齐蓝本 formatBlock)。
                let mut lines_cache: HashMap<String, Vec<String>> = HashMap::new();
                let mut output_lines: Vec<String> = Vec::new();
                let mut lines_truncated = false;
                for raw in &matches {
                    let relative_path = format_path(&raw.file_path);
                    if context_value == 0 {
                        let sanitized = raw
                            .line_text
                            .clone()
                            .unwrap_or_default()
                            .replace('\r', "");
                        let (text, was_truncated) = truncate_line(&sanitized, GREP_MAX_LINE_LENGTH);
                        if was_truncated {
                            lines_truncated = true;
                        }
                        output_lines.push(format!(
                            "{relative_path}:{}: {text}",
                            raw.line_number
                        ));
                    } else {
                        let lines = match lines_cache.get(&raw.file_path) {
                            Some(lines) => lines.clone(),
                            None => {
                                let lines: Vec<String> = match env
                                    .read_text_file(raw.file_path.clone(), None)
                                    .await
                                {
                                    Ok(content) => normalized_lines(&content),
                                    Err(_) => Vec::new(),
                                };
                                lines_cache.insert(raw.file_path.clone(), lines.clone());
                                lines
                            }
                        };
                        if lines.is_empty() {
                            output_lines
                                .push(format!("{relative_path}:{}: (unable to read file)", raw.line_number));
                            continue;
                        }
                        let start = if context_value > 0 {
                            raw.line_number.saturating_sub(context_value).max(1)
                        } else {
                            raw.line_number
                        };
                        let end = (raw.line_number + context_value).min(lines.len());
                        for current in start..=end {
                            let line_text = lines.get(current - 1).map(String::as_str).unwrap_or("");
                            let (text, was_truncated) =
                                truncate_line(line_text, GREP_MAX_LINE_LENGTH);
                            if was_truncated {
                                lines_truncated = true;
                            }
                            if current == raw.line_number {
                                output_lines.push(format!("{relative_path}:{current}: {text}"));
                            } else {
                                output_lines.push(format!("{relative_path}-{current}- {text}"));
                            }
                        }
                    }
                }

                let raw_output = output_lines.join("\n");
                let truncation = truncate_head(
                    &raw_output,
                    TruncationOptions {
                        max_lines: Some(usize::MAX),
                        max_bytes: None,
                    },
                );
                let mut output = truncation.content.clone();
                let details = GrepToolDetails {
                    truncation: if truncation.truncated {
                        Some(truncation.clone())
                    } else {
                        None
                    },
                    match_limit_reached: if match_limit_reached {
                        Some(effective_limit)
                    } else {
                        None
                    },
                    lines_truncated,
                };
                let mut notices: Vec<String> = Vec::new();
                if match_limit_reached {
                    notices.push(format!(
                        "{effective_limit} matches limit reached. Use limit={} for more, or refine pattern",
                        effective_limit * 2
                    ));
                }
                if truncation.truncated {
                    notices.push(format!("{} limit reached", format_size(DEFAULT_MAX_BYTES)));
                }
                if lines_truncated {
                    notices.push(format!(
                        "Some lines truncated to {GREP_MAX_LINE_LENGTH} chars. Use read tool to see full lines"
                    ));
                }
                if !notices.is_empty() {
                    output.push_str(&format!("\n\n[{}]", notices.join(". ")));
                }

                let details_value = if details.truncation.is_some()
                    || details.match_limit_reached.is_some()
                    || details.lines_truncated
                {
                    serde_json::to_value(&details).unwrap_or(Value::Null)
                } else {
                    Value::Null
                };
                Ok(AgentToolResult {
                    content: vec![crate::agent::types::TextOrImageContent::text(output)],
                    details: details_value,
                    ..Default::default()
                })
            })
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::harness::env::TokioEnv;

    fn text_of(result: &AgentToolResult) -> String {
        result
            .content
            .iter()
            .filter_map(|content| match content {
                crate::agent::types::TextOrImageContent::Text { text, .. } => Some(text.clone()),
                crate::agent::types::TextOrImageContent::Image { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    struct TempProject {
        root: std::path::PathBuf,
    }

    impl TempProject {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "repomeow-grep-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn write(&self, relative: &str, content: &str) {
            let path = self.root.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, content).unwrap();
        }

        fn env(&self) -> Arc<dyn ExecutionEnv> {
            Arc::new(TokioEnv::new(self.root.clone()))
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).ok();
        }
    }

    #[tokio::test]
    async fn finds_matches_with_relative_paths() {
        let project = TempProject::new();
        project.write("src/a.ts", "const alpha = 1;\nconst beta = 2;\n");
        project.write("docs/b.md", "alpha mentioned\n");
        let tool = create_grep_tool(project.env());
        let result = (tool.execute)(
            "call-1".to_string(),
            json!({ "pattern": "alpha" }),
            None,
            None,
        )
        .await
        .unwrap();
        let text = text_of(&result);
        assert!(text.contains("src/a.ts:1: const alpha = 1;"), "{text}");
        assert!(text.contains("docs/b.md:1: alpha mentioned"), "{text}");
    }

    #[tokio::test]
    async fn glob_filters_files() {
        let project = TempProject::new();
        project.write("src/a.ts", "target\n");
        project.write("src/a.md", "target\n");
        let tool = create_grep_tool(project.env());
        let result = (tool.execute)(
            "call-1".to_string(),
            json!({ "pattern": "target", "glob": "*.ts" }),
            None,
            None,
        )
        .await
        .unwrap();
        let text = text_of(&result);
        assert!(text.contains("src/a.ts:1: target"), "{text}");
        assert!(!text.contains("a.md"), "{text}");
    }

    #[tokio::test]
    async fn respects_gitignore_and_searches_hidden() {
        let project = TempProject::new();
        project.write(".gitignore", "ignored.txt\n");
        project.write("ignored.txt", "secret\n");
        project.write(".hidden", "secret\n");
        project.write("normal.txt", "secret\n");
        let tool = create_grep_tool(project.env());
        let result = (tool.execute)(
            "call-1".to_string(),
            json!({ "pattern": "secret" }),
            None,
            None,
        )
        .await
        .unwrap();
        let text = text_of(&result);
        assert!(text.contains("normal.txt"), "{text}");
        assert!(text.contains(".hidden"), "{text}");
        assert!(!text.contains("ignored.txt"), "{text}");
    }

    #[tokio::test]
    async fn literal_and_ignore_case() {
        let project = TempProject::new();
        project.write("a.txt", "Hello World\n");
        let tool = create_grep_tool(project.env());
        // 正则语义:未转义的 `.` 是通配符。
        let result = (tool.execute)(
            "call-1".to_string(),
            json!({ "pattern": "hello.world", "ignoreCase": true }),
            None,
            None,
        )
        .await
        .unwrap();
        assert!(text_of(&result).contains("a.txt:1: Hello World"));
        // literal 语义:按字面量匹配,不命中。
        let result = (tool.execute)(
            "call-2".to_string(),
            json!({ "pattern": "hello.world", "ignoreCase": true, "literal": true }),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(text_of(&result), "No matches found");
    }

    #[tokio::test]
    async fn context_lines_with_block_format() {
        let project = TempProject::new();
        project.write("a.txt", "one\ntwo\nthree\nfour\nfive\n");
        let tool = create_grep_tool(project.env());
        let result = (tool.execute)(
            "call-1".to_string(),
            json!({ "pattern": "three", "context": 1 }),
            None,
            None,
        )
        .await
        .unwrap();
        let text = text_of(&result);
        assert!(text.contains("a.txt-2- two"), "{text}");
        assert!(text.contains("a.txt:3: three"), "{text}");
        assert!(text.contains("a.txt-4- four"), "{text}");
    }

    #[tokio::test]
    async fn limit_reached_notice() {
        let project = TempProject::new();
        project.write("a.txt", "hit\nhit\nhit\n");
        let tool = create_grep_tool(project.env());
        let result = (tool.execute)(
            "call-1".to_string(),
            json!({ "pattern": "hit", "limit": 2 }),
            None,
            None,
        )
        .await
        .unwrap();
        let text = text_of(&result);
        assert!(result.details.is_object(), "{:?}", result.details);
        assert_eq!(result.details["matchLimitReached"], 2);
        assert!(text.contains("2 matches limit reached"), "{text}");
    }

    #[tokio::test]
    async fn long_line_truncated() {
        let project = TempProject::new();
        let long_line = "x".repeat(600);
        project.write("a.txt", &long_line);
        let tool = create_grep_tool(project.env());
        let result = (tool.execute)(
            "call-1".to_string(),
            json!({ "pattern": "xxx" }),
            None,
            None,
        )
        .await
        .unwrap();
        let text = text_of(&result);
        assert!(text.contains("[truncated]"), "{text}");
        assert_eq!(result.details["linesTruncated"], true);
    }

    #[tokio::test]
    async fn missing_path_errors() {
        let project = TempProject::new();
        let tool = create_grep_tool(project.env());
        let error = (tool.execute)(
            "call-1".to_string(),
            json!({ "pattern": "x", "path": "missing-dir" }),
            None,
            None,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("Path not found"), "{error}");
    }

    #[tokio::test]
    async fn invalid_regex_errors() {
        let project = TempProject::new();
        project.write("a.txt", "x\n");
        let tool = create_grep_tool(project.env());
        let error = (tool.execute)(
            "call-1".to_string(),
            json!({ "pattern": "(unclosed" }),
            None,
            None,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("Invalid pattern"), "{error}");
    }

    #[tokio::test]
    async fn single_file_search() {
        let project = TempProject::new();
        project.write("a.txt", "foo\nbar\nfoo\n");
        let tool = create_grep_tool(project.env());
        let result = (tool.execute)(
            "call-1".to_string(),
            json!({ "pattern": "foo", "path": "a.txt" }),
            None,
            None,
        )
        .await
        .unwrap();
        let text = text_of(&result);
        assert!(text.contains("a.txt:1: foo"), "{text}");
        assert!(text.contains("a.txt:3: foo"), "{text}");
    }
}
