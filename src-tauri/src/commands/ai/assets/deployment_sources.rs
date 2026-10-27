//! 全局及本地资源来源的解析与内容读取。
use super::*;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceChoice {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) description: String,
    pub(super) group_ids: Vec<String>,
    pub(super) supported_agents: Vec<String>,
    /// 市场技能来源(owner/repo);手动创建技能与 MCP 无此字段,前端按来源附加分组。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) source: Option<String>,
    /// 本地来源技能的全部来源目录(含主来源,按 Agent 目标顺序);非本地技能无此字段。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) source_dirs: Option<Vec<String>>,
}
pub(super) enum Source {
    Skill(Skill),
    Mcp(McpServer),
    /// 本地来源技能:内容以项目内来源目录为准。
    LocalSkill(LocalResource),
    /// 本地来源 MCP:附带来源配置解析出的通用定义。
    LocalMcp(LocalResource, McpServer),
}
impl Source {
    pub(super) fn id(&self) -> &str {
        match self {
            Self::Skill(s) => &s.id,
            Self::Mcp(s) => &s.id,
            Self::LocalSkill(l) | Self::LocalMcp(l, _) => &l.id,
        }
    }
    pub(super) fn choice(&self) -> ResourceChoice {
        match self {
            Self::Skill(s) => ResourceChoice {
                id: s.id.clone(),
                name: s.name.clone(),
                description: s.description.clone(),
                group_ids: s.group_ids.clone(),
                supported_agents: TARGETS.iter().map(|t| t.id.to_string()).collect(),
                source: s.marketplace.as_ref().map(|m| m.source.clone()),
                source_dirs: None,
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
                source_dirs: None,
            },
            Self::LocalSkill(l) => ResourceChoice {
                id: l.id.clone(),
                name: l.name.clone(),
                description: l.description.clone(),
                group_ids: vec![],
                supported_agents: TARGETS.iter().map(|t| t.id.to_string()).collect(),
                source: None,
                source_dirs: Some(l.source_dirs()),
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
                source_dirs: None,
            },
        }
    }
}
pub(super) fn sources(library: &Library, kind: &str) -> RlResult<(Vec<SkillGroup>, Vec<Source>)> {
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

pub(super) enum Content {
    Skill(SkillTree),
    Mcp(Value),
}
impl Content {
    pub(super) fn fingerprint(&self) -> String {
        match self {
            Self::Skill(tree) => tree_hash(tree),
            Self::Mcp(value) => json_hash(value),
        }
    }
}
pub(super) fn source_content(
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
pub(super) fn local_sources(root: &Path, manifest: &Manifest, kind: &str) -> Vec<Source> {
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
                    local.description = super::super::parse_skill_frontmatter(&content)
                        .1
                        .unwrap_or_default();
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
