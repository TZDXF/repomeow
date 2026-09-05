//! 资源库集成测试:数据 CRUD、加密(仅 mcp.json)、本地 bare remote
//! 同步/分叉/导入(备份保留)/聚合配置。
//!
//! 直接用 `Library::new(临时目录)` 驱动 ops/git/store,不经 AppHandle;
//! git 网络操作用本地 bare 仓库模拟远端。

use std::fs;
use std::path::{Path, PathBuf};

use crate::commands::git::git_command;
use crate::path_util::to_forward_slash;
use crate::time_util::now_ts_nanos;

use super::crypto::{is_container, key_for};
use super::errors::codes;
use super::git;
use super::models::McpServerInput;
use super::ops;
use super::store::{remove_dir_tolerating_readonly, Library};

/// 临时资源库:测试结束自动清理(含只读 git 对象)
struct TempLib {
    lib: Library,
    root: PathBuf,
}

impl Drop for TempLib {
    fn drop(&mut self) {
        super::crypto::clear_key(&self.root);
        let _ = remove_dir_tolerating_readonly(&self.root);
    }
}

fn temp_root(name: &str) -> (Library, PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "repomeow-rl-{name}-{}-{}",
        std::process::id(),
        now_ts_nanos()
    ));
    (Library::new(root.clone()), root)
}

/// 建库并播种默认文件(CRUD 前置条件)
fn temp_lib(name: &str) -> TempLib {
    let (lib, root) = temp_root(name);
    lib.ensure().unwrap();
    TempLib { lib, root }
}

/// 建目录但不播种(import 等场景)
fn temp_plain(name: &str) -> TempLib {
    let (lib, root) = temp_root(name);
    TempLib { lib, root }
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = git_command(&dir.to_string_lossy())
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {} 失败: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// 本地 bare 仓库(模拟远端),返回其路径
fn make_bare(name: &str) -> PathBuf {
    let (_, parent) = temp_root(&format!("{name}-parent"));
    fs::create_dir_all(&parent).unwrap();
    let bare = parent.join("remote.git");
    let out = git_command(&parent.to_string_lossy())
        .args(["init", "--bare", "remote.git"])
        .output()
        .unwrap();
    assert!(out.status.success(), "init --bare 失败");
    bare
}

fn bare_url(bare: &Path) -> String {
    to_forward_slash(bare)
}

fn clone_repo(bare: &Path, name: &str) -> PathBuf {
    let (_, parent) = temp_root(&format!("{name}-clone-parent"));
    fs::create_dir_all(&parent).unwrap();
    let url = bare_url(bare);
    let out = git_command(&parent.to_string_lossy())
        .args(["clone", &url, name])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "clone 失败: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    parent.join(name)
}

fn commit_count(dir: &Path) -> usize {
    git_out(dir, &["log", "--oneline"]).lines().count()
}

fn skill_by_name(lib: &Library, name: &str) -> super::models::Skill {
    let data = ops::skill_list(lib).unwrap();
    data.skills
        .into_iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("找不到技能 {name}"))
}

fn stdio_def(name: &str) -> McpServerInput {
    McpServerInput {
        name: name.into(),
        transport: "stdio".into(),
        command: Some("npx".into()),
        args: vec!["-y".into()],
        ..Default::default()
    }
}

// ── Skill 多分组 CRUD ──────────────────────────────────────────────────

#[test]
fn skills_are_multi_group_and_flat_listed() {
    let t = temp_lib("multi");
    let g1 = ops::group_create(&t.lib, "通用", None).unwrap();
    let g2 = ops::group_create(&t.lib, "前端", None).unwrap();
    let s = ops::skill_create(
        &t.lib,
        "审查代码",
        Some("描述".into()),
        vec![g1.id.clone(), g2.id.clone()],
        Some("# 正文".into()),
    )
    .unwrap();
    // 正文落在 skills/<directory>/SKILL.md(恒明文;带 name/description frontmatter)
    let body_path = t.root.join("skills").join(&s.directory).join("SKILL.md");
    assert!(body_path.exists());
    let raw_body = fs::read_to_string(&body_path).unwrap();
    assert!(raw_body.contains("name: \"审查代码\""), "{raw_body}");
    assert!(raw_body.contains("# 正文"));
    // 扁平结构:技能单份,group_ids 多对多
    let data = ops::skill_list(&t.lib).unwrap();
    assert_eq!(data.groups.len(), 2);
    assert_eq!(data.skills.len(), 1);
    assert_eq!(data.skills[0].group_ids, vec![g1.id, g2.id]);
    let content = ops::body_read(&t.lib, &s.id).unwrap().content;
    assert!(content.contains("# 正文"));
}

#[test]
fn group_delete_detaches_skills_and_ungrouped_is_allowed() {
    let t = temp_lib("detach");
    let g1 = ops::group_create(&t.lib, "甲", None).unwrap();
    let g2 = ops::group_create(&t.lib, "乙", None).unwrap();
    ops::skill_create(
        &t.lib,
        "无分组技能",
        None,
        vec![g1.id.clone(), g2.id.clone()],
        None,
    )
    .unwrap();
    // 删除组不要求为空,自动解除关联
    ops::group_delete(&t.lib, &g1.id).unwrap();
    let data = ops::skill_list(&t.lib).unwrap();
    assert_eq!(data.groups.len(), 1);
    assert_eq!(data.skills[0].group_ids, vec![g2.id.clone()]);
    // 无分组技能
    let free = ops::skill_create(&t.lib, "散装技能", None, vec![], None).unwrap();
    let data = ops::skill_list(&t.lib).unwrap();
    assert!(data
        .skills
        .iter()
        .find(|x| x.id == free.id)
        .unwrap()
        .group_ids
        .is_empty());
}

#[test]
fn skill_update_moves_groups_renames_directory_and_deletes() {
    let t = temp_lib("upd");
    let g1 = ops::group_create(&t.lib, "一组", None).unwrap();
    let g2 = ops::group_create(&t.lib, "二组", None).unwrap();
    let s = ops::skill_create(
        &t.lib,
        "旧名",
        None,
        vec![g1.id.clone()],
        Some("v1".to_string()),
    )
    .unwrap();
    // 改名 + 换组 + 换目录(正文随目录迁移)
    let updated = ops::skill_update(
        &t.lib,
        &s.id,
        Some("新名".into()),
        Some("新描述".into()),
        Some(vec![g2.id.clone()]),
        Some("custom-dir".into()),
    )
    .unwrap();
    assert_eq!(updated.name, "新名");
    assert_eq!(updated.directory, "custom-dir");
    assert_eq!(updated.group_ids, vec![g2.id]);
    assert!(!t.root.join("skills").join(&s.directory).exists());
    assert!(t.root.join("skills/custom-dir/SKILL.md").exists());
    // 改名/改描述同步 frontmatter,正文保留
    let content = ops::body_read(&t.lib, &s.id).unwrap().content;
    assert!(content.contains("name: \"新名\""), "{content}");
    assert!(content.contains("description: \"新描述\""), "{content}");
    assert!(content.ends_with("v1"));
    // 目录安全校验
    let err =
        ops::skill_update(&t.lib, &s.id, None, None, None, Some("../evil".into())).unwrap_err();
    assert_eq!(err.code(), codes::DIRECTORY_INVALID);
    let err = ops::skill_update(&t.lib, &s.id, None, None, None, Some("a/b".into())).unwrap_err();
    assert_eq!(err.code(), codes::DIRECTORY_INVALID);
    // 目录冲突
    let s2 = ops::skill_create(&t.lib, "另一个", None, vec![], None).unwrap();
    let err =
        ops::skill_update(&t.lib, &s2.id, None, None, None, Some("custom-dir".into())).unwrap_err();
    assert_eq!(err.code(), codes::DIRECTORY_CONFLICT);
    // 删除清理目录与元数据
    ops::skill_delete(&t.lib, &s.id).unwrap();
    assert!(!t.root.join("skills/custom-dir").exists());
    let err = ops::body_read(&t.lib, &s.id).unwrap_err();
    assert_eq!(err.code(), codes::SKILL_NOT_FOUND);
}

#[test]
fn group_color_validation_and_update() {
    let t = temp_lib("gcolor");
    let g = ops::group_create(&t.lib, "有颜色", Some("#A1b2C3".into())).unwrap();
    assert_eq!(g.color.as_deref(), Some("#a1b2c3"));
    // 非法颜色
    let err = ops::group_create(&t.lib, "野颜色", Some("red".into())).unwrap_err();
    assert_eq!(err.code(), codes::GROUP_COLOR_INVALID);
    let err = ops::group_create(&t.lib, "野颜色", Some("#12345".into())).unwrap_err();
    assert_eq!(err.code(), codes::GROUP_COLOR_INVALID);
    // 更新:None 保持,Some 覆盖,空串清除
    let updated = ops::group_rename(&t.lib, &g.id, "有颜色", None).unwrap();
    assert_eq!(updated.color.as_deref(), Some("#a1b2c3"));
    let updated = ops::group_rename(&t.lib, &g.id, "有颜色", Some("#FF0000".into())).unwrap();
    assert_eq!(updated.color.as_deref(), Some("#ff0000"));
    let updated = ops::group_rename(&t.lib, &g.id, "有颜色", Some("".into())).unwrap();
    assert_eq!(updated.color, None);
    // 序列化透传前端(无颜色时省略字段)
    let data = ops::skill_list(&t.lib).unwrap();
    let json = serde_json::to_string(&data.groups[0]).unwrap();
    assert!(!json.contains("color"));
}

#[test]
fn skill_body_frontmatter_is_source_of_truth() {
    let t = temp_lib("fm");
    // 无正文 → 生成最小 SKILL.md(frontmatter 含 name/description)
    let s = ops::skill_create(
        &t.lib,
        "空正文技能",
        Some("一句话描述".into()),
        vec![],
        None,
    )
    .unwrap();
    let content = ops::body_read(&t.lib, &s.id).unwrap().content;
    assert!(
        content.contains("name: \"空正文技能\"") && content.contains("description: \"一句话描述\""),
        "{content}"
    );
    // 有正文无 frontmatter → 前置 frontmatter,正文原样
    let s2 = ops::skill_create(
        &t.lib,
        "带正文技能",
        None,
        vec![],
        Some("# 标题\n正文内容".into()),
    )
    .unwrap();
    let content = ops::body_read(&t.lib, &s2.id).unwrap().content;
    assert!(content.contains("name: \"带正文技能\""), "{content}");
    assert!(content.ends_with("# 标题\n正文内容"));
    // 用户自带 frontmatter → 原样保留
    let own = "---\nname: 自带\ndescription: 自带描述\n---\n自带正文";
    let s3 = ops::skill_create(&t.lib, "库内名", None, vec![], Some(own.into())).unwrap();
    assert_eq!(ops::body_read(&t.lib, &s3.id).unwrap().content, own);
    // 改名 → frontmatter 同步,正文保留
    let s2b = ops::skill_update(
        &t.lib,
        &s2.id,
        Some("改名技能".into()),
        Some("新描述".into()),
        None,
        None,
    )
    .unwrap();
    let content = ops::body_read(&t.lib, &s2b.id).unwrap().content;
    assert!(
        content.contains("name: \"改名技能\"") && content.contains("description: \"新描述\""),
        "{content}"
    );
    assert!(content.ends_with("# 标题\n正文内容"));
    // 仅换组不重写正文文件
    let g = ops::group_create(&t.lib, "组", None).unwrap();
    let before = ops::body_read(&t.lib, &s2b.id).unwrap().content;
    ops::skill_update(&t.lib, &s2b.id, None, None, Some(vec![g.id]), None).unwrap();
    assert_eq!(ops::body_read(&t.lib, &s2b.id).unwrap().content, before);
    // body_write 反向同步:frontmatter 改名回填 skills.json,正文不丢
    let edited = before
        .replace("改名技能", "正文改名")
        .replace("# 标题\n正文内容", "# 标题\n正文内容(编辑)");
    ops::body_write(&t.lib, &s2b.id, &edited).unwrap();
    let data = ops::skill_list(&t.lib).unwrap();
    assert!(data.skills.iter().any(|x| x.name == "正文改名"));
    let content = ops::body_read(&t.lib, &s2b.id).unwrap().content;
    assert!(content.ends_with("正文内容(编辑)"));
    // frontmatter 名称冲突先报错,不落盘
    let conflict = edited.replace("正文改名", "空正文技能");
    assert_eq!(
        ops::body_write(&t.lib, &s2b.id, &conflict)
            .unwrap_err()
            .code(),
        codes::SKILL_NAME_CONFLICT
    );
    assert!(ops::skill_list(&t.lib)
        .unwrap()
        .skills
        .iter()
        .any(|x| x.name == "正文改名"));
}

#[test]
fn skill_reorder_assigns_sort_order() {
    let t = temp_lib("skillorder");
    let a = ops::skill_create(&t.lib, "甲", None, vec![], None).unwrap();
    let b = ops::skill_create(&t.lib, "乙", None, vec![], None).unwrap();
    let c = ops::skill_create(&t.lib, "丙", None, vec![], None).unwrap();
    assert_eq!(a.sort_order, 0);
    assert_eq!(c.sort_order, 2);
    // 重排:数量/成员不一致报错
    assert_eq!(
        ops::skill_reorder(&t.lib, &[a.id.clone()])
            .unwrap_err()
            .code(),
        codes::SKILL_NOT_FOUND
    );
    ops::skill_reorder(&t.lib, &[c.id.clone(), a.id.clone(), b.id.clone()]).unwrap();
    let data = ops::skill_list(&t.lib).unwrap();
    let mut ordered: Vec<_> = data
        .skills
        .iter()
        .map(|s| (s.sort_order, s.name.as_str()))
        .collect();
    ordered.sort_by_key(|(order, _)| *order);
    let names: Vec<_> = ordered.into_iter().map(|(_, n)| n).collect();
    assert_eq!(names, vec!["丙", "甲", "乙"]);
    // 打开目录的目标路径与正文目录一致
    let dir = ops::skill_dir_path(&t.lib, &b.id).unwrap();
    assert!(dir.ends_with(&b.directory));
}

#[test]
fn name_conflicts_and_group_reorder() {
    let t = temp_lib("conflict");
    let g1 = ops::group_create(&t.lib, "组A", None).unwrap();
    let g2 = ops::group_create(&t.lib, "组B", None).unwrap();
    let g3 = ops::group_create(&t.lib, "组C", None).unwrap();
    assert_eq!(
        ops::group_create(&t.lib, "组A", None).unwrap_err().code(),
        codes::GROUP_NAME_CONFLICT
    );
    assert_eq!(
        ops::group_create(&t.lib, "  ", None).unwrap_err().code(),
        codes::GROUP_NAME_REQUIRED
    );
    ops::skill_create(&t.lib, "技能X", None, vec![], None).unwrap();
    assert_eq!(
        ops::skill_create(&t.lib, "技能X", None, vec![], None)
            .unwrap_err()
            .code(),
        codes::SKILL_NAME_CONFLICT
    );
    assert_eq!(
        ops::skill_create(&t.lib, "  ", None, vec![], None)
            .unwrap_err()
            .code(),
        codes::SKILL_NAME_REQUIRED
    );
    // 引用不存在的分组
    assert_eq!(
        ops::skill_create(&t.lib, "技能Y", None, vec!["ghost".into()], None)
            .unwrap_err()
            .code(),
        codes::GROUP_NOT_FOUND
    );
    // 重排:必须与现有集合一致
    let wrong = vec![g1.id.clone()];
    assert_eq!(
        ops::group_reorder(&t.lib, &wrong).unwrap_err().code(),
        codes::GROUP_NOT_FOUND
    );
    ops::group_reorder(&t.lib, &[g3.id.clone(), g1.id.clone(), g2.id.clone()]).unwrap();
    let data = ops::skill_list(&t.lib).unwrap();
    let mut ordered: Vec<_> = data
        .groups
        .iter()
        .map(|g| (g.sort, g.name.as_str()))
        .collect();
    ordered.sort_by_key(|(sort, _)| *sort);
    let order: Vec<_> = ordered.into_iter().map(|(_, n)| n).collect();
    assert_eq!(order, vec!["组C", "组A", "组B"]);
}

// ── MCP CRUD(transport 判别)───────────────────────────────────────────

#[test]
fn mcp_crud_validates_transport_fields() {
    let t = temp_lib("mcp");
    let created = ops::mcp_create(&t.lib, &stdio_def("语义分析")).unwrap();
    assert_eq!(created.transport, "stdio");
    assert!(created.enabled);
    // 同名冲突 / 必填
    assert_eq!(
        ops::mcp_create(&t.lib, &stdio_def("语义分析"))
            .unwrap_err()
            .code(),
        codes::MCP_NAME_CONFLICT
    );
    let mut bad = stdio_def("缺命令");
    bad.command = None;
    assert_eq!(
        ops::mcp_create(&t.lib, &bad).unwrap_err().code(),
        codes::MCP_COMMAND_REQUIRED
    );
    let mut bad = McpServerInput {
        name: "远端".into(),
        transport: "http".into(),
        ..Default::default()
    };
    assert_eq!(
        ops::mcp_create(&t.lib, &bad).unwrap_err().code(),
        codes::MCP_URL_REQUIRED
    );
    bad.url = Some("https://example.com/mcp".into());
    let remote = ops::mcp_create(&t.lib, &bad).unwrap();
    assert_eq!(remote.url.as_deref(), Some("https://example.com/mcp"));
    // 非法 transport
    let mut bad = stdio_def("野路子");
    bad.transport = "carrier-pigeon".into();
    assert_eq!(
        ops::mcp_create(&t.lib, &bad).unwrap_err().code(),
        codes::MCP_TRANSPORT_INVALID
    );
    // 更新(整体替换定义,保留 created_at)
    let mut next = stdio_def("语义分析");
    next.enabled = false;
    let updated = ops::mcp_update(&t.lib, &created.id, &next).unwrap();
    assert!(!updated.enabled);
    assert_eq!(updated.created_at, created.created_at);
    // 删除
    ops::mcp_delete(&t.lib, &created.id).unwrap();
    assert_eq!(ops::mcp_list(&t.lib).unwrap().len(), 1);
    assert_eq!(
        ops::mcp_update(&t.lib, &created.id, &next)
            .unwrap_err()
            .code(),
        codes::MCP_NOT_FOUND
    );
}

// ── 自动 git init + 快照提交 ───────────────────────────────────────────

#[test]
fn every_mutation_auto_inits_and_commits() {
    let t = temp_lib("autocommit");
    assert!(!git::is_repo(&t.lib));
    let g = ops::group_create(&t.lib, "通用", None).unwrap();
    assert!(git::is_repo(&t.lib));
    assert_eq!(commit_count(&t.root), 1);
    // 本地 git 配置固化,不依赖用户全局配置
    assert_eq!(
        git_out(&t.root, &["config", "user.name"]).trim(),
        "RepoMeow"
    );
    assert_eq!(
        git_out(&t.root, &["config", "commit.gpgsign"]).trim(),
        "false"
    );
    let s = ops::skill_create(&t.lib, "技能", None, vec![g.id], Some("b".into())).unwrap();
    assert_eq!(commit_count(&t.root), 2);
    assert!(git_out(&t.root, &["log", "--oneline"]).contains("新增技能:技能"));
    ops::body_write(&t.lib, &s.id, "b2").unwrap();
    assert_eq!(commit_count(&t.root), 3);
    assert!(git_out(&t.root, &["log", "--oneline"]).contains("更新技能正文:技能"));
    // 手动提交:无变更时返回 None
    assert_eq!(git::commit(&t.lib, "手动").unwrap(), None);
    assert_eq!(commit_count(&t.root), 3);
    // 无变更时快照跳过,工作区干净
    assert!(!git::dirty(&t.lib).unwrap());
}

// ── 加密(仅 mcp.json;口令仅内存;历史重写)─────────────────────────────

#[test]
fn encryption_enable_unlock_lock_disable_flow() {
    let t = temp_lib("encflow");
    let s = ops::skill_create(&t.lib, "机密技能", None, vec![], Some("机密正文".into())).unwrap();
    ops::mcp_create(&t.lib, &stdio_def("内部 MCP")).unwrap();
    assert_eq!(commit_count(&t.root), 2);

    // 启用:无 remote,outcome 应 ok(未推送)
    let outcome = ops::encryption_enable(&t.lib, "口令123").unwrap();
    assert!(outcome.ok && !outcome.pushed);

    let meta = t.lib.meta().unwrap();
    assert!(meta.encrypted);
    assert!(meta.kdf_salt.is_some());
    assert!(meta.key_check.is_some());
    // **仅 mcp.json 容器化**;skills.json / 正文 / library.json 恒为明文
    assert!(is_container(&fs::read(t.root.join("mcp.json")).unwrap()));
    assert!(!is_container(
        &fs::read(t.root.join("skills.json")).unwrap()
    ));
    assert!(!is_container(
        &fs::read(t.root.join("skills").join(&s.directory).join("SKILL.md")).unwrap()
    ));
    assert!(!is_container(
        &fs::read(t.root.join("library.json")).unwrap()
    ));
    // 历史重写:旧明文提交不可达,只剩 1 个重建提交
    assert_eq!(commit_count(&t.root), 1);
    // 启用后保持解锁,可正常读写
    assert!(super::crypto::is_unlocked(&t.root));
    let content = ops::body_read(&t.lib, &s.id).unwrap().content;
    assert!(content.ends_with("机密正文"), "{content}");
    // 再次启用报 already
    assert_eq!(
        ops::encryption_enable(&t.lib, "x").unwrap_err().code(),
        codes::ALREADY_ENCRYPTED
    );

    // 上锁:技能照常管理(明文),仅 MCP 读取报 locked
    ops::encryption_lock(&t.lib);
    assert!(!super::crypto::is_unlocked(&t.root));
    assert_eq!(ops::mcp_list(&t.lib).unwrap_err().code(), codes::LOCKED);
    assert_eq!(
        ops::mcp_create(&t.lib, &stdio_def("新 MCP"))
            .unwrap_err()
            .code(),
        codes::LOCKED
    );
    assert_eq!(ops::skill_list(&t.lib).unwrap().skills.len(), 1);
    ops::skill_create(&t.lib, "上锁后新增技能", None, vec![], None).unwrap();
    assert_eq!(ops::skill_list(&t.lib).unwrap().skills.len(), 2);
    // library_info 不因上锁失败,仅 MCP 计数取 0
    let info = ops::library_info(&t.lib).unwrap();
    assert!(info.encrypted && !info.unlocked);
    assert_eq!(info.skill_count, 2);
    assert_eq!(info.mcp_count, 0);
    // 错误口令 / 正确口令
    assert_eq!(
        ops::encryption_unlock(&t.lib, "错误口令")
            .unwrap_err()
            .code(),
        codes::PASSWORD_INVALID
    );
    ops::encryption_unlock(&t.lib, "口令123").unwrap();
    assert!(super::crypto::is_unlocked(&t.root));

    // 关闭:解密回明文,meta 字段清空,密钥清除
    let outcome = ops::encryption_disable(&t.lib, "口令123").unwrap();
    assert!(outcome.ok);
    let meta = t.lib.meta().unwrap();
    assert!(!meta.encrypted && meta.kdf_salt.is_none() && meta.key_check.is_none());
    assert!(!is_container(&fs::read(t.root.join("mcp.json")).unwrap()));
    assert_eq!(ops::mcp_list(&t.lib).unwrap().len(), 1);
    assert!(!super::crypto::is_unlocked(&t.root));
    assert_eq!(
        ops::encryption_disable(&t.lib, "口令123")
            .unwrap_err()
            .code(),
        codes::NOT_ENCRYPTED
    );
    assert_eq!(
        ops::encryption_unlock(&t.lib, "口令123")
            .unwrap_err()
            .code(),
        codes::NOT_ENCRYPTED
    );
}

#[test]
fn encryption_rewrites_history_and_force_pushes_to_remote() {
    let t = temp_lib("encpush");
    let bare = make_bare("encpush-bare");
    ops::skill_create(&t.lib, "技能", None, vec![], Some("正文".into())).unwrap();
    git::remote_set(&t.lib, &bare_url(&bare)).unwrap();
    git::push_now(&t.lib).unwrap();
    assert_eq!(commit_count(&t.root), 1);

    // 启用加密:重建历史 + fetch + force-with-lease(显式租约)强推
    let outcome = ops::encryption_enable(&t.lib, "pw").unwrap();
    assert!(outcome.ok, "{outcome:?}");
    assert!(outcome.pushed, "{outcome:?}");
    assert_eq!(commit_count(&t.root), 1);

    // 远端被强推更新:clone 出来 mcp.json 为密文,其余明文,单提交历史
    let cloned = clone_repo(&bare, "clone");
    assert_eq!(commit_count(&cloned), 1);
    assert!(is_container(&fs::read(cloned.join("mcp.json")).unwrap()));
    assert!(!is_container(
        &fs::read(cloned.join("skills.json")).unwrap()
    ));
    assert!(!is_container(
        &fs::read(cloned.join("library.json")).unwrap()
    ));
    assert!(git_out(&cloned, &["log", "--oneline"]).contains("启用加密"));
    // 远端 clone 侧用同口令可解锁(盐随 meta 同步)
    let lib_b = Library::new(cloned.clone());
    ops::encryption_unlock(&lib_b, "pw").unwrap();
    assert_eq!(ops::mcp_list(&lib_b).unwrap().len(), 0);
    let _ = key_for(&cloned); // 密钥已登记(按 root 分片)
}

// ── 本地 bare remote 同步 / 分叉 / 导入 / 聚合配置 ────────────────────

#[test]
fn sync_pull_push_status_and_remote_gone() {
    let a = temp_lib("sync-a");
    let bare = make_bare("sync-bare");
    ops::skill_create(&a.lib, "技能A", None, vec![], None).unwrap();
    git::remote_set(&a.lib, &bare_url(&bare)).unwrap();
    git::push_now(&a.lib).unwrap();

    // B 首次导入远端
    let b = temp_plain("sync-b");
    git::import_remote(&b.lib, &bare_url(&bare), false).unwrap();
    let data = ops::skill_list(&b.lib).unwrap();
    assert!(data.skills.iter().any(|s| s.name == "技能A"));

    // B 新增 → sync_once 推送到远端;A 落后 1
    ops::skill_create(&b.lib, "技能B", None, vec![], None).unwrap();
    let outcome = git::sync_once_impl(&b.lib);
    assert!(outcome.ok && outcome.pushed, "{outcome:?}");
    let status = git::sync_status_impl(&a.lib).unwrap();
    assert!(status.initialized && !status.diverged && !status.remote_gone);
    assert_eq!((status.ahead, status.behind), (0, 1));

    // A sync_once → pulled;双方同步
    let outcome = git::sync_once_impl(&a.lib);
    assert!(outcome.ok && outcome.pulled, "{outcome:?}");
    assert!(ops::skill_list(&a.lib)
        .unwrap()
        .skills
        .iter()
        .any(|s| s.name == "技能B"));
    let status = git::sync_status_impl(&a.lib).unwrap();
    assert_eq!((status.ahead, status.behind), (0, 0));

    // 无 remote 时显式推送报 remote_required
    let c = temp_lib("sync-noremote");
    ops::skill_create(&c.lib, "x", None, vec![], None).unwrap();
    assert_eq!(
        git::push_now(&c.lib).unwrap_err().code(),
        codes::REMOTE_REQUIRED
    );
    // A 新增 → 显式推送;dirty 预检
    ops::skill_create(&a.lib, "技能C", None, vec![], None).unwrap();
    git::push_now(&a.lib).unwrap();
    fs::write(a.root.join("library.json"), "{\"version\":1}\n").unwrap();
    assert_eq!(git::push_now(&a.lib).unwrap_err().code(), codes::DIRTY);
    git::auto_commit(&a.lib, "恢复").unwrap();

    // 远端分支删除 → remote_gone(bare 的 HEAD 先移走,才允许删除当前分支)
    let branch = git::branch(&a.lib).unwrap().unwrap();
    git_out(&bare, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git_out(&a.root, &["push", "origin", "--delete", &branch]);
    let status = git::sync_status_impl(&a.lib).unwrap();
    assert!(status.remote_gone, "{status:?}");
}

#[test]
fn diverged_fork_resolve_remote_and_local() {
    let a = temp_lib("fork-a");
    let bare = make_bare("fork-bare");
    let s1 = ops::skill_create(&a.lib, "技能A", None, vec![], None).unwrap();
    git::remote_set(&a.lib, &bare_url(&bare)).unwrap();
    git::push_now(&a.lib).unwrap();

    let b = temp_plain("fork-b");
    git::import_remote(&b.lib, &bare_url(&bare), false).unwrap();

    // 双方各自修改同名技能 → 分叉
    ops::skill_update(&a.lib, &s1.id, Some("技能A-甲".into()), None, None, None).unwrap();
    git::push_now(&a.lib).unwrap();
    ops::skill_update(&b.lib, &s1.id, Some("技能A-乙".into()), None, None, None).unwrap();
    let outcome = git::sync_once_impl(&b.lib);
    assert!(!outcome.ok && outcome.diverged, "{outcome:?}");
    assert_eq!(outcome.error_code.as_deref(), Some(codes::DIVERGED));
    // 非分叉态 resolve 报错(a 已推平,不再分叉)
    assert_eq!(
        git::resolve_fork(&a.lib, "remote").unwrap_err().code(),
        codes::NOT_DIVERGED
    );
    // 方向非法
    assert_eq!(
        git::resolve_fork(&b.lib, "sideways").unwrap_err().code(),
        codes::DIRECTION_INVALID
    );

    // 再制造分叉后 resolve remote:B 重置为远端(A)版本
    ops::skill_update(&b.lib, &s1.id, Some("技能A-乙".into()), None, None, None).unwrap();
    git::resolve_fork(&b.lib, "remote").unwrap();
    assert_eq!(skill_by_name(&b.lib, "技能A-甲").name, "技能A-甲");

    // resolve local:B 强推(显式租约)覆盖远端
    ops::skill_update(&b.lib, &s1.id, Some("技能A-丙".into()), None, None, None).unwrap();
    ops::skill_update(&a.lib, &s1.id, Some("技能A-丁".into()), None, None, None).unwrap();
    git::push_now(&a.lib).unwrap();
    assert!(!git::sync_once_impl(&b.lib).ok);
    git::resolve_fork(&b.lib, "local").unwrap();
    // 远端 = B 的视角(技能A-丙;A 的丁被覆盖)
    let cloned = clone_repo(&bare, "fork-check");
    let lib_c = Library::new(cloned);
    let data = ops::skill_list(&lib_c).unwrap();
    assert!(data.skills.iter().any(|s| s.name == "技能A-丙"));
    assert!(!data.skills.iter().any(|s| s.name == "技能A-丁"));
}

#[test]
fn import_remote_conflict_backup_and_restore() {
    let seed = temp_lib("import-seed");
    let bare = make_bare("import-bare");
    ops::skill_create(
        &seed.lib,
        "远端的技能",
        None,
        vec![],
        Some("远端正文".into()),
    )
    .unwrap();
    git::remote_set(&seed.lib, &bare_url(&bare)).unwrap();
    git::push_now(&seed.lib).unwrap();

    // 已有本地内容 → 冲突
    let t = temp_lib("import-c");
    ops::skill_create(&t.lib, "本地技能", None, vec![], None).unwrap();
    let err = git::import_remote(&t.lib, &bare_url(&bare), false).unwrap_err();
    assert_eq!(err.code(), codes::IMPORT_CONFLICT);
    // 已初始化 git → 即使 force=false 也冲突
    let t2 = temp_plain("import-c2");
    fs::create_dir_all(t2.root.join("skills")).unwrap();
    git::init(&t2.lib).unwrap();
    let err = git::import_remote(&t2.lib, &bare_url(&bare), false).unwrap_err();
    assert_eq!(err.code(), codes::IMPORT_CONFLICT);

    // force 导入:本地先备份保留,再以远端覆盖
    let result = git::import_remote(&t.lib, &bare_url(&bare), true).unwrap();
    let backup = result.backup.expect("force 导入应产生备份");
    let backup_path = PathBuf::from(&backup);
    assert!(backup_path.exists());
    // 备份里保留本地原数据
    let backup_skills = fs::read_to_string(backup_path.join("skills.json")).unwrap();
    assert!(backup_skills.contains("本地技能"));
    // 当前库为远端内容
    let data = ops::skill_list(&t.lib).unwrap();
    assert!(data.skills.iter().any(|s| s.name == "远端的技能"));
    assert!(!data.skills.iter().any(|s| s.name == "本地技能"));
    let content = ops::body_read(&t.lib, &data.skills[0].id).unwrap().content;
    assert!(content.ends_with("远端正文"), "{content}");
    // 新内容可继续同步
    ops::skill_create(&t.lib, "新的技能", None, vec![], None).unwrap();
    assert!(git::sync_once_impl(&t.lib).ok);
    // 导入后旧密钥作废(安全)
    assert!(!super::crypto::is_unlocked(&t.root));

    // 失败导入:恢复原目录
    let t3 = temp_lib("import-restore");
    ops::skill_create(&t3.lib, "保命的技能", None, vec![], None).unwrap();
    let err = git::import_remote(&t3.lib, "https://127.0.0.1:1/never.git", true).unwrap_err();
    assert!(err.code() == "git_network_connect" || err.code() == "git_command_failed");
    // 原目录已恢复,数据完好
    assert!(ops::skill_list(&t3.lib)
        .unwrap()
        .skills
        .iter()
        .any(|s| s.name == "保命的技能"));
}

#[test]
fn remote_configure_first_import_and_branch_support() {
    // 场景 1:远端为空 → 推送本地;branch 参数可指定本地分支名
    let a = temp_lib("cfg-a");
    let bare = make_bare("cfg-bare");
    ops::skill_create(&a.lib, "本地技能", None, vec![], None).unwrap();
    let outcome = git::remote_configure_impl(&a.lib, &bare_url(&bare), Some("dev".into())).unwrap();
    assert!(outcome.ok && outcome.pushed, "{outcome:?}");
    // bare 库 HEAD 仍指向默认分支,按指定分支 clone 验证
    let (_, parent) = temp_root("cfg-clone-parent");
    fs::create_dir_all(&parent).unwrap();
    let url = bare_url(&bare);
    let out = git_command(&parent.to_string_lossy())
        .args(["clone", "-b", "dev", &url, "cfg-check"])
        .output()
        .unwrap();
    assert!(out.status.success(), "clone -b dev 失败");
    let cloned = parent.join("cfg-check");
    assert_eq!(
        git_out(&cloned, &["branch", "--show-current"]).trim(),
        "dev"
    );

    // 场景 2:本地全新(零提交)+ 远端有内容 → 首次远端优先导入
    let b = temp_plain("cfg-b");
    let outcome = git::remote_configure_impl(&b.lib, &bare_url(&bare), None).unwrap();
    assert!(outcome.ok && outcome.pulled, "{outcome:?}");
    assert!(ops::skill_list(&b.lib)
        .unwrap()
        .skills
        .iter()
        .any(|s| s.name == "本地技能"));

    // 场景 3:本地有提交 + 远端有提交且分叉 → 透出 diverged,待 resolve
    let c = temp_lib("cfg-c");
    ops::skill_create(&c.lib, "C 的技能", None, vec![], None).unwrap();
    let outcome = git::remote_configure_impl(&c.lib, &bare_url(&bare), None).unwrap();
    assert!(!outcome.ok && outcome.diverged, "{outcome:?}");
    // resolve remote 后与远端一致
    git::resolve_fork(&c.lib, "remote").unwrap();
    assert!(ops::skill_list(&c.lib)
        .unwrap()
        .skills
        .iter()
        .any(|s| s.name == "本地技能"));
}

// ── 同步结果记录(仓库外 state;网络失败不外抛)────────────────────────

#[test]
fn sync_outcome_records_error_without_failing_local_save() {
    let t = temp_lib("record");
    ops::skill_create(&t.lib, "技能", None, vec![], None).unwrap();
    // 未配置远端:sync_once 直接 ok
    let outcome = git::sync_once_impl(&t.lib);
    assert!(outcome.ok);
    // 配置一个不存在的远端:fetch 失败记录进 outcome,不抛错
    git::remote_set(&t.lib, "https://127.0.0.1:1/nonexistent.git").unwrap();
    let outcome = git::sync_once_impl(&t.lib);
    assert!(!outcome.ok);
    assert!(outcome.error_code.is_some());
    // 记录进仓库外 state 文件,且不触碰 git 跟踪内容(不弄脏 git)
    let record = super::models::SyncRecord {
        at: crate::time_util::now_ts(),
        ok: outcome.ok,
        error_code: outcome.error_code.clone(),
        error_message: outcome.error_message.clone(),
        ahead: outcome.ahead,
        behind: outcome.behind,
        diverged: outcome.diverged,
    };
    t.lib.record_sync(&record).unwrap();
    let state = t.lib.read_state();
    assert_eq!(state.last_sync.unwrap().error_code, outcome.error_code);
    assert!(t.lib.state_path().exists());
    assert!(!t.lib.state_path().starts_with(&t.root));
    assert!(
        !git::dirty(&t.lib).unwrap(),
        "记录同步状态不得弄脏 git 工作区"
    );
    // 本地数据完好(网络失败不影响已保存内容)
    assert!(ops::skill_list(&t.lib)
        .unwrap()
        .skills
        .iter()
        .any(|s| s.name == "技能"));
}

#[test]
fn library_info_reports_counts_and_git_state() {
    let t = temp_lib("info");
    let g = ops::group_create(&t.lib, "分组", None).unwrap();
    ops::skill_create(&t.lib, "技能", None, vec![g.id], None).unwrap();
    ops::mcp_create(&t.lib, &stdio_def("mcp1")).unwrap();
    let info = ops::library_info(&t.lib).unwrap();
    assert_eq!(
        (info.skill_count, info.group_count, info.mcp_count),
        (1, 1, 1)
    );
    assert!(info.git_initialized && !info.git_dirty);
    assert_eq!(info.remote_url, None);
    assert_eq!(info.root, t.root.to_string_lossy());
}
