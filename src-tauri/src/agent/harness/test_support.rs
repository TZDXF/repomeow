//! 测试夹具:进程内原子创建、每个实例排他拥有的临时目录。
//!
//! 背景(TZD-114):多个测试模块原先各自用 `pid + SystemTime::now()` 的纳秒值
//! 拼临时目录名,并假设该名字必然唯一。并发测试(默认多线程)下,不同线程读时钟
//! 可能取到相同纳秒值,于是两个实例拿到同一个目录;各自的 `Drop` 里又无条件
//! `remove_dir_all`,先结束的一方会删掉对方的夹具,表现为随机的
//! `NotFound`(os error 3)或 `文件已存在`(os error 183)。
//!
//! 本夹具把“名字唯一”这个假设换成可验证的所有权:名字只是候选名,
//! `create_dir` 原子创建成功的一方才是该目录的所有者;候选名已被占用
//! (历史残留或另一进程)就换名重试,绝不接管别人的目录。
//! 因此 `Drop` 只删除本实例创建成功的那一个目录。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// 进程内候选名计数器:即使时钟粒度不足,同进程内也不会重复取名。
static CANDIDATE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 候选名被连续占用的上限(仅防死循环;正常情况第一次就成功)。
const MAX_ATTEMPTS: u32 = 1_000;

/// 原子创建、排他拥有的临时目录。
pub(crate) struct ExclusiveTempDir {
    path: PathBuf,
}

impl ExclusiveTempDir {
    pub(crate) fn new(tag: &str) -> Self {
        let pid = std::process::id();
        for _ in 0..MAX_ATTEMPTS {
            let candidate = std::env::temp_dir().join(format!(
                "repomeow-test-{tag}-{pid}-{}",
                CANDIDATE_COUNTER.fetch_add(1, Ordering::SeqCst)
            ));
            match std::fs::create_dir(&candidate) {
                // 原子创建成功 = 本实例拥有该目录,不会与他人共享。
                Ok(()) => return Self { path: candidate },
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("创建测试临时目录失败 {candidate:?}: {error}"),
            }
        }
        panic!("连续 {MAX_ATTEMPTS} 次候选目录名都被占用(tag={tag})");
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ExclusiveTempDir {
    fn drop(&mut self) {
        // 只清理自己创建成功的目录;删不掉(已被人删除)也不影响测试结论。
        std::fs::remove_dir_all(&self.path).ok();
    }
}

mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn concurrent_creation_never_hands_out_the_same_directory() {
        // 32 线程 × 64 次:并发取名 + 原子创建,目录必须两两互异。
        let handles: Vec<_> = (0..32)
            .map(|_| {
                std::thread::spawn(|| {
                    (0..64)
                        .map(|_| ExclusiveTempDir::new("collide").path().to_path_buf())
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let mut paths = Vec::new();
        for handle in handles {
            paths.extend(handle.join().unwrap());
        }
        let unique: HashSet<&PathBuf> = paths.iter().collect();
        assert_eq!(paths.len(), unique.len(), "存在重复目录: {paths:?}");
    }

    #[test]
    fn drop_only_removes_own_directory() {
        let owner = ExclusiveTempDir::new("ownership");
        let path = owner.path().to_path_buf();
        assert!(path.is_dir());

        // 另一个实例的目录在 owner 析构后仍必须存在。
        let other = ExclusiveTempDir::new("ownership");
        let other_path = other.path().to_path_buf();
        drop(owner);
        assert!(!path.exists());
        assert!(other_path.is_dir());
    }
}
