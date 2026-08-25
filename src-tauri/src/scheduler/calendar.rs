use std::path::PathBuf;

use chrono::{Datelike, Local, NaiveDate, NaiveTime, Timelike};
use tokio::time::{Duration, Instant};

use crate::commands::report::ReportSchedule;
use crate::workday;

fn next_fire(schedule: &ReportSchedule, now_local: &chrono::DateTime<Local>) -> Option<Instant> {
    if !schedule.enabled {
        return None;
    }

    let time_parts: Vec<&str> = schedule.time_of_day.split(':').collect();
    if time_parts.len() != 2 {
        return None;
    }
    let hour: u32 = time_parts[0].parse().ok()?;
    let minute: u32 = time_parts[1].parse().ok()?;
    let target_time = NaiveTime::from_hms_opt(hour, minute, 0)?;

    let today = now_local.date_naive();
    let today_target = today
        .and_time(target_time)
        .and_local_timezone(Local)
        .single()?;

    if today_target <= *now_local {
        let tomorrow = today.succ_opt()?;
        return tomorrow
            .and_time(target_time)
            .and_local_timezone(Local)
            .single()
            .map(|dt| {
                let dur = dt.signed_duration_since(*now_local);
                Instant::now() + Duration::from_secs(dur.to_std().unwrap_or_default().as_secs())
            });
    }

    let dur = today_target.signed_duration_since(*now_local);
    Some(Instant::now() + Duration::from_secs(dur.to_std().unwrap_or_default().as_secs()))
}

pub(crate) fn earliest_fire(
    schedules: &[ReportSchedule],
    now_local: &chrono::DateTime<Local>,
) -> Option<Instant> {
    schedules
        .iter()
        .filter_map(|s| next_fire(s, now_local))
        .min()
}

fn has_workday_in_week_with(monday: NaiveDate, is_workday: &dyn Fn(NaiveDate) -> bool) -> bool {
    (0..7).any(|i| is_workday(monday + chrono::Duration::days(i)))
}

pub(crate) fn work_week_start_with(
    today: NaiveDate,
    is_workday: &dyn Fn(NaiveDate) -> bool,
) -> NaiveDate {
    let monday = today - chrono::Duration::days(today.weekday().num_days_from_monday() as i64);
    let sunday_before = monday - chrono::Duration::days(1);
    if has_workday_in_week_with(monday, is_workday) && is_workday(sunday_before) {
        return sunday_before;
    }
    (0..7)
        .map(|i| monday + chrono::Duration::days(i))
        .find(|d| is_workday(*d))
        .unwrap_or(monday)
}

pub(crate) fn work_week_start(today: NaiveDate, cache_root: &PathBuf) -> NaiveDate {
    work_week_start_with(today, &|d| workday::is_workday(d, cache_root))
}

pub(crate) fn is_work_week_last_day_with(
    today: NaiveDate,
    is_workday: &dyn Fn(NaiveDate) -> bool,
) -> bool {
    if !is_workday(today) {
        return false;
    }
    let dow = today.weekday().num_days_from_monday();
    if dow == 6 {
        let next_monday = today + chrono::Duration::days(1);
        return !has_workday_in_week_with(next_monday, is_workday);
    }
    for i in 1..=(6 - dow) {
        let d = today + chrono::Duration::days(i as i64);
        if !is_workday(d) {
            continue;
        }
        if i == 6 - dow {
            let next_monday = d + chrono::Duration::days(1);
            if has_workday_in_week_with(next_monday, is_workday) {
                continue;
            }
        }
        return false;
    }
    true
}

fn is_work_week_last_day(today: NaiveDate, cache_root: &PathBuf) -> bool {
    is_work_week_last_day_with(today, &|d| workday::is_workday(d, cache_root))
}

pub(crate) fn daily_report_date(fire_date: NaiveDate, previous_day: bool) -> NaiveDate {
    if previous_day {
        fire_date - chrono::Duration::days(1)
    } else {
        fire_date
    }
}

pub(crate) fn daily_filters_allow(
    report_date: NaiveDate,
    weekdays_only: bool,
    chinese_workday_only: bool,
    is_workday: &dyn Fn(NaiveDate) -> bool,
) -> bool {
    if weekdays_only && report_date.weekday().num_days_from_monday() >= 5 {
        return false;
    }
    if chinese_workday_only && !is_workday(report_date) {
        return false;
    }
    true
}

pub(crate) fn due_schedules(
    schedules: &[ReportSchedule],
    now_local: &chrono::DateTime<Local>,
    cache_root: &PathBuf,
) -> Vec<ReportSchedule> {
    let now_time = now_local.time();
    let today = now_local.date_naive();

    schedules
        .iter()
        .filter(|s| {
            if !s.enabled {
                return false;
            }

            let parts: Vec<&str> = s.time_of_day.split(':').collect();
            if parts.len() != 2 {
                return false;
            }
            let target_h: u32 = parts[0].parse().unwrap_or(99);
            let target_m: u32 = parts[1].parse().unwrap_or(99);
            let diff = (now_time.hour() as i32 * 60 + now_time.minute() as i32)
                - (target_h as i32 * 60 + target_m as i32);
            if diff.abs() > 1 {
                return false;
            }

            if let Some(last) = s.last_run_at {
                let last_date = chrono::DateTime::from_timestamp(last, 0).map(|dt| dt.date_naive());
                if last_date == Some(today) {
                    return false;
                }
            }

            if s.report_type == "weekly" {
                if s.weekly_workweek {
                    return is_work_week_last_day(today, cache_root);
                }
                return today.weekday().number_from_monday() == s.weekly_end_weekday;
            }

            let report_date = daily_report_date(today, s.previous_day);
            daily_filters_allow(report_date, s.weekdays_only, s.chinese_workday_only, &|d| {
                workday::is_workday(d, cache_root)
            })
        })
        .cloned()
        .collect()
}
