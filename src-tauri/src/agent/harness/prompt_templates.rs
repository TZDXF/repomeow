//! 提示词模板加载与参数替换:对齐 `packages/agent/src/harness/prompt-templates.ts`。

use regex::Regex;
use serde_json::Value;

use crate::agent::harness::types::{
    err, ok, ExecutionEnv, FileErrorCode, FileInfo, FileKind, PromptTemplate, Result,
};

/// 模板诊断码(对齐 TS `PromptTemplateDiagnosticCode`)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptTemplateDiagnosticCode {
    FileInfoFailed,
    ListFailed,
    ReadFailed,
    ParseFailed,
}

impl std::fmt::Display for PromptTemplateDiagnosticCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            PromptTemplateDiagnosticCode::FileInfoFailed => "file_info_failed",
            PromptTemplateDiagnosticCode::ListFailed => "list_failed",
            PromptTemplateDiagnosticCode::ReadFailed => "read_failed",
            PromptTemplateDiagnosticCode::ParseFailed => "parse_failed",
        };
        f.write_str(text)
    }
}

/// 加载模板时产生的警告(对齐 TS `PromptTemplateDiagnostic`)。
#[derive(Clone, Debug)]
pub struct PromptTemplateDiagnostic {
    pub code: PromptTemplateDiagnosticCode,
    pub message: String,
    pub path: String,
}

pub struct PromptTemplatesLoadResult {
    pub prompt_templates: Vec<PromptTemplate>,
    pub diagnostics: Vec<PromptTemplateDiagnostic>,
}

/// 从一个或多个路径加载模板:目录取直接 `.md` 子项(非递归),文件显式加载;
/// 缺失路径与非 markdown 文件跳过(对齐 TS `loadPromptTemplates`)。
pub async fn load_prompt_templates(env: &dyn ExecutionEnv, paths: &[&str]) -> PromptTemplatesLoadResult {
    let mut prompt_templates: Vec<PromptTemplate> = Vec::new();
    let mut diagnostics: Vec<PromptTemplateDiagnostic> = Vec::new();
    for path in paths {
        let path = path.to_string();
        let info = env.file_info(path.clone()).await;
        let info = match info {
            Ok(info) => info,
            Err(error) => {
                if error.code != FileErrorCode::NotFound {
                    diagnostics.push(PromptTemplateDiagnostic {
                        code: PromptTemplateDiagnosticCode::FileInfoFailed,
                        message: error.message,
                        path,
                    });
                }
                continue;
            }
        };
        let kind = resolve_kind(env, &info, &mut diagnostics).await;
        if kind == Some(FileKind::Directory) {
            let result = load_templates_from_dir(env, &info.path).await;
            prompt_templates.extend(result.prompt_templates);
            diagnostics.extend(result.diagnostics);
        } else if kind == Some(FileKind::File) && info.name.ends_with(".md") {
            let (prompt_template, template_diagnostics) =
                load_template_from_file(env, &info.path, &info.name).await;
            if let Some(prompt_template) = prompt_template {
                prompt_templates.push(prompt_template);
            }
            diagnostics.extend(template_diagnostics);
        }
    }
    PromptTemplatesLoadResult {
        prompt_templates,
        diagnostics,
    }
}

async fn load_templates_from_dir(
    env: &dyn ExecutionEnv,
    dir: &str,
) -> PromptTemplatesLoadResult {
    let mut prompt_templates: Vec<PromptTemplate> = Vec::new();
    let mut diagnostics: Vec<PromptTemplateDiagnostic> = Vec::new();
    let entries = env.list_dir(dir.to_string(), None).await;
    let entries = match entries {
        Ok(entries) => entries,
        Err(error) => {
            diagnostics.push(PromptTemplateDiagnostic {
                code: PromptTemplateDiagnosticCode::ListFailed,
                message: error.message,
                path: dir.to_string(),
            });
            return PromptTemplatesLoadResult {
                prompt_templates,
                diagnostics,
            };
        }
    };

    let mut sorted_entries = entries;
    sorted_entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    for entry in &sorted_entries {
        let kind = resolve_kind(env, entry, &mut diagnostics).await;
        if kind != Some(FileKind::File) || !entry.name.ends_with(".md") {
            continue;
        }
        let (prompt_template, template_diagnostics) =
            load_template_from_file(env, &entry.path, &entry.name).await;
        if let Some(prompt_template) = prompt_template {
            prompt_templates.push(prompt_template);
        }
        diagnostics.extend(template_diagnostics);
    }
    PromptTemplatesLoadResult {
        prompt_templates,
        diagnostics,
    }
}

async fn load_template_from_file(
    env: &dyn ExecutionEnv,
    file_path: &str,
    file_name: &str,
) -> (Option<PromptTemplate>, Vec<PromptTemplateDiagnostic>) {
    let mut diagnostics: Vec<PromptTemplateDiagnostic> = Vec::new();
    let raw_content = env.read_text_file(file_path.to_string(), None).await;
    let raw_content = match raw_content {
        Ok(content) => content,
        Err(error) => {
            diagnostics.push(PromptTemplateDiagnostic {
                code: PromptTemplateDiagnosticCode::ReadFailed,
                message: error.message,
                path: file_path.to_string(),
            });
            return (None, diagnostics);
        }
    };

    let parsed = parse_frontmatter(&raw_content);
    let (frontmatter, body) = match parsed {
        Ok(parsed) => parsed,
        Err(error) => {
            diagnostics.push(PromptTemplateDiagnostic {
                code: PromptTemplateDiagnosticCode::ParseFailed,
                message: error.message,
                path: file_path.to_string(),
            });
            return (None, diagnostics);
        }
    };

    let first_line = body.lines().find(|line| !line.trim().is_empty());
    let mut description = frontmatter
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if description.is_empty() {
        if let Some(first_line) = first_line {
            description = first_line.chars().take(60).collect();
            if first_line.chars().count() > 60 {
                description.push_str("...");
            }
        }
    }
    let name = strip_md_extension(file_name);
    (
        Some(PromptTemplate {
            name,
            description: if description.is_empty() {
                None
            } else {
                Some(description)
            },
            content: body,
        }),
        diagnostics,
    )
}

fn strip_md_extension(file_name: &str) -> String {
    // 对齐 TS `fileName.replace(/\.md$/i, "")`。
    let lower = file_name.to_lowercase();
    if lower.ends_with(".md") {
        file_name[..file_name.len() - 3].to_string()
    } else {
        file_name.to_string()
    }
}

async fn resolve_kind(
    env: &dyn ExecutionEnv,
    info: &FileInfo,
    diagnostics: &mut Vec<PromptTemplateDiagnostic>,
) -> Option<FileKind> {
    if info.kind == FileKind::File || info.kind == FileKind::Directory {
        return Some(info.kind);
    }
    let canonical_path = env.canonical_path(info.path.clone(), None).await;
    let canonical_path = match canonical_path {
        Ok(path) => path,
        Err(error) => {
            if error.code != FileErrorCode::NotFound {
                diagnostics.push(PromptTemplateDiagnostic {
                    code: PromptTemplateDiagnosticCode::FileInfoFailed,
                    message: error.message,
                    path: info.path.clone(),
                });
            }
            return None;
        }
    };
    let target = env.file_info(canonical_path).await;
    match target {
        Ok(target) => {
            if target.kind == FileKind::File || target.kind == FileKind::Directory {
                Some(target.kind)
            } else {
                None
            }
        }
        Err(error) => {
            if error.code != FileErrorCode::NotFound {
                diagnostics.push(PromptTemplateDiagnostic {
                    code: PromptTemplateDiagnosticCode::FileInfoFailed,
                    message: error.message,
                    path: info.path.clone(),
                });
            }
            None
        }
    }
}

/// frontmatter 解析(与 skills.rs 同一约定;独立实现保持模块对齐)。
pub fn parse_frontmatter(content: &str) -> Result<(serde_json::Map<String, Value>, String), SimpleTemplateError> {
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    if !normalized.starts_with("---") {
        return ok((serde_json::Map::new(), normalized));
    }
    let Some(end_index) = normalized[3..].find("\n---").map(|offset| offset + 3) else {
        return ok((serde_json::Map::new(), normalized));
    };
    let yaml_string = if end_index >= 4 {
        &normalized[4..end_index]
    } else {
        ""
    };
    let body = normalized[end_index + 4..].trim().to_string();
    let parsed: serde_json::Value = serde_yaml_ng::from_str(yaml_string)
        .map_err(|error| SimpleTemplateError {
            message: error.to_string(),
        })?;
    let object = match parsed {
        Value::Null => serde_json::Map::new(),
        Value::Object(map) => map,
        _ => {
            return err(SimpleTemplateError {
                message: "frontmatter is not a mapping".to_string(),
            })
        }
    };
    ok((object, body))
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct SimpleTemplateError {
    pub message: String,
}

/// shell 风格单/双引号参数解析(对齐 TS `parseCommandArgs`)。
pub fn parse_command_args(args_string: &str) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_quote: Option<char> = None;

    for char in args_string.chars() {
        if let Some(quote) = in_quote {
            if char == quote {
                in_quote = None;
            } else {
                current.push(char);
            }
        } else if char == '"' || char == '\'' {
            in_quote = Some(char);
        } else if char == ' ' || char == '\t' {
            if !current.is_empty() {
                args.push(std::mem::take(&mut current));
            }
        } else {
            current.push(char);
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

/// 替换模板占位符:`$1`、`$@`、`$ARGUMENTS`、`${@:N}`、`${@:N:L}`
/// (对齐 TS `substituteArgs`;替换顺序一致)。
pub fn substitute_args(content: &str, args: &[String]) -> String {
    let positional = Regex::new(r"\$(\d+)").expect("valid regex");
    let slice = Regex::new(r"\$\{@:(\d+)(?::(\d+))?\}").expect("valid regex");
    let all_args = args.join(" ");

    let mut result = positional
        .replace_all(content, |captures: &regex::Captures| {
            let num: usize = captures[1].parse().unwrap_or(0);
            args.get(num.saturating_sub(1)).cloned().unwrap_or_default()
        })
        .to_string();
    result = slice
        .replace_all(&result, |captures: &regex::Captures| {
            let start: usize = captures[1].parse().unwrap_or(0);
            let start = start.saturating_sub(1);
            match captures.get(2).and_then(|m| m.as_str().parse::<usize>().ok()) {
                Some(length) => args
                    .iter()
                    .skip(start)
                    .take(length)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" "),
                None => args.iter().skip(start).cloned().collect::<Vec<_>>().join(" "),
            }
        })
        .to_string();
    result = result.replace("$ARGUMENTS", &all_args);
    result.replace("$@", &all_args)
}

/// 用位置参数格式化模板调用(对齐 TS `formatPromptTemplateInvocation`)。
pub fn format_prompt_template_invocation(template: &PromptTemplate, args: &[String]) -> String {
    substitute_args(&template.content, args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_command_args_with_quotes() {
        assert_eq!(parse_command_args("a b c"), vec!["a", "b", "c"]);
        assert_eq!(
            parse_command_args(r#"fix "the bug" now"#),
            vec!["fix", "the bug", "now"]
        );
        assert_eq!(
            parse_command_args("'single quoted'  double"),
            vec!["single quoted", "double"]
        );
        assert_eq!(parse_command_args("   "), Vec::<String>::new());
        assert_eq!(
            parse_command_args("trim\t tabs"),
            vec!["trim", "tabs"]
        );
    }

    #[test]
    fn substitutes_positional_and_slices() {
        let args = vec!["one".to_string(), "two".to_string(), "three".to_string()];
        assert_eq!(substitute_args("$1 then $3", &args), "one then three");
        assert_eq!(substitute_args("$9", &args), "");
        assert_eq!(substitute_args("${@:2}", &args), "two three");
        assert_eq!(substitute_args("${@:2:1}", &args), "two");
        assert_eq!(substitute_args("${@:1:2}", &args), "one two");
        assert_eq!(substitute_args("$ARGUMENTS!", &args), "one two three!");
        assert_eq!(substitute_args("$@", &args), "one two three");
    }

    #[test]
    fn parses_template_frontmatter() {
        let content = "---\ndescription: Runs the build\nargument-hint: \"[target]\"\n---\nBuild it.";
        let (frontmatter, body) = parse_frontmatter(content).unwrap();
        assert_eq!(frontmatter.get("description").and_then(Value::as_str), Some("Runs the build"));
        assert_eq!(frontmatter.get("argument-hint").and_then(Value::as_str), Some("[target]"));
        assert_eq!(body, "Build it.");
    }

    #[test]
    fn template_name_strips_md_case_insensitively() {
        assert_eq!(strip_md_extension("Review.MD"), "Review");
        assert_eq!(strip_md_extension("review.md"), "review");
        assert_eq!(strip_md_extension("other.txt"), "other.txt");
    }
}
