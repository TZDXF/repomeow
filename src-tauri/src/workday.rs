//! 中国工作日判断:从 CDN 拉取 chinese-days 数据并缓存,提供 is_workday() 查询。
//!
//! 数据源: <https://cdn.jsdelivr.net/npm/chinese-days/dist/chinese-days.json>
//! 覆盖范围: 2004–2026 年官方节假日和调休安排。
//! 无数据年份回退为常规周一～周五判断。
//!
//! 缓存结构:把 `chinese-days.json` 包成 `{ "downloaded_at": <unix_ts>, "data": <原 JSON> }`,
//! 仅在包络解析成功且 `downloaded_at` 位于 TTL 内时使用缓存。
//! 文件不存在、解析失败、字段缺失或 TTL 过期时重新拉取数据。
//! 缓存文件位于安装目录 data/ 下(见 cache_root)。

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{Datelike, NaiveDate, Utc};
use reqwest::blocking::Client;
use serde::Deserialize;
use tauri::Manager;

use crate::error::{AppError, AppResult, ErrorCode};
use crate::APP_DATA_DIR_NAME;

const CDN_URL: &str = "https://cdn.jsdelivr.net/npm/chinese-days/dist/chinese-days.json";
const CACHE_FILE: &str = "chinese-days.json";
/// 缓存有效期(秒),默认 30 天
const CACHE_TTL_SECS: i64 = 30 * 86400;

#[derive(Debug, Deserialize)]
struct ChineseDaysData {
    holidays: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(default)]
    workdays: Option<serde_json::Map<String, serde_json::Value>>,
}

/// 缓存信封:`{ downloaded_at: <unix_ts>, data: <chinese-days 原始 JSON> }`
#[derive(Debug, Deserialize)]
struct CacheEnvelope {
    downloaded_at: i64,
    data: ChineseDaysData,
}

/// 加载工作日数据:优先读缓存,过期或不存在时从 CDN 拉取并写入缓存。
/// 返回 holidays 与 workdays 两个日期集合(均为 "YYYY-MM-DD" 格式)。
/// `cache_root` 为运行时数据根目录(`runtime_data_root`,缓存文件直接位于其下)。
pub fn load_data(cache_root: &PathBuf) -> AppResult<(HashSet<String>, HashSet<String>)> {
    let cache_path = cache_root.join(CACHE_FILE);

    // 缓存仅在包络合法且 TTL 未过期时命中
    if let Some(parsed) = try_load_envelope(&cache_path) {
        return Ok(parsed);
    }

    // 从 CDN 拉取
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| AppError::coded(ErrorCode::WorkdayHttpClientFailed, e.to_string()))?;

    let resp = client
        .get(CDN_URL)
        .header("User-Agent", "repomeow/0.1")
        .send()
        .map_err(|e| AppError::coded(ErrorCode::WorkdayFetchFailed, e.to_string()))?;

    let body = resp
        .text()
        .map_err(|e| AppError::coded(ErrorCode::WorkdayResponseReadFailed, e.to_string()))?;

    // 校验响应内容合法后再写缓存,避免把错误响应固化到缓存
    let parsed = parse_data(&body)?;
    write_cache(&cache_path, &body);
    Ok(parsed)
}

/// 纯函数:仅当缓存是合法 envelope 且未过期时才返回解析结果。
/// 失败 / 不存在 / 字段缺失 / 过期 → 返回 `None`,由调用方决定是否触发 fetch。
fn try_load_envelope(cache_path: &Path) -> Option<(HashSet<String>, HashSet<String>)> {
    let raw = fs::read_to_string(cache_path).ok()?;
    let (parsed, ts) = parse_cache(&raw)?;
    let age = Utc::now().timestamp().saturating_sub(ts);
    if age >= 0 && age < CACHE_TTL_SECS {
        Some(parsed)
    } else {
        None
    }
}

/// 把 CDN 响应写入缓存,包络当前时间戳。
/// 写盘失败仅记录日志,不影响本次返回。
fn write_cache(cache_path: &std::path::Path, body: &str) {
    let envelope = serde_json::json!({
        "downloaded_at": Utc::now().timestamp(),
        "data": serde_json::from_str::<serde_json::Value>(body).unwrap_or_else(|_| serde_json::Value::String(body.to_string())),
    });
    if let Ok(s) = serde_json::to_string(&envelope) {
        if let Err(e) = fs::write(cache_path, s) {
            eprintln!("[workday] 写入缓存失败: {e}");
        }
    }
}

/// 解析缓存包络。
/// 格式合法时返回数据与下载时间戳,否则返回 `None`。
fn parse_cache(raw: &str) -> Option<((HashSet<String>, HashSet<String>), i64)> {
    let env: CacheEnvelope = serde_json::from_str(raw).ok()?;
    let holidays: HashSet<String> = env
        .data
        .holidays
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    let workdays: HashSet<String> = env
        .data
        .workdays
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    Some(((holidays, workdays), env.downloaded_at))
}

fn parse_data(json: &str) -> AppResult<(HashSet<String>, HashSet<String>)> {
    let data: ChineseDaysData = serde_json::from_str(json)
        .map_err(|e| AppError::coded(ErrorCode::WorkdayParseFailed, e.to_string()))?;

    let holidays: HashSet<String> = data
        .holidays
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    let workdays: HashSet<String> = data
        .workdays
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();

    Ok((holidays, workdays))
}

/// 判断给定日期是否为中国工作日(含调休补班,排除法定节假日)。
///
/// * 若日期在 workdays 集合中 → `true`(调休上班的周六/周日)
/// * 若日期不在 holidays 集合中且为周一～周五 → `true`(常规工作日)
/// * 其他情况 → `false`
///
/// `cache_root` 为运行时数据根目录(安装目录 data/)。
pub fn is_workday(date: NaiveDate, cache_root: &PathBuf) -> bool {
    // 数据拉取失败时回退为常规周一～周五判断
    let (holidays, workdays) = match load_data(cache_root) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("[workday] 加载工作日数据失败,回退常规判断: {e}");
            return is_regular_weekday(date);
        }
    };

    let date_str = date.format("%Y-%m-%d").to_string();

    // 调休上班(周日/周六补班)优先级最高
    if workdays.contains(&date_str) {
        return true;
    }
    // 法定节假日
    if holidays.contains(&date_str) {
        return false;
    }
    // 常规判断
    is_regular_weekday(date)
}

fn is_regular_weekday(date: NaiveDate) -> bool {
    let w = date.weekday().num_days_from_monday();
    w < 5 // Monday(0) ~ Friday(4)
}

/// 预加载的工作日判定器:批量场景(如批量生成报告规划)避免每天重复读缓存文件。
/// 数据加载失败时为空集合,语义同 is_workday 的回退路径(常规周一~周五)。
pub struct WorkdayChecker {
    holidays: HashSet<String>,
    workdays: HashSet<String>,
}

impl WorkdayChecker {
    pub fn load(cache_root: &PathBuf) -> Self {
        let (holidays, workdays) = match load_data(cache_root) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("[workday] 加载工作日数据失败,回退常规判断: {e}");
                (HashSet::new(), HashSet::new())
            }
        };
        Self { holidays, workdays }
    }

    pub fn is_workday(&self, date: NaiveDate) -> bool {
        let date_str = date.format("%Y-%m-%d").to_string();
        // 调休上班(周日/周六补班)优先级最高
        if self.workdays.contains(&date_str) {
            return true;
        }
        // 法定节假日
        if self.holidays.contains(&date_str) {
            return false;
        }
        is_regular_weekday(date)
    }
}

/// 获取数据目录路径(与 lib.rs 中 Db::open 一致)
pub fn data_dir(app: &tauri::AppHandle) -> PathBuf {
    app.path()
        .home_dir()
        .unwrap_or_default()
        .join(APP_DATA_DIR_NAME)
}

/// chinese-days 缓存所在目录:安装目录 data/
pub fn cache_root() -> PathBuf {
    crate::runtime_data_root()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn regular_weekday_check() {
        // 2026-07-21 是周二
        let tue = NaiveDate::from_ymd_opt(2026, 7, 21).unwrap();
        assert!(is_regular_weekday(tue));

        // 2026-07-25 是周六
        let sat = NaiveDate::from_ymd_opt(2026, 7, 25).unwrap();
        assert!(!is_regular_weekday(sat));

        // 2026-07-26 是周日
        let sun = NaiveDate::from_ymd_opt(2026, 7, 26).unwrap();
        assert!(!is_regular_weekday(sun));
    }

    /// 构造一个伪 chinese-days JSON 的最小可用形态
    fn sample_inner_json() -> serde_json::Value {
        serde_json::json!({
            "holidays": {
                "2026-10-01": { "name": "国庆", "date": "2026-10-01" },
                "2026-10-02": { "name": "国庆", "date": "2026-10-02" },
            },
            "workdays": {
                "2026-09-27": { "name": "调休", "date": "2026-09-27" },
            },
        })
    }

    #[test]
    fn parse_cache_envelope_extracts_sets_and_timestamp() {
        let inner = sample_inner_json();
        let envelope = serde_json::json!({
            "downloaded_at": 1_700_000_000i64,
            "data": inner,
        })
        .to_string();
        let ((holidays, workdays), ts) = parse_cache(&envelope).expect("应解析包络");
        assert_eq!(ts, 1_700_000_000);
        let mut h: Vec<&String> = holidays.iter().collect();
        h.sort();
        assert_eq!(h, vec!["2026-10-01", "2026-10-02"]);
        let w: Vec<&String> = workdays.iter().collect();
        assert_eq!(w, vec!["2026-09-27"]);
    }

    #[test]
    fn parse_cache_rejects_legacy_raw_json() {
        // 裸 JSON 不符合缓存包络格式
        let legacy = sample_inner_json().to_string();
        assert!(
            parse_cache(&legacy).is_none(),
            "旧版裸 JSON 不应被包装格式解析器消费"
        );
    }

    #[test]
    fn parse_cache_rejects_missing_data_field() {
        // envelope 缺少 data 字段应解析失败
        let bad = serde_json::json!({"downloaded_at": 1}).to_string();
        assert!(parse_cache(&bad).is_none());
    }

    #[test]
    fn try_load_envelope_accepts_fresh_envelope() {
        let dir = std::env::temp_dir().join(format!(
            "repomeow-workday-test-fresh-{}-{}",
            std::process::id(),
            crate::time_util::now_ts_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(CACHE_FILE);
        let body = sample_inner_json().to_string();
        write_cache(&path, &body);

        let parsed = try_load_envelope(&path)
            .expect("fresh envelope should be honored without mtime fallback");
        let (holidays, workdays) = parsed;
        assert_eq!(
            holidays,
            HashSet::from(["2026-10-01".to_string(), "2026-10-02".to_string()])
        );
        assert_eq!(workdays, HashSet::from(["2026-09-27".to_string()]));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn try_load_envelope_rejects_legacy_bare_json() {
        let dir = std::env::temp_dir().join(format!(
            "repomeow-workday-test-legacy-{}-{}",
            std::process::id(),
            crate::time_util::now_ts_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(CACHE_FILE);
        // 裸 JSON 不符合缓存包络格式
        std::fs::write(&path, sample_inner_json().to_string()).unwrap();
        assert!(
            try_load_envelope(&path).is_none(),
            "legacy bare-JSON cache should not be promoted by mtime"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn try_load_envelope_rejects_expired_envelope() {
        let dir = std::env::temp_dir().join(format!(
            "repomeow-workday-test-expired-{}-{}",
            std::process::id(),
            crate::time_util::now_ts_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(CACHE_FILE);
        let stale_ts = crate::time_util::now_ts() - (CACHE_TTL_SECS + 60);
        let inner = sample_inner_json();
        let envelope = serde_json::json!({ "downloaded_at": stale_ts, "data": inner }).to_string();
        std::fs::write(&path, envelope).unwrap();

        assert!(
            try_load_envelope(&path).is_none(),
            "expired envelope must trigger refetch"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_data_handles_missing_optional_sections() {
        // holidays 缺失 → 空集合;workdays 缺失 → 空集合
        let no_holidays = serde_json::json!({"workdays": { "2026-09-27": {} }}).to_string();
        let (h, w) = parse_data(&no_holidays).unwrap();
        assert!(h.is_empty());
        assert_eq!(w, HashSet::from(["2026-09-27".to_string()]));

        let no_workdays = serde_json::json!({"holidays": { "2026-10-01": {} }}).to_string();
        let (h, w) = parse_data(&no_workdays).unwrap();
        assert_eq!(h, HashSet::from(["2026-10-01".to_string()]));
        assert!(w.is_empty());
    }

    #[test]
    fn write_cache_then_parse_cache_roundtrip() {
        // 写盘 → 读盘 → parse_cache 应能拿到原始数据,时间戳取写入瞬间
        let dir = std::env::temp_dir().join(format!(
            "repomeow-workday-test-{}-{}",
            std::process::id(),
            crate::time_util::now_ts_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(CACHE_FILE);

        let body = sample_inner_json().to_string();
        write_cache(&path, &body);

        let raw = std::fs::read_to_string(&path).unwrap();
        let ((holidays, workdays), ts) = parse_cache(&raw).expect("应解析刚刚写入的包络");
        // 时间戳应接近现在(±10 秒)
        let now = Utc::now().timestamp();
        assert!(
            (now - ts).abs() < 10,
            "downloaded_at 应为当前时刻(now={now}, ts={ts})"
        );
        assert_eq!(
            holidays,
            HashSet::from(["2026-10-01".to_string(), "2026-10-02".to_string()])
        );
        assert_eq!(workdays, HashSet::from(["2026-09-27".to_string()]));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ttl_boundary_for_envelope() {
        // 验证:now - downloaded_at < TTL 通过,>= TTL 失败(由调用方负责)
        let now = Utc::now().timestamp();
        let fresh = now - 10;
        let just_inside = now - (CACHE_TTL_SECS - 1);
        let just_outside = now - (CACHE_TTL_SECS + 1);

        let inner = sample_inner_json();
        let make = |ts: i64| serde_json::json!({ "downloaded_at": ts, "data": inner }).to_string();

        for (label, raw, expected_age_lt_ttl) in [
            ("fresh", make(fresh), true),
            ("just_inside", make(just_inside), true),
            ("just_outside", make(just_outside), false),
        ] {
            let ((_, _), ts) = parse_cache(&raw).unwrap();
            let age = Utc::now().timestamp().saturating_sub(ts);
            assert_eq!(
                age < CACHE_TTL_SECS,
                expected_age_lt_ttl,
                "{label}: age={age}"
            );
        }
    }
}
