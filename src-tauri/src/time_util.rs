//! 时间戳获取辅助:全仓统一入口,避免各处散写 `chrono::Utc::now()` 样板。

/// 当前 UTC 时间。
pub fn now_utc() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now()
}

/// 当前本地时间。
pub fn now_local() -> chrono::DateTime<chrono::Local> {
    chrono::Local::now()
}

/// 当前系统时钟，供需要保留 SystemTime 语义的调用方使用。
pub fn now_system() -> std::time::SystemTime {
    std::time::SystemTime::now()
}

/// 当前 Unix 秒
pub fn now_ts() -> i64 {
    now_utc().timestamp()
}

/// 当前 Unix 纳秒(平台时钟不支持纳秒时回退 0)
pub fn now_ts_nanos() -> i64 {
    now_utc().timestamp_nanos_opt().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_helpers_return_modern_time() {
        assert!(now_ts() > 1_600_000_000);
        assert!(now_ts_nanos() > 1_600_000_000_000_000_000);
        assert!(now_local().timestamp() > 1_600_000_000);
        assert!(now_system().duration_since(std::time::UNIX_EPOCH).is_ok());
    }
}
