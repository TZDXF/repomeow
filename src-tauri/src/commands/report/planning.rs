use chrono::NaiveDate;
use rusqlite::params;
use serde::Serialize;
use tauri::State;

use crate::db::Db;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::workday;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkWeekRange {
    /// 起止日期 "YYYY-MM-DD"
    pub from: String,
    pub to: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkWeekRanges {
    pub this_week: WorkWeekRange,
    pub last_week: WorkWeekRange,
}

/// 本周/上周工作周的具体日期范围,供生成报告弹窗展示与查询。
pub async fn get_work_week_ranges() -> AppResult<WorkWeekRanges> {
    tokio::task::spawn_blocking(move || {
        let cache_root = workday::cache_root();
        let today = chrono::Local::now().date_naive();
        let fmt = |d: NaiveDate| d.format("%Y-%m-%d").to_string();

        let this_start = crate::scheduler::work_week_start(today, &cache_root);
        let mut last_end = this_start - chrono::Duration::days(1);
        for _ in 0..14 {
            if workday::is_workday(last_end, &cache_root) {
                break;
            }
            last_end -= chrono::Duration::days(1);
        }
        let last_start = crate::scheduler::work_week_start(last_end, &cache_root);

        Ok(WorkWeekRanges {
            this_week: WorkWeekRange {
                from: fmt(this_start),
                to: fmt(today),
            },
            last_week: WorkWeekRange {
                from: fmt(last_start),
                to: fmt(last_end),
            },
        })
    })
    .await
    .map_err(|e| AppError::coded(ErrorCode::ReportTaskFailed, e.to_string()))?
}

/// 批量生成的单个时段(daily: 单日; weekly: 一个工作周)
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchRange {
    pub date_from: String,
    pub date_to: String,
    pub is_workday: bool,
}

/// 已有报告的日期范围(供批量生成"跳过已有"匹配)
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportDateRange {
    pub date_from: String,
    pub date_to: String,
}

const BATCH_DAILY_MAX_DAYS: i64 = 93;
const BATCH_WEEKLY_MAX_DAYS: i64 = 180;

fn parse_date(s: &str) -> AppResult<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
        .map_err(|_| AppError::coded(ErrorCode::ReportInvalidDate, s.to_string()))
}

fn first_workday_from(
    mut d: NaiveDate,
    to: NaiveDate,
    is_workday: &dyn Fn(NaiveDate) -> bool,
) -> NaiveDate {
    while d <= to && !is_workday(d) {
        d += chrono::Duration::days(1);
    }
    d
}

fn plan_weekly_ranges(
    from: NaiveDate,
    to: NaiveDate,
    is_workday: &dyn Fn(NaiveDate) -> bool,
) -> Vec<(NaiveDate, NaiveDate)> {
    let mut ranges = Vec::new();
    let mut seg_start = first_workday_from(from, to, is_workday);
    let mut d = from;
    while d <= to {
        if crate::scheduler::is_work_week_last_day_with(d, is_workday) {
            ranges.push((seg_start, d));
            seg_start = first_workday_from(d + chrono::Duration::days(1), to, is_workday);
        }
        d += chrono::Duration::days(1);
    }
    if seg_start <= to {
        ranges.push((seg_start, to));
    }
    ranges
}

/// 规划批量生成的时段列表。
pub async fn plan_batch_report_ranges(
    period_type: String,
    date_from: String,
    date_to: String,
) -> AppResult<Vec<BatchRange>> {
    tokio::task::spawn_blocking(move || {
        let from = parse_date(&date_from)?;
        let to = parse_date(&date_to)?;
        if from > to {
            return Err(AppError::coded(ErrorCode::ReportDateRangeInverted, ""));
        }
        let span = (to - from).num_days();
        let fmt = |d: NaiveDate| d.format("%Y-%m-%d").to_string();

        let checker = workday::WorkdayChecker::load(&workday::cache_root());
        let is_workday = |d: NaiveDate| checker.is_workday(d);

        if period_type == "weekly" {
            if span > BATCH_WEEKLY_MAX_DAYS {
                return Err(AppError::coded(
                    ErrorCode::ReportBatchWeeklySpanExceeded,
                    BATCH_WEEKLY_MAX_DAYS.to_string(),
                ));
            }
            Ok(plan_weekly_ranges(from, to, &is_workday)
                .into_iter()
                .map(|(seg_from, seg_to)| BatchRange {
                    date_from: fmt(seg_from),
                    date_to: fmt(seg_to),
                    is_workday: true,
                })
                .collect())
        } else {
            if span > BATCH_DAILY_MAX_DAYS {
                return Err(AppError::coded(
                    ErrorCode::ReportBatchDailySpanExceeded,
                    BATCH_DAILY_MAX_DAYS.to_string(),
                ));
            }
            let mut ranges = Vec::new();
            let mut d = from;
            while d <= to {
                ranges.push(BatchRange {
                    date_from: fmt(d),
                    date_to: fmt(d),
                    is_workday: is_workday(d),
                });
                d += chrono::Duration::days(1);
            }
            Ok(ranges)
        }
    })
    .await
    .map_err(|e| AppError::coded(ErrorCode::ReportTaskFailed, e.to_string()))?
}

/// 查询范围内已有报告的日期范围列表(按 period_type 过滤,去重)。
pub fn list_report_dates(
    db: State<'_, Db>,
    period_type: String,
    date_from: String,
    date_to: String,
) -> AppResult<Vec<ReportDateRange>> {
    let conn = db.0.lock().unwrap();
    let mut stmt = conn.prepare(
        "SELECT DISTINCT date_from, date_to FROM report_history
         WHERE period_type = ?1 AND date_to BETWEEN ?2 AND ?3
         ORDER BY date_to",
    )?;
    let rows = stmt
        .query_map(params![period_type, date_from, date_to], |r| {
            Ok(ReportDateRange {
                date_from: r.get(0)?,
                date_to: r.get(1)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn weekday_only(date: NaiveDate) -> bool {
        date.weekday().num_days_from_monday() < 5
    }

    fn range_strs(ranges: &[(NaiveDate, NaiveDate)]) -> Vec<(String, String)> {
        ranges
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    #[test]
    fn weekly_ranges_never_start_on_weekend() {
        let ranges = plan_weekly_ranges(d("2026-08-03"), d("2026-08-16"), &weekday_only);
        assert_eq!(
            range_strs(&ranges),
            vec![
                ("2026-08-03".to_string(), "2026-08-07".to_string()),
                ("2026-08-10".to_string(), "2026-08-14".to_string()),
            ]
        );
    }

    #[test]
    fn weekly_ranges_skip_trailing_weekend() {
        let ranges = plan_weekly_ranges(d("2026-08-03"), d("2026-08-09"), &weekday_only);
        assert_eq!(
            range_strs(&ranges),
            vec![("2026-08-03".to_string(), "2026-08-07".to_string())]
        );
    }

    #[test]
    fn weekly_ranges_trim_leading_weekend() {
        let ranges = plan_weekly_ranges(d("2026-08-08"), d("2026-08-16"), &weekday_only);
        assert_eq!(
            range_strs(&ranges),
            vec![("2026-08-10".to_string(), "2026-08-14".to_string())]
        );
    }

    #[test]
    fn weekly_ranges_no_workday_yields_empty() {
        let ranges = plan_weekly_ranges(d("2026-08-08"), d("2026-08-09"), &weekday_only);
        assert!(ranges.is_empty());
    }

    #[test]
    fn weekly_ranges_adjusted_sunday_starts_next_segment() {
        let adjusted = |date: NaiveDate| weekday_only(date) || date == d("2026-08-09");
        let ranges = plan_weekly_ranges(d("2026-08-03"), d("2026-08-16"), &adjusted);
        assert_eq!(
            range_strs(&ranges),
            vec![
                ("2026-08-03".to_string(), "2026-08-07".to_string()),
                ("2026-08-09".to_string(), "2026-08-14".to_string()),
            ]
        );
    }

    #[test]
    fn weekly_ranges_midweek_holiday_does_not_split() {
        let holiday = |date: NaiveDate| weekday_only(date) && date != d("2026-08-12");
        let ranges = plan_weekly_ranges(d("2026-08-10"), d("2026-08-16"), &holiday);
        assert_eq!(
            range_strs(&ranges),
            vec![("2026-08-10".to_string(), "2026-08-14".to_string())]
        );
        let ranges = plan_weekly_ranges(d("2026-08-12"), d("2026-08-16"), &holiday);
        assert_eq!(
            range_strs(&ranges),
            vec![("2026-08-13".to_string(), "2026-08-14".to_string())]
        );
    }

    #[test]
    fn weekly_ranges_partial_tail_week() {
        let ranges = plan_weekly_ranges(d("2026-08-03"), d("2026-08-12"), &weekday_only);
        assert_eq!(
            range_strs(&ranges),
            vec![
                ("2026-08-03".to_string(), "2026-08-07".to_string()),
                ("2026-08-10".to_string(), "2026-08-12".to_string()),
            ]
        );
    }
}
