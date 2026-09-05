//! 资源库业务操作:Skill 多分组 CRUD(恒明文)、正文读写、通用 MCP CRUD
//! (唯一加密对象)、加密开关。
//!
//! 命令层(mod.rs)负责 AppHandle 解析、进程内互斥与后台自动同步;
//! 本层纯 `Library` 驱动,可直接单测。所有写操作自动 git init + 快照提交
//! (写操作成功但网络同步失败不影响本地结果,同步结果另走 git.rs)。

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use crate::commands::open;
use crate::time_util::now_ts;

use super::crypto::{
    clear_key, derive_key, encrypt_bytes, is_unlocked, key_for, make_key_check, new_salt,
    store_key, verify_key_check,
};
use super::errors::{codes, RlError, RlResult};
use super::frontmatter as fm;
use super::git;
use super::models::{
    EncryptionStatus, LibraryInfo, MarketplaceSource, McpServer, McpServerInput, Skill, SkillBody,
    SkillGroup, SkillLibrary, SyncOutcome, TRANSPORTS,
};
use super::store::{is_safe_directory, Library, DIR_SKILLS, FILE_SKILLS};

pub(super) fn new_id(prefix: &str) -> String {
    let mut buf = [0u8; 8];
    getrandom::fill(&mut buf).expect("系统随机源不可用");
    let hex: String = buf.iter().map(|b| format!("{b:02x}")).collect();
    format!("{prefix}_{hex}")
}

/// 校验并规范化分组颜色:#RRGGBB(大小写不敏感,统一小写);空串 = 无颜色
fn normalize_color(color: Option<String>) -> RlResult<Option<String>> {
    match color {
        None => Ok(None),
        Some(c) => {
            let c = c.trim();
            if c.is_empty() {
                return Ok(None);
            }
            if c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|x| x.is_ascii_hexdigit()) {
                Ok(Some(c.to_ascii_lowercase()))
            } else {
                Err(RlError::coded(codes::GROUP_COLOR_INVALID, c.to_string()))
            }
        }
    }
}

/// 去除重复分组 id(保序)
fn dedup(ids: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    ids.into_iter()
        .filter(|id| seen.insert(id.clone()))
        .collect()
}

fn validate_group_ids(data: &SkillLibrary, group_ids: &[String]) -> RlResult<()> {
    for gid in group_ids {
        if !data.groups.iter().any(|g| &g.id == gid) {
            return Err(RlError::coded(codes::GROUP_NOT_FOUND, gid.to_string()));
        }
    }
    Ok(())
}

/// 校验并规范化 MCP 输入(transport 判别符 + 各 transport 的必备字段)
fn validate_mcp(def: &McpServerInput) -> RlResult<McpServerInput> {
    let mut def = def.clone();
    def.name = def.name.trim().to_string();
    def.transport = def.transport.trim().to_string();
    if def.name.is_empty() {
        return Err(RlError::coded(codes::MCP_NAME_REQUIRED, ""));
    }
    if !TRANSPORTS.contains(&def.transport.as_str()) {
        return Err(RlError::coded(
            codes::MCP_TRANSPORT_INVALID,
            def.transport.clone(),
        ));
    }
    if def.transport == "stdio" && def.command.as_deref().unwrap_or("").trim().is_empty() {
        return Err(RlError::coded(codes::MCP_COMMAND_REQUIRED, ""));
    }
    if (def.transport == "http" || def.transport == "sse")
        && def.url.as_deref().unwrap_or("").trim().is_empty()
    {
        return Err(RlError::coded(codes::MCP_URL_REQUIRED, ""));
    }
    Ok(def)
}

// ── 查询 ───────────────────────────────────────────────────────────────

pub(super) fn library_open_dir(lib: &Library) -> RlResult<()> {
    lib.ensure()?;
    open::open_explorer(&lib.root().to_string_lossy()).map_err(Into::into)
}

pub(super) fn library_info(lib: &Library) -> RlResult<LibraryInfo> {
    lib.ensure()?;
    let meta = lib.meta()?;
    let unlocked = is_unlocked(lib.root());
    // skills 恒明文,计数始终可用;MCP 加密未解锁时计数取 0(unlocked 已透出)
    let skills: SkillLibrary = lib.read_plain_json(FILE_SKILLS).unwrap_or_default();
    let mcp_count = if !meta.encrypted || unlocked {
        lib.read_mcp_json::<Vec<McpServer>>()
            .map(|list| list.len() as u32)
            .unwrap_or(0)
    } else {
        0
    };
    Ok(LibraryInfo {
        root: lib.root().to_string_lossy().into_owned(),
        version: meta.version,
        encrypted: meta.encrypted,
        unlocked,
        git_initialized: git::is_repo(lib),
        git_dirty: if git::is_repo(lib) {
            git::dirty(lib).unwrap_or(false)
        } else {
            false
        },
        remote_url: git::remote_get(lib).unwrap_or(None),
        branch: git::branch(lib).unwrap_or(None),
        skill_count: skills.skills.len() as u32,
        group_count: skills.groups.len() as u32,
        mcp_count,
        last_sync: lib.read_state().last_sync,
    })
}

pub(super) fn encryption_status(lib: &Library) -> RlResult<EncryptionStatus> {
    lib.ensure()?;
    let meta = lib.meta()?;
    Ok(EncryptionStatus {
        enabled: meta.encrypted,
        unlocked: is_unlocked(lib.root()),
    })
}

pub(super) fn skill_list(lib: &Library) -> RlResult<SkillLibrary> {
    lib.ensure()?;
    lib.read_plain_json(FILE_SKILLS)
}

pub(super) fn mcp_list(lib: &Library) -> RlResult<Vec<McpServer>> {
    lib.ensure()?;
    lib.read_mcp_json()
}

// ── 分组 CRUD ──────────────────────────────────────────────────────────

pub(super) fn group_create(
    lib: &Library,
    name: &str,
    color: Option<String>,
) -> RlResult<SkillGroup> {
    lib.ensure()?;
    let name = name.trim();
    if name.is_empty() {
        return Err(RlError::coded(codes::GROUP_NAME_REQUIRED, ""));
    }
    let color = normalize_color(color)?;
    let mut data: SkillLibrary = lib.read_plain_json(FILE_SKILLS)?;
    if data.groups.iter().any(|g| g.name == name) {
        return Err(RlError::coded(codes::GROUP_NAME_CONFLICT, name.to_string()));
    }
    let ts = now_ts();
    let sort = data
        .groups
        .iter()
        .map(|g| g.sort)
        .max()
        .map_or(0, |m| m + 1);
    let group = SkillGroup {
        id: new_id("grp"),
        name: name.to_string(),
        color,
        sort,
        created_at: ts,
        updated_at: ts,
    };
    data.groups.push(group.clone());
    lib.write_plain_json(FILE_SKILLS, &data)?;
    git::auto_commit(lib, &format!("新增分组:{name}"))?;
    Ok(group)
}

/// 分组更新(改名 + 颜色):color 为 None 时保持原值,Some 覆盖(空串清除)
pub(super) fn group_rename(
    lib: &Library,
    id: &str,
    name: &str,
    color: Option<String>,
) -> RlResult<SkillGroup> {
    lib.ensure()?;
    let name = name.trim();
    if name.is_empty() {
        return Err(RlError::coded(codes::GROUP_NAME_REQUIRED, ""));
    }
    let color_requested = color.is_some();
    let color = normalize_color(color)?;
    let mut data: SkillLibrary = lib.read_plain_json(FILE_SKILLS)?;
    if data.groups.iter().any(|g| g.name == name && g.id != id) {
        return Err(RlError::coded(codes::GROUP_NAME_CONFLICT, name.to_string()));
    }
    let old = data
        .groups
        .iter()
        .find(|g| g.id == id)
        .ok_or_else(|| RlError::coded(codes::GROUP_NOT_FOUND, id.to_string()))?
        .name
        .clone();
    let group = data
        .groups
        .iter_mut()
        .find(|g| g.id == id)
        .expect("已校验存在");
    group.name = name.to_string();
    if color_requested {
        group.color = color;
    }
    group.updated_at = now_ts();
    let result = group.clone();
    lib.write_plain_json(FILE_SKILLS, &data)?;
    git::auto_commit(lib, &format!("重命名分组:{old} → {name}"))?;
    Ok(result)
}

/// 删除分组:不做空检查,自动从所有技能解除该分组关联(允许无分组)
pub(super) fn group_delete(lib: &Library, id: &str) -> RlResult<()> {
    lib.ensure()?;
    let mut data: SkillLibrary = lib.read_plain_json(FILE_SKILLS)?;
    let Some(index) = data.groups.iter().position(|g| g.id == id) else {
        return Err(RlError::coded(codes::GROUP_NOT_FOUND, id.to_string()));
    };
    let removed = data.groups.remove(index);
    let ts = now_ts();
    for skill in &mut data.skills {
        let before = skill.group_ids.len();
        skill.group_ids.retain(|gid| gid != id);
        if skill.group_ids.len() != before {
            skill.updated_at = ts;
        }
    }
    lib.write_plain_json(FILE_SKILLS, &data)?;
    git::auto_commit(lib, &format!("删除分组:{}", removed.name))?;
    Ok(())
}

/// 全量重排:ids 必须与现有分组 id 集合完全一致(排序按数组顺序)
pub(super) fn group_reorder(lib: &Library, ids: &[String]) -> RlResult<()> {
    lib.ensure()?;
    let mut data: SkillLibrary = lib.read_plain_json(FILE_SKILLS)?;
    let existing: HashSet<&str> = data.groups.iter().map(|g| g.id.as_str()).collect();
    if ids.len() != existing.len() {
        return Err(RlError::coded(codes::GROUP_NOT_FOUND, "分组数量不一致"));
    }
    for gid in ids {
        if !existing.contains(gid.as_str()) {
            return Err(RlError::coded(codes::GROUP_NOT_FOUND, gid.to_string()));
        }
    }
    let ts = now_ts();
    for (index, gid) in ids.iter().enumerate() {
        if let Some(group) = data.groups.iter_mut().find(|g| &g.id == gid) {
            group.sort = index as u32;
            group.updated_at = ts;
        }
    }
    lib.write_plain_json(FILE_SKILLS, &data)?;
    git::auto_commit(lib, "调整分组排序")?;
    Ok(())
}

// ── 技能 CRUD(多分组)──────────────────────────────────────────────────

pub(super) fn skill_create(
    lib: &Library,
    name: &str,
    description: Option<String>,
    group_ids: Vec<String>,
    body: Option<String>,
) -> RlResult<Skill> {
    lib.ensure()?;
    let name = name.trim();
    if name.is_empty() {
        return Err(RlError::coded(codes::SKILL_NAME_REQUIRED, ""));
    }
    let mut data: SkillLibrary = lib.read_plain_json(FILE_SKILLS)?;
    if data.skills.iter().any(|s| s.name == name) {
        return Err(RlError::coded(codes::SKILL_NAME_CONFLICT, name.to_string()));
    }
    let group_ids = dedup(group_ids);
    validate_group_ids(&data, &group_ids)?;
    let ts = now_ts();
    let id = new_id("sk");
    let sort_order = data
        .skills
        .iter()
        .map(|s| s.sort_order)
        .max()
        .map_or(0, |m| m + 1);
    let skill = Skill {
        id: id.clone(),
        directory: id,
        name: name.to_string(),
        description: description.unwrap_or_default(),
        marketplace: None,
        group_ids,
        sort_order,
        created_at: ts,
        updated_at: ts,
    };
    // SKILL.md frontmatter 为 name/description 事实源:
    // 无正文时生成最小 frontmatter;有正文但无 frontmatter 时前置;自带则原样保留
    match body {
        Some(body) if !fm::starts_with_frontmatter(&body) => {
            let content = format!("{}{}", fm::build(&skill.name, &skill.description), body);
            lib.write_body(&skill.directory, &content)?;
        }
        Some(body) => lib.write_body(&skill.directory, &body)?,
        None => {
            let content = fm::build(&skill.name, &skill.description);
            lib.write_body(&skill.directory, &content)?;
        }
    }
    data.skills.push(skill.clone());
    lib.write_plain_json(FILE_SKILLS, &data)?;
    git::auto_commit(lib, &format!("新增技能:{name}"))?;
    Ok(skill)
}

/// 导入市场 Skill：同一市场条目幂等返回已有项，不覆盖用户编辑的正文。
pub(super) fn skill_import_marketplace(
    lib: &Library,
    source: MarketplaceSource,
    body: String,
) -> RlResult<Skill> {
    lib.ensure()?;
    let (frontmatter_name, frontmatter_description) = fm::name_description_of(&body);
    let name = frontmatter_name
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| RlError::coded(codes::MARKETPLACE_SKILL_INVALID, "缺少 frontmatter name"))?;
    let description = frontmatter_description.unwrap_or_default();
    let mut data: SkillLibrary = lib.read_plain_json(FILE_SKILLS)?;
    if let Some(existing) = data.skills.iter().find(|skill| {
        skill
            .marketplace
            .as_ref()
            .is_some_and(|item| item.id == source.id)
    }) {
        return Ok(existing.clone());
    }
    if data.skills.iter().any(|skill| skill.name == name) {
        return Err(RlError::coded(codes::SKILL_NAME_CONFLICT, name));
    }
    let ts = now_ts();
    let id = new_id("sk");
    let skill = Skill {
        id: id.clone(),
        directory: id,
        name: name.clone(),
        description,
        marketplace: Some(source),
        group_ids: Vec::new(),
        sort_order: data
            .skills
            .iter()
            .map(|item| item.sort_order)
            .max()
            .map_or(0, |max| max + 1),
        created_at: ts,
        updated_at: ts,
    };
    lib.write_body(&skill.directory, &body)?;
    data.skills.push(skill.clone());
    lib.write_plain_json(FILE_SKILLS, &data)?;
    git::auto_commit(lib, &format!("从市场添加技能:{name}"))?;
    Ok(skill)
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(super) fn skill_update(
    lib: &Library,
    id: &str,
    name: Option<String>,
    description: Option<String>,
    group_ids: Option<Vec<String>>,
    directory: Option<String>,
) -> RlResult<Skill> {
    skill_update_with_body(lib, id, name, description, group_ids, directory, None)
}

pub(super) fn skill_update_with_body(
    lib: &Library,
    id: &str,
    name: Option<String>,
    description: Option<String>,
    group_ids: Option<Vec<String>>,
    directory: Option<String>,
    body: Option<String>,
) -> RlResult<Skill> {
    lib.ensure()?;
    let name_changed = name.is_some();
    let description_changed = description.is_some();
    let mut data: SkillLibrary = lib.read_plain_json(FILE_SKILLS)?;
    let Some(index) = data.skills.iter().position(|s| s.id == id) else {
        return Err(RlError::coded(codes::SKILL_NOT_FOUND, id.to_string()));
    };
    let mut skill = data.skills[index].clone();
    if let Some(name) = name {
        let name = name.trim();
        if name.is_empty() {
            return Err(RlError::coded(codes::SKILL_NAME_REQUIRED, ""));
        }
        if data.skills.iter().any(|s| s.name == name && s.id != id) {
            return Err(RlError::coded(codes::SKILL_NAME_CONFLICT, name.to_string()));
        }
        skill.name = name.to_string();
    }
    if let Some(description) = description {
        skill.description = description;
    }
    if let Some(group_ids) = group_ids {
        let group_ids = dedup(group_ids);
        validate_group_ids(&data, &group_ids)?;
        skill.group_ids = group_ids;
    }
    if let Some(directory) = directory {
        if !is_safe_directory(&directory) {
            return Err(RlError::coded(codes::DIRECTORY_INVALID, directory));
        }
        if data
            .skills
            .iter()
            .any(|s| s.directory == directory && s.id != id)
        {
            return Err(RlError::coded(codes::DIRECTORY_CONFLICT, directory));
        }
        if directory != skill.directory {
            lib.rename_skill_dir(&skill.directory, &directory)?;
            skill.directory = directory;
        }
    }
    skill.updated_at = now_ts();
    // 编辑对话框可同时保存元数据与正文,在同一快照提交内完成。
    // 提交的正文若没有 frontmatter 则补齐;已有 frontmatter 始终同步 name/description。
    if let Some(body) = body {
        let synced = fm::with_frontmatter(&body, &skill.name, &skill.description);
        lib.write_body(&skill.directory, &synced)?;
    } else if name_changed || description_changed {
        let raw = lib.read_body(&skill.directory)?;
        if raw.is_empty() {
            let content = fm::build(&skill.name, &skill.description);
            lib.write_body(&skill.directory, &content)?;
        } else {
            let raw = String::from_utf8(raw)
                .map_err(|e| RlError::corrupt(format!("skills/{}/SKILL.md", skill.directory), e))?;
            let synced = fm::with_frontmatter(&raw, &skill.name, &skill.description);
            if synced != raw {
                lib.write_body(&skill.directory, &synced)?;
            }
        }
    }
    data.skills[index] = skill.clone();
    lib.write_plain_json(FILE_SKILLS, &data)?;
    git::auto_commit(lib, &format!("更新技能:{}", skill.name))?;
    Ok(skill)
}

pub(super) fn skill_delete(lib: &Library, id: &str) -> RlResult<()> {
    lib.ensure()?;
    let mut data: SkillLibrary = lib.read_plain_json(FILE_SKILLS)?;
    let Some(index) = data.skills.iter().position(|s| s.id == id) else {
        return Err(RlError::coded(codes::SKILL_NOT_FOUND, id.to_string()));
    };
    let removed = data.skills.remove(index);
    lib.remove_skill_dir(&removed.directory)?;
    lib.write_plain_json(FILE_SKILLS, &data)?;
    git::auto_commit(lib, &format!("删除技能:{}", removed.name))?;
    Ok(())
}

/// 全量重排:ids 必须与现有技能 id 集合完全一致(排序按数组顺序)
pub(super) fn skill_reorder(lib: &Library, ids: &[String]) -> RlResult<()> {
    lib.ensure()?;
    let mut data: SkillLibrary = lib.read_plain_json(FILE_SKILLS)?;
    let existing: HashSet<&str> = data.skills.iter().map(|s| s.id.as_str()).collect();
    if ids.len() != existing.len() {
        return Err(RlError::coded(codes::SKILL_NOT_FOUND, "技能数量不一致"));
    }
    for sid in ids {
        if !existing.contains(sid.as_str()) {
            return Err(RlError::coded(codes::SKILL_NOT_FOUND, sid.to_string()));
        }
    }
    let ts = now_ts();
    for (index, sid) in ids.iter().enumerate() {
        if let Some(skill) = data.skills.iter_mut().find(|s| &s.id == sid) {
            skill.sort_order = index as u32;
            skill.updated_at = ts;
        }
    }
    lib.write_plain_json(FILE_SKILLS, &data)?;
    git::auto_commit(lib, "调整技能排序")?;
    Ok(())
}

/// 技能正文目录路径(skills/<directory>),供打开目录与测试使用
pub(super) fn skill_dir_path(lib: &Library, id: &str) -> RlResult<PathBuf> {
    let data: SkillLibrary = lib.read_plain_json(FILE_SKILLS)?;
    let skill = data
        .skills
        .iter()
        .find(|s| s.id == id)
        .ok_or_else(|| RlError::coded(codes::SKILL_NOT_FOUND, id.to_string()))?;
    Ok(lib.root().join(DIR_SKILLS).join(&skill.directory))
}

/// 在系统文件管理器中打开技能目录
pub(super) fn skill_open_dir(lib: &Library, id: &str) -> RlResult<()> {
    let path = skill_dir_path(lib, id)?;
    fs::create_dir_all(&path)?;
    open::open_explorer(&path.to_string_lossy()).map_err(Into::into)
}

pub(super) fn body_read(lib: &Library, id: &str) -> RlResult<SkillBody> {
    lib.ensure()?;
    let data: SkillLibrary = lib.read_plain_json(FILE_SKILLS)?;
    let skill = data
        .skills
        .iter()
        .find(|s| s.id == id)
        .ok_or_else(|| RlError::coded(codes::SKILL_NOT_FOUND, id.to_string()))?;
    let bytes = lib.read_body(&skill.directory)?;
    let content = String::from_utf8(bytes)
        .map_err(|e| RlError::corrupt(format!("skills/{}/SKILL.md", skill.directory), e))?;
    Ok(SkillBody {
        id: id.to_string(),
        content,
    })
}

pub(super) fn body_write(lib: &Library, id: &str, content: &str) -> RlResult<()> {
    lib.ensure()?;
    let mut data: SkillLibrary = lib.read_plain_json(FILE_SKILLS)?;
    let index = data
        .skills
        .iter()
        .position(|s| s.id == id)
        .ok_or_else(|| RlError::coded(codes::SKILL_NOT_FOUND, id.to_string()))?;
    // 以 frontmatter 为名称/描述事实源:先解析(有 name 时校验冲突),再落盘正文
    let (fm_name, fm_description) = fm::name_description_of(content);
    let mut skill = data.skills[index].clone();
    if let Some(name) = fm_name {
        let name = name.trim();
        if !name.is_empty() {
            if data.skills.iter().any(|s| s.name == name && s.id != id) {
                return Err(RlError::coded(codes::SKILL_NAME_CONFLICT, name.to_string()));
            }
            skill.name = name.to_string();
        }
    }
    if let Some(description) = fm_description {
        skill.description = description;
    }
    lib.write_body(&skill.directory, content)?;
    skill.updated_at = now_ts();
    let name = skill.name.clone();
    data.skills[index] = skill;
    lib.write_plain_json(FILE_SKILLS, &data)?;
    git::auto_commit(lib, &format!("更新技能正文:{name}"))?;
    Ok(())
}

// ── MCP CRUD ───────────────────────────────────────────────────────────

pub(super) fn mcp_create(lib: &Library, def: &McpServerInput) -> RlResult<McpServer> {
    lib.ensure()?;
    let def = validate_mcp(def)?;
    let mut list: Vec<McpServer> = lib.read_mcp_json()?;
    if list.iter().any(|m| m.name == def.name) {
        return Err(RlError::coded(codes::MCP_NAME_CONFLICT, def.name.clone()));
    }
    let ts = now_ts();
    let server = McpServer {
        id: new_id("mcp"),
        name: def.name.clone(),
        description: def.description,
        transport: def.transport,
        command: def.command,
        args: def.args,
        env: def.env,
        url: def.url,
        headers: def.headers,
        enabled: def.enabled,
        created_at: ts,
        updated_at: ts,
    };
    list.push(server.clone());
    lib.write_mcp_json(&list)?;
    git::auto_commit(lib, &format!("新增 MCP:{}", server.name))?;
    Ok(server)
}

pub(super) fn mcp_update(lib: &Library, id: &str, def: &McpServerInput) -> RlResult<McpServer> {
    lib.ensure()?;
    let def = validate_mcp(def)?;
    let mut list: Vec<McpServer> = lib.read_mcp_json()?;
    let Some(index) = list.iter().position(|m| m.id == id) else {
        return Err(RlError::coded(codes::MCP_NOT_FOUND, id.to_string()));
    };
    if list.iter().any(|m| m.name == def.name && m.id != id) {
        return Err(RlError::coded(codes::MCP_NAME_CONFLICT, def.name.clone()));
    }
    let mut server = list[index].clone();
    server.name = def.name.clone();
    server.description = def.description;
    server.transport = def.transport;
    server.command = def.command;
    server.args = def.args;
    server.env = def.env;
    server.url = def.url;
    server.headers = def.headers;
    server.enabled = def.enabled;
    server.updated_at = now_ts();
    list[index] = server.clone();
    lib.write_mcp_json(&list)?;
    git::auto_commit(lib, &format!("更新 MCP:{}", server.name))?;
    Ok(server)
}

pub(super) fn mcp_delete(lib: &Library, id: &str) -> RlResult<()> {
    lib.ensure()?;
    let mut list: Vec<McpServer> = lib.read_mcp_json()?;
    let Some(index) = list.iter().position(|m| m.id == id) else {
        return Err(RlError::coded(codes::MCP_NOT_FOUND, id.to_string()));
    };
    let removed = list.remove(index);
    lib.write_mcp_json(&list)?;
    git::auto_commit(lib, &format!("删除 MCP:{}", removed.name))?;
    Ok(())
}

// ── 加密开关(仅加密 mcp.json;口令仅内存)──────────────────────────────

/// 启用加密:meta 先翻转(明文残留可容错直读),再加密 mcp.json,
/// 随后重建 git 历史清除含明文 MCP 的旧提交,配置了 remote 时 fetch +
/// force-with-lease(显式租约 OID)强推;成功后保持解锁。
pub(super) fn encryption_enable(lib: &Library, password: &str) -> RlResult<SyncOutcome> {
    lib.ensure()?;
    if password.is_empty() {
        return Err(RlError::coded(codes::PASSWORD_REQUIRED, ""));
    }
    let mut meta = lib.meta()?;
    if meta.encrypted {
        return Err(RlError::coded(codes::ALREADY_ENCRYPTED, ""));
    }
    let salt = new_salt();
    let key = derive_key(password, &salt)?;
    let key_check = make_key_check(&key)?;
    // 1) meta 翻转(提交点;此后明文残留经容器校验直通,不丢数据)
    meta.encrypted = true;
    meta.kdf_salt = Some(salt);
    meta.key_check = Some(key_check);
    lib.write_meta(&meta)?;
    // 2) 加密 mcp.json(唯一加密文件)
    lib.transform_encrypted_file(|bytes| encrypt_bytes(&key, bytes))?;
    // 3) 历史重写 + 强推
    let outcome = git::rewrite_and_push(lib, "启用加密:重建历史(清除明文)");
    // 4) 保持解锁
    store_key(lib.root(), key);
    Ok(outcome)
}

/// 关闭加密:先解密 mcp.json(meta 仍为加密态,中途崩溃可由容器校验容错),
/// 再翻转 meta,随后重建历史并强推;结束后清除内存密钥。
pub(super) fn encryption_disable(lib: &Library, password: &str) -> RlResult<SyncOutcome> {
    lib.ensure()?;
    let mut meta = lib.meta()?;
    if !meta.encrypted {
        return Err(RlError::coded(codes::NOT_ENCRYPTED, ""));
    }
    let key = match key_for(lib.root()) {
        Some(key) => key,
        None => {
            let salt = meta
                .kdf_salt
                .clone()
                .ok_or_else(|| RlError::coded(codes::CORRUPT, "缺少 kdf salt"))?;
            let key = derive_key(password, &salt)?;
            let check = meta
                .key_check
                .clone()
                .ok_or_else(|| RlError::coded(codes::CORRUPT, "缺少 key check"))?;
            if !verify_key_check(&key, &check) {
                return Err(RlError::coded(codes::PASSWORD_INVALID, ""));
            }
            key
        }
    };
    // 1) 批量解密 mcp.json
    lib.transform_encrypted_file(|bytes| {
        super::crypto::decrypt_bytes(Some(&key), bytes).map(|plain| plain.to_vec())
    })?;
    // 2) meta 翻转
    meta.encrypted = false;
    meta.kdf_salt = None;
    meta.key_check = None;
    lib.write_meta(&meta)?;
    // 3) 历史重写 + 强推
    let outcome = git::rewrite_and_push(lib, "关闭加密:重建历史");
    // 4) 清除内存密钥
    clear_key(lib.root());
    Ok(outcome)
}

/// 解锁:派生密钥并校验口令(区分口令错误与数据损坏),成功后仅驻留内存
pub(super) fn encryption_unlock(lib: &Library, password: &str) -> RlResult<()> {
    lib.ensure()?;
    let meta = lib.meta()?;
    if !meta.encrypted {
        return Err(RlError::coded(codes::NOT_ENCRYPTED, ""));
    }
    if password.is_empty() {
        return Err(RlError::coded(codes::PASSWORD_REQUIRED, ""));
    }
    if is_unlocked(lib.root()) {
        return Ok(());
    }
    let salt = meta
        .kdf_salt
        .clone()
        .ok_or_else(|| RlError::coded(codes::CORRUPT, "缺少 kdf salt"))?;
    let key = derive_key(password, &salt)?;
    let check = meta
        .key_check
        .clone()
        .ok_or_else(|| RlError::coded(codes::CORRUPT, "缺少 key check"))?;
    if !verify_key_check(&key, &check) {
        return Err(RlError::coded(codes::PASSWORD_INVALID, ""));
    }
    store_key(lib.root(), key);
    Ok(())
}

/// 上锁:立即清除内存密钥
pub(super) fn encryption_lock(lib: &Library) {
    clear_key(lib.root());
}
