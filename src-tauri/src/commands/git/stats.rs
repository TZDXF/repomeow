use super::*;

use crate::models::GitCommitChurn;

use std::collections::BTreeMap;

use git2::{ObjectType, TreeWalkResult};

/// 增删行(churn)统计覆盖的提交数上限:revwalk 新→旧,超出后只统计计数类指标
/// (逐提交 tree diff 是整个统计里最重的部分,上限保证超大仓库也在可接受时间内返回)
const CHURN_MAX_COMMITS: u64 = 10_000;

const DAY_SECS: i64 = 86_400;

/// 项目级 git 统计(图谱页「分析」视图):全量历史(全部引用,含合并提交)一次性聚合,
/// 只回传聚合结果不传明细。非 git 仓库报 not_a_repo;空仓库返回全零统计
#[tauri::command]
pub async fn git_project_stats(path: String) -> AppResult<GitProjectStats> {
    run_blocking(move || collect_project_stats(&path)).await
}

pub(crate) fn collect_project_stats(path: &str) -> AppResult<GitProjectStats> {
    let Some(repo) = open_repo(path)? else {
        return Err(not_a_repo());
    };
    let mut acc = StatsAccumulator::default();
    // 全量引用(本地+远程+标签),含合并提交;空仓库/空修订范围返回 None
    if let Some(walk) = build_graph_revwalk(&repo, None, None)? {
        for oid in walk.flatten() {
            let Ok(commit) = repo.find_commit(oid) else {
                continue;
            };
            acc.push_commit(&repo, &commit);
        }
    }
    acc.collect_file_types(&repo);
    Ok(acc.finish())
}

/// 单提交增删行:等价 `git diff --shortstat` 的 insertions/deletions(二进制不计入行数)。
/// 根提交相对空树;合并提交无单 diff 语义,调用方在调用前排除
fn commit_churn(repo: &Repository, commit: &git2::Commit) -> (u64, u64) {
    let Ok(new_tree) = commit.tree() else {
        return (0, 0);
    };
    let old_tree = if commit.parent_count() == 1 {
        commit.parent(0).and_then(|p| p.tree()).ok()
    } else {
        None
    };
    let mut opts = DiffOptions::new();
    opts.include_typechange(true);
    let Ok(diff) = repo.diff_tree_to_tree(old_tree.as_ref(), Some(&new_tree), Some(&mut opts))
    else {
        return (0, 0);
    };
    let Ok(stats) = diff.stats() else {
        return (0, 0);
    };
    (stats.insertions() as u64, stats.deletions() as u64)
}

/// 无扩展名时按完整文件名归并的常见清单文件(小写)
const SPECIAL_FILENAMES: [&str; 8] = [
    "dockerfile",
    "makefile",
    "jenkinsfile",
    "vagrantfile",
    "gemfile",
    "rakefile",
    "podfile",
    "justfile",
];

/// 文件类型归并键:小写扩展名;无扩展名的清单文件按文件名,其余进 "(other)"
fn file_type_key(file_name: &str) -> String {
    let lower = file_name.to_lowercase();
    if SPECIAL_FILENAMES.contains(&lower.as_str()) {
        return lower;
    }
    match lower.rsplit_once('.') {
        // ".gitignore" 这类点前缀文件不算有扩展名
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => ext.to_string(),
        _ => "(other)".to_string(),
    }
}

/// 提交时刻 + 时区偏移 → 提交者本地墙钟(按 UTC 刻度表示;聚合只关心日历位置)
fn local_wall_clock(ts: i64, offset_minutes: i32) -> i64 {
    ts + i64::from(offset_minutes) * 60
}

/// 本地墙钟所属的天然日(自纪元起的天数;乘回 86400 即该日零点刻度)
fn day_bucket(wall: i64) -> i64 {
    wall.div_euclid(DAY_SECS)
}

/// 本地墙钟的星期与小时:星期 0 = 周一..6 = 周日(1970-01-01 是周四,故偏移 +3)
fn weekday_hour_bucket(wall: i64) -> (usize, usize) {
    let days = wall.div_euclid(DAY_SECS);
    let weekday = (days + 3).rem_euclid(7) as usize;
    let hour = (wall.rem_euclid(DAY_SECS) / 3600) as usize;
    (weekday, hour)
}

#[derive(Default)]
struct DayAcc {
    count: u32,
    additions: u64,
    deletions: u64,
}

#[derive(Default)]
struct AuthorAcc {
    /// 展示名:该身份最近一次(遍历新→旧,即首次)出现的名字
    name: String,
    email: String,
    commits: u64,
    additions: u64,
    deletions: u64,
    first_at: i64,
    last_at: i64,
}

struct StatsAccumulator {
    total_commits: u64,
    merge_commits: u64,
    first_commit_at: Option<i64>,
    last_commit_at: Option<i64>,
    total_additions: u64,
    total_deletions: u64,
    /// 已做 churn 统计的非合并提交数(达到上限后停止 diff)
    churned_commits: u64,
    churn_truncated: bool,
    authors: HashMap<String, AuthorAcc>,
    /// 7*24 行主序:行 = 周一..周日
    weekday_hour: [u32; 168],
    /// key = 本地日桶,BTreeMap 保证输出按日升序
    by_day: BTreeMap<i64, DayAcc>,
    /// 逐提交 churn 明细(仅非合并提交,随 churn 上限一同截断);revwalk 新→旧,finish 时按时间排序
    churn_commits: Vec<GitCommitChurn>,
    file_types: HashMap<String, (u64, u64)>,
}

impl Default for StatsAccumulator {
    fn default() -> Self {
        Self {
            total_commits: 0,
            merge_commits: 0,
            first_commit_at: None,
            last_commit_at: None,
            total_additions: 0,
            total_deletions: 0,
            churned_commits: 0,
            churn_truncated: false,
            authors: HashMap::new(),
            weekday_hour: [0; 168],
            by_day: BTreeMap::new(),
            churn_commits: Vec::new(),
            file_types: HashMap::new(),
        }
    }
}

impl StatsAccumulator {
    fn push_commit(&mut self, repo: &Repository, commit: &git2::Commit) {
        self.total_commits += 1;
        // 时间桶用 committer 时间(与 git_log 的 since/until 语义一致),
        // 按提交自带时区偏移还原提交者本地墙钟
        let time = commit.time();
        let ts = time.seconds();
        let wall = local_wall_clock(ts, time.offset_minutes());
        let (weekday, hour) = weekday_hour_bucket(wall);
        self.weekday_hour[weekday * 24 + hour] += 1;
        let day_acc = self.by_day.entry(day_bucket(wall)).or_default();
        day_acc.count += 1;
        self.first_commit_at = Some(self.first_commit_at.map_or(ts, |v| v.min(ts)));
        self.last_commit_at = Some(self.last_commit_at.map_or(ts, |v| v.max(ts)));

        // 身份用 author(写代码的人);email 归并(改名不改人),无 email 退回名字
        let author = commit.author();
        let name = author.name().unwrap_or_default().trim();
        let email = author.email().unwrap_or_default().trim();
        let key = if email.is_empty() {
            format!("name:{}", name.to_lowercase())
        } else {
            email.to_lowercase()
        };
        let author_acc = self.authors.entry(key).or_insert_with(|| AuthorAcc {
            name: if name.is_empty() { email.to_string() } else { name.to_string() },
            email: email.to_string(),
            first_at: ts,
            last_at: ts,
            ..Default::default()
        });
        author_acc.commits += 1;
        author_acc.first_at = author_acc.first_at.min(ts);
        author_acc.last_at = author_acc.last_at.max(ts);

        if commit.parent_count() >= 2 {
            self.merge_commits += 1;
            return;
        }
        if self.churned_commits >= CHURN_MAX_COMMITS {
            self.churn_truncated = true;
            return;
        }
        self.churned_commits += 1;
        let (additions, deletions) = commit_churn(repo, commit);
        day_acc.additions += additions;
        day_acc.deletions += deletions;
        author_acc.additions += additions;
        author_acc.deletions += deletions;
        self.total_additions += additions;
        self.total_deletions += deletions;
        let mut short_id = commit.id().to_string();
        short_id.truncate(7);
        self.churn_commits.push(GitCommitChurn {
            t: ts,
            short_id,
            subject: commit
                .summary()
                .unwrap_or_default()
                .chars()
                .take(80)
                .collect(),
            additions,
            deletions,
        });
    }

    /// HEAD 树的文件类型分布:仅统计 blob(symlink/submodule 跳过)
    fn collect_file_types(&mut self, repo: &Repository) {
        let Ok(tree) = repo.head().and_then(|h| h.peel_to_tree()) else {
            return;
        };
        let _ = tree.walk(git2::TreeWalkMode::PreOrder, |_root, entry| {
            if entry.kind() == Some(ObjectType::Blob) {
                let key = file_type_key(entry.name().unwrap_or_default());
                let bytes = repo
                    .find_blob(entry.id())
                    .map(|b| b.size() as u64)
                    .unwrap_or(0);
                let slot = self.file_types.entry(key).or_default();
                slot.0 += 1;
                slot.1 += bytes;
            }
            TreeWalkResult::Ok
        });
    }

    fn finish(self) -> GitProjectStats {
        let mut authors: Vec<GitAuthorStat> = self
            .authors
            .into_values()
            .map(|a| GitAuthorStat {
                name: a.name,
                email: a.email,
                commits: a.commits,
                additions: a.additions,
                deletions: a.deletions,
                first_commit_at: a.first_at,
                last_commit_at: a.last_at,
            })
            .collect();
        authors.sort_by(|a, b| {
            b.commits
                .cmp(&a.commits)
                .then_with(|| a.name.cmp(&b.name))
        });

        let mut file_types: Vec<GitFileTypeStat> = self
            .file_types
            .into_iter()
            .map(|(ext, (files, bytes))| GitFileTypeStat { ext, files, bytes })
            .collect();
        file_types.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| b.files.cmp(&a.files)));

        let by_day = self
            .by_day
            .into_iter()
            .map(|(day, d)| GitDayStat {
                // 该日零点的 UTC 刻度:UTC 日期即提交者本地日期(仅作日历标识,非真实时刻)
                t: day * DAY_SECS,
                count: d.count,
                additions: d.additions,
                deletions: d.deletions,
            })
            .collect::<Vec<_>>();

        // revwalk 新→旧,前端 K 线按时间轴渲染需要升序;稳定排序保持同刻提交的拓扑序
        let mut churn_commits = self.churn_commits;
        churn_commits.sort_by_key(|c| c.t);

        GitProjectStats {
            total_commits: self.total_commits,
            merge_commits: self.merge_commits,
            first_commit_at: self.first_commit_at,
            last_commit_at: self.last_commit_at,
            active_days: by_day.len() as u64,
            total_additions: self.total_additions,
            total_deletions: self.total_deletions,
            churn_truncated: self.churn_truncated,
            churn_commits,
            authors,
            weekday_hour: self.weekday_hour.to_vec(),
            by_day,
            total_files: file_types.iter().map(|f| f.files).sum(),
            total_bytes: file_types.iter().map(|f| f.bytes).sum(),
            file_types,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_type_key_normalizes_extension_and_special_names() {
        assert_eq!(file_type_key("Main.RS"), "rs");
        assert_eq!(file_type_key("archive.tar.GZ"), "gz");
        assert_eq!(file_type_key("Dockerfile"), "dockerfile");
        assert_eq!(file_type_key("MAKEFILE"), "makefile");
        assert_eq!(file_type_key(".gitignore"), "(other)");
        assert_eq!(file_type_key("LICENSE"), "(other)");
        assert_eq!(file_type_key("no_ext"), "(other)");
    }

    #[test]
    fn weekday_hour_bucket_aligns_monday_first() {
        // 1970-01-05 10:30 +08:00(周一)= UTC 1970-01-05 02:30(纪元后 354600 秒)
        let wall = local_wall_clock(354_600, 480);
        assert_eq!(weekday_hour_bucket(wall), (0, 10));
        // 同一时刻 UTC 视角(周一 02:00)不应串桶
        let utc_wall = local_wall_clock(354_600, 0);
        assert_eq!(weekday_hour_bucket(utc_wall), (0, 2));
        // 1970-01-04 是周日(weekday = 6),纪元日 1970-01-01 是周四(weekday = 3)
        assert_eq!(weekday_hour_bucket(0), (3, 0));
        assert_eq!(weekday_hour_bucket(3 * DAY_SECS), (6, 0));
        // 纪元前(负时间戳)不串桶:1969-12-28 是周日
        assert_eq!(weekday_hour_bucket(-4 * DAY_SECS), (6, 0));
    }

    #[test]
    fn day_bucket_floors_negative_timestamps() {
        assert_eq!(day_bucket(0), 0);
        assert_eq!(day_bucket(DAY_SECS - 1), 0);
        assert_eq!(day_bucket(DAY_SECS), 1);
        assert_eq!(day_bucket(-1), -1);
    }
}
