use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

const BACKGROUND_TASK_EVENT: &str = "background://task-progress";
static NEXT_TASK_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Serialize)]
struct BackgroundTaskPayload<'a> {
    task_id: &'a str,
    kind: &'a str,
    label: &'a str,
    completed: usize,
    total: usize,
    status: &'a str,
}

/// Rust 后台任务的统一进度事件守卫。创建即发布 running，离开作用域时无论成功或失败
/// 都发布 finished，避免前端标题栏残留已经结束的任务。
pub(crate) struct BackgroundTask {
    app: AppHandle,
    task_id: String,
    kind: &'static str,
    label: String,
    completed: usize,
    total: usize,
}

impl BackgroundTask {
    pub(crate) fn new(
        app: &AppHandle,
        kind: &'static str,
        label: impl Into<String>,
        total: usize,
    ) -> Self {
        let task = Self {
            app: app.clone(),
            task_id: format!("{kind}-{}", NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed)),
            kind,
            label: label.into(),
            completed: 0,
            total,
        };
        task.emit("running");
        task
    }

    pub(crate) fn set_completed(&mut self, completed: usize) {
        self.completed = completed.min(self.total);
        self.emit("running");
    }

    fn emit(&self, status: &str) {
        let _ = self.app.emit(
            BACKGROUND_TASK_EVENT,
            BackgroundTaskPayload {
                task_id: &self.task_id,
                kind: self.kind,
                label: &self.label,
                completed: self.completed,
                total: self.total,
                status,
            },
        );
    }
}

impl Drop for BackgroundTask {
    fn drop(&mut self) {
        self.emit("finished");
    }
}
