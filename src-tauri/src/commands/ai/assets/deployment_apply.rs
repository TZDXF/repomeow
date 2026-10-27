//! 项目资源的登记、部署、认领与删除操作。
use super::*;

/// 命令入口公共前置:库与项目校验、manifest revision 并发检查(防多视图互相覆盖)。
pub(super) fn prepare(
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

pub(super) fn add(
    library: &Library,
    root: &Path,
    kind: &str,
    ids: &[String],
) -> RlResult<ApplyResult> {
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

pub(super) fn assign(
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
    // 来源被删除/损坏时仍允许解除配置;新增配置由 apply_one 拒绝。
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
            library,
            root,
            &state_path,
            &mut state,
            kind,
            agent.id,
            id,
            selected,
            source,
            false,
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

pub(super) fn remove_resource(
    library: &Library,
    root: &Path,
    kind: &str,
    id: &str,
) -> RlResult<AssignResult> {
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
            library,
            root,
            &state_path,
            &mut state,
            kind,
            &agent,
            id,
            false,
            source,
            false,
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

pub(super) fn repair(
    library: &Library,
    root: &Path,
    kind: &str,
    id: &str,
    agent: &str,
    action: &str,
) -> RlResult<()> {
    validate_kind(kind)?;
    target(agent)?;
    let state_path = manifest_file(library, root);
    let mut state = read_manifest(&state_path, root)?;
    if !state
        .entries
        .iter()
        .any(|e| e.kind == kind && e.agent_id == agent && e.resource_id == id)
    {
        return Err(problem(format!("not deployed: {agent} / {id}")));
    }
    match action {
        "detach" => {
            let mut next = state.clone();
            next.entries
                .retain(|e| !(e.kind == kind && e.agent_id == agent && e.resource_id == id));
            // 与 apply_local_origin 一致:本地来源最后一个部署解除时连同本地记录清理。
            if id.starts_with(LOCAL_PREFIX) && !next.entries.iter().any(|e| e.resource_id == id) {
                next.locals.retain(|l| !(l.kind == kind && l.id == id));
            }
            save_manifest(&state_path, &next)
        }
        "reapply" => {
            // 本地来源的主来源 Agent:来源即文件本身,修复 = 把现状认领为新基线;
            // skills 同名副本不在此列,走下方 force 部署以主来源内容覆盖统一。
            if let Some(local) = state
                .locals
                .iter()
                .find(|l| l.kind == kind && l.id == id)
                .cloned()
            {
                if agent == local.origin_agent {
                    let mut next = state.clone();
                    let entry = next
                        .entries
                        .iter_mut()
                        .find(|e| e.kind == kind && e.agent_id == agent && e.resource_id == id)
                        .ok_or_else(|| problem(id))?;
                    entry.fingerprint = current_hash(root, entry)?
                        .ok_or_else(|| problem("local source missing"))?;
                    return save_manifest(&state_path, &next);
                }
            }
            let list = sources(library, kind).ok().map(|(_, list)| list);
            let locals = local_sources(root, &state, kind);
            let source = list
                .as_ref()
                .and_then(|list| list.iter().find(|s| s.id() == id))
                .or_else(|| locals.iter().find(|s| s.id() == id))
                .ok_or_else(|| problem("source unavailable; cannot reapply"))?;
            apply_one(
                library,
                root,
                &state_path,
                &mut state,
                kind,
                agent,
                id,
                true,
                Some(source),
                true,
            )?;
            Ok(())
        }
        _ => Err(problem(format!("unknown repair action: {action}"))),
    }
}

/// 认领入库:部署记录指纹取项目现状,来源与现状不一致时自然落「可更新」。
pub(super) fn enlist(library: &Library, root: &Path, entry: Deployment) -> RlResult<()> {
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
        !(e.kind == entry.kind
            && e.agent_id == entry.agent_id
            && e.resource_id == entry.resource_id)
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

pub(super) fn import_skill(
    library: &Library,
    root: &Path,
    source: &str,
) -> RlResult<ImportOutcome> {
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
    let (name, description) = super::super::parse_skill_frontmatter(body);
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
    migrate_local(
        &manifest_file(library, root),
        root,
        "skills",
        source,
        None,
        &skill.id,
    )?;
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

pub(super) fn import_mcp(
    library: &Library,
    root: &Path,
    source: &str,
    name: &str,
) -> RlResult<ImportOutcome> {
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
        (
            crate::commands::ai::resource_library::import_mcp(library, &def)?,
            false,
        )
    };
    migrate_local(
        &manifest_file(library, root),
        root,
        "mcp",
        source,
        Some(name),
        &server.id,
    )?;
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
pub(super) fn migrate_local(
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
            && l.source_dirs().iter().any(|d| d == source_path)
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

/// 本地来源的来源 Agent(含同名副本所在目录)只登记/解除记录:文件是用户原始内容,
/// 认领取现状指纹,解除保留文件;最后一个部署解除时连同本地记录清理,目录回到非托管。
pub(super) fn apply_local_origin(
    root: &Path,
    state_path: &Path,
    state: &mut Manifest,
    local: &LocalResource,
    agent: &str,
    path: &str,
    selected: bool,
) -> RlResult<bool> {
    let existed = state
        .entries
        .iter()
        .any(|e| e.kind == local.kind && e.agent_id == agent && e.resource_id == local.id);
    if selected == existed {
        return Ok(false);
    }
    let mut next = state.clone();
    next.entries
        .retain(|e| !(e.kind == local.kind && e.agent_id == agent && e.resource_id == local.id));
    if selected {
        let mut entry = Deployment {
            kind: local.kind.clone(),
            agent_id: agent.to_string(),
            resource_id: local.id.clone(),
            name: local.name.clone(),
            path: path.to_string(),
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

pub(super) fn claim_local(
    library: &Library,
    root: &Path,
    kind: &str,
    source: &str,
    name: Option<&str>,
) -> RlResult<ClaimOutcome> {
    validate_kind(kind)?;
    let state_path = manifest_file(library, root);
    let mut state = read_manifest(&state_path, root)?;
    if kind == "skills" {
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
        let (name, description) = super::super::parse_skill_frontmatter(body);
        let name = name
            .filter(|n| !n.trim().is_empty())
            .ok_or_else(|| problem(source))?;
        // 同名技能散落在多个 Agent 目录时视为一体:认领时扫描全部同名副本,逐目录按现状登记。
        let dir_name = Path::new(source)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .ok_or_else(|| problem(source))?;
        let mut copies: Vec<(String, String, String)> = vec![];
        for t in TARGETS {
            let dir = format!("{}/{}", t.skill_path, dir_name);
            let Ok(path) = safe_path(root, &dir) else {
                continue;
            };
            if !path.is_dir() || !path.join("SKILL.md").is_file() {
                continue;
            }
            let Ok(copy_tree) = read_tree(&path) else {
                continue;
            };
            copies.push((t.id.to_string(), dir, tree_hash(&copy_tree)));
        }
        let local = LocalResource {
            kind: kind.to_string(),
            id: format!("{LOCAL_PREFIX}skills:{source}"),
            name,
            description: description.unwrap_or_default(),
            source_path: source.to_string(),
            source_paths: order_source_dirs(copies.iter().map(|(_, dir, _)| dir.clone()).collect()),
            origin_agent: origin.id.to_string(),
        };
        return claim_local_skill(&state_path, &mut state, local, copies);
    }
    let (entry, local) = {
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
                source_paths: vec![],
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

/// skills 认领落库:同名(跨 Agent 目录)已认领时并入既有来源记录并补登记新副本,
/// 否则新建本地来源;每个已存在副本按各自现状指纹登记部署,来源 Agent 不写文件。
/// copies = (agent_id, 目录, 目录现状指纹),按 TARGETS 顺序。
pub(super) fn claim_local_skill(
    state_path: &Path,
    state: &mut Manifest,
    local: LocalResource,
    copies: Vec<(String, String, String)>,
) -> RlResult<ClaimOutcome> {
    let existing = state
        .locals
        .iter()
        .position(|l| l.kind == "skills" && l.name == local.name);
    let id = existing
        .map(|pos| state.locals[pos].id.clone())
        .unwrap_or_else(|| local.id.clone());
    // 冲突:副本目录已被其他资源托管。
    for (_, dir, _) in &copies {
        if state
            .entries
            .iter()
            .any(|e| e.kind == "skills" && e.path == *dir && e.resource_id != id)
        {
            return Err(problem(format!("resource collision: {dir}")));
        }
    }
    // 补登记尚未登记的副本(各副本指纹独立,内容分叉时自然落 modified/update)。
    for (agent, dir, fingerprint) in &copies {
        if state
            .entries
            .iter()
            .any(|e| e.kind == "skills" && e.agent_id == *agent && e.resource_id == id)
        {
            continue;
        }
        state.entries.push(Deployment {
            kind: "skills".to_string(),
            agent_id: agent.clone(),
            resource_id: id.clone(),
            name: local.name.clone(),
            path: dir.clone(),
            fingerprint: fingerprint.clone(),
        });
    }
    if let Some(pos) = existing {
        let merged = &mut state.locals[pos];
        let mut dirs = merged.source_dirs();
        for (_, dir, _) in &copies {
            if !dirs.contains(dir) {
                dirs.push(dir.clone());
            }
        }
        merged.source_paths = order_source_dirs(dirs);
        if merged.description.is_empty() {
            merged.description = local.description.clone();
        }
    } else {
        state.locals.push(local);
    }
    save_manifest(state_path, state)?;
    Ok(ClaimOutcome { resource_id: id })
}

pub(super) fn delete_unmanaged(
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
pub(super) fn apply(
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
            false,
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
pub(super) fn apply_one(
    library: &Library,
    root: &Path,
    state_path: &Path,
    state: &mut Manifest,
    kind: &str,
    agent: &str,
    id: &str,
    selected: bool,
    source: Option<&Source>,
    force: bool,
) -> RlResult<bool> {
    let existing = state
        .entries
        .iter()
        .find(|e| e.kind == kind && e.agent_id == agent && e.resource_id == id)
        .cloned();
    // 本地来源(非托管认领):主来源 Agent 只登记/解除(来源即文件本身,认领/编辑取现状,
    // 解除保留文件);skills 同名副本所在 Agent 解除同样只删记录保留文件,
    // 但选中时走正常部署管线——以主来源内容写入,分叉副本由此可「应用更新」统一。
    if let Some(local) = state
        .locals
        .iter()
        .find(|l| l.kind == kind && l.id == id)
        .cloned()
    {
        if agent == local.origin_agent {
            let dir = if local.kind == "skills" {
                local
                    .source_dir_for(agent)
                    .unwrap_or_else(|| local.source_path.clone())
            } else {
                local.source_path.clone()
            };
            return apply_local_origin(root, state_path, state, &local, agent, &dir, selected);
        }
        if local.kind == "skills" && !selected {
            if let Some(dir) = local.source_dir_for(agent) {
                return apply_local_origin(root, state_path, state, &local, agent, &dir, false);
            }
        }
    }
    // 来源被删除/损坏时保留已配置项,不把空来源列表当成卸载指令。
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
    // force(repair 的覆盖更新)跳过基线校验:用户显式确认丢弃本地修改。
    if !force {
        if let Some(current) = &current {
            if baseline.is_none_or(|previous| previous.fingerprint != *current) {
                return Err(problem(format!(
                    "unmanaged or locally modified: {} / {}",
                    entry.path, entry.name
                )));
            }
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
