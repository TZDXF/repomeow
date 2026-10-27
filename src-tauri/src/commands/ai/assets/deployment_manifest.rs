//! 项目部署清单的读取、校验与持久化。
use super::*;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Deployment {
    pub(super) kind: String,
    pub(super) agent_id: String,
    pub(super) resource_id: String,
    pub(super) name: String,
    pub(super) path: String,
    pub(super) fingerprint: String,
}
/// 项目资源列表条目:先「添加」加入列表(shortlist),再按 Agent 配置部署(entries)。
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortlistEntry {
    pub(super) kind: String,
    pub(super) resource_id: String,
}
/// 本地来源资源 ID 前缀,与资源库 ID 区分。
pub(super) const LOCAL_PREFIX: &str = "local:";

/// 本地来源(非托管认领):记录 skills/mcp 的来源路径与归属 Agent,不进全局资源库。
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LocalResource {
    pub(super) kind: String,
    /// `local:skills:<目录>` / `local:mcp:<配置文件>#<服务器名>`。
    pub(super) id: String,
    pub(super) name: String,
    #[serde(default)]
    pub(super) description: String,
    /// 来源位置:skills = 技能目录(主来源);mcp = 配置文件路径。
    pub(super) source_path: String,
    /// 同名技能散落在多个 Agent 目录的全部来源实例(skills 专用,含主来源);
    /// 空数组按 source_path 单来源处理(兼容旧清单)。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) source_paths: Vec<String>,
    /// 来源路径归属的 Agent(该 Agent 只登记记录,文件保持原样)。
    pub(super) origin_agent: String,
}
/// 来源目录按 Agent 目标顺序稳定排列并去重。
pub(super) fn order_source_dirs(mut dirs: Vec<String>) -> Vec<String> {
    let rank = |dir: &str| {
        TARGETS
            .iter()
            .position(|t| Path::new(dir).parent() == Some(Path::new(t.skill_path)))
            .unwrap_or(TARGETS.len())
    };
    dirs.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.cmp(b)));
    dirs.dedup();
    dirs
}
impl LocalResource {
    /// 全部来源实例目录(skills):含主来源,按 Agent 目标顺序稳定排列;mcp 恒为单元素。
    pub(super) fn source_dirs(&self) -> Vec<String> {
        let dirs = if self.source_paths.is_empty() {
            vec![self.source_path.clone()]
        } else {
            self.source_paths.clone()
        };
        order_source_dirs(dirs)
    }
    /// 该 Agent 名下的来源实例目录(目录父级为其 skills 根);非来源 Agent 返回 None。
    pub(super) fn source_dir_for(&self, agent: &str) -> Option<String> {
        let skill_path = target(agent).ok()?.skill_path;
        self.source_dirs()
            .into_iter()
            .find(|d| Path::new(d).parent() == Some(Path::new(skill_path)))
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Manifest {
    pub(super) version: u32,
    pub(super) project_path: String,
    pub(super) entries: Vec<Deployment>,
    #[serde(default)]
    pub(super) shortlist: Vec<ShortlistEntry>,
    #[serde(default)]
    pub(super) locals: Vec<LocalResource>,
}

pub(super) fn manifest_file(library: &Library, root: &Path) -> PathBuf {
    let key = crate::path_util::clean_str(&root.to_string_lossy());
    library
        .root()
        .parent()
        .unwrap()
        .join("project-ai")
        .join(format!("{}.json", hash(key.as_bytes())))
}
pub(super) fn read_manifest(path: &Path, root: &Path) -> RlResult<Manifest> {
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
                && p.file_name()
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
        // 同名多副本来源目录也必须是某个 Agent skills 根下的安全目录。
        for dir in &local.source_paths {
            let p = Path::new(dir);
            let owned = TARGETS
                .iter()
                .any(|t| p.parent() == Some(Path::new(t.skill_path)));
            if local.kind != "skills"
                || !owned
                || !p
                    .file_name()
                    .is_some_and(|n| is_safe_directory(&n.to_string_lossy()))
            {
                return Err(problem("invalid local resource record"));
            }
        }
    }
    let mut shortlisted = HashSet::new();
    for item in &manifest.shortlist {
        validate_kind(&item.kind)?;
        if item.resource_id.is_empty() || !shortlisted.insert((&item.kind, &item.resource_id)) {
            return Err(problem("invalid shortlist record"));
        }
    }
    Ok(manifest)
}
pub(super) fn manifest_revision(manifest: &Manifest) -> RlResult<String> {
    Ok(hash(
        &serde_json::to_vec(manifest).map_err(|e| problem(e.to_string()))?,
    ))
}
pub(super) fn save_manifest(path: &Path, manifest: &Manifest) -> RlResult<()> {
    atomic_write(
        path,
        &serde_json::to_vec_pretty(manifest).map_err(|e| problem(e.to_string()))?,
    )
}
