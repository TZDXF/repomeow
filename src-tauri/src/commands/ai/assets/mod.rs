//! 项目 AI 资产探测、可视化管理与 cc-switch 资产导出命令(详情页「AI 面板」数据源)。
//!
//! - `scan_project_ai_assets`:固定路径 + 已知目录探测项目内的指令文件
//!   (CLAUDE.md / AGENTS.md / GEMINI.md 等)、MCP 配置(.mcp.json 等)与
//!   skills 目录(`.claude/skills/*`、`.agents/skills/*` 与 `.zcode/skills/*`,按技能名去重),
//!   并与 registry 的 13 个 agent 安装状态交叉。
//!   不做全仓库递归:直接 `Path::exists` 探测,天然覆盖隐藏条目且代价恒定。
//! - `create_project_skill` / `delete_project_skill` / `set_project_mcp_server` /
//!   `remove_project_mcp_server`(manage.rs):AI 面板的可视化管理——
//!   skills 的新建/删除,MCP 服务器的表单新增/修改/移除(只写探测表内的配置文件)。
//! - `set_project_cc_skill` / `set_project_cc_mcp`:把 cc-switch(`~/.cc-switch`)
//!   管理的 skill / MCP 服务器按项目勾选导出到项目文件
//!   (skill → `.claude/skills/<dir>`,MCP → `.mcp.json` 合并写入),取消勾选即移除。
//!   勾选状态不另建存储,由扫描重新探测项目文件推导(用户手动添加的也算)。

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::commands::agent::list_agents;
use crate::commands::files;
use crate::commands::usage::count_o200k_tokens;
use crate::error::{AppError, AppResult, ErrorCode};

mod cc_export;
mod manage;
#[cfg(test)]
mod tests;

pub use cc_export::*;
pub use manage::*;

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
    /// 服务器对象在文件里的键名(mcpServers / servers),前端写回时原样传回。
    pub servers_key: &'static str,
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

/// 一个 agent 工具的本机安装状态 + 本项目配置命中情况。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectAgentStatus {
    pub id: String,
    pub name: String,
    pub installed: bool,
    /// 本项目内检测到的、该 agent 会读取的配置路径('/' 分隔相对路径)。
    pub configs: Vec<String>,
}

/// scan_project_ai_assets 的聚合结果。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectAiAssets {
    pub files: Vec<AiAssetItem>,
    pub mcp: Vec<ProjectMcpFile>,
    pub skills: Vec<ProjectSkill>,
    pub agents: Vec<ProjectAgentStatus>,
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

/// 项目内 MCP 配置候选:(相对路径, 服务器对象键名)。
const MCP_PROBES: &[(&str, &str)] = &[
    (".mcp.json", "mcpServers"),
    (".cursor/mcp.json", "mcpServers"),
    (".vscode/mcp.json", "servers"),
];

/// 项目级 skills 目录候选(按优先级排序,同名技能先命中者保留)。
/// `.claude/skills` 是 Claude Code 约定,`.agents/skills` 是跨 agent 约定,
/// `.zcode/skills` 是 ZCode 项目级 skills 目录。
const SKILL_DIR_PROBES: &[&str] = &[".claude/skills", ".agents/skills", ".zcode/skills"];

/// agent → 项目内配置探测路径(agent 状态行的「已配置」判定)。
const AGENT_PROBES: &[(&str, &[&str])] = &[
    ("claude", &["CLAUDE.md", ".claude", ".mcp.json"]),
    ("codex", &["AGENTS.md", ".codex"]),
    ("gemini", &["GEMINI.md", ".gemini"]),
    (
        "copilot",
        &[".github/copilot-instructions.md", ".vscode/mcp.json"],
    ),
    ("cursor", &[".cursor", ".cursorrules"]),
    ("cline", &[".clinerules"]),
    ("qwen", &["QWEN.md"]),
    ("goose", &[".goosehints"]),
    ("opencode", &["opencode.json", ".opencode", "AGENTS.md"]),
    ("kimi", &[".kimi", "AGENTS.md"]),
    ("grok", &["AGENTS.md"]),
    ("glm", &["AGENTS.md"]),
    ("pi", &["AGENTS.md"]),
];

// ── 扫描命令 ─────────────────────────────────────────────────────────

/// 扫描项目的 AI 资产(指令文件 / MCP 配置 / skills / agent 状态)。
/// 固定路径探测 + which 安装检测,放 spawn_blocking 避免阻塞主线程。
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

    let mcp = MCP_PROBES
        .iter()
        .filter(|(rel, _)| root.join(rel).is_file())
        .map(|(rel, key)| ProjectMcpFile {
            path: rel.to_string(),
            servers_key: key,
            servers: read_mcp_servers(&root.join(rel), key),
        })
        .collect();

    let skills = scan_project_skills(root);

    let agents = list_agents()
        .into_iter()
        .map(|info| {
            let probes = AGENT_PROBES
                .iter()
                .find(|(id, _)| *id == info.id)
                .map(|(_, probes)| *probes)
                .unwrap_or_default();
            let configs = probes
                .iter()
                .filter(|rel| root.join(rel).exists())
                .map(|rel| rel.to_string())
                .collect();
            ProjectAgentStatus {
                id: info.id.to_string(),
                name: info.name.to_string(),
                installed: info.installed,
                configs,
            }
        })
        .collect();

    Ok(ProjectAiAssets {
        files: items,
        mcp,
        skills,
        agents,
    })
}

/// 读取 MCP 配置文件里声明的服务器(名称+原始定义);解析失败返回空(文件本身仍列出)。
fn read_mcp_servers(path: &Path, key: &str) -> Vec<McpServerEntry> {
    let Ok(raw) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(doc) = serde_json::from_str::<Value>(&raw) else {
        return Vec::new();
    };
    let Some(servers) = doc.get(key).and_then(Value::as_object) else {
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
/// 跨目录按技能名去重(候选顺序即优先级,先命中者保留)。
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
        for skill in dir_skills {
            if !skills.iter().any(|s| s.name == skill.name) {
                skills.push(skill);
            }
        }
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
