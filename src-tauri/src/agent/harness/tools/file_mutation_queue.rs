//! 文件变更串行化队列:对齐 `packages/agent/src/harness/tools/file-mutation-queue.ts`。
//!
//! 以 (env 实例地址, 规范化路径) 为键串行化文件变更;env 被丢弃后其队列条目
//! 会被惰性清除(对齐 TS WeakMap 语义)。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use crate::agent::harness::types::{ExecutionEnv, FileErrorCode};

struct QueueRegistry {
    /// env 指针 → (弱引用, path → 队列锁)。
    entries: Mutex<HashMap<usize, (Weak<dyn ExecutionEnv>, HashMap<String, Arc<tokio::sync::Mutex<()>>>)>>,
}

static REGISTRY: std::sync::OnceLock<QueueRegistry> = std::sync::OnceLock::new();

fn registry() -> &'static QueueRegistry {
    REGISTRY.get_or_init(|| QueueRegistry {
        entries: Mutex::new(HashMap::new()),
    })
}

fn env_key(env: &Arc<dyn ExecutionEnv>) -> usize {
    Arc::as_ptr(env) as *const () as usize
}

/// 计算变更队列键:目标存在的规范化路径,或缺失/不支持时的绝对路径
/// (对齐 TS `getMutationQueueKey`)。
async fn mutation_queue_key(env: &Arc<dyn ExecutionEnv>, path: &str) -> Result<String, crate::agent::harness::types::FileError> {
    let absolute_path: String = crate::agent::harness::types::get_or_throw(
        env.absolute_path(path.to_string(), None).await,
    );
    match env.canonical_path(absolute_path.clone(), None).await {
        Ok(canonical) => Ok(canonical),
        Err(error) => {
            if error.code == FileErrorCode::NotFound || error.code == FileErrorCode::NotSupported {
                Ok(absolute_path)
            } else {
                Err(error)
            }
        }
    }
}

/// 串行化针对同一环境与规范化路径的文件变更
/// (对齐 TS `withFileMutationQueue`)。
pub async fn with_file_mutation_queue<T, F, Fut>(
    env: &Arc<dyn ExecutionEnv>,
    path: &str,
    operation: F,
) -> Result<T, crate::agent::harness::types::FileError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    // 排空已释放 env 的条目(WeakMap 近似)。
    evict_dead_entries();

    let key = mutation_queue_key(env, path).await?;
    let queue_lock = {
        let registry = registry();
        let mut entries = registry
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = entries
            .entry(env_key(env))
            .or_insert_with(|| (Arc::downgrade(env), HashMap::new()));
        entry
            .1
            .entry(key)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    };

    let _guard = queue_lock.lock().await;
    Ok(operation().await)
}

fn evict_dead_entries() {
    let registry = registry();
    let mut entries = registry
        .entries
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    entries.retain(|_, (weak, _)| weak.upgrade().is_some());
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::future::BoxFuture;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 测试用最小 env(仅 absolute_path/canonical_path/exists 会用到)。
    struct NullEnv;

    macro_rules! null_fs_methods {
        ($self:ident) => {
            fn cwd(&$self) -> &str {
                "/"
            }

            fn absolute_path<'a>(&'a $self, path: String, _abort: Option<crate::agent::types::AbortSignal>) -> BoxFuture<'a, Result<String, crate::agent::harness::types::FileError>> {
                Box::pin(async move { Ok(path) })
            }

            fn join_path<'a>(&'a $self, parts: Vec<String>, _abort: Option<crate::agent::types::AbortSignal>) -> BoxFuture<'a, Result<String, crate::agent::harness::types::FileError>> {
                Box::pin(async move { Ok(parts.join("/")) })
            }

            fn read_text_file<'a>(&'a $self, _path: String, _abort: Option<crate::agent::types::AbortSignal>) -> BoxFuture<'a, Result<String, crate::agent::harness::types::FileError>> {
                Box::pin(async move { Ok(String::new()) })
            }

            fn read_text_lines<'a>(&'a $self, _path: String, _options: crate::agent::harness::types::ReadTextLinesOptions) -> BoxFuture<'a, Result<Vec<String>, crate::agent::harness::types::FileError>> {
                Box::pin(async move { Ok(Vec::new()) })
            }

            fn read_binary_file<'a>(&'a $self, _path: String, _abort: Option<crate::agent::types::AbortSignal>) -> BoxFuture<'a, Result<Vec<u8>, crate::agent::harness::types::FileError>> {
                Box::pin(async move { Ok(Vec::new()) })
            }

            fn write_file<'a>(&'a $self, _path: String, _content: crate::agent::harness::types::FileContent, _abort: Option<crate::agent::types::AbortSignal>) -> BoxFuture<'a, Result<(), crate::agent::harness::types::FileError>> {
                Box::pin(async move { Ok(()) })
            }

            fn append_file<'a>(&'a $self, _path: String, _content: crate::agent::harness::types::FileContent) -> BoxFuture<'a, Result<(), crate::agent::harness::types::FileError>> {
                Box::pin(async move { Ok(()) })
            }

            fn rename_file<'a>(&'a $self, _source: String, _destination: String, _abort: Option<crate::agent::types::AbortSignal>) -> BoxFuture<'a, Result<(), crate::agent::harness::types::FileError>> {
                Box::pin(async move { Ok(()) })
            }

            fn file_info<'a>(&'a $self, _path: String) -> BoxFuture<'a, Result<crate::agent::harness::types::FileInfo, crate::agent::harness::types::FileError>> {
                Box::pin(async move {
                    Err(crate::agent::harness::types::FileError::new(crate::agent::harness::types::FileErrorCode::NotFound, "missing"))
                })
            }

            fn list_dir<'a>(&'a $self, _path: String, _abort: Option<crate::agent::types::AbortSignal>) -> BoxFuture<'a, Result<Vec<crate::agent::harness::types::FileInfo>, crate::agent::harness::types::FileError>> {
                Box::pin(async move { Ok(Vec::new()) })
            }

            fn canonical_path<'a>(&'a $self, path: String, _abort: Option<crate::agent::types::AbortSignal>) -> BoxFuture<'a, Result<String, crate::agent::harness::types::FileError>> {
                Box::pin(async move { Ok(path) })
            }

            fn exists<'a>(&'a $self, _path: String, _abort: Option<crate::agent::types::AbortSignal>) -> BoxFuture<'a, Result<bool, crate::agent::harness::types::FileError>> {
                Box::pin(async move { Ok(true) })
            }

            fn create_dir<'a>(&'a $self, _path: String, _options: crate::agent::harness::types::CreateDirOptions) -> BoxFuture<'a, Result<(), crate::agent::harness::types::FileError>> {
                Box::pin(async move { Ok(()) })
            }

            fn remove<'a>(&'a $self, _path: String, _options: crate::agent::harness::types::RemoveOptions) -> BoxFuture<'a, Result<(), crate::agent::harness::types::FileError>> {
                Box::pin(async move { Ok(()) })
            }

            fn create_temp_dir<'a>(&'a $self, _prefix: Option<String>, _abort: Option<crate::agent::types::AbortSignal>) -> BoxFuture<'a, Result<String, crate::agent::harness::types::FileError>> {
                Box::pin(async move { Ok("/tmp/null".to_string()) })
            }

            fn create_temp_file<'a>(&'a $self, _options: crate::agent::harness::types::CreateTempFileOptions) -> BoxFuture<'a, Result<String, crate::agent::harness::types::FileError>> {
                Box::pin(async move { Ok("/tmp/null/file".to_string()) })
            }

            fn cleanup<'a>(&'a $self) -> BoxFuture<'a, ()> {
                Box::pin(async move {})
            }
        };
    }

    impl crate::agent::harness::types::FileSystem for NullEnv {
        null_fs_methods!(self);
    }

    impl crate::agent::harness::types::Shell for NullEnv {
        fn exec<'a>(
            &'a self,
            _command: String,
            _options: crate::agent::harness::types::ShellExecOptions,
        ) -> BoxFuture<'a, Result<crate::agent::harness::types::ExecOutcome, crate::agent::harness::types::ExecutionError>> {
            Box::pin(async move {
                Ok(crate::agent::harness::types::ExecOutcome {
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: 0,
                })
            })
        }

        fn cleanup<'a>(&'a self) -> BoxFuture<'a, ()> {
            Box::pin(async move {})
        }
    }

    #[tokio::test]
    async fn serializes_same_path_operations() {
        let env: Arc<dyn ExecutionEnv> = Arc::new(NullEnv);
        let order: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));
        let active = Arc::new(AtomicUsize::new(0));
        let overlaps = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for id in 0..6u32 {
            let env = env.clone();
            let order = order.clone();
            let active = active.clone();
            let overlaps = overlaps.clone();
            handles.push(tokio::spawn(async move {
                with_file_mutation_queue(&env, "/shared.txt", || Box::pin(async move {
                    if active.fetch_add(1, Ordering::SeqCst) > 0 {
                        overlaps.fetch_add(1, Ordering::SeqCst);
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    order.lock().unwrap().push(id);
                    active.fetch_sub(1, Ordering::SeqCst);
                }))
                .await
                .unwrap();
            }));
        }
        for handle in handles {
            handle.await.unwrap();
        }
        assert_eq!(overlaps.load(Ordering::SeqCst), 0, "operations must not overlap");
        assert_eq!(order.lock().unwrap().len(), 6);
    }

    #[tokio::test]
    async fn different_paths_run_concurrently() {
        let env: Arc<dyn ExecutionEnv> = Arc::new(NullEnv);
        let active = Arc::new(AtomicUsize::new(0));
        let overlaps = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for id in 0..4u32 {
            let env = env.clone();
            let active = active.clone();
            let overlaps = overlaps.clone();
            handles.push(tokio::spawn(async move {
                let path = format!("/file-{id}.txt");
                with_file_mutation_queue(&env, &path, || {
                    let active = active.clone();
                    let overlaps = overlaps.clone();
                    Box::pin(async move {
                        if active.fetch_add(1, Ordering::SeqCst) > 0 {
                            overlaps.fetch_add(1, Ordering::SeqCst);
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                        active.fetch_sub(1, Ordering::SeqCst);
                    })
                })
                .await
                .unwrap();
            }));
        }
        for handle in handles {
            handle.await.unwrap();
        }
        assert!(overlaps.load(Ordering::SeqCst) > 0, "different paths should overlap");
    }
}
