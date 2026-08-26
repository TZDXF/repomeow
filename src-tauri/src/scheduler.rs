//! 日报/周报定时调度引擎(订阅模式)。
//!
//! 调度器按职责拆分为日历计算、配置与提示词、任务执行及后台循环。

mod calendar;
mod config;
mod execution;
mod runtime;

pub(crate) use calendar::{
    is_work_week_last_day_with, work_week_start,
};
pub(crate) use execution::fire_schedule;
pub use runtime::run;

#[cfg(test)]
use calendar::{daily_filters_allow, daily_report_date};
#[cfg(test)]
use config::default_schedule_name;
#[cfg(test)]
use execution::{delete_report_history_row, mark_last_run};

#[cfg(test)]
mod tests;
