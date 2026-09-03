//! 项目 AI 资产探测与 cc-switch 资产导出命令(详情页「AI 面板」数据源)。
//!
//! - `scan_project_ai_assets`:固定路径 + 已知目录探测项目内的指令文件
//!   (CLAUDE.md / AGENTS.md / GEMINI.md 等)、MCP 配置(.mcp.json 等)与
//!   skills 目录(.claude/skills/*),并与 registry 的 13 个 agent 安装状态交叉。
//!   不做全仓库递归:直接 `Path::exists` 探测,天然覆盖隐藏条目且代价恒定。
//! - `set_project_cc_skill` / `set_project_cc_mcp`:把 cc-switch(`~/.cc-switch`)
//!   管理的 skill / MCP 服务器按项目勾选导出到项目文件
//!   (skill → `.claude/skills/<dir>`,MCP → `.mcp.json` 合并写入),取消勾选即移除。
//!   勾选状态不另建存储,由扫描重新探测项目文件推导(用户手动添加的也算)。

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use tauri::AppHandle;

use crate::ai::cc_switch;
use crate::commands::agent::list_agents;
use crate::commands::files;
use crate::error::{AppError, AppResult, ErrorCode};

// ── 返回结构(camelCase 序列化,与 src/types/index.ts 对齐) ─────────────

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

/// 项目内的一个 MCP 配置文件及其声明的服务器名。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMcpFile {
    pub path: String,
    /// mcpServers(VS Code 为 servers)对象的键;文件解析失败为空列表(文件仍列出)。
    pub servers: Vec<String>,
}

/// 项目 .claude/skills 下的一个技能。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSkill {
    /// 技能目录的仓库相对路径,如 ".claude/skills/foo"。
    pub dir: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
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
                    && p.extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| e.eq_ignore_ascii_case("mdc") || e.eq_ignore_ascii_case("md"))
            })
            .collect();
        entries.sort();
        for entry in entries {
            items.push(AiAssetItem {
                path: format!(".cursor/rules/{}", entry.file_name().unwrap_or_default().to_string_lossy()),
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
            servers: read_mcp_server_names(&root.join(rel), key),
        })
        .collect();

    let skills = scan_project_skills(&root.join(".claude").join("skills"));

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

/// 读取 MCP 配置文件里声明的服务器名;解析失败返回空(文件本身仍列出)。
fn read_mcp_server_names(path: &Path, key: &str) -> Vec<String> {
    let Ok(raw) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(doc) = serde_json::from_str::<Value>(&raw) else {
        return Vec::new();
    };
    doc.get(key)
        .and_then(Value::as_object)
        .map(|servers| {
            let mut names: Vec<String> = servers.keys().cloned().collect();
            names.sort();
            names
        })
        .unwrap_or_default()
}

/// 扫 .claude/skills 一层:每个含 SKILL.md 的子目录算一个技能。
fn scan_project_skills(skills_dir: &Path) -> Vec<ProjectSkill> {
    let Ok(entries) = fs::read_dir(skills_dir) else {
        return Vec::new();
    };
    let mut skills: Vec<ProjectSkill> = entries
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
            let (name, description) = fs::read_to_string(&skill_md)
                .map(|content| parse_skill_frontmatter(&content))
                .unwrap_or_default();
            Some(ProjectSkill {
                dir: format!(".claude/skills/{dir_name}"),
                name: name.unwrap_or_else(|| dir_name.clone()),
                description: description.unwrap_or_default(),
            })
        })
        .collect();
    skills.sort_by(|a, b| a.dir.cmp(&b.dir));
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

// ── cc-switch 导出(勾选 → 写项目文件) ────────────────────────────────

/// 勾选/取消一个 cc-switch 技能:导出到项目 `.claude/skills/<directory>`
/// (已存在则整体替换),取消则删除对应目录。
#[tauri::command]
pub fn set_project_cc_skill(
    app: AppHandle,
    path: String,
    directory: String,
    enable: bool,
) -> AppResult<()> {
    files::ensure_dir(&path)?;
    let cc_dir = cc_switch::cc_switch_dir(&app)?;
    set_project_cc_skill_at(&cc_dir, Path::new(&path), &directory, enable)
}

fn set_project_cc_skill_at(
    cc_dir: &Path,
    project: &Path,
    directory: &str,
    enable: bool,
) -> AppResult<()> {
    // directory 只能是单层目录名,防路径穿越
    if directory.is_empty()
        || directory == "."
        || directory == ".."
        || directory.contains(['/', '\\'])
    {
        return Err(AppError::coded(
            ErrorCode::InvalidPath,
            format!("非法的技能目录名:{directory}"),
        ));
    }
    let target = project.join(".claude").join("skills").join(directory);
    if enable {
        let source = cc_dir.join("skills").join(directory);
        if !source.is_dir() {
            return Err(AppError::coded(
                ErrorCode::InvalidPath,
                format!("cc-switch 中不存在该技能:{directory}"),
            ));
        }
        // 已存在则整体替换,保证与 cc-switch 源一致
        remove_dir_robust(&target)?;
        copy_dir_recursive(&source, &target)?;
    } else {
        remove_dir_robust(&target)?;
    }
    Ok(())
}

/// 递归复制目录;跳过符号链接(不跟随逃逸)。
fn copy_dir_recursive(source: &Path, target: &Path) -> AppResult<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let from = entry.path();
        let to = target.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if file_type.is_file() {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// 删除目录,容忍不存在;Windows 上先清只读位再重试(skill 目录可能含
/// 只读的 git 对象文件,裸 remove_dir_all 会「拒绝访问」,同 wiki remove_wiki_dir)。
fn remove_dir_robust(dir: &Path) -> AppResult<()> {
    match fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => {
            clear_readonly_recursive(dir);
            match fs::remove_dir_all(dir) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.into()),
            }
        }
    }
}

#[cfg(windows)]
fn clear_readonly_recursive(root: &Path) {
    fn visit(path: &Path) {
        let Ok(meta) = fs::metadata(path) else {
            return;
        };
        if meta.is_dir() {
            if let Ok(entries) = fs::read_dir(path) {
                for entry in entries.flatten() {
                    visit(&entry.path());
                }
            }
        }
        let mut perms = meta.permissions();
        if perms.readonly() {
            perms.set_readonly(false);
            let _ = fs::set_permissions(path, perms);
        }
    }
    visit(root);
}

#[cfg(not(windows))]
fn clear_readonly_recursive(_root: &Path) {}

/// 勾选/取消一个 cc-switch MCP 服务器:按 name 合并写入项目 `.mcp.json`
/// 的 mcpServers(项目自有条目不动),取消则移除该键。
/// disable 时若 cc-switch 库中已查不到该 id,用前端传入的 server_name 移除。
#[tauri::command]
pub fn set_project_cc_mcp(
    app: AppHandle,
    path: String,
    server_id: String,
    server_name: String,
    enable: bool,
) -> AppResult<()> {
    files::ensure_dir(&path)?;
    let cc_dir = cc_switch::cc_switch_dir(&app)?;
    let target = Path::new(&path).join(".mcp.json");
    if enable {
        let (name, config) = cc_switch::mcp_server_by_id(&cc_dir, &server_id)?
            .ok_or_else(|| {
                AppError::coded(
                    ErrorCode::InvalidPath,
                    "cc-switch 中已不存在该 MCP 服务器,请刷新列表".to_string(),
                )
            })?;
        upsert_mcp_server(&target, &name, config)
    } else {
        let name = cc_switch::mcp_server_by_id(&cc_dir, &server_id)?
            .map(|(name, _)| name)
            .unwrap_or(server_name);
        remove_mcp_server(&target, &name)
    }
}

/// 读取 `.mcp.json`(不存在则空对象);存在但损坏/非对象时报错,不盲写。
fn read_mcp_json(target: &Path) -> AppResult<Value> {
    if !target.is_file() {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    let raw = fs::read_to_string(target)?;
    let doc: Value = serde_json::from_str(&raw).map_err(|e| {
        AppError::coded(
            ErrorCode::InvalidPath,
            format!("项目 .mcp.json 解析失败({e}),请先手动修复后再操作"),
        )
    })?;
    if !doc.is_object() {
        return Err(AppError::coded(
            ErrorCode::InvalidPath,
            "项目 .mcp.json 顶层不是 JSON 对象,请先手动修复".to_string(),
        ));
    }
    Ok(doc)
}

/// tmp+rename 原子写(末尾换行,2 空格缩进,与常见格式化一致)。
fn atomic_write_json(target: &Path, doc: &Value) -> AppResult<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut text = serde_json::to_string_pretty(doc)
        .map_err(|e| AppError::coded(ErrorCode::IoError, e.to_string()))?;
    text.push('\n');
    let tmp = target.with_extension("json.repomeow-tmp");
    fs::write(&tmp, text)?;
    fs::rename(&tmp, target)?;
    Ok(())
}

fn mcp_servers_map_mut(doc: &mut Value) -> AppResult<&mut serde_json::Map<String, Value>> {
    let obj = doc
        .as_object_mut()
        .ok_or_else(|| AppError::coded(ErrorCode::InvalidPath, ".mcp.json 顶层非对象"))?;
    let entry = obj
        .entry("mcpServers")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if !entry.is_object() {
        return Err(AppError::coded(
            ErrorCode::InvalidPath,
            "项目 .mcp.json 的 mcpServers 不是对象,请先手动修复".to_string(),
        ));
    }
    Ok(entry.as_object_mut().unwrap())
}

fn upsert_mcp_server(target: &Path, name: &str, config: Value) -> AppResult<()> {
    let mut doc = read_mcp_json(target)?;
    mcp_servers_map_mut(&mut doc)?.insert(name.to_string(), config);
    atomic_write_json(target, &doc)
}

/// 移除键;键不存在时不改写文件(避免无意义的 mtime 变化)。
fn remove_mcp_server(target: &Path, name: &str) -> AppResult<()> {
    if !target.is_file() {
        return Ok(());
    }
    let mut doc = read_mcp_json(target)?;
    let removed = doc
        .get_mut("mcpServers")
        .and_then(Value::as_object_mut)
        .is_some_and(|servers| servers.remove(name).is_some());
    if removed {
        atomic_write_json(target, &doc)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_project_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "repomeow-ai-assets-{tag}-{}-{}",
            std::process::id(),
            crate::time_util::now_ts_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn detects_fixed_instruction_files() {
        let dir = temp_project_dir("files");
        fs::write(dir.join("CLAUDE.md"), "# claude").unwrap();
        fs::write(dir.join("AGENTS.md"), "# agents").unwrap();
        fs::create_dir_all(dir.join(".github")).unwrap();
        fs::write(dir.join(".github/copilot-instructions.md"), "hint").unwrap();
        fs::create_dir_all(dir.join(".cursor/rules")).unwrap();
        fs::write(dir.join(".cursor/rules/a.mdc"), "rule a").unwrap();
        fs::write(dir.join(".cursor/rules/b.md"), "rule b").unwrap();
        fs::write(dir.join(".cursor/rules/c.txt"), "not a rule").unwrap();

        let assets = scan_assets(&dir.to_string_lossy()).unwrap();
        let paths: Vec<&str> = assets.files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"CLAUDE.md"));
        assert!(paths.contains(&"AGENTS.md"));
        assert!(paths.contains(&".github/copilot-instructions.md"));
        assert!(paths.contains(&".cursor/rules/a.mdc"));
        assert!(paths.contains(&".cursor/rules/b.md"));
        assert!(!paths.contains(&".cursor/rules/c.txt"));
        let agents_md = assets
            .files
            .iter()
            .find(|f| f.path == "AGENTS.md")
            .unwrap();
        assert!(agents_md.agents.contains(&"codex".to_string()));

        // agent 状态:claude/codex/copilot/cursor 应报「已配置」
        let configured = |id: &str| {
            assets
                .agents
                .iter()
                .find(|a| a.id == id)
                .map(|a| !a.configs.is_empty())
                .unwrap_or(false)
        };
        assert!(configured("claude"));
        assert!(configured("codex"));
        assert!(configured("copilot"));
        assert!(configured("cursor"));
        assert!(!configured("gemini"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_mcp_server_names_and_tolerates_corrupt() {
        let dir = temp_project_dir("mcp");
        fs::write(
            dir.join(".mcp.json"),
            r#"{"mcpServers":{"b":{},"a":{}}}"#,
        )
        .unwrap();
        fs::create_dir_all(dir.join(".cursor")).unwrap();
        fs::write(dir.join(".cursor/mcp.json"), "not json").unwrap();

        let assets = scan_assets(&dir.to_string_lossy()).unwrap();
        assert_eq!(assets.mcp.len(), 2);
        let root_mcp = assets.mcp.iter().find(|m| m.path == ".mcp.json").unwrap();
        assert_eq!(root_mcp.servers, vec!["a".to_string(), "b".to_string()]);
        let cursor_mcp = assets
            .mcp
            .iter()
            .find(|m| m.path == ".cursor/mcp.json")
            .unwrap();
        assert!(cursor_mcp.servers.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn scans_project_skills_with_frontmatter() {
        let dir = temp_project_dir("skills");
        let skill = dir.join(".claude/skills/demo");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: demo-skill\ndescription: \"做演示\"\n---\n\n# Demo\n",
        )
        .unwrap();
        // 无 SKILL.md 的目录不算技能
        fs::create_dir_all(dir.join(".claude/skills/no-md")).unwrap();

        let assets = scan_assets(&dir.to_string_lossy()).unwrap();
        assert_eq!(assets.skills.len(), 1);
        assert_eq!(assets.skills[0].dir, ".claude/skills/demo");
        assert_eq!(assets.skills[0].name, "demo-skill");
        assert_eq!(assets.skills[0].description, "做演示");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn frontmatter_requires_opening_fence() {
        assert_eq!(parse_skill_frontmatter("# hi\nname: x\n"), (None, None));
        let (name, desc) = parse_skill_frontmatter("---\nname: x\n---\nbody");
        assert_eq!(name, Some("x".to_string()));
        assert_eq!(desc, None);
    }

    #[test]
    fn cc_skill_export_roundtrip() {
        let cc = temp_project_dir("cc-src");
        let project = temp_project_dir("cc-dst");
        let src = cc.join("skills/zip-skill");
        fs::create_dir_all(src.join("scripts")).unwrap();
        fs::write(src.join("SKILL.md"), "---\nname: zip\n---\n").unwrap();
        fs::write(src.join("scripts/run.py"), "print()").unwrap();

        set_project_cc_skill_at(&cc, &project, "zip-skill", true).unwrap();
        assert!(project.join(".claude/skills/zip-skill/SKILL.md").is_file());
        assert!(project
            .join(".claude/skills/zip-skill/scripts/run.py")
            .is_file());

        // 重复导出 = 整体替换(源里新增文件也带上)
        fs::write(src.join("extra.txt"), "x").unwrap();
        set_project_cc_skill_at(&cc, &project, "zip-skill", true).unwrap();
        assert!(project.join(".claude/skills/zip-skill/extra.txt").is_file());

        set_project_cc_skill_at(&cc, &project, "zip-skill", false).unwrap();
        assert!(!project.join(".claude/skills/zip-skill").exists());

        // 路径穿越与缺失源
        assert!(set_project_cc_skill_at(&cc, &project, "../evil", true).is_err());
        assert!(set_project_cc_skill_at(&cc, &project, "missing", true).is_err());
        // 取消不存在的目录幂等
        set_project_cc_skill_at(&cc, &project, "missing", false).unwrap();

        let _ = fs::remove_dir_all(&cc);
        let _ = fs::remove_dir_all(&project);
    }

    #[test]
    fn mcp_upsert_merges_and_remove_is_idempotent() {
        let dir = temp_project_dir("mcp-upsert");
        let target = dir.join(".mcp.json");

        // 项目已有自有条目,合并不覆盖
        fs::write(
            &target,
            r#"{"mcpServers":{"own":{"command":"own"}},"other":1}"#,
        )
        .unwrap();
        upsert_mcp_server(&target, "web", json!({"command": "npx", "args": ["mcp-web"]})).unwrap();
        let doc: Value =
            serde_json::from_str(&fs::read_to_string(&target).unwrap()).unwrap();
        assert!(doc["mcpServers"]["own"].is_object());
        assert_eq!(doc["mcpServers"]["web"]["command"], json!("npx"));
        assert_eq!(doc["other"], json!(1));

        // 覆盖同名键
        upsert_mcp_server(&target, "web", json!({"url": "https://x"})).unwrap();
        let doc: Value =
            serde_json::from_str(&fs::read_to_string(&target).unwrap()).unwrap();
        assert_eq!(doc["mcpServers"]["web"]["url"], json!("https://x"));

        remove_mcp_server(&target, "web").unwrap();
        let doc: Value =
            serde_json::from_str(&fs::read_to_string(&target).unwrap()).unwrap();
        assert!(doc["mcpServers"]["web"].is_null());
        assert!(doc["mcpServers"]["own"].is_object());
        // 再删幂等;文件不存在也幂等
        remove_mcp_server(&target, "web").unwrap();
        remove_mcp_server(&dir.join("nope/.mcp.json"), "web").unwrap();

        // 损坏 JSON 报错且不改写
        fs::write(&target, "not json").unwrap();
        assert!(upsert_mcp_server(&target, "a", json!({})).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "not json");

        let _ = fs::remove_dir_all(&dir);
    }
}
