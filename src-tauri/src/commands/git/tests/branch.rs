use super::super::*;
use super::helpers::*;
use std::fs;

#[test]
fn branches_checkout_and_create() {
    let dir = temp_dir("branch");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a").unwrap();
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-m", "init"]);

    let branches = list_branches_blocking(dir.to_str().unwrap()).unwrap();
    assert_eq!(branches.local, vec!["main".to_string()]);
    assert!(branches.remote.is_empty());

    // 新建并切换
    let st = checkout_blocking(dir.to_str().unwrap(), "feature", true, false, None).unwrap();
    assert_eq!(st.branch.as_deref(), Some("feature"));

    let branches = list_branches_blocking(dir.to_str().unwrap()).unwrap();
    assert_eq!(
        branches.local,
        vec!["feature".to_string(), "main".to_string()]
    );

    // 切回 main
    let st = checkout_blocking(dir.to_str().unwrap(), "main", false, false, None).unwrap();
    assert_eq!(st.branch.as_deref(), Some("main"));

    // 空分支名 / 不存在的分支
    assert!(checkout_blocking(dir.to_str().unwrap(), " ", false, false, None).is_err());
    assert!(checkout_blocking(dir.to_str().unwrap(), "nope", false, false, None).is_err());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn branches_keep_log_style_names_when_remote_shares_branch_name() {
    // 分支 zc 与 remote zc 同名时(refs/remotes/zc/HEAD 存在),
    // %(refname:short) 为消歧输出 "heads/zc",而 git log %D 装饰仍显示 "zc";
    // 分支列表必须与 %D 一致,否则图谱侧栏点分支定位顶端提交失败
    let dir = temp_dir("ambiguous-remote");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a").unwrap();
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-m", "init"]);
    git(&dir, &["branch", "zc"]);
    let head = git_command(dir.to_str().unwrap())
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap()
        .stdout;
    let head = String::from_utf8_lossy(&head).trim().to_string();
    git(&dir, &["update-ref", "refs/remotes/zc/HEAD", &head]);
    git(&dir, &["update-ref", "refs/remotes/zc/zc", &head]);

    // 前提校验:git 的 short 命名在此场景下确实会消歧成 heads/zc
    let short = git_command(dir.to_str().unwrap())
        .args(["branch", "--format=%(refname:short)"])
        .output()
        .unwrap()
        .stdout;
    assert!(String::from_utf8_lossy(&short).contains("heads/zc"));

    let branches = list_branches_blocking(dir.to_str().unwrap()).unwrap();
    assert_eq!(branches.local, vec!["main".to_string(), "zc".to_string()]);
    assert!(branches.remote.contains(&"zc/zc".to_string()));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn checkout_remote_creates_tracking_branch() {
    let origin = temp_dir("track-origin");
    git(&origin, &["init", "--bare", "-b", "main"]);

    // clone_a:推 main 和 feature 两个分支到远端
    let clone_a = temp_dir("track-a");
    git(&clone_a, &["clone", origin.to_str().unwrap(), "."]);
    git(&clone_a, &["config", "user.email", "test@example.com"]);
    git(&clone_a, &["config", "user.name", "test"]);
    fs::write(clone_a.join("a.txt"), "a").unwrap();
    git(&clone_a, &["add", "a.txt"]);
    git(&clone_a, &["commit", "-m", "c1"]);
    git(&clone_a, &["push", "-u", "origin", "main"]);
    git(&clone_a, &["checkout", "-b", "feature"]);
    fs::write(clone_a.join("b.txt"), "b").unwrap();
    git(&clone_a, &["add", "b.txt"]);
    git(&clone_a, &["commit", "-m", "c2"]);
    git(&clone_a, &["push", "-u", "origin", "feature"]);

    let clone_b = temp_dir("track-b");
    git(&clone_b, &["clone", origin.to_str().unwrap(), "."]);

    // 远程分支列出 feature/main,不含 origin/HEAD 符号引用
    let branches = list_branches_blocking(clone_b.to_str().unwrap()).unwrap();
    assert_eq!(branches.local, vec!["main".to_string()]);
    assert_eq!(
        branches.remote,
        vec!["origin/feature".to_string(), "origin/main".to_string()]
    );

    // 检出远程分支:本地无同名分支 → 创建跟踪分支
    let st = checkout_blocking(
        clone_b.to_str().unwrap(),
        "origin/feature",
        false,
        true,
        None,
    )
    .unwrap();
    assert_eq!(st.branch.as_deref(), Some("feature"));

    // 本地已有同名分支 → 直接切换(幂等,不报错)
    let st = checkout_blocking(
        clone_b.to_str().unwrap(),
        "origin/feature",
        false,
        true,
        None,
    )
    .unwrap();
    assert_eq!(st.branch.as_deref(), Some("feature"));

    let _ = fs::remove_dir_all(&origin);
    let _ = fs::remove_dir_all(&clone_a);
    let _ = fs::remove_dir_all(&clone_b);
}

#[test]
fn push_sets_upstream_when_missing() {
    let origin = temp_dir("push-origin");
    git(&origin, &["init", "--bare", "-b", "main"]);

    let clone = temp_dir("push-clone");
    git(&clone, &["clone", origin.to_str().unwrap(), "."]);
    git(&clone, &["config", "user.email", "test@example.com"]);
    git(&clone, &["config", "user.name", "test"]);
    fs::write(clone.join("a.txt"), "a").unwrap();
    git(&clone, &["add", "a.txt"]);
    git(&clone, &["commit", "-m", "c1"]);

    // 首次 push 无 upstream → 自动回退 `git push -u origin HEAD`
    let st = push_blocking(clone.to_str().unwrap()).unwrap();
    assert!(st.is_repo);

    // 已建立 upstream 后走普通 push 路径
    fs::write(clone.join("a.txt"), "a2").unwrap();
    git(&clone, &["commit", "-am", "c2"]);
    push_blocking(clone.to_str().unwrap()).unwrap();

    let out = git_command(origin.to_str().unwrap())
        .args(["rev-list", "--count", "main"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "2");

    let _ = fs::remove_dir_all(&origin);
    let _ = fs::remove_dir_all(&clone);
}

#[test]
fn push_first_time_uses_non_origin_remote() {
    let origin = temp_dir("push-nonorigin");
    git(&origin, &["init", "--bare", "-b", "main"]);

    let clone = temp_dir("push-nonorigin-clone");
    git(&clone, &["clone", origin.to_str().unwrap(), "."]);
    git(&clone, &["config", "user.email", "test@example.com"]);
    git(&clone, &["config", "user.name", "test"]);
    // 远端不叫 origin(如 "github")时,首推回退也应成功
    git(&clone, &["remote", "rename", "origin", "github"]);
    fs::write(clone.join("a.txt"), "a").unwrap();
    git(&clone, &["add", "a.txt"]);
    git(&clone, &["commit", "-m", "c1"]);

    let st = push_blocking(clone.to_str().unwrap()).unwrap();
    assert!(st.is_repo);

    // upstream 应指向 github/main
    let out = git_command(clone.to_str().unwrap())
        .args([
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "github/main");

    let _ = fs::remove_dir_all(&origin);
    let _ = fs::remove_dir_all(&clone);
}

#[test]
fn split_remote_branch_parses_remote_and_branch() {
    assert_eq!(
        split_remote_branch("origin/feature/x"),
        Some(("origin".to_string(), "feature/x".to_string()))
    );
    assert_eq!(
        split_remote_branch("github/main"),
        Some(("github".to_string(), "main".to_string()))
    );
    assert_eq!(split_remote_branch("main"), None);
    assert_eq!(split_remote_branch("/main"), None);
    assert_eq!(split_remote_branch("origin/"), None);
}

#[test]
fn branch_delete_merged_branch() {
    let dir = temp_dir("brdel-merged");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a").unwrap();
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-m", "c1"]);
    // topic 基于 main,无额外提交:视为已合并可安全删除
    git(&dir, &["branch", "topic"]);

    branch_delete_blocking(dir.to_str().unwrap(), "topic", false).unwrap();
    let out = git_command(dir.to_str().unwrap())
        .args(["branch", "--list", "topic"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn branch_delete_unmerged_requires_force() {
    let dir = temp_dir("brdel-unmerged");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a").unwrap();
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-m", "c1"]);
    // topic 有未合并进 main 的提交
    git(&dir, &["checkout", "-b", "topic"]);
    fs::write(dir.join("t.txt"), "t").unwrap();
    git(&dir, &["add", "t.txt"]);
    git(&dir, &["commit", "-m", "t1"]);
    git(&dir, &["checkout", "main"]);

    let err = branch_delete_blocking(dir.to_str().unwrap(), "topic", false).unwrap_err();
    assert!(err.is_code(ErrorCode::GitBranchNotMerged));
    // 强删成功
    branch_delete_blocking(dir.to_str().unwrap(), "topic", true).unwrap();
    let out = git_command(dir.to_str().unwrap())
        .args(["branch", "--list", "topic"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn branch_delete_rejects_current_and_empty() {
    let dir = temp_dir("brdel-current");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a").unwrap();
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-m", "c1"]);

    // 空分支名
    let err = branch_delete_blocking(dir.to_str().unwrap(), "  ", false).unwrap_err();
    assert!(err.is_code(ErrorCode::GitBranchNameRequired));
    // 当前检出分支不可删除(git 拒绝),且分支仍在(--list 输出带 * 前缀)
    assert!(branch_delete_blocking(dir.to_str().unwrap(), "main", true).is_err());
    let out = git_command(dir.to_str().unwrap())
        .args(["branch", "--list", "main"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "* main");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn remote_branch_delete_removes_remote_ref() {
    let (origin, clone_a) = setup_origin_with_feature("rdel");
    let clone_b = clone_with_config("rdel-b", &origin);

    // 删除 origin/feature(短名含多级目录时同样按首个 '/' 拆分)
    remote_branch_delete_blocking(clone_b.to_str().unwrap(), "origin/feature").unwrap();
    let out = git_command(clone_b.to_str().unwrap())
        .args(["ls-remote", "--heads", "origin", "feature"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "");
    // main 不受影响
    assert_eq!(
        rev_parse(&clone_b, "origin/main"),
        rev_parse(&clone_b, "main")
    );

    let _ = fs::remove_dir_all(&origin);
    let _ = fs::remove_dir_all(&clone_a);
    let _ = fs::remove_dir_all(&clone_b);
}

#[test]
fn remote_branch_delete_rejects_name_without_remote() {
    let dir = temp_dir("rdel-invalid");
    init_repo(&dir);

    // 无 '/' 或段为空时无法判定远端,报 git_branch_name_required
    for name in ["main", "", "/main", "origin/"] {
        let err = remote_branch_delete_blocking(dir.to_str().unwrap(), name).unwrap_err();
        assert!(
            err.is_code(ErrorCode::GitBranchNameRequired),
            "输入 {name:?} 实际输出: {err}"
        );
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn list_branches_reports_upstream_tracking() {
    let origin = temp_dir("track-origin");
    git(&origin, &["init", "--bare", "-b", "main"]);

    let clone_a = temp_dir("track-a");
    git(&clone_a, &["clone", origin.to_str().unwrap(), "."]);
    git(&clone_a, &["config", "user.email", "test@example.com"]);
    git(&clone_a, &["config", "user.name", "test"]);
    fs::write(clone_a.join("a.txt"), "a").unwrap();
    git(&clone_a, &["add", "a.txt"]);
    git(&clone_a, &["commit", "-m", "c1"]);
    git(&clone_a, &["push", "-u", "origin", "main"]);

    // feature 跟踪 origin/main;local-only 无 upstream
    git(&clone_a, &["branch", "--track", "feature", "origin/main"]);
    git(&clone_a, &["branch", "local-only"]);
    // main 本地多一个未推送提交
    fs::write(clone_a.join("a.txt"), "a2").unwrap();
    git(&clone_a, &["commit", "-am", "c2"]);

    // 另一 clone 推进 origin/main,使 main 分叉、feature 落后
    let clone_b = temp_dir("track-b");
    git(&clone_b, &["clone", origin.to_str().unwrap(), "."]);
    git(&clone_b, &["config", "user.email", "test@example.com"]);
    git(&clone_b, &["config", "user.name", "test"]);
    fs::write(clone_b.join("b.txt"), "b").unwrap();
    git(&clone_b, &["add", "b.txt"]);
    git(&clone_b, &["commit", "-m", "c3"]);
    git(&clone_b, &["push"]);
    git(&clone_a, &["fetch", "origin"]);

    // aheady 基于最新 origin/main 再提交一个:只领先不落后
    git(&clone_a, &["checkout", "-b", "aheady", "origin/main"]);
    fs::write(clone_a.join("c.txt"), "c").unwrap();
    git(&clone_a, &["add", "c.txt"]);
    git(&clone_a, &["commit", "-m", "c4"]);
    git(&clone_a, &["checkout", "main"]);

    let branches = list_branches_blocking(clone_a.to_str().unwrap()).unwrap();
    let track = |name: &str| branches.tracking.iter().find(|t| t.name == name).cloned();

    let main = track("main").expect("main 应有 tracking");
    assert_eq!(main.upstream.as_deref(), Some("origin/main"));
    assert_eq!((main.ahead, main.behind), (1, 1));

    let feature = track("feature").expect("feature 应有 tracking");
    assert_eq!((feature.ahead, feature.behind), (0, 1));

    let aheady = track("aheady").expect("aheady 应有 tracking");
    assert_eq!((aheady.ahead, aheady.behind), (1, 0));

    assert!(track("local-only").is_none(), "无 upstream 的分支不收录");

    let _ = fs::remove_dir_all(&origin);
    let _ = fs::remove_dir_all(&clone_a);
    let _ = fs::remove_dir_all(&clone_b);
}

