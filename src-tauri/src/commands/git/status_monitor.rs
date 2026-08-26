use super::*;

pub(crate) fn open_repo(path: &str) -> AppResult<Option<Repository>> {
    match Repository::discover(path) {
        Ok(repo) => Ok(Some(repo)),
        Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(None),
        Err(e) => Err(git_err(e)),
    }
}

/// 非仓库场景的统一错误(与原 CLI `fatal: not a git repository` 映射一致)
pub(super) fn not_a_repo() -> AppError {
    AppError::coded(ErrorCode::NotGitRepository, "")
}

/// git2/IO 错误统一映射 GitCommandFailed(全文件 19 处 map_err 共用)
pub(super) fn git_err(e: impl std::fmt::Display) -> AppError {
    AppError::coded(ErrorCode::GitCommandFailed, e.to_string())
}

/// 提交的短 hash(等价 %h,长度随仓库规模自动消歧)
pub(super) fn short_hash(commit: &git2::Commit) -> String {
    commit
        .as_object()
        .short_id()
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .unwrap_or_else(|_| commit.id().to_string())
}

/// 按 `git --date=format:%Y-%m-%d %H:%M` 语义格式化时间(使用提交自带时区偏移)
pub(super) fn format_git_time(t: git2::Time) -> String {
    let offset = chrono::FixedOffset::east_opt(t.offset_minutes() * 60)
        .unwrap_or_else(|| chrono::FixedOffset::east_opt(0).expect("零时区恒合法"));
    chrono::DateTime::from_timestamp(t.seconds(), 0)
        .map(|dt| {
            dt.with_timezone(&offset)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

/// 解析 git_log 的 since/until 参数(调用方统一传本地时间 "YYYY-MM-DD[ HH:MM:SS]",
/// 与 git --since/--until 的本地时区解释一致),解析失败回退 None(不过滤)
pub(super) fn parse_log_datetime(s: &str) -> Option<i64> {
    use chrono::{Local, NaiveDate, NaiveDateTime, TimeZone};
    let s = s.trim();
    let naive = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .ok()
        .or_else(|| {
            NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0))
        })?;
    // single():夏令时歧义/不存在时刻放弃过滤而非误过滤
    Local
        .from_local_datetime(&naive)
        .single()
        .map(|dt| dt.timestamp())
}

/// 当前 HEAD 的分支显示名(复刻 porcelain v2 `# branch.head` 语义):
/// 普通分支 → 短名;detached → "(detached from <short>)";unborn(尚无提交)→
/// HEAD 符号引用指向的分支名
pub(super) fn head_branch_name(repo: &Repository) -> Option<String> {
    match repo.head() {
        Ok(head) => {
            if let Some(name) = head.shorthand().filter(|_| head.is_branch()) {
                return Some(name.to_string());
            }
            // detached HEAD
            head.target().map(|oid| {
                let short = repo
                    .find_commit(oid)
                    .map(|c| short_hash(&c))
                    .unwrap_or_else(|_| oid.to_string());
                format!("(detached from {short})")
            })
        }
        Err(e) if e.code() == git2::ErrorCode::UnbornBranch => repo
            .find_reference("HEAD")
            .ok()
            .and_then(|r| r.symbolic_target().map(String::from))
            .map(|t| t.strip_prefix("refs/heads/").unwrap_or(&t).to_string()),
        Err(_) => None,
    }
}

/// 本地分支的 upstream 跟踪差值(ahead, behind);未配置/上游已删除/解析失败返回 None
pub(super) fn upstream_ahead_behind(repo: &Repository, branch_ref: &str) -> Option<(usize, usize)> {
    let upstream_name = repo.branch_upstream_name(branch_ref).ok()?;
    let upstream_ref = repo
        .find_reference(String::from_utf8_lossy(&upstream_name).as_ref())
        .ok()?;
    let local_oid = repo.find_reference(branch_ref).ok()?.target()?;
    let upstream_oid = upstream_ref.target()?;
    repo.graph_ahead_behind(local_oid, upstream_oid).ok()
}

pub fn status(path: &str) -> AppResult<GitStatus> {
    let Some(repo) = open_repo(path)? else {
        // 不是 git 仓库
        return Ok(GitStatus::default());
    };
    let mut st = GitStatus {
        is_repo: true,
        branch: head_branch_name(&repo),
        ..Default::default()
    };
    // HEAD 最新提交时间;无提交(空仓库)时为 None
    if let Ok(commit) = repo.head().and_then(|h| h.peel_to_commit()) {
        st.last_commit_at = Some(commit.time().seconds());
    }
    // ahead/behind:相对当前分支的 upstream(未配置/已删除保持 0)
    if let Some(branch) = st.branch.as_deref().filter(|b| !b.starts_with('(')) {
        if let Some((ahead, behind)) = upstream_ahead_behind(&repo, &format!("refs/heads/{branch}"))
        {
            st.ahead = ahead as i32;
            st.behind = behind as i32;
        }
    }
    st.remote_ahead = st.behind;

    // 工作区计数(与 status --porcelain=v2 语义对齐):
    // index 侧改动计 staged,worktree 侧改动计 modified;冲突同时计 modified + conflicted;
    // 未跟踪条目不递归目录(与 porcelain 默认折叠一致),嵌套 git 仓库(含 .git 的目录)
    // 是独立项目不计入未跟踪
    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(false)
        .include_ignored(false);
    let workdir = repo.workdir().map(|p| p.to_string_lossy().to_string());
    let statuses = repo.statuses(Some(&mut opts)).map_err(git_err)?;
    for entry in statuses.iter() {
        let s = entry.status();
        if s.contains(Git2Status::CONFLICTED) {
            // 冲突文件仍计入未暂存,保持「干净」判断语义
            st.modified += 1;
            st.conflicted += 1;
            continue;
        }
        if s.intersects(
            Git2Status::INDEX_NEW
                | Git2Status::INDEX_MODIFIED
                | Git2Status::INDEX_DELETED
                | Git2Status::INDEX_RENAMED
                | Git2Status::INDEX_TYPECHANGE,
        ) {
            st.staged += 1;
        }
        if s.contains(Git2Status::WT_NEW) {
            let entry_path = String::from_utf8_lossy(entry.path_bytes()).to_string();
            let nested = workdir
                .as_deref()
                .is_some_and(|wd| is_nested_repo(wd, &entry_path));
            if !nested {
                st.untracked += 1;
            }
        } else if s.intersects(
            Git2Status::WT_MODIFIED
                | Git2Status::WT_DELETED
                | Git2Status::WT_RENAMED
                | Git2Status::WT_TYPECHANGE,
        ) {
            st.modified += 1;
        }
    }
    Ok(st)
}

// ── 本地状态缓存 ─────────────────────────────────────────────────────────

/// 本地 git 状态缓存 TTL:详情页/批量刷新等高频调用直接命中,
/// 后台刷新循环每 30s 全量重查一次,15s 内命中即视为足够新
pub(super) const STATUS_TTL: Duration = Duration::from_secs(15);

pub(super) struct CachedStatus {
    status: GitStatus,
    at: Instant,
}

/// 路径 → 最近一次本地状态(写操作后主动回填,保持一致性)
pub(super) static STATUS_CACHE: OnceLock<Mutex<HashMap<String, CachedStatus>>> = OnceLock::new();

pub(super) fn status_cache() -> &'static Mutex<HashMap<String, CachedStatus>> {
    STATUS_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 缓存化状态查询:命中且未过期直接返回,否则执行 status() 并回填。
/// force 为 true 时绕过缓存强制重查(用户主动刷新/git 写操作后)。
/// key 归一化:同一仓库的不同路径写法共享缓存,invalidate 才能稳定命中
pub(super) fn status_cached(path: &str, force: bool) -> AppResult<GitStatus> {
    let key = crate::path_util::clean_str(path);
    if !force {
        if let Some(entry) = status_cache().lock().unwrap().get(&key) {
            if entry.at.elapsed() < STATUS_TTL {
                return Ok(entry.status.clone());
            }
        }
    }
    let st = status(path)?;
    status_cache().lock().unwrap().insert(
        key,
        CachedStatus {
            status: st.clone(),
            at: Instant::now(),
        },
    );
    Ok(st)
}

/// 写操作完成后回填缓存(返回的新状态即最新,不等待 TTL 过期)
pub(super) fn cache_status(path: &str, st: &GitStatus) {
    status_cache().lock().unwrap().insert(
        crate::path_util::clean_str(path),
        CachedStatus {
            status: st.clone(),
            at: Instant::now(),
        },
    );
}

/// 路径变更(重定向/移动/删除)后清除旧路径缓存
pub fn invalidate_status(path: &str) {
    status_cache()
        .lock()
        .unwrap()
        .remove(&crate::path_util::clean_str(path));
}

/// fetch 远端(无 remote 时跳过),返回最新状态。
/// http 协议带连接/低速超时配置:弱网/断网时 git fetch 默认无限期挂起
/// (TCP 重传可达数分钟),这里由 git 自身在慢速连接时主动中止。
/// 后台 fetch 走 fetch_with_timeout(带进程 kill 兜底),此同步版保留给测试
#[cfg_attr(not(test), allow(dead_code))]
pub fn fetch_and_status(path: &str) -> AppResult<GitStatus> {
    if repo_has_remote(path) {
        // 失败(如离线)不阻断,退回本地已知状态
        let _ = git_command(path)
            .args([
                "-c",
                "http.connectTimeout=10",
                "-c",
                "http.lowSpeedLimit=1000",
                "-c",
                "http.lowSpeedTime=30",
                "fetch",
                "--quiet",
            ])
            .output();
    }
    status(path)
}

/// 仓库是否有任一 remote(读取失败按无 remote 处理,跳过 fetch)
pub(super) fn repo_has_remote(path: &str) -> bool {
    open_repo(path)
        .ok()
        .flatten()
        .and_then(|r| r.remotes().ok())
        .is_some_and(|names| !names.is_empty())
}

#[derive(Debug, Clone, Serialize)]
pub struct GitRemote {
    pub name: String,
    pub url: String,
}

// ── 统一状态检查与后台刷新 ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct GitStatusItem {
    pub path: String,
    pub status: GitStatus,
}

pub(super) const GIT_UPDATE_SCHEDULE_ID: &str = "git_update";
pub(super) const DEFAULT_GIT_CHECK_INTERVAL_MINUTES: u64 = 10;
pub(super) const MIN_GIT_CHECK_INTERVAL_MINUTES: u64 = 1;
pub(super) const MAX_GIT_CHECK_INTERVAL_MINUTES: u64 = 24 * 60;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemSchedule {
    pub id: String,
    pub enabled: bool,
    pub interval_minutes: u64,
    pub last_run_at: Option<i64>,
}

impl Default for SystemSchedule {
    fn default() -> Self {
        Self {
            id: GIT_UPDATE_SCHEDULE_ID.to_string(),
            enabled: true,
            interval_minutes: DEFAULT_GIT_CHECK_INTERVAL_MINUTES,
            last_run_at: None,
        }
    }
}

/// Git 监控配置变更通知：设置页保存后立即唤醒休眠中的 monitor_loop。
pub struct GitMonitorNotify(pub Arc<Notify>);

pub(super) fn read_git_system_schedule(conn: &rusqlite::Connection) -> AppResult<SystemSchedule> {
    conn.query_row(
        "SELECT id, enabled, interval_minutes, last_run_at FROM system_schedules WHERE id = ?1",
        [GIT_UPDATE_SCHEDULE_ID],
        |row| {
            Ok(SystemSchedule {
                id: row.get(0)?,
                enabled: row.get::<_, i64>(1)? != 0,
                interval_minutes: row.get::<_, u64>(2)?,
                last_run_at: row.get(3)?,
            })
        },
    )
    .map_err(Into::into)
}

#[tauri::command]
pub fn list_system_schedules(db: State<'_, Db>) -> AppResult<Vec<SystemSchedule>> {
    let conn = db.0.lock().unwrap();
    Ok(vec![read_git_system_schedule(&conn)?])
}

#[tauri::command]
pub fn save_system_schedule(
    app: AppHandle,
    db: State<'_, Db>,
    id: String,
    enabled: bool,
    interval_minutes: u64,
) -> AppResult<SystemSchedule> {
    if id != GIT_UPDATE_SCHEDULE_ID {
        return Err(AppError::coded(
            ErrorCode::DbError,
            format!("unknown system schedule: {id}"),
        ));
    }
    let interval = interval_minutes.clamp(
        MIN_GIT_CHECK_INTERVAL_MINUTES,
        MAX_GIT_CHECK_INTERVAL_MINUTES,
    );
    let schedule = {
        let conn = db.0.lock().unwrap();
        conn.execute(
            "INSERT INTO system_schedules (id, enabled, interval_minutes, last_run_at)
             VALUES (?1, ?2, ?3, NULL)
             ON CONFLICT(id) DO UPDATE SET enabled = excluded.enabled,
                 interval_minutes = excluded.interval_minutes",
            rusqlite::params![GIT_UPDATE_SCHEDULE_ID, enabled as i64, interval],
        )?;
        read_git_system_schedule(&conn)?
    };
    if let Some(notify) = app.try_state::<GitMonitorNotify>() {
        notify.0.notify_one();
    }
    Ok(schedule)
}

#[derive(Debug, Clone)]
pub(super) struct GitCheckTarget {
    project_id: Option<i64>,
    name: Option<String>,
    path: String,
    auto_pull: bool,
    wiki_auto_update: bool,
}

/// 路径 → 上次已发布的 HEAD。用于发现应用外部 Git 操作造成的 HEAD 变化。
pub(super) static HEAD_CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();

pub(super) fn head_cache() -> &'static Mutex<HashMap<String, Option<String>>> {
    HEAD_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn head_sha(path: &str) -> Option<String> {
    let repo = open_repo(path).ok().flatten()?;
    let head = repo.head().ok()?;
    let sha = head.peel_to_commit().ok()?.id().to_string();
    Some(sha)
}

pub(super) fn observe_head(path: &str, current: Option<String>, force_changed: bool) -> bool {
    let key = crate::path_util::clean_str(path);
    let previous = head_cache().lock().unwrap().insert(key, current.clone());
    match previous {
        Some(old) => old != current,
        None => force_changed,
    }
}

pub(super) fn load_check_targets(
    app: &AppHandle,
    scope: GitCheckScope,
) -> AppResult<Vec<GitCheckTarget>> {
    if let GitCheckScope::Path { path } = scope {
        return Ok(vec![GitCheckTarget {
            project_id: None,
            name: None,
            path: crate::path_util::clean_str(&path),
            auto_pull: false,
            wiki_auto_update: false,
        }]);
    }

    let db = app.state::<Db>();
    let conn = db.0.lock().unwrap();
    let (sql, project_id) = match scope {
        GitCheckScope::All => (
            "SELECT id, name, path, auto_pull, wiki_auto_update FROM projects WHERE archived_at IS NULL",
            None,
        ),
        GitCheckScope::Project { project_id } => (
            "SELECT id, name, path, auto_pull, wiki_auto_update FROM projects WHERE archived_at IS NULL AND id = ?1",
            Some(project_id),
        ),
        GitCheckScope::Path { .. } => unreachable!(),
    };
    let mut stmt = conn.prepare(sql)?;
    let map = |row: &rusqlite::Row<'_>| {
        Ok(GitCheckTarget {
            project_id: Some(row.get(0)?),
            name: Some(row.get(1)?),
            path: row.get(2)?,
            auto_pull: row.get(3)?,
            wiki_auto_update: row.get(4)?,
        })
    };
    let targets = if let Some(id) = project_id {
        stmt.query_map([id], map)?.collect::<Result<Vec<_>, _>>()?
    } else {
        stmt.query_map([], map)?.collect::<Result<Vec<_>, _>>()?
    };
    if let Some(id) = project_id {
        if targets.is_empty() {
            return Err(AppError::coded(ErrorCode::ProjectNotFound, id.to_string()));
        }
    }
    Ok(targets)
}

pub(super) fn emit_project_changed(
    app: &AppHandle,
    target: &GitCheckTarget,
    status: GitStatus,
    source: &str,
    force_head_changed: bool,
    auto_pulled: bool,
    pulled_commits: i32,
) {
    let head_sha = head_sha(&target.path);
    let head_changed = observe_head(&target.path, head_sha.clone(), force_head_changed);
    let _ = app.emit(
        "git://project-changed",
        GitProjectChangedPayload {
            project_id: target.project_id,
            name: target.name.clone(),
            path: target.path.clone(),
            status,
            head_sha,
            head_changed,
            auto_pulled,
            pulled_commits,
            source: source.to_string(),
            wiki_auto_update: target.wiki_auto_update,
        },
    );
}

pub(super) fn target_for_path(app: &AppHandle, path: &str) -> GitCheckTarget {
    let clean = crate::path_util::clean_str(path);
    let db = app.state::<Db>();
    let conn = db.0.lock().unwrap();
    conn.query_row(
        "SELECT id, name, path, auto_pull, wiki_auto_update FROM projects WHERE archived_at IS NULL AND path = ?1",
        [&clean],
        |row| {
            Ok(GitCheckTarget {
                project_id: Some(row.get(0)?),
                name: Some(row.get(1)?),
                path: row.get(2)?,
                auto_pull: row.get(3)?,
                wiki_auto_update: row.get(4)?,
            })
        },
    )
    .unwrap_or(GitCheckTarget {
        project_id: None,
        name: None,
        path: clean,
        auto_pull: false,
        wiki_auto_update: false,
    })
}

/// Git 写操作成功后的唯一发布入口。
pub(super) fn publish_write_status(
    app: &AppHandle,
    path: &str,
    status: &GitStatus,
    source: &str,
    head_may_change: bool,
) {
    let target = target_for_path(app, path);
    emit_project_changed(
        app,
        &target,
        status.clone(),
        source,
        head_may_change,
        false,
        0,
    );
}

/// 单批状态查询并发上限。
pub(super) const STATUS_CONCURRENCY: usize = 8;

pub(super) async fn status_async(path: &str, force: bool) -> Option<GitStatus> {
    let path = path.to_string();
    tokio::task::spawn_blocking(move || status_cached(&path, force))
        .await
        .ok()?
        .ok()
}

pub(super) async fn check_target(
    app: AppHandle,
    target: GitCheckTarget,
    force: bool,
    fetch_remote: bool,
    source: String,
) -> Option<GitStatusItem> {
    let path = target.path.clone();
    let mut status = status_async(&path, force).await?;

    let mut auto_pulled = false;
    let mut pulled_commits = 0;
    if fetch_remote && status.is_repo && try_begin_fetch(&path) {
        let fetch_semaphore = FETCH_PERMITS.get_or_init(|| Semaphore::new(3));
        let _fetch_permit = fetch_semaphore.acquire().await;
        let ok = fetch_with_timeout(&path).await;
        if ok {
            if let Some(mut refreshed) = status_async(&path, true).await {
                refreshed.last_fetch_at = Some(crate::time_util::now_ts());
                status = refreshed;
                if target.auto_pull && status.behind > 0 {
                    pulled_commits = status.behind;
                    let pulled = tokio::task::spawn_blocking({
                        let path = path.clone();
                        move || ff_pull_blocking(&path)
                    })
                    .await
                    .unwrap_or(false);
                    if pulled {
                        if let Some(mut refreshed) = status_async(&path, true).await {
                            refreshed.last_fetch_at = Some(crate::time_util::now_ts());
                            status = refreshed;
                            auto_pulled = true;
                        }
                    }
                }
                cache_status(&path, &status);
            }
        }
        fetch_finished(&path, ok);
    }

    emit_project_changed(
        &app,
        &target,
        status.clone(),
        &source,
        auto_pulled,
        auto_pulled,
        pulled_commits,
    );
    Some(GitStatusItem { path, status })
}

pub(super) async fn check_targets(
    app: &AppHandle,
    targets: Vec<GitCheckTarget>,
    force: bool,
    fetch_remote: bool,
    source: &str,
) -> Vec<GitStatusItem> {
    let semaphore = Arc::new(Semaphore::new(STATUS_CONCURRENCY));
    let mut set = tokio::task::JoinSet::new();
    for target in targets {
        let app = app.clone();
        let semaphore = semaphore.clone();
        let source = source.to_string();
        set.spawn(async move {
            let _permit = semaphore.acquire().await;
            check_target(app, target, force, fetch_remote, source).await
        });
    }
    let mut items = Vec::new();
    while let Some(joined) = set.join_next().await {
        if let Ok(Some(item)) = joined {
            items.push(item);
        }
    }
    items.sort_by(|a, b| a.path.cmp(&b.path));
    items
}

/// 前端与后台共用的唯一 Git 检查入口。
#[tauri::command]
pub async fn check_git_status(
    app: AppHandle,
    scope: GitCheckScope,
    force: bool,
    fetch_remote: bool,
) -> AppResult<Vec<GitStatusItem>> {
    let targets = load_check_targets(&app, scope)?;
    Ok(check_targets(&app, targets, force, fetch_remote, "manual").await)
}

/// 启动后首轮刷新的延迟:先让首屏渲染完成,避免启动瞬间与列表请求抢资源
pub(super) const GIT_CHECK_FIRST_DELAY: Duration = Duration::from_secs(3);

pub(super) fn load_git_monitor_schedule(app: &AppHandle) -> SystemSchedule {
    let db = app.state::<Db>();
    let conn = db.0.lock().unwrap();
    read_git_system_schedule(&conn).unwrap_or_default()
}

pub(super) fn mark_git_monitor_run(app: &AppHandle) {
    let db = app.state::<Db>();
    let conn = db.0.lock().unwrap();
    if let Err(error) = conn.execute(
        "UPDATE system_schedules SET last_run_at = ?1 WHERE id = ?2",
        rusqlite::params![crate::time_util::now_ts(), GIT_UPDATE_SCHEDULE_ID],
    ) {
        eprintln!("[git] 更新定时检查执行时间失败: {error}");
    }
}

pub async fn monitor_loop(app: AppHandle) {
    tokio::time::sleep(GIT_CHECK_FIRST_DELAY).await;
    loop {
        let schedule = load_git_monitor_schedule(&app);
        if schedule.enabled {
            if let Ok(targets) = load_check_targets(&app, GitCheckScope::All) {
                check_targets(&app, targets, false, true, "periodic").await;
                mark_git_monitor_run(&app);
            }
        }
        let interval = schedule.interval_minutes.clamp(
            MIN_GIT_CHECK_INTERVAL_MINUTES,
            MAX_GIT_CHECK_INTERVAL_MINUTES,
        );
        let sleep = tokio::time::sleep(Duration::from_secs(interval * 60));
        tokio::pin!(sleep);
        if let Some(notify) = app.try_state::<GitMonitorNotify>() {
            tokio::select! {
                _ = &mut sleep => {}
                _ = notify.0.notified() => {}
            }
        } else {
            sleep.await;
        }
    }
}
