use super::*;

/// 后台 fetch 并发上限(超出排队)
pub(super) static FETCH_PERMITS: OnceLock<Semaphore> = OnceLock::new();
/// 单次 fetch 的总超时(覆盖 ssh:// 等非 http 协议;http 协议另有低速/连接超时配置)
pub(super) const FETCH_TIMEOUT: Duration = Duration::from_secs(90);
/// fetch 失败后的基础退避间隔,随连续失败次数指数增长,封顶 15 分钟
pub(super) const FETCH_RETRY_BASE: Duration = Duration::from_secs(30);
pub(super) const FETCH_RETRY_MAX: Duration = Duration::from_secs(15 * 60);

/// fetch 治理状态:进行中去重 + 失败退避
pub(super) struct FetchTracker {
    /// 正在 fetch 的路径(进行中不重复发起,key 与 STATUS_CACHE 同步走 clean_str)
    in_progress: HashSet<String>,
    /// 路径 → (下次允许 fetch 的时刻, 连续失败次数)
    retry_after: HashMap<String, (Instant, u32)>,
}

pub(super) static FETCH_TRACKER: OnceLock<Mutex<FetchTracker>> = OnceLock::new();

pub(super) fn fetch_tracker() -> &'static Mutex<FetchTracker> {
    FETCH_TRACKER.get_or_init(|| {
        Mutex::new(FetchTracker {
            in_progress: HashSet::new(),
            retry_after: HashMap::new(),
        })
    })
}

/// 原子检查并登记一次 fetch,避免多个检查请求在判断与登记之间发生竞争。
/// key 与 STATUS_CACHE 同步走 clean_str,避免 Windows 上同一仓库的混合路径
/// 绕过进行中去重 / 失败退避。
pub(super) fn try_begin_fetch(path: &str) -> bool {
    let key = crate::path_util::clean_str(path);
    let mut tracker = fetch_tracker().lock().unwrap();
    if tracker.in_progress.contains(&key) {
        return false;
    }
    let due = match tracker.retry_after.get(&key) {
        Some((at, _)) => *at <= Instant::now(),
        None => true,
    };
    if due {
        tracker.in_progress.insert(key);
    }
    due
}

/// fetch 结束回调:成功清除退避记录;失败按连续失败次数指数退避
/// (30s 起步、封顶 15 分钟),弱网/断网时不会每个检查周期都重复撞网络
pub(super) fn fetch_finished(path: &str, ok: bool) {
    let key = crate::path_util::clean_str(path);
    let mut tracker = fetch_tracker().lock().unwrap();
    tracker.in_progress.remove(&key);
    if ok {
        tracker.retry_after.remove(&key);
    } else {
        let fails = tracker.retry_after.get(&key).map(|(_, f)| *f).unwrap_or(0) + 1;
        let backoff = FETCH_RETRY_BASE
            .saturating_mul(2u32.saturating_pow(fails.saturating_sub(1)))
            .min(FETCH_RETRY_MAX);
        tracker
            .retry_after
            .insert(key, (Instant::now() + backoff, fails));
    }
}
