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

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectAiTarget {
    id: &'static str,
    name: &'static str,
    skill_path: &'static str,
    mcp_path: &'static str,
}
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

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Deployment {
    kind: String,
    agent_id: String,
    resource_id: String,
    name: String,
    path: String,
    fingerprint: String,
}
/// 项目资源列表条目:先「添加」加入列表(shortlist),再按 Agent 配置部署(entries)。
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortlistEntry {
    kind: String,
    resource_id: String,
}
/// 本地来源资源 ID 前缀,与资源库 ID 区分。
const LOCAL_PREFIX: &str = "local:";

/// 本地来源(非托管认领):记录 skills/mcp 的来源路径与归属 Agent,不进全局资源库。
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalResource {
    kind: String,
    /// `local:skills:<目录>` / `local:mcp:<配置文件>#<服务器名>`。
    id: String,
    name: String,
    #[serde(default)]
    description: String,
    /// 来源位置:skills = 技能目录;mcp = 配置文件路径。
    source_path: String,
    /// 来源路径归属的 Agent(该 Agent 只登记记录,文件保持原样)。
    origin_agent: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    version: u32,
    project_path: String,
    entries: Vec<Deployment>,
    #[serde(default)]
    shortlist: Vec<ShortlistEntry>,
    #[serde(default)]
    locals: Vec<LocalResource>,
}

fn manifest_file(library: &Library, root: &Path) -> PathBuf {
    let key = crate::path_util::clean_str(&root.to_string_lossy());
    library
        .root()
        .parent()
        .unwrap()
        .join("project-ai")
        .join(format!("{}.json", hash(key.as_bytes())))
}
fn read_manifest(path: &Path, root: &Path) -> RlResult<Manifest> {
    let project_path = crate::path_util::clean_str(&root.to_string_lossy());
    let manifest = match fs::read(path) {
        Ok(bytes) => {
            serde_json::from_slice::<Manifest>(&bytes).map_err(|e| problem(e.to_string()))?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Manifest {
            version: 1,
            project_path: project_path.clone(),
            entries: vec![],
            shortlist: vec![],
            locals: vec![],
        },
        Err(e) => return Err(e.into()),
    };
    if manifest.version != 1 || manifest.project_path != project_path {
        return Err(problem("invalid deployment manifest"));
    }
    let mut keys = HashSet::new();
    for entry in &manifest.entries {
        validate_kind(&entry.kind)?;
        let agent = target(&entry.agent_id)?;
        let valid = if entry.kind == "skills" {
            let p = Path::new(&entry.path);
            p.parent() == Some(Path::new(agent.skill_path))
                && p.file_name()
                    .is_some_and(|n| is_safe_directory(&n.to_string_lossy()))
        } else {
            entry.path == agent.mcp_path && !entry.name.is_empty()
        };
        if !valid || !keys.insert((&entry.kind, &entry.agent_id, &entry.resource_id)) {
            return Err(problem("invalid deployment record"));
        }
    }
    let mut local_ids = HashSet::new();
    for local in &manifest.locals {
        let agent = target(&local.origin_agent)?;
        let valid = if local.kind == "skills" {
            let p = Path::new(&local.source_path);
            p.parent() == Some(Path::new(agent.skill_path))
                && p
                    .file_name()
                    .is_some_and(|n| is_safe_directory(&n.to_string_lossy()))
        } else {
            local.kind == "mcp" && local.source_path == agent.mcp_path && !local.name.is_empty()
        };
        if !local.id.starts_with(LOCAL_PREFIX)
            || !valid
            || !local_ids.insert((&local.kind, &local.id))
        {
            return Err(problem("invalid local resource record"));
        }
    }
    let mut shortlisted = HashSet::new();
    for item in &manifest.shortlist {
        validate_kind(&item.kind)?;
        if item.resource_id.is_empty()
            || !shortlisted.insert((&item.kind, &item.resource_id))
        {
            return Err(problem("invalid shortlist record"));
        }
    }
    Ok(manifest)
}
fn manifest_revision(manifest: &Manifest) -> RlResult<String> {
    Ok(hash(
        &serde_json::to_vec(manifest).map_err(|e| problem(e.to_string()))?,
    ))
}
fn save_manifest(path: &Path, manifest: &Manifest) -> RlResult<()> {
    atomic_write(
        path,
        &serde_json::to_vec_pretty(manifest).map_err(|e| problem(e.to_string()))?,
    )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceChoice {
    id: String,
    name: String,
    description: String,
    group_ids: Vec<String>,
    supported_agents: Vec<String>,
    /// 市场技能来源(owner/repo);手动创建技能与 MCP 无此字段,前端按来源附加分组。
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
}
enum Source {
    Skill(Skill),
    Mcp(McpServer),
    /// 本地来源技能:内容以项目内来源目录为准。
    LocalSkill(LocalResource),
    /// 本地来源 MCP:附带来源配置解析出的通用定义。
    LocalMcp(LocalResource, McpServer),
}
impl Source {
    fn id(&self) -> &str {
        match self {
            Self::Skill(s) => &s.id,
            Self::Mcp(s) => &s.id,
            Self::LocalSkill(l) | Self::LocalMcp(l, _) => &l.id,
        }
    }
    fn choice(&self) -> ResourceChoice {
        match self {
            Self::Skill(s) => ResourceChoice {
                id: s.id.clone(),
                name: s.name.clone(),
                description: s.description.clone(),
                group_ids: s.group_ids.clone(),
                supported_agents: TARGETS.iter().map(|t| t.id.to_string()).collect(),
                source: s.marketplace.as_ref().map(|m| m.source.clone()),
            },
            Self::Mcp(s) => ResourceChoice {
                id: s.id.clone(),
                name: s.name.clone(),
                description: s.description.clone().unwrap_or_default(),
                group_ids: vec![],
                supported_agents: MCP_TARGETS
                    .iter()
                    .filter(|t| definition(s, t).is_ok())
                    .flat_map(|t| t.agents.iter().map(|a| a.to_string()))
                    .collect(),
                source: None,
            },
            Self::LocalSkill(l) => ResourceChoice {
                id: l.id.clone(),
                name: l.name.clone(),
                description: l.description.clone(),
                group_ids: vec![],
                supported_agents: TARGETS.iter().map(|t| t.id.to_string()).collect(),
                source: None,
            },
            Self::LocalMcp(l, server) => ResourceChoice {
                id: l.id.clone(),
                name: l.name.clone(),
                description: server.description.clone().unwrap_or_default(),
                group_ids: vec![],
                supported_agents: MCP_TARGETS
                    .iter()
                    .filter(|t| definition(server, t).is_ok())
                    .flat_map(|t| t.agents.iter().map(|a| a.to_string()))
                    .collect(),
                source: None,
            },
        }
    }
}
fn sources(library: &Library, kind: &str) -> RlResult<(Vec<SkillGroup>, Vec<Source>)> {
    if kind == "skills" {
        let mut data: SkillLibrary = library.read_plain_json(FILE_SKILLS)?;
        data.groups.sort_by_key(|g| g.sort);
        data.skills.sort_by_key(|s| s.sort_order);
        Ok((
            data.groups,
            data.skills.into_iter().map(Source::Skill).collect(),
        ))
    } else {
        let data: Vec<McpServer> = library.read_mcp_json()?;
        Ok((vec![], data.into_iter().map(Source::Mcp).collect()))
    }
}

enum Content {
    Skill(SkillTree),
    Mcp(Value),
}
impl Content {
    fn fingerprint(&self) -> String {
        match self {
            Self::Skill(tree) => tree_hash(tree),
            Self::Mcp(value) => json_hash(value),
        }
    }
}
fn source_content(
    library: &Library,
    root: &Path,
    source: &Source,
    agent: &str,
) -> RlResult<Content> {
    match source {
        Source::Skill(skill) => {
            if !is_safe_directory(&skill.directory) {
                return Err(problem(&skill.directory));
            }
            let path = safe_path(library.root(), &format!("{DIR_SKILLS}/{}", skill.directory))?;
            let tree = read_tree(&path)?;
            if !tree.contains_key("SKILL.md") {
                return Err(problem("SKILL.md missing"));
            }
            Ok(Content::Skill(tree))
        }
        Source::Mcp(server) => Ok(Content::Mcp(definition(server, mcp_target(agent)?)?)),
        Source::LocalSkill(local) => Ok(Content::Skill(read_tree(&safe_path(
            root,
            &local.source_path,
        )?)?)),
        Source::LocalMcp(_, server) => Ok(Content::Mcp(definition(server, mcp_target(agent)?)?)),
    }
}

/// 本地来源(非托管认领)解析为与库资源同构的 Source;来源缺失/不可解析时跳过(状态落 sourceMissing)。
fn local_sources(root: &Path, manifest: &Manifest, kind: &str) -> Vec<Source> {
    manifest
        .locals
        .iter()
        .filter(|l| l.kind == kind)
        .filter_map(|local| {
            if kind == "skills" {
                let path = safe_path(root, &local.source_path).ok()?;
                if !path.is_dir() {
                    return None;
                }
                let mut local = local.clone();
                if let Ok(content) = fs::read_to_string(path.join("SKILL.md")) {
                    local.description =
                        super::parse_skill_frontmatter(&content).1.unwrap_or_default();
                }
                Some(Source::LocalSkill(local))
            } else {
                let target = mcp_target(&local.origin_agent).ok()?;
                let value = McpDocument::read(root, target)
                    .ok()?
                    .entry(target, &local.name)
                    .ok()??;
                let input = parse_server_value(target, &local.name, &value).ok()?;
                Some(Source::LocalMcp(
                    local.clone(),
                    McpServer {
                        id: local.id.clone(),
                        name: input.name,
                        description: input.description,
                        transport: input.transport,
                        command: input.command,
                        args: input.args,
                        env: input.env,
                        url: input.url,
                        headers: input.headers,
                        enabled: input.enabled,
                        created_at: 0,
                        updated_at: 0,
                    },
                ))
            }
        })
        .collect()
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
                    } else if let Some(source) =
                        all.iter().find(|s| s.id() == entry.resource_id)
                    {
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

/// 命令入口公共前置:库与项目校验、manifest revision 并发检查(防多视图互相覆盖)。
fn prepare(
    app: &AppHandle,
    path: &str,
    kind: &str,
    expected_revision: &str,
) -> RlResult<(Library, PathBuf)> {
    let library = Library::app(app)?;
    library.ensure()?;
    let root = project_root(path)?;
    validate_kind(kind)?;
    let manifest = read_manifest(&manifest_file(&library, &root), &root)?;
    if manifest_revision(&manifest)? != expected_revision {
        return Err(problem(
            "configuration changed in another view; refresh before applying",
        ));
    }
    Ok((library, root))
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
fn add(library: &Library, root: &Path, kind: &str, ids: &[String]) -> RlResult<ApplyResult> {
    validate_kind(kind)?;
    let state_path = manifest_file(library, root);
    let mut state = read_manifest(&state_path, root)?;
    let (_, sources) = sources(library, kind)?;
    let not_found = if kind == "skills" {
        "resource_library_skill_not_found"
    } else {
        "resource_library_mcp_not_found"
    };
    let mut result = ApplyResult::default();
    for id in ids {
        if state
            .entries
            .iter()
            .any(|e| e.kind == kind && &e.resource_id == id)
            || state
                .shortlist
                .iter()
                .any(|s| s.kind == kind && &s.resource_id == id)
        {
            continue;
        }
        if !sources.iter().any(|s| s.id() == id) {
            result.failures.push(ApplyFailure {
                resource_id: id.clone(),
                code: not_found.to_string(),
                message: id.clone(),
            });
            continue;
        }
        state.shortlist.push(ShortlistEntry {
            kind: kind.to_string(),
            resource_id: id.clone(),
        });
        result.applied += 1;
    }
    if result.applied > 0 {
        save_manifest(&state_path, &state)?;
    }
    Ok(result)
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
fn assign(
    library: &Library,
    root: &Path,
    kind: &str,
    id: &str,
    agent_ids: &[String],
) -> RlResult<AssignResult> {
    validate_kind(kind)?;
    for agent in agent_ids {
        target(agent)?;
    }
    let state_path = manifest_file(library, root);
    let mut state = read_manifest(&state_path, root)?;
    // 来源被删除/上锁时仍允许解除配置;新增配置由 apply_one 拒绝。
    let list = sources(library, kind).ok().map(|(_, list)| list);
    let locals = local_sources(root, &state, kind);
    let source = list
        .as_ref()
        .and_then(|list| list.iter().find(|s| s.id() == id))
        .or_else(|| locals.iter().find(|s| s.id() == id));
    let wanted: HashSet<&str> = agent_ids.iter().map(String::as_str).collect();
    let mut result = AssignResult::default();
    for agent in TARGETS {
        let existing = state
            .entries
            .iter()
            .any(|e| e.kind == kind && e.agent_id == agent.id && e.resource_id == id);
        let selected = wanted.contains(agent.id);
        if !selected && !existing {
            continue;
        }
        match apply_one(
            library, root, &state_path, &mut state, kind, agent.id, id, selected, source,
        ) {
            Ok(changed) => result.applied += usize::from(changed),
            Err(e) => result.failures.push(AssignFailure {
                agent_id: agent.id.to_string(),
                code: e.code().to_string(),
                message: e.message(),
            }),
        }
    }
    // 配置过的资源视为已加入项目列表(本地来源由 locals 记录,不进 shortlist)。
    if source.is_some()
        && !id.starts_with(LOCAL_PREFIX)
        && !state
            .shortlist
            .iter()
            .any(|s| s.kind == kind && s.resource_id == id)
    {
        state.shortlist.push(ShortlistEntry {
            kind: kind.to_string(),
            resource_id: id.to_string(),
        });
        save_manifest(&state_path, &state)?;
    }
    Ok(result)
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
fn remove_resource(library: &Library, root: &Path, kind: &str, id: &str) -> RlResult<AssignResult> {
    validate_kind(kind)?;
    let state_path = manifest_file(library, root);
    let mut state = read_manifest(&state_path, root)?;
    let list = sources(library, kind).ok().map(|(_, list)| list);
    let locals = local_sources(root, &state, kind);
    let source = list
        .as_ref()
        .and_then(|list| list.iter().find(|s| s.id() == id))
        .or_else(|| locals.iter().find(|s| s.id() == id));
    let agents: Vec<String> = state
        .entries
        .iter()
        .filter(|e| e.kind == kind && e.resource_id == id)
        .map(|e| e.agent_id.clone())
        .collect();
    let mut result = AssignResult::default();
    for agent in agents {
        match apply_one(
            library, root, &state_path, &mut state, kind, &agent, id, false, source,
        ) {
            Ok(changed) => result.applied += usize::from(changed),
            Err(e) => result.failures.push(AssignFailure {
                agent_id: agent,
                code: e.code().to_string(),
                message: e.message(),
            }),
        }
    }
    if result.failures.is_empty() {
        let before = state.shortlist.len();
        state
            .shortlist
            .retain(|s| !(s.kind == kind && s.resource_id == id));
        let before_locals = state.locals.len();
        state.locals.retain(|l| !(l.kind == kind && l.id == id));
        if state.shortlist.len() != before || state.locals.len() != before_locals {
            save_manifest(&state_path, &state)?;
        }
    }
    Ok(result)
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

/// 认领入库:部署记录指纹取项目现状,来源与现状不一致时自然落「可更新」。
fn enlist(
    library: &Library,
    root: &Path,
    entry: Deployment,
) -> RlResult<()> {
    let state_path = manifest_file(library, root);
    let mut state = read_manifest(&state_path, root)?;
    let collided = state.entries.iter().any(|e| {
        e.kind == entry.kind
            && e.path == entry.path
            && (e.kind == "skills" || e.name == entry.name)
            && e.resource_id != entry.resource_id
    });
    if collided {
        return Err(problem(format!("resource collision: {}", entry.path)));
    }
    state.entries.retain(|e| {
        !(e.kind == entry.kind && e.agent_id == entry.agent_id && e.resource_id == entry.resource_id)
    });
    if !state
        .shortlist
        .iter()
        .any(|s| s.kind == entry.kind && s.resource_id == entry.resource_id)
    {
        state.shortlist.push(ShortlistEntry {
            kind: entry.kind.clone(),
            resource_id: entry.resource_id.clone(),
        });
    }
    state.entries.push(entry);
    save_manifest(&state_path, &state)
}

fn import_skill(library: &Library, root: &Path, source: &str) -> RlResult<ImportOutcome> {
    // source 必须是某个 Agent skills 目录下的安全目录名,据此归属 Agent。
    let agent = TARGETS
        .iter()
        .find_map(|t| {
            let prefix = format!("{}/", t.skill_path);
            source
                .strip_prefix(&prefix)
                .filter(|rest| is_safe_directory(rest))
                .map(|_| t)
        })
        .ok_or_else(|| problem(source))?;
    let tree = read_tree(&safe_path(root, source)?)?;
    let body = tree
        .get("SKILL.md")
        .ok_or_else(|| problem("SKILL.md missing"))?;
    let body = std::str::from_utf8(&body.bytes).map_err(|e| problem(e.to_string()))?;
    let (name, description) = super::parse_skill_frontmatter(body);
    let name = name
        .filter(|n| !n.trim().is_empty())
        .ok_or_else(|| problem(source))?;
    let data: SkillLibrary = library.read_plain_json(FILE_SKILLS)?;
    let (skill, existed) = if let Some(existing) = data.skills.iter().find(|s| s.name == name) {
        (existing.clone(), true)
    } else {
        let skill = crate::commands::ai::resource_library::import_skill(
            library,
            &name,
            description,
            body.to_string(),
        )?;
        // skill_create 只落 SKILL.md,其余文件按原样补齐(保留权限位)。
        let dest = safe_path(library.root(), &format!("{DIR_SKILLS}/{}", skill.directory))?;
        for (rel, file) in &tree {
            if rel == "SKILL.md" {
                continue;
            }
            let target = safe_path(&dest, rel)?;
            atomic_write(&target, &file.bytes)?;
            fs::set_permissions(&target, file.permissions.clone())?;
        }
        (skill, false)
    };
    migrate_local(&manifest_file(library, root), root, "skills", source, None, &skill.id)?;
    enlist(
        library,
        root,
        Deployment {
            kind: "skills".to_string(),
            agent_id: agent.id.to_string(),
            resource_id: skill.id.clone(),
            name: skill.name.clone(),
            path: source.to_string(),
            fingerprint: tree_hash(&tree),
        },
    )?;
    Ok(ImportOutcome {
        resource_id: skill.id,
        existed,
    })
}

fn import_mcp(library: &Library, root: &Path, source: &str, name: &str) -> RlResult<ImportOutcome> {
    let target = MCP_TARGETS
        .iter()
        .find(|t| t.path == source)
        .ok_or_else(|| problem(source))?;
    // 每个 MCP 配置文件固定归属一个 Agent。
    let agent = target.agents[0];
    let value = McpDocument::read(root, target)?
        .entry(target, name)?
        .ok_or_else(|| problem(name))?;
    let def = parse_server_value(target, name, &value)?;
    let list: Vec<McpServer> = library.read_mcp_json()?;
    let (server, existed) = if let Some(existing) = list.iter().find(|m| m.name == def.name) {
        (existing.clone(), true)
    } else {
        (crate::commands::ai::resource_library::import_mcp(library, &def)?, false)
    };
    migrate_local(&manifest_file(library, root), root, "mcp", source, Some(name), &server.id)?;
    enlist(
        library,
        root,
        Deployment {
            kind: "mcp".to_string(),
            agent_id: agent.to_string(),
            resource_id: server.id.clone(),
            name: name.to_string(),
            path: target.path.to_string(),
            fingerprint: json_hash(&value),
        },
    )?;
    Ok(ImportOutcome {
        resource_id: server.id,
        existed,
    })
}

/// 已认领的本地来源再导入资源库:部署条目改挂库资源 ID,移除本地来源记录。
fn migrate_local(
    state_path: &Path,
    root: &Path,
    kind: &str,
    source_path: &str,
    name: Option<&str>,
    new_id: &str,
) -> RlResult<()> {
    let mut state = read_manifest(state_path, root)?;
    let Some(pos) = state.locals.iter().position(|l| {
        l.kind == kind
            && l.source_path == source_path
            && (kind == "skills" || l.name == name.unwrap_or_default())
    }) else {
        return Ok(());
    };
    let local = state.locals.remove(pos);
    for entry in &mut state.entries {
        if entry.kind == kind && entry.resource_id == local.id {
            entry.resource_id = new_id.to_string();
        }
    }
    save_manifest(state_path, &state)
}

/// 本地来源的来源 Agent 只登记/解除记录:文件是用户原始内容,认领取现状指纹,
/// 解除保留文件;最后一个部署解除时连同本地记录清理,目录回到非托管。
fn apply_local_origin(
    root: &Path,
    state_path: &Path,
    state: &mut Manifest,
    local: &LocalResource,
    agent: &str,
    selected: bool,
) -> RlResult<bool> {
    let existed = state.entries.iter().any(|e| {
        e.kind == local.kind && e.agent_id == agent && e.resource_id == local.id
    });
    if selected == existed {
        return Ok(false);
    }
    let mut next = state.clone();
    next.entries.retain(|e| {
        !(e.kind == local.kind && e.agent_id == agent && e.resource_id == local.id)
    });
    if selected {
        let mut entry = Deployment {
            kind: local.kind.clone(),
            agent_id: agent.to_string(),
            resource_id: local.id.clone(),
            name: local.name.clone(),
            path: local.source_path.clone(),
            fingerprint: String::new(),
        };
        entry.fingerprint =
            current_hash(root, &entry)?.ok_or_else(|| problem("local source missing"))?;
        next.entries.push(entry);
    } else if !next.entries.iter().any(|e| e.resource_id == local.id) {
        next.locals
            .retain(|l| !(l.kind == local.kind && l.id == local.id));
    }
    save_manifest(state_path, &next)?;
    *state = next;
    Ok(true)
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

fn claim_local(
    library: &Library,
    root: &Path,
    kind: &str,
    source: &str,
    name: Option<&str>,
) -> RlResult<ClaimOutcome> {
    validate_kind(kind)?;
    let state_path = manifest_file(library, root);
    let mut state = read_manifest(&state_path, root)?;
    let (entry, local) = if kind == "skills" {
        // source 必须是某个 Agent skills 目录下的安全目录名,据此归属来源 Agent。
        let origin = TARGETS
            .iter()
            .find_map(|t| {
                let prefix = format!("{}/", t.skill_path);
                source
                    .strip_prefix(&prefix)
                    .filter(|rest| is_safe_directory(rest))
                    .map(|_| t)
            })
            .ok_or_else(|| problem(source))?;
        let tree = read_tree(&safe_path(root, source)?)?;
        let body = tree
            .get("SKILL.md")
            .ok_or_else(|| problem("SKILL.md missing"))?;
        let body = std::str::from_utf8(&body.bytes).map_err(|e| problem(e.to_string()))?;
        let (name, description) = super::parse_skill_frontmatter(body);
        let name = name
            .filter(|n| !n.trim().is_empty())
            .ok_or_else(|| problem(source))?;
        let id = format!("{LOCAL_PREFIX}skills:{source}");
        (
            Deployment {
                kind: kind.to_string(),
                agent_id: origin.id.to_string(),
                resource_id: id.clone(),
                name: name.clone(),
                path: source.to_string(),
                fingerprint: tree_hash(&tree),
            },
            LocalResource {
                kind: kind.to_string(),
                id,
                name,
                description: description.unwrap_or_default(),
                source_path: source.to_string(),
                origin_agent: origin.id.to_string(),
            },
        )
    } else {
        let name = name.ok_or_else(|| problem("MCP server name required"))?;
        let target = MCP_TARGETS
            .iter()
            .find(|t| t.path == source)
            .ok_or_else(|| problem(source))?;
        let value = McpDocument::read(root, target)?
            .entry(target, name)?
            .ok_or_else(|| problem(name))?;
        // 认领时校验可转换为通用定义,之后才能部署到其他 Agent。
        parse_server_value(target, name, &value)?;
        let id = format!("{LOCAL_PREFIX}mcp:{source}#{name}");
        (
            Deployment {
                kind: kind.to_string(),
                agent_id: target.agents[0].to_string(),
                resource_id: id.clone(),
                name: name.to_string(),
                path: source.to_string(),
                fingerprint: json_hash(&value),
            },
            LocalResource {
                kind: kind.to_string(),
                id,
                name: name.to_string(),
                description: String::new(),
                source_path: source.to_string(),
                origin_agent: target.agents[0].to_string(),
            },
        )
    };
    // 幂等:已认领直接返回,由调用方继续 assign 目标 Agent。
    if state
        .locals
        .iter()
        .any(|l| l.kind == kind && l.id == local.id)
    {
        return Ok(ClaimOutcome {
            resource_id: local.id,
        });
    }
    if state.entries.iter().any(|e| {
        e.kind == kind && e.path == entry.path && (kind == "skills" || e.name == entry.name)
    }) {
        return Err(problem(format!("resource collision: {}", entry.path)));
    }
    let resource_id = local.id.clone();
    state.locals.push(local);
    state.entries.push(entry);
    save_manifest(&state_path, &state)?;
    Ok(ClaimOutcome { resource_id })
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

fn delete_unmanaged(
    library: &Library,
    root: &Path,
    kind: &str,
    source: &str,
    name: Option<&str>,
) -> RlResult<()> {
    let state = read_manifest(&manifest_file(library, root), root)?;
    if kind == "skills" {
        // source 必须位于某个 Agent skills 目录下,且目录名为安全名(与认领同一套校验)。
        let valid = TARGETS.iter().any(|t| {
            let prefix = format!("{}/", t.skill_path);
            source
                .strip_prefix(&prefix)
                .filter(|rest| is_safe_directory(rest))
                .is_some()
        });
        if !valid {
            return Err(problem(source));
        }
        if state
            .entries
            .iter()
            .any(|e| e.kind == kind && e.path == source)
        {
            return Err(problem(format!("managed resource: {source}")));
        }
        replace_tree(root, source, None)
    } else {
        let name = name.ok_or_else(|| problem("MCP server name required"))?;
        let target = MCP_TARGETS
            .iter()
            .find(|t| t.path == source)
            .ok_or_else(|| problem(source))?;
        if state
            .entries
            .iter()
            .any(|e| e.kind == kind && e.path == source && e.name == name)
        {
            return Err(problem(format!("managed resource: {name}")));
        }
        let doc = McpDocument::read(root, target)?;
        if doc.entry(target, name)?.is_none() {
            return Err(problem(name));
        }
        let bytes = doc.edit(target, name, None)?;
        atomic_write(&safe_path(root, source)?, &bytes)
    }
}

/// selectedIds 是指定 Agent/种类的完整选择集;仅测试引用,命令面为 add/assign/remove。
#[cfg(test)]
fn apply(
    library: &Library,
    root: &Path,
    kind: &str,
    agent: &str,
    selected: &[String],
) -> RlResult<ApplyResult> {
    validate_kind(kind)?;
    target(agent)?;
    let state_path = manifest_file(library, root);
    let mut state = read_manifest(&state_path, root)?;
    let source_result = sources(library, kind);
    let selected: HashSet<_> = selected.iter().cloned().collect();
    let mut ids: Vec<_> = state
        .entries
        .iter()
        .filter(|e| e.kind == kind && e.agent_id == agent)
        .map(|e| e.resource_id.clone())
        .chain(selected.iter().cloned())
        .collect();
    ids.sort();
    ids.dedup();
    ids.sort_by_key(|id| selected.contains(id)); // 同批先移除,再处理同名来源替换。
    let mut result = ApplyResult::default();
    for id in ids {
        let source = source_result
            .as_ref()
            .ok()
            .and_then(|(_, list)| list.iter().find(|s| s.id() == id));
        let outcome = apply_one(
            library,
            root,
            &state_path,
            &mut state,
            kind,
            agent,
            &id,
            selected.contains(&id),
            source,
        );
        match outcome {
            Ok(changed) => result.applied += usize::from(changed),
            Err(e) => result.failures.push(ApplyFailure {
                resource_id: id,
                code: e.code().to_string(),
                message: e.message(),
            }),
        }
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
fn apply_one(
    library: &Library,
    root: &Path,
    state_path: &Path,
    state: &mut Manifest,
    kind: &str,
    agent: &str,
    id: &str,
    selected: bool,
    source: Option<&Source>,
) -> RlResult<bool> {
    let existing = state
        .entries
        .iter()
        .find(|e| e.kind == kind && e.agent_id == agent && e.resource_id == id)
        .cloned();
    // 本地来源(非托管认领)的来源 Agent:认领取现状指纹,解除仅删记录保留文件。
    if let Some(local) = state
        .locals
        .iter()
        .find(|l| l.kind == kind && l.id == id)
        .cloned()
    {
        if agent == local.origin_agent {
            return apply_local_origin(root, state_path, state, &local, agent, selected);
        }
    }
    // 来源被删除/上锁时保留已配置项,不把空来源列表当成卸载指令。
    if selected && source.is_none() {
        return if existing.is_some() {
            Ok(false)
        } else {
            Err(problem("resource unavailable; unlock or refresh library"))
        };
    }
    let mut entry = if let Some(entry) = existing.clone() {
        entry
    } else {
        let source = source.ok_or_else(|| problem(id))?;
        let path = match source {
            Source::Skill(skill) => {
                if !is_safe_directory(&skill.directory) {
                    return Err(problem(&skill.directory));
                }
                format!("{}/{}", target(agent)?.skill_path, skill.directory)
            }
            Source::Mcp(_) | Source::LocalMcp(..) => target(agent)?.mcp_path.to_string(),
            Source::LocalSkill(local) => {
                let dir = local
                    .source_path
                    .rsplit('/')
                    .next()
                    .filter(|d| is_safe_directory(d))
                    .ok_or_else(|| problem(&local.source_path))?;
                format!("{}/{}", target(agent)?.skill_path, dir)
            }
        };
        Deployment {
            kind: kind.to_string(),
            agent_id: agent.to_string(),
            resource_id: id.to_string(),
            name: source.choice().name,
            path,
            fingerprint: String::new(),
        }
    };
    let peers: Vec<_> = state
        .entries
        .iter()
        .filter(|e| {
            e.kind == kind
                && e.path == entry.path
                && (kind == "skills" || e.name == entry.name)
                && !(e.agent_id == agent && e.resource_id == id)
        })
        .collect();
    if peers.iter().any(|e| e.resource_id != id) {
        return Err(problem(format!("resource collision: {}", entry.path)));
    }
    // 共享目录仍有其他目标引用时只解除当前关联,不能删除实际内容。
    if !selected && !peers.is_empty() {
        let mut next = state.clone();
        next.entries
            .retain(|e| !(e.kind == kind && e.agent_id == agent && e.resource_id == id));
        save_manifest(state_path, &next)?;
        *state = next;
        return Ok(true);
    }
    let current = current_hash(root, &entry)?;
    let baseline = existing.as_ref().or_else(|| peers.first().copied());
    if let Some(current) = &current {
        if baseline.is_none_or(|previous| previous.fingerprint != *current) {
            return Err(problem(format!(
                "unmanaged or locally modified: {} / {}",
                entry.path, entry.name
            )));
        }
    }
    let content = if selected {
        Some(source_content(library, root, source.unwrap(), agent)?)
    } else {
        None
    };
    let desired = content.as_ref().map(Content::fingerprint);
    if existing.is_some() && selected && current == desired {
        return Ok(false);
    }
    entry.fingerprint = desired.clone().unwrap_or_default();
    let mut next = state.clone();
    next.entries
        .retain(|e| !(e.kind == kind && e.agent_id == agent && e.resource_id == id));
    if selected {
        for peer in &mut next.entries {
            if peer.kind == kind
                && peer.path == entry.path
                && (kind == "skills" || peer.name == entry.name)
            {
                peer.fingerprint = entry.fingerprint.clone();
            }
        }
        next.entries.push(entry.clone());
    }
    // 添加/更新先持久化意图,崩溃后旧内容会标记 modified 而非被误认领。
    // 移除先清理文件再解除记录,崩溃后会标记 missing,再次移除幂等。
    if selected {
        save_manifest(state_path, &next)?;
    }
    let write_result: RlResult<()> = (|| {
        if current_hash(root, &entry)? != current {
            return Err(problem("project changed during apply; refresh"));
        }
        if kind == "skills" {
            let tree = match &content {
                Some(Content::Skill(tree)) => Some(tree),
                _ => None,
            };
            if current.is_some() || tree.is_some() {
                replace_tree(root, &entry.path, tree)?;
            }
        } else if current.is_some() || selected {
            let t = mcp_target(agent)?;
            let doc = McpDocument::read(root, t)?;
            if doc.entry(t, &entry.name)?.as_ref().map(json_hash) != current
                || !doc.unchanged(root, t)?
            {
                return Err(problem("MCP changed during apply; refresh"));
            }
            let value = match &content {
                Some(Content::Mcp(value)) => Some(value),
                _ => None,
            };
            let bytes = doc.edit(t, &entry.name, value)?;
            atomic_write(&safe_path(root, &entry.path)?, &bytes)?;
        }
        Ok(())
    })();
    if let Err(e) = write_result {
        if selected {
            save_manifest(state_path, state)?;
        }
        return Err(e);
    }
    if !selected {
        save_manifest(state_path, &next)?;
    }
    *state = next;
    Ok(true)
}

#[cfg(test)]
#[path = "deployment_tests.rs"]
mod tests;
