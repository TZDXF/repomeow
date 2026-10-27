use futures::FutureExt;
use std::panic::AssertUnwindSafe;
use tauri::{AppHandle, Manager};
use tokio::select;
use tokio::time::{self, Duration};

use crate::commands::report::{read_schedules, ReportSchedule};
use crate::error::AppResult;
use crate::workday;

use super::calendar::{due_schedules, earliest_fire};
use super::execution::fire_schedule;

const IDLE_INTERVAL: Duration = Duration::from_secs(60);

fn load_schedules(app: &AppHandle) -> AppResult<Vec<ReportSchedule>> {
    let db = app.state::<crate::db::Db>();
    let conn = db.0.lock().unwrap();
    read_schedules(&conn)
}

async fn isolate_panic<F: std::future::Future>(future: F) -> std::thread::Result<F::Output> {
    AssertUnwindSafe(future).catch_unwind().await
}

pub async fn run(app: AppHandle) {
    loop {
        if isolate_panic(run_loop(&app)).await.is_err() {
            eprintln!("[scheduler] 调度循环发生 panic，将在稍后重启");
        }
        time::sleep(IDLE_INTERVAL).await;
    }
}

async fn run_loop(app: &AppHandle) {
    let data_dir = workday::data_dir(&app);
    let cache_root = workday::cache_root();
    let notify = app
        .state::<crate::commands::report::ScheduleNotify>()
        .0
        .clone();

    loop {
        let schedules = match load_schedules(app) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[scheduler] 读取定时任务失败: {e}");
                time::sleep(IDLE_INTERVAL).await;
                continue;
            }
        };

        let now_local = crate::time_util::now_local();
        let due = due_schedules(&schedules, &now_local, &cache_root);
        for schedule in &due {
            match isolate_panic(fire_schedule(app, &data_dir, schedule)).await {
                Ok(Ok(_)) => {},
                Ok(Err(e)) => eprintln!("[scheduler] {}: 执行失败: {e}", schedule.name),
                Err(_) => eprintln!("[scheduler] {}: 执行发生 panic", schedule.name),
            }
        }

        if let Some(deadline) = earliest_fire(&schedules, &now_local) {
            select! {
                _ = time::sleep_until(deadline) => continue,
                _ = notify.notified() => {
                    eprintln!("[scheduler] 定时任务配置已变更,重算触发时间");
                    continue;
                }
            }
        } else {
            select! {
                _ = notify.notified() => {
                    eprintln!("[scheduler] 收到通知,检查定时任务");
                    continue;
                }
                _ = time::sleep(IDLE_INTERVAL) => continue,
            }
        }
    }
}

#[cfg(test)]
mod hardening_tests {
    use super::*;

    #[tokio::test]
    async fn panic_is_isolated_from_following_tasks() {
        let failed = isolate_panic(async { panic!("test panic") }).await;
        assert!(failed.is_err());
        let next = isolate_panic(async { 42 }).await;
        assert_eq!(next.unwrap(), 42);
    }
}
