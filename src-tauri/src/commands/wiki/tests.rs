use std::fs;
use std::path::PathBuf;

use crate::commands::git;

use super::context::{is_wiki_relevant, read_manifests, render_file_tree, FILE_TREE_MAX_CHARS};
use super::paths::folder_name;
use super::snapshot::{commit_message, commit_wiki_in, wiki_changed_files, TEST_WIKI_GIT_NAME};
use super::storage::{
    begin_wiki_in, begin_wiki_page_staging_in, cancel_wiki_page_staging_in,
    cleanup_wiki_page_staging_in, has_wiki_in, load_config_in, load_wiki_in, promote_validated,
    promote_wiki_page_staging_in, read_wiki_page_staging_in, remove_wiki_dir, save_config_in,
    save_meta_in, save_page_in, valid_page_file, validate_staged_page, CONFIG_FILE,
    MAX_STAGED_PAGE_BYTES, META_FILE, META_VERSION, PAGES_DIR, STAGING_PREFIX,
};
use super::types::{
    WikiCommitKind, WikiGenerationConfig, WikiMeta, WikiOutlinePage, CONFIG_VERSION,
};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "repomeow-wiki-{tag}-{}-{}",
        std::process::id(),
        crate::time_util::now_ts_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn folder_name_distinguishes_same_basename() {
    let a = folder_name("D:/code/web");
    let b = folder_name("E:/other/web");
    assert!(a.starts_with("web-") && b.starts_with("web-"));
    assert_ne!(a, b, "同名不同路径的项目必须落到不同 wiki 目录");
    assert_eq!(folder_name("D:/code/web/"), folder_name("D:/code/web"));
}

#[test]
fn save_and_load_roundtrip() {
    let dir = temp_dir("roundtrip");
    save_page_in(&dir, "01-overview.md", "# 概览").unwrap();
    let meta = WikiMeta {
        status: "completed".into(),
        outline: vec![WikiOutlinePage {
            id: "overview".into(),
            file: "01-overview.md".into(),
            title: "概览".into(),
            importance: "high".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    save_meta_in(&dir, meta).unwrap();

    let (meta, pages) = load_wiki_in(&dir).unwrap();
    assert_eq!(meta.version, META_VERSION);
    assert!(!meta.generated_at.is_empty(), "generated_at 由后端覆写");
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].content, "# 概览");
    assert!(!dir.join(PAGES_DIR).join("01-overview.md.tmp").exists());
    assert!(!dir.join("meta.json.tmp").exists());
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn generation_config_roundtrip_and_default() {
    let dir = temp_dir("config-roundtrip");
    assert!(matches!(
        load_config_in(&dir).unwrap().backend,
        crate::commands::ai::WikiGenerationBackend::Builtin { .. }
    ));
    save_config_in(
        &dir,
        WikiGenerationConfig {
            version: 99,
            backend: crate::commands::ai::WikiGenerationBackend::Agent {
                agent_id: Some("codex".into()),
                custom_command: None,
                model: Some("gpt-5".into()),
                thinking: Some("high".into()),
                concurrency: Some(3),
            },
        },
    )
    .unwrap();
    let loaded = load_config_in(&dir).unwrap();
    assert_eq!(loaded.version, CONFIG_VERSION, "保存时应覆写配置版本");
    match loaded.backend {
        crate::commands::ai::WikiGenerationBackend::Agent {
            agent_id,
            model,
            thinking,
            concurrency,
            ..
        } => {
            assert_eq!(agent_id.as_deref(), Some("codex"));
            assert_eq!(model.as_deref(), Some("gpt-5"));
            assert_eq!(thinking.as_deref(), Some("high"));
            assert_eq!(concurrency, Some(3));
        }
        crate::commands::ai::WikiGenerationBackend::Builtin { .. } => panic!("应读回 agent 配置"),
    }
    assert!(!dir.join("config.json.tmp").exists());
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn load_returns_none_when_incomplete_or_missing() {
    let dir = temp_dir("incomplete");
    assert!(load_wiki_in(&dir).is_none(), "无 meta.json 返回 None");
    save_meta_in(
        &dir,
        WikiMeta {
            status: "generating".into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(load_wiki_in(&dir).is_none(), "未完结的 meta 视为无效");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn rejects_traversal_page_name() {
    let dir = temp_dir("traversal");
    assert!(save_page_in(&dir, "../evil.md", "x").is_err());
    assert!(save_page_in(&dir, "a/b.md", "x").is_err());
    assert!(save_page_in(&dir, "01-ok.md", "x").is_ok());
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn file_tree_folds_when_over_budget() {
    let mut paths = Vec::new();
    for d in 0..50 {
        for f in 0..50 {
            paths.push(format!(
                "src/module-{d:03}/sub/file-{f:03}-with-a-long-name.rs"
            ));
        }
    }
    let (tree, truncated) = render_file_tree(&paths);
    assert!(truncated);
    assert!(tree.len() <= FILE_TREE_MAX_CHARS + 64);
    assert!(tree.contains("files)"), "应以目录折叠摘要为主: {tree}");
}

#[test]
fn wiki_relevance_filters() {
    assert!(is_wiki_relevant("src/main.rs"));
    assert!(is_wiki_relevant("docs/guide.md"));
    assert!(!is_wiki_relevant("target/debug/app.exe"));
    assert!(!is_wiki_relevant("dist/bundle.js"));
    assert!(!is_wiki_relevant("pnpm-lock.yaml"));
    assert!(!is_wiki_relevant("assets/logo.png"));
    assert!(!is_wiki_relevant("src/app.min.js"));
    assert!(!is_wiki_relevant("data/app.sqlite"));
}

#[test]
fn manifests_only_include_existing() {
    let dir = temp_dir("manifest");
    fs::write(dir.join("package.json"), "{}").unwrap();
    let manifests = read_manifests(&dir);
    assert_eq!(manifests.len(), 1);
    assert_eq!(manifests[0].path, "package.json");
    assert_eq!(manifests[0].content, "{}");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn begin_wiki_keeps_git_dir() {
    let dir = temp_dir("begin-keep-git");
    fs::create_dir_all(dir.join(PAGES_DIR)).unwrap();
    fs::write(dir.join(PAGES_DIR).join("01-old.md"), "old").unwrap();
    fs::write(dir.join(META_FILE), "{}").unwrap();
    fs::write(
        dir.join(CONFIG_FILE),
        "{\"version\":1,\"backend\":{\"kind\":\"builtin\"}}",
    )
    .unwrap();
    fs::create_dir_all(dir.join(".git")).unwrap();

    begin_wiki_in(&dir).unwrap();
    assert!(dir.join(".git").is_dir(), "重新生成必须保留 .git 历史");
    assert!(!dir.join(META_FILE).exists());
    assert!(!dir.join(PAGES_DIR).join("01-old.md").exists());
    assert!(dir.join(PAGES_DIR).is_dir(), "pages/ 应重建为空目录");
    assert!(dir.join(CONFIG_FILE).is_file(), "重新生成必须保留项目配置");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn changed_files_since() {
    let dir = temp_dir("changed-files");
    let repo = git2::Repository::init(&dir).unwrap();
    let sig = git2::Signature::now("t", "t@localhost").unwrap();
    let mut oids = Vec::new();
    for i in 0..3 {
        let blob = repo.blob(format!("v{i}").as_bytes()).unwrap();
        let mut tb = repo.treebuilder(None).unwrap();
        tb.insert("f.txt", blob, 0o100644).unwrap();
        let tree = repo.find_tree(tb.write().unwrap()).unwrap();
        let parents: Vec<git2::Commit> = oids
            .last()
            .map(|oid| repo.find_commit(*oid).unwrap())
            .into_iter()
            .collect();
        let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
        oids.push(
            repo.commit(
                Some("HEAD"),
                &sig,
                &sig,
                &format!("c{i}"),
                &tree,
                &parent_refs,
            )
            .unwrap(),
        );
    }
    let path = dir.to_string_lossy().to_string();
    let result = wiki_changed_files(path.clone(), oids[0].to_string()).unwrap();
    assert_eq!(result.files, vec!["f.txt".to_string()]);
    let result = wiki_changed_files(path, oids[2].to_string()).unwrap();
    assert!(result.files.is_empty(), "from == HEAD 时不应有变更文件");
    assert_eq!(
        result.head_sha.as_deref(),
        Some(oids[2].to_string().as_str())
    );
    fs::remove_dir_all(&dir).ok();
}

fn git_available() -> bool {
    git::git_command(".")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

fn completed_meta(file: &str) -> WikiMeta {
    WikiMeta {
        status: "completed".into(),
        head_sha: Some("0123456789abcdef".into()),
        outline: vec![WikiOutlinePage {
            id: "overview".into(),
            file: file.into(),
            title: "概览".into(),
            importance: "high".into(),
            ..Default::default()
        }],
        ..Default::default()
    }
}

#[test]
fn commit_wiki_snapshots_and_skips_when_clean() {
    if !git_available() {
        return;
    }
    let dir = temp_dir("git-commit");
    save_page_in(&dir, "01-overview.md", "# v1").unwrap();
    save_meta_in(&dir, completed_meta("01-overview.md")).unwrap();
    commit_wiki_in(&dir, "生成 wiki(共 1 页)").unwrap();
    commit_wiki_in(&dir, "重复提交").unwrap();
    let repo = git2::Repository::open(&dir).unwrap();
    let count = || {
        let mut walk = repo.revwalk().unwrap();
        walk.push_head().unwrap();
        walk.count()
    };
    assert_eq!(count(), 1, "无变更不应产生新提交");
    save_page_in(&dir, "01-overview.md", "# v2").unwrap();
    commit_wiki_in(&dir, "重新生成页面:概览").unwrap();
    assert_eq!(count(), 2);
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.author().name(), Some(TEST_WIKI_GIT_NAME));
    assert_eq!(head.message().unwrap().trim_end(), "重新生成页面:概览");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn commit_messages_by_kind() {
    let meta = completed_meta("01-overview.md");
    let sha = "0123456789abcdef";
    assert_eq!(
        commit_message(WikiCommitKind::Generate, &meta, None, Some(sha)),
        "生成 wiki(共 1 页,代码 0123456)"
    );
    assert_eq!(
        commit_message(WikiCommitKind::Update, &meta, None, Some(sha)),
        "增量更新 wiki(代码 0123456)"
    );
    assert_eq!(
        commit_message(WikiCommitKind::Page, &meta, Some("概览"), Some(sha)),
        "重新生成页面:概览(代码 0123456)"
    );
    assert_eq!(
        commit_message(WikiCommitKind::Generate, &meta, None, None),
        "生成 wiki(共 1 页)"
    );
    assert_eq!(
        commit_message(WikiCommitKind::Update, &meta, None, None),
        "增量更新 wiki"
    );
    assert_eq!(
        commit_message(WikiCommitKind::Page, &meta, Some("概览"), None),
        "重新生成页面:概览"
    );
}

#[test]
fn remove_wiki_dir_tolerates_readonly_files() {
    let dir = temp_dir("remove-readonly");
    let git_objects = dir.join(".git").join("objects");
    fs::create_dir_all(&git_objects).unwrap();
    let object = git_objects.join("abc123");
    fs::write(&object, b"pack").unwrap();
    let mut perms = fs::metadata(&object).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(&object, perms).unwrap();
    remove_wiki_dir(&dir).unwrap();
    assert!(!dir.exists());
}

#[test]
fn has_wiki_requires_nonempty_dir() {
    let dir = temp_dir("has-wiki");
    fs::remove_dir_all(&dir).unwrap();
    assert!(!has_wiki_in(&dir), "目录不存在不算有 wiki");
    fs::create_dir_all(&dir).unwrap();
    assert!(!has_wiki_in(&dir), "空目录不算有 wiki");
    fs::write(dir.join(META_FILE), "{}").unwrap();
    assert!(has_wiki_in(&dir));
    fs::remove_dir_all(&dir).ok();
}

// ── 页面暂存事务(Agent 直接写入)─────────────────────────────────────────

/// 带真实文件的项目目录(供 sources 路径「项目内真实文件」回退校验用)
fn project_dir(tag: &str) -> PathBuf {
    let dir = temp_dir(tag);
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("docs")).unwrap();
    fs::write(dir.join("README.md"), "# demo\n").unwrap();
    fs::write(dir.join("src").join("main.rs"), "fn main() {}\n").unwrap();
    fs::write(dir.join("docs").join("guide.md"), "guide\n").unwrap();
    fs::write(dir.join("extra.txt"), "x\n").unwrap();
    dir
}

fn project_path_of(dir: &PathBuf) -> String {
    dir.to_string_lossy().into_owned()
}

fn staged_page() -> WikiOutlinePage {
    WikiOutlinePage {
        id: "overview".into(),
        file: "01-overview.md".into(),
        title: "概览".into(),
        importance: "high".into(),
        relevant_files: vec![
            "README.md".into(),
            "src/main.rs".into(),
            "docs/guide.md".into(),
        ],
        ..Default::default()
    }
}

fn staged_content() -> String {
    "# 概览\n\n正文介绍。\n\n<!-- sources\nREADME.md:1-3\nsrc/main.rs\ndocs/guide.md:10-20\n-->\n"
        .into()
}

fn sources_wrapped(entries: &str) -> String {
    format!("# 概览\n\n正文介绍。\n\n<!-- sources\n{entries}\n-->\n")
}

#[test]
fn staging_begin_read_promote_success() {
    let wiki = temp_dir("staging-success");
    let project = project_dir("staging-success-proj");
    let page = staged_page();

    let staging = begin_wiki_page_staging_in(&wiki, "run-1", &page).unwrap();
    let staging_path = PathBuf::from(&staging);
    assert_eq!(
        staging_path.parent(),
        Some(wiki.join(PAGES_DIR).as_path()),
        "暂存文件必须落在 pages/ 下"
    );
    assert!(staging_path.is_file(), "首次生成暂存文件应为空文件占位");
    let name = staging_path.file_name().unwrap().to_str().unwrap();
    assert!(name.starts_with(STAGING_PREFIX));
    assert!(!valid_page_file(name), "暂存名不得通过正式页名校验: {name}");
    assert_eq!(
        read_wiki_page_staging_in(&wiki, "run-1", &page.file).unwrap(),
        ""
    );

    // 模拟 agent 直接写入暂存文件
    fs::write(&staging_path, staged_content()).unwrap();
    assert_eq!(
        read_wiki_page_staging_in(&wiki, "run-1", &page.file).unwrap(),
        staged_content()
    );

    promote_wiki_page_staging_in(&wiki, &project_path_of(&project), "run-1", &page).unwrap();
    let official = wiki.join(PAGES_DIR).join("01-overview.md");
    assert_eq!(fs::read_to_string(&official).unwrap(), staged_content());
    assert!(!staging_path.exists(), "提升后暂存文件应消失");
    assert_eq!(
        cleanup_wiki_page_staging_in(&wiki).unwrap(),
        0,
        "成功流程不应残留暂存文件"
    );
    fs::remove_dir_all(&wiki).ok();
    fs::remove_dir_all(&project).ok();
}

#[test]
fn staging_promote_rejects_invalid_content() {
    let wiki = temp_dir("staging-invalid");
    let project = project_dir("staging-invalid-proj");
    let page = staged_page();
    let eleven = "README.md\n".repeat(11);
    let cases: Vec<(String, &str)> = vec![
        (String::new(), "空内容"),
        ("   \n\t\n".into(), "纯空白"),
        (
            "# 错误标题\n\n正文。\n\n<!-- sources\nREADME.md\nsrc/main.rs\ndocs/guide.md\n-->\n"
                .into(),
            "首行标题不精确",
        ),
        ("#  概览\n\n正文。\n".into(), "标题多一个空格"),
        ("# 概览\n\n正文没有来源块。\n".into(), "缺 sources 块"),
        (
            "# 概览\n\n<!-- sources\nREADME.md\nsrc/main.rs\n".into(),
            "sources 未闭合",
        ),
        (
            "# 概览\n\n<!-- sources\nREADME.md\nsrc/main.rs\ndocs/guide.md\n-->\n尾随文字\n".into(),
            "sources 块后还有内容",
        ),
        (sources_wrapped("README.md\nsrc/main.rs"), "只有 2 条来源"),
        (sources_wrapped(eleven.trim_end()), "来源超过 10 条"),
        (
            sources_wrapped("README.md\nsrc/main.rs\nmissing/not-here.rs"),
            "路径不在 relevantFiles 且项目内不存在",
        ),
        (
            sources_wrapped("README.md\nsrc/main.rs\n../outside.md"),
            "路径穿越",
        ),
        (
            sources_wrapped("README.md:0-2\nsrc/main.rs\ndocs/guide.md"),
            "行号从 0 开始",
        ),
        (
            sources_wrapped("README.md:9-2\nsrc/main.rs\ndocs/guide.md"),
            "行区间倒置",
        ),
        (
            sources_wrapped("README.md:1-2-3\nsrc/main.rs\ndocs/guide.md"),
            "行区间格式非法",
        ),
        (
            sources_wrapped(":12\nsrc/main.rs\ndocs/guide.md"),
            "条目路径为空",
        ),
    ];
    for (content, why) in cases {
        let staging = begin_wiki_page_staging_in(&wiki, "run-bad", &page).unwrap();
        fs::write(&staging, &content).unwrap();
        let error =
            promote_wiki_page_staging_in(&wiki, &project_path_of(&project), "run-bad", &page)
                .err()
                .unwrap_or_else(|| panic!("应拒绝非法内容: {why}"));
        assert_eq!(error.code(), "ai_response_parse_failed", "{why}");
        assert!(
            PathBuf::from(&staging).exists(),
            "校验失败后暂存文件应保留: {why}"
        );
        assert!(
            !wiki.join(PAGES_DIR).join("01-overview.md").exists(),
            "非法内容不得提升为正式页: {why}"
        );
        cancel_wiki_page_staging_in(&wiki, "run-bad", &page.file).unwrap();
    }
    fs::remove_dir_all(&wiki).ok();
    fs::remove_dir_all(&project).ok();
}

#[test]
fn staging_validation_allows_path_variants() {
    let project = project_dir("staging-variants");
    let page = staged_page();
    let cases = [
        // 严格命中 relevantFiles
        sources_wrapped("README.md\nsrc/main.rs\ndocs/guide.md"),
        // bare filename 按 basename 查表补全(与前端容错一致)
        sources_wrapped("main.rs\nREADME.md\ndocs/guide.md"),
        // 单 start 区间、反斜杠路径、./ 前缀
        sources_wrapped("README.md:5\nsrc\\main.rs\n./docs/guide.md"),
        // 非 relevant 但项目内真实文件(回退分支)
        sources_wrapped("extra.txt\nREADME.md\nsrc/main.rs"),
    ];
    for content in cases {
        validate_staged_page(&content, &page, &project_path_of(&project))
            .unwrap_or_else(|error| panic!("应放行合法变体: {error}"));
    }
    fs::remove_dir_all(&project).ok();
}

#[test]
fn staging_rejects_oversized_content() {
    let project = project_dir("staging-oversize");
    let page = staged_page();
    let mut content = staged_content();
    content.push_str(&"x".repeat(MAX_STAGED_PAGE_BYTES));
    let error = validate_staged_page(&content, &page, &project_path_of(&project)).err();
    assert_eq!(
        error.expect("超限应报错").code(),
        "ai_response_parse_failed"
    );
    fs::remove_dir_all(&project).ok();
}

#[test]
fn staging_begin_seeds_from_official_page() {
    let wiki = temp_dir("staging-seed");
    let project = project_dir("staging-seed-proj");
    let page = staged_page();

    // 更新场景:暂存文件从正式页复制旧内容
    save_page_in(&wiki, "01-overview.md", "# 概览(旧版)\n旧内容\n").unwrap();
    let staging = begin_wiki_page_staging_in(&wiki, "run-upd", &page).unwrap();
    assert_eq!(
        fs::read_to_string(&staging).unwrap(),
        "# 概览(旧版)\n旧内容\n",
        "更新场景应从正式页复制旧内容"
    );

    // 写入新内容提升后正式页被替换
    fs::write(&staging, staged_content()).unwrap();
    promote_wiki_page_staging_in(&wiki, &project_path_of(&project), "run-upd", &page).unwrap();
    assert_eq!(
        fs::read_to_string(wiki.join(PAGES_DIR).join("01-overview.md")).unwrap(),
        staged_content(),
        "提升必须替换旧版正式页"
    );
    fs::remove_dir_all(&wiki).ok();
    fs::remove_dir_all(&project).ok();
}

#[test]
fn staging_cleanup_removes_leftovers() {
    let wiki = temp_dir("staging-cleanup");
    let pages = wiki.join(PAGES_DIR);
    fs::create_dir_all(&pages).unwrap();
    save_page_in(&wiki, "01-overview.md", "official").unwrap();
    let staging = begin_wiki_page_staging_in(&wiki, "run-a", &staged_page()).unwrap();
    fs::write(&staging, "partial").unwrap();
    fs::write(pages.join(".staging_junk_02-x.md"), "orphan").unwrap();
    fs::write(
        pages.join(format!("{STAGING_PREFIX}bak_123_01-overview.md")),
        "bak",
    )
    .unwrap();

    assert_eq!(cleanup_wiki_page_staging_in(&wiki).unwrap(), 3);
    assert!(pages.join("01-overview.md").is_file(), "正式页不应被清理");
    assert_eq!(cleanup_wiki_page_staging_in(&wiki).unwrap(), 0, "清理幂等");

    // pages/ 缺失时直接返回 0
    let empty = temp_dir("staging-cleanup-empty");
    fs::remove_dir_all(&empty).unwrap();
    assert_eq!(cleanup_wiki_page_staging_in(&empty).unwrap(), 0);
    fs::remove_dir_all(&wiki).ok();
}

#[test]
fn staging_promote_replaces_existing_and_rolls_back() {
    let wiki = temp_dir("staging-promote");
    let project = project_dir("staging-promote-proj");
    let pages = wiki.join(PAGES_DIR);
    let page = staged_page();

    // Windows 已存在目标的覆盖提升
    save_page_in(&wiki, "01-overview.md", "# 概览(旧)\n").unwrap();
    let staging = begin_wiki_page_staging_in(&wiki, "run-p", &page).unwrap();
    fs::write(&staging, staged_content()).unwrap();
    promote_wiki_page_staging_in(&wiki, &project_path_of(&project), "run-p", &page).unwrap();
    assert_eq!(
        fs::read_to_string(pages.join("01-overview.md")).unwrap(),
        staged_content()
    );
    assert!(!PathBuf::from(&staging).exists());
    assert_eq!(
        cleanup_wiki_page_staging_in(&wiki).unwrap(),
        0,
        "不应残留备份"
    );

    // 回滚:提升中途失败(暂存缺失)时旧页必须恢复,且不残留备份
    let missing = pages.join(format!("{STAGING_PREFIX}missing_01-overview.md"));
    promote_validated(&wiki, "01-overview.md", &missing)
        .err()
        .expect("暂存缺失应报错");
    assert_eq!(
        fs::read_to_string(pages.join("01-overview.md")).unwrap(),
        staged_content(),
        "提升失败必须回滚恢复旧页"
    );
    assert_eq!(
        cleanup_wiki_page_staging_in(&wiki).unwrap(),
        0,
        "回滚后不应残留备份"
    );
    fs::remove_dir_all(&wiki).ok();
    fs::remove_dir_all(&project).ok();
}

#[test]
fn staging_cancel_and_read_missing_are_tolerant() {
    let wiki = temp_dir("staging-cancel");
    let page = staged_page();
    assert_eq!(
        read_wiki_page_staging_in(&wiki, "run-x", &page.file).unwrap(),
        "",
        "暂存缺失时预览返回空串"
    );
    cancel_wiki_page_staging_in(&wiki, "run-x", &page.file).unwrap();
    let staging = begin_wiki_page_staging_in(&wiki, "run-x", &page).unwrap();
    assert!(PathBuf::from(&staging).is_file());
    cancel_wiki_page_staging_in(&wiki, "run-x", &page.file).unwrap();
    assert!(!PathBuf::from(&staging).exists(), "取消应删除暂存文件");
    cancel_wiki_page_staging_in(&wiki, "run-x", &page.file).unwrap();
    fs::remove_dir_all(&wiki).ok();
}

#[test]
fn staging_run_id_and_page_file_are_sanitized() {
    let wiki = temp_dir("staging-sanitize");
    let page = staged_page();
    // run id 中的非法字符(含路径分隔符)必须被清洗,暂存文件仍落在 pages/ 平级
    let staging = begin_wiki_page_staging_in(&wiki, "../../evil path:run", &page).unwrap();
    assert_eq!(
        PathBuf::from(&staging).parent(),
        Some(wiki.join(PAGES_DIR).as_path()),
        "run id 不得引入子目录或穿越"
    );

    // 非法正式页名拒绝建暂存
    let mut bad = staged_page();
    bad.file = "../evil.md".into();
    assert!(begin_wiki_page_staging_in(&wiki, "run", &bad).is_err());
    bad.file = "sub/evil.md".into();
    assert!(begin_wiki_page_staging_in(&wiki, "run", &bad).is_err());
    fs::remove_dir_all(&wiki).ok();
}
