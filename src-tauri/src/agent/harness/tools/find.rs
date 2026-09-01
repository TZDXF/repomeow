//! find 工具:对齐 `packages/coding-agent/src/core/tools/find.ts`。
//!
//! 蓝本默认经 `fd --glob --color=never --hidden` 子进程;本实现用
//! `ignore::WalkBuilder + globset` 纯 Rust 等价:隐藏文件参与搜索、遵守
//! .gitignore(仓库外按 fd `--no-require-git` 语义仍生效、仓库内父级规则在
//! 嵌套仓库边界停止)。契约保持:默认 1000 条、输出相对搜索根的 POSIX 路径、
//! 含 `/` 的 pattern 按 fd `--full-path` 语义匹配并补 `**/` 前缀、50KB 截断。

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::harness::tools::path_utils::resolve_tool_path;
use crate::agent::harness::types::{ExecutionEnv, SimpleError};
use crate::agent::harness::utils::truncate::{
    format_size, truncate_head, TruncationOptions, TruncationResult, DEFAULT_MAX_BYTES,
};
use crate::agent::types::{AbortSignal, AgentTool, AgentToolResult, ToolExecutionError};

const DEFAULT_LIMIT: usize = 1000;

/// find 工具参数(对齐 TS `FindToolInput`)。
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindToolInput {
    pub pattern: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub limit: Option<f64>,
}

/// find 工具详情(对齐 TS `FindToolDetails`)。
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindToolDetails {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncation: Option<TruncationResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_limit_reached: Option<usize>,
}

/// 相对化并 POSIX 化(对齐 TS `relativizeFindResultPath`)。
fn relativize_result_path(result_path: &str, search_path: &str) -> String {
    let path = std::path::Path::new(result_path);
    let relative = if path.is_absolute() {
        match path.strip_prefix(search_path) {
            Ok(relative) => relative.to_string_lossy().to_string(),
            Err(_) => result_path.to_string(),
        }
    } else {
        result_path.to_string()
    };
    relative.replace('\\', "/")
}

/// 从搜索根向上找 `.git` 判定是否位于 git 仓库内(fd 据此决定
/// `--no-require-git`;ignore crate 对应 `require_git`)。
fn inside_git_repo(search_path: &str) -> bool {
    let mut current: Option<&std::path::Path> = Some(std::path::Path::new(search_path));
    while let Some(dir) = current {
        if dir.join(".git").exists() {
            return true;
        }
        current = dir.parent();
    }
    false
}

/// glob 匹配器:含 `/` 的 pattern 按 fd `--full-path` 语义(补 `**/` 前缀,
/// `literal_separator` 保证 `**` 跨目录、`*` 不跨目录);否则匹配 basename。
/// 候选路径统一 POSIX 化后匹配,跨平台无需分隔符类。
enum PathMatcher {
    FullPath(globset::GlobMatcher),
    BaseName(globset::GlobMatcher),
}

impl PathMatcher {
    fn build(pattern: &str) -> Result<Self, ToolExecutionError> {
        let mut effective = pattern.to_string();
        let full_path = pattern.contains('/');
        if full_path
            && !effective.starts_with('/')
            && !effective.starts_with("**/")
            && effective != "**"
        {
            effective = format!("**/{effective}");
        }
        let matcher = globset::GlobBuilder::new(&effective)
            .literal_separator(true)
            .build()
            .map_err(|error| {
                ToolExecutionError::from(SimpleError::new(format!("Invalid pattern: {error}")))
            })?
            .compile_matcher();
        if full_path {
            Ok(PathMatcher::FullPath(matcher))
        } else {
            Ok(PathMatcher::BaseName(matcher))
        }
    }

    fn is_match(&self, absolute_posix: &str, relative_posix: &str, file_name: &str) -> bool {
        match self {
            PathMatcher::FullPath(matcher) => {
                // 相对与绝对两种候选都参与,覆盖根相对化与绝对 pattern。
                matcher.is_match(relative_posix) || matcher.is_match(absolute_posix)
            }
            PathMatcher::BaseName(matcher) => matcher.is_match(file_name),
        }
    }
}

/// 阻塞遍历(调用方放 spawn_blocking)。
fn find_entries(
    root: &std::path::Path,
    matcher: &PathMatcher,
    limit: usize,
    signal: Option<&AbortSignal>,
) -> Result<Vec<String>, ToolExecutionError> {
    let mut builder = ignore::WalkBuilder::new(root);
    // fd --hidden:隐藏文件参与。
    builder.hidden(false);
    // 仓库外仍尊重 .gitignore(fd --no-require-git)。
    builder.require_git(inside_git_repo(root.to_string_lossy().as_ref()));

    let root_text = root.to_string_lossy().to_string();
    let mut results: Vec<String> = Vec::new();
    for entry in builder.build() {
        if let Some(signal) = signal {
            if signal.is_cancelled() {
                return Err(ToolExecutionError::from(SimpleError::new(
                    "Operation aborted",
                )));
            }
        }
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path == root {
            continue;
        }
        let absolute = path.to_string_lossy().to_string();
        let relative = relativize_result_path(&absolute, &root_text);
        let absolute_posix = absolute.replace('\\', "/");
        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        if matcher.is_match(&absolute_posix, &relative, &file_name) {
            results.push(relative);
            if results.len() >= limit {
                break;
            }
        }
    }
    // 稳定输出(蓝本依赖 fd 的遍历序;此处排序保证跨平台确定性)。
    results.sort_by_key(|path| path.to_lowercase());
    Ok(results)
}

/// 创建 find 工具(构造时捕获 env;返回 core AgentTool)。
pub fn create_find_tool(env: Arc<dyn ExecutionEnv>) -> AgentTool {
    AgentTool {
        name: "find".to_string(),
        label: "find".to_string(),
        description: format!(
            "Search for files by glob pattern. Returns matching file paths relative to the search directory. Respects .gitignore. Output is truncated to {DEFAULT_LIMIT} results or {}KB (whichever is hit first).",
            DEFAULT_MAX_BYTES / 1024
        ),
        parameters: json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern to match files, e.g. '*.ts', '**/*.json', or 'src/**/*.spec.ts'"
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in (default: current directory)"
                },
                "limit": {
                    "type": "number",
                    "description": "Maximum number of results (default: 1000)"
                }
            },
            "required": ["pattern"]
        }),
        execution_mode: None,
        prepare_arguments: None,
        execute: Arc::new(move |_tool_call_id, params, signal, _on_update| {
            let env = env.clone();
            Box::pin(async move {
                let input: FindToolInput = serde_json::from_value(params)
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
                if !env.exists(search_path.clone(), signal.clone()).await.unwrap_or(false) {
                    return Err(ToolExecutionError::from(SimpleError::new(format!(
                        "Path not found: {search_path}"
                    ))));
                }

                let effective_limit = input
                    .limit
                    .map(|limit| limit.max(0.0) as usize)
                    .unwrap_or(DEFAULT_LIMIT);
                let matcher = PathMatcher::build(&input.pattern)?;

                let root = std::path::PathBuf::from(&search_path);
                let signal_for_blocking = signal.clone();
                let results = tokio::task::spawn_blocking(move || {
                    find_entries(
                        &root,
                        &matcher,
                        effective_limit,
                        signal_for_blocking.as_ref(),
                    )
                })
                .await
                .map_err(|error| ToolExecutionError::from(SimpleError::new(error.to_string())))??;

                if results.is_empty() {
                    return Ok(AgentToolResult {
                        content: vec![crate::agent::types::TextOrImageContent::text(
                            "No files found matching pattern",
                        )],
                        ..Default::default()
                    });
                }

                let result_limit_reached = results.len() >= effective_limit;
                let raw_output = results.join("\n");
                let truncation = truncate_head(
                    &raw_output,
                    TruncationOptions {
                        max_lines: Some(usize::MAX),
                        max_bytes: None,
                    },
                );
                let mut output = truncation.content.clone();
                let mut notices: Vec<String> = Vec::new();
                if result_limit_reached {
                    notices.push(format!(
                        "{effective_limit} results limit reached. Use limit={} for more, or refine pattern",
                        effective_limit * 2
                    ));
                }
                if truncation.truncated {
                    notices.push(format!("{} limit reached", format_size(DEFAULT_MAX_BYTES)));
                }
                if !notices.is_empty() {
                    output.push_str(&format!("\n\n[{}]", notices.join(". ")));
                }

                let has_details = result_limit_reached || truncation.truncated;
                let details = FindToolDetails {
                    truncation: if truncation.truncated {
                        Some(truncation.clone())
                    } else {
                        None
                    },
                    result_limit_reached: if result_limit_reached {
                        Some(effective_limit)
                    } else {
                        None
                    },
                };
                Ok(AgentToolResult {
                    content: vec![crate::agent::types::TextOrImageContent::text(output)],
                    details: if has_details {
                        serde_json::to_value(&details).unwrap_or(Value::Null)
                    } else {
                        Value::Null
                    },
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
    use crate::agent::types::TextOrImageContent;

    fn text_of(result: &AgentToolResult) -> String {
        result
            .content
            .iter()
            .filter_map(|content| match content {
                TextOrImageContent::Text { text, .. } => Some(text.clone()),
                TextOrImageContent::Image { .. } => None,
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
                "repomeow-find-{}-{:?}",
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

        fn root_text(&self) -> String {
            self.root.to_string_lossy().replace('\\', "/")
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).ok();
        }
    }

    #[tokio::test]
    async fn basename_glob_matches_any_depth() {
        let project = TempProject::new();
        project.write("a.ts", "");
        project.write("src/nested/b.ts", "");
        project.write("src/c.md", "");
        let tool = create_find_tool(project.env());
        let result = (tool.execute)(
            "call-1".to_string(),
            json!({ "pattern": "*.ts" }),
            None,
            None,
        )
        .await
        .unwrap();
        let text = text_of(&result);
        assert!(text.contains("a.ts"), "{text}");
        assert!(text.contains("src/nested/b.ts"), "{text}");
        assert!(!text.contains("c.md"), "{text}");
    }

    #[tokio::test]
    async fn path_glob_requires_prefix_and_relates_to_root() {
        // 上游回归 3302:含路径段的 glob 按 --full-path 匹配,自动补 **/。
        let project = TempProject::new();
        project.write("src/a.spec.ts", "");
        project.write("lib/src/b.spec.ts", "");
        project.write("src/c.ts", "");
        let tool = create_find_tool(project.env());
        let result = (tool.execute)(
            "call-1".to_string(),
            json!({ "pattern": "src/**/*.spec.ts" }),
            None,
            None,
        )
        .await
        .unwrap();
        let text = text_of(&result);
        assert!(text.contains("src/a.spec.ts"), "{text}");
        assert!(text.contains("lib/src/b.spec.ts"), "{text}");
        assert!(!text.contains("src/c.ts"), "{text}");
    }

    #[tokio::test]
    async fn respects_nested_gitignore_boundaries() {
        // 上游回归 3303:嵌套仓库边界处父级 .gitignore 不再生效。
        let project = TempProject::new();
        project.write(".gitignore", "*.log\n");
        project.write("keep.txt", "");
        project.write("outer.log", "");
        // 嵌套 git 仓库:有自己的 .gitignore,允许 .log。
        project.write("sub/.git/HEAD", "ref: refs/heads/main\n");
        project.write("sub/.gitignore", "!*.log\n");
        project.write("sub/inner.log", "");
        let tool = create_find_tool(project.env());
        let result = (tool.execute)(
            "call-1".to_string(),
            json!({ "pattern": "**/*" }),
            None,
            None,
        )
        .await
        .unwrap();
        let text = text_of(&result);
        assert!(text.contains("keep.txt"), "{text}");
        assert!(text.contains("sub/inner.log"), "{text}");
        assert!(!text.contains("outer.log"), "{text}");
    }

    #[tokio::test]
    async fn gitignore_applies_outside_repo() {
        // 上游回归 3303 的仓库外半区:无 .git 也遵守 .gitignore(fd --no-require-git)。
        let project = TempProject::new();
        project.write(".gitignore", "ignored.txt\n");
        project.write("ignored.txt", "");
        project.write("kept.txt", "");
        let tool = create_find_tool(project.env());
        let result = (tool.execute)(
            "call-1".to_string(),
            json!({ "pattern": "*.txt" }),
            None,
            None,
        )
        .await
        .unwrap();
        let text = text_of(&result);
        assert!(text.contains("kept.txt"), "{text}");
        assert!(!text.contains("ignored.txt"), "{text}");
    }

    #[tokio::test]
    async fn results_relative_to_search_root() {
        // 上游回归 6104:path 指向子目录时,结果相对该搜索根。
        let project = TempProject::new();
        project.write("outer.txt", "");
        project.write("sub/inner.txt", "");
        project.write("sub/deep/x.ts", "");
        let tool = create_find_tool(project.env());
        let result = (tool.execute)(
            "call-1".to_string(),
            json!({ "pattern": "*.txt", "path": "sub" }),
            None,
            None,
        )
        .await
        .unwrap();
        let text = text_of(&result);
        assert!(text.contains("inner.txt"), "{text}");
        assert!(!text.contains("outer.txt"), "{text}");
        assert!(!text.starts_with(&project.root_text()), "{text}");
    }

    #[tokio::test]
    async fn includes_hidden_and_directories() {
        let project = TempProject::new();
        project.write(".hidden-file", "");
        project.write("src/deep/file.ts", "");
        let tool = create_find_tool(project.env());
        let result = (tool.execute)(
            "call-1".to_string(),
            json!({ "pattern": "**/*" }),
            None,
            None,
        )
        .await
        .unwrap();
        let text = text_of(&result);
        assert!(text.contains(".hidden-file"), "{text}");
        assert!(text.contains("src/"), "{text}");
        assert!(text.contains("src/deep/file.ts"), "{text}");
    }

    #[tokio::test]
    async fn limit_notice() {
        let project = TempProject::new();
        project.write("a.txt", "");
        project.write("b.txt", "");
        project.write("c.txt", "");
        let tool = create_find_tool(project.env());
        let result = (tool.execute)(
            "call-1".to_string(),
            json!({ "pattern": "*.txt", "limit": 2 }),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(result.details["resultLimitReached"], 2);
        assert!(text_of(&result).contains("2 results limit reached"));
    }

    #[tokio::test]
    async fn missing_path_errors() {
        let project = TempProject::new();
        let tool = create_find_tool(project.env());
        let error = (tool.execute)(
            "call-1".to_string(),
            json!({ "pattern": "*.ts", "path": "missing" }),
            None,
            None,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("Path not found"), "{error}");
    }

    #[tokio::test]
    async fn invalid_glob_errors() {
        let project = TempProject::new();
        project.write("a.ts", "");
        let tool = create_find_tool(project.env());
        let error = (tool.execute)(
            "call-1".to_string(),
            json!({ "pattern": "[" }),
            None,
            None,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("Invalid pattern"), "{error}");
    }
}
