use std::fs;
use std::path::PathBuf;

use crate::commands::git;

use super::context::{is_wiki_relevant, read_manifests, render_file_tree, FILE_TREE_MAX_CHARS};
use super::paths::folder_name;
use super::snapshot::{commit_message, commit_wiki_in, wiki_changed_files, TEST_WIKI_GIT_NAME};
use super::storage::{
    begin_wiki_in, has_wiki_in, load_config_in, load_wiki_in, remove_wiki_dir, save_config_in,
    save_meta_in, save_page_in, CONFIG_FILE, META_FILE, META_VERSION, PAGES_DIR,
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
        crate::commands::ai::WikiGenerationBackend::Builtin
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
        crate::commands::ai::WikiGenerationBackend::Builtin => panic!("应读回 agent 配置"),
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
