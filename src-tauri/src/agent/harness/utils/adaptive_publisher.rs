//! 自适应输出发布器:只发布最新状态,不排队中间变更。
//!
//! 空闲后的首个脏状态立即发布;每次发布按其大小“购买”下一段间隔
//! (目标 100KB/s),并用最小间隔限制事件数。调用方在返回的延迟后触发
//! 尾随冲刷,保证最终状态一定会发布。

use std::sync::Arc;
use std::time::{Duration, Instant};

const MIN_INTERVAL_MS: u64 = 100;
const TARGET_BYTES_PER_SECOND: u64 = 100 * 1024; // 100 KB/s

trait PublisherClock: Send + Sync {
    fn now(&self) -> Instant;
}

#[derive(Default)]
struct SystemClock;

impl PublisherClock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

pub struct AdaptivePublisher {
    min_interval: Duration,
    target_bytes_per_second: u64,
    next_emit_at: Option<Instant>,
    dirty: bool,
    clock: Arc<dyn PublisherClock>,
}

impl Default for AdaptivePublisher {
    fn default() -> Self {
        Self::new()
    }
}

impl AdaptivePublisher {
    pub fn new() -> Self {
        Self::with_clock(Arc::new(SystemClock))
    }

    fn with_clock(clock: Arc<dyn PublisherClock>) -> Self {
        Self {
            min_interval: Duration::from_millis(MIN_INTERVAL_MS),
            target_bytes_per_second: TARGET_BYTES_PER_SECOND,
            next_emit_at: None,
            dirty: false,
            clock,
        }
    }

    /// 标记有新输出。返回 `Some` 时调用方应在该延迟后尝试尾随冲刷;
    /// 返回 `None` 表示当前允许立即冲刷。
    pub fn mark_dirty(&mut self, _update_size_bytes: u64) -> Option<Duration> {
        self.dirty = true;
        let now = self.clock.now();
        match self.next_emit_at {
            Some(next_emit_at) if now < next_emit_at => Some(next_emit_at - now),
            _ => None,
        }
    }

    /// 成功冲刷后记录本次大小,并安排下一次允许发布的时间。
    pub fn record_flush(&mut self, update_size_bytes: u64) {
        let delay = self.delay_for_update(update_size_bytes);
        self.next_emit_at = Some(self.clock.now() + delay);
        self.dirty = false;
    }

    pub fn should_flush(&self) -> bool {
        self.dirty
            && self
                .next_emit_at
                .map_or(true, |emit_at| self.clock.now() >= emit_at)
    }

    fn delay_for_update(&self, update_size_bytes: u64) -> Duration {
        let millis = update_size_bytes
            .saturating_mul(1000)
            .checked_div(self.target_bytes_per_second.max(1))
            .unwrap_or(u64::MAX);
        Duration::from_millis(millis).max(self.min_interval)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    #[derive(Default)]
    struct TestClock {
        current: StdMutex<Option<Instant>>,
    }

    impl PublisherClock for TestClock {
        fn now(&self) -> Instant {
            let current = self
                .current
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            current
                .as_ref()
                .expect("test clock should be initialised")
                .to_owned()
        }
    }

    impl TestClock {
        fn set(&self, instant: Instant) {
            *self
                .current
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(instant);
        }
    }

    #[test]
    fn first_update_is_immediate_and_flush_buys_delay() {
        let clock = Arc::new(TestClock::default());
        let start = Instant::now();
        clock.set(start);
        let mut publisher = AdaptivePublisher::with_clock(clock.clone());

        assert_eq!(publisher.mark_dirty(100 * 1024), None);
        publisher.record_flush(100 * 1024);
        assert!(!publisher.should_flush());

        clock.set(start + Duration::from_millis(999));
        assert!(!publisher.should_flush());
        clock.set(start + Duration::from_millis(1000));
        assert!(publisher.mark_dirty(1).is_none());
    }

    #[test]
    fn dirty_state_within_interval_is_delayed() {
        let clock = Arc::new(TestClock::default());
        let start = Instant::now();
        clock.set(start);
        let mut publisher = AdaptivePublisher::with_clock(clock.clone());
        publisher.record_flush(0);

        clock.set(start + Duration::from_millis(10));
        let delay = publisher.mark_dirty(1).expect("dirty update should delay");
        assert_eq!(delay, Duration::from_millis(90));

        clock.set(start + Duration::from_millis(100));
        assert!(publisher.should_flush());
    }

    #[test]
    fn large_flush_delay_is_proportional_to_size() {
        let clock = Arc::new(TestClock::default());
        let start = Instant::now();
        clock.set(start);
        let mut publisher = AdaptivePublisher::with_clock(clock.clone());

        publisher.record_flush(200 * 1024);
        assert_eq!(
            publisher.next_emit_at,
            Some(start + Duration::from_millis(2000))
        );
    }
}
