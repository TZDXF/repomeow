use super::*;

/// 进行中的克隆任务(job_id -> 子进程),供 cancel_git_clone 查找并 kill
pub(super) static CLONE_JOBS: OnceLock<tokio::sync::Mutex<HashMap<String, tokio::process::Child>>> =
    OnceLock::new();

pub(super) fn clone_jobs() -> &'static tokio::sync::Mutex<HashMap<String, tokio::process::Child>> {
    CLONE_JOBS.get_or_init(|| tokio::sync::Mutex::new(HashMap::new()))
}

/// 所有运行中的 git 子进程 PID(fetch + clone 统一登记),
/// 供应用退出钩子 cleanup_on_exit 在拿不到句柄时按 PID 杀进程树
pub(super) static GIT_PIDS: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();

pub(super) fn git_pids() -> &'static Mutex<HashSet<u32>> {
    GIT_PIDS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// RAII 守卫:构造时登记 PID,drop 时注销。保证无论函数从成功/失败/超时/
/// 取消哪条路径返回都自动移除,不会残留已退出的 PID(或残留也无害——
/// cleanup_on_exit 对不存在的 PID taskkill 只会返回错误,不影响其他 PID)
pub(super) struct TrackedPid(Option<u32>);

impl TrackedPid {
    pub(super) fn new(pid: Option<u32>) -> Self {
        if let Some(p) = pid {
            git_pids().lock().unwrap().insert(p);
        }
        Self(pid)
    }
}

impl Drop for TrackedPid {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            git_pids().lock().unwrap().remove(&p);
        }
    }
}

/// 应用退出收尾:杀掉所有仍在运行的 git 子进程(fetch/clone 及其
/// remote-helper 孙进程整棵进程树)。由 lib.rs 的 RunEvent::Exit 钩子调用。
///
/// 走 PID 层而非句柄层:fetch child 句柄困在 spawn 出的 task 内部,
/// clone child 在 async Mutex(CLONE_JOBS)里,退出钩子很难可靠触及;
/// 而 PID 是 spawn 后立即拷出的独立副本,始终可达。
pub fn cleanup_on_exit() {
    // 1) PID 层:taskkill 杀整棵进程树(覆盖孙进程)
    let pids: Vec<u32> = git_pids().lock().unwrap().drain().collect();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        for pid in pids {
            let mut cmd = Command::new("taskkill");
            cmd.args(["/PID", &pid.to_string(), "/T", "/F"]);
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            let _ = cmd.output();
        }
    }
    // 2) 兜底:回收 clone 句柄(CLONE_JOBS 是 async Mutex,try_lock 重试几次)
    for _ in 0..20 {
        if let Ok(mut jobs) = clone_jobs().try_lock() {
            for (_, mut child) in jobs.drain() {
                let _ = child.start_kill();
                let _ = child.wait();
            }
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
