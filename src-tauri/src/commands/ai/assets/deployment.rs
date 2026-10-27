//! 全局资源库 → 项目配置;非托管目录/服务器也可认领为本地来源(记录来源路径,不入库)。
//! 部署记录位于应用数据目录,不污染项目。分组是选择快捷方式而非动态订阅;更新仅由用户手动应用。
use super::super::resource_library::{
    store::{is_safe_directory, lock_op, Library, DIR_SKILLS, FILE_SKILLS},
    McpServer, RlResult, Skill, SkillGroup, SkillLibrary,
};
use super::deployment_io::*;
use super::deployment_mcp::{definition, parse_server_value, McpDocument};
use super::{McpTarget, MCP_TARGETS};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};
use tauri::AppHandle;

#[path = "deployment_apply.rs"]
mod apply;
#[path = "deployment_manifest.rs"]
mod manifest;
#[path = "deployment_sources.rs"]
mod sources;

use apply::*;
use manifest::{
    manifest_file, manifest_revision, order_source_dirs, read_manifest, save_manifest,
    LocalResource, Manifest, LOCAL_PREFIX,
};
pub use manifest::{Deployment, ShortlistEntry};
pub use sources::ResourceChoice;
use sources::{local_sources, source_content, sources, Content, Source};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectAiTarget {
    id: &'static str,
    name: &'static str,
    skill_path: &'static str,
    /// 空字符串表示尚未接入项目级 MCP 部署。
    mcp_path: &'static str,
}
// 新目标路径依据: Kimi Code docs/en/customization/skills.md 与 mcp.md;
// DeepSeek Harness packages/skill/skill-filesystem/README.md;
// MiniMax Code: vercel-labs/skills src/agents.ts; Pi: packages/coding-agent/docs/skills.md。
const TARGETS: &[ProjectAiTarget] = &[
    ProjectAiTarget {
        id: "claude",
        name: "Claude Code",
        skill_path: ".claude/skills",
        mcp_path: ".mcp.json",
    },
    ProjectAiTarget {
        id: "cursor",
        name: "Cursor",
        skill_path: ".cursor/skills",
        mcp_path: ".cursor/mcp.json",
    },
    ProjectAiTarget {
        id: "copilot",
        name: "GitHub Copilot",
        skill_path: ".github/skills",
        mcp_path: ".vscode/mcp.json",
    },
    ProjectAiTarget {
        id: "gemini",
        name: "Gemini CLI",
        skill_path: ".gemini/skills",
        mcp_path: ".gemini/settings.json",
    },
    ProjectAiTarget {
        id: "codex",
        name: "Codex",
        skill_path: ".agents/skills",
        mcp_path: ".codex/config.toml",
    },
    ProjectAiTarget {
        id: "opencode",
        name: "OpenCode",
        skill_path: ".opencode/skills",
        mcp_path: "opencode.json",
    },
    ProjectAiTarget {
        id: "zcode",
        name: "ZCode",
        skill_path: ".zcode/skills",
        mcp_path: ".zcode/config.json",
    },
    ProjectAiTarget {
        id: "kimi",
        name: "Kimi Code CLI",
        skill_path: ".kimi-code/skills",
        mcp_path: ".kimi-code/mcp.json",
    },
    ProjectAiTarget {
        id: "dsh",
        name: "DeepSeek Harness",
        skill_path: ".dsh/skills",
        mcp_path: "",
    },
    ProjectAiTarget {
        id: "minimax",
        name: "MiniMax Code",
        skill_path: ".minimax/skills",
        mcp_path: "",
    },
    ProjectAiTarget {
        id: "pi",
        name: "Pi",
        skill_path: ".pi/skills",
        mcp_path: ".pi/mcp.json",
    },
];

#[tauri::command]
pub fn project_ai_targets() -> Vec<ProjectAiTarget> {
    TARGETS.to_vec()
}

fn target(id: &str) -> RlResult<&'static ProjectAiTarget> {
    TARGETS
        .iter()
        .find(|t| t.id == id)
        .ok_or_else(|| problem(format!("unknown agent: {id}")))
}
fn mcp_target(id: &str) -> RlResult<&'static McpTarget> {
    MCP_TARGETS
        .iter()
        .find(|t| t.agents.contains(&id))
        .ok_or_else(|| problem(id))
}
fn validate_kind(kind: &str) -> RlResult<()> {
    if matches!(kind, "skills" | "mcp") {
        Ok(())
    } else {
        Err(problem(kind))
    }
}

fn current_hash(root: &Path, entry: &Deployment) -> RlResult<Option<String>> {
    let path = safe_path(root, &entry.path)?;
    if entry.kind == "skills" {
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(tree_hash(&read_tree(&path)?)))
    } else {
        Ok(McpDocument::read(root, mcp_target(&entry.agent_id)?)?
            .entry(mcp_target(&entry.agent_id)?, &entry.name)?
            .as_ref()
            .map(json_hash))
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentStatus {
    #[serde(flatten)]
    entry: Deployment,
    status: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectResourceSnapshot {
    revision: String,
    groups: Vec<SkillGroup>,
    resources: Vec<ResourceChoice>,
    deployments: Vec<DeploymentStatus>,
    /// 已加入项目列表的资源 ID(含尚未配置任何 Agent 的项)。
    shortlist: Vec<String>,
    source_error: Option<String>,
}
fn snapshot(library: &Library, root: &Path, kind: &str) -> RlResult<ProjectResourceSnapshot> {
    validate_kind(kind)?;
    let manifest = read_manifest(&manifest_file(library, root), root)?;
    let revision = manifest_revision(&manifest)?;
    let shortlist = manifest
        .shortlist
        .iter()
        .filter(|s| s.kind == kind)
        .map(|s| s.resource_id.clone())
        .collect();
    let (groups, choices, source_error) = match sources(library, kind) {
        Ok((groups, sources)) => (groups, sources, None),
        Err(e) => (vec![], vec![], Some(e.code().to_string())),
    };
    let all: Vec<Source> = choices
        .into_iter()
        .chain(local_sources(root, &manifest, kind))
        .collect();
    let deployments = manifest
        .entries
        .into_iter()
        .filter(|e| e.kind == kind)
        .map(|entry| {
            let status = match current_hash(root, &entry) {
                Err(_) => "conflict",
                Ok(None) => "missing",
                Ok(Some(hash)) if hash != entry.fingerprint => "modified",
                Ok(Some(_)) => {
                    if source_error.is_some() && !entry.resource_id.starts_with(LOCAL_PREFIX) {
                        "sourceUnavailable"
                    } else if let Some(source) = all.iter().find(|s| s.id() == entry.resource_id) {
                        match source_content(library, root, source, &entry.agent_id) {
                            Ok(content) if content.fingerprint() == entry.fingerprint => {
                                "configured"
                            }
                            Ok(_) => "update",
                            Err(_) => "conflict",
                        }
                    } else {
                        "sourceMissing"
                    }
                }
            };
            DeploymentStatus {
                entry,
                status: status.to_string(),
            }
        })
        .collect();
    Ok(ProjectResourceSnapshot {
        revision,
        groups,
        resources: all.iter().map(Source::choice).collect(),
        deployments,
        shortlist,
        source_error,
    })
}

#[tauri::command]
pub async fn project_ai_resources(
    app: AppHandle,
    path: String,
    kind: String,
) -> RlResult<ProjectResourceSnapshot> {
    tokio::task::spawn_blocking(move || {
        let _guard = lock_op();
        let library = Library::app(&app)?;
        library.ensure()?;
        let root = project_root(&path)?;
        snapshot(&library, &root, &kind)
    })
    .await
    .map_err(|e| problem(e.to_string()))?
}
fn project_root(path: &str) -> RlResult<PathBuf> {
    let root = fs::canonicalize(crate::path_util::clean_str(path))?;
    if !root.is_dir() {
        return Err(problem(path));
    }
    Ok(root)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyFailure {
    resource_id: String,
    code: String,
    message: String,
}
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ApplyResult {
    applied: usize,
    failures: Vec<ApplyFailure>,
}

/// 把资源库资源加入项目列表:仅登记 shortlist,不部署到任何 Agent。
#[tauri::command]
pub async fn project_ai_add(
    app: AppHandle,
    path: String,
    kind: String,
    resource_ids: Vec<String>,
    expected_revision: String,
) -> RlResult<ApplyResult> {
    tokio::task::spawn_blocking(move || {
        let _guard = lock_op();
        let (library, root) = prepare(&app, &path, &kind, &expected_revision)?;
        add(&library, &root, &kind, &resource_ids)
    })
    .await
    .map_err(|e| problem(e.to_string()))?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssignFailure {
    agent_id: String,
    code: String,
    message: String,
}
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AssignResult {
    applied: usize,
    failures: Vec<AssignFailure>,
}

/// agentIds 是单个资源的完整目标 Agent 集合;逐 Agent 复用部署管线,失败互不影响。
#[tauri::command]
pub async fn project_ai_assign(
    app: AppHandle,
    path: String,
    kind: String,
    resource_id: String,
    agent_ids: Vec<String>,
    expected_revision: String,
) -> RlResult<AssignResult> {
    tokio::task::spawn_blocking(move || {
        let _guard = lock_op();
        let (library, root) = prepare(&app, &path, &kind, &expected_revision)?;
        assign(&library, &root, &kind, &resource_id, &agent_ids)
    })
    .await
    .map_err(|e| problem(e.to_string()))?
}

/// 从项目列表移除资源:先解除全部 Agent 的部署,全部成功后才移出 shortlist。
#[tauri::command]
pub async fn project_ai_remove(
    app: AppHandle,
    path: String,
    kind: String,
    resource_id: String,
    expected_revision: String,
) -> RlResult<AssignResult> {
    tokio::task::spawn_blocking(move || {
        let _guard = lock_op();
        let (library, root) = prepare(&app, &path, &kind, &expected_revision)?;
        remove_resource(&library, &root, &kind, &resource_id)
    })
    .await
    .map_err(|e| problem(e.to_string()))?
}

/// 修复异常部署(modified/conflict 等)的兜底出口:
/// - `reapply`:覆盖更新——以来源最新定义重写项目内容,**丢弃本地修改**(跳过基线校验);
///   本地来源的来源 Agent 语义为把现状认领为新基线(来源即文件本身,无内容可写)。
/// - `detach`:强制解除——仅删除托管记录,**保留项目文件**,之后可按非托管资源重新认领/导入。
#[tauri::command]
pub async fn project_ai_repair(
    app: AppHandle,
    path: String,
    kind: String,
    resource_id: String,
    agent_id: String,
    action: String,
    expected_revision: String,
) -> RlResult<()> {
    tokio::task::spawn_blocking(move || {
        let _guard = lock_op();
        let (library, root) = prepare(&app, &path, &kind, &expected_revision)?;
        repair(&library, &root, &kind, &resource_id, &agent_id, &action)
    })
    .await
    .map_err(|e| problem(e.to_string()))?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportOutcome {
    resource_id: String,
    /// 资源库已存在同名条目(直接认领,未新建)。
    existed: bool,
}

/// 一键导入非托管资源:收入资源库(同名复用已有条目)并按现状认领为托管配置。
/// skills: source = 项目内技能目录(如 .claude/skills/foo);mcp: source = 配置文件路径、name = 服务器名。
#[tauri::command]
pub async fn project_ai_import(
    app: AppHandle,
    path: String,
    kind: String,
    source: String,
    name: Option<String>,
    expected_revision: String,
) -> RlResult<ImportOutcome> {
    tokio::task::spawn_blocking(move || {
        let _guard = lock_op();
        let (library, root) = prepare(&app, &path, &kind, &expected_revision)?;
        if kind == "skills" {
            import_skill(&library, &root, &source)
        } else {
            let name = name.ok_or_else(|| problem("MCP server name required"))?;
            import_mcp(&library, &root, &source, &name)
        }
    })
    .await
    .map_err(|e| problem(e.to_string()))?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimOutcome {
    resource_id: String,
}

/// 认领非托管资源为项目本地来源(不进资源库):记录来源路径,来源 Agent 按现状登记为已配置。
/// skills: source = 项目内技能目录;mcp: source = 配置文件路径、name = 服务器名。
#[tauri::command]
pub async fn project_ai_claim_local(
    app: AppHandle,
    path: String,
    kind: String,
    source: String,
    name: Option<String>,
    expected_revision: String,
) -> RlResult<ClaimOutcome> {
    tokio::task::spawn_blocking(move || {
        let _guard = lock_op();
        let (library, root) = prepare(&app, &path, &kind, &expected_revision)?;
        claim_local(&library, &root, &kind, &source, name.as_deref())
    })
    .await
    .map_err(|e| problem(e.to_string()))?
}

/// 删除非托管资源:skills 删除整个技能目录,mcp 从配置文件中移除服务器条目。
/// 仅允许删除 manifest 未托管的路径/条目,避免误删已认领或已部署内容。
#[tauri::command]
pub async fn project_ai_delete_unmanaged(
    app: AppHandle,
    path: String,
    kind: String,
    source: String,
    name: Option<String>,
    expected_revision: String,
) -> RlResult<()> {
    tokio::task::spawn_blocking(move || {
        let _guard = lock_op();
        let (library, root) = prepare(&app, &path, &kind, &expected_revision)?;
        delete_unmanaged(&library, &root, &kind, &source, name.as_deref())
    })
    .await
    .map_err(|e| problem(e.to_string()))?
}

#[cfg(test)]
#[path = "deployment_tests.rs"]
mod tests;
