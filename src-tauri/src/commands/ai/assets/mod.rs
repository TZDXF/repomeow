//! 项目 AI 资产只读扫描 + 资源库按 Agent 部署。
//! 指令文件可编辑;Skills/MCP 只允许经 deployment 接受资源库 ID 写入。
//! 不扫描工具安装状态,不提供自定义创建或 cc-switch 直写接口。

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::commands::files;
use crate::commands::usage::count_o200k_tokens;
use crate::error::{AppError, AppResult, ErrorCode};

mod deployment;
mod deployment_io;
mod deployment_mcp;
mod mcp_formats;
pub use deployment::*;
#[cfg(test)]
mod tests;

// ── 返回结构(camelCase 序列化,与 src/types/ai-assets.ts 对齐) ─────────────

/// 项目内检测到的一个 AI 指令/规则/设置文件。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiAssetItem {
    /// 仓库相对路径('/' 分隔)。
    pub path: String,
    /// instruction(行为指令)/ rule(规则)/ setting(配置)。
    pub kind: &'static str,
    /// 该文件归属的 agent id(registry 内的 id;windsurf 等未收录的也直接标注)。
    pub agents: Vec<String>,
}

/// 项目内的一个 MCP 服务器条目(名称 + 原始定义)。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerEntry {
    pub name: String,
    /// 原始服务器定义(stdio: command/args/env;远程: url/type/headers 等)。
    pub config: Value,
}

/// 项目内的一个 MCP 配置文件及其声明的服务器。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMcpFile {
    pub path: String,
    /// 配置方言,决定服务器定义的字段映射(claude/codex/gemini/opencode)。
    pub dialect: &'static str,
    /// 该文件归属的 agent id。
    pub agents: Vec<String>,
    /// 服务器条目按名称排序;文件解析失败为空列表(文件仍列出)。
    pub servers: Vec<McpServerEntry>,
}

/// 项目 skills 目录下的一个技能。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSkill {
    /// 技能目录的仓库相对路径,如 ".claude/skills/foo"。
    pub dir: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// frontmatter description 按固定 o200k_base 编码器统计的 token 数。
    pub description_token_count: i64,
    /// 完整 SKILL.md 按固定 o200k_base 编码器统计的 token 数。
    pub token_count: i64,
}

/// scan_project_ai_assets 的聚合结果。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectAiAssets {
    pub files: Vec<AiAssetItem>,
    pub mcp: Vec<ProjectMcpFile>,
    pub skills: Vec<ProjectSkill>,
}

// ── 探测表 ──────────────────────────────────────────────────────────

/// 指令/规则类固定文件:(相对路径, 类别, 归属 agent)。
/// AGENTS.md 是跨 agent 标准,标注主要支持它的 agent。
const FILE_PROBES: &[(&str, &str, &[&str])] = &[
    ("CLAUDE.md", "instruction", &["claude"]),
    ("CLAUDE.local.md", "instruction", &["claude"]),
    (
        "AGENTS.md",
        "instruction",
        &["codex", "cursor", "opencode", "pi", "kimi", "grok", "glm"],
    ),
    ("GEMINI.md", "instruction", &["gemini"]),
    ("QWEN.md", "instruction", &["qwen"]),
    (".cursorrules", "rule", &["cursor"]),
    (".windsurfrules", "rule", &["windsurf"]),
    (".clinerules", "rule", &["cline"]),
    (".goosehints", "instruction", &["goose"]),
    (
        ".github/copilot-instructions.md",
        "instruction",
        &["copilot"],
    ),
    (".claude/settings.json", "setting", &["claude"]),
    (".claude/settings.local.json", "setting", &["claude"]),
    ("opencode.json", "setting", &["opencode"]),
];

/// 一个 MCP 管理目标的静态定义。
pub(super) struct McpTarget {
    /// 仓库相对路径。
    pub path: &'static str,
    /// 配置方言(claude/codex/gemini/opencode),决定读写字段映射与文件格式。
    pub dialect: &'static str,
    /// 服务器对象在文件里的键名(codex 为 TOML 的 mcp_servers 表名)。
    pub key: &'static str,
    /// 归属 agent id。
    pub agents: &'static [&'static str],
}

/// 项目内 MCP 管理目标(各 agent 的项目级 MCP 配置,按各家官方文档:
/// claude=`.mcp.json`、cursor=`.cursor/mcp.json`、copilot=`.vscode/mcp.json`、
/// gemini=`.gemini/settings.json` 的 mcpServers、codex=`.codex/config.toml` 的
/// mcp_servers 表、opencode=`opencode.json` 的 mcp 键、zcode=`.zcode/config.json`
/// 的嵌套 mcp.servers 键(服务器对象与 claude 同形,复用 claude 方言);
/// pi 无 MCP 支持故不在列)。
pub(super) const MCP_TARGETS: &[McpTarget] = &[
    McpTarget {
        path: ".mcp.json",
        dialect: "claude",
        key: "mcpServers",
        agents: &["claude"],
    },
    McpTarget {
        path: ".cursor/mcp.json",
        dialect: "claude",
        key: "mcpServers",
        agents: &["cursor"],
    },
    McpTarget {
        path: ".vscode/mcp.json",
        dialect: "claude",
        key: "servers",
        agents: &["copilot"],
    },
    McpTarget {
        path: ".gemini/settings.json",
        dialect: "gemini",
        key: "mcpServers",
        agents: &["gemini"],
    },
    McpTarget {
        path: ".codex/config.toml",
        dialect: "codex",
        key: "mcp_servers",
        agents: &["codex"],
    },
    McpTarget {
        path: "opencode.json",
        dialect: "opencode",
        key: "mcp",
        agents: &["opencode"],
    },
    McpTarget {
        path: ".zcode/config.json",
        dialect: "claude",
        key: "mcp.servers",
        agents: &["zcode"],
    },
];

/// 项目级 skills 目录候选;同名技能按真实目录独立保留。
/// `.claude/skills` 是 Claude Code 约定,`.agents/skills` 是跨 agent 约定,
/// `.zcode/skills` 是 ZCode 项目级 skills 目录。
const SKILL_DIR_PROBES: &[&str] = &[
    ".claude/skills",
    ".agents/skills",
    ".zcode/skills",
    ".cursor/skills",
    ".github/skills",
    ".gemini/skills",
    ".opencode/skills",
];

// ── 扫描命令 ─────────────────────────────────────────────────────────

/// 扫描项目的 AI 资产(指令文件 / MCP 配置 / skills)。
/// 仅扫描项目固定路径,不探测 Agent 安装状态。
#[tauri::command]
pub async fn scan_project_ai_assets(path: String) -> AppResult<ProjectAiAssets> {
    tokio::task::spawn_blocking(move || scan_assets(&path))
        .await
        .map_err(|e| AppError::coded(ErrorCode::IoError, e.to_string()))?
}

fn scan_assets(path: &str) -> AppResult<ProjectAiAssets> {
    files::ensure_dir(path)?;
    let root = Path::new(path);

    let mut items: Vec<AiAssetItem> = Vec::new();
    for (rel, kind, agents) in FILE_PROBES {
        if root.join(rel).is_file() {
            items.push(AiAssetItem {
                path: rel.to_string(),
                kind,
                agents: agents.iter().map(|a| a.to_string()).collect(),
            });
        }
    }
    // .cursor/rules/*.mdc 读一层(cursor 规则目录)
    let cursor_rules = root.join(".cursor").join("rules");
    if cursor_rules.is_dir() {
        let mut entries: Vec<PathBuf> = fs::read_dir(&cursor_rules)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                        e.eq_ignore_ascii_case("mdc") || e.eq_ignore_ascii_case("md")
                    })
            })
            .collect();
        entries.sort();
        for entry in entries {
            items.push(AiAssetItem {
                path: format!(
                    ".cursor/rules/{}",
                    entry.file_name().unwrap_or_default().to_string_lossy()
                ),
                kind: "rule",
                agents: vec!["cursor".to_string()],
            });
        }
    }

    let mcp = MCP_TARGETS
        .iter()
        .filter(|target| root.join(target.path).is_file())
        .map(|target| ProjectMcpFile {
            path: target.path.to_string(),
            dialect: target.dialect,
            agents: target.agents.iter().map(|a| (*a).to_string()).collect(),
            servers: read_mcp_servers(&root.join(target.path), target),
        })
        .collect();
    let skills = scan_project_skills(root);

    Ok(ProjectAiAssets {
        files: items,
        mcp,
        skills,
    })
}

/// 按点号嵌套键路径(如 mcp.servers)逐层取 JSON 值。
fn get_json_path<'a>(doc: &'a Value, key: &str) -> Option<&'a Value> {
    let mut current = doc;
    for segment in key.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

/// 读取一个 MCP 目标里声明的服务器(名称+原始定义);解析失败返回空(文件本身仍列出)。
fn read_mcp_servers(path: &Path, target: &McpTarget) -> Vec<McpServerEntry> {
    if target.dialect == "codex" {
        return mcp_formats::read_codex_servers(path);
    }
    let Ok(raw) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(doc) = serde_json::from_str::<Value>(&raw) else {
        return Vec::new();
    };
    let Some(servers) = get_json_path(&doc, target.key).and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut entries: Vec<McpServerEntry> = servers
        .into_iter()
        .map(|(name, config)| McpServerEntry {
            name: name.clone(),
            config: config.clone(),
        })
        .collect();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

/// 扫全部候选 skills 目录:每个含 SKILL.md 的子目录算一个技能,
/// 按真实目录保留所有实例,同名资源可在不同 Agent 目录各自配置。
fn scan_project_skills(root: &Path) -> Vec<ProjectSkill> {
    let mut skills: Vec<ProjectSkill> = Vec::new();
    for rel_dir in SKILL_DIR_PROBES {
        let Ok(entries) = fs::read_dir(root.join(rel_dir)) else {
            continue;
        };
        let mut dir_skills: Vec<ProjectSkill> = entries
            .flatten()
            .filter_map(|entry| {
                let dir = entry.path();
                if !dir.is_dir() {
                    return None;
                }
                let skill_md = dir.join("SKILL.md");
                if !skill_md.is_file() {
                    return None;
                }
                let dir_name = dir.file_name()?.to_string_lossy().to_string();
                let (name, description, description_token_count, token_count) =
                    match fs::read_to_string(&skill_md) {
                        Ok(content) => {
                            let (name, description) = parse_skill_frontmatter(&content);
                            let description_token_count = description
                                .as_deref()
                                .map(count_o200k_tokens)
                                .unwrap_or_default();
                            (
                                name,
                                description,
                                description_token_count,
                                count_o200k_tokens(&content),
                            )
                        }
                        Err(_) => (None, None, 0, 0),
                    };
                Some(ProjectSkill {
                    dir: format!("{rel_dir}/{dir_name}"),
                    name: name.unwrap_or_else(|| dir_name.clone()),
                    description: description.unwrap_or_default(),
                    description_token_count,
                    token_count,
                })
            })
            .collect();
        dir_skills.sort_by(|a, b| a.dir.cmp(&b.dir));
        skills.extend(dir_skills);
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.dir.cmp(&b.dir)));
    skills
}

/// 最小 frontmatter 提取(只取 name / description 两个标量;
/// harness 的完整解析器耦合 ExecutionEnv,这里不复用)。
fn parse_skill_frontmatter(content: &str) -> (Option<String>, Option<String>) {
    let mut name = None;
    let mut description = None;
    let mut lines = content.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (None, None);
    }
    for line in lines {
        let line = line.trim_end();
        if line == "---" {
            break;
        }
        for (prefix, slot) in [("name:", &mut name), ("description:", &mut description)] {
            if let Some(value) = line.strip_prefix(prefix) {
                let value = value
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string();
                if !value.is_empty() {
                    *slot = Some(value);
                }
            }
        }
    }
    (name, description)
}
