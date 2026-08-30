//! 技能加载:对齐 `packages/agent/src/harness/skills.ts`。
//!
//! SKILL.md frontmatter 用 `serde_yaml_ng` 解析;目录遍历语义按蓝本(每目录只取
//! 首个 SKILL.md、递归子目录、根目录 .md 也按 frontmatter 技能解析、支持
//! .gitignore/.ignore/.fdignore)。

use std::sync::Arc;

use ignore::gitignore::GitignoreBuilder;
use serde_json::Value;

use crate::agent::harness::types::{
    err, ok, ExecutionEnv, FileErrorCode, FileInfo, FileKind, Result, Skill,
};

const MAX_NAME_LENGTH: usize = 64;
const MAX_DESCRIPTION_LENGTH: usize = 1024;
const IGNORE_FILE_NAMES: [&str; 3] = [".gitignore", ".ignore", ".fdignore"];

/// 技能诊断码(对齐 TS `SkillDiagnosticCode`)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkillDiagnosticCode {
    FileInfoFailed,
    ListFailed,
    ReadFailed,
    ParseFailed,
    InvalidMetadata,
}

impl std::fmt::Display for SkillDiagnosticCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            SkillDiagnosticCode::FileInfoFailed => "file_info_failed",
            SkillDiagnosticCode::ListFailed => "list_failed",
            SkillDiagnosticCode::ReadFailed => "read_failed",
            SkillDiagnosticCode::ParseFailed => "parse_failed",
            SkillDiagnosticCode::InvalidMetadata => "invalid_metadata",
        };
        f.write_str(text)
    }
}

/// 加载技能时产生的警告(对齐 TS `SkillDiagnostic`)。
#[derive(Clone, Debug)]
pub struct SkillDiagnostic {
    pub code: SkillDiagnosticCode,
    pub message: String,
    pub path: String,
}

/// 目录发现根(任务允许参数化;默认可传任意根目录)。
#[derive(Clone, Debug)]
pub struct SkillsLoadResult {
    pub skills: Vec<Skill>,
    pub diagnostics: Vec<SkillDiagnostic>,
}

/// 格式化技能调用 prompt(对齐 TS `formatSkillInvocation`)。
pub fn format_skill_invocation(skill: &Skill, additional_instructions: Option<&str>) -> String {
    let skill_block = format!(
        "<skill name=\"{}\" location=\"{}\">\nReferences are relative to {}.\n\n{}\n</skill>",
        skill.name,
        skill.file_path,
        dirname_env_path(&skill.file_path),
        skill.content
    );
    match additional_instructions {
        Some(instructions) => format!("{skill_block}\n\n{instructions}"),
        None => skill_block,
    }
}

/// 从一个或多个目录加载技能(对齐 TS `loadSkills`;缺失目录跳过)。
pub async fn load_skills(
    env: &dyn ExecutionEnv,
    dirs: &[&str],
) -> SkillsLoadResult {
    let mut skills: Vec<Skill> = Vec::new();
    let mut diagnostics: Vec<SkillDiagnostic> = Vec::new();
    for dir in dirs {
        let dir = dir.to_string();
        let root_info = env.file_info(dir.clone()).await;
        let root_info = match root_info {
            Ok(info) => info,
            Err(error) => {
                if error.code != FileErrorCode::NotFound {
                    diagnostics.push(SkillDiagnostic {
                        code: SkillDiagnosticCode::FileInfoFailed,
                        message: error.message,
                        path: dir,
                    });
                }
                continue;
            }
        };
        if resolve_kind(env, &root_info, &mut diagnostics).await != Some(FileKind::Directory) {
            continue;
        }
        let result = load_skills_from_dir_internal(
            env,
            &root_info.path,
            true,
            Arc::new(tokio::sync::Mutex::new(GitignoreState::new())),
            &root_info.path,
        )
        .await;
        skills.extend(result.skills);
        diagnostics.extend(result.diagnostics);
    }
    SkillsLoadResult { skills, diagnostics }
}

/// Gitignore 匹配器:复用 `ignore` crate 的 gitignore 语义。
struct GitignoreState {
    builder: GitignoreBuilder,
    /// 已编译 matcher(首次 use 时构建;增量 add 后失效重建)。
    compiled: Option<ignore::gitignore::Gitignore>,
    version: usize,
    compiled_version: usize,
}

impl GitignoreState {
    fn new() -> Self {
        Self {
            builder: GitignoreBuilder::new(""),
            compiled: None,
            version: 0,
            compiled_version: 0,
        }
    }

    fn add_lines(&mut self, lines: &[String]) {
        for line in lines {
            let _ = self.builder.add_line(None, line);
        }
        self.version += 1;
    }

    fn ignores(&mut self, path: &str, is_dir: bool) -> bool {
        if self.compiled.is_none() || self.compiled_version != self.version {
            self.compiled = Some(self.builder.build().unwrap_or_else(|_| {
                GitignoreBuilder::new("")
                    .build()
                    .expect("empty gitignore builder always builds")
            }));
            self.compiled_version = self.version;
        }
        matches!(
            self.compiled
                .as_ref()
                .expect("compiled above")
                .matched(path, is_dir),
            ignore::Match::Ignore(_)
        )
    }
}

async fn load_skills_from_dir_internal(
    env: &dyn ExecutionEnv,
    dir: &str,
    include_root_files: bool,
    ignore_matcher: Arc<tokio::sync::Mutex<GitignoreState>>,
    root_dir: &str,
) -> SkillsLoadResult {
    let mut skills: Vec<Skill> = Vec::new();
    let mut diagnostics: Vec<SkillDiagnostic> = Vec::new();

    let dir_info = env.file_info(dir.to_string()).await;
    let dir_info = match dir_info {
        Ok(info) => info,
        Err(error) => {
            if error.code != FileErrorCode::NotFound {
                diagnostics.push(SkillDiagnostic {
                    code: SkillDiagnosticCode::FileInfoFailed,
                    message: error.message,
                    path: dir.to_string(),
                });
            }
            return SkillsLoadResult { skills, diagnostics };
        }
    };
    if resolve_kind(env, &dir_info, &mut diagnostics).await != Some(FileKind::Directory) {
        return SkillsLoadResult { skills, diagnostics };
    }

    add_ignore_rules(env, &ignore_matcher, dir, root_dir, &mut diagnostics).await;

    let entries = env.list_dir(dir.to_string(), None).await;
    let entries = match entries {
        Ok(entries) => entries,
        Err(error) => {
            diagnostics.push(SkillDiagnostic {
                code: SkillDiagnosticCode::ListFailed,
                message: error.message,
                path: dir.to_string(),
            });
            return SkillsLoadResult { skills, diagnostics };
        }
    };

    // 每目录只取一个 SKILL.md(首个命中即返回;与蓝本一致)。
    for entry in &entries {
        if entry.name != "SKILL.md" {
            continue;
        }
        let full_path = entry.path.clone();
        let kind = resolve_kind(env, entry, &mut diagnostics).await;
        if kind != Some(FileKind::File) {
            continue;
        }
        let rel_path = relative_env_path(root_dir, &full_path);
        if ignore_matcher
            .lock()
            .await
            .ignores(&rel_path, false)
        {
            continue;
        }
        let (skill, skill_diagnostics) = load_skill_from_file(env, &full_path, &dir_info.name).await;
        if let Some(skill) = skill {
            skills.push(skill);
        }
        diagnostics.extend(skill_diagnostics);
        return SkillsLoadResult { skills, diagnostics };
    }

    let mut sorted_entries = entries;
    sorted_entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    for entry in &sorted_entries {
        if entry.name.starts_with('.') || entry.name == "node_modules" {
            continue;
        }
        let full_path = entry.path.clone();
        let Some(kind) = resolve_kind(env, entry, &mut diagnostics).await else {
            continue;
        };

        let rel_path = relative_env_path(root_dir, &full_path);
        let ignore_path = if kind == FileKind::Directory {
            format!("{rel_path}/")
        } else {
            rel_path
        };
        if ignore_matcher
            .lock()
            .await
            .ignores(&ignore_path, kind == FileKind::Directory)
        {
            continue;
        }

        if kind == FileKind::Directory {
            let nested = Box::pin(load_skills_from_dir_internal(
                env,
                &full_path,
                false,
                ignore_matcher.clone(),
                root_dir,
            ))
            .await;
            skills.extend(nested.skills);
            diagnostics.extend(nested.diagnostics);
            continue;
        }

        if kind != FileKind::File || !include_root_files || !entry.name.ends_with(".md") {
            continue;
        }
        let (skill, skill_diagnostics) = load_skill_from_file(env, &full_path, &dir_info.name).await;
        if let Some(skill) = skill {
            skills.push(skill);
        }
        diagnostics.extend(skill_diagnostics);
    }

    SkillsLoadResult { skills, diagnostics }
}

async fn add_ignore_rules(
    env: &dyn ExecutionEnv,
    ignore_matcher: &Arc<tokio::sync::Mutex<GitignoreState>>,
    dir: &str,
    root_dir: &str,
    diagnostics: &mut Vec<SkillDiagnostic>,
) {
    let relative_dir = relative_env_path(root_dir, dir);
    let prefix = if relative_dir.is_empty() {
        String::new()
    } else {
        format!("{relative_dir}/")
    };

    for filename in IGNORE_FILE_NAMES {
        let ignore_path = match env.join_path(vec![dir.to_string(), filename.to_string()], None).await
        {
            Ok(path) => path,
            Err(error) => {
                diagnostics.push(SkillDiagnostic {
                    code: SkillDiagnosticCode::FileInfoFailed,
                    message: error.message,
                    path: dir.to_string(),
                });
                continue;
            }
        };
        let info = env.file_info(ignore_path.clone()).await;
        let info = match info {
            Ok(info) => info,
            Err(error) => {
                if error.code != FileErrorCode::NotFound {
                    diagnostics.push(SkillDiagnostic {
                        code: SkillDiagnosticCode::FileInfoFailed,
                        message: error.message,
                        path: ignore_path,
                    });
                }
                continue;
            }
        };
        if info.kind != FileKind::File {
            continue;
        }
        let content = env.read_text_file(ignore_path.clone(), None).await;
        let content = match content {
            Ok(content) => content,
            Err(error) => {
                diagnostics.push(SkillDiagnostic {
                    code: SkillDiagnosticCode::ReadFailed,
                    message: error.message,
                    path: ignore_path,
                });
                continue;
            }
        };
        let patterns: Vec<String> = content
            .lines()
            .filter_map(|line| prefix_ignore_pattern(line, &prefix))
            .collect();
        if !patterns.is_empty() {
            ignore_matcher.lock().await.add_lines(&patterns);
        }
    }
}

fn prefix_ignore_pattern(line: &str, prefix: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with('#') && !trimmed.starts_with("\\#") {
        return None;
    }

    let mut pattern = line.to_string();
    let mut negated = false;
    if pattern.starts_with('!') {
        negated = true;
        pattern = pattern[1..].to_string();
    } else if pattern.starts_with("\\!") {
        pattern = pattern[1..].to_string();
    }
    if pattern.starts_with('/') {
        pattern = pattern[1..].to_string();
    }
    let prefixed = if prefix.is_empty() {
        pattern
    } else {
        format!("{prefix}{pattern}")
    };
    Some(if negated {
        format!("!{prefixed}")
    } else {
        prefixed
    })
}

/// 解析 frontmatter + 正文(对齐 TS `parseFrontmatter` 的 `---` 分隔约定)。
pub fn parse_frontmatter(content: &str) -> Result<(serde_json::Map<String, Value>, String), SimpleSkillError> {
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    if !normalized.starts_with("---") {
        return ok((serde_json::Map::new(), normalized));
    }
    let Some(end_index) = normalized[3..].find("\n---").map(|offset| offset + 3) else {
        return ok((serde_json::Map::new(), normalized));
    };
    // 对齐 JS slice 语义:end < start 时 yaml 为空串。
    let yaml_string = if end_index >= 4 {
        &normalized[4..end_index]
    } else {
        ""
    };
    let body = normalized[end_index + 4..].trim().to_string();
    let parsed: serde_json::Value = serde_yaml_ng::from_str(yaml_string)
        .map_err(|error| SimpleSkillError {
            message: error.to_string(),
        })?;
    let object = match parsed {
        Value::Null => serde_json::Map::new(),
        Value::Object(map) => map,
        _ => {
            return err(SimpleSkillError {
                message: "frontmatter is not a mapping".to_string(),
            })
        }
    };
    ok((object, body))
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct SimpleSkillError {
    pub message: String,
}

async fn load_skill_from_file(
    env: &dyn ExecutionEnv,
    file_path: &str,
    parent_dir_name: &str,
) -> (Option<Skill>, Vec<SkillDiagnostic>) {
    let mut diagnostics: Vec<SkillDiagnostic> = Vec::new();
    let is_declared_skill = file_path
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .map(|name| name == "SKILL.md")
        .unwrap_or(false);
    let raw_content = env.read_text_file(file_path.to_string(), None).await;
    let raw_content = match raw_content {
        Ok(content) => content,
        Err(error) => {
            diagnostics.push(SkillDiagnostic {
                code: SkillDiagnosticCode::ReadFailed,
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
            if is_declared_skill {
                diagnostics.push(SkillDiagnostic {
                    code: SkillDiagnosticCode::ParseFailed,
                    message: error.message,
                    path: file_path.to_string(),
                });
            }
            return (None, diagnostics);
        }
    };

    let description = frontmatter
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_string);
    if !is_declared_skill
        && description.as_ref().map(|d| d.trim().is_empty()).unwrap_or(true)
    {
        return (None, diagnostics);
    }

    for error in validate_description(description.as_deref()) {
        diagnostics.push(SkillDiagnostic {
            code: SkillDiagnosticCode::InvalidMetadata,
            message: error,
            path: file_path.to_string(),
        });
    }

    let frontmatter_name = frontmatter
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_string);
    let name = frontmatter_name.unwrap_or_else(|| parent_dir_name.to_string());
    for error in validate_name(&name, parent_dir_name) {
        diagnostics.push(SkillDiagnostic {
            code: SkillDiagnosticCode::InvalidMetadata,
            message: error,
            path: file_path.to_string(),
        });
    }

    if description.as_ref().map(|d| d.trim().is_empty()).unwrap_or(true) {
        return (None, diagnostics);
    }

    (
        Some(Skill {
            name,
            description: description.unwrap_or_default(),
            content: body,
            file_path: file_path.to_string(),
            disable_model_invocation: frontmatter
                .get("disable-model-invocation")
                .and_then(Value::as_bool),
        }),
        diagnostics,
    )
}

fn validate_name(name: &str, parent_dir_name: &str) -> Vec<String> {
    let mut errors = Vec::new();
    if name != parent_dir_name {
        errors.push(format!(
            "name \"{name}\" does not match parent directory \"{parent_dir_name}\""
        ));
    }
    if name.chars().count() > MAX_NAME_LENGTH {
        errors.push(format!(
            "name exceeds {MAX_NAME_LENGTH} characters ({})",
            name.chars().count()
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        errors.push("name contains invalid characters (must be lowercase a-z, 0-9, hyphens only)".to_string());
    }
    if name.starts_with('-') || name.ends_with('-') {
        errors.push("name must not start or end with a hyphen".to_string());
    }
    if name.contains("--") {
        errors.push("name must not contain consecutive hyphens".to_string());
    }
    errors
}

fn validate_description(description: Option<&str>) -> Vec<String> {
    match description {
        None => vec!["description is required".to_string()],
        Some(description) if description.trim().is_empty() => {
            vec!["description is required".to_string()]
        }
        Some(description) => {
            let length = description.chars().count();
            if length > MAX_DESCRIPTION_LENGTH {
                vec![format!(
                    "description exceeds {MAX_DESCRIPTION_LENGTH} characters ({length})"
                )]
            } else {
                Vec::new()
            }
        }
    }
}

async fn resolve_kind(
    env: &dyn ExecutionEnv,
    info: &FileInfo,
    diagnostics: &mut Vec<SkillDiagnostic>,
) -> Option<FileKind> {
    if info.kind == FileKind::File || info.kind == FileKind::Directory {
        return Some(info.kind);
    }
    // symlink:显式 canonicalPath 再判目标类别。
    let canonical_path = env.canonical_path(info.path.clone(), None).await;
    let canonical_path = match canonical_path {
        Ok(path) => path,
        Err(error) => {
            if error.code != FileErrorCode::NotFound {
                diagnostics.push(SkillDiagnostic {
                    code: SkillDiagnosticCode::FileInfoFailed,
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
                diagnostics.push(SkillDiagnostic {
                    code: SkillDiagnosticCode::FileInfoFailed,
                    message: error.message,
                    path: info.path.clone(),
                });
            }
            None
        }
    }
}

/// 环境路径的 dirname(接受 `/` 与 `\`;对齐 TS `dirnameEnvPath`)。
pub fn dirname_env_path(path: &str) -> String {
    let normalized = path.trim_end_matches(['/', '\\']);
    let separator_index = normalized
        .rfind('/')
        .map(|i| i)
        .into_iter()
        .chain(normalized.rfind('\\'))
        .max();
    match separator_index {
        Some(index) if index == 2 && normalized.as_bytes().get(1) == Some(&b':') => {
            normalized[..3].to_string()
        }
        Some(index) if index > 0 => normalized[..index].to_string(),
        Some(_) => "/".to_string(),
        None => "/".to_string(),
    }
}

/// 相对路径(归一化为 `/`;对齐 TS `relativeEnvPath`)。
pub fn relative_env_path(root: &str, path: &str) -> String {
    let normalized_root: String = root.replace('\\', "/");
    let normalized_root = normalized_root.trim_end_matches('/');
    let normalized_path: String = path.replace('\\', "/");
    let normalized_path = normalized_path.trim_end_matches('/');
    if normalized_path == normalized_root {
        return String::new();
    }
    let root_prefix = format!("{normalized_root}/");
    if let Some(stripped) = normalized_path.strip_prefix(&root_prefix) {
        stripped.to_string()
    } else {
        normalized_path
            .trim_start_matches('/')
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_skill_frontmatter() {
        let content = "---\nname: commit-helper\ndescription: Helps with commits\ndisable-model-invocation: true\n---\n\nBody text.";
        let (frontmatter, body) = parse_frontmatter(content).unwrap();
        assert_eq!(frontmatter.get("name").and_then(Value::as_str), Some("commit-helper"));
        assert_eq!(
            frontmatter.get("description").and_then(Value::as_str),
            Some("Helps with commits")
        );
        assert_eq!(
            frontmatter.get("disable-model-invocation").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(body, "Body text.");
    }

    #[test]
    fn frontmatter_without_delimiters_is_body() {
        let (frontmatter, body) = parse_frontmatter("Just body").unwrap();
        assert!(frontmatter.is_empty());
        assert_eq!(body, "Just body");
        let (frontmatter, body) = parse_frontmatter("---\nno end marker").unwrap();
        assert!(frontmatter.is_empty());
        assert_eq!(body, "---\nno end marker");
    }

    #[test]
    fn invalid_yaml_is_error() {
        let content = "---\nname: [unclosed\n---\nbody";
        let result = parse_frontmatter(content);
        assert!(result.is_err());
    }

    #[test]
    fn validates_name_rules() {
        assert!(validate_name("commit-helper", "commit-helper").is_empty());
        assert_eq!(validate_name("other", "commit-helper").len(), 1);
        assert!(validate_name("Bad_Name", "Bad_Name")
            .iter()
            .any(|e| e.contains("invalid characters")));
        assert!(validate_name("-bad-", "-bad-")
            .iter()
            .any(|e| e.contains("start or end")));
        assert!(validate_name("a--b", "a--b")
            .iter()
            .any(|e| e.contains("consecutive hyphens")));
    }

    #[test]
    fn validates_description_rules() {
        assert!(validate_description(Some("ok")).is_empty());
        assert_eq!(validate_description(None), vec!["description is required"]);
        assert_eq!(
            validate_description(Some("   ")),
            vec!["description is required"]
        );
        let long = "x".repeat(1025);
        assert!(validate_description(Some(&long))
            .iter()
            .any(|e| e.contains("exceeds")));
    }

    #[test]
    fn relative_and_dirname_paths() {
        assert_eq!(relative_env_path("/root", "/root/sub/dir"), "sub/dir");
        assert_eq!(relative_env_path("/root", "/root"), "");
        assert_eq!(relative_env_path("C:\\root", "C:\\root\\file.md"), "file.md");
        assert_eq!(dirname_env_path("/root/sub/SKILL.md"), "/root/sub");
        assert_eq!(dirname_env_path("C:\\skills\\x\\SKILL.md"), "C:\\skills\\x");
        assert_eq!(dirname_env_path("/SKILL.md"), "/");
    }

    #[test]
    fn formats_skill_invocation() {
        let skill = Skill {
            name: "commit".into(),
            description: "d".into(),
            content: "Do commits.".into(),
            file_path: "/skills/commit/SKILL.md".into(),
            disable_model_invocation: None,
        };
        let output = format_skill_invocation(&skill, Some("extra"));
        assert!(output.starts_with("<skill name=\"commit\" location=\"/skills/commit/SKILL.md\">"));
        assert!(output.contains("References are relative to /skills/commit."));
        assert!(output.ends_with("extra"));
        let bare = format_skill_invocation(&skill, None);
        assert!(bare.ends_with("</skill>"));
    }
}
