//! 时间戳获取辅助:全仓统一入口,避免各处散写 `chrono::Utc::now()` 样板。

/// 当前 Unix 秒
pub fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

/// 当前 Unix 纳秒(平台时钟不支持纳秒时回退 0)
pub fn now_ts_nanos() -> i64 {
    chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_ts_monotonic_nanos() {
        let a = now_ts_nanos();
        let b = now_ts_nanos();
        assert!(b >= a, "纳秒时钟不应回退");
        assert!(now_ts() > 1_600_000_000, "Unix 秒应为现代时间");
    }
}
