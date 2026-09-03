use super::super::*;
use super::helpers::*;
use std::fs;

#[test]
fn project_stats_aggregates_history_churn_and_file_types() {
    let dir = temp_dir("project-stats");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "init"]);

    // 分支提交,稍后 --no-ff 合并产生一个合并提交
    git(&dir, &["checkout", "-b", "feature"]);
    fs::write(dir.join("b.txt"), "b\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "feature"]);
    git(&dir, &["checkout", "main"]);
    fs::write(dir.join("a.txt"), "a1\na2\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "main work"]);
    git(
        &dir,
        &["merge", "--no-ff", "feature", "-m", "merge feature"],
    );

    let stats = collect_project_stats(dir.to_str().unwrap()).unwrap();
    assert_eq!(stats.total_commits, 4);
    assert_eq!(stats.merge_commits, 1);
    // churn: init +1/-0,feature +1/-0,main work +2/-1;合并提交不计
    assert_eq!(stats.total_additions, 4);
    assert_eq!(stats.total_deletions, 1);
    assert!(!stats.churn_truncated);
    assert_eq!(stats.authors.len(), 1);
    assert_eq!(stats.authors[0].commits, 4);
    assert_eq!(stats.authors[0].additions, 4);
    assert_eq!(stats.authors[0].deletions, 1);
    assert_eq!(stats.active_days, stats.by_day.len() as u64);
    // 星期×小时与按日两个桶的合计都应等于总提交数
    let bucketed: u32 = stats.weekday_hour.iter().sum();
    assert_eq!(u64::from(bucketed), stats.total_commits);
    let daily: u32 = stats.by_day.iter().map(|d| d.count).sum();
    assert_eq!(u64::from(daily), stats.total_commits);
    // 首末时间与按日 churn 聚合
    assert!(stats.first_commit_at.is_some());
    assert_eq!(
        stats
            .first_commit_at
            .zip(stats.last_commit_at)
            .map(|(f, l)| f <= l),
        Some(true)
    );
    let adds: u64 = stats.by_day.iter().map(|d| d.additions).sum();
    assert_eq!(adds, stats.total_additions);
    // 逐提交 churn:合并提交不进明细,合计与总数一致,按时间升序
    assert_eq!(stats.churn_commits.len(), 3);
    let adds: u64 = stats.churn_commits.iter().map(|c| c.additions).sum();
    let dels: u64 = stats.churn_commits.iter().map(|c| c.deletions).sum();
    assert_eq!((adds, dels), (stats.total_additions, stats.total_deletions));
    assert!(stats.churn_commits.windows(2).all(|w| w[0].t <= w[1].t));
    assert!(stats.churn_commits.iter().all(|c| c.short_id.len() == 7));
    let mut subjects: Vec<&str> = stats
        .churn_commits
        .iter()
        .map(|c| c.subject.as_str())
        .collect();
    subjects.sort_unstable();
    assert_eq!(subjects, ["feature", "init", "main work"]);
    // HEAD 树文件类型:a.txt / b.txt 归 "txt"
    let txt = stats
        .file_types
        .iter()
        .find(|f| f.ext == "txt")
        .expect("txt 应在文件类型分布中");
    assert_eq!(txt.files, 2);
    assert_eq!(stats.total_files, 2);
    assert!(stats.total_bytes >= 4);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn project_stats_rejects_non_repo_and_allows_empty_repo() {
    let dir = temp_dir("project-stats-empty");
    assert!(collect_project_stats(dir.to_str().unwrap()).is_err());
    init_repo(&dir);
    let stats = collect_project_stats(dir.to_str().unwrap()).unwrap();
    assert_eq!(stats.total_commits, 0);
    assert!(stats.authors.is_empty());
    assert_eq!(stats.weekday_hour.len(), 168);
    assert!(stats.file_types.is_empty());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn project_stats_merges_authors_by_email_case_insensitive() {
    let dir = temp_dir("project-stats-authors");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "init"]);
    // 同一 email 不同大小写/名字:归并为一人
    git(&dir, &["config", "user.email", "TEST@example.com"]);
    git(&dir, &["config", "user.name", "Test Renamed"]);
    fs::write(dir.join("a.txt"), "a1\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "second"]);

    let stats = collect_project_stats(dir.to_str().unwrap()).unwrap();
    assert_eq!(stats.authors.len(), 1);
    assert_eq!(stats.authors[0].commits, 2);
    // 展示名取最近一次(遍历新→旧的首次)出现的名字
    assert_eq!(stats.authors[0].name, "Test Renamed");
    let _ = fs::remove_dir_all(&dir);
}

