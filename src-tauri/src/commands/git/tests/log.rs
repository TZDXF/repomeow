use super::super::*;
use super::helpers::*;
use std::fs;

#[test]
fn git_log_parses_and_filters() {
    let dir = temp_dir("log");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a").unwrap();
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-m", "feat: first"]);
    fs::write(dir.join("a.txt"), "a2").unwrap();
    git(&dir, &["commit", "-am", "fix: second"]);
    // 另一条作者的提交,验证 --author 过滤
    fs::write(dir.join("b.txt"), "b").unwrap();
    git(&dir, &["add", "b.txt"]);
    git(
        &dir,
        &[
            "-c",
            "user.name=other",
            "-c",
            "user.email=other@example.com",
            "commit",
            "-m",
            "docs: third",
        ],
    );

    let all = run_git_log(dir.to_str().unwrap(), None, None, None, None).unwrap();
    assert_eq!(all.len(), 3);
    // 时间倒序:最新在前
    assert_eq!(all[0].subject, "docs: third");
    assert_eq!(all[1].subject, "fix: second");
    assert_eq!(all[2].subject, "feat: first");
    assert_eq!(all[1].author, "test");
    assert!(!all[0].hash.is_empty());

    // author 过滤:仅含匹配作者的提交
    let mine = run_git_log(dir.to_str().unwrap(), None, None, None, Some("test")).unwrap();
    assert_eq!(mine.len(), 2);
    assert!(mine.iter().all(|c| c.author == "test"));
    let nobody = run_git_log(
        dir.to_str().unwrap(),
        None,
        None,
        None,
        Some("no-such-author"),
    )
    .unwrap();
    assert!(nobody.is_empty());

    // max_count 截断
    let one = run_git_log(dir.to_str().unwrap(), None, None, Some(1), None).unwrap();
    assert_eq!(one.len(), 1);

    // until 远早于提交时间 → 空
    let none = run_git_log(dir.to_str().unwrap(), None, Some("2000-01-01"), None, None).unwrap();
    assert!(none.is_empty());

    // 非仓库 → 空数组而非报错
    let plain = temp_dir("log-plain");
    let res = run_git_log(plain.to_str().unwrap(), None, None, None, None).unwrap();
    assert!(res.is_empty());

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&plain);
}

#[test]
fn git_current_user_reads_config() {
    let dir = temp_dir("user");
    init_repo(&dir);
    let user = run_git_current_user(dir.to_str().unwrap()).unwrap();
    assert_eq!(user.name, "test");
    assert_eq!(user.email, "test@example.com");

    // 非仓库:不报错即可(字段取决于全局配置,内容不可断言)
    let plain = temp_dir("user-plain");
    run_git_current_user(plain.to_str().unwrap()).unwrap();

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&plain);
}

#[test]
fn graph_log_walks_topo_with_decorations() {
    // 线性历史 + 分支 + 标签:验证拓扑序、refs 装饰、HEAD 标记
    let dir = temp_dir("graph");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "c1"]);
    fs::write(dir.join("a.txt"), "a2").unwrap();
    git(&dir, &["commit", "-am", "c2"]);
    git(&dir, &["branch", "feature"]);
    git(&dir, &["tag", "v1.0"]);

    let repo = open_repo(dir.to_str().unwrap()).unwrap().unwrap();
    let walk = build_graph_revwalk(&repo, None, None).unwrap().unwrap();
    let deco = GraphDeco::collect(&repo);
    let commits: Vec<GitGraphCommit> = walk
        .flatten()
        .filter_map(|oid| repo.find_commit(oid).ok())
        .map(|c| deco.commit_entry(&c))
        .collect();

    assert_eq!(commits.len(), 2);
    // 拓扑序:子提交(c2)先于父提交(c1)
    assert_eq!(commits[0].subject, "c2");
    assert_eq!(commits[1].subject, "c1");
    assert_eq!(commits[0].parents, vec![commits[1].hash.clone()]);
    // HEAD -> main 置顶;同提交上的 feature 分支与 tag 装饰一并列出
    assert!(commits[0].is_head);
    assert_eq!(commits[0].refs[0], "main");
    assert!(commits[0].refs.contains(&"feature".to_string()));
    assert!(commits[0].refs.contains(&"tag: v1.0".to_string()));
    assert!(!commits[1].is_head);
    assert!(commits[1].parents.is_empty());

    // 指定分支范围与空仓库的 done 语义
    assert!(
        build_graph_revwalk(&repo, Some(vec!["feature".to_string()]), None)
            .unwrap()
            .is_some()
    );
    assert!(
        build_graph_revwalk(&repo, Some(vec!["no-such".to_string()]), None).is_err(),
        "无法解析的修订名应报错"
    );
    let empty = temp_dir("graph-empty");
    init_repo(&empty);
    let empty_repo = open_repo(empty.to_str().unwrap()).unwrap().unwrap();
    assert!(build_graph_revwalk(&empty_repo, None, None)
        .unwrap()
        .is_none());
    assert!(build_graph_revwalk(&empty_repo, Some(vec![]), None)
        .unwrap()
        .is_none());

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&empty);
}

#[test]
fn graph_log_excludes_remote_refs_when_disabled() {
    let (origin, _clone_a) = setup_origin_with_feature("graph-scope");
    let clone_b = clone_with_config("graph-scope-b", &origin);
    let repo = open_repo(clone_b.to_str().unwrap()).unwrap().unwrap();

    // 默认(全量):含 origin/feature 装饰
    let deco = GraphDeco::collect(&repo);
    let walk = build_graph_revwalk(&repo, None, None).unwrap().unwrap();
    let commits: Vec<GitGraphCommit> = walk
        .flatten()
        .filter_map(|oid| repo.find_commit(oid).ok())
        .map(|c| deco.commit_entry(&c))
        .collect();
    let all_refs: Vec<&str> = commits
        .iter()
        .flat_map(|c| c.refs.iter().map(String::as_str))
        .collect();
    assert!(all_refs.contains(&"origin/main"), "实际: {all_refs:?}");
    // origin/HEAD 符号引用不出现在装饰中
    assert!(!all_refs.contains(&"origin/HEAD"), "实际: {all_refs:?}");

    // include_remote=false:只走本地分支+标签,feature 提交不可达
    let walk = build_graph_revwalk(&repo, None, Some(false))
        .unwrap()
        .unwrap();
    let _deco = GraphDeco::collect(&repo);
    let subjects: Vec<String> = walk
        .flatten()
        .filter_map(|oid| repo.find_commit(oid).ok())
        .map(|c| c.summary().unwrap_or_default().to_string())
        .collect();
    assert_eq!(subjects, vec!["c1".to_string()]);

    let _ = fs::remove_dir_all(&origin);
    let _ = fs::remove_dir_all(&clone_b);
}

