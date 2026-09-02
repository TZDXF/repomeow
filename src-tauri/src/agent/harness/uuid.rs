//! UUIDv7 生成器:补齐蓝本从 `@earendil-works/pi-ai` 引入的 `uuidv7()`。
//!
//! 格式遵循 RFC 9562:48 位 Unix 毫秒时间戳 + 版本 7 + 变体位 `10` + 随机位。
//! 随机源使用进程内自增计数器与系统纳秒时钟混淆的 xoshiro256**(未引入 uuid/rand
//! 依赖;用于会话/条目 id 生成,不用于安全用途)。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn seed() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    nanos ^ COUNTER.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed)
}

struct Xoshiro256 {
    state: [u64; 4],
}

impl Xoshiro256 {
    fn new(seed: u64) -> Self {
        // SplitMix64 扩散单种子。
        let mut z = seed;
        let mut state = [0u64; 4];
        for slot in &mut state {
            z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut x = z;
            x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            *slot = x ^ (x >> 31);
        }
        if state.iter().all(|&value| value == 0) {
            state[0] = 1;
        }
        Self { state }
    }

    fn next_u64(&mut self) -> u64 {
        let result = self.state[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.state[1] << 17;
        self.state[2] ^= self.state[0];
        self.state[3] ^= self.state[1];
        self.state[1] ^= self.state[2];
        self.state[0] ^= self.state[3];
        self.state[2] ^= t;
        self.state[3] = self.state[3].rotate_left(45);
        result
    }
}

/// 生成一个小写连字符格式的 UUIDv7 字符串。
pub fn uuid_v7() -> String {
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    let mut rng = Xoshiro256::new(seed());
    let rand_a = rng.next_u64() & 0x0FFF;
    let rand_b = rng.next_u64() & 0x3FFF_FFFF_FFFF_FFFF;

    let mut bytes = [0u8; 16];
    bytes[0] = (timestamp_ms >> 40) as u8;
    bytes[1] = (timestamp_ms >> 32) as u8;
    bytes[2] = (timestamp_ms >> 24) as u8;
    bytes[3] = (timestamp_ms >> 16) as u8;
    bytes[4] = (timestamp_ms >> 8) as u8;
    bytes[5] = timestamp_ms as u8;
    // 版本 7(高 4 位)。
    bytes[6] = 0x70 | (rand_a >> 8) as u8;
    bytes[7] = rand_a as u8;
    // 变体位 `10`。
    bytes[8] = 0x80 | (rand_b >> 56) as u8;
    bytes[9] = (rand_b >> 48) as u8;
    bytes[10] = (rand_b >> 40) as u8;
    bytes[11] = (rand_b >> 32) as u8;
    bytes[12] = (rand_b >> 24) as u8;
    bytes[13] = (rand_b >> 16) as u8;
    bytes[14] = (rand_b >> 8) as u8;
    bytes[15] = rand_b as u8;

    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_v7_format_and_version() {
        for _ in 0..64 {
            let id = uuid_v7();
            assert_eq!(id.len(), 36);
            let parts: Vec<&str> = id.split('-').collect();
            assert_eq!(parts.len(), 5);
            assert_eq!(
                (
                    parts[0].len(),
                    parts[1].len(),
                    parts[2].len(),
                    parts[3].len(),
                    parts[4].len()
                ),
                (8, 4, 4, 4, 12)
            );
            assert!(parts[2].starts_with('7'), "version nibble must be 7: {id}");
            let variant = u8::from_str_radix(&parts[3][0..1], 16).unwrap();
            assert!((0x8..=0xB).contains(&variant), "variant must be 10xx: {id}");
            assert!(id.chars().all(|c| c == '-' || c.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn uuid_v7_is_unique() {
        let first = uuid_v7();
        let second = uuid_v7();
        assert_ne!(first, second);
    }
}
