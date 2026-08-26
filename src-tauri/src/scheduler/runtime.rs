use chrono::Local;
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

pub async fn run(app: AppHandle) {
    let data_dir = workday::data_dir(&app);
    let cache_root = workday::cache_root();
    let notify = app
        .state::<crate::commands::report::ScheduleNotify>()
        .0
        .clone();

    loop {
        let schedules = match load_schedules(&app) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[scheduler] 读取定时任务失败: {e}");
                time::sleep(IDLE_INTERVAL).await;
                continue;
            }
        };

        let now_local = Local::now();
        let due = due_schedules(&schedules, &now_local, &cache_root);
        for schedule in &due {
            if let Err(e) = fire_schedule(&app, &data_dir, schedule).await {
                eprintln!("[scheduler] {}: 执行失败: {e}", schedule.name);
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
