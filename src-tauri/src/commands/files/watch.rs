//! 文件树变更监听:递归 notify 监听根目录,变更去抖后 emit `files://tree-changed`。
//! 与 walk.rs 的资产缓存监听相互独立:那里只关心 package.json/yaml/ignore 规则,
//! 这里关心目录结构的任意增删(前端文件树局部刷新用)。

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use notify::Watcher;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::error::{AppError, AppResult, ErrorCode};

/// 变更去抖窗口:git checkout / npm install 等批量变更合并为一次刷新事件
const DEBOUNCE: Duration = Duration::from_millis(400);
/// 单次上报的相对路径上限:超过即视为大规模变更,前端改为全量刷新
const MAX_REPORT_PATHS: usize = 200;

pub const TREE_CHANGED_EVENT: &str = "files://tree-changed";

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TreeChangedPayload {
    /// 监听根目录(`/` 分隔,已归一化)
    root: String,
    /// 变更的仓库相对路径(`/` 分隔,去重);空数组表示事件过多/丢失,前端全量刷新
    paths: Vec<String>,
}

#[derive(Default)]
struct PendingChanges {
    paths: Vec<PathBuf>,
    /// 事件缓冲区溢出(paths 为空)或累计超限:放弃明细,上报全量刷新
    overflow: bool,
}

struct WatchState {
    /// 仅持有保活:drop 即注销递归监听
    _watcher: notify::RecommendedWatcher,
}

static TREE_WATCHERS: OnceLock<Mutex<HashMap<PathBuf, WatchState>>> = OnceLock::new();

/// 为根目录安装(幂等)文件树变更监听。
/// 安装失败返回 Err,前端静默降级为不自动刷新;重复调用同一根目录不叠加监听。
pub(super) fn watch_project_files(app: AppHandle, root: String) -> AppResult<()> {
    let root_path = crate::path_util::clean(Path::new(&root));
    if !root_path.is_dir() {
        return Err(AppError::coded(ErrorCode::InvalidPath, root));
    }
    let watchers = TREE_WATCHERS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = watchers.lock().unwrap();
    if map.contains_key(&root_path) {
        return Ok(());
    }

    let pending = Arc::new(Mutex::new(PendingChanges::default()));
    let scheduled = Arc::new(AtomicBool::new(false));
    let cb_pending = Arc::clone(&pending);
    let cb_scheduled = Arc::clone(&scheduled);
    let cb_root = root_path.clone();
    let cb_app = app.clone();

    let mut watcher = notify::recommended_watcher(move |res| {
        let Ok(event) = res else { return };
        record_event(&cb_pending, &cb_root, &event);
        // 去抖:窗口内首个事件安排一次延迟 flush,期间事件只累积;
        // store(false) 先于取 pending,窗口后到达的事件会再排一轮,不会丢
        if !cb_scheduled.swap(true, Ordering::SeqCst) {
            let flush_pending = Arc::clone(&cb_pending);
            let flush_scheduled = Arc::clone(&cb_scheduled);
            let flush_root = cb_root.clone();
            let flush_app = cb_app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(DEBOUNCE);
                flush_scheduled.store(false, Ordering::SeqCst);
                flush(&flush_app, &flush_root, &flush_pending);
            });
        }
    })
    .map_err(|e| AppError::coded(ErrorCode::IoError, format!("创建文件监听器失败: {e}")))?;

    watcher
        .watch(&root_path, notify::RecursiveMode::Recursive)
        .map_err(|e| AppError::coded(ErrorCode::IoError, format!("监听安装失败: {e}")))?;
    map.insert(root_path, WatchState { _watcher: watcher });
    Ok(())
}

/// 注销根目录的文件树监听(幂等,未监听时为空操作)
pub(super) fn unwatch_project_files(root: String) -> AppResult<()> {
    let root_path = crate::path_util::clean(Path::new(&root));
    if let Some(watchers) = TREE_WATCHERS.get() {
        // watcher 随条目移除 drop,递归监听随之注销
        watchers.lock().unwrap().remove(&root_path);
    }
    Ok(())
}

/// 累积单条事件:只记录 `.git` 之外的路径;paths 为空(缓冲区溢出)或累计超限时
/// 转 overflow,明细不再有意义,flush 时上报空 paths 让前端全量刷新
fn record_event(pending: &Mutex<PendingChanges>, root: &Path, event: &notify::Event) {
    let mut p = pending.lock().unwrap();
    if p.overflow {
        return;
    }
    if event.paths.is_empty() {
        p.overflow = true;
        p.paths.clear();
        return;
    }
    for path in &event.paths {
        if is_inside_git(path) {
            continue;
        }
        p.paths.push(path.strip_prefix(root).unwrap_or(path).to_path_buf());
    }
    if p.paths.len() > MAX_REPORT_PATHS {
        p.overflow = true;
        p.paths.clear();
    }
}

/// 路径是否位于 .git 内:树不展示 .git(list_project_files 跳过),其内部变更无需刷新
fn is_inside_git(path: &Path) -> bool {
    path.components()
        .any(|c| matches!(c, Component::Normal(name) if name == ".git"))
}

/// 去抖窗口结束后上报:相对路径统一为 `/` 分隔并去重;overflow 时 paths 为空
fn flush(app: &AppHandle, root: &Path, pending: &Mutex<PendingChanges>) {
    let taken = {
        let mut p = pending.lock().unwrap();
        std::mem::take(&mut *p)
    };
    let mut paths: Vec<String> = taken
        .paths
        .iter()
        .map(|p| crate::path_util::to_forward_slash(p))
        .collect();
    paths.sort();
    paths.dedup();
    let payload = TreeChangedPayload {
        root: crate::path_util::to_forward_slash(root),
        paths: if taken.overflow { Vec::new() } else { paths },
    };
    let _ = app.emit(TREE_CHANGED_EVENT, payload);
}
